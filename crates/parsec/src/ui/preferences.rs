//! Preferences window. Edits the shared config and saves it on every change;
//! the daemon's file monitor then reloads it, so the window, the file and the
//! providers never disagree.

use crate::config::{Config, SharedConfig};
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

thread_local! {
    static OPEN: RefCell<Option<adw::PreferencesWindow>> = const { RefCell::new(None) };
}

/// Show the preferences window, creating it if needed. One at a time.
pub fn open(app: &adw::Application, cfg: SharedConfig) {
    tracing::info!("opening settings");
    if let Some(existing) = OPEN.with(|o| o.borrow().clone()) {
        existing.present();
        return;
    }
    let window = build(app, cfg);
    window.connect_close_request(|w| {
        // Drop focus so a text row being edited commits through its focus-leave.
        gtk::prelude::GtkWindowExt::set_focus(w, None::<&gtk::Widget>);
        OPEN.with(|o| *o.borrow_mut() = None);
        glib::Propagation::Proceed
    });
    OPEN.with(|o| *o.borrow_mut() = Some(window.clone()));
    window.present();
}

fn build(app: &adw::Application, cfg: SharedConfig) -> adw::PreferencesWindow {
    let window = adw::PreferencesWindow::builder()
        .application(app)
        .title("Parsec Settings")
        .default_width(640)
        .default_height(720)
        .search_enabled(true)
        .build();
    window.add(&launcher_page(&window, cfg.clone()));
    window.add(&general_page(&window, cfg.clone()));
    window.add(&providers_page(&window, cfg.clone()));
    window.add(&crate::ui::shortcuts_page::page(&window, cfg.clone()));
    window.add(&crate::ui::plugins_page::page(&window, cfg.clone()));
    window
}

// ---------------------------------------------------------------- pages

fn general_page(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("General")
        .icon_name("preferences-system-symbolic")
        .build();

    // Project folders: one row each, added straight to the group.
    let projects = adw::PreferencesGroup::builder()
        .title("Project folders")
        .description("Scanned for git repositories.")
        .build();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add folder")
        .css_classes(["flat"])
        .build();
    projects.set_header_suffix(Some(&add));
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    refresh_roots(&projects, &rows, &cfg);
    add.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        projects,
        #[strong]
        rows,
        #[strong]
        cfg,
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Add project folder")
                .build();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                projects,
                #[strong]
                rows,
                #[strong]
                cfg,
                async move {
                    if let Ok(file) = dialog.select_folder_future(Some(&window)).await {
                        if let Some(path) = file.path() {
                            let shown = crate::config::abbreviate_home(&path);
                            edit(&cfg, |c| {
                                if !c.projects.roots.contains(&shown) {
                                    c.projects.roots.push(shown);
                                }
                            });
                            refresh_roots(&projects, &rows, &cfg);
                        }
                    }
                }
            ));
        }
    ));
    page.add(&projects);

    let scanning = adw::PreferencesGroup::new();
    scanning.add(&spin_row(
        "Scan depth",
        "How many levels below each folder to look for a repository",
        cfg.borrow().projects.max_depth as f64,
        1.0,
        8.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.projects.max_depth = v as usize)
        ),
    ));
    page.add(&scanning);

    // Editor.
    let editor = adw::PreferencesGroup::builder()
        .title("Editor")
        .description("Used by \"Open in editor\" on a project. {path} is the project folder.")
        .build();
    editor.add(&argv_row(
        "Command",
        &cfg.borrow().editor.command,
        glib::clone!(
            #[strong]
            cfg,
            move |argv| edit(&cfg, |c| c.editor.command = argv)
        ),
    ));
    editor.add(&switch_row(
        "Runs in a terminal",
        "Enable for nvim, vim, helix and other terminal editors",
        cfg.borrow().editor.in_terminal,
        glib::clone!(
            #[strong]
            cfg,
            move |on| edit(&cfg, |c| c.editor.in_terminal = on)
        ),
    ));
    page.add(&editor);

    // Terminal.
    let terminal = adw::PreferencesGroup::builder()
        .title("Terminal")
        .description(
            "Opens at {cwd} running {exec}. {exec} is dropped, with its separator, \
             when there is nothing to run. Also used by the shell verb.",
        )
        .build();
    terminal.add(&argv_row(
        "Command",
        &cfg.borrow().terminal.command,
        glib::clone!(
            #[strong]
            cfg,
            move |argv| edit(&cfg, |c| c.terminal.command = argv)
        ),
    ));
    page.add(&terminal);

    page
}

fn providers_page(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Providers")
        .icon_name("view-list-symbolic")
        .build();

    let github = adw::PreferencesGroup::builder()
        .title("GitHub")
        .description("Needs the gh CLI, logged in.")
        .build();
    github.add(&text_row(
        "Owners",
        "Comma separated. Empty lists the logged-in account's repositories.",
        &cfg.borrow().github.owners.join(", "),
        glib::clone!(
            #[strong]
            cfg,
            move |text| {
                let owners = text
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                edit(&cfg, |c| c.github.owners = owners);
                true
            }
        ),
    ));
    github.add(&spin_row(
        "Cache (seconds)",
        "How long gh results are kept before asking again",
        cfg.borrow().github.cache_secs as f64,
        10.0,
        86400.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.github.cache_secs = v as u64)
        ),
    ));
    page.add(&github);

    let clipboard = adw::PreferencesGroup::builder()
        .title("Clipboard")
        .description("History needs wl-clipboard; on GNOME before 48 also xclip. Takes effect at the next start.")
        .build();
    clipboard.add(&switch_row(
        "Keep history",
        "",
        cfg.borrow().clipboard.enabled,
        glib::clone!(
            #[strong]
            cfg,
            move |on| edit(&cfg, |c| c.clipboard.enabled = on)
        ),
    ));
    clipboard.add(&spin_row(
        "Entries kept",
        "Pinned snippets never count against this",
        cfg.borrow().clipboard.max_items as f64,
        10.0,
        5000.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.clipboard.max_items = v as usize)
        ),
    ));
    clipboard.add(&spin_row(
        "Largest entry (KB)",
        "Bigger clipboard contents are ignored",
        (cfg.borrow().clipboard.max_bytes / 1024) as f64,
        1.0,
        10240.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.clipboard.max_bytes = v as usize * 1024)
        ),
    ));
    clipboard.add(&spin_row(
        "Poll interval (seconds)",
        "Only on compositors without the data-control protocol",
        cfg.borrow().clipboard.poll_secs as f64,
        1.0,
        60.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.clipboard.poll_secs = v as u64)
        ),
    ));
    page.add(&clipboard);

    let files = adw::PreferencesGroup::builder()
        .title("Files")
        .description("Search by name through GNOME's Tracker index and plocate.")
        .build();
    files.add(&text_row(
        "Only under",
        "Comma separated folders, ~ allowed. Empty = anywhere.",
        &cfg.borrow().files.roots.join(", "),
        glib::clone!(
            #[strong]
            cfg,
            move |text| {
                let roots = text
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                edit(&cfg, |c| c.files.roots = roots);
                true
            }
        ),
    ));
    files.add(&text_row(
        "Excluded folder names",
        "Comma separated",
        &cfg.borrow().files.exclude.join(", "),
        glib::clone!(
            #[strong]
            cfg,
            move |text| {
                let ex = text
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                edit(&cfg, |c| c.files.exclude = ex);
                true
            }
        ),
    ));
    files.add(&switch_row(
        "Show hidden files",
        "",
        cfg.borrow().files.hidden,
        glib::clone!(
            #[strong]
            cfg,
            move |on| edit(&cfg, |c| c.files.hidden = on)
        ),
    ));
    files.add(&switch_row(
        "Use Tracker",
        "GNOME's index: fresh within seconds, limited to Documents, Downloads, Desktop and media",
        cfg.borrow().files.tracker,
        glib::clone!(
            #[strong]
            cfg,
            move |on| edit(&cfg, |c| c.files.tracker = on)
        ),
    ));
    files.add(&switch_row(
        "Use plocate",
        "Everything on disk, refreshed nightly by the system",
        cfg.borrow().files.plocate,
        glib::clone!(
            #[strong]
            cfg,
            move |on| edit(&cfg, |c| c.files.plocate = on)
        ),
    ));
    page.add(&files);

    let keepass = adw::PreferencesGroup::builder()
        .title("KeePass")
        .description("Entries from a .kdbx database, unlocked with the master password typed in the launcher.")
        .build();
    keepass.add(&path_row(
        window,
        "Database",
        &cfg.borrow().keepass.database,
        &["*.kdbx"],
        glib::clone!(
            #[strong]
            cfg,
            move |p| edit(&cfg, |c| c.keepass.database = p)
        ),
    ));
    keepass.add(&path_row(
        window,
        "Key file (optional)",
        &cfg.borrow().keepass.key_file,
        &["*"],
        glib::clone!(
            #[strong]
            cfg,
            move |p| edit(&cfg, |c| c.keepass.key_file = p)
        ),
    ));
    keepass.add(&spin_row(
        "Lock after (minutes)",
        "Decrypted entries are forgotten after this long",
        (cfg.borrow().keepass.lock_after_secs / 60) as f64,
        1.0,
        720.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.keepass.lock_after_secs = v as u64 * 60)
        ),
    ));
    keepass.add(&spin_row(
        "Clear password after (seconds)",
        "0 leaves a copied password on the clipboard",
        cfg.borrow().keepass.clipboard_clear_secs as f64,
        0.0,
        300.0,
        glib::clone!(
            #[strong]
            cfg,
            move |v| edit(&cfg, |c| c.keepass.clipboard_clear_secs = v as u64)
        ),
    ));
    keepass.add(&text_row(
        "Hidden groups",
        "Comma separated group names whose entries are not shown",
        &cfg.borrow().keepass.skip_groups.join(", "),
        glib::clone!(
            #[strong]
            cfg,
            move |text| {
                let groups = text
                    .split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(String::from)
                    .collect();
                edit(&cfg, |c| c.keepass.skip_groups = groups);
                true
            }
        ),
    ));
    page.add(&keepass);

    let verbs = adw::PreferencesGroup::builder()
        .title("Verbs")
        .description("Trigger words. A word verb needs a space after it, a symbol does not.")
        .build();
    let v = cfg.borrow().verbs.clone();
    for (title, value, set) in [
        (
            "Shell command",
            v.shell,
            Box::new(|c: &mut Config, s: String| c.verbs.shell = s)
                as Box<dyn Fn(&mut Config, String)>,
        ),
        (
            "GitHub repositories",
            v.github,
            Box::new(|c: &mut Config, s: String| c.verbs.github = s),
        ),
        (
            "Pull requests",
            v.prs,
            Box::new(|c: &mut Config, s: String| c.verbs.prs = s),
        ),
        (
            "Clipboard",
            v.clipboard,
            Box::new(|c: &mut Config, s: String| c.verbs.clipboard = s),
        ),
        (
            "Windows (needs the Shell extension)",
            v.windows,
            Box::new(|c: &mut Config, s: String| c.verbs.windows = s),
        ),
    ] {
        let set = Rc::new(set);
        verbs.add(&text_row(
            title,
            "",
            &value,
            glib::clone!(
                #[strong]
                cfg,
                move |text| {
                    let text = text.trim().to_string();
                    if text.is_empty() || text.contains(char::is_whitespace) {
                        return false;
                    }
                    edit(&cfg, |c| set(c, text));
                    true
                }
            ),
        ));
    }
    page.add(&verbs);

    page
}

fn launcher_page(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Launcher")
        .icon_name("input-keyboard-symbolic")
        .build();

    // About.
    let about = adw::PreferencesGroup::new();
    let card = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(4)
        .halign(gtk::Align::Center)
        .margin_top(6)
        .margin_bottom(12)
        .build();
    card.append(&crate::brand::logo(72));
    card.append(
        &gtk::Label::builder()
            .label(crate::brand::NAME)
            .css_classes(["title-1"])
            .margin_top(8)
            .build(),
    );
    card.append(
        &gtk::Label::builder()
            .label(crate::brand::TAGLINE)
            .css_classes(["dim-label"])
            .build(),
    );
    card.append(
        &gtk::Label::builder()
            .label(format!("version {}", crate::brand::VERSION))
            .css_classes(["dim-label", "caption"])
            .build(),
    );
    about.add(&card);
    page.add(&about);

    // Startup.
    let startup = adw::PreferencesGroup::builder().title("Startup").build();
    let autostart = switch_row(
        "Start at login",
        "Runs the daemon in the background when you log in",
        crate::autostart::is_enabled(),
        |on| {
            if let Err(e) = crate::autostart::set_enabled(on) {
                tracing::warn!("autostart change failed: {e}");
            }
        },
    );
    startup.add(&autostart);
    page.add(&startup);

    let hotkey = adw::PreferencesGroup::builder()
        .title("Hotkey")
        .description("The key combination that opens Parsec, stored as a GNOME custom shortcut.")
        .build();
    let shown = |accel: Option<String>| match accel {
        Some(a) => crate::ui::hotkey::label(&a),
        None => "Not set".to_string(),
    };
    let key_row = adw::ActionRow::builder()
        .title("Open Parsec")
        .subtitle(shown(crate::ui::hotkey::current()))
        .build();
    key_row.add_prefix(&gtk::Image::from_icon_name(
        "preferences-desktop-keyboard-shortcuts-symbolic",
    ));
    let change = gtk::Button::builder()
        .label("Change…")
        .valign(gtk::Align::Center)
        .build();
    change.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        key_row,
        move |_| {
            let key_row = key_row.clone();
            let w = window.clone();
            crate::ui::hotkey::capture(&window, move |result| match result {
                Some(accel) => {
                    key_row.set_subtitle(&crate::ui::hotkey::label(&accel));
                    w.add_toast(adw::Toast::new(&format!(
                        "Hotkey set to {}",
                        crate::ui::hotkey::label(&accel)
                    )));
                }
                None => {
                    if crate::ui::hotkey::current().is_none() {
                        w.add_toast(adw::Toast::new(
                            "Could not save: GNOME keyboard settings not available",
                        ));
                    }
                }
            });
        }
    ));
    key_row.add_suffix(&change);
    hotkey.add(&key_row);
    page.add(&hotkey);
    page.add(&extension_group(window, cfg.clone()));

    let look = adw::PreferencesGroup::builder()
        .title("Appearance")
        .description("Theme and accent, then a stylesheet for everything else, reloaded live.")
        .build();
    let themes = gtk::StringList::new(&["Dark", "Light", "Follow system"]);
    let theme_row = adw::ComboRow::builder()
        .title("Theme")
        .model(&themes)
        .build();
    theme_row.set_selected(match cfg.borrow().appearance.theme.as_str() {
        "light" => 1,
        "system" => 2,
        _ => 0,
    });
    theme_row.connect_selected_notify(glib::clone!(
        #[strong]
        cfg,
        move |r| {
            let value = match r.selected() {
                1 => "light",
                2 => "system",
                _ => "dark",
            };
            edit(&cfg, |c| c.appearance.theme = value.into());
            crate::ui::theme::apply(&cfg);
        }
    ));
    look.add(&theme_row);
    look.add(&text_row(
        "Accent colour",
        "\"system\" follows GNOME's accent (47+), or a hex colour like #ff7a59",
        &cfg.borrow().appearance.accent,
        glib::clone!(
            #[strong]
            cfg,
            move |text| {
                let t = text.trim().to_lowercase();
                let ok = t == "system"
                    || ((t.len() == 7 || t.len() == 4)
                        && t.starts_with('#')
                        && t[1..].chars().all(|c| c.is_ascii_hexdigit()));
                if ok {
                    edit(&cfg, |c| c.appearance.accent = t);
                    crate::ui::theme::apply(&cfg);
                }
                ok
            }
        ),
    ));
    let css_path = crate::ui::window::user_css_path();
    let edit_css = link_row("Edit stylesheet", "document-edit-symbolic");
    let shown = adw::ActionRow::builder()
        .title("Location")
        .subtitle(crate::config::abbreviate_home(&css_path))
        .css_classes(["property"])
        .build();
    edit_css.connect_activated(move |_| {
        if !css_path.exists() {
            if let Some(dir) = css_path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            let _ = std::fs::write(&css_path, crate::ui::window::USER_CSS_TEMPLATE);
        }
        let uri = format!("file://{}", css_path.display());
        let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, gtk::gio::AppLaunchContext::NONE);
    });
    look.add(&edit_css);
    look.add(&shown);
    page.add(&look);

    let about = adw::PreferencesGroup::builder()
        .title("Configuration file")
        .build();
    let path = Config::path();
    let cfg_row = adw::ActionRow::builder()
        .title("Location")
        .subtitle(crate::config::abbreviate_home(&path))
        .css_classes(["property"])
        .build();
    let open_dir = gtk::Button::builder()
        .icon_name("folder-open-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("Show in file manager")
        .build();
    open_dir.connect_clicked(move |_| {
        if let Some(dir) = path.parent() {
            let uri = format!("file://{}", dir.display());
            let _ =
                gtk::gio::AppInfo::launch_default_for_uri(&uri, gtk::gio::AppLaunchContext::NONE);
        }
    });
    cfg_row.add_suffix(&open_dir);
    about.add(&cfg_row);
    page.add(&about);

    let _ = window;
    page
}

// ---------------------------------------------------------------- helpers

/// Mutate the shared config and persist it.
/// The GNOME Shell extension: status, install, update, remove, and the
/// switch that tells the daemon whether to use it.
fn extension_group(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesGroup {
    use crate::gnome_shell::{self, Status};
    let group = adw::PreferencesGroup::builder()
        .title("GNOME Shell extension")
        .description(
            "Optional. Lets Parsec switch between open windows, paste clipboard entries \
             straight into the window you came from, and follow the clipboard without polling.",
        )
        .build();

    let row = adw::ActionRow::builder().title("Extension").build();
    row.add_prefix(&gtk::Image::from_icon_name("application-x-addon-symbolic"));
    let action = gtk::Button::builder().valign(gtk::Align::Center).build();
    let remove = gtk::Button::builder()
        .label("Remove")
        .valign(gtk::Align::Center)
        .build();
    row.add_suffix(&action);
    row.add_suffix(&remove);

    let refresh: Rc<dyn Fn()> = {
        let (row, action, remove) = (row.clone(), action.clone(), remove.clone());
        Rc::new(move || {
            let status = gnome_shell::status();
            row.set_subtitle(&status.describe());
            match &status {
                Status::Unsupported => {
                    action.set_visible(false);
                    remove.set_visible(false);
                }
                Status::NotInstalled => {
                    action.set_label("Install");
                    action.set_visible(true);
                    remove.set_visible(false);
                }
                Status::Installed { outdated, .. } => {
                    action.set_label("Update");
                    action.set_visible(*outdated);
                    remove.set_visible(true);
                }
            }
        })
    };
    refresh();

    action.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        refresh,
        move |_| {
            match gnome_shell::install() {
                Ok(true) => window.add_toast(adw::Toast::new(
                    "Extension installed. Log out and back in to activate it.",
                )),
                Ok(false) => window.add_toast(adw::Toast::new(
                    "Extension updated. Log out and back in to load the new version.",
                )),
                Err(e) => window.add_toast(adw::Toast::new(&format!("Install failed: {e:#}"))),
            }
            refresh();
        }
    ));
    remove.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        refresh,
        move |_| {
            match gnome_shell::remove() {
                Ok(()) => window.add_toast(adw::Toast::new("Extension removed")),
                Err(e) => window.add_toast(adw::Toast::new(&format!("Remove failed: {e:#}"))),
            }
            refresh();
        }
    ));
    // Follow the bus, so the row updates when the extension comes and goes.
    gnome_shell::bridge().on_state(glib::clone!(
        #[weak]
        row,
        #[strong]
        refresh,
        move |_| {
            let _ = &row;
            refresh();
        }
    ));
    group.add(&row);

    let enabled = cfg.borrow().shell.extension;
    group.add(&switch_row(
        "Use the extension",
        "Off keeps it installed but Parsec behaves as if it were absent",
        enabled,
        glib::clone!(
            #[strong]
            cfg,
            move |on| edit(&cfg, |c| c.shell.extension = on)
        ),
    ));
    group
}

fn edit(cfg: &SharedConfig, f: impl FnOnce(&mut Config)) {
    let mut c = cfg.borrow_mut();
    f(&mut c);
    c.save();
}

fn refresh_roots(
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    cfg: &SharedConfig,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    let roots = cfg.borrow().projects.roots.clone();
    let mut new_rows = Vec::new();
    if roots.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No folders yet")
            .subtitle("Add one with the + button")
            .sensitive(false)
            .build();
        group.add(&row);
        new_rows.push(row);
    }
    for root in roots {
        let row = adw::ActionRow::builder().title(&root).build();
        row.add_prefix(&gtk::Image::from_icon_name("folder-symbolic"));
        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Remove")
            .build();
        remove.connect_clicked(glib::clone!(
            #[strong]
            group,
            #[strong]
            rows,
            #[strong]
            cfg,
            move |_| {
                edit(&cfg, |c| c.projects.roots.retain(|r| r != &root));
                refresh_roots(&group, &rows, &cfg);
            }
        ));
        row.add_suffix(&remove);
        group.add(&row);
        new_rows.push(row);
    }
    *rows.borrow_mut() = new_rows;
}

/// A file path, typed or picked. Shown with `~`, stored the same way.
fn path_row(
    window: &adw::PreferencesWindow,
    title: &str,
    value: &str,
    patterns: &[&str],
    on_apply: impl Fn(String) + 'static,
) -> adw::EntryRow {
    let on_apply = Rc::new(on_apply);
    let row = text_row(
        title,
        "",
        value,
        glib::clone!(
            #[strong]
            on_apply,
            move |text| {
                on_apply(text.trim().to_string());
                true
            }
        ),
    );
    let pick = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("Choose file")
        .build();
    let filter = gtk::FileFilter::new();
    for p in patterns {
        filter.add_pattern(p);
    }
    let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
    filters.append(&filter);
    pick.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        row,
        #[strong]
        filters,
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose file")
                .filters(&filters)
                .build();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                row,
                #[strong]
                on_apply,
                async move {
                    if let Ok(file) = dialog.open_future(Some(&window)).await {
                        if let Some(path) = file.path() {
                            let shown = crate::config::abbreviate_home(&path);
                            row.set_text(&shown);
                            on_apply(shown);
                        }
                    }
                }
            ));
        }
    ));
    row.add_suffix(&pick);
    row
}

/// Free text with an apply button. Also commits when focus leaves the row, so
/// an edit isn't lost by clicking elsewhere or closing the window. `on_apply`
/// returns false to flag invalid input.
fn text_row(
    title: &str,
    subtitle: &str,
    value: &str,
    on_apply: impl Fn(&str) -> bool + 'static,
) -> adw::EntryRow {
    let row = adw::EntryRow::builder()
        .title(title)
        .text(value)
        .show_apply_button(true)
        .build();
    if !subtitle.is_empty() {
        row.set_tooltip_text(Some(subtitle));
    }
    // Last committed text, so leaving an untouched row doesn't save.
    let committed = Rc::new(RefCell::new(value.to_string()));
    let commit = Rc::new(move |r: &adw::EntryRow| {
        let text = r.text();
        if text.as_str() == committed.borrow().as_str() {
            r.remove_css_class("error");
        } else if on_apply(text.as_str()) {
            r.remove_css_class("error");
            *committed.borrow_mut() = text.to_string();
        } else {
            r.add_css_class("error");
        }
    });
    row.connect_apply(glib::clone!(
        #[strong]
        commit,
        move |r| commit(r)
    ));
    let focus = gtk::EventControllerFocus::new();
    focus.connect_leave(glib::clone!(
        #[weak]
        row,
        move |_| commit(&row)
    ));
    row.add_controller(focus);
    row
}

/// An argv shown as one shell-quoted line; parsed back with shell rules.
fn argv_row(
    title: &str,
    argv: &[String],
    on_apply: impl Fn(Vec<String>) + 'static,
) -> adw::EntryRow {
    let shown = shlex::try_join(argv.iter().map(String::as_str)).unwrap_or_default();
    text_row(title, "", &shown, move |text| match shlex::split(text) {
        Some(parts) if !parts.is_empty() => {
            on_apply(parts);
            true
        }
        _ => false,
    })
}

fn switch_row(
    title: &str,
    subtitle: &str,
    active: bool,
    on_change: impl Fn(bool) + 'static,
) -> adw::SwitchRow {
    let row = adw::SwitchRow::builder()
        .title(title)
        .subtitle(subtitle)
        .active(active)
        .build();
    row.connect_active_notify(move |r| on_change(r.is_active()));
    row
}

fn spin_row(
    title: &str,
    subtitle: &str,
    value: f64,
    min: f64,
    max: f64,
    on_change: impl Fn(f64) + 'static,
) -> adw::SpinRow {
    let row = adw::SpinRow::with_range(min, max, 1.0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row.set_value(value);
    row.connect_value_notify(move |r| on_change(r.value()));
    row
}

/// A row that acts as a button: icon, title, chevron. (ButtonRow is 1.6+.)
fn link_row(title: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .activatable(true)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}
