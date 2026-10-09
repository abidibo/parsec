use super::{Item, Matcher};
use async_trait::async_trait;
use nucleo_matcher::pattern::Pattern;

/// What a provider gets asked. `text` has any provider prefix already
/// stripped, so a provider never needs to know how it was invoked.
pub struct Query<'a> {
    pub text: &'a str,
    /// The keyword that routed the query here, for providers with several.
    pub verb: Option<&'a str>,
    pub matcher: &'a Matcher,
    pattern: Pattern,
}

impl<'a> Query<'a> {
    pub fn new(text: &'a str, verb: Option<&'a str>, matcher: &'a Matcher) -> Self {
        Self {
            text,
            verb,
            matcher,
            pattern: matcher.pattern(text),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    pub fn score(&self, haystack: &str) -> Option<u32> {
        self.matcher.score(&self.pattern, haystack)
    }

    pub fn score_any<'b, I>(&self, haystacks: I) -> Option<u32>
    where
        I: IntoIterator<Item = &'b str>,
    {
        self.matcher.score_any(&self.pattern, haystacks)
    }
}

/// A source of results. Core providers implement this directly; external
/// plugins will be wrapped by a bridge provider speaking the JSON protocol.
///
/// Providers run on the GTK main thread and must not block. Anything slow
/// (disk, network, child processes) goes through async GIO or a thread.
#[async_trait(?Send)]
pub trait Provider {
    /// Short stable identifier, also used as the item id namespace.
    fn id(&self) -> &'static str;

    /// Trigger words. `["$"]` means the provider only runs when the query
    /// starts with `$`, and it receives the remainder with `q.verb == "$"`.
    /// Empty means it runs on every query. Owned so they can come from config.
    fn prefixes(&self) -> Vec<String> {
        Vec::new()
    }

    /// A provider with keywords that also wants plain queries (plugins
    /// without keywords live behind a host that has some).
    fn accepts_unprefixed(&self) -> bool {
        false
    }

    /// Produce items for this query. An empty query means "the launcher just
    /// opened": return cheap candidates (the engine keeps those with frecency).
    async fn query(&self, q: &Query<'_>) -> Vec<Item>;

    /// Suggestions for a query nothing matched (e.g. "search the web for…").
    /// Called with the full text, no verb.
    async fn fallback(&self, _q: &Query<'_>) -> Vec<Item> {
        Vec::new()
    }
}
