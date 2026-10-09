use super::frecency::Frecency;
use super::{secrets, ActionKind, Hit, Item, Matcher, Prompt, Provider, Query};
use anyhow::{Context, Result};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

/// Owns the providers, the matcher and the frecency store. One per process,
/// living on the GTK main thread behind an `Rc`.
pub struct Engine {
    providers: Vec<Box<dyn Provider>>,
    matcher: Matcher,
    frecency: RefCell<Frecency>,
}

pub const RESULT_LIMIT: usize = 10;

impl Engine {
    pub fn new(providers: Vec<Box<dyn Provider>>) -> Self {
        Self {
            providers,
            matcher: Matcher::new(),
            frecency: RefCell::new(Frecency::load()),
        }
    }

    /// The keyword at the start of `raw`, with the name to show for it.
    pub fn verb_info(&self, raw: &str) -> Option<(String, String)> {
        let raw = raw.trim_start();
        for p in &self.providers {
            for pre in p.prefixes() {
                if strip_verb(raw, &pre).is_some() {
                    return Some((pre.clone(), p.verb_label(&pre)));
                }
            }
        }
        None
    }

    /// Every keyword with its label, in provider order.
    pub fn verbs(&self) -> Vec<(String, String)> {
        self.providers
            .iter()
            .flat_map(|p| {
                p.prefixes()
                    .into_iter()
                    .map(|pre| {
                        let label = p.verb_label(&pre);
                        (pre, label)
                    })
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// Run the query through every applicable provider, apply frecency, sort.
    pub async fn search(&self, raw: &str) -> Vec<Hit> {
        let raw = raw.trim_start();
        let mut items: Vec<Hit> = Vec::new();

        let verb_active = self.providers.iter().any(|o| {
            o.prefixes()
                .iter()
                .any(|pre| strip_verb(raw, pre).is_some())
        });

        for p in &self.providers {
            let prefixes = p.prefixes();
            let matched = prefixes
                .iter()
                .find_map(|pre| strip_verb(raw, pre).map(|rest| (rest, pre.as_str())));
            let (text, verb) = match matched {
                Some((rest, verb)) => (rest, Some(verb)),
                None => {
                    // A prefixed query is for that provider alone.
                    if verb_active || !(prefixes.is_empty() || p.accepts_unprefixed()) {
                        continue;
                    }
                    (raw, None)
                }
            };
            let q = Query::new(text, verb, &self.matcher);
            let found = p.query(&q).await;
            tracing::trace!(provider = p.id(), count = found.len(), "provider results");
            items.extend(found.into_iter().map(|item| hit(p.as_ref(), &q, item)));
        }

        if items.is_empty() && !raw.is_empty() && !verb_active {
            let q = Query::new(raw, None, &self.matcher);
            for p in &self.providers {
                let found = p.fallback(&q).await;
                items.extend(found.into_iter().map(|item| hit(p.as_ref(), &q, item)));
            }
        }

        let frec = self.frecency.borrow();
        if raw.is_empty() {
            // Launcher just opened: show things we've picked before.
            items.retain(|h| frec.has(&h.item.id));
        }

        let mut scored: Vec<(f64, Hit)> = items
            .into_iter()
            .map(|h| {
                let s = (h.item.score.max(1) as f64) * frec.boost(&h.item.id);
                (s, h)
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| a.1.item.title.cmp(&b.1.item.title))
        });
        scored.truncate(RESULT_LIMIT);
        let hits: Vec<Hit> = scored.into_iter().map(|(_, h)| h).collect();
        group_by_provider(hits)
    }

    /// Execute the item's action at `index` (0 = default) and record the pick.
    pub fn activate(&self, item: &Item, index: usize) -> Result<Outcome> {
        let action = item
            .actions
            .get(index)
            .or_else(|| item.actions.first())
            .context("item has no actions")?;
        tracing::info!(id = %item.id, action = %action.label, "activate");
        if let ActionKind::Prompt(p) = &action.kind {
            // Not a pick yet; the provider decides what happens after input.
            return Ok(Outcome::Prompt(p.clone()));
        }
        run(&action.kind)?;
        let mut frec = self.frecency.borrow_mut();
        frec.record(&item.id);
        frec.save();
        Ok(Outcome::Done)
    }
}

/// Keep rank order between providers (by their best hit) but gather each
/// provider's hits together, so a section header appears once.
fn group_by_provider(hits: Vec<Hit>) -> Vec<Hit> {
    let mut order: Vec<&'static str> = Vec::new();
    for h in &hits {
        if !order.contains(&h.provider) {
            order.push(h.provider);
        }
    }
    let mut out = Vec::with_capacity(hits.len());
    for p in order {
        out.extend(hits.iter().filter(|h| h.provider == p).cloned());
    }
    out
}

fn hit(p: &dyn Provider, q: &Query<'_>, item: Item) -> Hit {
    let highlight = if q.is_empty() {
        Vec::new()
    } else {
        q.indices(&item.title)
    };
    Hit {
        section: p.title(),
        provider: p.id(),
        highlight,
        item,
    }
}

/// What the UI should do after an activation.
pub enum Outcome {
    /// Hide the launcher, the action ran.
    Done,
    /// Keep the launcher open and collect input.
    Prompt(Prompt),
}

/// Clear the clipboard after a delay, unless something else was copied in
/// the meantime (checked when the compositor lets us read without stealing
/// focus; otherwise cleared unconditionally).
fn schedule_clipboard_clear(text: String, after_secs: u64) {
    glib::timeout_add_local_once(std::time::Duration::from_secs(after_secs), move || {
        glib::spawn_future_local(async move {
            let current = gio::spawn_blocking(crate::providers::clipboard::current_text)
                .await
                .ok()
                .flatten();
            let still_there = current.as_deref().is_none_or(|c| c == text);
            if still_there {
                tracing::info!("clearing copied secret from the clipboard");
                let _ = std::process::Command::new("wl-copy")
                    .arg("--clear")
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .status();
            }
        });
    });
}

/// `gh foo` -> Some("foo"), `gh` -> Some(""), `ghost` -> None.
/// A symbol prefix like `$` needs no separator: `$ls` works.
fn strip_verb<'a>(raw: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = raw.strip_prefix(prefix)?;
    let word_like = prefix.chars().last().is_some_and(|c| c.is_alphanumeric());
    if word_like && !(rest.is_empty() || rest.starts_with(char::is_whitespace)) {
        return None;
    }
    Some(rest.trim_start())
}

/// Put text on the clipboard. `wl-copy` is preferred: it forks a process
/// that keeps serving the selection after our window is hidden, which the
/// GTK clipboard cannot guarantee on Wayland. GTK is the fallback.
fn copy_text(text: &str) -> Result<()> {
    if crate::detect::which("wl-copy").is_some() {
        use std::io::Write;
        let mut child = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("spawning wl-copy")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin
                .write_all(text.as_bytes())
                .context("writing to wl-copy")?;
        }
        return Ok(());
    }
    let display = gtk::gdk::Display::default().context("no display")?;
    display.clipboard().set_text(text);
    Ok(())
}

/// Run an action outside the normal pick flow (prompt submissions).
pub fn run_detached(kind: &ActionKind) -> Result<()> {
    run(kind)
}

fn run(kind: &ActionKind) -> Result<()> {
    match kind {
        ActionKind::LaunchApp(info) => {
            let ctx = gtk::gdk::Display::default().map(|d| d.app_launch_context());
            info.launch(&[], ctx.as_ref())
                .with_context(|| format!("launching {}", info.id().unwrap_or_default()))?;
        }
        ActionKind::Command(argv) => {
            let (program, args) = argv.split_first().context("empty command")?;
            std::process::Command::new(program)
                .args(args)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .with_context(|| format!("spawning {program}"))?;
        }
        ActionKind::CopyText(text) => copy_text(text)?,
        ActionKind::CopySecret {
            text,
            clear_after_secs,
        } => {
            secrets::remember(text);
            copy_text(text)?;
            if *clear_after_secs > 0 {
                schedule_clipboard_clear(text.clone(), *clear_after_secs);
            }
        }
        ActionKind::Callback(f) => f()?,
        ActionKind::Prompt(_) => unreachable!("prompts are handled by the UI"),
        ActionKind::OpenUri(uri) => {
            gio::AppInfo::launch_default_for_uri(uri, None::<&gio::AppLaunchContext>)
                .with_context(|| format!("opening {uri}"))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::strip_verb;

    #[test]
    fn word_prefix_needs_boundary() {
        assert_eq!(strip_verb("gh foo", "gh"), Some("foo"));
        assert_eq!(strip_verb("gh", "gh"), Some(""));
        assert_eq!(strip_verb("ghost", "gh"), None);
    }

    #[test]
    fn symbol_prefix_needs_none() {
        assert_eq!(strip_verb("$ls -la", "$"), Some("ls -la"));
        assert_eq!(strip_verb("$ ls", "$"), Some("ls"));
    }
}
