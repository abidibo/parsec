//! Frequency + recency ranking, persisted as JSON. Firefox-style buckets:
//! each recorded pick contributes a weight that decays with age.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_VISITS_PER_ITEM: usize = 10;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Frecency {
    /// item id -> unix timestamps of the most recent picks.
    visits: HashMap<String, Vec<u64>>,
}

impl Frecency {
    pub fn path() -> PathBuf {
        dirs::data_local_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("parsec")
            .join("frecency.json")
    }

    pub fn load() -> Self {
        let path = Self::path();
        match fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_else(|e| {
                tracing::warn!("frecency file unreadable, starting fresh: {e}");
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        match serde_json::to_vec_pretty(self) {
            Ok(bytes) => {
                if let Err(e) = fs::write(&path, bytes) {
                    tracing::warn!("could not save frecency: {e}");
                }
            }
            Err(e) => tracing::warn!("could not serialize frecency: {e}"),
        }
    }

    pub fn record(&mut self, id: &str) {
        let now = now();
        let v = self.visits.entry(id.to_string()).or_default();
        v.push(now);
        if v.len() > MAX_VISITS_PER_ITEM {
            let drop = v.len() - MAX_VISITS_PER_ITEM;
            v.drain(..drop);
        }
    }

    /// Raw frecency: 0 for never picked, up to ~1000 for something picked
    /// ten times in the last few hours.
    pub fn raw(&self, id: &str) -> u32 {
        let now = now();
        self.visits
            .get(id)
            .map(|v| v.iter().map(|&t| weight(now.saturating_sub(t))).sum())
            .unwrap_or(0)
    }

    /// Multiplier in `[1.0, 2.0]` to apply to a match score. Frecency can at
    /// most double a result; it never resurrects a non-match.
    pub fn boost(&self, id: &str) -> f64 {
        let raw = self.raw(id) as f64;
        1.0 + (1.0 + raw).ln() / (1001.0_f64).ln()
    }

    pub fn has(&self, id: &str) -> bool {
        self.visits.get(id).is_some_and(|v| !v.is_empty())
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn weight(age_secs: u64) -> u32 {
    const HOUR: u64 = 3600;
    const DAY: u64 = 24 * HOUR;
    match age_secs {
        a if a < 4 * HOUR => 100,
        a if a < DAY => 80,
        a if a < 3 * DAY => 60,
        a if a < 7 * DAY => 40,
        a if a < 30 * DAY => 20,
        _ => 10,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn never_picked_has_no_boost() {
        let f = Frecency::default();
        assert_eq!(f.raw("x"), 0);
        assert_eq!(f.boost("x"), 1.0);
        assert!(!f.has("x"));
    }

    #[test]
    fn boost_grows_with_picks_and_caps_near_two() {
        let mut f = Frecency::default();
        f.record("x");
        let one = f.boost("x");
        assert!(one > 1.0 && one < 2.0);
        for _ in 0..20 {
            f.record("x");
        }
        let many = f.boost("x");
        assert!(many > one);
        assert!(many <= 2.0 + 1e-9);
        assert_eq!(f.raw("x"), 100 * MAX_VISITS_PER_ITEM as u32);
    }

    #[test]
    fn weights_decay_with_age() {
        assert!(weight(0) > weight(5 * 3600));
        assert!(weight(5 * 3600) > weight(2 * 86400));
        assert_eq!(weight(400 * 86400), 10);
    }
}
