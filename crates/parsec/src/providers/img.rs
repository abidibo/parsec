//! `img`: resize, convert, crop, trim, rotate or clean the images you
//! copied in Files (or a copied path to one). Enter saves the results next
//! to the originals, never over them; Tab copies the result to the
//! clipboard when there is a single image. An image itself on the
//! clipboard (a screenshot) works the other way round: Enter puts the
//! result back on the clipboard, Tab saves it to Pictures.
//!
//!   img 50%  · img 800 · img x600 · img 800x600     resize (fit inside)
//!   img png  · img jpg · img webp                   convert
//!   img crop 1:1 · img crop 1200x630                crop centred
//!   img trim · img rotate [left|180] · img flip [v] · img strip
//!
//! Steps combine in the order typed: `img crop 1:1 50% webp`. Everything
//! goes through gdk-pixbuf, which GTK already brings: PNG and JPEG work
//! everywhere, WebP where its loader is installed. Re-encoding drops the
//! metadata (location, camera), which is what `strip` is for. Phone photos
//! are turned upright from their EXIF orientation first.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use anyhow::{Context, Result};
use async_trait::async_trait;
use gtk::gdk;
use gtk::gdk_pixbuf::{Colorspace, InterpType, Pixbuf, PixbufRotation};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::Rc;

const QUALITY: &str = "85";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Format {
    Png,
    Jpeg,
    Webp,
    Tiff,
    Bmp,
}

impl Format {
    fn from_word(w: &str) -> Option<Self> {
        Some(match w {
            "png" => Self::Png,
            "jpg" | "jpeg" => Self::Jpeg,
            "webp" => Self::Webp,
            "tif" | "tiff" => Self::Tiff,
            "bmp" => Self::Bmp,
            _ => return None,
        })
    }

    /// gdk-pixbuf's name for the saver.
    fn pixbuf_name(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpeg",
            Self::Webp => "webp",
            Self::Tiff => "tiff",
            Self::Bmp => "bmp",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            other => other.pixbuf_name(),
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Png => "PNG",
            Self::Jpeg => "JPEG",
            Self::Webp => "WebP",
            Self::Tiff => "TIFF",
            Self::Bmp => "BMP",
        }
    }

    fn mime(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Webp => "image/webp",
            Self::Tiff => "image/tiff",
            Self::Bmp => "image/bmp",
        }
    }

    fn options(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Self::Jpeg | Self::Webp => &[("quality", QUALITY)],
            _ => &[],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// Largest centred area with this width:height ratio.
    CropRatio(u32, u32),
    /// Centred crop to this shape, then scaled to exactly this size.
    CropExact(u32, u32),
    Trim,
    /// Quarter turns clockwise: 1, 2 or 3.
    Rotate(u8),
    Flip {
        horizontal: bool,
    },
    Percent(u32),
    Width(u32),
    Height(u32),
    /// Scale down to fit inside, keeping the shape.
    Fit(u32, u32),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Plan {
    steps: Vec<Step>,
    format: Option<Format>,
    strip: bool,
}

impl Plan {
    fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.format.is_none() && !self.strip
    }
}

fn pair(word: &str, sep: char) -> Option<(u32, u32)> {
    let (a, b) = word.split_once(sep)?;
    let (a, b) = (a.parse().ok()?, b.parse().ok()?);
    (a > 0 && b > 0).then_some((a, b))
}

fn positive(word: &str) -> Option<u32> {
    word.parse().ok().filter(|n| *n > 0)
}

/// Read what was typed after the verb into steps and a format.
fn parse(text: &str) -> Result<Plan, String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_lowercase).collect();
    let mut plan = Plan::default();
    let mut i = 0;
    while i < words.len() {
        let w = words[i].as_str();
        let next = words.get(i + 1).map(String::as_str);
        let mut take_next = false;
        match w {
            "crop" => {
                let arg = next.ok_or("crop needs a ratio or a size: crop 1:1, crop 1200x630")?;
                take_next = true;
                if let Some((a, b)) = pair(arg, ':') {
                    plan.steps.push(Step::CropRatio(a, b));
                } else if let Some((a, b)) = pair(arg, 'x') {
                    plan.steps.push(Step::CropExact(a, b));
                } else {
                    return Err(format!("crop {arg}? Try crop 1:1 or crop 1200x630"));
                }
            }
            "trim" => plan.steps.push(Step::Trim),
            "rotate" => {
                let turns = match next {
                    Some("left" | "-90" | "270") => Some(3),
                    Some("right" | "90") => Some(1),
                    Some("180") => Some(2),
                    _ => None,
                };
                take_next = turns.is_some();
                plan.steps.push(Step::Rotate(turns.unwrap_or(1)));
            }
            "flip" => {
                let horizontal = match next {
                    Some("v" | "vertical") => Some(false),
                    Some("h" | "horizontal") => Some(true),
                    _ => None,
                };
                take_next = horizontal.is_some();
                plan.steps.push(Step::Flip {
                    horizontal: horizontal.unwrap_or(true),
                });
            }
            "strip" => plan.strip = true,
            _ => {
                if let Some(f) = Format::from_word(w) {
                    plan.format = Some(f);
                } else if let Some(p) = w.strip_suffix('%').and_then(positive) {
                    plan.steps.push(Step::Percent(p));
                } else if let Some(h) = w
                    .strip_prefix('x')
                    .or_else(|| w.strip_suffix('h'))
                    .and_then(positive)
                {
                    plan.steps.push(Step::Height(h));
                } else if let Some(wd) = positive(w.strip_suffix('w').unwrap_or(w)) {
                    plan.steps.push(Step::Width(wd));
                } else if let Some((a, b)) = pair(w, 'x') {
                    plan.steps.push(Step::Fit(a, b));
                } else {
                    return Err(format!("Don't know \u{201c}{w}\u{201d}"));
                }
            }
        }
        i += if take_next { 2 } else { 1 };
    }
    Ok(plan)
}

/// What the plan does, for a row title.
fn describe(plan: &Plan) -> String {
    let mut parts: Vec<String> = plan
        .steps
        .iter()
        .map(|s| match *s {
            Step::CropRatio(a, b) if a == b => "Square crop".into(),
            Step::CropRatio(a, b) => format!("Crop to {a}:{b}"),
            Step::CropExact(a, b) => format!("Crop to {a}×{b}"),
            Step::Trim => "Trim borders".into(),
            Step::Rotate(1) => "Rotate right".into(),
            Step::Rotate(3) => "Rotate left".into(),
            Step::Rotate(_) => "Turn upside down".into(),
            Step::Flip { horizontal: true } => "Flip".into(),
            Step::Flip { horizontal: false } => "Flip vertically".into(),
            Step::Percent(50) => "Half size".into(),
            Step::Percent(p) => format!("{p}% size"),
            Step::Width(w) => format!("{w} px wide"),
            Step::Height(h) => format!("{h} px high"),
            Step::Fit(w, h) => format!("Fit in {w}×{h}"),
        })
        .collect();
    if let Some(f) = plan.format {
        parts.push(format!("as {}", f.label()));
    }
    if plan.strip && parts.is_empty() {
        parts.push("Remove metadata (location, camera)".into());
    }
    let lower = |s: &String| {
        let mut c = s.chars();
        c.next()
            .map(|f| f.to_lowercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    };
    let mut out = parts.iter().map(lower).collect::<Vec<_>>().join(", ");
    if let Some(first) = out.get(..1) {
        out = first.to_uppercase() + &out[1..];
    }
    out
}

/// A short tag for the output file name: `photo-1x1-50.jpg`.
fn suffix(plan: &Plan) -> String {
    let mut parts: Vec<String> = plan
        .steps
        .iter()
        .map(|s| match *s {
            Step::CropRatio(a, b) => format!("{a}x{b}"),
            Step::CropExact(a, b) => format!("{a}x{b}"),
            Step::Trim => "trim".into(),
            Step::Rotate(1) => "rot".into(),
            Step::Rotate(3) => "rotl".into(),
            Step::Rotate(_) => "rot180".into(),
            Step::Flip { horizontal: true } => "flip".into(),
            Step::Flip { horizontal: false } => "flipv".into(),
            Step::Percent(p) => p.to_string(),
            Step::Width(w) => format!("{w}w"),
            Step::Height(h) => format!("{h}h"),
            Step::Fit(w, h) => format!("{w}x{h}"),
        })
        .collect();
    if parts.is_empty() && plan.format.is_none() {
        parts.push("clean".into());
    }
    parts.join("-")
}

/// Size after the steps, from the size before; `None` once trimmed, as
/// that depends on the pixels.
fn result_size(mut w: u32, mut h: u32, plan: &Plan) -> Option<(u32, u32)> {
    let scale = |w: u32, h: u32, nw: u32| -> (u32, u32) {
        let nh = ((h as f64) * (nw as f64) / (w as f64)).round().max(1.0) as u32;
        (nw, nh)
    };
    for step in &plan.steps {
        (w, h) = match *step {
            Step::CropRatio(a, b) => crop_box(w, h, a, b).map(|(_, _, cw, ch)| (cw, ch))?,
            Step::CropExact(a, b) => (a, b),
            Step::Trim => return None,
            Step::Rotate(1 | 3) => (h, w),
            Step::Rotate(_) | Step::Flip { .. } => (w, h),
            Step::Percent(p) => scale(w, h, ((w as u64 * p as u64) / 100).max(1) as u32),
            Step::Width(nw) => scale(w, h, nw),
            Step::Height(nh) => {
                let (nh2, nw) = scale(h, w, nh);
                (nw, nh2)
            }
            Step::Fit(bw, bh) => {
                let f = (bw as f64 / w as f64).min(bh as f64 / h as f64).min(1.0);
                (
                    ((w as f64) * f).round().max(1.0) as u32,
                    ((h as f64) * f).round().max(1.0) as u32,
                )
            }
        };
    }
    Some((w, h))
}

/// The largest centred `a:b` rectangle inside `w`×`h`: (x, y, width, height).
fn crop_box(w: u32, h: u32, a: u32, b: u32) -> Option<(u32, u32, u32, u32)> {
    let (w64, h64, a64, b64) = (w as u64, h as u64, a as u64, b as u64);
    let (cw, ch) = if w64 * b64 > h64 * a64 {
        ((h64 * a64 / b64) as u32, h)
    } else {
        (w, (w64 * b64 / a64) as u32)
    };
    (cw > 0 && ch > 0).then(|| ((w - cw) / 2, (h - ch) / 2, cw, ch))
}

/// Bounding box of everything that differs from the top-left pixel by
/// more than a little: (x, y, width, height), or `None` if all of it is
/// background.
fn trim_box(
    pixels: &[u8],
    width: usize,
    height: usize,
    stride: usize,
    channels: usize,
) -> Option<(usize, usize, usize, usize)> {
    const TOLERANCE: i16 = 12;
    let bg = &pixels[..channels];
    let differs = |x: usize, y: usize| {
        let p = &pixels[y * stride + x * channels..][..channels];
        p.iter()
            .zip(bg)
            .any(|(a, b)| (*a as i16 - *b as i16).abs() > TOLERANCE)
    };
    let (mut x0, mut y0, mut x1, mut y1) = (width, height, 0, 0);
    for y in 0..height {
        for x in 0..width {
            if differs(x, y) {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
        }
    }
    (x0 <= x1 && y0 <= y1).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

// ---------------------------------------------------------------- pixels
// Everything below touches Pixbufs, which aren't Send: call it inside one
// worker thread and pass back only paths and bytes.

fn sized(p: &Pixbuf, w: u32, h: u32) -> Result<Pixbuf> {
    p.scale_simple(w as i32, h as i32, InterpType::Hyper)
        .context("scaling failed")
}

fn apply(mut p: Pixbuf, plan: &Plan) -> Result<Pixbuf> {
    for step in &plan.steps {
        let (w, h) = (p.width() as u32, p.height() as u32);
        p = match *step {
            Step::CropRatio(a, b) => {
                let (x, y, cw, ch) = crop_box(w, h, a, b).context("image too small")?;
                p.new_subpixbuf(x as i32, y as i32, cw as i32, ch as i32)
            }
            Step::CropExact(a, b) => {
                let (x, y, cw, ch) = crop_box(w, h, a, b).context("image too small")?;
                sized(
                    &p.new_subpixbuf(x as i32, y as i32, cw as i32, ch as i32),
                    a,
                    b,
                )?
            }
            Step::Trim => {
                let bytes = p.read_pixel_bytes();
                match trim_box(
                    &bytes,
                    w as usize,
                    h as usize,
                    p.rowstride() as usize,
                    p.n_channels() as usize,
                ) {
                    Some((x, y, tw, th)) => {
                        p.new_subpixbuf(x as i32, y as i32, tw as i32, th as i32)
                    }
                    None => p,
                }
            }
            Step::Rotate(turns) => {
                let angle = match turns {
                    1 => PixbufRotation::Clockwise,
                    3 => PixbufRotation::Counterclockwise,
                    _ => PixbufRotation::Upsidedown,
                };
                p.rotate_simple(angle).context("rotating failed")?
            }
            Step::Flip { horizontal } => p.flip(horizontal).context("flipping failed")?,
            Step::Percent(_) | Step::Width(_) | Step::Height(_) | Step::Fit(..) => {
                let single = Plan {
                    steps: vec![*step],
                    ..Plan::default()
                };
                let (nw, nh) = result_size(w, h, &single).context("bad size")?;
                if (nw, nh) == (w, h) {
                    p
                } else {
                    sized(&p, nw, nh)?
                }
            }
        };
    }
    Ok(p)
}

/// JPEG has no transparency: put transparent images on white first.
fn flatten(p: &Pixbuf) -> Result<Pixbuf> {
    if !p.has_alpha() {
        return Ok(p.clone());
    }
    let out = Pixbuf::new(Colorspace::Rgb, false, 8, p.width(), p.height())
        .context("allocating image")?;
    out.fill(0xffff_ffff);
    p.composite(
        &out,
        0,
        0,
        p.width(),
        p.height(),
        0.0,
        0.0,
        1.0,
        1.0,
        InterpType::Nearest,
        255,
    );
    Ok(out)
}

fn load(img: &Image, plan: &Plan) -> Result<Pixbuf> {
    let p = match &img.source {
        Source::File(path) => {
            Pixbuf::from_file(path).with_context(|| format!("reading {}", path.display()))?
        }
        Source::Data(bytes) => {
            let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from(&bytes[..]));
            Pixbuf::from_stream(&stream, gio::Cancellable::NONE)
                .context("reading the clipboard image")?
        }
    };
    let p = p.apply_embedded_orientation().unwrap_or(p);
    apply(p, plan)
}

fn ready_for(p: Pixbuf, format: Format) -> Result<Pixbuf> {
    if format == Format::Jpeg {
        flatten(&p)
    } else {
        Ok(p)
    }
}

/// Formats this machine can write.
fn writable() -> Vec<Format> {
    Pixbuf::formats()
        .iter()
        .filter(|f| f.is_writable())
        .filter_map(|f| Format::from_word(f.name()?.as_str()))
        .collect()
}

/// The format to save `src` in: the one asked for, else its own when it
/// can be written, else PNG.
fn target_format(src_format: Option<&str>, plan: &Plan, can: &[Format]) -> Format {
    plan.format
        .or_else(|| {
            src_format
                .and_then(Format::from_word)
                .filter(|f| can.contains(f))
        })
        .unwrap_or(Format::Png)
}

/// Where a result goes: next to the original file, or in Pictures for a
/// clipboard image, named after the time.
fn output_base(img: &Image) -> (PathBuf, String) {
    match &img.source {
        Source::File(path) => (
            path.parent().unwrap_or(Path::new(".")).to_path_buf(),
            path.file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "image".into()),
        ),
        Source::Data(_) => {
            let dir = glib::user_special_dir(glib::UserDirectory::Pictures)
                .or_else(dirs::picture_dir)
                .or_else(dirs::home_dir)
                .unwrap_or_else(|| PathBuf::from("."));
            let stamp = glib::DateTime::now_local()
                .and_then(|d| d.format("%Y%m%d-%H%M%S"))
                .map(|s| s.to_string())
                .unwrap_or_default();
            (dir, format!("clipboard-{stamp}"))
        }
    }
}

/// `photo.jpg` → `photo-50.jpg`, or `photo-50-2.jpg` if that's taken.
fn output_path(dir: &Path, stem: String, plan: &Plan, format: Format) -> PathBuf {
    let tag = suffix(plan);
    let base = if tag.is_empty() {
        stem
    } else {
        format!("{stem}-{tag}")
    };
    let mut path = dir.join(format!("{base}.{}", format.ext()));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{base}-{n}.{}", format.ext()));
        n += 1;
    }
    path
}

/// Process every image and save the results. Blocking.
fn save_all(images: &[Image], plan: &Plan) -> Result<Vec<PathBuf>> {
    let can = writable();
    let mut out = Vec::new();
    for img in images {
        let format = target_format(img.format.as_deref(), plan, &can);
        let p = ready_for(load(img, plan)?, format)?;
        let (dir, stem) = output_base(img);
        let dest = output_path(&dir, stem, plan, format);
        p.savev(&dest, format.pixbuf_name(), format.options())
            .with_context(|| format!("saving {}", dest.display()))?;
        out.push(dest);
    }
    Ok(out)
}

/// Process one image into encoded bytes and their media type. Blocking.
fn encode(img: &Image, plan: &Plan) -> Result<(Vec<u8>, &'static str)> {
    let format = target_format(img.format.as_deref(), plan, &writable());
    let p = ready_for(load(img, plan)?, format)?;
    let bytes = p
        .save_to_bufferv(format.pixbuf_name(), format.options())
        .context("encoding failed")?;
    Ok((bytes, format.mime()))
}

// ---------------------------------------------------------------- input

#[derive(Debug, Clone)]
enum Source {
    File(PathBuf),
    /// Image data on the clipboard, as PNG.
    Data(std::sync::Arc<Vec<u8>>),
}

#[derive(Debug, Clone)]
struct Image {
    source: Source,
    /// gdk-pixbuf's name for the format it was read as.
    format: Option<String>,
    width: u32,
    height: u32,
}

impl Image {
    fn name(&self) -> String {
        match &self.source {
            Source::File(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            Source::Data(_) => "Clipboard image".into(),
        }
    }

    fn is_data(&self) -> bool {
        matches!(self.source, Source::Data(_))
    }
}

/// Paths on the clipboard: files copied in Files, or a copied path.
async fn copied_paths() -> Vec<PathBuf> {
    let Some(display) = gdk::Display::default() else {
        return Vec::new();
    };
    let clipboard = display.clipboard();
    if let Ok(value) = clipboard
        .read_value_future(gdk::FileList::static_type(), glib::Priority::DEFAULT)
        .await
    {
        if let Ok(list) = value.get::<gdk::FileList>() {
            let paths: Vec<PathBuf> = list.files().iter().filter_map(|f| f.path()).collect();
            if !paths.is_empty() {
                return paths;
            }
        }
    }
    let text = super::clipboard_text().await.unwrap_or_default();
    text.lines()
        .map(|l| l.trim())
        .map(|l| l.strip_prefix("file://").unwrap_or(l))
        .map(crate::config::expand_home)
        .filter(|p| p.is_absolute() && p.is_file())
        .collect()
}

/// An image itself on the clipboard (a screenshot, a copied picture).
async fn copied_image_data() -> Option<Image> {
    let clipboard = gdk::Display::default()?.clipboard();
    let texture = clipboard.read_texture_future().await.ok()??;
    let (width, height) = (texture.width() as u32, texture.height() as u32);
    // Encoding a big screenshot takes a while: not on the UI thread.
    let png = gio::spawn_blocking(move || texture.save_to_png_bytes().to_vec())
        .await
        .ok()?;
    Some(Image {
        source: Source::Data(std::sync::Arc::new(png)),
        format: Some("png".into()),
        width,
        height,
    })
}

/// The copied images: files that are images, with their size from the
/// header, or else image data.
async fn copied_images() -> Vec<Image> {
    let paths = copied_paths().await;
    let files: Vec<Image> = gio::spawn_blocking(move || {
        paths
            .into_iter()
            .filter_map(|path| {
                let (fmt, w, h) = Pixbuf::file_info(&path)?;
                Some(Image {
                    format: fmt.name().map(|n| n.to_string()),
                    width: w as u32,
                    height: h as u32,
                    source: Source::File(path),
                })
            })
            .collect()
    })
    .await
    .unwrap_or_default();
    if !files.is_empty() {
        return files;
    }
    copied_image_data().await.into_iter().collect()
}

// ---------------------------------------------------------------- output

fn notify(title: &str, body: &str) {
    if let Some(app) = gio::Application::default() {
        let n = gio::Notification::new(title);
        n.set_body(Some(body));
        app.send_notification(None, &n);
    }
}

/// Image bytes on the clipboard: `wl-copy` keeps serving them after the
/// launcher hides, GTK is the fallback.
fn copy_image(bytes: Vec<u8>, mime: &str) -> Result<()> {
    if crate::detect::which("wl-copy").is_some() {
        use std::io::Write;
        let mut child = std::process::Command::new("wl-copy")
            .args(["--type", mime])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .context("spawning wl-copy")?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(&bytes).context("writing to wl-copy")?;
        }
        return Ok(());
    }
    let texture = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))?;
    gdk::Display::default()
        .context("no display")?
        .clipboard()
        .set_texture(&texture);
    Ok(())
}

fn save_action(images: Rc<Vec<Image>>, plan: Plan) -> Action {
    let to_pictures = images.iter().all(Image::is_data);
    Action {
        label: if to_pictures {
            "Save to Pictures".into()
        } else {
            "Save next to the originals".into()
        },
        kind: ActionKind::Callback(Rc::new(move || {
            let (images, plan) = ((*images).clone(), plan.clone());
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || {
                    save_all(&images, &plan).map_err(|e| format!("{e:#}"))
                })
                .await
                .unwrap_or_else(|_| Err("image thread failed".into()));
                match result {
                    Ok(paths) => {
                        let names: Vec<String> = paths
                            .iter()
                            .filter_map(|p| p.file_name())
                            .map(|n| n.to_string_lossy().into_owned())
                            .collect();
                        let title = match paths.len() {
                            1 => "Image saved".to_string(),
                            n => format!("{n} images saved"),
                        };
                        notify(&title, &names.join(", "));
                    }
                    Err(e) => notify("Couldn't process the images", &e),
                }
            });
            Ok(())
        })),
    }
}

fn copy_action(image: Image, plan: Plan) -> Action {
    Action {
        label: "Copy to the clipboard".into(),
        kind: ActionKind::Callback(Rc::new(move || {
            let (image, plan) = (image.clone(), plan.clone());
            glib::spawn_future_local(async move {
                let result = gio::spawn_blocking(move || {
                    encode(&image, &plan).map_err(|e| format!("{e:#}"))
                })
                .await
                .unwrap_or_else(|_| Err("image thread failed".into()));
                match result.and_then(|(b, mime)| copy_image(b, mime).map_err(|e| format!("{e:#}")))
                {
                    Ok(()) => {}
                    Err(e) => notify("Couldn't copy the image", &e),
                }
            });
            Ok(())
        })),
    }
}

/// "Open in GIMP": the first app that edits this kind of image and isn't
/// the one that opens it by default (usually a viewer).
fn editor_action(image: &Image) -> Option<Action> {
    let Source::File(path) = &image.source else {
        return None;
    };
    let mime = image
        .format
        .as_deref()
        .and_then(Format::from_word)
        .map(Format::mime)
        .unwrap_or("image/png");
    let default = gio::AppInfo::default_for_type(mime, false).and_then(|a| a.id());
    let app = gio::AppInfo::recommended_for_type(mime)
        .into_iter()
        .find(|a| a.id() != default)?;
    let file = gio::File::for_path(path);
    Some(Action {
        label: format!("Open in {}", app.name()),
        kind: ActionKind::Callback(Rc::new(move || {
            let ctx = crate::core::scope::launch_context();
            app.launch(std::slice::from_ref(&file), ctx.as_ref())?;
            Ok(())
        })),
    })
}

fn info(id: &str, title: &str, subtitle: &str, icon: &str) -> Item {
    Item {
        output: None,
        preview: None,
        id: format!("img:{id}"),
        title: title.into(),
        subtitle: Some(subtitle.into()),
        body: None,
        icon: Icon::Named(icon.into()),
        score: 1,
        actions: vec![Action {
            label: String::new(),
            kind: ActionKind::Callback(Rc::new(|| Ok(()))),
        }],
    }
}

/// One row: what the plan does to the copied images, and its actions.
fn plan_item(images: &Rc<Vec<Image>>, plan: Plan, can: &[Format], score: u32) -> Item {
    if let Some(f) = plan.format.filter(|f| !can.contains(f)) {
        let hint = if f == Format::Webp {
            "Install webp-pixbuf-loader to write WebP"
        } else {
            "No saver for it in gdk-pixbuf"
        };
        return info(
            "unwritable",
            &format!("This system can't write {}", f.label()),
            hint,
            "dialog-warning-symbolic",
        );
    }
    let subtitle = match images.as_slice() {
        [one] => {
            let name = one.name();
            let format = target_format(one.format.as_deref(), &plan, can);
            let size = match result_size(one.width, one.height, &plan) {
                Some((w, h)) => format!("{w}×{h}"),
                None => "trimmed".into(),
            };
            format!(
                "{name} · {}×{} → {size} {}",
                one.width,
                one.height,
                format.label()
            )
        }
        many => format!("{} images, saved next to each", many.len()),
    };
    let save = save_action(images.clone(), plan.clone());
    let actions = match images.as_slice() {
        // From the clipboard, back to the clipboard; saving is the extra.
        [one] if one.is_data() => vec![copy_action(one.clone(), plan.clone()), save],
        [one] => {
            let mut actions = vec![save, copy_action(one.clone(), plan.clone())];
            actions.extend(editor_action(one));
            actions
        }
        _ => vec![save],
    };
    Item {
        output: None,
        preview: None,
        id: format!("img:{}", suffix(&plan)),
        title: describe(&plan),
        subtitle: Some(subtitle),
        body: None,
        icon: Icon::Named("image-x-generic-symbolic".into()),
        score,
        actions,
    }
}

/// What `img` alone offers.
const PRESETS: &[&str] = &[
    "50%",
    "jpg",
    "png",
    "webp",
    "crop 1:1",
    "trim",
    "rotate",
    "rotate left",
    "strip",
];

pub struct ImgProvider {
    cfg: SharedConfig,
    /// What the clipboard held at the last read, kept while typing.
    cache: RefCell<Option<Rc<Vec<Image>>>>,
    /// Set when the clipboard changes; watching starts on first use.
    stale: Rc<Cell<bool>>,
    watching: Cell<bool>,
}

impl ImgProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            cache: RefCell::new(None),
            stale: Rc::new(Cell::new(true)),
            watching: Cell::new(false),
        }
    }

    /// The copied images, read again only when the clipboard changed or
    /// the verb was just typed, so typing the steps stays instant.
    async fn images(&self, fresh: bool) -> Rc<Vec<Image>> {
        if !self.watching.replace(true) {
            if let Some(display) = gdk::Display::default() {
                let stale = self.stale.clone();
                display
                    .clipboard()
                    .connect_changed(move |_| stale.set(true));
            }
        }
        if !fresh && !self.stale.get() {
            if let Some(cached) = self.cache.borrow().clone() {
                return cached;
            }
        }
        self.stale.set(false);
        let images = Rc::new(copied_images().await);
        *self.cache.borrow_mut() = Some(images.clone());
        images
    }
}

#[async_trait(?Send)]
impl Provider for ImgProvider {
    fn id(&self) -> &'static str {
        "img"
    }

    fn title(&self) -> String {
        "Images".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.img.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let images = self.images(q.text.trim().is_empty()).await;
        if images.is_empty() {
            return vec![info(
                "none",
                "Copy some images in Files first",
                "then: img 50% · img webp · img crop 1:1 · img trim",
                "image-x-generic-symbolic",
            )];
        }
        let can = writable();
        match parse(q.text) {
            Ok(plan) if plan.is_empty() => {
                let total = PRESETS.len() as u32;
                PRESETS
                    .iter()
                    .enumerate()
                    .filter_map(|(i, p)| parse(p).ok().map(|plan| (i, plan)))
                    .map(|(i, plan)| plan_item(&images, plan, &can, total - i as u32))
                    .collect()
            }
            Ok(plan) => vec![plan_item(&images, plan, &can, 1)],
            Err(why) => vec![info(
                "error",
                &why,
                "img 50% · 800 · x600 · 800x600 · png · jpg · webp · crop 1:1 · trim · rotate · flip · strip",
                "dialog-warning-symbolic",
            )],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_combined_steps_in_order() {
        let plan = parse("crop 1:1 50% webp").unwrap();
        assert_eq!(plan.steps, [Step::CropRatio(1, 1), Step::Percent(50)]);
        assert_eq!(plan.format, Some(Format::Webp));
        assert_eq!(describe(&plan), "Square crop, half size, as WebP");
        assert_eq!(suffix(&plan), "1x1-50");
    }

    #[test]
    fn parses_sizes() {
        assert_eq!(parse("800").unwrap().steps, [Step::Width(800)]);
        assert_eq!(parse("800w").unwrap().steps, [Step::Width(800)]);
        assert_eq!(parse("x600").unwrap().steps, [Step::Height(600)]);
        assert_eq!(parse("600h").unwrap().steps, [Step::Height(600)]);
        assert_eq!(parse("800x600").unwrap().steps, [Step::Fit(800, 600)]);
        assert_eq!(
            parse("crop 1200x630").unwrap().steps,
            [Step::CropExact(1200, 630)]
        );
        assert_eq!(parse("rotate left").unwrap().steps, [Step::Rotate(3)]);
        assert_eq!(
            parse("flip v").unwrap().steps,
            [Step::Flip { horizontal: false }]
        );
        assert!(parse("crop").is_err());
        assert!(parse("crop banana").is_err());
        assert!(parse("blur").is_err());
        assert!(parse("0%").is_err());
    }

    #[test]
    fn strip_alone_gets_a_name() {
        let plan = parse("strip").unwrap();
        assert_eq!(describe(&plan), "Remove metadata (location, camera)");
        assert_eq!(suffix(&plan), "clean");
        assert_eq!(suffix(&parse("png").unwrap()), "");
    }

    #[test]
    fn sizes_follow_the_steps() {
        let size = |t: &str| result_size(4000, 3000, &parse(t).unwrap());
        assert_eq!(size("50%"), Some((2000, 1500)));
        assert_eq!(size("800"), Some((800, 600)));
        assert_eq!(size("x300"), Some((400, 300)));
        assert_eq!(size("1000x1000"), Some((1000, 750)));
        assert_eq!(size("8000x8000"), Some((4000, 3000)), "fit never enlarges");
        assert_eq!(size("crop 1:1"), Some((3000, 3000)));
        assert_eq!(size("crop 1200x630"), Some((1200, 630)));
        assert_eq!(size("rotate"), Some((3000, 4000)));
        assert_eq!(size("trim"), None);
    }

    #[test]
    fn crop_box_is_centred() {
        assert_eq!(crop_box(4000, 3000, 1, 1), Some((500, 0, 3000, 3000)));
        assert_eq!(crop_box(3000, 4000, 16, 9), Some((0, 1156, 3000, 1687)));
    }

    #[test]
    fn trim_finds_the_content() {
        // 6×4 RGB, white with a black 2×2 square at (2, 1).
        let (w, h, ch) = (6, 4, 3);
        let mut px = vec![255u8; w * h * ch];
        for (x, y) in [(2, 1), (3, 1), (2, 2), (3, 2)] {
            px[(y * w + x) * ch..][..ch].fill(0);
        }
        assert_eq!(trim_box(&px, w, h, w * ch, ch), Some((2, 1, 2, 2)));
        let blank = vec![255u8; w * h * ch];
        assert_eq!(trim_box(&blank, w, h, w * ch, ch), None);
    }

    #[test]
    fn outputs_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("parsec-img-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let stem = || "photo".to_string();
        let plan = parse("50%").unwrap();
        assert_eq!(
            output_path(&dir, stem(), &plan, Format::Jpeg),
            dir.join("photo-50.jpg")
        );
        std::fs::write(dir.join("photo-50.jpg"), b"").unwrap();
        assert_eq!(
            output_path(&dir, stem(), &plan, Format::Jpeg),
            dir.join("photo-50-2.jpg")
        );
        assert_eq!(
            output_path(&dir, stem(), &parse("webp").unwrap(), Format::Webp),
            dir.join("photo.webp")
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn target_format_falls_back_to_png() {
        let can = [Format::Png, Format::Jpeg];
        let plan = Plan::default();
        assert_eq!(target_format(Some("jpeg"), &plan, &can), Format::Jpeg);
        assert_eq!(target_format(Some("gif"), &plan, &can), Format::Png);
        assert_eq!(target_format(Some("webp"), &plan, &can), Format::Png);
    }
}
