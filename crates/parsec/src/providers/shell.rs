//! `$ <command>`: run a shell command. Enter opens it in a terminal that
//! stays open; the second action runs it detached. `$` alone, or `$ text`,
//! also lists your command history (zsh, bash, fish, and what Parsec ran),
//! so a past command is one Enter away.
//!
//! No live preview while typing: executing half-typed commands is how you
//! lose a home directory. A preview will need an explicit trigger.
//!
//! In a pipe, `$` takes the input itself: on stdin, or in place of `{}`.
//! A folder piped in without `{}` is where the command runs instead, so
//! `parsec ⇢ $ git status` works in the project.
//! Committing a `$` row as a stage (Ctrl+Enter) is that explicit trigger:
//! the command runs in the background and its stdout is handed on.

use super::shell_history::{Index, Own};
use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Output, Provider, Query};
use async_trait::async_trait;
use futures_util::future::{select, Either};
use gtk::gio;
use gtk::glib;
use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

/// Larger input reaches `{}` through a file: a single argument can't
/// exceed 128 KiB on Linux, and a command line that long helps nobody.
const INLINE_INPUT_MAX: usize = 32 * 1024;
/// How much of a captured run's stdout is handed on.
const CAPTURE_MAX: usize = 1024 * 1024;
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(120);

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

    /// Where a run starts and the input it still gets: a pipe's input
    /// naming a folder becomes the working directory, unless the command
    /// places it with `{}`.
    fn place<'a>(&self, cmd: &str, input: Option<&'a str>) -> (String, Option<&'a str>) {
        match input.filter(|_| !cmd.contains("{}")).and_then(folder) {
            Some(dir) => (dir, None),
            None => (self.home.clone(), input),
        }
    }

    fn actions(&self, cmd: &str, input: Option<&str>, cwd: &str) -> Vec<Action> {
        if let Some(input) = input {
            return self.piped_actions(cmd, input);
        }
        // Run the command, then hand over to an interactive shell so the
        // output stays on screen.
        let exec = vec![
            self.shell.clone(),
            "-ic".into(),
            format!("{cmd}; exec {}", self.shell),
        ];
        let in_terminal = ActionKind::Command(self.cfg.borrow().terminal_command(cwd, &exec));
        let background = vec![self.shell.clone(), "-c".into(), cmd.to_string()];
        let cwd = cwd.to_string();
        vec![
            Action {
                label: "Run in terminal".into(),
                kind: self.recorded(cmd, move || crate::core::engine::run_detached(&in_terminal)),
            },
            Action {
                label: "Run in background".into(),
                kind: self.recorded(cmd, move || {
                    crate::core::engine::spawn_in(&background, Some(&cwd))
                }),
            },
            Action {
                label: "Copy".into(),
                kind: ActionKind::CopyText(cmd.to_string()),
            },
        ]
    }

    /// The same three actions with a pipe's input. The input is written
    /// out only when one runs, so typing costs nothing.
    fn piped_actions(&self, cmd: &str, input: &str) -> Vec<Action> {
        let run = |terminal: bool| {
            let (own, cfg, shell, home) = (
                self.own.clone(),
                self.cfg.clone(),
                self.shell.clone(),
                self.home.clone(),
            );
            let (cmd, input) = (cmd.to_string(), input.to_string());
            ActionKind::Callback(Rc::new(move || {
                own.borrow_mut().record(&cmd);
                let (script, file) = with_input(&cmd, &input, &shell)?;
                let cleanup = file
                    .map(|f| format!("; rm -f {}", quote(&f.to_string_lossy())))
                    .unwrap_or_default();
                let argv = if terminal {
                    let exec = vec![
                        shell.clone(),
                        "-ic".into(),
                        format!("{script}{cleanup}; exec {shell}"),
                    ];
                    cfg.borrow().terminal_command(&home, &exec)
                } else {
                    vec![shell.clone(), "-c".into(), format!("{script}{cleanup}")]
                };
                crate::core::engine::run_detached(&ActionKind::Command(argv))
            }))
        };
        vec![
            Action {
                label: "Run in terminal".into(),
                kind: run(true),
            },
            Action {
                label: "Run in background".into(),
                kind: run(false),
            },
            Action {
                label: "Copy".into(),
                kind: ActionKind::CopyText(cmd.to_string()),
            },
        ]
    }

    /// Wrap a run so it lands in Parsec's own history first.
    fn recorded(&self, cmd: &str, run: impl Fn() -> anyhow::Result<()> + 'static) -> ActionKind {
        let own = self.own.clone();
        let cmd = cmd.to_string();
        ActionKind::Callback(Rc::new(move || {
            own.borrow_mut().record(&cmd);
            run()
        }))
    }

    /// Committing the row as a pipe stage: run it in the background and
    /// hand on its stdout.
    fn output(&self, cmd: &str, input: Option<&str>, cwd: &str) -> Output {
        let (own, shell, cwd) = (self.own.clone(), self.shell.clone(), cwd.to_string());
        let (cmd, input) = (cmd.to_string(), input.map(str::to_string));
        Output::Later {
            waiting: format!("Running {}…", cmd.lines().next().unwrap_or("")),
            produce: Rc::new(move || {
                own.borrow_mut().record(&cmd);
                let (cmd, input, shell, cwd) =
                    (cmd.clone(), input.clone(), shell.clone(), cwd.clone());
                Box::pin(async move { capture(&cmd, input.as_deref(), &shell, &cwd).await })
            }),
        }
    }

    fn subtitle(&self, base: &str, cmd: &str, input: Option<&str>, cwd: &str) -> String {
        if cwd != self.home {
            let dir = crate::config::abbreviate_home(std::path::Path::new(cwd));
            return format!("{base} · in {dir}");
        }
        match input {
            None => base.to_string(),
            Some(_) if cmd.contains("{}") => format!("{base} · input in {{}}"),
            Some(_) => format!("{base} · input on stdin"),
        }
    }
}

/// The folder a pipe's input names, if it is one: a single absolute or
/// `~` path to an existing directory.
fn folder(input: &str) -> Option<String> {
    let line = input.trim();
    if line.contains('\n') || !(line.starts_with('/') || line.starts_with('~')) {
        return None;
    }
    let path = crate::config::expand_home(line);
    path.is_dir().then(|| path.to_string_lossy().into_owned())
}

fn quote(s: &str) -> String {
    shlex::try_quote(s)
        .map(|q| q.into_owned())
        .unwrap_or_else(|_| "''".into())
}

/// The script that feeds `input` to `cmd`: in place of `{}`, quoted inline
/// when small or read from a file when not; without `{}`, on the stdin of
/// the whole command line. Returns the file to delete afterwards.
fn with_input(cmd: &str, input: &str, shell: &str) -> anyhow::Result<(String, Option<PathBuf>)> {
    if cmd.contains("{}") {
        if input.len() <= INLINE_INPUT_MAX && !input.contains('\0') {
            return Ok((cmd.replace("{}", &quote(input)), None));
        }
        let file = input_file(input)?;
        let read = format!("\"$(cat {})\"", quote(&file.to_string_lossy()));
        return Ok((cmd.replace("{}", &read), Some(file)));
    }
    let file = input_file(input)?;
    Ok((on_stdin(cmd, &file.to_string_lossy(), shell), Some(file)))
}

/// `cmd` grouped so the redirect reaches its first command, even in a
/// pipeline (`a | b < f` would feed `b`). fish groups with begin/end.
fn on_stdin(cmd: &str, file: &str, shell: &str) -> String {
    if shell.ends_with("fish") {
        format!("begin; {cmd}; end < {}", quote(file))
    } else {
        format!("{{ {cmd}\n}} < {}", quote(file))
    }
}

/// Write the input where only we can read it, for a terminal to pick up.
fn input_file(input: &str) -> anyhow::Result<PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = glib::user_runtime_dir().join("parsec");
    std::fs::create_dir_all(&dir)?;
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let path = dir.join(format!("pipe-{}-{n}.txt", std::process::id()));
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?
        .write_all(input.as_bytes())?;
    Ok(path)
}

/// Kills a captured run that is dropped before it finished: the stage was
/// cancelled, so the work is no longer wanted.
struct KillOnDrop(Option<gio::Subprocess>);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(p) = self.0.take() {
            p.force_exit();
        }
    }
}

/// Run `cmd` in `cwd` without a terminal and return its stdout. The input
/// goes on stdin, or in place of `{}`.
async fn capture(cmd: &str, input: Option<&str>, shell: &str, cwd: &str) -> Result<String, String> {
    let (script, file, stdin) = match input {
        Some(input) if cmd.contains("{}") => {
            let (script, file) = with_input(cmd, input, shell).map_err(|e| e.to_string())?;
            (script, file, None)
        }
        Some(input) => (cmd.to_string(), None, Some(input.to_string())),
        None => (cmd.to_string(), None, None),
    };
    let launcher = gio::SubprocessLauncher::new(
        gio::SubprocessFlags::STDIN_PIPE
            | gio::SubprocessFlags::STDOUT_PIPE
            | gio::SubprocessFlags::STDERR_PIPE,
    );
    launcher.set_cwd(cwd);
    let argv = [shell, "-c", script.as_str()].map(std::ffi::OsStr::new);
    let proc = launcher.spawn(&argv).map_err(|e| e.to_string())?;
    if let Some(pid) = proc.identifier().and_then(|p| p.parse().ok()) {
        crate::core::scope::adopt(pid, crate::core::scope::app_id_for(shell));
    }
    let mut guard = KillOnDrop(Some(proc.clone()));
    // Always a buffer, empty for none: with a stdin pipe GIO aborts the
    // process when it gets no buffer at all.
    let stdin = glib::Bytes::from_owned(stdin.unwrap_or_default().into_bytes());
    let run = proc.communicate_future(Some(&stdin));
    let result = match select(run, glib::timeout_future(CAPTURE_TIMEOUT)).await {
        Either::Left((r, _)) => r.map_err(|e| e.to_string()),
        Either::Right(_) => Err(format!(
            "still running after {} s, stopped",
            CAPTURE_TIMEOUT.as_secs()
        )),
    };
    if let Some(f) = file {
        let _ = std::fs::remove_file(f);
    }
    let (stdout, stderr) = result?;
    guard.0 = None;
    let text = |b: Option<glib::Bytes>| {
        b.map(|b| String::from_utf8_lossy(&b).into_owned())
            .unwrap_or_default()
    };
    let (mut out, err) = (text(stdout), text(stderr));
    if out.len() > CAPTURE_MAX {
        let mut end = CAPTURE_MAX;
        while !out.is_char_boundary(end) {
            end -= 1;
        }
        out.truncate(end);
    }
    let out = out.trim_end_matches('\n').to_string();
    if out.trim().is_empty() {
        let why = err.lines().rev().find(|l| !l.trim().is_empty());
        return Err(match (proc.is_successful(), why) {
            (_, Some(line)) => line.trim().to_string(),
            (true, None) => "The command printed nothing".into(),
            (false, None) => format!("Exited with status {}", proc.exit_status()),
        });
    }
    Ok(out)
}

pub(crate) fn ago(secs: u64) -> String {
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

    fn takes_input(&self, verb: &str) -> bool {
        verb == self.cfg.borrow().verbs.shell
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let cmd = q.text.trim();
        let mut items = Vec::new();
        if !cmd.is_empty() {
            let (cwd, input) = self.place(cmd, q.input);
            items.push(Item {
                output: Some(self.output(cmd, input, &cwd)),
                preview: None,
                body: None,
                // Stable id so frecency learns that you use the shell verb,
                // not every distinct command line.
                id: "shell:run".into(),
                title: cmd.to_string(),
                subtitle: Some(self.subtitle("Run in terminal", cmd, input, &cwd)),
                icon: Icon::Named("utilities-terminal-symbolic".into()),
                score: 1000,
                actions: self.actions(cmd, input, &cwd),
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
            let (cwd, input) = self.place(&e.cmd, q.input);
            Some(Item {
                output: Some(self.output(&e.cmd, input, &cwd)),
                preview: None,
                body: None,
                // Hashed: the frecency file must not list command lines.
                id: format!("shell:hist:{}", crate::core::secrets::hash(&e.cmd)),
                title: e.cmd.lines().next().unwrap_or("").to_string(),
                subtitle: Some(self.subtitle(
                    &if sub.is_empty() {
                        "History".into()
                    } else {
                        format!("History · {}", sub.join(" · "))
                    },
                    &e.cmd,
                    input,
                    &cwd,
                )),
                icon: Icon::Named("document-open-recent-symbolic".into()),
                score,
                actions: self.actions(&e.cmd, input, &cwd),
            })
        }));
        items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(cmd: &str, input: Option<&str>) -> Result<String, String> {
        glib::MainContext::new().block_on(capture(cmd, input, "/bin/sh", "/tmp"))
    }

    #[test]
    fn input_goes_on_stdin() {
        assert_eq!(run("tr a-z A-Z", Some("ciao")).unwrap(), "CIAO");
    }

    #[test]
    fn input_fills_braces_quoted() {
        assert_eq!(
            run("printf '%s' {}", Some("a b; rm -rf x")).unwrap(),
            "a b; rm -rf x"
        );
    }

    #[test]
    fn large_input_reaches_braces_through_a_file() {
        let big = "x".repeat(INLINE_INPUT_MAX + 1);
        assert_eq!(
            run("printf '%s' {} | wc -c", Some(&big)).unwrap().trim(),
            (INLINE_INPUT_MAX + 1).to_string()
        );
    }

    #[test]
    fn stdin_reaches_the_first_command_of_a_pipeline() {
        let (script, file) = with_input("cat | wc -l", "a\nb\n", "/bin/sh").unwrap();
        let out = std::process::Command::new("/bin/sh")
            .args(["-c", &script])
            .output()
            .unwrap();
        let _ = std::fs::remove_file(file.unwrap());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "2");
    }

    #[test]
    fn runs_in_the_given_folder() {
        assert_eq!(run("pwd", None).unwrap(), "/tmp");
    }

    #[test]
    fn a_piped_folder_is_where_the_command_runs() {
        let dir = std::env::temp_dir();
        let dir = dir.to_string_lossy();
        assert_eq!(folder(&dir).as_deref(), Some(&*dir));
        assert_eq!(folder(&format!("{dir}\n")).as_deref(), Some(&*dir));
        assert_eq!(
            folder("~").as_deref(),
            dirs::home_dir().as_deref().and_then(|h| h.to_str())
        );
    }

    #[test]
    fn other_input_is_not_a_folder() {
        assert_eq!(folder("/etc/hostname"), None, "a file");
        assert_eq!(folder("/no/such/folder"), None);
        assert_eq!(folder("tmp"), None, "relative");
        assert_eq!(folder("/tmp\n/etc"), None, "several lines");
        assert_eq!(folder("hello world"), None);
    }

    #[test]
    fn failures_say_why() {
        assert_eq!(run("echo nope >&2; exit 3", None).unwrap_err(), "nope");
        assert_eq!(run("exit 4", None).unwrap_err(), "Exited with status 4");
        assert_eq!(
            run("true", None).unwrap_err(),
            "The command printed nothing"
        );
    }

    #[test]
    fn dropping_a_capture_kills_the_command() {
        let marker = std::env::temp_dir().join(format!("parsec-capture-{}", std::process::id()));
        let cmd = format!("sleep 2; touch {}", marker.display());
        glib::MainContext::new().block_on(async {
            let run = Box::pin(capture(&cmd, None, "/bin/sh", "/tmp"));
            let soon = glib::timeout_future(Duration::from_millis(200));
            assert!(matches!(select(run, soon).await, Either::Right(_)));
        });
        std::thread::sleep(Duration::from_millis(2500));
        assert!(!marker.exists(), "the command outlived its stage");
    }
}
