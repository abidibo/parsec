//! The single launcher window: a search bar with a mode chip, results with
//! highlighted matches, icon tiles and section headers, a context footer.
//! Created once at startup and toggled, never destroyed.
//!
//! Look: a near-opaque dark panel floating on a shadow, independent of the
//! GNOME theme. Colours are `@define-color` variables so
//! `~/.config/parsec/style.css` can override them without touching rules.

use crate::core::{ActionKind, Browse, Engine, Hit, Icon, Item, Outcome, Prompt};
use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::Rc;

const WIDTH: i32 = 720;
const ROW_HEIGHT: i32 = 56;
const VISIBLE_ROWS: i32 = 8;
/// Transparent margin around the panel where the shadow is drawn.
const SHADOW_MARGIN: i32 = 28;
const PLACEHOLDER: &str = "Search apps, projects, clipboard…";
const FALLBACK_ACCENT: &str = "#8b7cff";

/// Written when the settings window's "Edit stylesheet" finds no file.
pub const USER_CSS_TEMPLATE: &str = r#"/* Parsec user stylesheet. Reloaded live while the daemon runs.

   Colours (override any with @define-color):
     parsec_bg            panel background
     parsec_border        hairline border
     parsec_fg            main text
     parsec_dim           secondary text, hints
     parsec_accent        caret, chip, highlighted letters, selected action
     parsec_row_selected  selected row background
     parsec_row_hover
     tile_apps tile_projects tile_shell tile_github tile_clipboard
     tile_keepass tile_shortcuts tile_plugins tile_files tile_ssh
     tile_docker tile_services tile_parsec           icon tile tints
   Theme (dark/light) and accent are chosen in Settings › Launcher.

   Selectors:
     window.parsec  .parsec-panel  .parsec-search  .parsec-entry  .parsec-chip
     .parsec-results row (:selected)  .parsec-tile  .parsec-title
     .parsec-subtitle  .parsec-action  .parsec-section  .parsec-empty
     .parsec-welcome  .parsec-footer  .parsec-key
*/

/* examples:
@define-color parsec_accent #ff7a59;
@define-color parsec_bg #101014;
.parsec-entry { font-size: 24px; }
*/
"#;

const CSS: &str = r#"
@define-color parsec_bg rgba(24, 24, 30, 0.985);
@define-color parsec_border rgba(255, 255, 255, 0.09);
@define-color parsec_fg #f2f2f5;
@define-color parsec_dim rgba(242, 242, 245, 0.50);
@define-color parsec_accent #8b7cff;
@define-color parsec_row_hover rgba(255, 255, 255, 0.045);
@define-color parsec_row_selected rgba(255, 255, 255, 0.085);
@define-color tile_apps rgba(255, 255, 255, 0.06);
@define-color tile_projects #5b9cff;
@define-color tile_shell #b0b7c3;
@define-color tile_github #c58bff;
@define-color tile_clipboard #4fd1a1;
@define-color tile_keepass #ffb454;
@define-color tile_shortcuts #ff7a9a;
@define-color tile_plugins #6ad4ff;
@define-color tile_files #f0c674;
@define-color tile_ssh #7ee0c8;
@define-color tile_docker #4aa8ff;
@define-color tile_services #ff9f7a;
@define-color tile_parsec #8b7cff;
@define-color parsec_key_bg rgba(255, 255, 255, 0.10);
@define-color parsec_chip_bg rgba(255, 255, 255, 0.06);
@define-color parsec_shadow rgba(0, 0, 0, 0.55);
@define-color parsec_shadow_soft rgba(0, 0, 0, 0.35);
@define-color parsec_inner_highlight rgba(255, 255, 255, 0.05);

window.parsec {
    background-color: transparent;
}
.parsec-panel {
    background-color: @parsec_bg;
    border: 1px solid @parsec_border;
    border-radius: 18px;
    color: @parsec_fg;
    box-shadow:
        0 24px 60px @parsec_shadow,
        0 2px 8px @parsec_shadow_soft,
        inset 0 1px 0 @parsec_inner_highlight;
}

/* search bar */
.parsec-search {
    padding: 6px 18px 6px 20px;
}
.parsec-search-icon {
    color: @parsec_dim;
    margin-right: 10px;
}
.parsec-chip {
    background-color: alpha(@parsec_accent, 0.22);
    color: @parsec_accent;
    border-radius: 8px;
    padding: 3px 9px;
    margin-right: 10px;
    font-size: 13px;
    font-weight: 600;
    letter-spacing: 0.3px;
}
.parsec-entry {
    font-size: 21px;
    font-weight: 300;
    padding: 10px 0;
    border: none;
    box-shadow: none;
    outline: none;
    background: transparent;
    color: @parsec_fg;
    caret-color: @parsec_accent;
}
.parsec-entry.prompt {
    caret-color: @parsec_fg;
}
.parsec-entry selection {
    background-color: alpha(@parsec_accent, 0.45);
}
.parsec-separator {
    background-color: @parsec_border;
    min-height: 1px;
}

/* results */
.parsec-results {
    background: transparent;
    padding: 6px 8px;
}
.parsec-results row {
    padding: 0 12px;
    min-height: 56px;
    border-radius: 12px;
    background: transparent;
    outline: none;
    transition: background-color 120ms ease-out;
}
.parsec-results row:focus,
.parsec-results row:focus-visible {
    outline: none;
    box-shadow: none;
}
.parsec-results row:hover {
    background-color: @parsec_row_hover;
}
.parsec-results row:selected {
    background-color: @parsec_row_selected;
}
.parsec-section {
    font-size: 11px;
    font-weight: 600;
    letter-spacing: 1px;
    color: @parsec_dim;
    padding: 10px 14px 4px 14px;
}
.parsec-tile {
    min-width: 36px;
    min-height: 36px;
    border-radius: 10px;
    background-color: @tile_apps;
    margin-right: 2px;
}
.parsec-tile.tile-projects  { background-color: alpha(@tile_projects, 0.16);  color: @tile_projects; }
.parsec-tile.tile-shell     { background-color: alpha(@tile_shell, 0.14);     color: @tile_shell; }
.parsec-tile.tile-github    { background-color: alpha(@tile_github, 0.16);    color: @tile_github; }
.parsec-tile.tile-github-prs{ background-color: alpha(@tile_github, 0.16);    color: @tile_github; }
.parsec-tile.tile-clipboard { background-color: alpha(@tile_clipboard, 0.16); color: @tile_clipboard; }
.parsec-tile.tile-keepass   { background-color: alpha(@tile_keepass, 0.16);   color: @tile_keepass; }
.parsec-tile.tile-shortcuts { background-color: alpha(@tile_shortcuts, 0.16); color: @tile_shortcuts; }
.parsec-tile.tile-plugins   { background-color: alpha(@tile_plugins, 0.16);   color: @tile_plugins; }
.parsec-tile.tile-system    { background-color: alpha(@tile_parsec, 0.16);    color: @tile_parsec; }
.parsec-tile.tile-files     { background-color: alpha(@tile_files, 0.16);     color: @tile_files; }
.parsec-tile.tile-ssh       { background-color: alpha(@tile_ssh, 0.16);       color: @tile_ssh; }
.parsec-tile.tile-docker    { background-color: alpha(@tile_docker, 0.16);    color: @tile_docker; }
.parsec-tile.tile-services  { background-color: alpha(@tile_services, 0.16);  color: @tile_services; }
.parsec-title {
    font-size: 15px;
    font-weight: 500;
    color: @parsec_fg;
}
.parsec-subtitle {
    font-size: 12px;
    color: @parsec_dim;
}
.parsec-action {
    font-size: 12px;
    color: @parsec_dim;
    margin-left: 12px;
    transition: color 120ms ease-out;
}
.parsec-results row:selected .parsec-action {
    color: @parsec_accent;
}
.parsec-action-key {
    font-family: monospace;
    font-size: 9px;
    color: @parsec_dim;
    background-color: @parsec_key_bg;
    border-radius: 4px;
    padding: 1px 4px;
    margin-left: 8px;
}
.parsec-results row:selected .parsec-action-key {
    color: @parsec_fg;
    background-color: alpha(@parsec_accent, 0.35);
}

/* empty states */
.parsec-empty {
    font-size: 13px;
    color: @parsec_dim;
    padding: 22px 0 26px 0;
}
.parsec-welcome {
    padding: 26px 0 22px 0;
}
.parsec-welcome-tagline {
    font-size: 13px;
    color: @parsec_dim;
    margin-top: 6px;
    margin-bottom: 14px;
}
.parsec-verb-chip {
    background-color: @parsec_chip_bg;
    border: 1px solid @parsec_border;
    border-radius: 999px;
    padding: 4px 12px;
    margin: 0 4px;
    font-size: 12px;
    color: @parsec_fg;
    transition: background-color 120ms ease-out;
}
.parsec-verb-chip:hover {
    background-color: alpha(@parsec_accent, 0.22);
}
.parsec-verb-chip .verb {
    font-family: monospace;
    color: @parsec_accent;
    margin-right: 6px;
}

/* footer */
.parsec-footer {
    padding: 8px 18px 10px 20px;
    font-size: 11px;
    color: @parsec_dim;
}
.parsec-key {
    font-family: monospace;
    font-size: 10px;
    color: @parsec_fg;
    background-color: @parsec_key_bg;
    border-radius: 4px;
    padding: 1px 5px;
    margin-right: 5px;
}
.parsec-hint {
    margin-right: 18px;
}
.parsec-gear {
    min-height: 0;
    min-width: 0;
    padding: 2px;
    color: @parsec_dim;
    -gtk-icon-size: 13px;
}
.parsec-gear:hover {
    color: @parsec_fg;
}
.parsec-brand {
    color: @parsec_dim;
    font-size: 11px;
    letter-spacing: 1px;
}
"#;

const HINTS_SEARCH: &[(&str, &str)] = &[
    ("↑↓", "navigate"),
    ("⇥", "actions"),
    ("↵", "run"),
    ("esc", "close"),
];
const HINTS_PROMPT: &[(&str, &str)] = &[("↵", "submit"), ("esc", "cancel")];
const HINTS_CHIP: &[(&str, &str)] = &[("⌫", "leave mode"), ("↵", "run"), ("esc", "close")];
const HINTS_BROWSE: &[(&str, &str)] = &[("⌫", "back"), ("⇥", "actions"), ("↵", "run")];

/// One drilled-into list. The stack of these is the breadcrumb.
struct Level {
    browse: Browse,
    items: Vec<Item>,
    provider: &'static str,
    /// What the entry held when this level was opened, put back on leaving.
    entry_before: String,
    chip_before: Option<(String, String)>,
}

#[derive(Clone)]
pub struct LauncherWindow {
    window: adw::ApplicationWindow,
    panel: gtk::Box,
    entry: gtk::Entry,
    chip: gtk::Label,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    welcome: gtk::Box,
    hints: gtk::Box,
    engine: Rc<Engine>,
    results: Rc<RefCell<Vec<Hit>>>,
    /// Selected action per result row, cycled with Tab.
    action_idx: Rc<RefCell<Vec<usize>>>,
    /// Monotonic search id so a slow provider can't paint stale results.
    generation: Rc<Cell<u64>>,
    /// Whether the window got keyboard focus since it was last shown.
    was_active: Rc<Cell<bool>>,
    /// Active input request, if a provider asked for one (password...).
    prompt: Rc<RefCell<Option<Prompt>>>,
    busy: Rc<Cell<bool>>,
    /// Keyword shown as a chip; the entry then holds only the rest.
    verb: Rc<RefCell<Option<String>>>,
    /// Open drill-down levels, innermost last.
    levels: Rc<RefCell<Vec<Level>>>,
    /// Set while the code changes the entry text itself.
    suppress: Rc<Cell<bool>>,
    entrance: adw::TimedAnimation,
}

impl LauncherWindow {
    pub fn new(app: &adw::Application, engine: Rc<Engine>) -> Self {
        install_css();

        // Search bar: icon, mode chip, entry.
        let search_icon = gtk::Image::builder()
            .icon_name("edit-find-symbolic")
            .pixel_size(20)
            .css_classes(["parsec-search-icon"])
            .build();
        let chip = gtk::Label::builder()
            .css_classes(["parsec-chip"])
            .visible(false)
            .valign(gtk::Align::Center)
            .build();
        let entry = gtk::Entry::builder()
            .placeholder_text(PLACEHOLDER)
            .hexpand(true)
            .css_classes(["parsec-entry"])
            .build();
        let search = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["parsec-search"])
            .build();
        search.append(&search_icon);
        search.append(&chip);
        search.append(&entry);

        // Results.
        let list = gtk::ListBox::builder()
            .selection_mode(gtk::SelectionMode::Single)
            .css_classes(["parsec-results"])
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .propagate_natural_height(true)
            .max_content_height(ROW_HEIGHT * VISIBLE_ROWS + 40)
            .build();
        let empty = gtk::Label::builder()
            .label("No results")
            .css_classes(["parsec-empty"])
            .visible(false)
            .build();
        let welcome = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .css_classes(["parsec-welcome"])
            .visible(false)
            .build();

        // Footer: hints, gear, brand.
        let footer = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["parsec-footer"])
            .build();
        let hints = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        footer.append(&hints);
        let right = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        right.set_hexpand(true);
        right.set_halign(gtk::Align::End);
        let gear = gtk::Button::builder()
            .icon_name("emblem-system-symbolic")
            .tooltip_text("Settings (Ctrl+,)")
            .css_classes(["flat", "parsec-gear"])
            .valign(gtk::Align::Center)
            .build();
        gear.connect_clicked(|_| {
            if let Some(app) = gtk::gio::Application::default() {
                app.activate_action("preferences", None);
            }
        });
        right.append(&gear);
        let brand = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        brand.add_css_class("parsec-brand");
        brand.append(&crate::brand::logo(14));
        brand.append(&gtk::Label::new(Some(crate::brand::NAME)));
        right.append(&brand);
        footer.append(&right);

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["parsec-panel"])
            .margin_top(SHADOW_MARGIN)
            .margin_bottom(SHADOW_MARGIN)
            .margin_start(SHADOW_MARGIN)
            .margin_end(SHADOW_MARGIN)
            .build();
        panel.append(&search);
        panel.append(&separator());
        panel.append(&scroller);
        panel.append(&empty);
        panel.append(&welcome);
        panel.append(&separator());
        panel.append(&footer);

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Parsec")
            .default_width(WIDTH + 2 * SHADOW_MARGIN)
            .resizable(false)
            .decorated(false)
            .content(&panel)
            .css_classes(["parsec"])
            .build();

        // Entrance: fade and a short rise.
        let target = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak]
            panel,
            move |v| {
                panel.set_opacity(v);
                panel.set_margin_top(SHADOW_MARGIN + ((1.0 - v) * 12.0) as i32);
            }
        ));
        let entrance = adw::TimedAnimation::new(&panel, 0.0, 1.0, 160, target);
        entrance.set_easing(adw::Easing::EaseOutCubic);

        let this = Self {
            window,
            panel,
            entry,
            chip,
            list,
            scroller,
            empty,
            welcome,
            hints,
            engine,
            results: Rc::new(RefCell::new(Vec::new())),
            action_idx: Rc::new(RefCell::new(Vec::new())),
            generation: Rc::new(Cell::new(0)),
            was_active: Rc::new(Cell::new(false)),
            prompt: Rc::new(RefCell::new(None)),
            busy: Rc::new(Cell::new(false)),
            verb: Rc::new(RefCell::new(None)),
            levels: Rc::new(RefCell::new(Vec::new())),
            suppress: Rc::new(Cell::new(false)),
            entrance,
        };
        this.build_welcome();
        this.set_hints(HINTS_SEARCH);
        this.wire();
        this
    }

    fn wire(&self) {
        self.entry.connect_changed(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |_| {
                if this.suppress.get() || this.prompt.borrow().is_some() {
                    return;
                }
                this.on_typed();
            }
        ));

        self.entry.connect_activate(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |_| {
                if this.prompt.borrow().is_some() {
                    this.submit_prompt();
                } else {
                    this.activate_selected();
                }
            }
        ));

        self.list.connect_row_activated(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |_, row| {
                this.list.select_row(Some(row));
                this.activate_selected();
            }
        ));

        // Section headers when results come from several providers.
        self.list.set_header_func(glib::clone!(
            #[strong(rename_to = results)]
            self.results,
            move |row, before| {
                let results = results.borrow();
                let idx = row.index() as usize;
                let Some(hit) = results.get(idx) else {
                    row.set_header(None::<&gtk::Widget>);
                    return;
                };
                let mixed = results.iter().any(|h| h.provider != results[0].provider);
                let first_of_section = match before {
                    None => true,
                    Some(b) => results
                        .get(b.index() as usize)
                        .is_none_or(|prev| prev.provider != hit.provider),
                };
                if mixed && first_of_section {
                    let label = gtk::Label::builder()
                        .label(hit.section.to_uppercase())
                        .halign(gtk::Align::Start)
                        .css_classes(["parsec-section"])
                        .build();
                    row.set_header(Some(&label));
                } else {
                    row.set_header(None::<&gtk::Widget>);
                }
            }
        ));

        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        keys.connect_key_pressed(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |_, key, _, state| {
                use gtk::gdk::{Key, ModifierType};
                match key {
                    Key::comma if state.contains(ModifierType::CONTROL_MASK) => {
                        this.hide();
                        if let Some(app) = gtk::gio::Application::default() {
                            app.activate_action("preferences", None);
                        }
                        glib::Propagation::Stop
                    }
                    Key::Escape => {
                        if this.prompt.borrow().is_some() {
                            this.leave_prompt(true);
                        } else if this.in_browse() {
                            this.leave_browse();
                        } else {
                            this.hide();
                        }
                        glib::Propagation::Stop
                    }
                    _ if this.prompt.borrow().is_some() => glib::Propagation::Proceed,
                    Key::BackSpace if this.in_browse() && this.entry.text().is_empty() => {
                        this.leave_browse();
                        glib::Propagation::Stop
                    }
                    Key::BackSpace
                        if this.verb.borrow().is_some() && this.entry.text().is_empty() =>
                    {
                        // Leave the mode: the keyword comes back as text.
                        let verb = this.verb.borrow().clone().unwrap_or_default();
                        this.set_chip(None);
                        this.set_entry_text(&verb);
                        this.search(&verb);
                        glib::Propagation::Stop
                    }
                    Key::Down => {
                        this.move_selection(1);
                        glib::Propagation::Stop
                    }
                    Key::Up => {
                        this.move_selection(-1);
                        glib::Propagation::Stop
                    }
                    Key::Tab => {
                        this.cycle_action(1);
                        glib::Propagation::Stop
                    }
                    Key::ISO_Left_Tab => {
                        this.cycle_action(-1);
                        glib::Propagation::Stop
                    }
                    _ => glib::Propagation::Proceed,
                }
            }
        ));
        self.window.add_controller(keys);

        self.window.connect_is_active_notify(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |w| {
                if w.is_active() {
                    this.was_active.set(true);
                } else if w.is_visible() && this.was_active.get() {
                    // Compositors flip active off and on around map and
                    // hotkey release. Hide only if focus is really gone.
                    let this = this.clone();
                    glib::timeout_add_local_once(
                        std::time::Duration::from_millis(250),
                        move || {
                            if this.window.is_visible() && !this.window.is_active() {
                                tracing::debug!("hiding: focus lost");
                                this.hide();
                            }
                        },
                    );
                }
            }
        ));
    }

    // ------------------------------------------------------------ show/hide

    pub fn toggle(&self) {
        if self.window.is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn show(&self) {
        self.was_active.set(false);
        self.levels.borrow_mut().clear();
        self.set_chip(None);
        self.set_entry_text("");
        self.search("");
        self.panel.set_opacity(0.0);
        self.window.present();
        self.entry.grab_focus();
        self.entrance.play();
    }

    pub fn hide(&self) {
        if self.prompt.borrow().is_some() {
            self.leave_prompt(false);
        }
        self.window.set_visible(false);
    }

    // ------------------------------------------------------------ typing

    /// The full query the engine sees: chip keyword plus the entry text.
    fn composed(&self) -> String {
        let text = self.entry.text().to_string();
        match self.verb.borrow().as_deref() {
            Some(v) => format!("{v} {text}"),
            None => text,
        }
    }

    fn set_entry_text(&self, text: &str) {
        self.suppress.set(true);
        self.entry.set_text(text);
        self.entry.set_position(-1);
        self.suppress.set(false);
    }

    fn set_chip(&self, verb_and_label: Option<(String, String)>) {
        match verb_and_label {
            Some((verb, label)) => {
                self.chip.set_label(&label);
                self.chip.set_visible(true);
                *self.verb.borrow_mut() = Some(verb);
                self.entry.set_placeholder_text(Some("Type…"));
                self.set_hints(HINTS_CHIP);
            }
            None => {
                self.chip.set_visible(false);
                *self.verb.borrow_mut() = None;
                self.entry.set_placeholder_text(Some(PLACEHOLDER));
                self.set_hints(HINTS_SEARCH);
            }
        }
    }

    fn on_typed(&self) {
        if self.in_browse() {
            self.render_level();
            return;
        }
        if self.verb.borrow().is_none() {
            let text = self.entry.text().to_string();
            if let Some((verb, label)) = self.engine.verb_info(&text) {
                let word_like = verb.chars().last().is_some_and(|c| c.is_alphanumeric());
                let rest = text.trim_start()[verb.len()..].to_string();
                // Word keywords become a chip once the space is typed.
                if !word_like || rest.starts_with(char::is_whitespace) {
                    self.set_chip(Some((verb, label)));
                    self.set_entry_text(rest.trim_start());
                }
            }
        }
        let q = self.composed();
        self.search(&q);
    }

    fn search(&self, text: &str) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let this = self.clone();
        let text = text.to_owned();
        glib::spawn_future_local(async move {
            let hits = this.engine.search(&text).await;
            if this.generation.get() != gen {
                return;
            }
            this.render(hits, text.trim().is_empty());
        });
    }

    fn render(&self, hits: Vec<Hit>, query_empty: bool) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        *self.results.borrow_mut() = hits.clone();
        let accent = self.accent_hex();
        for hit in &hits {
            self.list.append(&row_for(hit, &accent));
        }
        self.list.invalidate_headers();
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
        let none = hits.is_empty();
        let in_prompt = self.prompt.borrow().is_some();
        self.scroller.set_visible(!none);
        self.empty.set_visible(none && !query_empty && !in_prompt);
        self.welcome.set_visible(
            none && query_empty && !in_prompt && self.verb.borrow().is_none() && !self.in_browse(),
        );
        // Hints only when there is nothing else to look at.
        self.hints.set_visible(none || in_prompt);
        *self.action_idx.borrow_mut() = vec![0; hits.len()];
    }

    fn move_selection(&self, delta: i32) {
        let n = self.results.borrow().len() as i32;
        if n == 0 {
            return;
        }
        let current = self.list.selected_row().map(|r| r.index()).unwrap_or(0);
        let next = (current + delta).rem_euclid(n);
        if let Some(row) = self.list.row_at_index(next) {
            self.list.select_row(Some(&row));
            row.grab_focus();
            self.entry.grab_focus_without_selecting();
        }
    }

    fn cycle_action(&self, delta: i32) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let idx = row.index() as usize;
        let n = match self.results.borrow().get(idx) {
            Some(h) if h.item.actions.len() > 1 => h.item.actions.len() as i32,
            _ => return,
        };
        let mut actions = self.action_idx.borrow_mut();
        let Some(slot) = actions.get_mut(idx) else {
            return;
        };
        *slot = (*slot as i32 + delta).rem_euclid(n) as usize;
        let label = self.results.borrow()[idx].item.actions[*slot].label.clone();
        set_action_label(&row, &label);
    }

    fn activate_selected(&self) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let idx = row.index() as usize;
        let item = {
            let results = self.results.borrow();
            match results.get(idx) {
                Some(h) => h.item.clone(),
                None => return,
            }
        };
        let action = self.action_idx.borrow().get(idx).copied().unwrap_or(0);
        let stays_open = matches!(
            item.actions.get(action).map(|a| &a.kind),
            Some(ActionKind::Prompt(_) | ActionKind::Browse(_))
        );
        if !stays_open {
            self.hide();
        }
        let provider = self.results.borrow()[idx].provider;
        match self.engine.activate(&item, action) {
            Ok(Outcome::Done) => {}
            Ok(Outcome::Prompt(p)) => self.enter_prompt(p),
            Ok(Outcome::Browse(b)) => self.enter_browse(b, provider),
            Err(e) => tracing::error!("activation failed: {e:#}"),
        }
    }

    // ------------------------------------------------------------ drill-down

    fn in_browse(&self) -> bool {
        !self.levels.borrow().is_empty()
    }

    /// Push a level: remember where we were, show the chip, load the list.
    fn enter_browse(&self, browse: Browse, provider: &'static str) {
        let chip_before = if self.in_browse() {
            None
        } else {
            self.verb
                .borrow()
                .clone()
                .map(|v| (v, self.chip.label().to_string()))
        };
        self.levels.borrow_mut().push(Level {
            items: Vec::new(),
            provider,
            entry_before: self.entry.text().to_string(),
            chip_before,
            browse: browse.clone(),
        });
        *self.verb.borrow_mut() = None;
        self.chip.set_label(&browse.title);
        self.chip.set_visible(true);
        self.entry.set_placeholder_text(Some("Filter…"));
        self.set_hints(HINTS_BROWSE);
        self.set_entry_text("");
        self.render(Vec::new(), true);

        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let depth = self.levels.borrow().len();
        let this = self.clone();
        glib::spawn_future_local(async move {
            let items = (browse.load)().await;
            if this.generation.get() != gen || this.levels.borrow().len() != depth {
                return;
            }
            if let Some(level) = this.levels.borrow_mut().last_mut() {
                level.items = items;
            }
            this.render_level();
        });
    }

    /// Show the innermost level filtered by the entry text.
    fn render_level(&self) {
        let text = self.entry.text().to_string();
        let hits = {
            let levels = self.levels.borrow();
            let Some(level) = levels.last() else { return };
            self.engine
                .filter(&level.items, &text, level.provider, &level.browse.title)
        };
        self.render(hits, text.trim().is_empty());
    }

    /// Pop a level: back to the parent list, or to the search.
    fn leave_browse(&self) {
        let Some(level) = self.levels.borrow_mut().pop() else {
            return;
        };
        self.generation.set(self.generation.get() + 1);
        if let Some(parent) = self.levels.borrow().last() {
            self.chip.set_label(&parent.browse.title);
        }
        if self.in_browse() {
            self.set_entry_text(&level.entry_before);
            self.render_level();
            return;
        }
        self.set_chip(level.chip_before);
        self.set_entry_text(&level.entry_before);
        self.search(&self.composed());
    }

    // ------------------------------------------------------------ prompts

    fn enter_prompt(&self, prompt: Prompt) {
        self.entry.set_visibility(!prompt.secret);
        self.entry.set_placeholder_text(Some(&prompt.title));
        self.entry.add_css_class("prompt");
        *self.prompt.borrow_mut() = Some(prompt);
        self.set_entry_text("");
        self.set_hints(HINTS_PROMPT);
        self.render(Vec::new(), true);
        self.entry.grab_focus();
    }

    fn leave_prompt(&self, restore: bool) {
        let prompt = self.prompt.borrow_mut().take();
        self.busy.set(false);
        self.entry.set_visibility(true);
        self.entry.set_sensitive(true);
        self.entry.remove_css_class("prompt");
        self.set_chip(None);
        let text = match (restore, prompt) {
            (true, Some(p)) => p.restore,
            _ => String::new(),
        };
        self.set_entry_text(&text);
        self.on_typed();
        // The entry was insensitive while working, which drops focus.
        self.entry.grab_focus_without_selecting();
    }

    fn submit_prompt(&self) {
        if self.busy.get() {
            return;
        }
        let Some(prompt) = self.prompt.borrow().clone() else {
            return;
        };
        let input = self.entry.text().to_string();
        self.set_entry_text("");
        self.entry.set_sensitive(false);
        self.entry.set_placeholder_text(Some("Working…"));
        self.busy.set(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = (prompt.submit)(input).await;
            if this.prompt.borrow().is_none() {
                return;
            }
            this.busy.set(false);
            this.entry.set_sensitive(true);
            match result {
                Ok(()) => this.leave_prompt(true),
                Err(message) => {
                    this.entry.set_placeholder_text(Some(&message));
                    this.entry.grab_focus();
                }
            }
        });
    }

    // ------------------------------------------------------------ chrome

    fn set_hints(&self, hints: &[(&str, &str)]) {
        while let Some(c) = self.hints.first_child() {
            self.hints.remove(&c);
        }
        for (key, what) in hints {
            let hint = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            hint.add_css_class("parsec-hint");
            hint.append(
                &gtk::Label::builder()
                    .label(*key)
                    .css_classes(["parsec-key"])
                    .build(),
            );
            hint.append(&gtk::Label::new(Some(what)));
            self.hints.append(&hint);
        }
    }

    /// First-run state: logo, tagline, and chips for the keywords.
    fn build_welcome(&self) {
        self.welcome.append(&crate::brand::logo(40));
        self.welcome.append(
            &gtk::Label::builder()
                .label("Type to search. Keywords open a mode:")
                .css_classes(["parsec-welcome-tagline"])
                .build(),
        );
        let chips = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .halign(gtk::Align::Center)
            .build();
        for (verb, label) in self.engine.verbs().into_iter().take(6) {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            content.append(
                &gtk::Label::builder()
                    .label(&verb)
                    .css_classes(["verb"])
                    .build(),
            );
            content.append(&gtk::Label::new(Some(&label)));
            let button = gtk::Button::builder()
                .child(&content)
                .css_classes(["flat", "parsec-verb-chip"])
                .build();
            button.connect_clicked(glib::clone!(
                #[strong(rename_to = this)]
                self,
                move |_| {
                    this.set_entry_text(&format!("{verb} "));
                    this.on_typed();
                    this.entry.grab_focus();
                }
            ));
            chips.append(&button);
        }
        self.welcome.append(&chips);
    }

    #[allow(deprecated)]
    fn accent_hex(&self) -> String {
        self.window
            .style_context()
            .lookup_color("parsec_accent")
            .map(|c| {
                format!(
                    "#{:02x}{:02x}{:02x}",
                    (c.red() * 255.0) as u8,
                    (c.green() * 255.0) as u8,
                    (c.blue() * 255.0) as u8
                )
            })
            .unwrap_or_else(|| FALLBACK_ACCENT.to_string())
    }
}

// ---------------------------------------------------------------- widgets

fn separator() -> gtk::Separator {
    gtk::Separator::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["parsec-separator"])
        .build()
}

/// Title with the matched characters in accent, as Pango markup.
fn title_markup(title: &str, highlight: &[u32], accent: &str) -> String {
    let set: HashSet<u32> = highlight.iter().copied().collect();
    let mut out = String::with_capacity(title.len() + 64);
    let mut open = false;
    for (i, ch) in title.chars().enumerate() {
        let hit = set.contains(&(i as u32));
        if hit && !open {
            out.push_str(&format!("<span foreground=\"{accent}\" weight=\"bold\">"));
            open = true;
        } else if !hit && open {
            out.push_str("</span>");
            open = false;
        }
        out.push_str(&glib::markup_escape_text(&ch.to_string()));
    }
    if open {
        out.push_str("</span>");
    }
    out
}

fn row_for(hit: &Hit, accent: &str) -> gtk::ListBoxRow {
    let item = &hit.item;
    let symbolic = !matches!(item.icon, Icon::GIcon(_) | Icon::Path(_));
    let image = gtk::Image::builder()
        .pixel_size(if symbolic { 18 } else { 28 })
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    match &item.icon {
        Icon::None => image.set_icon_name(Some("application-x-executable-symbolic")),
        Icon::Named(name) => image.set_icon_name(Some(name)),
        Icon::GIcon(gicon) => image.set_from_gicon(gicon),
        Icon::Path(path) => image.set_from_file(Some(path)),
    }
    let tile = gtk::Box::builder()
        .css_classes(["parsec-tile", &format!("tile-{}", hit.provider)])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Start)
        .hexpand(false)
        .width_request(36)
        .height_request(36)
        .build();
    image.set_halign(gtk::Align::Center);
    image.set_valign(gtk::Align::Center);
    image.set_hexpand(true);
    image.set_vexpand(true);
    tile.set_hexpand_set(true);
    tile.append(&image);

    let title = gtk::Label::builder()
        .use_markup(true)
        .label(title_markup(&item.title, &hit.highlight, accent))
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["parsec-title"])
        .build();

    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    texts.append(&title);
    if let Some(sub) = &item.subtitle {
        texts.append(
            &gtk::Label::builder()
                .label(sub)
                .halign(gtk::Align::Start)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["parsec-subtitle"])
                .build(),
        );
    }

    let action = gtk::Label::builder()
        .label(item.actions.first().map(|a| a.label.as_str()).unwrap_or(""))
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["parsec-action"])
        .build();
    let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    trailing.set_valign(gtk::Align::Center);
    trailing.append(&action);
    if matches!(
        item.actions.first().map(|a| &a.kind),
        Some(ActionKind::Browse(_))
    ) {
        trailing.append(
            &gtk::Label::builder()
                .label("›")
                .css_classes(["parsec-action-key"])
                .tooltip_text("Enter to open")
                .build(),
        );
    }
    if item.actions.len() > 1 {
        trailing.append(
            &gtk::Label::builder()
                .label("⇥")
                .css_classes(["parsec-action-key"])
                .tooltip_text("Tab to change action")
                .build(),
        );
    }

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hbox.append(&tile);
    hbox.append(&texts);
    hbox.append(&trailing);
    gtk::ListBoxRow::builder().child(&hbox).build()
}

fn set_action_label(row: &gtk::ListBoxRow, text: &str) {
    // row > hbox > [tile, texts, trailing > [action, key?]]
    let Some(hbox) = row.child() else { return };
    let Some(trailing) = hbox.last_child() else {
        return;
    };
    let Some(action) = trailing.first_child() else {
        return;
    };
    if let Ok(label) = action.downcast::<gtk::Label>() {
        label.set_label(text);
    }
}

fn install_css() {
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    let builtin = gtk::CssProvider::new();
    builtin.load_from_string(CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &builtin,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    // User overrides: ~/.config/parsec/style.css, reloaded whenever it
    // changes, so the look can be tuned with the daemon running.
    let path = user_css_path();
    let user = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &user, gtk::STYLE_PROVIDER_PRIORITY_USER);
    let load = {
        let user = user.clone();
        let path = path.clone();
        move || {
            if path.is_file() {
                user.load_from_path(&path);
                tracing::info!(path = %path.display(), "loaded user stylesheet");
            } else {
                user.load_from_string("");
            }
        }
    };
    load();
    let file = gtk::gio::File::for_path(&path);
    if let Ok(monitor) = file.monitor_file(
        gtk::gio::FileMonitorFlags::NONE,
        gtk::gio::Cancellable::NONE,
    ) {
        monitor.connect_changed(move |_, _, _, event| {
            use gtk::gio::FileMonitorEvent as E;
            if matches!(
                event,
                E::ChangesDoneHint | E::Created | E::Deleted | E::Renamed
            ) {
                load();
            }
        });
        std::mem::forget(monitor);
    }
}

pub fn user_css_path() -> std::path::PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("parsec")
        .join("style.css")
}

#[cfg(test)]
mod tests {
    use super::title_markup;

    #[test]
    fn markup_wraps_runs_and_escapes() {
        let m = title_markup("a<b", &[0, 2], "#fff");
        assert_eq!(
            m,
            "<span foreground=\"#fff\" weight=\"bold\">a</span>&lt;<span foreground=\"#fff\" weight=\"bold\">b</span>"
        );
        assert_eq!(
            title_markup("abc", &[1, 2], "#000"),
            "a<span foreground=\"#000\" weight=\"bold\">bc</span>"
        );
        assert_eq!(title_markup("abc", &[], "#000"), "abc");
    }
}
