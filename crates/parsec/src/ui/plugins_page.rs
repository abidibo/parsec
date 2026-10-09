//! Settings › Plugins: installed plugins with enable/remove, and install
//! from a zip, a folder, or a git URL.

use crate::config::SharedConfig;
use crate::plugins;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub fn page(window: &adw::PreferencesWindow, cfg: SharedConfig) -> adw::PreferencesPage {
    let page = adw::PreferencesPage::builder()
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
        let _ = gtk::gio::AppInfo::launch_default_for_uri(&uri, gtk::gio::AppLaunchContext::NONE);
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
        row.add_suffix(&switch);

        let remove = gtk::Button::builder()
            .icon_name("user-trash-symbolic")
            .valign(gtk::Align::Center)
            .css_classes(["flat"])
            .tooltip_text("Remove")
            .build();
        let id = m.id.clone();
        remove.connect_clicked(glib::clone!(
            #[strong]
            group,
            #[strong]
            rows,
            #[strong]
            cfg,
            move |_| {
                if let Err(e) = plugins::remove(&id) {
                    tracing::warn!("{e:#}");
                }
                refresh(&group, &rows, &cfg);
            }
        ));
        row.add_suffix(&remove);
        group.add(&row);
        new_rows.push(row);
    }
    *rows.borrow_mut() = new_rows;
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
