//! `cb <text>`: searchable clipboard history. Enter copies the entry back to
//! the clipboard (Wayland offers no safe way to paste into another window
//! without a Shell extension). Tab offers pin, unpin and delete; pinned
//! entries are your snippets and never expire.

mod store;
mod watcher;

pub use watcher::current_text;

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use std::cell::RefCell;
use std::rc::Rc;
use store::{Entry, Store};

pub struct ClipboardProvider {
    cfg: SharedConfig,
    store: Rc<RefCell<Store>>,
}

impl ClipboardProvider {
    pub fn new(cfg: SharedConfig, daemon: bool) -> Self {
        let store = Rc::new(RefCell::new(Store::load()));
        let (enabled, poll_secs) = {
            let c = cfg.borrow();
            (c.clipboard.enabled, c.clipboard.poll_secs)
        };
        if daemon && enabled {
            let store = store.clone();
            let cfg = cfg.clone();
            watcher::start(poll_secs, move |text| {
                let (max_items, max_bytes) = {
                    let c = cfg.borrow();
                    (c.clipboard.max_items, c.clipboard.max_bytes)
                };
                if text.len() > max_bytes || crate::core::secrets::is_secret(&text) {
                    return;
                }
                let mut s = store.borrow_mut();
                if s.push(&text, max_items) {
                    s.save();
                }
            });
        }
        Self { cfg, store }
    }

    fn actions(&self, e: &Entry) -> Vec<Action> {
        let id = e.id();
        let store = self.store.clone();
        let pin = {
            let (store, id, pinned) = (store.clone(), id.clone(), e.pinned);
            Action {
                label: if pinned { "Unpin" } else { "Pin as snippet" }.into(),
                kind: ActionKind::Callback(Rc::new(move || {
                    let mut s = store.borrow_mut();
                    s.set_pinned(&id, !pinned);
                    s.save();
                    Ok(())
                })),
            }
        };
        let delete = Action {
            label: "Delete".into(),
            kind: ActionKind::Callback(Rc::new(move || {
                let mut s = store.borrow_mut();
                s.remove(&id);
                s.save();
                Ok(())
            })),
        };
        vec![
            Action {
                label: "Copy".into(),
                kind: ActionKind::CopyText(e.text.clone()),
            },
            pin,
            delete,
        ]
    }
}

#[async_trait(?Send)]
impl Provider for ClipboardProvider {
    fn id(&self) -> &'static str {
        "clipboard"
    }

    fn prefix(&self) -> Option<String> {
        Some(self.cfg.borrow().verbs.clipboard.clone())
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let store = self.store.borrow();
        let total = store.entries.len() as u32;
        store
            .entries
            .iter()
            .enumerate()
            .filter_map(|(rank, e)| {
                // Recency order when browsing, fuzzy score when searching.
                // Pinned entries get a nudge either way.
                let base = if q.is_empty() {
                    total - rank as u32
                } else {
                    q.score(&e.text)?
                };
                let score = if e.pinned { base + base / 4 } else { base };
                Some(Item {
                    id: e.id(),
                    title: first_line(&e.text, 90),
                    subtitle: Some(describe(e)),
                    icon: Icon::Named(
                        if e.pinned {
                            "starred-symbolic"
                        } else {
                            "edit-paste-symbolic"
                        }
                        .into(),
                    ),
                    score,
                    actions: self.actions(e),
                })
            })
            .collect()
    }
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.chars().count() > max_chars {
        let cut: String = line.chars().take(max_chars - 1).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

fn describe(e: &Entry) -> String {
    let lines = e.text.lines().count();
    let mut parts = Vec::new();
    if e.pinned {
        parts.push("Pinned".to_string());
    }
    if lines > 1 {
        parts.push(format!("{lines} lines"));
    } else {
        parts.push(format!("{} chars", e.text.chars().count()));
    }
    parts.push(ago(store::now().saturating_sub(e.last_seen)));
    parts.join(" · ")
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s => format!("{} d ago", s / 86400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_line_skips_blank_and_truncates() {
        assert_eq!(first_line("\n\n  hello world  \nmore", 90), "hello world");
        assert_eq!(first_line("abcdefghij", 5), "abcd…");
    }

    #[test]
    fn ago_buckets() {
        assert_eq!(ago(5), "just now");
        assert_eq!(ago(120), "2 min ago");
        assert_eq!(ago(7200), "2 h ago");
        assert_eq!(ago(3 * 86400), "3 d ago");
    }
}
