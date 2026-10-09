//! `gh <repo>` lists your GitHub repositories, `pr` lists your open pull
//! requests. Both shell out to the `gh` CLI off the main thread and cache
//! the answer for a while.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use serde::Deserialize;
use std::cell::RefCell;
use std::process::Command;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Repo {
    name_with_owner: String,
    #[serde(default)]
    description: Option<String>,
    url: String,
    #[serde(default)]
    is_private: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pr {
    number: u64,
    title: String,
    url: String,
    repository: PrRepo,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrRepo {
    name_with_owner: String,
}

struct Cache<T> {
    data: Vec<T>,
    at: Option<Instant>,
}

impl<T> Default for Cache<T> {
    fn default() -> Self {
        Self {
            data: Vec::new(),
            at: None,
        }
    }
}

impl<T> Cache<T> {
    fn fresh(&self, ttl: Duration) -> bool {
        self.at.is_some_and(|t| t.elapsed() < ttl)
    }
}

fn gh_json<T: for<'de> Deserialize<'de>>(args: &[&str]) -> Vec<T> {
    let out = match Command::new("gh").args(args).output() {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!("gh not runnable: {e}");
            return Vec::new();
        }
    };
    if !out.status.success() {
        tracing::warn!(
            "gh {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
        return Vec::new();
    }
    serde_json::from_slice(&out.stdout).unwrap_or_else(|e| {
        tracing::warn!("gh output unparsable: {e}");
        Vec::new()
    })
}

pub struct GithubRepos {
    cfg: SharedConfig,
    cache: RefCell<Cache<Repo>>,
}

impl GithubRepos {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            cache: RefCell::default(),
        }
    }

    async fn repos(&self) -> Vec<Repo> {
        let (ttl, owners) = {
            let c = self.cfg.borrow();
            (
                Duration::from_secs(c.github.cache_secs),
                c.github.owners.clone(),
            )
        };
        if self.cache.borrow().fresh(ttl) {
            return self.cache.borrow().data.clone();
        }
        let repos = gio::spawn_blocking(move || {
            let fields = "nameWithOwner,description,url,isPrivate";
            if owners.is_empty() {
                gh_json::<Repo>(&["repo", "list", "--limit", "200", "--json", fields])
            } else {
                owners
                    .iter()
                    .flat_map(|o| {
                        gh_json::<Repo>(&["repo", "list", o, "--limit", "200", "--json", fields])
                    })
                    .collect()
            }
        })
        .await
        .unwrap_or_default();
        tracing::debug!(count = repos.len(), "fetched github repos");
        let mut c = self.cache.borrow_mut();
        c.data = repos.clone();
        c.at = Some(Instant::now());
        repos
    }
}

#[async_trait(?Send)]
impl Provider for GithubRepos {
    fn id(&self) -> &'static str {
        "github"
    }

    fn title(&self) -> String {
        "GitHub".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.github.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let repos = self.repos().await;
        repos
            .into_iter()
            .filter_map(|r| {
                let score = if q.is_empty() {
                    1
                } else {
                    q.score(&r.name_with_owner)?
                };
                let clone_url = format!("{}.git", r.url);
                Some(Item {
                    id: format!("gh:{}", r.name_with_owner),
                    title: r.name_with_owner.clone(),
                    subtitle: r.description.filter(|d| !d.is_empty()),
                    icon: Icon::Named(if r.is_private {
                        "channel-secure-symbolic".into()
                    } else {
                        "network-server-symbolic".into()
                    }),
                    score,
                    actions: vec![
                        Action {
                            label: "Open on GitHub".into(),
                            kind: ActionKind::OpenUri(r.url.clone()),
                        },
                        Action {
                            label: "Copy clone URL".into(),
                            kind: ActionKind::CopyText(clone_url),
                        },
                    ],
                })
            })
            .collect()
    }
}

pub struct GithubPrs {
    cfg: SharedConfig,
    cache: RefCell<Cache<Pr>>,
}

impl GithubPrs {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            cache: RefCell::default(),
        }
    }

    async fn prs(&self) -> Vec<Pr> {
        let ttl = Duration::from_secs(self.cfg.borrow().github.cache_secs);
        if self.cache.borrow().fresh(ttl) {
            return self.cache.borrow().data.clone();
        }
        let prs = gio::spawn_blocking(|| {
            gh_json::<Pr>(&[
                "search",
                "prs",
                "--author=@me",
                "--state=open",
                "--limit",
                "50",
                "--json",
                "number,title,url,repository",
            ])
        })
        .await
        .unwrap_or_default();
        tracing::debug!(count = prs.len(), "fetched open prs");
        let mut c = self.cache.borrow_mut();
        c.data = prs.clone();
        c.at = Some(Instant::now());
        prs
    }
}

#[async_trait(?Send)]
impl Provider for GithubPrs {
    fn id(&self) -> &'static str {
        "github-prs"
    }

    fn title(&self) -> String {
        "Pull requests".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.prs.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let prs = self.prs().await;
        prs.into_iter()
            .filter_map(|pr| {
                let score = if q.is_empty() {
                    1
                } else {
                    q.score_any([pr.title.as_str(), pr.repository.name_with_owner.as_str()])?
                };
                Some(Item {
                    id: format!("pr:{}", pr.url),
                    title: pr.title.clone(),
                    subtitle: Some(format!("{}#{}", pr.repository.name_with_owner, pr.number)),
                    icon: Icon::Named("object-select-symbolic".into()),
                    score,
                    actions: vec![
                        Action {
                            label: "Open on GitHub".into(),
                            kind: ActionKind::OpenUri(pr.url.clone()),
                        },
                        Action {
                            label: "Copy URL".into(),
                            kind: ActionKind::CopyText(pr.url),
                        },
                    ],
                })
            })
            .collect()
    }
}
