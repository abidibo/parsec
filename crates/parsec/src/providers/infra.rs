//! Infrastructure at your fingertips: SSH hosts (`ssh`), Docker containers
//! (`dk`) and systemd services (`svc`). Each shells out to the usual tool,
//! caches briefly, and offers the obvious actions in your terminal.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use serde::Deserialize;
use std::cell::RefCell;
use std::process::Command;
use std::time::{Duration, Instant, SystemTime};

fn home() -> String {
    dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into())
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

// ------------------------------------------------------------------ ssh

#[derive(Clone)]
struct SshHost {
    alias: String,
    hostname: String,
    user: String,
}

pub struct SshProvider {
    cfg: SharedConfig,
    hosts: RefCell<Vec<SshHost>>,
    read_at: RefCell<Option<SystemTime>>,
}

impl SshProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            hosts: RefCell::new(Vec::new()),
            read_at: RefCell::new(None),
        }
    }

    fn refresh(&self) {
        let path = dirs::home_dir().unwrap_or_default().join(".ssh/config");
        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if *self.read_at.borrow() == mtime && mtime.is_some() {
            return;
        }
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        *self.hosts.borrow_mut() = parse_ssh_config(&text);
        *self.read_at.borrow_mut() = mtime;
    }
}

fn parse_ssh_config(text: &str) -> Vec<SshHost> {
    let mut hosts: Vec<SshHost> = Vec::new();
    let mut current: Vec<usize> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (k.to_lowercase(), v.trim().trim_start_matches('=').trim()),
            None => continue,
        };
        match key.as_str() {
            "host" => {
                current.clear();
                for alias in value.split_whitespace() {
                    if alias.contains(['*', '?', '!']) {
                        continue;
                    }
                    hosts.push(SshHost {
                        alias: alias.to_string(),
                        hostname: String::new(),
                        user: String::new(),
                    });
                    current.push(hosts.len() - 1);
                }
            }
            "hostname" => {
                for &i in &current {
                    hosts[i].hostname = value.to_string();
                }
            }
            "user" => {
                for &i in &current {
                    hosts[i].user = value.to_string();
                }
            }
            _ => {}
        }
    }
    hosts
}

#[async_trait(?Send)]
impl Provider for SshProvider {
    fn id(&self) -> &'static str {
        "ssh"
    }

    fn title(&self) -> String {
        "SSH".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.ssh.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        self.refresh();
        let cfg = self.cfg.borrow();
        let hosts = self.hosts.borrow();
        let mut items: Vec<Item> = hosts
            .iter()
            .filter_map(|h| {
                let score = if q.is_empty() {
                    1
                } else {
                    q.score_any([h.alias.as_str(), h.hostname.as_str()])?
                };
                let target = if h.user.is_empty() {
                    h.hostname.clone()
                } else {
                    format!("{}@{}", h.user, h.hostname)
                };
                Some(Item {
                    id: format!("ssh:{}", h.alias),
                    title: h.alias.clone(),
                    subtitle: (!target.is_empty()).then_some(target.clone()),
                    icon: Icon::Named("network-server-symbolic".into()),
                    score,
                    actions: vec![
                        Action {
                            label: "Connect".into(),
                            kind: ActionKind::Command(
                                cfg.terminal_command(&home(), &argv(&["ssh", &h.alias])),
                            ),
                        },
                        Action {
                            label: "Copy ssh command".into(),
                            kind: ActionKind::CopyText(format!("ssh {}", h.alias)),
                        },
                    ],
                })
            })
            .collect();
        // Anything typed that isn't a known alias can still be a host.
        if !q.is_empty() && items.is_empty() && !q.text.contains(char::is_whitespace) {
            let target = q.text.trim().to_string();
            items.push(Item {
                id: "ssh:adhoc".into(),
                title: format!("ssh {target}"),
                subtitle: Some("Not in ~/.ssh/config".into()),
                icon: Icon::Named("network-server-symbolic".into()),
                score: 1,
                actions: vec![Action {
                    label: "Connect".into(),
                    kind: ActionKind::Command(
                        cfg.terminal_command(&home(), &argv(&["ssh", &target])),
                    ),
                }],
            });
        }
        items
    }
}

// --------------------------------------------------------------- docker

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Container {
    #[serde(rename = "ID")]
    id: String,
    names: String,
    image: String,
    state: String,
    status: String,
}

struct Cached<T> {
    data: Vec<T>,
    at: Option<Instant>,
}

impl<T> Default for Cached<T> {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            at: None,
        }
    }
}

pub struct DockerProvider {
    cfg: SharedConfig,
    available: bool,
    cache: RefCell<Cached<Container>>,
}

impl DockerProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            available: crate::detect::which("docker").is_some(),
            cache: RefCell::default(),
        }
    }

    async fn containers(&self) -> Vec<Container> {
        if self
            .cache
            .borrow()
            .at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(5))
        {
            return self.cache.borrow().data.clone();
        }
        let list = gio::spawn_blocking(|| {
            let out = Command::new("docker")
                .args(["ps", "-a", "--format", "{{json .}}"])
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            Some(
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .filter_map(|l| serde_json::from_str::<Container>(l).ok())
                    .collect::<Vec<_>>(),
            )
        })
        .await
        .ok()
        .flatten()
        .unwrap_or_default();
        let mut c = self.cache.borrow_mut();
        c.data = list.clone();
        c.at = Some(Instant::now());
        list
    }
}

#[async_trait(?Send)]
impl Provider for DockerProvider {
    fn id(&self) -> &'static str {
        "docker"
    }

    fn title(&self) -> String {
        "Docker".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.docker.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if !self.available {
            return Vec::new();
        }
        let containers = self.containers().await;
        let cfg = self.cfg.borrow();
        containers
            .into_iter()
            .filter_map(|c| {
                let score = if q.is_empty() {
                    if c.state == "running" {
                        2
                    } else {
                        1
                    }
                } else {
                    q.score_any([c.names.as_str(), c.image.as_str()])?
                };
                let running = c.state == "running";
                let term = |args: &[&str]| cfg.terminal_command(&home(), &argv(args));
                let mut actions = Vec::new();
                if running {
                    actions.push(Action {
                        label: "Shell".into(),
                        kind: ActionKind::Command(term(&[
                            "docker",
                            "exec",
                            "-it",
                            &c.id,
                            "sh",
                            "-c",
                            "command -v bash >/dev/null 2>&1 && exec bash || exec sh",
                        ])),
                    });
                }
                actions.push(Action {
                    label: "Logs".into(),
                    kind: ActionKind::Command(term(&[
                        "docker", "logs", "-f", "--tail", "200", &c.id,
                    ])),
                });
                if running {
                    actions.push(Action {
                        label: "Restart".into(),
                        kind: ActionKind::Command(argv(&["docker", "restart", &c.id])),
                    });
                    actions.push(Action {
                        label: "Stop".into(),
                        kind: ActionKind::Command(argv(&["docker", "stop", &c.id])),
                    });
                } else {
                    actions.push(Action {
                        label: "Start".into(),
                        kind: ActionKind::Command(argv(&["docker", "start", &c.id])),
                    });
                }
                actions.push(Action {
                    label: "Copy ID".into(),
                    kind: ActionKind::CopyText(c.id.clone()),
                });
                Some(Item {
                    id: format!("docker:{}", c.names),
                    title: c.names.clone(),
                    subtitle: Some(format!("{} · {}", c.image, c.status)),
                    icon: Icon::Named(
                        if running {
                            "media-playback-start-symbolic"
                        } else {
                            "media-playback-stop-symbolic"
                        }
                        .into(),
                    ),
                    score,
                    actions,
                })
            })
            .collect()
    }
}

// ------------------------------------------------------------- services

#[derive(Debug, Clone, Deserialize)]
struct Unit {
    unit: String,
    active: String,
    sub: String,
    description: String,
    #[serde(default)]
    user: bool,
}

pub struct ServicesProvider {
    cfg: SharedConfig,
    available: bool,
    cache: RefCell<Cached<Unit>>,
}

impl ServicesProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            available: crate::detect::which("systemctl").is_some(),
            cache: RefCell::default(),
        }
    }

    async fn units(&self) -> Vec<Unit> {
        if self
            .cache
            .borrow()
            .at
            .is_some_and(|t| t.elapsed() < Duration::from_secs(10))
        {
            return self.cache.borrow().data.clone();
        }
        let list = gio::spawn_blocking(|| {
            let mut all = Vec::new();
            for user in [false, true] {
                let mut cmd = Command::new("systemctl");
                if user {
                    cmd.arg("--user");
                }
                let out = cmd
                    .args([
                        "list-units",
                        "--type=service",
                        "--all",
                        "--plain",
                        "--no-legend",
                        "--output=json",
                    ])
                    .output();
                if let Ok(out) = out {
                    if let Ok(mut units) = serde_json::from_slice::<Vec<Unit>>(&out.stdout) {
                        for u in &mut units {
                            u.user = user;
                        }
                        all.extend(units);
                    }
                }
            }
            all
        })
        .await
        .unwrap_or_default();
        let mut c = self.cache.borrow_mut();
        c.data = list.clone();
        c.at = Some(Instant::now());
        list
    }
}

#[async_trait(?Send)]
impl Provider for ServicesProvider {
    fn id(&self) -> &'static str {
        "services"
    }

    fn title(&self) -> String {
        "Services".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.services.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if !self.available {
            return Vec::new();
        }
        let units = self.units().await;
        let cfg = self.cfg.borrow();
        units
            .into_iter()
            .filter_map(|u| {
                let name = u.unit.trim_end_matches(".service").to_string();
                let score = if q.is_empty() {
                    return None; // hundreds of units: ask for a name first
                } else {
                    q.score_any([name.as_str(), u.description.as_str()])?
                };
                let running = u.sub == "running";
                let scope = if u.user { "user" } else { "system" };
                let sc: Vec<&str> = if u.user {
                    vec!["systemctl", "--user"]
                } else {
                    vec!["systemctl"]
                };
                let term = |args: &[&str]| cfg.terminal_command(&home(), &argv(args));
                let mut status = sc.clone();
                status.extend(["status", &u.unit]);
                let mut logs = if u.user {
                    vec!["journalctl", "--user"]
                } else {
                    vec!["journalctl"]
                };
                logs.extend(["-u", &u.unit, "-f", "-n", "200"]);
                // Changing system units needs root: run in a terminal via sudo.
                let control = |verb: &str| -> Vec<String> {
                    if u.user {
                        argv(&["systemctl", "--user", verb, &u.unit])
                    } else {
                        term(&["sudo", "systemctl", verb, &u.unit])
                    }
                };
                let mut actions = vec![
                    Action {
                        label: "Status".into(),
                        kind: ActionKind::Command(term(&status)),
                    },
                    Action {
                        label: "Logs".into(),
                        kind: ActionKind::Command(term(&logs)),
                    },
                ];
                if running {
                    actions.push(Action {
                        label: "Restart".into(),
                        kind: ActionKind::Command(control("restart")),
                    });
                    actions.push(Action {
                        label: "Stop".into(),
                        kind: ActionKind::Command(control("stop")),
                    });
                } else {
                    actions.push(Action {
                        label: "Start".into(),
                        kind: ActionKind::Command(control("start")),
                    });
                }
                Some(Item {
                    id: format!("service:{scope}:{}", u.unit),
                    title: name,
                    subtitle: Some(format!(
                        "{} · {} ({}) · {scope}",
                        u.description, u.active, u.sub
                    )),
                    icon: Icon::Named(
                        if running {
                            "emblem-ok-symbolic"
                        } else {
                            "media-playback-stop-symbolic"
                        }
                        .into(),
                    ),
                    score: if running { score + score / 5 } else { score },
                    actions,
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config_parsing() {
        let text = "# c\nHost github.com\n  HostName github.com\n  User git\n\nHost * \n  ForwardAgent yes\nHost web db\n  User root\n";
        let hosts = parse_ssh_config(text);
        let aliases: Vec<&str> = hosts.iter().map(|h| h.alias.as_str()).collect();
        assert_eq!(aliases, ["github.com", "web", "db"]);
        assert_eq!(hosts[0].user, "git");
        assert_eq!(hosts[2].user, "root");
        assert_eq!(hosts[2].hostname, "");
    }
}
