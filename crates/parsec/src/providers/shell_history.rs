//! Command history behind the `$` verb: what you typed in zsh, bash and
//! fish, plus what you ran through Parsec itself. Files are parsed lazily
//! and re-read only when their mtime moves, checked on each query; no
//! monitors, nothing happens between queries.
//!
//! Shells write their file when they feel like it (zsh and bash at exit
//! unless told otherwise), so the newest lines of a still-open terminal may
//! be missing. Parsec's own history is written on every run.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// One distinct command line, merged across sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub cmd: String,
    /// Unix seconds of the most recent use, when a source recorded it.
    pub last: Option<u64>,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Zsh,
    Bash,
    Fish,
}

/// The history files worth reading on this machine: `$HISTFILE` if set,
/// then each shell's default location, existing ones only.
pub fn detect_files() -> Vec<(PathBuf, Format)> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let mut out: Vec<(PathBuf, Format)> = Vec::new();
    if let Some(f) = std::env::var_os("HISTFILE") {
        let p = PathBuf::from(f);
        if p.is_file() {
            out.push((p.clone(), format_for(&p)));
        }
    }
    let defaults = [
        (home.join(".zsh_history"), Format::Zsh),
        (home.join(".bash_history"), Format::Bash),
        (
            dirs::data_dir()
                .unwrap_or_else(|| home.join(".local/share"))
                .join("fish/fish_history"),
            Format::Fish,
        ),
    ];
    for (p, f) in defaults {
        if p.is_file() && !out.iter().any(|(q, _)| *q == p) {
            out.push((p, f));
        }
    }
    out
}

/// Guess a file's format from its name; unknown names parse as plain lines.
pub fn format_for(path: &Path) -> Format {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if name.contains("fish") {
        Format::Fish
    } else if name.contains("zsh") {
        Format::Zsh
    } else {
        Format::Bash
    }
}

/// Parse one file's text into (command, timestamp) pairs, oldest first.
pub fn parse(text: &str, format: Format) -> Vec<(String, Option<u64>)> {
    match format {
        Format::Zsh => parse_zsh(text),
        Format::Bash => parse_bash(text),
        Format::Fish => parse_fish(text),
    }
}

/// `: 1700000000:0;cmd` with extended history, or plain lines. A line
/// ending in a backslash continues on the next one.
fn parse_zsh(text: &str) -> Vec<(String, Option<u64>)> {
    let mut out = Vec::new();
    let mut pending: Option<(String, Option<u64>)> = None;
    for raw in text.lines() {
        let (cmd, ts, continues) = match &mut pending {
            Some((cmd, ts)) => {
                let continues = raw.ends_with('\\');
                cmd.push('\n');
                cmd.push_str(raw.strip_suffix('\\').unwrap_or(raw));
                (cmd.clone(), *ts, continues)
            }
            None => {
                let (ts, body) = split_zsh_meta(raw);
                let continues = body.ends_with('\\');
                (
                    body.strip_suffix('\\').unwrap_or(body).to_string(),
                    ts,
                    continues,
                )
            }
        };
        if continues {
            pending = Some((cmd, ts));
        } else {
            pending = None;
            push(&mut out, cmd, ts);
        }
    }
    if let Some((cmd, ts)) = pending {
        push(&mut out, cmd, ts);
    }
    out
}

fn split_zsh_meta(line: &str) -> (Option<u64>, &str) {
    if let Some(rest) = line.strip_prefix(": ") {
        if let Some((meta, body)) = rest.split_once(';') {
            let ts = meta.split(':').next().and_then(|t| t.trim().parse().ok());
            return (ts, body);
        }
    }
    (None, line)
}

/// Plain lines; `#1700000000` before a line is its timestamp when the
/// shell was configured to keep them.
fn parse_bash(text: &str) -> Vec<(String, Option<u64>)> {
    let mut out = Vec::new();
    let mut ts: Option<u64> = None;
    for line in text.lines() {
        if let Some(t) = line.strip_prefix('#') {
            if let Ok(v) = t.trim().parse::<u64>() {
                ts = Some(v);
                continue;
            }
        }
        push(&mut out, line.to_string(), ts.take());
    }
    out
}

/// YAML-ish blocks: `- cmd: ...` then indented `when: ...` and `paths:`.
fn parse_fish(text: &str) -> Vec<(String, Option<u64>)> {
    let mut out = Vec::new();
    let mut current: Option<(String, Option<u64>)> = None;
    for line in text.lines() {
        if let Some(cmd) = line.strip_prefix("- cmd: ") {
            if let Some((c, t)) = current.take() {
                push(&mut out, c, t);
            }
            current = Some((unescape_fish(cmd), None));
        } else if let Some(when) = line.trim_start().strip_prefix("when: ") {
            if let Some((_, t)) = &mut current {
                *t = when.trim().parse().ok();
            }
        }
    }
    if let Some((c, t)) = current {
        push(&mut out, c, t);
    }
    out
}

fn unescape_fish(s: &str) -> String {
    s.replace("\\n", "\n").replace("\\\\", "\\")
}

/// Lines not worth offering from a launcher.
fn trivial(cmd: &str) -> bool {
    cmd.chars().count() < 3 || matches!(cmd, "exit" | "logout" | "clear" | "reset")
}

fn push(out: &mut Vec<(String, Option<u64>)>, cmd: String, ts: Option<u64>) {
    let cmd = cmd.trim().to_string();
    if !trivial(&cmd) {
        out.push((cmd, ts));
    }
}

/// Merge parsed lists into distinct commands, most recent first. Commands
/// without timestamps keep their file order, which is also oldest first.
pub fn merge(lists: Vec<Vec<(String, Option<u64>)>>) -> Vec<Entry> {
    let mut by_cmd: HashMap<String, (Entry, usize)> = HashMap::new();
    let mut order = 0usize;
    for list in lists {
        for (cmd, ts) in list {
            order += 1;
            match by_cmd.get_mut(&cmd) {
                Some((e, seen)) => {
                    e.count += 1;
                    e.last = e.last.max(ts);
                    *seen = order;
                }
                None => {
                    by_cmd.insert(
                        cmd.clone(),
                        (
                            Entry {
                                cmd,
                                last: ts,
                                count: 1,
                            },
                            order,
                        ),
                    );
                }
            }
        }
    }
    let mut entries: Vec<(Entry, usize)> = by_cmd.into_values().collect();
    // Timestamped entries first, newest first; then untimestamped by order.
    entries.sort_by(|a, b| b.0.last.cmp(&a.0.last).then_with(|| b.1.cmp(&a.1)));
    entries.into_iter().map(|(e, _)| e).collect()
}

// ---------------------------------------------------------------- own history

/// What was run through `$`, newest first. Always fresh, unlike the shells'.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Own {
    #[serde(default)]
    pub entries: Vec<(String, u64)>,
}

impl Own {
    pub fn path() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("parsec")
            .join("shell_history.json")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    pub fn record(&mut self, cmd: &str) {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.entries.retain(|(c, _)| c != cmd);
        self.entries.insert(0, (cmd.to_string(), now));
        self.entries.truncate(1000);
        let path = Self::path();
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(text) = serde_json::to_string(self) {
            let _ = std::fs::write(&path, text);
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
            }
        }
    }

    /// As a parsed list, oldest first, so it merges like a file.
    pub fn as_list(&self) -> Vec<(String, Option<u64>)> {
        self.entries
            .iter()
            .rev()
            .map(|(c, t)| (c.clone(), Some(*t)))
            .collect()
    }
}

// ---------------------------------------------------------------- index

/// Everything merged, rebuilt when a source file's mtime changes.
#[derive(Default)]
pub struct Index {
    files: Vec<(PathBuf, Format, Option<SystemTime>)>,
    parsed: Vec<Vec<(String, Option<u64>)>>,
    own_len: usize,
    pub entries: Vec<Entry>,
}

impl Index {
    /// Point at these files (empty = detect) and rebuild if anything moved.
    /// Blocking: file reads. Call off the main thread.
    pub fn refresh(&mut self, configured: &[String], own: &Own) {
        let wanted: Vec<(PathBuf, Format)> = if configured.is_empty() {
            detect_files()
        } else {
            configured
                .iter()
                .map(|p| crate::config::expand_home(p))
                .filter(|p| p.is_file())
                .map(|p| (p.clone(), format_for(&p)))
                .collect()
        };
        let same_set = wanted.len() == self.files.len()
            && wanted
                .iter()
                .zip(&self.files)
                .all(|((p, _), (q, _, _))| p == q);
        let mut changed = !same_set || own.entries.len() != self.own_len;
        if !same_set {
            self.files = wanted.into_iter().map(|(p, f)| (p, f, None)).collect();
            self.parsed = vec![Vec::new(); self.files.len()];
        }
        for (i, (path, format, mtime)) in self.files.iter_mut().enumerate() {
            let now = std::fs::metadata(&*path).and_then(|m| m.modified()).ok();
            if now != *mtime {
                *mtime = now;
                let text = std::fs::read(&*path)
                    .map(|b| String::from_utf8_lossy(&b).into_owned())
                    .unwrap_or_default();
                self.parsed[i] = parse(&text, *format);
                changed = true;
            }
        }
        if changed {
            self.own_len = own.entries.len();
            let mut lists = self.parsed.clone();
            lists.push(own.as_list());
            self.entries = merge(lists);
            tracing::debug!(count = self.entries.len(), "shell history indexed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_extended_and_plain() {
        let text = ": 1700000001:0;ls -la\nplain one\n: 1700000002:3;echo a \\\\\n  b\n";
        let got = parse(text, Format::Zsh);
        assert_eq!(got[0], ("ls -la".into(), Some(1700000001)));
        assert_eq!(got[1], ("plain one".into(), None));
        assert_eq!(got[2], ("echo a \\\n  b".into(), Some(1700000002)));
    }

    #[test]
    fn bash_timestamps_attach_to_next_line() {
        let got = parse("#1718974886\nhttpyac auth.http\ncd /tmp\n", Format::Bash);
        assert_eq!(got[0], ("httpyac auth.http".into(), Some(1718974886)));
        assert_eq!(got[1], ("cd /tmp".into(), None));
    }

    #[test]
    fn fish_blocks() {
        let text = "- cmd: git status\n  when: 1700000000\n- cmd: echo hi\\nthere\n  when: 1700000005\n  paths:\n    - x\n";
        let got = parse(text, Format::Fish);
        assert_eq!(got[0], ("git status".into(), Some(1700000000)));
        assert_eq!(got[1], ("echo hi\nthere".into(), Some(1700000005)));
    }

    #[test]
    fn merge_dedupes_and_orders_newest_first() {
        let a = vec![("ls".to_string(), Some(10)), ("cd".to_string(), Some(20))];
        let b = vec![("ls".to_string(), Some(30)), ("old".to_string(), None)];
        let got = merge(vec![a, b]);
        assert_eq!(got[0].cmd, "ls");
        assert_eq!(got[0].count, 2);
        assert_eq!(got[0].last, Some(30));
        assert_eq!(got[1].cmd, "cd");
        assert_eq!(got[2].cmd, "old");
    }
}
