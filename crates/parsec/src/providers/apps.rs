//! Installed desktop applications, via GIO. Reindexes when the app database
//! changes (install/uninstall), so no manual refresh is needed.

use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

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
            _monitor: monitor,
        }
    }
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

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
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
                let subtitle = e
                    .info
                    .generic_name()
                    .map(|g| g.to_string())
                    .or_else(|| e.info.description().map(|d| d.to_string()));
                Some(Item {
                    id: e.id.clone(),
                    title: e.name.clone(),
                    subtitle,
                    icon: e
                        .info
                        .icon()
                        .map(Icon::GIcon)
                        .unwrap_or(Icon::Named("application-x-executable".into())),
                    score,
                    actions: vec![Action {
                        label: "Launch".into(),
                        kind: ActionKind::LaunchApp(e.info.clone()),
                    }],
                })
            })
            .collect()
    }
}
