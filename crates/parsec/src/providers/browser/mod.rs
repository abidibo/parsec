//! `h <text>`: your browser's history and bookmarks. Bookmarks also show in
//! plain searches, below apps. Enter opens the page in the browser it came
//! from; Tab offers copying the URL.
//!
//! One browser at a time: the default one, or the one named in the config.
//! Reading copies the database (see `source`), so it happens in the
//! background: queries serve the last copy read, and a refresh starts when
//! the files changed, at most every [`REREAD_SECS`].

mod source;

pub use source::known;
use source::{Entry, Source};

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

/// History pages kept in memory, most recent first.
const HISTORY_LIMIT: usize = 10_000;
/// Rows handed to the engine per query; it shows ten after frecency.
const CANDIDATES: usize = 50;
/// A browser in use rewrites its database constantly; read it this often.
const REREAD_SECS: u64 = 60;
/// How often a query may check whether a refresh is due.
const CHECK_SECS: u64 = 5;

pub struct BrowserProvider {
    cfg: SharedConfig,
    state: Rc<RefCell<State>>,
}

#[derive(Default)]
struct State {
    /// `None` while a refresh borrows it on another thread.
    reader: Option<Reader>,
    entries: Rc<Vec<Entry>>,
    source: Option<Source>,
    app: Option<gio_unix::DesktopAppInfo>,
    loaded: bool,
    checked: Option<Instant>,
}

/// What a refresh needs to decide whether to read again.
#[derive(Default)]
struct Reader {
    source: Option<Source>,
    stamp: Vec<Option<SystemTime>>,
    read_at: Option<Instant>,
}

/// The settings a refresh runs with, owned so they can cross threads.
struct Wanted {
    name: String,
    profile: String,
    path: String,
    default_app: Option<String>,
}

impl Reader {
    /// Resolve the profile and read it if it changed. `Some` = new entries
    /// (empty when no browser was found). Blocking.
    fn refresh(&mut self, w: &Wanted) -> Option<Vec<Entry>> {
        let src = source::resolve(&w.name, &w.profile, &w.path, w.default_app.as_deref());
        if src != self.source {
            self.source = src;
            self.stamp.clear();
            self.read_at = None;
        }
        let src = self.source.as_ref()?;
        let stamp = src.stamp();
        let fresh = self
            .read_at
            .is_some_and(|t| t.elapsed().as_secs() < REREAD_SECS);
        if stamp == self.stamp || fresh {
            return None;
        }
        match source::read(src, HISTORY_LIMIT) {
            Ok(entries) => {
                tracing::debug!(
                    browser = %src.name,
                    profile = %src.profile.display(),
                    count = entries.len(),
                    "browser history read"
                );
                self.stamp = stamp;
                self.read_at = Some(Instant::now());
                Some(entries)
            }
            Err(e) => {
                tracing::warn!("reading {} history: {e:#}", src.name);
                self.read_at = Some(Instant::now());
                None
            }
        }
    }
}

impl BrowserProvider {
    /// `daemon`: read the history now, so the first query has it.
    pub fn new(cfg: SharedConfig, daemon: bool) -> Self {
        let this = Self {
            cfg,
            state: Rc::new(RefCell::new(State {
                reader: Some(Reader::default()),
                ..Default::default()
            })),
        };
        if daemon && this.cfg.borrow().browser.enabled {
            let (state, cfg) = (this.state.clone(), this.cfg.clone());
            glib::spawn_future_local(async move { refresh(&state, &cfg).await });
        }
        this
    }

    fn item(&self, e: &Entry, score: u32, now: u64) -> Item {
        let state = self.state.borrow();
        let short = short_url(&e.url);
        let (subtitle, icon) = match &e.folder {
            Some(folder) if folder.is_empty() => (short, Icon::Named("starred-symbolic".into())),
            Some(folder) => (
                format!("{folder} · {short}"),
                Icon::Named("starred-symbolic".into()),
            ),
            None => {
                let mut sub = short;
                if let Some(t) = e.last {
                    sub.push_str(" · ");
                    sub.push_str(&crate::providers::shell::ago(now.saturating_sub(t)));
                }
                let icon = state
                    .app
                    .as_ref()
                    .and_then(|a| a.icon())
                    .map_or(Icon::Named("web-browser-symbolic".into()), Icon::GIcon);
                (sub, icon)
            }
        };
        let open = match &state.app {
            Some(app) => {
                let (app, url) = (app.clone(), e.url.clone());
                Action {
                    label: format!(
                        "Open in {}",
                        state.source.as_ref().map_or("browser", |s| s.name.as_str())
                    ),
                    kind: ActionKind::Callback(Rc::new(move || {
                        let ctx = gtk::gdk::Display::default().map(|d| d.app_launch_context());
                        app.launch_uris(&[&url], ctx.as_ref())?;
                        Ok(())
                    })),
                }
            }
            None => Action {
                label: "Open".into(),
                kind: ActionKind::OpenUri(e.url.clone()),
            },
        };
        Item {
            // Hashed: the frecency file must not list the pages you visit.
            // Shared by a page's history and bookmark rows.
            id: format!("web:{}", crate::core::secrets::hash(&e.url)),
            title: if e.title.trim().is_empty() {
                e.url.clone()
            } else {
                e.title.clone()
            },
            subtitle: Some(subtitle),
            icon,
            score: score.max(1),
            actions: vec![
                open,
                Action {
                    label: "Copy URL".into(),
                    kind: ActionKind::CopyText(e.url.clone()),
                },
            ],
        }
    }
}

/// Start a refresh unless one ran recently. Awaits it only before the first
/// read, so a slow copy never holds up typing.
async fn ensure(state: &Rc<RefCell<State>>, cfg: &SharedConfig) {
    let (loaded, due) = {
        let s = state.borrow();
        let due = s
            .checked
            .is_none_or(|t| t.elapsed().as_secs() >= CHECK_SECS);
        (s.loaded, due)
    };
    if !due {
        return;
    }
    if loaded {
        let (state, cfg) = (state.clone(), cfg.clone());
        glib::spawn_future_local(async move { refresh(&state, &cfg).await });
    } else {
        refresh(state, cfg).await;
    }
}

async fn refresh(state: &Rc<RefCell<State>>, cfg: &SharedConfig) {
    let Some(mut reader) = state.borrow_mut().reader.take() else {
        return; // another refresh is running
    };
    state.borrow_mut().checked = Some(Instant::now());
    let wanted = {
        let c = cfg.borrow();
        Wanted {
            name: c.browser.name.clone(),
            profile: c.browser.profile.clone(),
            path: c.browser.path.clone(),
            default_app: gio::AppInfo::default_for_uri_scheme("https")
                .and_then(|a| a.id())
                .map(|id| id.to_string()),
        }
    };
    let (reader, read) = gio::spawn_blocking(move || {
        let read = reader.refresh(&wanted);
        (reader, read)
    })
    .await
    .unwrap_or_default();
    let mut s = state.borrow_mut();
    if reader.source != s.source {
        s.app = reader.source.as_ref().and_then(|src| {
            src.desktop_ids
                .iter()
                .find_map(|id| gio_unix::DesktopAppInfo::new(id))
        });
        s.source = reader.source.clone();
        if s.source.is_none() {
            s.entries = Rc::new(Vec::new());
        }
    }
    if let Some(entries) = read {
        s.entries = Rc::new(entries);
    }
    s.reader = Some(reader);
    s.loaded = true;
}

/// `https://github.com/abidibo/parsec?tab=x` → `github.com/abidibo/parsec?tab=x`,
/// shortened to fit a subtitle.
fn short_url(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.strip_prefix("www.").unwrap_or(rest);
    let rest = rest.strip_suffix('/').unwrap_or(rest);
    if rest.chars().count() > 70 {
        rest.chars().take(69).chain(['…']).collect()
    } else {
        rest.to_string()
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait(?Send)]
impl Provider for BrowserProvider {
    fn id(&self) -> &'static str {
        "browser"
    }

    fn title(&self) -> String {
        "Bookmarks".into()
    }

    fn verb_label(&self, _verb: &str) -> String {
        match &self.state.borrow().source {
            Some(src) => format!("{} history", src.name),
            None => "Browser history".into(),
        }
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.browser.clone()]
    }

    fn accepts_unprefixed(&self) -> bool {
        true
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let (enabled, in_search) = {
            let c = self.cfg.borrow();
            (c.browser.enabled, c.browser.bookmarks_in_search)
        };
        let plain = q.verb.is_none();
        if !enabled || (plain && !in_search) {
            return Vec::new();
        }
        ensure(&self.state, &self.cfg).await;
        let entries = self.state.borrow().entries.clone();
        let total = entries.len() as u32;
        let text = q.text.trim();

        let mut scored: Vec<(u32, &Entry)> = entries
            .iter()
            .enumerate()
            .filter_map(|(rank, e)| {
                if plain {
                    // Bookmarks only, by name, kept below apps (which get
                    // half again on a name match).
                    e.folder.as_ref()?;
                    let s = if text.is_empty() {
                        1
                    } else {
                        q.score(&e.title)?
                    };
                    return Some((s / 2, e));
                }
                if text.is_empty() {
                    return Some((total - rank as u32, e));
                }
                let s = q.score_any([e.title.as_str(), short_url(&e.url).as_str()])?;
                // Frequent pages win ties, 10% per visit, capped; bookmarks
                // a quarter more, like pinned clipboard entries.
                let mut s = s + s * e.visits.min(10) / 10;
                if e.folder.is_some() {
                    s += s / 4;
                }
                Some((s, e))
            })
            .collect();
        scored.sort_by_key(|(s, _)| std::cmp::Reverse(*s));
        scored.truncate(CANDIDATES);
        let now = now();
        scored
            .into_iter()
            .map(|(s, e)| self.item(e, s, now))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::short_url;

    #[test]
    fn short_urls() {
        assert_eq!(short_url("https://www.rust-lang.org/"), "rust-lang.org");
        assert_eq!(
            short_url("https://github.com/abidibo/parsec?tab=readme"),
            "github.com/abidibo/parsec?tab=readme"
        );
        assert_eq!(
            short_url(&format!("https://x.io/{}", "a".repeat(100)))
                .chars()
                .count(),
            70
        );
    }
}
