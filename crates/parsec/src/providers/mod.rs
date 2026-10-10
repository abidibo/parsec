//! Core providers compiled into the binary. External plugins will join via a
//! bridge provider speaking the JSON-lines protocol.

pub mod apps;
pub mod clipboard;
pub mod files;
pub mod github;
pub mod infra;
pub mod keepass;
pub mod plugin_host;
pub mod processes;
pub mod projects;
pub mod shell;
pub mod shell_history;
pub mod shortcuts;
pub mod sys;
pub mod system;
pub mod windows;

pub use crate::config::SharedConfig;
use crate::core::Provider;

/// Every core provider, in ranking-neutral order. `daemon` is true in the
/// resident process, which is the only one that should run background
/// watchers.
pub fn all(cfg: SharedConfig, daemon: bool) -> Vec<Box<dyn Provider>> {
    vec![
        Box::new(apps::AppsProvider::new()),
        Box::new(windows::WindowsProvider::new(cfg.clone())),
        Box::new(projects::ProjectsProvider::new(cfg.clone())),
        Box::new(shell::ShellProvider::new(cfg.clone(), daemon)),
        Box::new(github::GithubRepos::new(cfg.clone())),
        Box::new(github::GithubPrs::new(cfg.clone())),
        Box::new(clipboard::ClipboardProvider::new(cfg.clone(), daemon)),
        Box::new(keepass::KeepassProvider::new(cfg.clone())),
        Box::new(shortcuts::ShortcutsProvider::new(cfg.clone())),
        Box::new(files::FilesProvider::new(cfg.clone())),
        Box::new(infra::SshProvider::new(cfg.clone())),
        Box::new(infra::DockerProvider::new(cfg.clone())),
        Box::new(infra::ServicesProvider::new(cfg.clone())),
        Box::new(sys::SysProvider::new(cfg.clone())),
        Box::new(processes::ProcessesProvider::new(cfg.clone())),
        Box::new(plugin_host::PluginHost::new(cfg)),
        Box::new(system::SystemProvider),
    ]
}
