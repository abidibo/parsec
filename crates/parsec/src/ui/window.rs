//! The single launcher window: a search entry on top, results below, key
//! hints at the bottom. Created once at startup and toggled, never destroyed.
//!
//! Look: a translucent dark panel independent of the GNOME theme. Colours
//! are `@define-color` variables so `~/.config/parsec/style.css` can
//! override them without touching the rules.

use crate::core::{Engine, Icon, Item, Outcome, Prompt};
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// Written when the settings window's "Edit stylesheet" finds no file.
pub const USER_CSS_TEMPLATE: &str = r#"/* Parsec user stylesheet. Reloaded live while the daemon runs.

   Colours (override any with @define-color):
     parsec_bg            panel background, rgba for translucency
     parsec_border        hairline border
     parsec_fg            main text
     parsec_dim           secondary text, hints
     parsec_accent        caret, selected action label
     parsec_row_selected  selected row background
     parsec_row_hover

   Selectors:
     window.parsec  .parsec-panel  .parsec-search  .parsec-entry
     .parsec-results row (:selected)  .parsec-title  .parsec-subtitle
     .parsec-action  .parsec-empty  .parsec-footer  .parsec-key
*/

/* examples:
@define-color parsec_accent #ff7a59;
@define-color parsec_bg rgba(10, 10, 14, 0.85);
.parsec-entry { font-size: 24px; }
*/
"#;

const WIDTH: i32 = 720;
const ROW_HEIGHT: i32 = 56;
const VISIBLE_ROWS: i32 = 8;

const CSS: &str = r#"
@define-color parsec_bg rgba(22, 22, 28, 0.94);
@define-color parsec_border rgba(255, 255, 255, 0.10);
@define-color parsec_fg #f2f2f5;
@define-color parsec_dim rgba(242, 242, 245, 0.50);
@define-color parsec_accent #8b7cff;
@define-color parsec_row_hover rgba(255, 255, 255, 0.05);
@define-color parsec_row_selected rgba(255, 255, 255, 0.09);

window.parsec {
    background-color: transparent;
}
.parsec-panel {
    background-color: @parsec_bg;
    border: 1px solid @parsec_border;
    border-radius: 16px;
    color: @parsec_fg;
}

/* search bar */
.parsec-search {
    padding: 6px 18px 6px 20px;
}
.parsec-search-icon {
    color: @parsec_dim;
    margin-right: 10px;
}
.parsec-entry {
    font-size: 21px;
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
    padding: 0 14px;
    min-height: 56px;
    border-radius: 10px;
    background: transparent;
    outline: none;
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
.parsec-icon {
    margin-right: 2px;
}
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
}
.parsec-results row:selected .parsec-action {
    color: @parsec_accent;
}
.parsec-action-key {
    font-family: monospace;
    font-size: 9px;
    color: @parsec_dim;
    background-color: rgba(255, 255, 255, 0.08);
    border-radius: 4px;
    padding: 1px 4px;
    margin-left: 8px;
}
.parsec-results row:selected .parsec-action-key {
    color: @parsec_fg;
    background-color: alpha(@parsec_accent, 0.35);
}

/* empty state */
.parsec-empty {
    font-size: 13px;
    color: @parsec_dim;
    padding: 22px 0 26px 0;
}

/* footer */
.parsec-footer {
    padding: 8px 20px 10px 20px;
    font-size: 11px;
    color: @parsec_dim;
}
.parsec-key {
    font-family: monospace;
    font-size: 10px;
    color: @parsec_fg;
    background-color: rgba(255, 255, 255, 0.10);
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

#[derive(Clone)]
pub struct LauncherWindow {
    window: adw::ApplicationWindow,
    entry: gtk::Entry,
    list: gtk::ListBox,
    scroller: gtk::ScrolledWindow,
    empty: gtk::Label,
    engine: Rc<Engine>,
    results: Rc<RefCell<Vec<Item>>>,
    /// Selected action per result row, cycled with Tab.
    action_idx: Rc<RefCell<Vec<usize>>>,
    /// Monotonic search id so a slow provider can't paint stale results.
    generation: Rc<Cell<u64>>,
    /// Whether the window got keyboard focus since it was last shown. Focus
    /// loss hides the launcher only after that, otherwise the brief
    /// not-yet-active moment right after `present()` would hide it.
    was_active: Rc<Cell<bool>>,
    /// Active input request, if a provider asked for one (password...).
    prompt: Rc<RefCell<Option<Prompt>>>,
    /// A prompt submission is being processed.
    busy: Rc<Cell<bool>>,
}

const PLACEHOLDER: &str = "Search apps, projects, clipboard…";

impl LauncherWindow {
    pub fn new(app: &adw::Application, engine: Rc<Engine>) -> Self {
        install_css();

        // Search bar: icon + entry.
        let search_icon = gtk::Image::builder()
            .icon_name("edit-find-symbolic")
            .pixel_size(20)
            .css_classes(["parsec-search-icon"])
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
            .max_content_height(ROW_HEIGHT * VISIBLE_ROWS + 12)
            .build();

        let empty = gtk::Label::builder()
            .label("No results")
            .css_classes(["parsec-empty"])
            .visible(false)
            .build();

        let panel = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["parsec-panel"])
            .build();
        panel.append(&search);
        panel.append(&separator());
        panel.append(&scroller);
        panel.append(&empty);
        panel.append(&separator());
        panel.append(&footer());

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Parsec")
            .default_width(WIDTH)
            .resizable(false)
            .decorated(false)
            .content(&panel)
            .css_classes(["parsec"])
            .build();

        let this = Self {
            window,
            entry,
            list,
            scroller,
            empty,
            engine,
            results: Rc::new(RefCell::new(Vec::new())),
            action_idx: Rc::new(RefCell::new(Vec::new())),
            generation: Rc::new(Cell::new(0)),
            was_active: Rc::new(Cell::new(false)),
            prompt: Rc::new(RefCell::new(None)),
            busy: Rc::new(Cell::new(false)),
        };
        this.wire();
        this
    }

    fn wire(&self) {
        // Typing re-runs the search.
        self.entry.connect_changed(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |e| {
                if this.prompt.borrow().is_none() {
                    this.search(e.text().as_str());
                }
            }
        ));

        // Enter: submit the prompt, or activate the selected row.
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

        // Clicking a row activates it.
        self.list.connect_row_activated(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |_, row| {
                this.list.select_row(Some(row));
                this.activate_selected();
            }
        ));

        // Keyboard navigation while focus stays in the entry.
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
                        } else {
                            this.hide();
                        }
                        glib::Propagation::Stop
                    }
                    _ if this.prompt.borrow().is_some() => glib::Propagation::Proceed,
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

        // Losing focus hides the launcher, once it has had focus.
        self.window.connect_is_active_notify(glib::clone!(
            #[strong(rename_to = this)]
            self,
            move |w| {
                tracing::debug!(
                    active = w.is_active(),
                    visible = w.is_visible(),
                    was_active = this.was_active.get(),
                    "focus change"
                );
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
                            } else {
                                tracing::debug!("focus came back, staying");
                            }
                        },
                    );
                }
            }
        ));
    }

    pub fn toggle(&self) {
        tracing::debug!(visible = self.window.is_visible(), "toggle");
        if self.window.is_visible() {
            self.hide();
        } else {
            self.show();
        }
    }

    pub fn show(&self) {
        self.was_active.set(false);
        self.entry.set_text("");
        self.search("");
        self.window.present();
        self.entry.grab_focus();
    }

    pub fn hide(&self) {
        if self.prompt.borrow().is_some() {
            self.leave_prompt(false);
        }
        self.window.set_visible(false);
    }

    /// Turn the search box into an input field for `prompt`.
    fn enter_prompt(&self, prompt: Prompt) {
        self.entry.set_visibility(!prompt.secret);
        self.entry.set_placeholder_text(Some(&prompt.title));
        self.entry.add_css_class("prompt");
        *self.prompt.borrow_mut() = Some(prompt);
        self.entry.set_text("");
        self.render(Vec::new(), true);
        self.entry.grab_focus();
    }

    /// Back to searching. `restore` puts the provider's query back.
    fn leave_prompt(&self, restore: bool) {
        let prompt = self.prompt.borrow_mut().take();
        self.busy.set(false);
        self.entry.set_visibility(true);
        self.entry.set_sensitive(true);
        self.entry.set_placeholder_text(Some(PLACEHOLDER));
        self.entry.remove_css_class("prompt");
        let text = match (restore, prompt) {
            (true, Some(p)) => p.restore,
            _ => String::new(),
        };
        self.entry.set_text(&text);
        self.entry.set_position(-1);
        self.search(&text);
    }

    fn submit_prompt(&self) {
        if self.busy.get() {
            return;
        }
        let Some(prompt) = self.prompt.borrow().clone() else {
            return;
        };
        let input = self.entry.text().to_string();
        self.entry.set_text("");
        self.entry.set_sensitive(false);
        self.entry.set_placeholder_text(Some("Working…"));
        self.busy.set(true);
        let this = self.clone();
        glib::spawn_future_local(async move {
            let result = (prompt.submit)(input).await;
            if this.prompt.borrow().is_none() {
                return; // cancelled or hidden meanwhile
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

    fn search(&self, text: &str) {
        let gen = self.generation.get() + 1;
        self.generation.set(gen);
        let this = self.clone();
        let text = text.to_owned();
        glib::spawn_future_local(async move {
            let items = this.engine.search(&text).await;
            if this.generation.get() != gen {
                return;
            }
            this.render(items, text.trim().is_empty());
        });
    }

    fn render(&self, items: Vec<Item>, query_empty: bool) {
        while let Some(child) = self.list.first_child() {
            self.list.remove(&child);
        }
        for item in &items {
            self.list.append(&row_for(item));
        }
        if let Some(first) = self.list.row_at_index(0) {
            self.list.select_row(Some(&first));
        }
        // An empty query with nothing to show is the first-run state: no
        // message, just the search bar. A typed query with no hits says so.
        let none = items.is_empty();
        self.scroller.set_visible(!none);
        self.empty.set_visible(none && !query_empty);
        *self.action_idx.borrow_mut() = vec![0; items.len()];
        *self.results.borrow_mut() = items;
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

    /// Rotate the selected row's action and refresh its label.
    fn cycle_action(&self, delta: i32) {
        let Some(row) = self.list.selected_row() else {
            return;
        };
        let idx = row.index() as usize;
        let n = match self.results.borrow().get(idx) {
            Some(item) if item.actions.len() > 1 => item.actions.len() as i32,
            _ => return,
        };
        let mut actions = self.action_idx.borrow_mut();
        let Some(slot) = actions.get_mut(idx) else {
            return;
        };
        *slot = (*slot as i32 + delta).rem_euclid(n) as usize;
        let label = self.results.borrow()[idx].actions[*slot].label.clone();
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
                Some(i) => i.clone(),
                None => return,
            }
        };
        let action = self.action_idx.borrow().get(idx).copied().unwrap_or(0);
        let is_prompt = matches!(
            item.actions.get(action).map(|a| &a.kind),
            Some(crate::core::ActionKind::Prompt(_))
        );
        if !is_prompt {
            // Hide first so the launched thing gets focus.
            self.hide();
        }
        match self.engine.activate(&item, action) {
            Ok(Outcome::Done) => {}
            Ok(Outcome::Prompt(p)) => self.enter_prompt(p),
            Err(e) => tracing::error!("activation failed: {e:#}"),
        }
    }
}

fn separator() -> gtk::Separator {
    gtk::Separator::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["parsec-separator"])
        .build()
}

fn footer() -> gtk::Box {
    let bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .css_classes(["parsec-footer"])
        .build();
    for (key, what) in [
        ("↑↓", "navigate"),
        ("⇥", "actions"),
        ("↵", "run"),
        ("esc", "close"),
    ] {
        let hint = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        hint.add_css_class("parsec-hint");
        hint.append(
            &gtk::Label::builder()
                .label(key)
                .css_classes(["parsec-key"])
                .build(),
        );
        hint.append(&gtk::Label::new(Some(what)));
        bar.append(&hint);
    }
    // Settings gear and brand, right-aligned.
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
    bar.append(&right);
    bar
}

fn row_for(item: &Item) -> gtk::ListBoxRow {
    let image = gtk::Image::builder()
        .pixel_size(30)
        .css_classes(["parsec-icon"])
        .build();
    match &item.icon {
        Icon::None => image.set_icon_name(Some("application-x-executable")),
        Icon::Named(name) => image.set_icon_name(Some(name)),
        Icon::GIcon(gicon) => image.set_from_gicon(gicon),
        Icon::Path(path) => image.set_from_file(Some(path)),
    }

    let title = gtk::Label::builder()
        .label(&item.title)
        .halign(gtk::Align::Start)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["parsec-title"])
        .build();

    let texts = gtk::Box::new(gtk::Orientation::Vertical, 1);
    texts.set_valign(gtk::Align::Center);
    texts.set_hexpand(true);
    texts.append(&title);
    if let Some(sub) = &item.subtitle {
        let subtitle = gtk::Label::builder()
            .label(sub)
            .halign(gtk::Align::Start)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["parsec-subtitle"])
            .build();
        texts.append(&subtitle);
    }

    // Current action, right-aligned. Tab cycles it when more than one exists.
    let action = gtk::Label::builder()
        .label(item.actions.first().map(|a| a.label.as_str()).unwrap_or(""))
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["parsec-action"])
        .build();
    let trailing = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    trailing.set_valign(gtk::Align::Center);
    trailing.append(&action);
    if item.actions.len() > 1 {
        // A Tab keycap tells the user there is more than one action.
        let key = gtk::Label::builder()
            .label("⇥")
            .css_classes(["parsec-action-key"])
            .tooltip_text("Tab to change action")
            .build();
        trailing.append(&key);
    }

    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hbox.append(&image);
    hbox.append(&texts);
    hbox.append(&trailing);

    gtk::ListBoxRow::builder().child(&hbox).build()
}

fn set_action_label(row: &gtk::ListBoxRow, text: &str) {
    // row > hbox > [image, texts, trailing > [action, key?]]
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
