//! User configuration, `~/.config/parsec/config.toml`.
//!
//! Every value has a default detected from the environment (see `detect`),
//! and the file is written with comments on first run so people can see what
//! was picked and change it. The daemon reloads it when it changes.
//!
//! Command templates are argv arrays. `{path}` is a project directory,
//! `{cwd}` a terminal's working directory, `{exec}` the command a terminal
//! should run (expands to several argv entries, or vanishes with its
//! separator when there is none).

use crate::detect;
use serde::Deserialize;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;

/// Shared, reloadable configuration handed to providers and the settings UI.
pub type SharedConfig = Rc<RefCell<Config>>;

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub projects: Projects,
    pub editor: Editor,
    pub terminal: Terminal,
    pub github: Github,
    pub clipboard: Clipboard,
    pub keepass: Keepass,
    pub verbs: Verbs,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Keepass {
    /// Path to the .kdbx file. Empty disables the provider's entries.
    pub database: String,
    /// Optional key file combined with the master password.
    pub key_file: String,
    /// Forget the decrypted entries this long after unlocking.
    pub lock_after_secs: u64,
    /// Clear a copied password from the clipboard after this many seconds.
    pub clipboard_clear_secs: u64,
    /// Group names whose entries are hidden (the recycle bin is also found
    /// through the database header; this catches databases that lack it).
    pub skip_groups: Vec<String>,
}

impl Default for Keepass {
    fn default() -> Self {
        Self {
            database: detect::keepass_database().unwrap_or_default(),
            key_file: String::new(),
            lock_after_secs: 600,
            clipboard_clear_secs: 15,
            skip_groups: vec!["Recycle Bin".into(), "Backup".into()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Clipboard {
    /// Watch the clipboard and keep a history.
    pub enabled: bool,
    /// Unpinned entries kept; oldest are dropped first.
    pub max_items: usize,
    /// Entries longer than this (bytes) are not recorded.
    pub max_bytes: usize,
    /// Polling interval when the compositor lacks the data-control protocol
    /// (GNOME before 48).
    pub poll_secs: u64,
}

/// Trigger words. Change them if they clash with how you type.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Verbs {
    pub shell: String,
    pub github: String,
    pub prs: String,
    pub clipboard: String,
    pub keepass: String,
}

impl Default for Clipboard {
    fn default() -> Self {
        Self {
            enabled: true,
            max_items: 200,
            max_bytes: 100 * 1024,
            poll_secs: 1,
        }
    }
}

impl Default for Verbs {
    fn default() -> Self {
        Self {
            shell: "$".into(),
            github: "gh".into(),
            prs: "pr".into(),
            clipboard: "cb".into(),
            keepass: "kp".into(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Projects {
    /// Directories scanned for git repositories. `~` is expanded.
    pub roots: Vec<String>,
    /// How deep below each root to look for a `.git`.
    pub max_depth: usize,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Editor {
    /// Opens `{path}`.
    pub command: Vec<String>,
    /// True for editors that run inside a terminal (nvim, helix...). The
    /// command is then launched through `terminal.command`.
    pub in_terminal: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Terminal {
    /// Opens a terminal at `{cwd}` running `{exec}`.
    pub command: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Github {
    /// Owners whose repositories are listed. Empty means the logged-in user.
    pub owners: Vec<String>,
    /// How long to keep `gh` results before asking again.
    pub cache_secs: u64,
}

impl Default for Projects {
    fn default() -> Self {
        Self {
            roots: detect::project_roots(),
            max_depth: 3,
        }
    }
}

impl Default for Editor {
    fn default() -> Self {
        detect::editor()
    }
}

impl Default for Terminal {
    fn default() -> Self {
        detect::terminal()
    }
}

impl Default for Github {
    fn default() -> Self {
        Self {
            owners: Vec::new(),
            cache_secs: 300,
        }
    }
}

impl Config {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("parsec")
            .join("config.toml")
    }

    /// Load the file, or detect defaults and write them out for next time.
    pub fn load() -> Self {
        let path = Self::path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(cfg) => {
                    tracing::info!(path = %path.display(), "loaded config");
                    cfg
                }
                Err(e) => {
                    tracing::warn!("config file invalid, using defaults: {e}");
                    Self::default()
                }
            },
            Err(_) => {
                let cfg = Self::default();
                cfg.save();
                cfg
            }
        }
    }

    /// Write the file, comments included. Used for the first run and by the
    /// settings window.
    pub fn save(&self) {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        match std::fs::write(&path, self.to_commented_toml()) {
            Ok(()) => tracing::info!(path = %path.display(), "saved config"),
            Err(e) => tracing::warn!("could not save config: {e}"),
        }
    }

    /// The config as a file a human will read, with the detected values
    /// filled in and a comment explaining each setting.
    pub fn to_commented_toml(&self) -> String {
        format!(
            r#"# Parsec configuration. Values below were detected on first run;
# edit freely, the daemon reloads this file when it changes.
# Command templates are argv arrays. Placeholders:
#   {{path}}  project directory        {{cwd}}  terminal working directory
#   {{exec}}  command for the terminal to run (omitted when there is none)

[projects]
# Folders scanned for git repositories, "~" allowed.
roots = {roots}
# How many levels below each root to look.
max_depth = {depth}

[editor]
# "Open in editor" on a project. Detected from $VISUAL / $EDITOR.
command = {editor}
# true for terminal editors (nvim, vim, helix...): the command runs inside
# terminal.command, in the project directory.
in_terminal = {in_terminal}

[terminal]
# Opens a terminal at {{cwd}} running {{exec}}. Also used by the "$ cmd" verb.
# Examples:
#   ["gnome-terminal", "--working-directory={{cwd}}", "--", "{{exec}}"]
#   ["kitty", "--directory={{cwd}}", "{{exec}}"]
#   ["alacritty", "--working-directory", "{{cwd}}", "-e", "{{exec}}"]
command = {terminal}

[github]
# Repository owners for the "gh" verb. Empty = the account `gh` is logged into.
owners = {owners}
# Seconds to cache `gh` output.
cache_secs = {cache}

[clipboard]
# Keep a searchable clipboard history (needs wl-clipboard on Wayland).
enabled = {clip_enabled}
# Unpinned entries to keep.
max_items = {clip_max}
# Skip clipboard contents larger than this many bytes.
max_bytes = {clip_bytes}
# Seconds between checks on compositors without data-control (GNOME < 48).
poll_secs = {clip_poll}

[keepass]
# Path to a KeePass .kdbx database for the "kp" verb. Empty = off.
database = {kp_db}
# Optional key file used together with the master password.
key_file = {kp_key}
# Seconds after unlocking before the entries are forgotten again.
lock_after_secs = {kp_lock}
# Seconds before a copied password is cleared from the clipboard. 0 = never.
clipboard_clear_secs = {kp_clear}
# Entries under groups with these names are hidden, case-insensitive.
skip_groups = {kp_skip}

[verbs]
# Trigger words. A word verb needs a space after it ("gh foo"); a symbol
# verb does not ("$ls").
shell = {v_shell}
github = {v_github}
prs = {v_prs}
clipboard = {v_clip}
keepass = {v_kp}
"#,
            roots = toml_array(&self.projects.roots),
            depth = self.projects.max_depth,
            editor = toml_array(&self.editor.command),
            in_terminal = self.editor.in_terminal,
            terminal = toml_array(&self.terminal.command),
            owners = toml_array(&self.github.owners),
            cache = self.github.cache_secs,
            clip_enabled = self.clipboard.enabled,
            clip_max = self.clipboard.max_items,
            clip_bytes = self.clipboard.max_bytes,
            clip_poll = self.clipboard.poll_secs,
            v_shell = toml_str(&self.verbs.shell),
            v_github = toml_str(&self.verbs.github),
            v_prs = toml_str(&self.verbs.prs),
            v_clip = toml_str(&self.verbs.clipboard),
            kp_db = toml_str(&self.keepass.database),
            kp_key = toml_str(&self.keepass.key_file),
            kp_lock = self.keepass.lock_after_secs,
            kp_clear = self.keepass.clipboard_clear_secs,
            kp_skip = toml_array(&self.keepass.skip_groups),
            v_kp = toml_str(&self.verbs.keepass),
        )
    }

    /// Argv to open `path` in the editor, through the terminal if needed.
    pub fn editor_command(&self, path: &str) -> Vec<String> {
        let editor = render(&self.editor.command, &[("path", path)]);
        if self.editor.in_terminal {
            self.terminal_command(path, &editor)
        } else {
            editor
        }
    }

    /// Argv to open a terminal at `cwd`, running `exec` (empty = a shell).
    pub fn terminal_command(&self, cwd: &str, exec: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for tok in &self.terminal.command {
            if tok == "{exec}" {
                if exec.is_empty() {
                    // Drop a dangling separator: `--`, `-e`, `-x`.
                    if out
                        .last()
                        .is_some_and(|l| matches!(l.as_str(), "--" | "-e" | "-x"))
                    {
                        out.pop();
                    }
                } else {
                    out.extend(exec.iter().cloned());
                }
            } else {
                out.push(tok.replace("{cwd}", cwd));
            }
        }
        out
    }
}

fn toml_str(s: &str) -> String {
    toml::Value::String(s.to_string()).to_string()
}

fn toml_array(items: &[String]) -> String {
    toml::Value::Array(
        items
            .iter()
            .map(|s| toml::Value::String(s.clone()))
            .collect(),
    )
    .to_string()
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(p: &str) -> PathBuf {
    if p == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(p));
    }
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

/// Replace `~/…` back when showing a path to the user.
pub fn abbreviate_home(p: &Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = p.strip_prefix(&home) {
            return format!("~/{}", rest.display());
        }
    }
    p.display().to_string()
}

/// Fill `{key}` placeholders in an argv template.
pub fn render(template: &[String], vars: &[(&str, &str)]) -> Vec<String> {
    template
        .iter()
        .map(|arg| {
            let mut out = arg.clone();
            for (k, v) in vars {
                out = out.replace(&format!("{{{k}}}"), v);
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    fn cfg(terminal: &[&str], editor: &[&str], in_terminal: bool) -> Config {
        Config {
            projects: Projects {
                roots: vec![],
                max_depth: 1,
            },
            editor: Editor {
                command: s(editor),
                in_terminal,
            },
            terminal: Terminal {
                command: s(terminal),
            },
            github: Github::default(),
            clipboard: Clipboard::default(),
            keepass: Keepass::default(),
            verbs: Verbs::default(),
        }
    }

    #[test]
    fn terminal_without_exec_drops_separator() {
        let c = cfg(
            &["wezterm", "start", "--cwd", "{cwd}", "--", "{exec}"],
            &[],
            false,
        );
        assert_eq!(
            c.terminal_command("/p", &[]),
            s(&["wezterm", "start", "--cwd", "/p"])
        );
    }

    #[test]
    fn terminal_with_exec_splats_argv() {
        let c = cfg(&["kitty", "--directory={cwd}", "{exec}"], &[], false);
        assert_eq!(
            c.terminal_command("/p", &s(&["nvim", "."])),
            s(&["kitty", "--directory=/p", "nvim", "."])
        );
    }

    #[test]
    fn tui_editor_goes_through_terminal() {
        let c = cfg(
            &[
                "gnome-terminal",
                "--working-directory={cwd}",
                "--",
                "{exec}",
            ],
            &["nvim", "{path}"],
            true,
        );
        assert_eq!(
            c.editor_command("/p"),
            s(&[
                "gnome-terminal",
                "--working-directory=/p",
                "--",
                "nvim",
                "/p"
            ])
        );
    }

    #[test]
    fn gui_editor_runs_directly() {
        let c = cfg(&["kitty", "{exec}"], &["code", "{path}"], false);
        assert_eq!(c.editor_command("/p"), s(&["code", "/p"]));
    }

    #[test]
    fn commented_toml_round_trips() {
        let c = cfg(
            &["kitty", "--directory={cwd}", "{exec}"],
            &["nvim", "{path}"],
            true,
        );
        let parsed: Config = toml::from_str(&c.to_commented_toml()).expect("valid toml");
        assert_eq!(parsed.editor.command, c.editor.command);
        assert!(parsed.editor.in_terminal);
        assert_eq!(parsed.terminal.command, c.terminal.command);
        assert_eq!(parsed.verbs.shell, "$");
        assert_eq!(parsed.clipboard.max_items, 200);
    }
}
