//! Core providers compiled into the binary. External plugins will join via a
//! bridge provider speaking the JSON-lines protocol.

pub mod apps;
pub mod clipboard;
pub mod github;
pub mod keepass;
pub mod projects;
pub mod shell;
pub mod system;

pub use crate::config::SharedConfig;
use crate::core::Provider;

/// Every core provider, in ranking-neutral order. `daemon` is true in the
/// resident process, which is the only one that should run background
/// watchers.
pub fn all(cfg: SharedConfig, daemon: bool) -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(apps::AppsProvider::new()),
        Box::new(projects::ProjectsProvider::new(cfg.clone())),
        Box::new(shell::ShellProvider::new(cfg.clone())),
        Box::new(github::GithubRepos::new(cfg.clone())),
        Box::new(github::GithubPrs::new(cfg.clone())),
        Box::new(clipboard::ClipboardProvider::new(cfg.clone(), daemon)),
        Box::new(keepass::KeepassProvider::new(cfg)),
        Box::new(system::SystemProvider),
    ]
}
