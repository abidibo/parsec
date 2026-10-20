//! Updates for Parsec and its plugins. Once a day (and on demand from
//! Settings) it asks GitHub for the latest release, not the tags, so it
//! never announces a version whose binaries are still building, and asks
//! each git plugin's remote for its commit (see `plugins::updates`). What
//! it finds tints the launcher's gear, shows in Settings and under the
//! `parsec` verb, and sends one desktop notification per new finding. Off
//! with `[updates] check = false`. Dev builds never check for Parsec.
//!
//! Updating Parsec runs the install script in the background, unattended
//! and pinned to the version found. It stops this daemon and starts the
//! new one, which confirms with a notification. When the update would need
//! a password or a build, the script exits 3 before changing anything and
//! the update is offered in a terminal instead, as it used to run. Only
//! offered when this binary is the one the script installs; a checkout,
//! `cargo install` or a distro package is left to whoever put it there.

use crate::plugins::{self, updates::Available};
use crate::providers::SharedConfig;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use serde::{Deserialize, Serialize};
use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const LATEST_URL: &str = "https://api.github.com/repos/abidibo/parsec/releases/latest";
pub const RELEASES_URL: &str = "https://github.com/abidibo/parsec/releases";
/// Installs or updates to the latest release.
pub const INSTALL_COMMAND: &str =
    "curl -fsSL https://raw.githubusercontent.com/abidibo/parsec/main/install.sh | bash";
const INSTALL_SCRIPT: &str = "https://raw.githubusercontent.com/abidibo/parsec/main/install.sh";
/// The script's exit code for "needs a password or a build".
const NEEDS_TERMINAL: i32 = 3;
const FIRST_DELAY: u32 = 30;
const EVERY: Duration = Duration::from_secs(24 * 60 * 60);

#[derive(Debug, Clone)]
pub struct Update {
    pub current: String,
    pub latest: String,
}

/// Why updating Parsec in the background didn't happen.
#[derive(Debug, Clone)]
pub enum Failure {
    /// The script would need a password or a build: run it in a terminal.
    NeedsTerminal(String),
    Failed(String),
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NeedsTerminal(why) | Self::Failed(why) => f.write_str(why),
        }
    }
}

thread_local! {
    static AVAILABLE: RefCell<Option<Update>> = const { RefCell::new(None) };
    static PLUGINS: RefCell<Vec<Available>> = const { RefCell::new(Vec::new()) };
    /// Called whenever either list changes; dropped once they return false.
    static WATCHERS: RefCell<Vec<Box<dyn Fn() -> bool>>> = const { RefCell::new(Vec::new()) };
    static INSTALLING: Cell<bool> = const { Cell::new(false) };
}

/// The newer version found by the last check, if any.
pub fn available() -> Option<Update> {
    AVAILABLE.with(|a| a.borrow().clone())
}

/// The plugins with an update, as of the last check.
pub fn plugin_updates() -> Vec<Available> {
    PLUGINS.with(|p| p.borrow().clone())
}

/// Run `f` whenever what's available changes, until it returns false
/// (say, because the widget it updates is gone).
pub fn watch(f: impl Fn() -> bool + 'static) {
    WATCHERS.with(|w| w.borrow_mut().push(Box::new(f)));
}

fn changed() {
    // Taken out while they run, so a watcher may itself call `watch`.
    let watchers = WATCHERS.with(|w| std::mem::take(&mut *w.borrow_mut()));
    let kept: Vec<_> = watchers.into_iter().filter(|f| f()).collect();
    WATCHERS.with(|w| {
        let mut w = w.borrow_mut();
        let added = std::mem::take(&mut *w);
        *w = kept;
        w.extend(added);
    });
}

/// What the last checks learned, so a daemon restart does not hit the
/// network again within a day and each finding is notified once.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    checked_at: u64,
    latest: String,
    notified: String,
    plugins_checked_at: u64,
    /// Each plugin remote's commit, by URL.
    heads: BTreeMap<String, String>,
    /// `id@commit` of the plugin updates last notified.
    plugins_notified: Vec<String>,
    /// Set while the install script runs; the new daemon confirms it.
    updating_to: String,
}

impl Cache {
    fn path() -> PathBuf {
        cache_dir().join("update.json")
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

fn cache_dir() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("parsec")
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

/// A dev build: Parsec itself is never checked.
pub fn is_dev_build() -> bool {
    parse(crate::brand::VERSION).is_none()
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

/// Start the daily checks, and confirm an update that just happened.
/// `on_change` runs on the main thread whenever what's available changes.
pub fn start(cfg: SharedConfig, app: &adw::Application, on_change: impl Fn() + 'static) {
    watch(move || {
        on_change();
        true
    });
    confirm_update(app);
    let app = app.downgrade();
    glib::spawn_future_local(async move {
        glib::timeout_future_seconds(FIRST_DELAY).await;
        loop {
            if cfg.borrow().updates.check {
                match check_parsec(false).await {
                    Ok(Some(update)) => {
                        if let Some(app) = app.upgrade() {
                            notify(&app, &update);
                        }
                    }
                    Ok(None) => {}
                    Err(e) => tracing::warn!("update check failed: {e:#}"),
                }
                let found = check_plugins(false).await;
                if let Some(app) = app.upgrade() {
                    notify_plugins(&app, &found);
                }
            }
            glib::timeout_future(EVERY).await;
        }
    });
}

/// Check both now, skipping the daily cache: the Settings buttons.
pub async fn check_now() -> (anyhow::Result<Option<Update>>, Vec<Available>) {
    let parsec = check_parsec(true).await;
    let plugins = check_plugins(true).await;
    (parsec, plugins)
}

/// Ask GitHub for the latest release, or reuse today's answer unless
/// `force`. `None` when up to date, and always on a dev build.
async fn check_parsec(force: bool) -> anyhow::Result<Option<Update>> {
    let current = crate::brand::VERSION;
    let Some(cur) = parse(current) else {
        return Ok(None);
    };
    let mut cache = Cache::load();
    let fresh = !force
        && !cache.latest.is_empty()
        && now().saturating_sub(cache.checked_at) < EVERY.as_secs();
    let latest = if fresh {
        cache.latest.clone()
    } else {
        let latest = gio::spawn_blocking(fetch_latest)
            .await
            .map_err(|_| anyhow::anyhow!("check thread panicked"))??;
        cache.checked_at = now();
        cache.latest = latest.clone();
        cache.save();
        latest
    };
    let update = parse(&latest).filter(|l| *l > cur).map(|_| Update {
        current: current.to_string(),
        latest,
    });
    match &update {
        Some(u) => tracing::info!(current, latest = %u.latest, "update available"),
        None => tracing::debug!(current, "up to date"),
    }
    AVAILABLE.with(|a| *a.borrow_mut() = update.clone());
    changed();
    Ok(update)
}

/// Ask each git plugin's remote for its commit, or reuse today's answers
/// unless `force`, and work out which plugins have an update.
async fn check_plugins(force: bool) -> Vec<Available> {
    let installed = gio::spawn_blocking(plugins::installed)
        .await
        .unwrap_or_default();
    let mut cache = Cache::load();
    let fresh = !force && now().saturating_sub(cache.plugins_checked_at) < EVERY.as_secs();
    if !fresh {
        let list = installed.clone();
        cache.heads = gio::spawn_blocking(move || {
            let heads = plugins::updates::remote_heads(&list);
            plugins::updates::recognize(&list, &heads);
            heads
        })
        .await
        .unwrap_or_default();
        cache.plugins_checked_at = now();
        cache.save();
    }
    let found = plugins::updates::available(&installed, &cache.heads);
    if !found.is_empty() {
        let ids: Vec<&str> = found.iter().map(|a| a.id.as_str()).collect();
        tracing::info!(?ids, "plugin updates available");
    }
    PLUGINS.with(|p| *p.borrow_mut() = found.clone());
    changed();
    found
}

/// Install a plugin's update: clone it again, which records the new commit.
pub async fn update_plugin(a: &Available) -> anyhow::Result<plugins::Installed> {
    let url = a.origin.url.clone();
    let installed = gio::spawn_blocking(move || plugins::install(&url))
        .await
        .map_err(|_| anyhow::anyhow!("install thread panicked"))??;
    let commit = plugins::updates::origin(&installed).and_then(|o| o.commit);
    let mut cache = Cache::load();
    if let Some(commit) = commit {
        // What we just installed is the newest we know of.
        cache.heads.insert(a.origin.url.clone(), commit);
        cache.save();
    }
    PLUGINS.with(|p| p.borrow_mut().retain(|p| p.id != a.id));
    changed();
    Ok(installed)
}

fn notify(app: &adw::Application, u: &Update) {
    let mut cache = Cache::load();
    if cache.notified == u.latest {
        return;
    }
    cache.notified = u.latest.clone();
    cache.save();
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

/// One notification per new set of plugin updates.
fn notify_plugins(app: &adw::Application, found: &[Available]) {
    let key: Vec<String> = found
        .iter()
        .map(|a| format!("{}@{}", a.id, a.head))
        .collect();
    let mut cache = Cache::load();
    if key.iter().all(|k| cache.plugins_notified.contains(k)) {
        return;
    }
    cache.plugins_notified = key;
    cache.save();
    let names: Vec<&str> = found.iter().map(|a| a.name.as_str()).collect();
    let n = gio::Notification::new(if found.len() == 1 {
        "A plugin update is available"
    } else {
        "Plugin updates are available"
    });
    n.set_body(Some(&format!(
        "{}. See what changed in Settings.",
        names.join(", ")
    )));
    n.set_default_action("app.plugin-updates");
    n.add_button("Review", "app.plugin-updates");
    app.send_notification(Some("plugin-updates"), &n);
}

/// After a background update the new daemon says so, once.
fn confirm_update(app: &adw::Application) {
    let mut cache = Cache::load();
    if cache.updating_to.is_empty() {
        return;
    }
    if cache.updating_to == crate::brand::VERSION {
        let n = gio::Notification::new(&format!("Parsec {} is installed", crate::brand::VERSION));
        n.set_body(Some("Updated in the background."));
        n.add_button("What's new", "app.whats-new");
        app.send_notification(Some("update"), &n);
    }
    cache.updating_to.clear();
    cache.save();
}

/// The release page of the running version.
pub fn release_url() -> String {
    format!("{RELEASES_URL}/tag/{}", crate::brand::VERSION)
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

/// Update in a terminal, as before background updates: the fallback.
pub fn install_in_terminal(cfg: &crate::config::Config) -> anyhow::Result<()> {
    let Some(u) = available() else {
        anyhow::bail!("no update available");
    };
    if !can_install() {
        anyhow::bail!("this parsec is not the one install.sh manages");
    }
    tracing::info!(version = %u.latest, "updating in a terminal");
    crate::core::engine::run_detached(&crate::core::ActionKind::Command(install_command(
        cfg, &u.latest,
    )))
}

/// Update to the version the last check found, in the background. On
/// success the script stops this daemon, so this only returns when it
/// didn't: it needs a terminal, or it failed (see the log it names).
pub async fn install() -> Result<(), Failure> {
    let Some(u) = available() else {
        return Err(Failure::Failed("No update available".into()));
    };
    if !can_install() {
        return Err(Failure::Failed(
            "This Parsec wasn't installed by the install script".into(),
        ));
    }
    if INSTALLING.with(|i| i.replace(true)) {
        return Err(Failure::Failed("Already updating".into()));
    }
    tracing::info!(version = %u.latest, "updating in the background");
    let mut cache = Cache::load();
    cache.updating_to = u.latest.clone();
    cache.save();
    let log = cache_dir().join("update.log");
    let result = match spawn_script(&u.latest, &log) {
        Ok(child) => gio::spawn_blocking(move || {
            let mut child = child;
            child.wait()
        })
        .await
        .map_err(|_| std::io::Error::other("wait thread panicked"))
        .and_then(|r| r),
        Err(e) => Err(e),
    };
    INSTALLING.with(|i| i.set(false));
    let mut cache = Cache::load();
    cache.updating_to.clear();
    cache.save();
    let tail = || last_line(&log).unwrap_or_else(|| format!("see {}", log.display()));
    match result {
        // Ended without replacing us, yet fine: nothing to report.
        Ok(status) if status.success() => Ok(()),
        Ok(status) if status.code() == Some(NEEDS_TERMINAL) => {
            let why = tail();
            let why = why.strip_prefix("needs a terminal: ").unwrap_or(&why);
            Err(Failure::NeedsTerminal(capitalized(why)))
        }
        Ok(_) => Err(Failure::Failed(capitalized(&tail()))),
        Err(e) => Err(Failure::Failed(format!(
            "Couldn't run the install script: {e}"
        ))),
    }
}

/// The install script, unattended, with its output in `log`. It runs in a
/// scope of its own, so it lives on when it stops this daemon.
fn spawn_script(version: &str, log: &std::path::Path) -> std::io::Result<std::process::Child> {
    if let Some(dir) = log.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let out = std::fs::File::create(log)?;
    let child = std::process::Command::new("bash")
        .arg("-c")
        .arg(format!(
            "set -o pipefail; curl -fsSL {INSTALL_SCRIPT} | bash"
        ))
        .env("PARSEC_VERSION", version)
        .env("PARSEC_UNATTENDED", "1")
        .current_dir(dirs::home_dir().unwrap_or_else(|| "/".into()))
        .stdin(std::process::Stdio::null())
        .stdout(out.try_clone()?)
        .stderr(out)
        .spawn()?;
    crate::core::scope::adopt(child.id(), "parsec-update");
    Ok(child)
}

/// The script's last words, without its colours.
fn last_line(log: &std::path::Path) -> Option<String> {
    let text = std::fs::read_to_string(log).ok()?;
    let line = text.lines().rev().find(|l| !l.trim().is_empty())?;
    let mut plain = String::new();
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip an escape sequence up to its final letter.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    let plain = plain.trim();
    Some(plain.strip_prefix("error: ").unwrap_or(plain).to_string())
}

fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{last_line, parse};

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
    fn the_last_log_line_loses_colours_and_prefix() {
        let dir = std::env::temp_dir().join(format!("parsec-update-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("update.log");
        std::fs::write(
            &log,
            "\u{1b}[1;34m==>\u{1b}[0m Downloading\n\u{1b}[1;31merror:\u{1b}[0m The download does not match its checksum.\n\n",
        )
        .unwrap();
        assert_eq!(
            last_line(&log).as_deref(),
            Some("The download does not match its checksum.")
        );
        let _ = std::fs::remove_dir_all(dir);
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
