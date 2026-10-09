//! `$ <command>`: run a shell command. Enter opens it in a terminal that
//! stays open; the second action runs it detached.
//!
//! No live preview while typing: executing half-typed commands is how you
//! lose a home directory. A preview will need an explicit trigger.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;

pub struct ShellProvider {
    cfg: SharedConfig,
    shell: String,
    home: String,
}

impl ShellProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            shell: std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()),
            home: dirs::home_dir()
                .map(|h| h.to_string_lossy().into_owned())
                .unwrap_or_else(|| "/".into()),
        }
    }
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
        if cmd.is_empty() {
            return Vec::new();
        }
        // Run the command, then hand over to an interactive shell so the
        // output stays on screen.
        let exec = vec![
            self.shell.clone(),
            "-ic".into(),
            format!("{cmd}; exec {}", self.shell),
        ];
        let in_terminal = self.cfg.borrow().terminal_command(&self.home, &exec);
        vec![Item {
            // Stable id so frecency learns that you use the shell verb,
            // not every distinct command line.
            id: "shell:run".into(),
            title: cmd.to_string(),
            subtitle: Some("Run in terminal".into()),
            icon: Icon::Named("utilities-terminal-symbolic".into()),
            score: 1000,
            actions: vec![
                Action {
                    label: "Run in terminal".into(),
                    kind: ActionKind::Command(in_terminal),
                },
                Action {
                    label: "Run in background".into(),
                    kind: ActionKind::Command(vec![
                        self.shell.clone(),
                        "-c".into(),
                        cmd.to_string(),
                    ]),
                },
                Action {
                    label: "Copy".into(),
                    kind: ActionKind::CopyText(cmd.to_string()),
                },
            ],
        }]
    }
}
