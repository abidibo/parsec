//! `qr <text>`: a QR code of the text, drawn large in the launcher as you
//! type, to scan with a phone. `qr` alone encodes the clipboard. Enter
//! copies the image; Tab saves it to Pictures or copies the text.
//!
//! Always black on white with the standard quiet zone, whatever the theme:
//! phone cameras read that best.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use anyhow::Context;
use async_trait::async_trait;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use qrcode::{Color, EcLevel, QrCode};
use std::rc::Rc;

/// Modules of white around the code, as the standard asks.
const QUIET: usize = 4;
/// Texture size to aim for: twice the 240 px it is shown at, so it stays
/// sharp on HiDPI and halves cleanly on normal screens.
const TARGET_PX: usize = 480;

pub struct QrProvider {
    cfg: SharedConfig,
}

impl QrProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self { cfg }
    }
}

/// The code as a texture, or why it can't be one. Medium error correction
/// when it fits, low for long text.
pub fn render(text: &str) -> Result<gdk::Texture, String> {
    let code = QrCode::with_error_correction_level(text, EcLevel::M)
        .or_else(|_| QrCode::with_error_correction_level(text, EcLevel::L))
        .map_err(|_| {
            format!(
                "Too long for a QR code: {} bytes, about 2,900 fit",
                text.len()
            )
        })?;
    let (rgba, side) = pixels(code.width(), &code.to_colors());
    let bytes = glib::Bytes::from_owned(rgba);
    Ok(gdk::MemoryTexture::new(
        side as i32,
        side as i32,
        gdk::MemoryFormat::R8g8b8a8,
        &bytes,
        side * 4,
    )
    .upcast())
}

/// RGBA pixels for a `width`×`width` module grid plus the quiet zone, each
/// module an even number of pixels.
fn pixels(width: usize, colors: &[Color]) -> (Vec<u8>, usize) {
    let modules = width + 2 * QUIET;
    let px = (TARGET_PX / modules).max(1) & !1;
    let px = px.max(2);
    let side = modules * px;
    let mut out = vec![255u8; side * side * 4];
    for (i, c) in colors.iter().enumerate() {
        if *c != Color::Dark {
            continue;
        }
        let (mx, my) = (i % width + QUIET, i / width + QUIET);
        for y in my * px..(my + 1) * px {
            let row = y * side * 4;
            for x in mx * px..(mx + 1) * px {
                let o = row + x * 4;
                out[o..o + 3].fill(0);
            }
        }
    }
    (out, side)
}

/// What the clipboard holds as text, read while the launcher has focus.
async fn clipboard_text() -> Option<String> {
    let display = gdk::Display::default()?;
    let text = display.clipboard().read_text_future().await.ok()??;
    let text = text.trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// Put the PNG on the clipboard: `wl-copy` keeps serving it after the
/// launcher hides, GTK is the fallback.
fn copy_png(texture: &gdk::Texture) -> anyhow::Result<()> {
    if crate::detect::which("wl-copy").is_some() {
        use std::io::Write;
        let png = texture.save_to_png_bytes();
        let mut child = std::process::Command::new("wl-copy")
            .args(["--type", "image/png"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("spawning wl-copy")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&png).context("writing to wl-copy")?;
        }
        return Ok(());
    }
    let display = gdk::Display::default().context("no display")?;
    display.clipboard().set_texture(texture);
    Ok(())
}

/// Save to `~/Pictures/qr-<time>.png` and say where in a notification.
fn save_png(texture: &gdk::Texture) -> anyhow::Result<()> {
    let dir = glib::user_special_dir(glib::UserDirectory::Pictures)
        .or_else(dirs::picture_dir)
        .or_else(dirs::home_dir)
        .context("no folder to save to")?;
    let stamp = glib::DateTime::now_local()
        .and_then(|d| d.format("%Y%m%d-%H%M%S"))
        .map(|s| s.to_string())
        .unwrap_or_else(|_| "code".into());
    let path = dir.join(format!("qr-{stamp}.png"));
    texture
        .save_to_png(&path)
        .with_context(|| format!("saving {}", path.display()))?;
    tracing::info!(path = %path.display(), "saved QR code");
    if let Some(app) = gio::Application::default() {
        let n = gio::Notification::new("QR code saved");
        n.set_body(Some(&crate::config::abbreviate_home(&path)));
        app.send_notification(None, &n);
    }
    Ok(())
}

fn one_line(text: &str, max: usize) -> String {
    let line = text.lines().next().unwrap_or("");
    let mut out: String = line.chars().take(max).collect();
    if line.chars().count() > max || text.lines().nth(1).is_some() {
        out.push('…');
    }
    out
}

#[async_trait(?Send)]
impl Provider for QrProvider {
    fn id(&self) -> &'static str {
        "qr"
    }

    fn title(&self) -> String {
        "QR code".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.qr.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let typed = q.text.trim();
        let (text, source) = if typed.is_empty() {
            match clipboard_text().await {
                // A password Parsec just copied stays off the screen.
                Some(t) if !crate::core::secrets::is_secret(&t) => (t, "From the clipboard"),
                _ => return Vec::new(),
            }
        } else {
            (typed.to_string(), "Typed")
        };
        let count = text.chars().count();
        let subtitle = format!(
            "{source} · {count} character{}",
            if count == 1 { "" } else { "s" }
        );
        let copy_text = Action {
            label: "Copy text".into(),
            kind: ActionKind::CopyText(text.clone()),
        };
        let texture = match render(&text) {
            Ok(t) => t,
            Err(why) => {
                return vec![Item {
                    id: "qr:code".into(),
                    title: why,
                    subtitle: Some(subtitle),
                    icon: Icon::Named("dialog-warning-symbolic".into()),
                    score: 1,
                    actions: vec![copy_text],
                }];
            }
        };
        let (for_copy, for_save) = (texture.clone(), texture.clone());
        vec![Item {
            // One id for every code: frecency learns the verb, not the text.
            id: "qr:code".into(),
            title: one_line(&text, 80),
            subtitle: Some(subtitle),
            icon: Icon::Picture(texture),
            score: 1,
            actions: vec![
                Action {
                    label: "Copy image".into(),
                    kind: ActionKind::Callback(Rc::new(move || copy_png(&for_copy))),
                },
                Action {
                    label: "Save to Pictures".into(),
                    kind: ActionKind::Callback(Rc::new(move || save_png(&for_save))),
                },
                copy_text,
            ],
        }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modules_scale_evenly_with_a_quiet_zone() {
        let code = QrCode::with_error_correction_level("https://example.org", EcLevel::M).unwrap();
        let w = code.width();
        let (rgba, side) = pixels(w, &code.to_colors());
        let px = side / (w + 2 * QUIET);
        assert_eq!(px % 2, 0);
        assert_eq!(rgba.len(), side * side * 4);
        // The quiet zone is white; the top-left finder pattern starts dark.
        assert_eq!(rgba[0..4], [255, 255, 255, 255]);
        let corner = (QUIET * px * side + QUIET * px) * 4;
        assert_eq!(rgba[corner..corner + 4], [0, 0, 0, 255]);
    }

    #[test]
    fn too_long_text_is_refused() {
        let long = "x".repeat(5000);
        assert!(QrCode::with_error_correction_level(&long, EcLevel::L).is_err());
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(one_line("abc", 80), "abc");
        assert_eq!(one_line("abc\ndef", 80), "abc…");
        assert_eq!(one_line("abcdef", 3), "abc…");
    }
}
