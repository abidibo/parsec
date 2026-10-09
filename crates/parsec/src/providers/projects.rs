//! Project jumper: every git repository under the configured roots.
//! No prefix, so typing a project name finds it next to apps.

use super::SharedConfig;
use crate::config;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

const RESCAN_AFTER: Duration = Duration::from_secs(120);

#[derive(Clone)]
struct Repo {
    name: String,
    path: PathBuf,
    shown: String,
}

pub struct ProjectsProvider {
    cfg: SharedConfig,
    repos: RefCell<Vec<Repo>>,
    scanned_at: RefCell<Option<Instant>>,
    /// Settings the last scan used, so a config change triggers a rescan.
    scanned_with: RefCell<(Vec<String>, usize)>,
}

impl ProjectsProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            repos: RefCell::new(Vec::new()),
            scanned_at: RefCell::new(None),
            scanned_with: RefCell::new((Vec::new(), 0)),
        }
    }

    async fn ensure_fresh(&self) {
        let (roots, depth) = {
            let c = self.cfg.borrow();
            (c.projects.roots.clone(), c.projects.max_depth)
        };
        let settings_changed = *self.scanned_with.borrow() != (roots.clone(), depth);
        let stale = self
            .scanned_at
            .borrow()
            .is_none_or(|t| t.elapsed() > RESCAN_AFTER);
        if !stale && !settings_changed {
            return;
        }
        let paths: Vec<PathBuf> = roots.iter().map(|r| config::expand_home(r)).collect();
        let found = gio::spawn_blocking(move || scan(&paths, depth))
            .await
            .unwrap_or_default();
        tracing::debug!(count = found.len(), "scanned projects");
        *self.repos.borrow_mut() = found;
        *self.scanned_at.borrow_mut() = Some(Instant::now());
        *self.scanned_with.borrow_mut() = (roots, depth);
    }
}

fn scan(roots: &[PathBuf], max_depth: usize) -> Vec<Repo> {
    let mut out = Vec::new();
    for root in roots {
        walk(root, 1, max_depth, &mut out);
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn walk(dir: &Path, depth: usize, max_depth: usize, out: &mut Vec<Repo>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        if path.join(".git").exists() {
            out.push(Repo {
                shown: config::abbreviate_home(&path),
                name,
                path,
            });
            continue; // don't descend into a repo looking for nested ones
        }
        if depth < max_depth {
            walk(&path, depth + 1, max_depth, out);
        }
    }
}

#[async_trait(?Send)]
impl Provider for ProjectsProvider {
    fn id(&self) -> &'static str {
        "projects"
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        self.ensure_fresh().await;
        let cfg = self.cfg.borrow();
        let repos = self.repos.borrow();
        repos
            .iter()
            .filter_map(|r| {
                let score = if q.is_empty() { 1 } else { q.score(&r.name)? };
                let path = r.path.to_string_lossy();
                Some(Item {
                    id: format!("project:{}", r.path.display()),
                    title: r.name.clone(),
                    subtitle: Some(r.shown.clone()),
                    icon: Icon::Named("folder-symbolic".into()),
                    score,
                    actions: vec![
                        Action {
                            label: "Open in editor".into(),
                            kind: ActionKind::Command(cfg.editor_command(&path)),
                        },
                        Action {
                            label: "Open terminal".into(),
                            kind: ActionKind::Command(cfg.terminal_command(&path, &[])),
                        },
                        Action {
                            label: "Open folder".into(),
                            kind: ActionKind::OpenUri(format!("file://{path}")),
                        },
                        Action {
                            label: "Copy path".into(),
                            kind: ActionKind::CopyText(path.into_owned()),
                        },
                    ],
                })
            })
            .collect()
    }
}
