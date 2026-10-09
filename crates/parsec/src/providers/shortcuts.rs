//! User-defined keywords: `g something` opens a search URL, or runs a
//! script with the text as argument. Defined in `[[shortcuts]]` and in the
//! settings window. The ulauncher idea, with Parsec's prompt for the bare
//! keyword and a fallback suggestion when nothing else matched.

use super::SharedConfig;
use crate::config::{self, Shortcut};
use crate::core::{Action, ActionKind, Icon, Item, Prompt, Provider, Query};
use async_trait::async_trait;
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use std::rc::Rc;

pub struct ShortcutsProvider {
    cfg: SharedConfig,
}

impl ShortcutsProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self { cfg }
    }
}

fn is_url(command: &str) -> bool {
    let c = command.trim_start();
    c.starts_with("http://") || c.starts_with("https://")
}

/// What Enter does for `shortcut` with `query`.
pub fn action_for(shortcut: &Shortcut, query: &str) -> ActionKind {
    let cmd = shortcut.command.trim();
    if is_url(cmd) {
        let encoded = utf8_percent_encode(query, NON_ALPHANUMERIC).to_string();
        let url = cmd.replace("{query}", &encoded).replace("%s", &encoded);
        ActionKind::OpenUri(url)
    } else {
        // The script sees the text as `$1` and `$PARSEC_QUERY`, and a literal
        // {query} / %s is replaced by it, shell-quoted. Scripts are the
        // user's own, so shell interpretation of the script is intended.
        let quoted = shlex::try_quote(query)
            .map(|q| q.into_owned())
            .unwrap_or_default();
        let script = cmd.replace("{query}", &quoted).replace("%s", &quoted);
        ActionKind::Command(vec![
            "sh".into(),
            "-c".into(),
            format!("export PARSEC_QUERY=\"$1\"; {script}"),
            "parsec-shortcut".into(),
            query.to_string(),
        ])
    }
}

/// Icon for a shortcut: automatic, a theme name, or an image file.
pub fn icon_of(shortcut: &Shortcut) -> Icon {
    icon_for(shortcut)
}

fn icon_for(shortcut: &Shortcut) -> Icon {
    let icon = shortcut.icon.trim();
    if icon.is_empty() {
        return Icon::Named(
            if is_url(&shortcut.command) {
                "web-browser-symbolic"
            } else {
                "utilities-terminal-symbolic"
            }
            .into(),
        );
    }
    let path = config::expand_home(icon);
    if path.is_file() {
        Icon::Path(path)
    } else {
        Icon::Named(icon.to_string())
    }
}

fn verb_label(shortcut: &Shortcut, query: &str) -> String {
    if is_url(&shortcut.command) {
        format!("Search {} for \"{query}\"", shortcut.name)
    } else {
        format!("Run {} with \"{query}\"", shortcut.name)
    }
}

impl ShortcutsProvider {
    fn item(&self, s: &Shortcut, query: &str, score: u32) -> Item {
        let id = format!("shortcut:{}", s.keyword);
        let label = if is_url(&s.command) { "Open" } else { "Run" };
        if query.is_empty() && !s.run_without_args {
            // Bare keyword: ask for the text instead of doing nothing.
            let shortcut = s.clone();
            let prompt = Prompt {
                title: format!("{}…", s.name),
                secret: false,
                restore: format!("{} ", s.keyword),
                submit: Rc::new(move |text: String| {
                    let kind = action_for(&shortcut, &text);
                    Box::pin(async move {
                        crate::core::engine::run_detached(&kind).map_err(|e| e.to_string())
                    })
                }),
            };
            return Item {
                id,
                title: s.name.clone(),
                subtitle: Some("Type what to search, then Enter".into()),
                icon: icon_for(s),
                score,
                actions: vec![Action {
                    label: "Type…".into(),
                    kind: ActionKind::Prompt(prompt),
                }],
            };
        }
        Item {
            id,
            title: if query.is_empty() {
                s.name.clone()
            } else {
                verb_label(s, query)
            },
            subtitle: Some(if query.is_empty() {
                s.keyword.clone()
            } else {
                s.command.trim().chars().take(80).collect()
            }),
            icon: icon_for(s),
            score,
            actions: vec![Action {
                label: label.into(),
                kind: action_for(s, query),
            }],
        }
    }
}

#[async_trait(?Send)]
impl Provider for ShortcutsProvider {
    fn id(&self) -> &'static str {
        "shortcuts"
    }

    fn title(&self) -> String {
        "Shortcuts".into()
    }

    fn verb_label(&self, verb: &str) -> String {
        self.cfg
            .borrow()
            .shortcuts
            .iter()
            .find(|s| s.keyword == verb)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Shortcut".into())
    }

    fn prefixes(&self) -> Vec<String> {
        self.cfg
            .borrow()
            .shortcuts
            .iter()
            .map(|s| s.keyword.clone())
            .filter(|k| !k.trim().is_empty())
            .collect()
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let Some(verb) = q.verb else {
            return Vec::new();
        };
        let cfg = self.cfg.borrow();
        cfg.shortcuts
            .iter()
            .filter(|s| s.keyword == verb)
            .map(|s| self.item(s, q.text.trim(), 1000))
            .collect()
    }

    async fn fallback(&self, q: &Query<'_>) -> Vec<Item> {
        let cfg = self.cfg.borrow();
        cfg.shortcuts
            .iter()
            .filter(|s| s.default_search)
            .map(|s| self.item(s, q.text.trim(), 1))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(command: &str) -> Shortcut {
        Shortcut {
            name: "Test".into(),
            keyword: "t".into(),
            command: command.into(),
            icon: String::new(),
            default_search: false,
            run_without_args: false,
        }
    }

    #[test]
    fn url_gets_encoded_query() {
        match action_for(&sc("https://x.test/?q={query}"), "a b&c") {
            ActionKind::OpenUri(u) => assert_eq!(u, "https://x.test/?q=a%20b%26c"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn percent_s_works_too() {
        match action_for(&sc("https://x.test/%s"), "hi") {
            ActionKind::OpenUri(u) => assert_eq!(u, "https://x.test/hi"),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn script_gets_query_as_argument_and_substitution() {
        match action_for(&sc("notify-send {query}"), "it's here") {
            ActionKind::Command(argv) => {
                assert_eq!(argv[0], "sh");
                assert!(argv[2].contains("notify-send \"it's here\""));
                assert_eq!(argv[4], "it's here");
            }
            other => panic!("{other:?}"),
        }
    }
}
