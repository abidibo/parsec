//! Clipboard history on disk: `~/.local/share/parsec/clipboard.json`,
//! mode 0600. Newest first. Pinned entries are the snippets layer: they
//! never age out.

use serde::{Deserialize, Serialize};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    pub text: String,
    pub last_seen: u64,
    #[serde(default)]
    pub pinned: bool,
}

impl Entry {
    /// Stable id for frecency and for actions targeting this entry.
    pub fn id(&self) -> String {
        let mut h = DefaultHasher::new();
        self.text.hash(&mut h);
        format!("clip:{:016x}", h.finish())
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Store {
    pub entries: Vec<Entry>,
}

impl Store {
    pub fn path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("parsec")
            .join("clipboard.json")
    }

    pub fn load() -> Self {
        match std::fs::read(Self::path()) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("clipboard store unreadable, starting fresh: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(bytes) = serde_json::to_vec(self) else {
            return;
        };
        let tmp = path.with_extension("json.tmp");
        let result = (|| -> std::io::Result<()> {
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            std::io::Write::write_all(&mut f, &bytes)?;
            std::fs::rename(&tmp, &path)
        })();
        if let Err(e) = result {
            tracing::warn!("could not save clipboard store: {e}");
        }
    }

    /// Record a clipboard change. Returns true when something changed.
    pub fn push(&mut self, text: &str, max_items: usize) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        let now = now();
        if let Some(pos) = self.entries.iter().position(|e| e.text == text) {
            let mut e = self.entries.remove(pos);
            e.last_seen = now;
            self.entries.insert(0, e);
        } else {
            self.entries.insert(
                0,
                Entry {
                    text: text.to_string(),
                    last_seen: now,
                    pinned: false,
                },
            );
        }
        self.trim(max_items);
        true
    }

    /// Drop the oldest unpinned entries beyond `max_items`.
    fn trim(&mut self, max_items: usize) {
        let mut unpinned = 0;
        self.entries.retain(|e| {
            if e.pinned {
                return true;
            }
            unpinned += 1;
            unpinned <= max_items
        });
    }

    pub fn set_pinned(&mut self, id: &str, pinned: bool) -> bool {
        match self.entries.iter_mut().find(|e| e.id() == id) {
            Some(e) => {
                e.pinned = pinned;
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id() != id);
        before != self.entries.len()
    }
}

pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_dedupes_and_moves_to_front() {
        let mut s = Store::default();
        s.push("a", 10);
        s.push("b", 10);
        s.push("a", 10);
        let texts: Vec<_> = s.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["a", "b"]);
    }

    #[test]
    fn trim_keeps_pinned() {
        let mut s = Store::default();
        s.push("old", 2);
        let id = s.entries[0].id();
        s.set_pinned(&id, true);
        s.push("1", 2);
        s.push("2", 2);
        s.push("3", 2);
        let texts: Vec<_> = s.entries.iter().map(|e| e.text.as_str()).collect();
        assert_eq!(texts, ["3", "2", "old"]);
    }

    #[test]
    fn blank_is_ignored() {
        let mut s = Store::default();
        assert!(!s.push("   \n", 10));
        assert!(s.entries.is_empty());
    }
}
