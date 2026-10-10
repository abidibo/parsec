//! The optional GNOME Shell extension and the bridge to it.
//!
//! A Wayland client cannot list or focus other windows, type into them, or
//! hear about clipboard changes on GNOME before 48. The extension in
//! `data/extension` runs inside the Shell and exports those four things on
//! the session bus as `org.abidibo.Parsec.Shell`; this module is the client.
//!
//! Everything here degrades to "not available": with the extension absent,
//! disabled in the config, or not yet loaded (a fresh install needs a logout
//! on Wayland), `is_active()` is false and callers fall back to what Parsec
//! did before: copy instead of paste, no window results, polling for the
//! clipboard.
//!
//! The bridge is a per-process singleton behind `bridge()`, because the
//! D-Bus callbacks GIO hands us must be `Send` and cannot capture our
//! main-thread state; they reach it through the thread-local instead.

use anyhow::{Context, Result};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

pub const UUID: &str = "parsec@abidibo.org";
const BUS_NAME: &str = "org.abidibo.Parsec.Shell";
const OBJECT_PATH: &str = "/org/abidibo/Parsec/Shell";
const INTERFACE: &str = "org.abidibo.Parsec.Shell";

const EXT_METADATA: &str = include_str!("../../../data/extension/parsec@abidibo.org/metadata.json");
const EXT_JS: &str = include_str!("../../../data/extension/parsec@abidibo.org/extension.js");

type ClipboardListener = Rc<dyn Fn(String)>;
type StateListener = Rc<dyn Fn(bool)>;

thread_local! {
    static BRIDGE: Rc<Bridge> = Rc::new(Bridge::default());
}

/// The process-wide bridge. Call `connect()` once from the main loop.
pub fn bridge() -> Rc<Bridge> {
    BRIDGE.with(Rc::clone)
}

/// One open window as the Shell reports it.
#[derive(Debug, Clone)]
pub struct WindowInfo {
    pub id: u64,
    pub title: String,
    pub wm_class: String,
    /// Desktop file id such as `firefox.desktop`, empty when unknown.
    pub app_id: String,
    pub app_name: String,
    pub workspace: i32,
    pub minimized: bool,
}

#[derive(Default)]
pub struct Bridge {
    proxy: RefCell<Option<gio::DBusProxy>>,
    /// The first connection attempt finished, one way or the other.
    resolved: Cell<bool>,
    /// The config switch. Off means "act as if the extension were absent".
    enabled: Cell<bool>,
    on_clipboard: RefCell<Vec<ClipboardListener>>,
    on_state: RefCell<Vec<StateListener>>,
}

impl Bridge {
    /// Create the proxy and start following the name. Idempotent.
    pub async fn connect(&self) {
        if self.proxy.borrow().is_some() {
            return;
        }
        let result = gio::DBusProxy::for_bus_future(
            gio::BusType::Session,
            gio::DBusProxyFlags::DO_NOT_AUTO_START,
            None,
            BUS_NAME,
            OBJECT_PATH,
            INTERFACE,
        )
        .await;
        match result {
            Ok(proxy) => {
                proxy.connect_g_signal(|_, _, name, params| {
                    if name == "ClipboardChanged" {
                        if let Some(text) = params.child_value(0).get::<String>() {
                            let b = bridge();
                            let listeners = b.on_clipboard.borrow().clone();
                            for l in listeners {
                                l(text.clone());
                            }
                        }
                    }
                });
                proxy.connect_g_name_owner_notify(|_| bridge().state_changed());
                *self.proxy.borrow_mut() = Some(proxy);
            }
            Err(e) => tracing::warn!("shell extension bridge unavailable: {e}"),
        }
        self.resolved.set(true);
        self.state_changed();
    }

    pub fn set_enabled(&self, on: bool) {
        if self.enabled.replace(on) != on {
            self.state_changed();
        }
    }

    /// True when the extension is loaded in the Shell and not switched off.
    pub fn is_active(&self) -> bool {
        self.enabled.get()
            && self
                .proxy
                .borrow()
                .as_ref()
                .is_some_and(|p| p.name_owner().is_some())
    }

    /// True when the extension answers on the bus, regardless of the switch.
    pub fn is_running(&self) -> bool {
        self.proxy
            .borrow()
            .as_ref()
            .is_some_and(|p| p.name_owner().is_some())
    }

    /// Version the running extension reports.
    pub fn running_version(&self) -> Option<String> {
        self.proxy
            .borrow()
            .as_ref()?
            .cached_property("Version")?
            .get::<String>()
    }

    /// Run `f` with every clipboard text the Shell reports.
    pub fn on_clipboard(&self, f: impl Fn(String) + 'static) {
        self.on_clipboard.borrow_mut().push(Rc::new(f));
    }

    /// Run `f(active)` once the first connection attempt has resolved and
    /// again whenever the extension appears, disappears or is switched.
    pub fn on_state(&self, f: impl Fn(bool) + 'static) {
        let f: StateListener = Rc::new(f);
        if self.resolved.get() {
            f(self.is_active());
        }
        self.on_state.borrow_mut().push(f);
    }

    fn state_changed(&self) {
        let active = self.is_active();
        if self.is_running() {
            tracing::info!(
                active,
                version = self.running_version().unwrap_or_default(),
                "shell extension connected"
            );
        } else {
            tracing::debug!("shell extension not running");
        }
        let listeners = self.on_state.borrow().clone();
        for l in listeners {
            l(active);
        }
    }

    fn proxy(&self) -> Result<gio::DBusProxy> {
        if !self.is_active() {
            anyhow::bail!("shell extension not available");
        }
        self.proxy.borrow().clone().context("no proxy")
    }

    pub async fn windows(&self) -> Vec<WindowInfo> {
        let Ok(proxy) = self.proxy() else {
            return Vec::new();
        };
        let reply = proxy
            .call_future("ListWindows", None, gio::DBusCallFlags::NONE, 1000)
            .await;
        let reply = match reply {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("ListWindows failed: {e}");
                return Vec::new();
            }
        };
        let list = reply.child_value(0);
        list.iter()
            .filter_map(|v| v.get::<HashMap<String, glib::Variant>>())
            .map(|m| {
                let s = |k: &str| m.get(k).and_then(|v| v.get::<String>()).unwrap_or_default();
                let b = |k: &str| m.get(k).and_then(|v| v.get::<bool>()).unwrap_or(false);
                WindowInfo {
                    id: m.get("id").and_then(|v| v.get::<u64>()).unwrap_or(0),
                    title: s("title"),
                    wm_class: s("wm_class"),
                    app_id: s("app_id"),
                    app_name: s("app_name"),
                    workspace: m
                        .get("workspace")
                        .and_then(|v| v.get::<i32>())
                        .unwrap_or(-1),
                    minimized: b("minimized"),
                }
            })
            .collect()
    }

    pub fn activate_window(&self, id: u64) -> Result<()> {
        self.fire("ActivateWindow", Some(&(id,).to_variant()))
    }

    pub fn close_window(&self, id: u64) -> Result<()> {
        self.fire("CloseWindow", Some(&(id,).to_variant()))
    }

    /// Put `text` on the clipboard and type the paste shortcut into the
    /// window that has focus once the launcher is hidden.
    pub fn paste(&self, text: &str) -> Result<()> {
        self.fire("Paste", Some(&(text,).to_variant()))
    }

    /// The clipboard's current text, without touching focus.
    pub async fn clipboard_text(&self) -> Option<String> {
        let proxy = self.proxy().ok()?;
        let reply = proxy
            .call_future("GetClipboard", None, gio::DBusCallFlags::NONE, 1000)
            .await
            .ok()?;
        reply.child_value(0).get::<String>()
    }

    /// A method call whose result only matters in the log.
    fn fire(&self, method: &str, params: Option<&glib::Variant>) -> Result<()> {
        let proxy = self.proxy()?;
        let method = method.to_string();
        let params = params.cloned();
        glib::spawn_future_local(async move {
            if let Err(e) = proxy
                .call_future(&method, params.as_ref(), gio::DBusCallFlags::NONE, 1000)
                .await
            {
                tracing::warn!("{method} failed: {e}");
            }
        });
        Ok(())
    }
}

// ---------------------------------------------------------------- install

/// Where GNOME looks for user extensions.
pub fn dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("gnome-shell")
        .join("extensions")
        .join(UUID)
}

/// The version of the extension built into this binary.
pub fn bundled_version() -> String {
    version_in(EXT_METADATA).unwrap_or_default()
}

fn version_in(metadata: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(metadata).ok()?;
    v.get("version-name")?.as_str().map(str::to_string)
}

/// The version on disk, if installed.
pub fn installed_version() -> Option<String> {
    let text = std::fs::read_to_string(dir().join("metadata.json")).ok()?;
    version_in(&text)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// No GNOME Shell here (or no `org.gnome.shell` schema): nothing to do.
    Unsupported,
    NotInstalled,
    Installed {
        version: String,
        enabled: bool,
        /// Answering on the bus right now.
        running: bool,
        /// The files on disk are older than the bundled ones.
        outdated: bool,
    },
}

impl Status {
    /// One line for the settings row and the CLI.
    pub fn describe(&self) -> String {
        match self {
            Status::Unsupported => "GNOME Shell not detected".into(),
            Status::NotInstalled => {
                "Not installed. Adds window switching, direct paste and native clipboard tracking."
                    .into()
            }
            Status::Installed {
                version,
                enabled,
                running,
                outdated,
            } => {
                let mut s = if *running {
                    format!("Active, version {version}")
                } else if *enabled {
                    format!("Installed, version {version}. Log out and back in to load it.")
                } else {
                    format!("Installed, version {version}, disabled in GNOME")
                };
                if *outdated {
                    s.push_str(&format!(
                        " Version {} is bundled with this Parsec.",
                        bundled_version()
                    ));
                }
                s
            }
        }
    }
}

fn shell_settings() -> Option<gio::Settings> {
    gio::SettingsSchemaSource::default()?
        .lookup("org.gnome.shell", true)
        .map(|_| gio::Settings::new("org.gnome.shell"))
}

/// Whether the uuid is in GNOME's `enabled-extensions` list.
pub fn is_enabled() -> bool {
    shell_settings().is_some_and(|s| s.strv("enabled-extensions").iter().any(|u| u == UUID))
}

pub fn status() -> Status {
    if shell_settings().is_none() {
        return Status::Unsupported;
    }
    match installed_version() {
        None => Status::NotInstalled,
        Some(version) => Status::Installed {
            outdated: version != bundled_version(),
            running: bridge().is_running(),
            enabled: is_enabled(),
            version,
        },
    }
}

fn set_enabled(on: bool) -> Result<()> {
    let settings = shell_settings().context("org.gnome.shell settings not available")?;
    let mut list: Vec<String> = settings
        .strv("enabled-extensions")
        .iter()
        .map(|s| s.to_string())
        .collect();
    let present = list.iter().any(|u| u == UUID);
    if on && !present {
        list.push(UUID.to_string());
    } else if !on && present {
        list.retain(|u| u != UUID);
    } else {
        return Ok(());
    }
    let refs: Vec<&str> = list.iter().map(String::as_str).collect();
    settings
        .set_strv("enabled-extensions", refs.as_slice())
        .context("writing enabled-extensions")?;
    gio::Settings::sync();
    Ok(())
}

/// Write the bundled extension to the user's extension folder and enable it.
/// Returns true when a logout is needed before it starts working.
pub fn install() -> Result<bool> {
    let dir = dir();
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    std::fs::write(dir.join("metadata.json"), EXT_METADATA).context("writing metadata.json")?;
    std::fs::write(dir.join("extension.js"), EXT_JS).context("writing extension.js")?;
    set_enabled(true)?;
    tracing::info!(path = %dir.display(), "shell extension installed");
    Ok(!bridge().is_running())
}

/// Disable the extension in GNOME and delete its folder.
pub fn remove() -> Result<()> {
    let _ = set_enabled(false);
    let dir = dir();
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("removing {}", dir.display()))?;
    }
    tracing::info!("shell extension removed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_metadata_has_a_version() {
        assert!(!bundled_version().is_empty());
        let v: serde_json::Value = serde_json::from_str(EXT_METADATA).unwrap();
        assert_eq!(v["uuid"], UUID);
    }
}
