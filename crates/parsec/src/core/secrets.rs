//! Texts that must never land in the clipboard history. Remembered as
//! hashes, for a short while, by the secret-copy action; checked by the
//! clipboard watcher before recording anything.

use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::time::{Duration, Instant};

const TTL: Duration = Duration::from_secs(120);

thread_local! {
    static RECENT: RefCell<Vec<(u64, Instant)>> = const { RefCell::new(Vec::new()) };
}

pub fn hash(text: &str) -> u64 {
    let mut h = DefaultHasher::new();
    text.hash(&mut h);
    h.finish()
}

pub fn remember(text: &str) {
    let h = hash(text);
    RECENT.with(|r| {
        let mut r = r.borrow_mut();
        r.retain(|(_, t)| t.elapsed() < TTL);
        r.push((h, Instant::now()));
    });
}

pub fn is_secret(text: &str) -> bool {
    let h = hash(text);
    RECENT.with(|r| r.borrow().iter().any(|(x, t)| *x == h && t.elapsed() < TTL))
}
