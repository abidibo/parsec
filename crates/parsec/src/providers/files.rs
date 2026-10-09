//! `f <name>`: files by name. Candidates come from GNOME's Tracker index
//! (Documents, Downloads, Desktop, media: fresh within seconds) and from
//! plocate (everything, refreshed nightly), merged, filtered by the
//! configured roots and exclusions, then fuzzy-ranked on the file name with
//! a recency boost. Also offered as a fallback when nothing else matched.

use super::SharedConfig;
use crate::config;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use gtk::prelude::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const CANDIDATES_PER_SOURCE: usize = 800;
const RESULTS: usize = 15;

pub struct FilesProvider {
    cfg: SharedConfig,
    tracker: bool,
    plocate: bool,
}

impl FilesProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        let tracker = crate::detect::which("tracker3").is_some();
        let plocate = crate::detect::which("plocate").is_some();
        tracing::debug!(tracker, plocate, "file sources");
        Self {
            cfg,
            tracker,
            plocate,
        }
    }
}

/// The words of the query, lowercased: every one must appear (Tracker: in
/// the file name; plocate: anywhere in the path). Fuzzy ranking on the full
/// query happens afterwards.
fn seed(text: &str) -> Option<Vec<String>> {
    let words: Vec<String> = text.split_whitespace().map(|w| w.to_lowercase()).collect();
    (!words.is_empty()).then_some(words)
}

fn tracker_candidates(words: &[String]) -> Vec<PathBuf> {
    let filters: Vec<String> = words
        .iter()
        .map(|w| {
            let escaped = w.replace('\\', "\\\\").replace('\'', "\\'");
            format!("CONTAINS(LCASE(?name), '{escaped}')")
        })
        .collect();
    let sparql = format!(
        "SELECT DISTINCT ?url WHERE {{ ?f a nfo:FileDataObject ; nie:url ?url ; \
         nfo:fileName ?name ; nfo:fileLastModified ?mod . \
         FILTER({}) }} ORDER BY DESC(?mod) LIMIT {CANDIDATES_PER_SOURCE}",
        filters.join(" && ")
    );
    let out = Command::new("tracker3")
        .args([
            "sparql",
            "--dbus-service=org.freedesktop.Tracker3.Miner.Files",
            "-q",
            &sparql,
        ])
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            l.starts_with("file://")
                .then(|| gtk::glib::filename_from_uri(l).ok().map(|(p, _)| p))
                .flatten()
        })
        .collect()
}

fn plocate_candidates(words: &[String]) -> Vec<PathBuf> {
    // Several patterns are ANDed by plocate.
    let out = Command::new("plocate")
        .args(["-i", "-l", &CANDIDATES_PER_SOURCE.to_string(), "--"])
        .args(words)
        .output();
    let Ok(out) = out else { return Vec::new() };
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(PathBuf::from)
        .collect()
}

fn allowed(path: &Path, roots: &[PathBuf], excludes: &[String], hidden: bool) -> bool {
    if !roots.is_empty() && !roots.iter().any(|r| path.starts_with(r)) {
        return false;
    }
    let rel_components: Vec<String> = path
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    // Hidden entries below the home directory, unless wanted.
    if !hidden {
        let home = dirs::home_dir().unwrap_or_default();
        let below_home = path.strip_prefix(&home).ok();
        if let Some(rel) = below_home {
            if rel
                .components()
                .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
            {
                return false;
            }
        }
    }
    !rel_components
        .iter()
        .any(|c| excludes.iter().any(|e| e == c))
}

fn mtime(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 3600 => "just now".into(),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s if s < 30 * 86400 => format!("{} d ago", s / 86400),
        s if s < 365 * 86400 => format!("{} mo ago", s / (30 * 86400)),
        s => format!("{} y ago", s / (365 * 86400)),
    }
}

/// Recency multiplier: today 1.5x, this week 1.3x, this month 1.15x.
fn recency_boost(age: u64) -> f64 {
    match age {
        a if a < 86400 => 1.5,
        a if a < 7 * 86400 => 1.3,
        a if a < 30 * 86400 => 1.15,
        _ => 1.0,
    }
}

fn icon_for(path: &Path) -> Icon {
    let file = gio::File::for_path(path);
    let info = file.query_info(
        "standard::icon,thumbnail::path,thumbnail::is-valid",
        gio::FileQueryInfoFlags::NONE,
        gio::Cancellable::NONE,
    );
    match info {
        Ok(info) => {
            if info.boolean("thumbnail::is-valid") {
                if let Some(thumb) = info.attribute_byte_string("thumbnail::path") {
                    return Icon::Path(PathBuf::from(thumb.to_string()));
                }
            }
            info.icon().map(Icon::GIcon).unwrap_or(Icon::None)
        }
        Err(_) => Icon::None,
    }
}

impl FilesProvider {
    fn item(&self, path: &Path, age: u64, score: u32) -> Item {
        let cfg = self.cfg.borrow();
        let is_dir = path.is_dir();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let parent = path.parent().unwrap_or(path);
        let p = path.to_string_lossy().into_owned();
        let dir = if is_dir {
            p.clone()
        } else {
            parent.to_string_lossy().into_owned()
        };
        let mut actions = vec![Action {
            label: if is_dir { "Open folder" } else { "Open" }.into(),
            kind: ActionKind::OpenUri(format!("file://{p}")),
        }];
        if !is_dir {
            actions.push(Action {
                label: "Show in folder".into(),
                kind: ActionKind::OpenUri(format!("file://{}", parent.display())),
            });
        }
        actions.push(Action {
            label: "Open terminal here".into(),
            kind: ActionKind::Command(cfg.terminal_command(&dir, &[])),
        });
        actions.push(Action {
            label: "Open in editor".into(),
            kind: ActionKind::Command(cfg.editor_command(&p)),
        });
        actions.push(Action {
            label: "Copy path".into(),
            kind: ActionKind::CopyText(p.clone()),
        });
        Item {
            id: format!("file:{p}"),
            title: name,
            subtitle: Some(format!(
                "{} · {}",
                config::abbreviate_home(parent),
                ago(age)
            )),
            icon: icon_for(path),
            score,
            actions,
        }
    }

    async fn find(&self, q: &Query<'_>) -> Vec<Item> {
        let Some(seed) = seed(q.text) else {
            return Vec::new();
        };
        let (roots, excludes, hidden, use_tracker, use_plocate) = {
            let c = self.cfg.borrow();
            (
                c.files
                    .roots
                    .iter()
                    .map(|r| config::expand_home(r))
                    .collect::<Vec<_>>(),
                c.files.exclude.clone(),
                c.files.hidden,
                c.files.tracker && self.tracker,
                c.files.plocate && self.plocate,
            )
        };
        let candidates = gio::spawn_blocking(move || {
            let mut seen: HashMap<PathBuf, u64> = HashMap::new();
            let mut all = Vec::new();
            if use_tracker {
                all.extend(tracker_candidates(&seed));
            }
            if use_plocate {
                all.extend(plocate_candidates(&seed));
            }
            for p in all {
                if seen.contains_key(&p) || !allowed(&p, &roots, &excludes, hidden) {
                    continue;
                }
                if !p.exists() {
                    continue;
                }
                seen.insert(p.clone(), mtime(&p));
            }
            seen.into_iter().collect::<Vec<_>>()
        })
        .await
        .unwrap_or_default();

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let mut ranked: Vec<(f64, PathBuf, u64)> = candidates
            .into_iter()
            .filter_map(|(path, modified)| {
                let name = path.file_name()?.to_string_lossy().into_owned();
                let score = q.score(&name)? as f64;
                let age = now.saturating_sub(modified);
                Some((score * recency_boost(age), path, age))
            })
            .collect();
        ranked.sort_by(|a, b| b.0.total_cmp(&a.0));
        ranked.truncate(RESULTS);
        ranked
            .into_iter()
            .map(|(score, path, age)| self.item(&path, age, score as u32))
            .collect()
    }
}

#[async_trait(?Send)]
impl Provider for FilesProvider {
    fn id(&self) -> &'static str {
        "files"
    }

    fn title(&self) -> String {
        "Files".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.files.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if q.is_empty() || (!self.tracker && !self.plocate) {
            return Vec::new();
        }
        self.find(q).await
    }

    async fn fallback(&self, q: &Query<'_>) -> Vec<Item> {
        if !self.tracker && !self.plocate {
            return Vec::new();
        }
        // Cheap enough to just run; the query is short-lived either way.
        let mut items = self.find(q).await;
        items.truncate(5);
        for i in &mut items {
            i.score = 1;
        }
        items
    }
}

#[allow(dead_code)]
const _: Duration = Duration::from_secs(0);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_is_every_word_lowercased() {
        assert_eq!(
            seed("Rep q3 Summary"),
            Some(vec!["rep".into(), "q3".into(), "summary".into()])
        );
        assert_eq!(seed("  "), None);
    }

    #[test]
    fn exclusions_and_hidden() {
        let home = dirs::home_dir().unwrap();
        let ex = vec!["node_modules".to_string()];
        assert!(!allowed(&home.join("x/node_modules/y.js"), &[], &ex, false));
        assert!(!allowed(&home.join(".cache/y"), &[], &ex, false));
        assert!(allowed(&home.join(".cache/y"), &[], &ex, true));
        assert!(allowed(
            &home.join("Documents/a.pdf"),
            &[home.clone()],
            &ex,
            false
        ));
        assert!(!allowed(
            Path::new("/usr/share/x"),
            &[home.clone()],
            &ex,
            false
        ));
    }
}
