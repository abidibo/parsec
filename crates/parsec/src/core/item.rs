use gtk::gio;
use gtk::prelude::*;
use std::rc::Rc;

/// An item as the engine hands it to the UI: with its provider and the
/// matched character positions in the title.
#[derive(Debug, Clone)]
pub struct Hit {
    pub item: Item,
    pub provider: &'static str,
    /// Section header text when results are mixed.
    pub section: String,
    pub highlight: Vec<u32>,
}

/// A single search result. Providers produce these; the UI renders them;
/// the engine ranks them and records picks for frecency.
#[derive(Debug, Clone)]
pub struct Item {
    /// Stable identity across runs, e.g. `app:firefox.desktop`.
    /// Used as the frecency key, so it must not depend on the query.
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    /// Longer text shown wrapped under the title (an answer, a draft),
    /// as many lines as it takes.
    pub body: Option<String>,
    pub icon: Icon,
    /// Raw relevance from the provider (fuzzy match score). Frecency is
    /// layered on top by the engine, never by the provider.
    pub score: u32,
    /// Ordered actions; the first one runs on Enter.
    pub actions: Vec<Action>,
    /// What the row hands on when piped (Ctrl+Enter). `None` derives it
    /// from the actions, see [`Item::pipe_output`].
    pub output: Option<Output>,
    /// What the Quick Look pane shows while the row is selected.
    pub preview: Option<Preview>,
}

impl Item {
    /// What piping this row passes to the next verb: `output` when the
    /// provider set one, else the text of its first copy, paste or open
    /// action (a `file://` URI becomes its path). A row whose Enter is a
    /// secret hands on nothing, and neither does a secret's text.
    pub fn pipe_output(&self) -> Option<Output> {
        if let Some(o) = &self.output {
            return Some(o.clone());
        }
        if matches!(
            self.actions.first().map(|a| &a.kind),
            Some(ActionKind::CopySecret { .. } | ActionKind::PasteSecret { .. })
        ) {
            return None;
        }
        self.actions
            .iter()
            .find_map(|a| match &a.kind {
                ActionKind::CopyText(t) | ActionKind::Paste(t) => Some(t.clone()),
                ActionKind::OpenUri(u) => Some(uri_text(u)),
                _ => None,
            })
            .filter(|t| !t.trim().is_empty() && !super::secrets::is_secret(t))
            .map(Output::Text)
    }
}

/// A URI as pipe text: local files as their path, anything else as is.
fn uri_text(uri: &str) -> String {
    match uri.strip_prefix("file://") {
        Some(path) => percent_encoding::percent_decode_str(path)
            .decode_utf8_lossy()
            .into_owned(),
        None => uri.to_string(),
    }
}

/// What a row hands to the next stage of a pipe.
#[derive(Clone)]
pub enum Output {
    Text(String),
    /// Produced when the stage is committed, e.g. a command's stdout;
    /// `Err` is shown instead of moving on. Dropping the future cancels it.
    Later {
        waiting: String,
        produce: Rc<dyn Fn() -> LocalFuture<Result<String, String>>>,
    },
}

impl std::fmt::Debug for Output {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Text(t) => write!(f, "Text({} bytes)", t.len()),
            Self::Later { waiting, .. } => write!(f, "Later({waiting:?})"),
        }
    }
}

/// Quick Look content for a row.
#[derive(Clone)]
pub enum Preview {
    /// A file or folder; the pane works out how to show it.
    File(std::path::PathBuf),
    Sheet(Sheet),
    /// Built the first time the row is selected (a README, git state).
    Later(Rc<dyn Fn() -> LocalFuture<Option<Sheet>>>),
}

impl std::fmt::Debug for Preview {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::File(p) => write!(f, "File({})", p.display()),
            Self::Sheet(_) => write!(f, "Sheet"),
            Self::Later(_) => write!(f, "Later"),
        }
    }
}

/// What the Quick Look pane draws, top to bottom: a picture (or a large
/// icon, or a colour swatch), the text, then label/value facts.
#[derive(Clone, Default)]
pub struct Sheet {
    pub picture: Option<gtk::gdk::Texture>,
    pub icon: Option<gio::Icon>,
    pub swatch: Option<gtk::gdk::RGBA>,
    pub text: Option<String>,
    /// Show `text` in a monospace face (code, logs, file contents).
    pub mono: bool,
    /// `text` is Markdown, shown rendered (a README, a `.md` file).
    pub markdown: bool,
    pub facts: Vec<(String, String)>,
}

impl Sheet {
    pub fn text(text: impl Into<String>, mono: bool) -> Self {
        Self {
            text: Some(text.into()),
            mono,
            ..Self::default()
        }
    }
}

// Variants not yet produced by a core provider are part of the plugin
// protocol surface; keep them.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub enum Icon {
    None,
    /// A themed icon name such as `utilities-terminal`.
    Named(String),
    /// Anything GIO resolved for us (desktop app icons, file icons).
    GIcon(gio::Icon),
    /// An image file (shortcut and plugin icons).
    Path(std::path::PathBuf),
    /// A picture that is the result itself (a QR code): drawn large, with
    /// the title and actions below it instead of beside an icon.
    Picture(gtk::gdk::Texture),
}

#[derive(Debug, Clone)]
pub struct Action {
    pub label: String,
    pub kind: ActionKind,
}

#[allow(dead_code)]
#[derive(Clone)]
pub enum ActionKind {
    LaunchApp(gio_unix::DesktopAppInfo),
    /// argv, spawned detached.
    Command(Vec<String>),
    CopyText(String),
    /// Type the text into the window that had focus, through the Shell
    /// extension; falls back to `CopyText` without it.
    Paste(String),
    /// `Paste` for a secret: kept out of the history and cleared from the
    /// clipboard afterwards, like `CopySecret`. Falls back to it.
    PasteSecret {
        text: String,
        clear_after_secs: u64,
    },
    OpenUri(String),
    /// Provider-side effect (pin, delete, ...). Runs on the main thread.
    Callback(Rc<dyn Fn() -> anyhow::Result<()>>),
    /// Like `CopyText`, but kept out of the clipboard history and cleared
    /// from the clipboard after `clear_after_secs` if still there.
    CopySecret {
        text: String,
        clear_after_secs: u64,
    },
    /// Ask the user for a line of text (a password, a name...) in the search
    /// box, then hand it to the provider. The launcher stays open.
    Prompt(Prompt),
    /// Drill into a list: the launcher pushes a level, the search box
    /// filters it, Backspace or Esc come back. Items in the list can
    /// themselves browse further.
    Browse(Browse),
    /// Put the text in the search box as if typed. The launcher stays open.
    Query(String),
}

/// A list to drill into. `load` runs when the level opens, so the items
/// are fresh and nothing is computed for rows nobody expands. A slow load
/// can show what it has so far through the `Progress` it is handed; the
/// launcher drops the load (and with it the work) when the level closes.
#[derive(Clone)]
pub struct Browse {
    /// Shown in the chip while the level is open, e.g. "Branches".
    pub title: String,
    /// Shown while the list is empty and still loading, e.g. "Thinking…".
    pub waiting: Option<String>,
    pub load: Rc<dyn Fn(Progress) -> LocalFuture<Vec<Item>>>,
}

/// Replace the items a loading level shows, before its load finishes.
pub type Progress = Rc<dyn Fn(Vec<Item>)>;

impl Browse {
    pub fn new<F, Fut>(title: impl Into<String>, load: F) -> Self
    where
        F: Fn() -> Fut + 'static,
        Fut: std::future::Future<Output = Vec<Item>> + 'static,
    {
        Self {
            title: title.into(),
            waiting: None,
            load: Rc::new(move |_| Box::pin(load())),
        }
    }

    /// A load that reports partial lists while it works.
    pub fn streaming<F, Fut>(title: impl Into<String>, waiting: Option<String>, load: F) -> Self
    where
        F: Fn(Progress) -> Fut + 'static,
        Fut: std::future::Future<Output = Vec<Item>> + 'static,
    {
        Self {
            title: title.into(),
            waiting,
            load: Rc::new(move |progress| Box::pin(load(progress))),
        }
    }
}

impl std::fmt::Debug for Browse {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Browse({:?})", self.title)
    }
}

/// A request for input. `submit` runs with what was typed and resolves to
/// `Err(message)` to keep the prompt open with that message.
#[derive(Clone)]
pub struct Prompt {
    pub title: String,
    /// Mask the input.
    pub secret: bool,
    /// Query to put back in the search box on success, e.g. `"kp "`.
    pub restore: String,
    pub submit: Rc<dyn Fn(String) -> LocalFuture<Result<(), String>>>,
}

pub type LocalFuture<T> = std::pin::Pin<Box<dyn std::future::Future<Output = T>>>;

impl std::fmt::Debug for Prompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Prompt({:?}, secret={})", self.title, self.secret)
    }
}

impl std::fmt::Debug for ActionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::LaunchApp(info) => write!(f, "LaunchApp({:?})", info.id()),
            Self::Command(argv) => write!(f, "Command({argv:?})"),
            Self::CopyText(t) => write!(f, "CopyText({} bytes)", t.len()),
            Self::Paste(t) => write!(f, "Paste({} bytes)", t.len()),
            Self::OpenUri(u) => write!(f, "OpenUri({u})"),
            Self::Callback(_) => write!(f, "Callback"),
            Self::CopySecret { .. } => write!(f, "CopySecret"),
            Self::PasteSecret { .. } => write!(f, "PasteSecret"),
            Self::Prompt(p) => write!(f, "{p:?}"),
            Self::Browse(b) => write!(f, "{b:?}"),
            Self::Query(t) => write!(f, "Query({} bytes)", t.len()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(actions: Vec<ActionKind>) -> Item {
        Item {
            id: "t".into(),
            title: "t".into(),
            subtitle: None,
            body: None,
            icon: Icon::None,
            score: 1,
            actions: actions
                .into_iter()
                .map(|kind| Action {
                    label: "a".into(),
                    kind,
                })
                .collect(),
            output: None,
            preview: None,
        }
    }

    fn text(i: &Item) -> Option<String> {
        match i.pipe_output()? {
            Output::Text(t) => Some(t),
            Output::Later { .. } => None,
        }
    }

    #[test]
    fn output_is_the_first_copy_paste_or_open() {
        let i = item(vec![
            ActionKind::Command(vec!["code".into()]),
            ActionKind::OpenUri("file:///home/me/My%20Notes".into()),
            ActionKind::CopyText("ignored".into()),
        ]);
        assert_eq!(text(&i).as_deref(), Some("/home/me/My Notes"));
        let i = item(vec![ActionKind::OpenUri("https://x.org/a%20b".into())]);
        assert_eq!(text(&i).as_deref(), Some("https://x.org/a%20b"));
        assert!(item(vec![ActionKind::Command(vec!["x".into()])])
            .pipe_output()
            .is_none());
    }

    #[test]
    fn secrets_never_flow() {
        let i = item(vec![
            ActionKind::CopySecret {
                text: "hunter2".into(),
                clear_after_secs: 15,
            },
            ActionKind::CopyText("alice".into()),
        ]);
        assert!(
            i.pipe_output().is_none(),
            "a password row handed on its username"
        );
        super::super::secrets::remember("s3cret");
        assert!(item(vec![ActionKind::CopyText("s3cret".into())])
            .pipe_output()
            .is_none());
    }

    #[test]
    fn an_explicit_output_wins() {
        let mut i = item(vec![ActionKind::CopyText("copy".into())]);
        i.output = Some(Output::Text("explicit".into()));
        assert_eq!(text(&i).as_deref(), Some("explicit"));
    }
}
