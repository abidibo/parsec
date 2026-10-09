//! GApplication wiring. The first `parsec` process becomes the resident
//! daemon; every later `parsec` invocation is forwarded to it as an
//! `activate`, which toggles the window. That is our D-Bus toggle, for free,
//! via GApplication's single-instance machinery.

use crate::config::Config;
use crate::core::Engine;
use crate::providers;
use crate::ui::LauncherWindow;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use crate::brand::APP_ID;

pub fn build(background: bool, open_settings: bool) -> adw::Application {
    let app = adw::Application::builder()
        .application_id(APP_ID)
        .flags(gio::ApplicationFlags::default())
        .build();

    let window: Rc<RefCell<Option<LauncherWindow>>> = Rc::new(RefCell::new(None));
    // True only for the first activate of a process started with --background.
    let skip_first_show = Rc::new(Cell::new(background));

    app.connect_startup(glib::clone!(
        #[strong]
        window,
        move |app| {
            tracing::info!("starting daemon");
            // The panel is dark by design; keep GTK's text colours consistent.
            adw::StyleManager::default().set_color_scheme(adw::ColorScheme::ForceDark);
            let cfg: providers::SharedConfig = Rc::new(RefCell::new(Config::load()));
            watch_config(cfg.clone());
            register_actions(app, cfg.clone());
            let engine = Rc::new(Engine::new(providers::all(cfg, true)));
            *window.borrow_mut() = Some(LauncherWindow::new(app, engine));
            // Stay alive with the window hidden. The guard releases on drop,
            // and we want the hold to last as long as the process does.
            std::mem::forget(app.hold());
        }
    ));

    app.connect_activate(move |app| {
        tracing::debug!("activate received");
        if open_settings {
            app.activate_action("preferences", None);
            return;
        }
        if skip_first_show.replace(false) {
            tracing::info!("background start, window stays hidden");
            return;
        }
        if let Some(w) = window.borrow().as_ref() {
            w.toggle();
        }
    });

    app
}

/// `app.preferences` opens the settings window, `app.quit` stops the daemon.
/// Reachable from the launcher (Ctrl+,), the system provider and the CLI.
fn register_actions(app: &adw::Application, cfg: providers::SharedConfig) {
    let prefs = gio::SimpleAction::new("preferences", None);
    prefs.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| crate::ui::preferences::open(&app, cfg.clone())
    ));
    app.add_action(&prefs);

    let quit = gio::SimpleAction::new("quit", None);
    quit.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            tracing::info!("quit requested");
            app.quit();
        }
    ));
    app.add_action(&quit);
}

/// Reload the config whenever its file changes. The monitor is leaked on
/// purpose: it must live as long as the daemon.
fn watch_config(cfg: providers::SharedConfig) {
    let file = gio::File::for_path(Config::path());
    match file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
        Ok(monitor) => {
            monitor.connect_changed(move |_, _, _, event| {
                use gio::FileMonitorEvent as E;
                if matches!(event, E::ChangesDoneHint | E::Created | E::Renamed) {
                    tracing::info!("config changed, reloading");
                    *cfg.borrow_mut() = Config::load();
                }
            });
            std::mem::forget(monitor);
        }
        Err(e) => tracing::warn!("cannot watch config file: {e}"),
    }
}
