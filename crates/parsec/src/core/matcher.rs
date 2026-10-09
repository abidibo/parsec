//! Thin wrapper over nucleo so providers share one tuned matcher.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Utf32Str};
use std::cell::RefCell;

pub struct Matcher {
    inner: RefCell<nucleo_matcher::Matcher>,
    buf: RefCell<Vec<char>>,
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new()
    }
}

impl Matcher {
    pub fn new() -> Self {
        Self {
            inner: RefCell::new(nucleo_matcher::Matcher::new(Config::DEFAULT)),
            buf: RefCell::new(Vec::new()),
        }
    }

    /// Compile a query once; score many haystacks with it.
    pub fn pattern(&self, query: &str) -> Pattern {
        Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart)
    }

    /// Returns `None` when the haystack does not match at all.
    pub fn score(&self, pattern: &Pattern, haystack: &str) -> Option<u32> {
        let mut buf = self.buf.borrow_mut();
        let mut inner = self.inner.borrow_mut();
        pattern.score(Utf32Str::new(haystack, &mut buf), &mut inner)
    }

    /// Best score across several candidate strings (name, keywords, exe...).
    pub fn score_any<'a, I>(&self, pattern: &Pattern, haystacks: I) -> Option<u32>
    where
        I: IntoIterator<Item = &'a str>,
    {
        haystacks
            .into_iter()
            .filter_map(|h| self.score(pattern, h))
            .max()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_beats_scattered_match() {
        let m = Matcher::new();
        let p = m.pattern("fire");
        let firefox = m.score(&p, "Firefox Web Browser").unwrap();
        let scattered = m.score(&p, "File Roller Extractor").unwrap();
        assert!(firefox > scattered);
    }

    #[test]
    fn non_match_is_none() {
        let m = Matcher::new();
        let p = m.pattern("xyzzy");
        assert!(m.score(&p, "Text Editor").is_none());
    }

    #[test]
    fn score_any_takes_best() {
        let m = Matcher::new();
        let p = m.pattern("term");
        let best = m
            .score_any(&p, ["Console", "gnome-terminal", "Terminal"])
            .unwrap();
        assert_eq!(best, m.score(&p, "Terminal").unwrap());
    }
}
