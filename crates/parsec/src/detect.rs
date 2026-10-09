//! Environment detection used for first-run defaults. Nothing here is
//! authoritative: every result lands in the user's config file where it can
//! be changed. The goal is a sensible launcher on a machine we've never seen.

use crate::config::{Editor, Terminal};
use std::path::{Path, PathBuf};

/// Find an executable on `PATH`.
pub fn which(bin: &str) -> Option<PathBuf> {
    if bin.contains('/') {
        let p = Path::new(bin);
        return p.is_file().then(|| p.to_path_buf());
    }
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .map(|dir| Path::new(dir).join(bin))
        .find(|p| p.is_file())
}

fn first_installed<'a>(candidates: &[&'a str]) -> Option<&'a str> {
    candidates.iter().copied().find(|c| which(c).is_some())
}

/// Editors that run inside a terminal and therefore need one opened.
const TUI_EDITORS: &[&str] = &["nvim", "vim", "vi", "hx", "helix", "nano", "micro", "kak"];
const GUI_EDITORS: &[&str] = &[
    "code",
    "codium",
    "zed",
    "zeditor",
    "gnome-text-editor",
    "gedit",
    "kate",
];

fn is_tui(command: &[String]) -> bool {
    let Some(bin) = command.first() else {
        return false;
    };
    let base = Path::new(bin)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_default();
    if base == "emacs" || base == "emacsclient" {
        return command
            .iter()
            .any(|a| a == "-nw" || a == "-t" || a == "--tty");
    }
    TUI_EDITORS.contains(&base.as_str())
}

/// `$VISUAL`, then `$EDITOR`, then whatever is installed.
pub fn editor() -> Editor {
    for var in ["VISUAL", "EDITOR"] {
        if let Ok(v) = std::env::var(var) {
            let mut command: Vec<String> = v.split_whitespace().map(String::from).collect();
            if !command.is_empty() {
                command.push("{path}".into());
                let in_terminal = is_tui(&command);
                return Editor {
                    command,
                    in_terminal,
                };
            }
        }
    }
    if let Some(bin) = first_installed(GUI_EDITORS) {
        return Editor {
            command: vec![bin.into(), "{path}".into()],
            in_terminal: false,
        };
    }
    if let Some(bin) = first_installed(TUI_EDITORS) {
        return Editor {
            command: vec![bin.into(), "{path}".into()],
            in_terminal: true,
        };
    }
    Editor {
        command: vec!["xdg-open".into(), "{path}".into()],
        in_terminal: false,
    }
}

/// Known terminals and how each takes a working directory and a command.
/// `{exec}` expands to the command argv, or disappears (with its separator)
/// when there is none.
const TERMINALS: &[(&str, &[&str])] = &[
    ("kitty", &["kitty", "--directory={cwd}", "{exec}"]),
    (
        "wezterm",
        &["wezterm", "start", "--cwd", "{cwd}", "--", "{exec}"],
    ),
    (
        "alacritty",
        &["alacritty", "--working-directory", "{cwd}", "-e", "{exec}"],
    ),
    ("foot", &["foot", "--working-directory={cwd}", "{exec}"]),
    (
        "ptyxis",
        &["ptyxis", "--working-directory={cwd}", "--", "{exec}"],
    ),
    (
        "gnome-terminal",
        &[
            "gnome-terminal",
            "--working-directory={cwd}",
            "--",
            "{exec}",
        ],
    ),
    (
        "konsole",
        &["konsole", "--workdir", "{cwd}", "-e", "{exec}"],
    ),
    (
        "xfce4-terminal",
        &[
            "xfce4-terminal",
            "--working-directory={cwd}",
            "-x",
            "{exec}",
        ],
    ),
    ("xterm", &["xterm", "-e", "{exec}"]),
];

fn template_for(bin: &str) -> Vec<String> {
    let base = Path::new(bin)
        .file_name()
        .map(|f| f.to_string_lossy().into_owned())
        .unwrap_or_else(|| bin.to_string());
    match TERMINALS.iter().find(|(name, _)| *name == base) {
        Some((_, tpl)) => tpl.iter().map(|s| s.to_string()).collect(),
        // Unknown terminal: `-e` is the de facto convention. No cwd support.
        None => vec![bin.to_string(), "-e".into(), "{exec}".into()],
    }
}

/// `$TERMINAL`, then hand-installed terminals (a signal of preference),
/// then the desktop defaults.
pub fn terminal() -> Terminal {
    if let Ok(t) = std::env::var("TERMINAL") {
        if !t.is_empty() && which(&t).is_some() {
            return Terminal {
                command: template_for(&t),
            };
        }
    }
    let names: Vec<&str> = TERMINALS.iter().map(|(n, _)| *n).collect();
    let bin = first_installed(&names).unwrap_or("xterm");
    Terminal {
        command: template_for(bin),
    }
}

/// Common project folders that exist in this home directory.
pub fn project_roots() -> Vec<String> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    [
        "Dev", "dev", "Projects", "projects", "Code", "code", "src", "work", "repos", "git",
    ]
    .iter()
    .filter(|d| home.join(d).is_dir())
    .map(|d| format!("~/{d}"))
    .collect()
}

/// First `.kdbx` found directly in the home directory or one level down,
/// hidden files included, as a `~/` path.
pub fn keepass_database() -> Option<String> {
    let home = dirs::home_dir()?;
    let mut found: Vec<PathBuf> = Vec::new();
    let mut scan = |dir: &Path| {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().is_some_and(|x| x == "kdbx") && p.is_file() {
                    found.push(p);
                }
            }
        }
    };
    scan(&home);
    if let Ok(rd) = std::fs::read_dir(&home) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir()
                && !p
                    .file_name()
                    .is_some_and(|n| n.to_string_lossy() == ".cache")
            {
                scan(&p);
            }
        }
    }
    found.sort();
    found.first().map(|p| crate::config::abbreviate_home(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tui_detection() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        assert!(is_tui(&s(&["nvim", "{path}"])));
        assert!(is_tui(&s(&["/usr/bin/vim", "{path}"])));
        assert!(!is_tui(&s(&["code", "{path}"])));
        assert!(!is_tui(&s(&["emacs", "{path}"])));
        assert!(is_tui(&s(&["emacs", "-nw", "{path}"])));
    }

    #[test]
    fn unknown_terminal_gets_generic_template() {
        assert_eq!(
            template_for("weird-term"),
            vec!["weird-term", "-e", "{exec}"]
        );
        assert_eq!(template_for("/usr/bin/kitty")[0], "kitty");
    }
}
