//! Settings › Shortcuts: the list of user keywords and the editor dialog.

use crate::config::{SharedConfig, Shortcut};
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub fn page(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .title("Shortcuts")
        .icon_name("insert-link-symbolic")
        .build();

    let group = adw::PreferencesGroup::builder()
        .title("Custom shortcuts")
        .description(
            "A keyword that opens a search URL or runs a script with what you typed. \
             Use {query} (or %s) in a URL; a script gets the text as $1.",
        )
        .build();
    let add = gtk::Button::builder()
        .icon_name("list-add-symbolic")
        .tooltip_text("Add shortcut")
        .css_classes(["flat"])
        .build();
    group.set_header_suffix(Some(&add));
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    refresh(&group, &rows, &cfg, window);
    add.connect_clicked(glib::clone!(
        #[weak]
        window,
        #[strong]
        group,
        #[strong]
        rows,
        #[strong]
        cfg,
        move |_| {
            let (group, rows, cfg2) = (group.clone(), rows.clone(), cfg.clone());
            let w = window.clone();
            editor(&window, Shortcut::default(), cfg.clone(), move |s| {
                edit(&cfg2, |c| c.shortcuts.push(s));
                refresh(&group, &rows, &cfg2, &w);
            });
        }
    ));
    page.add(&group);
    page
}

fn edit(cfg: &SharedConfig, f: impl FnOnce(&mut crate::config::Config)) {
    let mut c = cfg.borrow_mut();
    f(&mut c);
    c.save();
}

fn refresh(
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    cfg: &SharedConfig,
    window: &adw::PreferencesWindow,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    let shortcuts = cfg.borrow().shortcuts.clone();
    let mut new_rows = Vec::new();
    if shortcuts.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No shortcuts yet")
            .subtitle("Add one with the + button")
            .sensitive(false)
            .build();
        group.add(&row);
        new_rows.push(row);
    }
    for (index, s) in shortcuts.into_iter().enumerate() {
        let row = adw::ActionRow::builder()
            .title(&s.name)
            .subtitle(format!("{}  ·  {}", s.keyword, s.command.trim()))
            .activatable(true)
            .build();
        row.add_prefix(&icon_widget(&s));
        if s.default_search {
            row.add_suffix(
                &gtk::Label::builder()
                    .label("default")
                    .css_classes(["dim-label", "caption"])
                    .valign(gtk::Align::Center)
                    .build(),
            );
        }
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
            #[strong]
            window,
            move |_| {
                edit(&cfg, |c| {
                    if index < c.shortcuts.len() {
                        c.shortcuts.remove(index);
                    }
                });
                refresh(&group, &rows, &cfg, &window);
            }
        ));
        row.add_suffix(&remove);
        row.connect_activated(glib::clone!(
            #[strong]
            group,
            #[strong]
            rows,
            #[strong]
            cfg,
            #[strong]
            window,
            move |_| {
                let (group, rows, cfg2, w) =
                    (group.clone(), rows.clone(), cfg.clone(), window.clone());
                editor(&window, s.clone(), cfg.clone(), move |updated| {
                    edit(&cfg2, |c| {
                        if let Some(slot) = c.shortcuts.get_mut(index) {
                            *slot = updated;
                        }
                    });
                    refresh(&group, &rows, &cfg2, &w);
                });
            }
        ));
        group.add(&row);
        new_rows.push(row);
    }
    *rows.borrow_mut() = new_rows;
}

fn icon_widget(s: &Shortcut) -> gtk::Image {
    let image = gtk::Image::builder().pixel_size(24).build();
    match crate::providers::shortcuts::icon_of(s) {
        crate::core::Icon::Path(p) => image.set_from_file(Some(p)),
        crate::core::Icon::Named(n) => image.set_icon_name(Some(&n)),
        _ => image.set_icon_name(Some("web-browser-symbolic")),
    }
    image
}

/// The editor dialog. `on_save` receives the validated shortcut.
fn editor(
    parent: &adw::PreferencesWindow,
    initial: Shortcut,
    cfg: SharedConfig,
    on_save: impl Fn(Shortcut) + 'static,
) {
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(560)
        .default_height(620)
        .title(if initial.name.is_empty() {
            "New shortcut"
        } else {
            "Edit shortcut"
        })
        .build();

    let cancel = gtk::Button::with_label("Cancel");
    let save = gtk::Button::builder()
        .label("Save")
        .css_classes(["suggested-action"])
        .build();
    let header = adw::HeaderBar::builder()
        .show_start_title_buttons(false)
        .show_end_title_buttons(false)
        .build();
    header.pack_start(&cancel);
    header.pack_end(&save);

    let name = adw::EntryRow::builder()
        .title("Name")
        .text(&initial.name)
        .build();
    let keyword = adw::EntryRow::builder()
        .title("Keyword")
        .text(&initial.keyword)
        .build();
    let icon = adw::EntryRow::builder()
        .title("Icon (theme name or image file, optional)")
        .text(&initial.icon)
        .build();
    let pick_icon = gtk::Button::builder()
        .icon_name("document-open-symbolic")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("Choose image")
        .build();
    pick_icon.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        #[strong]
        icon,
        move |_| {
            let filter = gtk::FileFilter::new();
            filter.add_mime_type("image/*");
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let chooser = gtk::FileDialog::builder()
                .title("Choose icon")
                .filters(&filters)
                .build();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                icon,
                async move {
                    if let Ok(file) = chooser.open_future(Some(&dialog)).await {
                        if let Some(p) = file.path() {
                            icon.set_text(&crate::config::abbreviate_home(&p));
                        }
                    }
                }
            ));
        }
    ));
    icon.add_suffix(&pick_icon);

    let basics = adw::PreferencesGroup::new();
    basics.add(&name);
    basics.add(&keyword);
    basics.add(&icon);

    let command_group = adw::PreferencesGroup::builder()
        .title("Query or script")
        .description(
            "A URL with {query} or %s where the text goes, for example \
             https://duckduckgo.com/?q={query}. Anything else runs as a shell script \
             with the text as $1 and $PARSEC_QUERY.",
        )
        .build();
    let command = gtk::TextView::builder()
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(10)
        .right_margin(10)
        .build();
    command.buffer().set_text(&initial.command);
    let scroller = gtk::ScrolledWindow::builder()
        .child(&command)
        .min_content_height(140)
        .css_classes(["card"])
        .build();
    command_group.add(&scroller);

    let options = adw::PreferencesGroup::new();
    let default_search = adw::SwitchRow::builder()
        .title("Default search")
        .subtitle("Suggest this shortcut when nothing else matches what you typed")
        .active(initial.default_search)
        .build();
    let without_args = adw::SwitchRow::builder()
        .title("Run without arguments")
        .subtitle("Enter on the bare keyword runs it with an empty query")
        .active(initial.run_without_args)
        .build();
    options.add(&default_search);
    options.add(&without_args);

    let page = adw::PreferencesPage::new();
    page.add(&basics);
    page.add(&command_group);
    page.add(&options);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&header);
    content.append(&page);
    dialog.set_content(Some(&content));

    cancel.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| dialog.close()
    ));
    let original_keyword = initial.keyword.clone();
    save.connect_clicked(glib::clone!(
        #[weak]
        dialog,
        move |_| {
            let buffer = command.buffer();
            let text = buffer
                .text(&buffer.start_iter(), &buffer.end_iter(), false)
                .to_string();
            let s = Shortcut {
                name: name.text().trim().to_string(),
                keyword: keyword.text().trim().to_string(),
                command: text.trim().to_string(),
                icon: icon.text().trim().to_string(),
                default_search: default_search.is_active(),
                run_without_args: without_args.is_active(),
            };
            // Validation: everything required, keyword a single word, unique.
            let mut ok = true;
            for (row, bad) in [
                (&name, s.name.is_empty()),
                (
                    &keyword,
                    s.keyword.is_empty() || s.keyword.contains(char::is_whitespace),
                ),
            ] {
                if bad {
                    row.add_css_class("error");
                    ok = false;
                } else {
                    row.remove_css_class("error");
                }
            }
            let taken = s.keyword != original_keyword
                && cfg
                    .borrow()
                    .shortcuts
                    .iter()
                    .any(|o| o.keyword == s.keyword);
            if taken {
                keyword.add_css_class("error");
                keyword.set_title("Keyword (already used)");
                ok = false;
            }
            if s.command.is_empty() {
                scroller.add_css_class("error");
                ok = false;
            } else {
                scroller.remove_css_class("error");
            }
            if ok {
                on_save(s);
                dialog.close();
            }
        }
    ));
    dialog.present();
}
