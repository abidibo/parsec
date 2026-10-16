//! Daily check for a newer release on GitHub. Reads the latest release,
//! not the tags, so it never announces a version whose binaries are still
//! building. Compares with the baked-in version, and if there is something newer tells the
//! launcher (accent gear, entry under the `parsec` verb) and sends one
//! desktop notification per version. Off with `[updates] check = false`.
//! Dev builds (anything but a bare x.y.z) never check.
//!
//! Updating runs the install script in your terminal, pinned to the version
//! found, so you see it work (and can answer sudo if it has to build). Only
//! offered when this binary is the one the script installs; a checkout,
//! `cargo install` or a distro package is left to whoever put it there.

use crate::providers::SharedConfig;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LATEST_URL: &str = "https://api.github.com/repos/abidibo/parsec/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/abidibo/parsec/releases";
/// Installs or updates to the latest release.
pub const INSTALL_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/abidibo/parsec/main/install.sh | bash";
const INSTALL_SCRIPT: &str = "https://raw.githubusercontent.com/abidibo/parsec/main/install.sh";
const FIRST_DELAY: u32 = 30;
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct Update {
    pub current: String,
    pub latest: String,
}

thread_local! {
    static AVAILABLE: RefCell<Option<Update>> = const { RefCell::new(None) };
}

/// The newer version found by the last check, if any.
pub fn available() -> Option<Update> {
    AVAILABLE.with(|a| a.borrow().clone())
}

/// What the last check learned, so a daemon restart does not hit the
/// network again within a day and the notification fires once per version.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Cache {
    checked_at: u64,
    latest: String,
    notified: String,
}

impl Cache {
    fn path() -> PathBuf {
        dirs::cache_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("parsec")
            .join("update.json")
    }
    fn load() -> Self {
        std::fs::read(Self::path())
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }
    fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_vec(self) {
            let _ = std::fs::write(path, json);
        }
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `x.y.z` exactly; `1.0.0-3-gabc` or `1.0.0-dirty` are dev builds.
pub fn parse(v: &str) -> Option<(u64, u64, u64)> {
    let mut it = v.trim().trim_start_matches('v').split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    Some((a, b, c))
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// The latest GitHub release. Blocking; run it off the main loop.
fn fetch_latest() -> anyhow::Result<String> {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(10)))
        .user_agent(format!("parsec/{}", crate::brand::VERSION))
        .build()
        .new_agent();
    let release: Release = agent
        .get(LATEST_URL)
        .header("Accept", "application/vnd.github+json")
        .call()?
        .body_mut()
        .read_json()?;
    let tag = release.tag_name.trim_start_matches('v');
    parse(tag)
        .map(|_| tag.to_string())
        .ok_or_else(|| anyhow::anyhow!("latest release is not a version: {tag}"))
}

/// Start the periodic check. `on_found` runs on the main thread each time
/// a newer version is known, including from the cache right at startup.
pub fn start(cfg: SharedConfig, app: &adw::Application, on_found: impl Fn(&Update) + 'static) {
    let current = crate::brand::VERSION;
    let Some(cur) = parse(current) else {
        tracing::info!(version = current, "dev build, update check off");
        return;
    };
    let app = app.downgrade();
    glib::spawn_future_local(async move {
        glib::timeout_future_seconds(FIRST_DELAY).await;
        loop {
            if cfg.borrow().updates.check {
                let mut cache = Cache::load();
                let fresh = now().saturating_sub(cache.checked_at) < EVERY.as_secs();
                let latest = if fresh && !cache.latest.is_empty() {
                    Ok(cache.latest.clone())
                } else {
                    gio::spawn_blocking(fetch_latest)
                        .await
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("check thread panicked")))
                };
                match latest {
                    Ok(latest) => {
                        if !fresh {
                            cache.checked_at = now();
                            cache.latest = latest.clone();
                            cache.save();
                        }
                        if parse(&latest).is_some_and(|l| l > cur) {
                            tracing::info!(current, latest, "update available");
                            let update = Update {
                                current: current.to_string(),
                                latest: latest.clone(),
                            };
                            AVAILABLE.with(|a| *a.borrow_mut() = Some(update.clone()));
                            on_found(&update);
                            if cache.notified != latest {
                                if let Some(app) = app.upgrade() {
                                    notify(&app, &update);
                                }
                                cache.notified = latest;
                                cache.save();
                            }
                        } else {
                            tracing::debug!(current, latest, "up to date");
                        }
                    }
                    Err(e) => tracing::warn!("update check failed: {e}"),
                }
            }
            glib::timeout_future(EVERY).await;
        }
    });
}

fn notify(app: &adw::Application, u: &Update) {
    let n = gio::Notification::new(&format!("Parsec {} is available", u.latest));
    if can_install() {
        n.set_body(Some(&format!("You have {}.", u.current)));
        n.add_button("Update", "app.update");
    } else {
        n.set_body(Some(&format!(
            "You have {}. Run the install command again to update, or see the releases page.",
            u.current
        )));
    }
    app.send_notification(Some("update"), &n);
}

/// Where the install script puts the binary.
fn installed_path() -> Option<PathBuf> {
    Some(dirs::home_dir()?.join(".local/bin/parsec"))
}

/// True when this process runs the binary the install script manages, so
/// running the script again updates it rather than installing a second one.
pub fn can_install() -> bool {
    let (Ok(exe), Some(installed)) = (std::env::current_exe(), installed_path()) else {
        return false;
    };
    // Replaced on disk since we started: Linux appends " (deleted)".
    let exe = exe
        .to_str()
        .and_then(|s| s.strip_suffix(" (deleted)"))
        .map(PathBuf::from)
        .unwrap_or(exe);
    let canon = |p: &std::path::Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    canon(&exe) == canon(&installed)
}

/// Argv that opens the terminal on the install script for `version`. The
/// script stops this daemon and starts the new one; the terminal stays
/// open until Enter so the outcome can be read.
pub fn install_command(cfg: &crate::config::Config, version: &str) -> Vec<String> {
    let script = format!(
        "curl -fsSL {INSTALL_SCRIPT} | PARSEC_VERSION={version} bash; \
         echo; read -rp 'Press Enter to close. ' _"
    );
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    cfg.terminal_command(&home, &["bash".into(), "-c".into(), script])
}

/// Start updating to the version the last check found. Does nothing when
/// there is none or this binary isn't the script's to replace.
pub fn install(cfg: &crate::config::Config) -> anyhow::Result<()> {
    let Some(u) = available() else {
        anyhow::bail!("no update available");
    };
    if !can_install() {
        anyhow::bail!("this parsec is not the one install.sh manages");
    }
    tracing::info!(version = %u.latest, "updating");
    crate::core::engine::run_detached(&crate::core::ActionKind::Command(install_command(
        cfg, &u.latest,
    )))
}

#[cfg(test)]
mod tests {
    use super::parse;

    /// Hits GitHub: `cargo test -- --ignored fetches_latest_release`.
    #[test]
    #[ignore]
    fn fetches_latest_release() {
        let latest = super::fetch_latest().expect("latest release from GitHub");
        assert!(parse(&latest).is_some(), "not a release tag: {latest}");
    }

    #[test]
    fn install_runs_the_pinned_script_in_the_terminal() {
        let mut cfg = crate::config::Config::default();
        cfg.terminal.command = ["kitty", "--directory={cwd}", "{exec}"]
            .map(String::from)
            .to_vec();
        let argv = super::install_command(&cfg, "1.2.0");
        assert_eq!(argv[0], "kitty");
        assert_eq!(argv[2..4], ["bash".to_string(), "-c".to_string()]);
        let script = &argv[4];
        assert!(script.contains("install.sh | PARSEC_VERSION=1.2.0 bash; echo; read"));
    }

    #[test]
    fn only_bare_releases_parse() {
        assert_eq!(parse("1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse("v1.2.3"), Some((1, 2, 3)));
        assert_eq!(parse("1.0.0-dirty"), None);
        assert_eq!(parse("1.0.0-3-g54791bd"), None);
        assert_eq!(parse("54791bd"), None);
        assert!(parse("1.10.0") > parse("1.9.9"));
    }
}
