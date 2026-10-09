use gtk::gio;
use gtk::prelude::*;
use std::rc::Rc;

/// A single search result. Providers produce these; the UI renders them;
/// the engine ranks them and records picks for frecency.
#[derive(Debug, Clone)]
pub struct Item {
    /// Stable identity across runs, e.g. `app:firefox.desktop`.
    /// Used as the frecency key, so it must not depend on the query.
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Icon,
    /// Raw relevance from the provider (fuzzy match score). Frecency is
    /// layered on top by the engine, never by the provider.
    pub score: u32,
    /// Ordered actions; the first one runs on Enter.
    pub actions: Vec<Action>,
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
            Self::OpenUri(u) => write!(f, "OpenUri({u})"),
            Self::Callback(_) => write!(f, "Callback"),
            Self::CopySecret { .. } => write!(f, "CopySecret"),
            Self::Prompt(p) => write!(f, "{p:?}"),
        }
    }
}
