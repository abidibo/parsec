//! Installed desktop applications, via GIO. Reindexes when the app database
//! changes (install/uninstall), so no manual refresh is needed.
//!
//! With the Shell extension, apps that have windows open say so and get
//! Quit (as the dash does it) and Force quit (SIGKILL) on Tab.

use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use crate::gnome_shell::bridge;
use async_trait::async_trait;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Instant;

/// How long the list of running apps is reused across keystrokes. Only
/// decides which rows offer Quit; Force quit asks again for the pids.
const RUNNING_TTL_MS: u128 = 1000;

type Running = Rc<HashMap<String, Vec<u32>>>;

struct AppEntry {
    info: gio_unix::DesktopAppInfo,
    id: String,
    name: String,
    /// Everything we match against besides the name: generic name, keywords,
    /// executable basename.
    extra: Vec<String>,
}

pub struct AppsProvider {
    entries: Rc<RefCell<Vec<AppEntry>>>,
    running: RefCell<Option<(Instant, Running)>>,
    _monitor: gio::AppInfoMonitor,
}

impl AppsProvider {
    pub fn new() -> Self {
        let entries = Rc::new(RefCell::new(scan()));
        let monitor = gio::AppInfoMonitor::get();
        monitor.connect_changed(glib::clone!(
            #[strong]
            entries,
            move |_| {
                tracing::info!("app database changed, rescanning");
                *entries.borrow_mut() = scan();
            }
        ));
        Self {
            entries,
            running: RefCell::new(None),
            _monitor: monitor,
        }
    }

    /// Desktop ids of apps with open windows, from the Shell extension.
    async fn running(&self) -> Running {
        if let Some((at, apps)) = &*self.running.borrow() {
            if at.elapsed().as_millis() < RUNNING_TTL_MS {
                return apps.clone();
            }
        }
        let apps: Running = if bridge().is_active() {
            Rc::new(bridge().running_apps().await)
        } else {
            Rc::default()
        };
        *self.running.borrow_mut() = Some((Instant::now(), apps.clone()));
        apps
    }
}

fn quit_actions(desktop_id: &str) -> [Action; 2] {
    let quit_id = desktop_id.to_string();
    let kill_id = desktop_id.to_string();
    [
        Action {
            label: "Quit".into(),
            kind: ActionKind::Callback(Rc::new(move || bridge().quit_app(&quit_id))),
        },
        Action {
            label: "Force quit".into(),
            kind: ActionKind::Callback(Rc::new(move || {
                // Pids now, not from the cached list: a pid that is seconds
                // old may belong to something else already.
                let id = kill_id.clone();
                glib::spawn_future_local(async move {
                    let pids: Vec<String> = bridge()
                        .running_apps()
                        .await
                        .remove(&id)
                        .unwrap_or_default()
                        .into_iter()
                        .filter(|p| *p > 1)
                        .map(|p| p.to_string())
                        .collect();
                    if pids.is_empty() {
                        tracing::warn!(app = %id, "force quit: no process found");
                        return;
                    }
                    let mut argv = vec!["kill".to_string(), "-KILL".to_string()];
                    argv.extend(pids);
                    if let Err(e) = crate::core::engine::run_detached(&ActionKind::Command(argv)) {
                        tracing::warn!("force quit {id}: {e:#}");
                    }
                });
                Ok(())
            })),
        },
    ]
}

impl Default for AppsProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn scan() -> Vec<AppEntry> {
    let mut out = Vec::new();
    for info in gio::AppInfo::all() {
        if !info.should_show() {
            continue;
        }
        let Ok(info) = info.downcast::<gio_unix::DesktopAppInfo>() else {
            continue;
        };
        let Some(desktop_id) = info.id() else {
            continue;
        };
        let name = info.name().to_string();
        let mut extra = Vec::new();
        if let Some(generic) = info.generic_name() {
            extra.push(generic.to_string());
        }
        for kw in info.keywords() {
            extra.push(kw.to_string());
        }
        if let Some(exe) = info.executable().file_name() {
            extra.push(exe.to_string_lossy().into_owned());
        }
        out.push(AppEntry {
            id: format!("app:{desktop_id}"),
            info,
            name,
            extra,
        });
    }
    tracing::debug!(count = out.len(), "scanned applications");
    out
}

#[async_trait(?Send)]
impl Provider for AppsProvider {
    fn id(&self) -> &'static str {
        "apps"
    }

    fn title(&self) -> String {
        "Apps".into()
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let running = self.running().await;
        let entries = self.entries.borrow();
        entries
            .iter()
            .filter_map(|e| {
                let score = if q.is_empty() {
                    1
                } else {
                    // Name matches count more than keyword/exe matches.
                    let by_name = q.score(&e.name).map(|s| s + s / 2);
                    let by_extra = q.score_any(e.extra.iter().map(String::as_str));
                    by_name.into_iter().chain(by_extra).max()?
                };
                let desktop_id = &e.id["app:".len()..];
                let open = running.contains_key(desktop_id);
                let about = e
                    .info
                    .generic_name()
                    .map(|g| g.to_string())
                    .or_else(|| e.info.description().map(|d| d.to_string()));
                let subtitle = match (open, about) {
                    (true, Some(a)) => Some(format!("Running · {a}")),
                    (true, None) => Some("Running".into()),
                    (false, a) => a,
                };
                let mut actions = vec![Action {
                    label: "Launch".into(),
                    kind: ActionKind::LaunchApp(e.info.clone()),
                }];
                if open {
                    actions.extend(quit_actions(desktop_id));
                }
                Some(Item {
                    body: None,
                    id: e.id.clone(),
                    title: e.name.clone(),
                    subtitle,
                    icon: e
                        .info
                        .icon()
                        .map(Icon::GIcon)
                        .unwrap_or(Icon::Named("application-x-executable".into())),
                    score,
                    actions,
                })
            })
            .collect()
    }
}
