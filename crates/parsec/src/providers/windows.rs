//! Open windows, through the GNOME Shell extension. Mixed into plain
//! queries, so typing an app's name offers both its running windows and a
//! fresh launch; `win` on its own lists everything, most recent first.
//! Silent when the extension is not available.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use crate::gnome_shell::{bridge, WindowInfo};
use async_trait::async_trait;
use gtk::prelude::*;
use std::rc::Rc;

pub struct WindowsProvider {
    cfg: SharedConfig,
}

impl WindowsProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self { cfg }
    }
}

fn icon_for(w: &WindowInfo) -> Icon {
    if !w.app_id.is_empty() {
        if let Some(icon) = gio_unix::DesktopAppInfo::new(&w.app_id).and_then(|i| i.icon()) {
            return Icon::GIcon(icon);
        }
    }
    Icon::Named("preferences-system-windows-symbolic".into())
}

fn subtitle(w: &WindowInfo) -> String {
    let mut parts = Vec::new();
    if !w.app_name.is_empty() {
        parts.push(w.app_name.clone());
    } else if !w.wm_class.is_empty() {
        parts.push(w.wm_class.clone());
    }
    if w.workspace >= 0 {
        parts.push(format!("Workspace {}", w.workspace + 1));
    }
    if w.minimized {
        parts.push("Minimized".into());
    }
    parts.join(" · ")
}

fn actions(w: &WindowInfo) -> Vec<Action> {
    let id = w.id;
    vec![
        Action {
            label: "Switch to".into(),
            kind: ActionKind::Callback(Rc::new(move || bridge().activate_window(id))),
        },
        Action {
            label: "Close window".into(),
            kind: ActionKind::Callback(Rc::new(move || bridge().close_window(id))),
        },
    ]
}

#[async_trait(?Send)]
impl Provider for WindowsProvider {
    fn id(&self) -> &'static str {
        "windows"
    }

    fn title(&self) -> String {
        "Windows".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.windows.clone()]
    }

    fn accepts_unprefixed(&self) -> bool {
        true
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if !bridge().is_active() {
            return Vec::new();
        }
        // Plain queries get windows only when there is something to match:
        // the welcome list is for things picked before, not every window.
        if q.verb.is_none() && q.is_empty() {
            return Vec::new();
        }
        let mut windows = bridge().windows().await;
        // The Shell lists bottom to top; the focused window is the one you
        // just left, so the one under it is the most useful first pick.
        windows.reverse();
        let total = windows.len() as u32;
        windows
            .into_iter()
            .enumerate()
            .filter_map(|(rank, w)| {
                let score = if q.is_empty() {
                    total - rank as u32
                } else {
                    let fields = [w.title.as_str(), w.app_name.as_str(), w.wm_class.as_str()];
                    q.score_any(fields)?
                };
                let key = if w.app_id.is_empty() {
                    w.wm_class.clone()
                } else {
                    w.app_id.clone()
                };
                Some(Item {
                    id: format!("win:{key}"),
                    title: w.title.clone(),
                    subtitle: Some(subtitle(&w)),
                    icon: icon_for(&w),
                    score: score.max(1),
                    actions: actions(&w),
                })
            })
            .collect()
    }
}
