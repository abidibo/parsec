//! `$ <command>`: run a shell command. Enter opens it in a terminal that
//! stays open; the second action runs it detached. `$` alone, or `$ text`,
//! also lists your command history (zsh, bash, fish, and what Parsec ran),
//! so a past command is one Enter away.
//!
//! No live preview while typing: executing half-typed commands is how you
//! lose a home directory. A preview will need an explicit trigger.

use super::shell_history::{Index, Own};
use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub struct ShellProvider {
    cfg: SharedConfig,
    shell: String,
    home: String,
    /// `None` while a refresh borrows it on another thread.
    index: Rc<RefCell<Option<Index>>>,
    own: Rc<RefCell<Own>>,
}

impl ShellProvider {
    /// `daemon`: parse the history files now, in the background, so the
    /// first `$` query doesn't wait for it.
    pub fn new(cfg: SharedConfig, daemon: bool) -> Self {
        let this = Self {
            cfg,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            home: dirs::home_dir()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
            index: Rc::new(RefCell::new(Some(Index::default()))),
            own: Rc::new(RefCell::new(Own::load())),
        };
        if daemon && this.cfg.borrow().history.enabled {
            let (index, cfg, own) = (this.index.clone(), this.cfg.clone(), this.own.clone());
            glib::spawn_future_local(async move {
                Self::refresh_shared(&index, &cfg, &own).await;
            });
        }
        this
    }

    /// Re-read history files whose mtime moved, off the main thread.
    async fn refresh(&self) {
        Self::refresh_shared(&self.index, &self.cfg, &self.own).await;
    }

    async fn refresh_shared(
        slot: &Rc<RefCell<Option<Index>>>,
        cfg: &SharedConfig,
        own: &Rc<RefCell<Own>>,
    ) {
        let Some(mut index) = slot.borrow_mut().take() else {
            return; // another refresh is running; it will serve the result
        };
        let files = cfg.borrow().history.files.clone();
        let own = own.borrow().clone();
        let index = gio::spawn_blocking(move || {
            index.refresh(&files, &own);
            index
        })
        .await
        .unwrap_or_default();
        *slot.borrow_mut() = Some(index);
    }

    fn actions(&self, cmd: &str) -> Vec<Action> {
        // Run the command, then hand over to an interactive shell so the
        // output stays on screen.
        let exec = vec![
            self.shell.clone(),
            "-ic".into(),
            format!("{cmd}; exec {}", self.shell),
        ];
        let in_terminal = self.cfg.borrow().terminal_command(&self.home, &exec);
        let background = vec![self.shell.clone(), "-c".into(), cmd.to_string()];
        vec![
            Action {
                label: "Run in terminal".into(),
                kind: self.recorded(cmd, ActionKind::Command(in_terminal)),
            },
            Action {
                label: "Run in background".into(),
                kind: self.recorded(cmd, ActionKind::Command(background)),
            },
            Action {
                label: "Copy".into(),
                kind: ActionKind::CopyText(cmd.to_string()),
            },
        ]
    }

    /// Wrap a run so it lands in Parsec's own history first.
    fn recorded(&self, cmd: &str, kind: ActionKind) -> ActionKind {
        let own = self.own.clone();
        let cmd = cmd.to_string();
        ActionKind::Callback(Rc::new(move || {
            own.borrow_mut().record(&cmd);
            crate::core::engine::run_detached(&kind)
        }))
    }
}

fn ago(secs: u64) -> String {
    match secs {
        s if s < 60 => "just now".into(),
        s if s < 3600 => format!("{} min ago", s / 60),
        s if s < 86400 => format!("{} h ago", s / 3600),
        s if s < 30 * 86400 => format!("{} d ago", s / 86400),
        s => format!("{} months ago", s / (30 * 86400)),
    }
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[async_trait(?Send)]
impl Provider for ShellProvider {
    fn id(&self) -> &'static str {
        "shell"
    }

    fn title(&self) -> String {
        "Shell".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.shell.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let cmd = q.text.trim();
        let mut items = Vec::new();
        if !cmd.is_empty() {
            items.push(Item {
                // Stable id so frecency learns that you use the shell verb,
                // not every distinct command line.
                id: "shell:run".into(),
                title: cmd.to_string(),
                subtitle: Some("Run in terminal".into()),
                icon: Icon::Named("utilities-terminal-symbolic".into()),
                score: 1000,
                actions: self.actions(cmd),
            });
        }
        if !self.cfg.borrow().history.enabled {
            return items;
        }
        self.refresh().await;
        let index = self.index.borrow();
        let Some(index) = index.as_ref() else {
            return items;
        };
        let total = index.entries.len() as u32;
        let now = now();
        items.extend(index.entries.iter().enumerate().filter_map(|(rank, e)| {
            if e.cmd == cmd {
                return None; // already the typed row
            }
            let score = if cmd.is_empty() {
                total.saturating_sub(rank as u32)
            } else {
                // Frequent commands win ties; 10% per repeat, capped.
                let s = q.score(&e.cmd)?;
                s + s * e.count.min(10) / 10
            };
            let mut sub = Vec::new();
            if let Some(t) = e.last {
                sub.push(ago(now.saturating_sub(t)));
            }
            if e.count > 1 {
                sub.push(format!("{}×", e.count));
            }
            Some(Item {
                // Hashed: the frecency file must not list command lines.
                id: format!("shell:hist:{}", crate::core::secrets::hash(&e.cmd)),
                title: e.cmd.lines().next().unwrap_or("").to_string(),
                subtitle: Some(if sub.is_empty() {
                    "History".into()
                } else {
                    format!("History · {}", sub.join(" · "))
                }),
                icon: Icon::Named("document-open-recent-symbolic".into()),
                score,
                actions: self.actions(&e.cmd),
            })
        }));
        items
    }
}
