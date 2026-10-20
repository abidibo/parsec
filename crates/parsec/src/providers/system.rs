//! Parsec's own entries: settings, quit and update. They talk to the application
//! through GActions, so this provider has no handle on any window.

use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use gtk::prelude::*;
use std::rc::Rc;

pub struct SystemProvider;

struct Entry {
    id: &'static str,
    title: &'static str,
    subtitle: &'static str,
    keywords: &'static [&'static str],
    icon: &'static str,
    action: &'static str,
    label: &'static str,
}

const ENTRIES: &[Entry] = &[
    Entry {
        id: "system:settings",
        title: "Parsec Settings",
        subtitle: "Editor, terminal, project folders, verbs",
        keywords: &["settings", "preferences", "config", "parsec"],
        icon: "emblem-system-symbolic",
        action: "preferences",
        label: "Open",
    },
    Entry {
        id: "system:quit",
        title: "Quit Parsec",
        subtitle: "Stop the launcher daemon",
        keywords: &["quit", "exit", "parsec"],
        icon: "application-exit-symbolic",
        action: "quit",
        label: "Quit",
    },
];

fn app_action(name: &'static str) -> ActionKind {
    ActionKind::Callback(Rc::new(move || {
        match gio::Application::default() {
            Some(app) => app.activate_action(name, None),
            None => tracing::warn!("no running application for action {name}"),
        }
        Ok(())
    }))
}

#[async_trait(?Send)]
impl Provider for SystemProvider {
    fn id(&self) -> &'static str {
        "system"
    }

    fn title(&self) -> String {
        "Parsec".into()
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if q.is_empty() {
            return Vec::new();
        }
        let update = crate::update::available().and_then(|u| {
            let score = q
                .score(&format!("Update Parsec to {}", u.latest))
                .into_iter()
                .chain(q.score_any(["update", "upgrade", "version", "parsec"]))
                .max()?;
            let installable = crate::update::can_install();
            let mut actions = Vec::new();
            if installable {
                actions.push(Action {
                    label: "Update".into(),
                    kind: app_action("update"),
                });
            }
            actions.extend([
                Action {
                    label: "Copy install command".into(),
                    kind: ActionKind::CopyText(crate::update::INSTALL_COMMAND.into()),
                },
                Action {
                    label: "Releases".into(),
                    kind: ActionKind::OpenUri(crate::update::RELEASES_URL.into()),
                },
            ]);
            Some(Item {
                output: None,
                preview: None,
                body: None,
                id: "system:update".into(),
                title: format!("Update Parsec to {}", u.latest),
                subtitle: Some(if installable {
                    format!(
                        "You have {}. Enter updates in the background; Parsec restarts",
                        u.current
                    )
                } else {
                    format!(
                        "You have {}. Enter copies the install command; run it in a terminal",
                        u.current
                    )
                }),
                icon: Icon::Named("software-update-available-symbolic".into()),
                score,
                actions,
            })
        });
        let plugins = crate::update::plugin_updates();
        let plugin_update = (!plugins.is_empty())
            .then(|| {
                let names: Vec<&str> = plugins.iter().map(|a| a.name.as_str()).collect();
                let score = q
                    .score("Update plugins")
                    .into_iter()
                    .chain(q.score_any(["update", "upgrade", "plugins", "parsec"]))
                    .max()?;
                Some(Item {
                    output: None,
                    preview: None,
                    body: None,
                    id: "system:plugin-updates".into(),
                    title: "Update plugins".into(),
                    subtitle: Some(format!("{}. Enter shows what changed", names.join(", "))),
                    icon: Icon::Named("software-update-available-symbolic".into()),
                    score,
                    actions: vec![Action {
                        label: "Review".into(),
                        kind: app_action("plugin-updates"),
                    }],
                })
            })
            .flatten();
        update
            .into_iter()
            .chain(plugin_update)
            .chain(ENTRIES.iter().filter_map(|e| {
                let score = q
                    .score(e.title)
                    .into_iter()
                    .chain(q.score_any(e.keywords.iter().copied()))
                    .max()?;
                Some(Item {
                    output: None,
                    preview: None,
                    body: None,
                    id: e.id.into(),
                    title: e.title.into(),
                    subtitle: Some(e.subtitle.into()),
                    icon: Icon::Named(e.icon.into()),
                    // A touch below an equally good app-name match.
                    score: score.saturating_sub(score / 10),
                    actions: vec![Action {
                        label: e.label.into(),
                        kind: app_action(e.action),
                    }],
                })
            }))
            .collect()
    }
}
