//! Settings › Plugins: installed plugins with an on/off switch and a menu
//! (settings, folder, remove), updates for those installed from git (what
//! changed, then install), and install from a zip, a folder, or a git URL.

use crate::config::SharedConfig;
use crate::plugins;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub fn page(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
        .name("plugins")
        .title("Plugins")
        .icon_name("application-x-addon-symbolic")
        .build();

    // Installed.
    let installed = adw::PreferencesGroup::builder()
        .title("Installed")
        .description(
            "Each plugin is a small program Parsec talks to. Keywords are shown next to the name.",
        )
        .build();
    let rows: Rc<RefCell<Vec<adw::ActionRow>>> = Rc::new(RefCell::new(Vec::new()));
    refresh(&installed, &rows, &cfg);
    page.add(&installed);

    let check = gtk::Button::builder()
        .label("Check for updates")
        .valign(gtk::Align::Center)
        .css_classes(["flat"])
        .tooltip_text("Asks each plugin's git repository for new commits")
        .build();
    check.connect_clicked(glib::clone!(
        #[weak]
        window,
        move |check| {
            crate::ui::preferences::working(check, "Checking…");
            let check = check.clone();
            glib::spawn_future_local(async move {
                let (_, found) = crate::update::check_now().await;
                check.set_label("Check for updates");
                check.set_sensitive(true);
                let message = match found.len() {
                    0 => "Every plugin is up to date".to_string(),
                    1 => "1 plugin has an update".to_string(),
                    n => format!("{n} plugins have updates"),
                };
                window.add_toast(adw::Toast::new(&message));
            });
        }
    ));
    installed.set_header_suffix(Some(&check));
    // Rows follow every check, the daily one included, and every update.
    let (weak, weak_rows, weak_cfg) = (
        installed.downgrade(),
        Rc::downgrade(&rows),
        Rc::downgrade(&cfg),
    );
    crate::update::watch(move || {
        let (Some(group), Some(rows), Some(cfg)) =
            (weak.upgrade(), weak_rows.upgrade(), weak_cfg.upgrade())
        else {
            return false;
        };
        refresh(&group, &rows, &cfg);
        true
    });

    // Install.
    let install = adw::PreferencesGroup::builder()
        .title("Install")
        .description(
            "From a .zip or a folder containing plugin.toml, or by cloning a git repository.",
        )
        .build();

    let from_zip = link_row("From a zip file…", "package-x-generic-symbolic");
    from_zip.connect_activated(glib::clone!(
        #[weak]
        window,
        #[strong]
        installed,
        #[strong]
        rows,
        #[strong]
        cfg,
        move |_| {
            let filter = gtk::FileFilter::new();
            filter.add_pattern("*.zip");
            let filters = gtk::gio::ListStore::new::<gtk::FileFilter>();
            filters.append(&filter);
            let dialog = gtk::FileDialog::builder()
                .title("Choose plugin zip")
                .filters(&filters)
                .build();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                window,
                #[strong]
                installed,
                #[strong]
                rows,
                #[strong]
                cfg,
                async move {
                    if let Ok(file) = dialog.open_future(Some(&window)).await {
                        if let Some(p) = file.path() {
                            install_and_report(
                                &window,
                                &installed,
                                &rows,
                                &cfg,
                                &p.to_string_lossy(),
                            )
                            .await;
                        }
                    }
                }
            ));
        }
    ));
    install.add(&from_zip);

    let from_dir = link_row("From a folder…", "folder-symbolic");
    from_dir.connect_activated(glib::clone!(
        #[weak]
        window,
        #[strong]
        installed,
        #[strong]
        rows,
        #[strong]
        cfg,
        move |_| {
            let dialog = gtk::FileDialog::builder()
                .title("Choose plugin folder")
                .build();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                window,
                #[strong]
                installed,
                #[strong]
                rows,
                #[strong]
                cfg,
                async move {
                    if let Ok(file) = dialog.select_folder_future(Some(&window)).await {
                        if let Some(p) = file.path() {
                            install_and_report(
                                &window,
                                &installed,
                                &rows,
                                &cfg,
                                &p.to_string_lossy(),
                            )
                            .await;
                        }
                    }
                }
            ));
        }
    ));
    install.add(&from_dir);

    let git = adw::EntryRow::builder()
        .title("Git repository URL")
        .show_apply_button(true)
        .build();
    git.connect_apply(glib::clone!(
        #[weak]
        window,
        #[strong]
        installed,
        #[strong]
        rows,
        #[strong]
        cfg,
        move |row| {
            let url = row.text().trim().to_string();
            if url.is_empty() {
                return;
            }
            let row = row.clone();
            glib::spawn_future_local(glib::clone!(
                #[strong]
                window,
                #[strong]
                installed,
                #[strong]
                rows,
                #[strong]
                cfg,
                async move {
                    row.set_sensitive(false);
                    install_and_report(&window, &installed, &rows, &cfg, &url).await;
                    row.set_sensitive(true);
                    row.set_text("");
                }
            ));
        }
    ));
    install.add(&git);

    let open_dir = link_row("Open plugins folder", "folder-open-symbolic");
    open_dir.connect_activated(|_| {
        let dir = plugins::dir();
        let _ = std::fs::create_dir_all(&dir);
        let uri = format!("file://{}", dir.display());
        let _ = gtk::gio::AppInfo::launch_default_for_uri(
            &uri,
            crate::core::scope::launch_context().as_ref(),
        );
    });
    install.add(&open_dir);
    page.add(&install);

    page
}

async fn install_and_report(
    window: &adw::PreferencesWindow,
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    cfg: &SharedConfig,
    source: &str,
) {
    let source = source.to_string();
    let result =
        gtk::gio::spawn_blocking(move || plugins::install(&source).map_err(|e| format!("{e:#}")))
            .await
            .unwrap_or_else(|_| Err("install thread failed".into()));
    let toast = match result {
        Ok(p) => adw::Toast::new(&format!(
            "Installed {} {}",
            p.manifest.name, p.manifest.version
        )),
        Err(e) => adw::Toast::builder().title(&e).timeout(8).build(),
    };
    window.add_toast(toast);
    refresh(group, rows, cfg);
}

fn refresh(
    group: &adw::PreferencesGroup,
    rows: &Rc<RefCell<Vec<adw::ActionRow>>>,
    cfg: &SharedConfig,
) {
    for row in rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    let all = plugins::installed();
    let disabled = cfg.borrow().plugins.disabled.clone();
    let updates = crate::update::plugin_updates();
    let mut new_rows = Vec::new();
    if all.is_empty() {
        let row = adw::ActionRow::builder()
            .title("No plugins installed")
            .subtitle("Try the calculator from the Parsec repository: examples/plugins/calc")
            .sensitive(false)
            .build();
        group.add(&row);
        new_rows.push(row);
    }
    for p in all {
        let m = p.manifest.clone();
        let keywords = if m.keywords.is_empty() {
            "runs on every query".to_string()
        } else {
            m.keywords.join(", ")
        };
        let row = adw::ActionRow::builder()
            .title(format!(
                "{}  <span size='small' alpha='60%'>{}</span>",
                glib::markup_escape_text(&m.name),
                glib::markup_escape_text(&m.version)
            ))
            .use_markup(true)
            .subtitle(format!("{}  ·  {}", keywords, m.description))
            .build();
        let image = gtk::Image::builder().pixel_size(24).build();
        match p.icon() {
            crate::core::Icon::Path(path) => image.set_from_file(Some(path)),
            crate::core::Icon::Named(n) => image.set_icon_name(Some(&n)),
            _ => image.set_icon_name(Some("application-x-addon-symbolic")),
        }
        row.add_prefix(&image);

        let switch = gtk::Switch::builder()
            .active(!disabled.contains(&m.id))
            .valign(gtk::Align::Center)
            .tooltip_text("Enabled")
            .build();
        let id = m.id.clone();
        switch.connect_active_notify(glib::clone!(
            #[strong]
            cfg,
            move |s| {
                let on = s.is_active();
                let mut c = cfg.borrow_mut();
                c.plugins.disabled.retain(|d| d != &id);
                if !on {
                    c.plugins.disabled.push(id.clone());
                }
                c.save();
            }
        ));
        // Every row ends the same way, a menu then the switch, so the
        // switches line up whatever each plugin offers.
        let actions = gtk::gio::SimpleActionGroup::new();
        let menu = gtk::gio::Menu::new();
        if !m.config.is_empty() {
            let settings = gtk::gio::SimpleAction::new("settings", None);
            let plugin = p.clone();
            settings.connect_activate(move |_, _| match plugins::ensure_settings(&plugin) {
                Ok(path) => open_path(&path),
                Err(e) => tracing::warn!("{e:#}"),
            });
            actions.add_action(&settings);
            menu.append(Some("Edit settings"), Some("plugin.settings"));
        }
        let folder = gtk::gio::SimpleAction::new("folder", None);
        let dir = p.dir.clone();
        folder.connect_activate(move |_, _| open_path(&dir));
        actions.add_action(&folder);
        menu.append(Some("Open folder"), Some("plugin.folder"));

        let remove = gtk::gio::SimpleAction::new("remove", None);
        let id = m.id.clone();
        remove.connect_activate(glib::clone!(
            #[strong]
            group,
            #[strong]
            rows,
            #[strong]
            cfg,
            move |_, _| {
                if let Err(e) = plugins::remove(&id) {
                    tracing::warn!("{e:#}");
                }
                // Not while the menu that asked is still closing.
                let (group, rows, cfg) = (group.clone(), rows.clone(), cfg.clone());
                glib::idle_add_local_once(move || refresh(&group, &rows, &cfg));
            }
        ));
        actions.add_action(&remove);
        let danger = gtk::gio::Menu::new();
        danger.append(Some("Remove"), Some("plugin.remove"));
        menu.append_section(None, &danger);
        row.insert_action_group("plugin", Some(&actions));

        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .menu_model(&menu)
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("More")
            .build();
        if let Some(a) = updates.iter().find(|a| a.id == m.id) {
            let update = gtk::Button::builder()
                .label("Update…")
                .valign(gtk::Align::Center)
                .css_classes(["suggested-action"])
                .tooltip_text("See what changed, then update")
                .build();
            let a = a.clone();
            update.connect_clicked(move |button| {
                glib::spawn_future_local(review(button.clone(), a.clone()));
            });
            row.add_suffix(&update);
        }
        row.add_suffix(&more);
        row.add_suffix(&switch);
        group.add(&row);
        new_rows.push(row);
    }
    *rows.borrow_mut() = new_rows;
}

/// Show what an update brings and install it if asked to.
async fn review(button: gtk::Button, a: plugins::updates::Available) {
    let Some(window) = button.root().and_downcast::<adw::PreferencesWindow>() else {
        return;
    };
    crate::ui::preferences::working(&button, "Checking…");
    let origin = a.origin.clone();
    let changes = gtk::gio::spawn_blocking(move || plugins::updates::changes(&origin))
        .await
        .unwrap_or_else(|_| Err(anyhow::anyhow!("thread panicked")));
    button.set_label("Update…");
    button.set_sensitive(true);

    let mut body = Vec::new();
    let mut commits = Vec::new();
    match changes {
        Ok(c) => {
            match c.version.as_deref() {
                Some(v) if v != a.version => body.push(format!("{} → {v}", a.version)),
                _ => body.push(format!("Still version {}, with new commits.", a.version)),
            }
            if !c.complete {
                body.push(
                    "It was installed before Parsec kept track of plugin versions, \
                     so these are its latest changes."
                        .into(),
                );
            }
            commits = c.commits;
        }
        Err(e) => body.push(format!("Couldn't read what changed: {e:#}")),
    }
    let dialog = adw::AlertDialog::new(Some(&format!("Update {}?", a.name)), None);
    dialog.set_body(&body.join("\n\n"));
    if !commits.is_empty() {
        let lines: Vec<String> = commits
            .iter()
            .map(|(hash, subject)| {
                format!(
                    "<span font_family='monospace' alpha='60%'>{}</span>  {}",
                    glib::markup_escape_text(hash),
                    glib::markup_escape_text(subject)
                )
            })
            .collect();
        let list = gtk::Label::builder()
            .label(lines.join("\n"))
            .use_markup(true)
            .wrap(true)
            .xalign(0.0)
            .build();
        let scroller = gtk::ScrolledWindow::builder()
            .child(&list)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_height(true)
            .max_content_height(260)
            .build();
        dialog.set_extra_child(Some(&scroller));
    }
    dialog.add_responses(&[("cancel", "Cancel"), ("update", "Update")]);
    dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("update"));
    dialog.set_close_response("cancel");
    if dialog.choose_future(Some(&window)).await != "update" {
        return;
    }

    crate::ui::preferences::working(&button, "Updating…");
    // On success the rows are rebuilt, this button with them.
    let toast = match crate::update::update_plugin(&a).await {
        Ok(p) => adw::Toast::new(&format!(
            "Updated {} to {}",
            p.manifest.name, p.manifest.version
        )),
        Err(e) => {
            button.set_label("Update…");
            button.set_sensitive(true);
            adw::Toast::builder()
                .title(format!("{e:#}"))
                .timeout(8)
                .build()
        }
    };
    window.add_toast(toast);
}

fn open_path(path: &std::path::Path) {
    let uri = format!("file://{}", path.display());
    if let Err(e) = gtk::gio::AppInfo::launch_default_for_uri(
        &uri,
        crate::core::scope::launch_context().as_ref(),
    ) {
        tracing::warn!("opening {}: {e}", path.display());
    }
}

fn link_row(title: &str, icon: &str) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(title)
        .activatable(true)
        .build();
    row.add_prefix(&gtk::Image::from_icon_name(icon));
    row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
    row
}
