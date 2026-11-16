//! `ocr [lang]`: extract text from an image on the clipboard, an image
//! copied in Files, or a file piped in (`f page.png ⇢ ocr`), through
//! `tesseract`. Enter pastes the text into the window you came from when
//! the Shell extension is active, or copies it to the clipboard; Ctrl+Enter
//! pipes the text to the next verb (`ocr ⇢ tr >en`, `ocr ⇢ ask`).
//!
//! When nothing is on the clipboard (or as a second row when an image is
//! already there), `ocr` offers *Capture screen area*, which opens GNOME's
//! interactive screenshot tool, reads the text from the captured image, and
//! copies it to the clipboard.
//!
//! `tesseract` is optional: when it is missing, `ocr` shows how to install
//! it and offers to run the command in your terminal.

use super::img::{self, Image, Source};
use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Preview, Provider, Query, Sheet};
use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::rc::Rc;

/// Fingerprint of the images a cached OCR run came from, so typing or
/// re-opening doesn't run `tesseract` again until the image or language
/// changes.
#[derive(PartialEq, Eq, Clone)]
enum ImageKey {
    File(PathBuf, u32, u32),
    Data(usize, u32, u32),
}

impl ImageKey {
    fn of(img: &Image) -> Self {
        match &img.source {
            Source::File(p) => Self::File(p.clone(), img.width, img.height),
            Source::Data(b) => Self::Data(std::sync::Arc::as_ptr(b) as usize, img.width, img.height),
        }
    }
}

#[derive(Clone)]
struct CachedRun {
    keys: Vec<ImageKey>,
    lang: String,
    result: Result<String, String>,
}

pub struct OcrProvider {
    cfg: SharedConfig,
    /// Clipboard or captured images from the last read, invalidated when the clipboard changes.
    images: Rc<RefCell<Option<Rc<Vec<Image>>>>>,
    stale: Rc<Cell<bool>>,
    /// True right after *Capture screen area* put its file into `images`, so
    /// re-opening on bare `ocr ` keeps that file instead of re-reading the clipboard.
    from_capture: Rc<Cell<bool>>,
    watching: Cell<bool>,
    /// Installed tesseract languages, read once after `tesseract` is found.
    langs: RefCell<Option<Vec<String>>>,
    /// Last OCR output for `(images, lang)`.
    last: RefCell<Option<CachedRun>>,
}

impl OcrProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            images: Rc::new(RefCell::new(None)),
            stale: Rc::new(Cell::new(true)),
            from_capture: Rc::new(Cell::new(false)),
            watching: Cell::new(false),
            langs: RefCell::new(None),
            last: RefCell::new(None),
        }
    }

    async fn clipboard_images(&self, fresh: bool) -> Rc<Vec<Image>> {
        if !self.watching.replace(true) {
            if let Some(display) = gdk::Display::default() {
                let stale = self.stale.clone();
                let from_capture = self.from_capture.clone();
                display.clipboard().connect_changed(move |_| {
                    stale.set(true);
                    from_capture.set(false);
                });
            }
        }
        if self.from_capture.replace(false) || (!fresh && !self.stale.get()) {
            if let Some(cached) = self.images.borrow().clone() {
                return cached;
            }
        }
        self.stale.set(false);
        let found = Rc::new(img::copied_images().await);
        *self.images.borrow_mut() = Some(found.clone());
        found
    }

    async fn available_langs(&self, fresh: bool) -> Vec<String> {
        if !fresh {
            if let Some(cached) = self.langs.borrow().clone() {
                return cached;
            }
        }
        let langs = gio::spawn_blocking(list_langs)
            .await
            .unwrap_or_default();
        if !langs.is_empty() {
            *self.langs.borrow_mut() = Some(langs.clone());
        }
        langs
    }

    async fn recognize(&self, images: &[Image], lang: &str) -> Result<String, String> {
        let keys: Vec<ImageKey> = images.iter().map(ImageKey::of).collect();
        if let Some(c) = self.last.borrow().as_ref() {
            if c.keys == keys && c.lang == lang {
                return c.result.clone();
            }
        }
        let imgs = images.to_vec();
        let lang_owned = lang.to_string();
        let result = gio::spawn_blocking(move || {
            recognize_all(&imgs, &lang_owned).map_err(|e| format!("{e:#}"))
        })
        .await
        .unwrap_or_else(|_| Err("OCR worker thread failed".into()));
        *self.last.borrow_mut() = Some(CachedRun {
            keys,
            lang: lang.to_string(),
            result: result.clone(),
        });
        result
    }
}

/// Parse `tesseract --list-langs` output into language codes (`eng`, `ita`…),
/// leaving out the header line and orientation/equation models (`osd`, `equ`).
fn parse_langs(output: &str) -> Vec<String> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| {
            !l.is_empty()
                && !l.contains(':')
                && !l.contains(char::is_whitespace)
                && !matches!(*l, "osd" | "equ")
        })
        .map(String::from)
        .collect()
}

fn list_langs() -> Vec<String> {
    let Ok(out) = Command::new("tesseract").arg("--list-langs").output() else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&out.stdout);
    let mut langs = parse_langs(&text);
    if langs.is_empty() {
        let err = String::from_utf8_lossy(&out.stderr);
        langs = parse_langs(&err);
    }
    langs
}

/// Default language: `eng` when installed, else the first available one.
fn default_lang(installed: &[String]) -> String {
    if installed.is_empty() || installed.iter().any(|l| l == "eng") {
        "eng".into()
    } else {
        installed[0].clone()
    }
}

/// Clean up `tesseract` stdout: strip form-feed page breaks and trailing spaces.
fn clean_ocr_text(raw: &str) -> String {
    raw.replace('\x0c', "")
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn recognize_one(img: &Image, lang: &str) -> Result<String> {
    let out = match &img.source {
        Source::File(path) => Command::new("tesseract")
            .arg(path)
            .arg("stdout")
            .args(["-l", lang])
            .stdin(Stdio::null())
            .output()
            .context("running tesseract")?,
        Source::Data(png) => {
            let mut child = Command::new("tesseract")
                .arg("-")
                .arg("stdout")
                .args(["-l", lang])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .context("spawning tesseract")?;
            if let Some(mut stdin) = child.stdin.take() {
                stdin
                    .write_all(png)
                    .context("sending image to tesseract")?;
            }
            child.wait_with_output().context("waiting for tesseract")?
        }
    };
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let line = err
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("tesseract failed");
        bail!("{line}");
    }
    Ok(clean_ocr_text(&String::from_utf8_lossy(&out.stdout)))
}

fn recognize_all(images: &[Image], lang: &str) -> Result<String> {
    let mut parts = Vec::new();
    for img in images {
        let text = recognize_one(img, lang)?;
        if !text.is_empty() {
            parts.push(text);
        }
    }
    Ok(parts.join("\n\n"))
}

/// Package manager command to install `tesseract` (or a language pack).
fn install_command(lang_pack: Option<&str>) -> String {
    if crate::detect::which("apt").is_some() {
        match lang_pack {
            Some(l) => format!("sudo apt install tesseract-ocr-{l}"),
            None => "sudo apt install tesseract-ocr".into(),
        }
    } else if crate::detect::which("dnf").is_some() {
        match lang_pack {
            Some(l) => format!("sudo dnf install tesseract-langpack-{l}"),
            None => "sudo dnf install tesseract".into(),
        }
    } else if crate::detect::which("pacman").is_some() {
        match lang_pack {
            Some(l) => format!("sudo pacman -S tesseract-data-{l}"),
            None => "sudo pacman -S tesseract tesseract-data-eng".into(),
        }
    } else {
        match lang_pack {
            Some(l) => format!("Install tesseract language pack \"{l}\""),
            None => "Install tesseract-ocr with your package manager".into(),
        }
    }
}

fn install_actions(cfg: &crate::config::Config, cmd: &str) -> Vec<Action> {
    let sh = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into());
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    let exec = vec![sh.clone(), "-ic".into(), format!("{cmd}; exec {sh}")];
    let argv = cfg.terminal_command(&home, &exec);
    let cmd_owned = cmd.to_string();
    vec![
        Action {
            label: "Install in terminal".into(),
            kind: ActionKind::Command(argv),
        },
        Action {
            label: "Copy command".into(),
            kind: ActionKind::CopyText(cmd_owned),
        },
    ]
}

fn first_line(text: &str, max_chars: usize) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let line = line.trim();
    if line.chars().count() > max_chars {
        let cut: String = line.chars().take(max_chars - 1).collect();
        format!("{cut}…")
    } else {
        line.to_string()
    }
}

fn source_label(images: &[Image]) -> String {
    match images {
        [one] => one.name(),
        many => format!("{} images", many.len()),
    }
}

fn build_preview(images: &[Image], text: &str, lang: &str) -> Preview {
    let lines = text.lines().count();
    let words = text.split_whitespace().count();
    let chars = text.chars().count();
    let mut facts = vec![("Source".into(), source_label(images))];
    if let [one] = images {
        facts.push(("Size".into(), format!("{}×{}", one.width, one.height)));
    }
    facts.push(("Lines".into(), lines.to_string()));
    facts.push(("Words".into(), words.to_string()));
    facts.push(("Characters".into(), chars.to_string()));
    facts.push(("Language".into(), lang.to_string()));
    Preview::Sheet(Sheet {
        text: Some(text.to_string()),
        mono: false,
        facts,
        ..Sheet::default()
    })
}

fn text_actions(text: &str) -> Vec<Action> {
    let copy = Action {
        label: "Copy".into(),
        kind: ActionKind::CopyText(text.to_string()),
    };
    if crate::gnome_shell::bridge().is_active() {
        let paste = Action {
            label: "Paste".into(),
            kind: ActionKind::Paste(text.to_string()),
        };
        vec![paste, copy]
    } else {
        vec![copy]
    }
}

/// Open the desktop's interactive screenshot UI through the XDG portal and
/// hand the saved file to `on_saved`. Waits briefly so the launcher window
/// is hidden before the screen is frozen for selection.
pub(super) fn interactive_screenshot(on_saved: impl FnOnce(PathBuf) + 'static) -> Result<()> {
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let on_saved = Rc::new(RefCell::new(Some(
        Box::new(on_saved) as Box<dyn FnOnce(PathBuf)>
    )));
    glib::timeout_add_local_once(std::time::Duration::from_millis(120), move || {
        let token = format!("parsec_{}", glib::monotonic_time().unsigned_abs());
        let sub_slot: Rc<Cell<Option<gio::SignalSubscription>>> = Rc::new(Cell::new(None));
        let sub_slot_cb = sub_slot.clone();
        let on_saved_cb = on_saved.clone();
        let sub_id = bus.subscribe_to_signal(
            Some("org.freedesktop.portal.Desktop"),
            Some("org.freedesktop.portal.Request"),
            Some("Response"),
            None,
            None,
            gio::DBusSignalFlags::NONE,
            move |sig| {
                drop(sub_slot_cb.take());
                let status: u32 = sig.parameters.child_value(0).get().unwrap_or(1);
                if status != 0 {
                    return;
                }
                let results: std::collections::HashMap<String, glib::Variant> =
                    sig.parameters.child_value(1).get().unwrap_or_default();
                let Some(uri) = results.get("uri").and_then(|v| v.get::<String>()) else {
                    return;
                };
                let Some(path) = gio::File::for_uri(&uri).path() else {
                    return;
                };
                if let Some(cb) = on_saved_cb.borrow_mut().take() {
                    cb(path);
                }
            },
        );
        sub_slot.set(Some(sub_id));

        let mut opts: std::collections::HashMap<String, glib::Variant> =
            std::collections::HashMap::new();
        opts.insert("interactive".into(), true.to_variant());
        opts.insert("handle_token".into(), token.to_variant());
        let args = ("", opts).to_variant();
        let sub_slot_err = sub_slot.clone();
        bus.call(
            Some("org.freedesktop.portal.Desktop"),
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.Screenshot",
            "Screenshot",
            Some(&args),
            Some(glib::VariantTy::new("(o)").unwrap()),
            gio::DBusCallFlags::NONE,
            10_000,
            gio::Cancellable::NONE,
            move |res| {
                if let Err(e) = res {
                    tracing::warn!("portal screenshot call failed: {e}");
                    drop(sub_slot_err.take());
                }
            },
        );
    });
    Ok(())
}

impl OcrProvider {
    /// Interactive screenshot through the desktop portal: when the user captures
    /// an area, load that file as the current OCR image and re-open the launcher
    /// on `ocr` so the extracted text and its Quick Look preview are on screen.
    fn capture_item(&self, restore: &str, score: u32) -> Item {
        let restore = restore.to_string();
        let images = self.images.clone();
        let stale = self.stale.clone();
        let from_capture = self.from_capture.clone();
        Item {
            output: None,
            preview: None,
            body: None,
            id: "ocr:capture".into(),
            title: "Capture screen area".into(),
            subtitle: Some("Select an area on screen and read its text".into()),
            icon: Icon::Named("camera-photo-symbolic".into()),
            score,
            actions: vec![Action {
                label: "Capture".into(),
                kind: ActionKind::Callback(Rc::new(move || {
                    let restore = restore.clone();
                    let images = images.clone();
                    let stale = stale.clone();
                    let from_capture = from_capture.clone();
                    interactive_screenshot(move |path| {
                        glib::spawn_future_local(async move {
                            let img = gio::spawn_blocking(move || img::image_from_path(path))
                                .await
                                .ok()
                                .flatten();
                            let Some(img) = img else {
                                return;
                            };
                            *images.borrow_mut() = Some(Rc::new(vec![img]));
                            stale.set(false);
                            from_capture.set(true);
                            glib::timeout_add_local_once(
                                std::time::Duration::from_millis(80),
                                move || {
                                    if let Some(app) = gio::Application::default() {
                                        app.activate_action(
                                            "show-query",
                                            Some(&restore.to_variant()),
                                        );
                                    }
                                },
                            );
                        });
                    })
                })),
            }],
        }
    }
}

fn info_item(id: &str, title: &str, subtitle: &str, icon: &str) -> Item {
    Item {
        output: None,
        preview: None,
        body: None,
        id: format!("ocr:{id}"),
        title: title.into(),
        subtitle: Some(subtitle.into()),
        icon: Icon::Named(icon.into()),
        score: 10,
        actions: vec![Action {
            label: String::new(),
            kind: ActionKind::Callback(Rc::new(|| Ok(()))),
        }],
    }
}

/// Images from a pipe's input (`f scan.png ⇢ ocr`).
async fn piped_images(input: &str) -> Vec<Image> {
    let paths: Vec<PathBuf> = input
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(|l| l.strip_prefix("file://").unwrap_or(l))
        .map(crate::config::expand_home)
        .filter(|p| p.is_file())
        .collect();
    if paths.is_empty() {
        return Vec::new();
    }
    gio::spawn_blocking(move || paths.into_iter().filter_map(img::image_from_path).collect())
        .await
        .unwrap_or_default()
}

/// Whether `spec` looks like a language code (`ita`, `eng+ita`, `chi_sim`).
fn is_lang_spec(spec: &str) -> bool {
    !spec.is_empty()
        && !spec.contains(char::is_whitespace)
        && spec.split('+').all(|part| {
            (2..=8).contains(&part.len())
                && part.chars().all(|c| c.is_ascii_lowercase() || c == '_')
        })
}

#[async_trait(?Send)]
impl Provider for OcrProvider {
    fn id(&self) -> &'static str {
        "ocr"
    }

    fn title(&self) -> String {
        "OCR".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.ocr.clone()]
    }

    fn takes_input(&self, _verb: &str) -> bool {
        true
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        if crate::detect::which("tesseract").is_none() {
            let cmd = install_command(None);
            return vec![Item {
                output: None,
                preview: None,
                body: None,
                id: "ocr:missing".into(),
                title: "Install tesseract to read text from images".into(),
                subtitle: Some(cmd.clone()),
                icon: Icon::Named("dialog-warning-symbolic".into()),
                score: 100,
                actions: install_actions(&self.cfg.borrow(), &cmd),
            }];
        }

        let mut installed = self.available_langs(false).await;
        let typed = q.text.trim().to_lowercase();

        // Check if the user asked for a language that isn't installed yet.
        let lang = if typed.is_empty() {
            default_lang(&installed)
        } else if is_lang_spec(&typed) {
            if typed
                .split('+')
                .any(|part| !installed.is_empty() && !installed.iter().any(|l| l == part))
            {
                // Re-read once in case the language pack was just installed.
                installed = self.available_langs(true).await;
            }
            let missing = typed
                .split('+')
                .find(|part| !installed.is_empty() && !installed.iter().any(|l| l == part));
            if let Some(m) = missing {
                let cmd = install_command(Some(m));
                let have = if installed.is_empty() {
                    String::new()
                } else {
                    format!(" · installed: {}", installed.join(", "))
                };
                return vec![Item {
                    output: None,
                    preview: None,
                    body: None,
                    id: format!("ocr:lang:{m}"),
                    title: format!("Language \"{m}\" is not installed"),
                    subtitle: Some(format!("{cmd}{have}")),
                    icon: Icon::Named("dialog-warning-symbolic".into()),
                    score: 100,
                    actions: install_actions(&self.cfg.borrow(), &cmd),
                }];
            }
            typed.clone()
        } else {
            return vec![info_item(
                "usage",
                "Type a language code, or leave empty for the default",
                &format!(
                    "e.g. ocr · ocr ita · ocr eng+ita (installed: {})",
                    if installed.is_empty() {
                        "eng".into()
                    } else {
                        installed.join(", ")
                    }
                ),
                "dialog-information-symbolic",
            )];
        };

        let verb = self.cfg.borrow().verbs.ocr.clone();
        let restore = if typed.is_empty() {
            format!("{verb} ")
        } else {
            format!("{verb} {typed}")
        };

        let images: Rc<Vec<Image>> = if let Some(input) = q.input {
            Rc::new(piped_images(input).await)
        } else {
            self.clipboard_images(q.text.trim().is_empty()).await
        };

        if images.is_empty() {
            return vec![
                self.capture_item(&restore, 100),
                info_item(
                    "hint",
                    "Or copy an image or screenshot first",
                    "Copy an image in Files, from a browser, or with Print, then type ocr",
                    "image-x-generic-symbolic",
                ),
            ];
        }

        let item = match self.recognize(&images, &lang).await {
            Ok(text) if text.is_empty() => info_item(
                "empty",
                "No text found in the image",
                &format!("{} · {lang}", source_label(&images)),
                "dialog-information-symbolic",
            ),
            Ok(text) => {
                let lines = text.lines().count();
                let words = text.split_whitespace().count();
                let mut meta = vec![source_label(&images)];
                if lines > 1 {
                    meta.push(format!("{lines} lines"));
                }
                meta.push(format!("{words} word{}", if words == 1 { "" } else { "s" }));
                if !q.text.trim().is_empty() || lang != "eng" {
                    meta.push(lang.clone());
                }
                Item {
                    output: None,
                    preview: Some(build_preview(&images, &text, &lang)),
                    body: None,
                    id: "ocr:result".into(),
                    title: first_line(&text, 90),
                    subtitle: Some(meta.join(" · ")),
                    icon: Icon::Named("document-edit-symbolic".into()),
                    score: 100,
                    actions: text_actions(&text),
                }
            }
            Err(why) => info_item("error", "Couldn't read text from the image", &why, "dialog-warning-symbolic"),
        };

        vec![item, self.capture_item(&restore, 1)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_tesseract_lang_list() {
        let raw = "List of available languages in \"/usr/share/tesseract-ocr/5/tessdata/\" (3):\neng\nita\nosd\n";
        assert_eq!(parse_langs(raw), vec!["eng", "ita"]);
    }

    #[test]
    fn cleans_form_feeds_and_trailing_spaces() {
        assert_eq!(
            clean_ocr_text("  Hello world   \nSecond line  \n\x0c\n"),
            "Hello world\nSecond line"
        );
    }

    #[test]
    fn validates_language_specs() {
        assert!(is_lang_spec("eng"));
        assert!(is_lang_spec("eng+ita"));
        assert!(is_lang_spec("chi_sim"));
        assert!(!is_lang_spec(""));
        assert!(!is_lang_spec("eng ita"));
        assert!(!is_lang_spec("123"));
    }
}
