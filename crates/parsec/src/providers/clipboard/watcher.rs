//! Feeds clipboard changes to a callback.
//!
//! Preferred: `wl-paste --watch`, push-based, needs the data-control
//! protocol (sway, Hyprland, GNOME 48+...). Fallback: poll `xclip` through
//! XWayland every `poll_secs`; the compositor bridges the Wayland clipboard
//! to X11 and an X selection read never touches keyboard focus. Polling
//! `wl-paste` one-shot is not an option: without data-control it opens a
//! surface to get focus, stealing it from whatever you're typing in, once
//! per tick. A GNOME Shell extension will replace polling later.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::ffi::OsStr;
use std::rc::Rc;
use std::time::Duration;

pub fn start(poll_secs: u64, on_text: impl Fn(String) + 'static) {
    if crate::detect::which("wl-paste").is_none() {
        tracing::warn!("wl-paste not found: clipboard history disabled (install wl-clipboard)");
        return;
    }
    let on_text: Rc<dyn Fn(String)> = Rc::new(on_text);
    glib::spawn_future_local(async move {
        if !watch(on_text.clone()).await {
            if xclip_usable() {
                tracing::info!(
                    "compositor lacks data-control, polling via xclip every {poll_secs}s"
                );
                poll(Duration::from_secs(poll_secs.max(1)), on_text).await;
            } else {
                tracing::warn!(
                    "clipboard history disabled: no data-control protocol and no xclip. \
                     Install xclip (polled through XWayland) or upgrade to GNOME 48+."
                );
            }
        }
    });
}

/// Run `wl-paste --watch` until it exits. Returns false if it never worked.
async fn watch(on_text: Rc<dyn Fn(String)>) -> bool {
    // `cat` echoes the selection, then we add a NUL as a record separator.
    let argv = [
        "wl-paste",
        "--no-newline",
        "--type",
        "text",
        "--watch",
        "sh",
        "-c",
        "cat; printf '\\0'",
    ];
    let argv: Vec<&OsStr> = argv.iter().map(OsStr::new).collect();
    let proc = match gio::Subprocess::newv(
        &argv,
        gio::SubprocessFlags::STDOUT_PIPE | gio::SubprocessFlags::STDERR_SILENCE,
    ) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("cannot start wl-paste: {e}");
            return false;
        }
    };
    let Some(stdout) = proc.stdout_pipe() else {
        return false;
    };

    let mut pending: Vec<u8> = Vec::new();
    let mut delivered = false;
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        match stdout.read_future(buf, glib::Priority::DEFAULT).await {
            Ok((b, 0)) => {
                buf = b;
                let _ = &buf;
                break; // EOF: the child exited
            }
            Ok((b, n)) => {
                pending.extend_from_slice(&b[..n]);
                buf = b;
                while let Some(pos) = pending.iter().position(|&c| c == 0) {
                    let record: Vec<u8> = pending.drain(..=pos).collect();
                    let text = String::from_utf8_lossy(&record[..record.len() - 1]).into_owned();
                    on_text(text);
                    delivered = true;
                }
            }
            Err((_, e)) => {
                tracing::warn!("clipboard watcher read failed: {e}");
                break;
            }
        }
    }
    let _ = proc.wait_future().await;
    if proc.is_successful() || delivered {
        // Worked, then ended (compositor restart?). Treat as supported.
        tracing::warn!("clipboard watcher exited");
        true
    } else {
        false
    }
}

/// Current clipboard text without touching focus, or `None` when that is
/// not possible here (no xclip / no XWayland) or the clipboard is not text.
pub fn current_text() -> Option<String> {
    if xclip_usable() {
        xclip_read()
    } else {
        None
    }
}

fn xclip_usable() -> bool {
    crate::detect::which("xclip").is_some() && std::env::var_os("DISPLAY").is_some()
}

/// One clipboard read through X11. `None` when empty, non-text, or when the
/// owner does not answer in time (xclip blocks on an unresponsive owner).
fn xclip_read() -> Option<String> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let mut child = Command::new("xclip")
        .args(["-o", "-selection", "clipboard", "-t", "UTF8_STRING"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < Duration::from_secs(2) => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut out = String::new();
    child.stdout.take()?.read_to_string(&mut out).ok()?;
    Some(out)
}

/// Poll the clipboard and report changes. Runs forever.
async fn poll(every: Duration, on_text: Rc<dyn Fn(String)>) {
    let last: RefCell<Option<String>> = RefCell::new(None);
    loop {
        let text = gio::spawn_blocking(xclip_read).await.ok().flatten();
        if let Some(text) = text {
            if last.borrow().as_deref() != Some(text.as_str()) {
                *last.borrow_mut() = Some(text.clone());
                on_text(text);
            }
        }
        glib::timeout_future(every).await;
    }
}
