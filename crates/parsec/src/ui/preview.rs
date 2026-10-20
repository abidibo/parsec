//! Quick Look: the pane that slides in beside the results while the
//! selected row has something to show, and follows the selection.
//!
//! A row's `Preview` is resolved into a `Sheet` (picture or icon or swatch,
//! text, facts). Files are inspected off the main thread: images are
//! scaled down there, anything GNOME has thumbnailed shows its thumbnail,
//! text shows its first lines. Sheets are cached by item id until the
//! launcher is shown again, so moving back and forth costs nothing.

use crate::core::{Item, Preview, Sheet};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub const WIDTH: i32 = 360;
/// Tall enough for a picture even when only one result shows.
const MIN_HEIGHT: i32 = 320;
const PICTURE_HEIGHT: i32 = 220;
/// Scaled images are drawn at most this large (2x the pane, for HiDPI).
const IMAGE_MAX: i32 = 2 * WIDTH;
const TEXT_LINES: usize = 200;
const TEXT_BYTES: usize = 64 * 1024;
/// What the label is given at most; longer text is cut with a note.
const TEXT_SHOWN: usize = 20_000;
const DIR_ENTRIES: usize = 40;

pub struct PreviewPane {
    revealer: gtk::Revealer,
    picture: gtk::Picture,
    icon: gtk::Image,
    swatch: gtk::DrawingArea,
    colour: Rc<Cell<gdk::RGBA>>,
    text: gtk::Label,
    text_scroller: gtk::ScrolledWindow,
    facts: gtk::Grid,
    /// Bumped on every selection so a slow load can't paint a stale row.
    generation: Cell<u64>,
    cache: RefCell<HashMap<String, Rc<Sheet>>>,
}

impl PreviewPane {
    pub fn new() -> Rc<Self> {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .height_request(PICTURE_HEIGHT)
            .css_classes(["parsec-preview-picture"])
            .build();
        let icon = gtk::Image::builder()
            .pixel_size(96)
            .margin_top(28)
            .margin_bottom(20)
            .css_classes(["parsec-preview-icon"])
            .build();
        let colour = Rc::new(Cell::new(gdk::RGBA::BLACK));
        let swatch = gtk::DrawingArea::builder()
            .height_request(120)
            .css_classes(["parsec-preview-swatch"])
            .build();
        swatch.set_draw_func({
            let colour = colour.clone();
            move |_, cr, w, h| {
                let (w, h, r) = (w as f64, h as f64, 12.0);
                let c = colour.get();
                cr.new_sub_path();
                cr.arc(w - r, r, r, -90f64.to_radians(), 0.0);
                cr.arc(w - r, h - r, r, 0.0, 90f64.to_radians());
                cr.arc(r, h - r, r, 90f64.to_radians(), 180f64.to_radians());
                cr.arc(r, r, r, 180f64.to_radians(), 270f64.to_radians());
                cr.close_path();
                cr.set_source_rgba(
                    c.red() as f64,
                    c.green() as f64,
                    c.blue() as f64,
                    c.alpha() as f64,
                );
                let _ = cr.fill();
            }
        });
        let text = gtk::Label::builder()
            .wrap(true)
            .focus_on_click(false)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .xalign(0.0)
            .yalign(0.0)
            .valign(gtk::Align::Start)
            .css_classes(["parsec-preview-text"])
            .build();
        let text_scroller = gtk::ScrolledWindow::builder()
            .child(&text)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let facts = gtk::Grid::builder()
            .column_spacing(12)
            .row_spacing(4)
            .css_classes(["parsec-preview-facts"])
            .build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .width_request(WIDTH)
            .height_request(MIN_HEIGHT)
            .css_classes(["parsec-preview"])
            .build();
        content.append(&picture);
        content.append(&icon);
        content.append(&swatch);
        content.append(&text_scroller);
        content.append(&facts);
        let revealer = gtk::Revealer::builder()
            .transition_type(gtk::RevealerTransitionType::SlideLeft)
            .transition_duration(160)
            .child(&content)
            .reveal_child(false)
            .build();
        Rc::new(Self {
            revealer,
            picture,
            icon,
            swatch,
            colour,
            text,
            text_scroller,
            facts,
            generation: Cell::new(0),
            cache: RefCell::new(HashMap::new()),
        })
    }

    pub fn widget(&self) -> &gtk::Revealer {
        &self.revealer
    }

    /// Forget cached sheets: files may have changed since the last show.
    pub fn reset(&self) {
        self.cache.borrow_mut().clear();
        self.generation.set(self.generation.get() + 1);
        self.revealer.set_reveal_child(false);
    }

    /// Show the preview of `item`, or slide away if it has none.
    pub fn show_for(self: &Rc<Self>, item: Option<&Item>) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let Some((item, preview)) = item.and_then(|i| Some((i, i.preview.clone()?))) else {
            self.revealer.set_reveal_child(false);
            return;
        };
        if let Some(sheet) = self.cache.borrow().get(&item.id).cloned() {
            self.render(&sheet);
            return;
        }
        let id = item.id.clone();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let sheet = resolve(preview).await;
            if this.generation.get() != gen {
                return;
            }
            match sheet {
                Some(sheet) => {
                    let sheet = Rc::new(sheet);
                    this.cache.borrow_mut().insert(id, sheet.clone());
                    this.render(&sheet);
                }
                None => this.revealer.set_reveal_child(false),
            }
        });
    }

    fn render(&self, sheet: &Sheet) {
        self.picture.set_paintable(sheet.picture.as_ref());
        self.picture.set_visible(sheet.picture.is_some());
        let icon = sheet.icon.as_ref().filter(|_| sheet.picture.is_none());
        self.icon
            .set_from_gicon(icon.unwrap_or(&gio::ThemedIcon::new("text-x-generic").upcast()));
        self.icon.set_visible(icon.is_some());
        if let Some(c) = sheet.swatch {
            self.colour.set(c);
            self.swatch.queue_draw();
        }
        self.swatch.set_visible(sheet.swatch.is_some());

        let text = sheet.text.as_deref().unwrap_or("");
        let shown = shown_text(text);
        if sheet.markdown {
            self.text.set_markup(&super::markdown::to_markup(&shown));
        } else {
            self.text.set_text(&shown);
        }
        if sheet.mono {
            self.text.add_css_class("mono");
        } else {
            self.text.remove_css_class("mono");
        }
        self.text_scroller.set_visible(!text.is_empty());
        self.text_scroller.vadjustment().set_value(0.0);

        while let Some(c) = self.facts.first_child() {
            self.facts.remove(&c);
        }
        for (row, (key, value)) in sheet.facts.iter().enumerate() {
            let key = gtk::Label::builder()
                .label(key)
                .xalign(0.0)
                .css_classes(["parsec-preview-key"])
                .build();
            let value = gtk::Label::builder()
                .label(value)
                .xalign(0.0)
                .hexpand(true)
                .ellipsize(gtk::pango::EllipsizeMode::Middle)
                .css_classes(["parsec-preview-value"])
                .build();
            self.facts.attach(&key, 0, row as i32, 1, 1);
            self.facts.attach(&value, 1, row as i32, 1, 1);
        }
        self.facts.set_visible(!sheet.facts.is_empty());
        self.revealer.set_reveal_child(true);
    }
}

fn shown_text(text: &str) -> String {
    if text.len() <= TEXT_SHOWN {
        return text.to_string();
    }
    let mut end = TEXT_SHOWN;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n\n… {} more characters", &text[..end], text.len() - end)
}

async fn resolve(preview: Preview) -> Option<Sheet> {
    match preview {
        Preview::Sheet(sheet) => Some(sheet),
        Preview::Later(load) => load().await,
        Preview::File(path) => file_sheet(path).await,
    }
}

/// What the background thread learns about a file. Plain data, so it can
/// cross back to the main thread where textures and icons are made.
#[derive(Default)]
struct FileFacts {
    content_type: String,
    size: Option<u64>,
    modified: Option<u64>,
    /// PNG bytes: the image scaled down, or GNOME's thumbnail.
    picture: Option<Vec<u8>>,
    dimensions: Option<(i32, i32)>,
    text: Option<String>,
    /// For a folder: entry names (folders end in `/`) and the total count.
    entries: Option<(Vec<String>, usize)>,
}

pub async fn file_sheet(path: PathBuf) -> Option<Sheet> {
    let facts = gio::spawn_blocking(move || inspect(&path)).await.ok()??;
    let mut sheet = Sheet {
        picture: facts
            .picture
            .and_then(|png| gdk::Texture::from_bytes(&glib::Bytes::from_owned(png)).ok()),
        markdown: gio::content_type_is_a(&facts.content_type, "text/markdown"),
        ..Sheet::default()
    };
    sheet.mono = !sheet.markdown;
    if sheet.picture.is_none() {
        sheet.icon = Some(gio::content_type_get_icon(&facts.content_type));
    }
    let kind = gio::content_type_get_description(&facts.content_type).to_string();
    sheet.facts.push(("Type".into(), kind));
    if let Some((w, h)) = facts.dimensions {
        sheet
            .facts
            .push(("Dimensions".into(), format!("{w} × {h}")));
    }
    if let Some(size) = facts.size.filter(|_| facts.entries.is_none()) {
        sheet.facts.push(("Size".into(), human_size(size)));
    }
    if let Some((names, total)) = facts.entries {
        sheet.facts.push(("Items".into(), total.to_string()));
        let more = total.saturating_sub(names.len());
        let mut listing = names.join("\n");
        if more > 0 {
            listing.push_str(&format!("\n… {more} more"));
        }
        sheet.text = Some(listing);
    } else {
        sheet.text = facts.text;
    }
    if let Some(m) = facts.modified {
        let age = now().saturating_sub(m);
        sheet
            .facts
            .push(("Modified".into(), crate::providers::shell::ago(age)));
    }
    Some(sheet)
}

fn inspect(path: &Path) -> Option<FileFacts> {
    let file = gio::File::for_path(path);
    let info = file
        .query_info(
            "standard::content-type,standard::size,standard::type,time::modified,\
             thumbnail::path,thumbnail::is-valid",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let mut facts = FileFacts {
        content_type: info
            .content_type()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "application/octet-stream".into()),
        size: Some(info.size() as u64),
        modified: info.modification_date_time().map(|t| t.to_unix() as u64),
        ..FileFacts::default()
    };
    if info.file_type() == gio::FileType::Directory {
        facts.entries = Some(list_dir(path));
        return Some(facts);
    }
    let ct = facts.content_type.clone();
    if ct.starts_with("image/") {
        if let Some((_, w, h)) = gtk::gdk_pixbuf::Pixbuf::file_info(path) {
            facts.dimensions = Some((w, h));
        }
        facts.picture = scaled_png(path);
    }
    if facts.picture.is_none() && info.boolean("thumbnail::is-valid") {
        if let Some(thumb) = info.attribute_byte_string("thumbnail::path") {
            facts.picture = std::fs::read(thumb.as_str()).ok();
        }
    }
    if facts.picture.is_none() && gio::content_type_is_a(&ct, "text/plain") {
        facts.text = head(path);
    }
    Some(facts)
}

/// The image at most `IMAGE_MAX` on its longest side, as PNG.
fn scaled_png(path: &Path) -> Option<Vec<u8>> {
    let pixbuf =
        gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, IMAGE_MAX, IMAGE_MAX, true).ok()?;
    let pixbuf = pixbuf.apply_embedded_orientation().unwrap_or(pixbuf);
    pixbuf.save_to_bufferv("png", &[]).ok()
}

/// The first lines of a text file; `None` if it looks binary.
fn head(path: &Path) -> Option<String> {
    use std::io::Read;
    let mut buf = Vec::with_capacity(TEXT_BYTES);
    std::fs::File::open(path)
        .ok()?
        .take(TEXT_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.contains(&0) {
        return None;
    }
    let text = String::from_utf8_lossy(&buf);
    let mut lines: Vec<&str> = text.lines().take(TEXT_LINES + 1).collect();
    let cut = lines.len() > TEXT_LINES;
    lines.truncate(TEXT_LINES);
    let mut out = lines.join("\n");
    if cut {
        out.push_str("\n…");
    }
    Some(out)
}

/// Folder entries, folders first, hidden ones skipped, with the total.
fn list_dir(dir: &Path) -> (Vec<String>, usize) {
    let Ok(read) = std::fs::read_dir(dir) else {
        return (Vec::new(), 0);
    };
    let mut all: Vec<(bool, String)> = read
        .flatten()
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .map(|e| {
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            (is_dir, e.file_name().to_string_lossy().into_owned())
        })
        .collect();
    all.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    let total = all.len();
    let names = all
        .into_iter()
        .take(DIR_ENTRIES)
        .map(|(is_dir, name)| if is_dir { format!("{name}/") } else { name })
        .collect();
    (names, total)
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1000.0 && unit < UNITS.len() - 1 {
        size /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else if size < 10.0 {
        format!("{size:.1} {}", UNITS[unit])
    } else {
        format!("{size:.0} {}", UNITS[unit])
    }
}

/// A colour written the way CSS does, as a swatch; plain words like
/// "red" or "tan" are left alone, they are more often just words.
pub fn colour(text: &str) -> Option<gdk::RGBA> {
    let t = text.trim();
    let looks = t.starts_with('#') && matches!(t.len(), 4 | 5 | 7 | 9)
        || ["rgb(", "rgba(", "hsl(", "hsla("]
            .iter()
            .any(|p| t.starts_with(p) && t.ends_with(')'));
    looks.then(|| gdk::RGBA::parse(t).ok()).flatten()
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_like_files() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1_500), "1.5 KB");
        assert_eq!(human_size(24_000), "24 KB");
        assert_eq!(human_size(3_200_000_000), "3.2 GB");
    }

    #[test]
    fn colours_only_in_css_notation() {
        assert!(colour("#8b7cff").is_some());
        assert!(colour(" rgb(10, 20, 30) ").is_some());
        assert!(colour("red").is_none());
        assert!(colour("#zzzzzz").is_none());
        assert!(colour("#issue-42").is_none());
    }

    #[test]
    fn long_text_is_cut_with_a_note() {
        let long = "é".repeat(TEXT_SHOWN);
        let shown = shown_text(&long);
        assert!(shown.ends_with("more characters"));
        assert!(shown.len() < long.len() + 40);
    }

    #[test]
    fn text_files_show_their_head_and_binaries_nothing() {
        let dir = std::env::temp_dir().join(format!("parsec-preview-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let text = dir.join("a.txt");
        std::fs::write(
            &text,
            (0..300).map(|i| format!("line {i}\n")).collect::<String>(),
        )
        .unwrap();
        let shown = head(&text).unwrap();
        assert!(shown.starts_with("line 0\n"));
        assert!(shown.ends_with("line 199\n…"));
        let bin = dir.join("b.bin");
        std::fs::write(&bin, [0u8, 1, 2]).unwrap();
        assert!(head(&bin).is_none());
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        let (names, total) = list_dir(&dir);
        assert_eq!(names[0], "sub/");
        assert_eq!(total, 3);
        let _ = std::fs::remove_dir_all(dir);
    }
}
