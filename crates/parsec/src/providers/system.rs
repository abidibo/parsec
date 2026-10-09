//! Parsec's own entries: settings and quit. They talk to the application
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

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if q.is_empty() {
            return Vec::new();
        }
        ENTRIES
            .iter()
            .filter_map(|e| {
                let score = q
                    .score(e.title)
                    .into_iter()
                    .chain(q.score_any(e.keywords.iter().copied()))
                    .max()?;
                Some(Item {
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
            })
            .collect()
    }
}
