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

            let cfg: providers::SharedConfig = Rc::new(RefCell::new(Config::load()));
            watch_config(cfg.clone());
            let bridge = crate::gnome_shell::bridge();
            bridge.set_enabled(cfg.borrow().shell.extension);
            glib::spawn_future_local(async move { bridge.connect().await });
            register_actions(app, cfg.clone());
            let engine = Rc::new(Engine::new(providers::all(cfg.clone(), true)));
            let launcher = LauncherWindow::new(app, engine);
            crate::update::start(
                cfg.clone(),
                app,
                glib::clone!(
                    #[strong]
                    launcher,
                    move || launcher.show_updates()
                ),
            );
            *window.borrow_mut() = Some(launcher);
            crate::ui::theme::apply(&cfg);
            crate::ui::theme::watch(cfg);
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

/// `app.preferences` opens the settings window, `app.plugin-updates` its
/// Plugins page, `app.quit` stops the daemon, `app.update` installs the
/// release the update check found in the background and
/// `app.update-in-terminal` in a terminal, `app.whats-new` opens the running
/// version's release notes. Reachable from the launcher (Ctrl+,), the system
/// provider, notifications and the CLI.
fn register_actions(app: &adw::Application, cfg: providers::SharedConfig) {
    let prefs = gio::SimpleAction::new("preferences", None);
    prefs.connect_activate(glib::clone!(
        #[weak]
        app,
        #[strong]
        cfg,
        move |_, _| crate::ui::preferences::open(&app, cfg.clone())
    ));
    app.add_action(&prefs);

    let plugin_updates = gio::SimpleAction::new("plugin-updates", None);
    plugin_updates.connect_activate(glib::clone!(
        #[weak]
        app,
        #[strong]
        cfg,
        move |_, _| crate::ui::preferences::open_at(&app, cfg.clone(), "plugins")
    ));
    app.add_action(&plugin_updates);

    let update = gio::SimpleAction::new("update", None);
    update.connect_activate(glib::clone!(
        #[weak]
        app,
        move |_, _| {
            glib::spawn_future_local(async move {
                if let Err(failure) = crate::update::install().await {
                    tracing::warn!("update: {failure}");
                    notify_update_failure(&app, &failure);
                }
            });
        }
    ));
    app.add_action(&update);

    let in_terminal = gio::SimpleAction::new("update-in-terminal", None);
    in_terminal.connect_activate(move |_, _| {
        if let Err(e) = crate::update::install_in_terminal(&cfg.borrow()) {
            tracing::warn!("update: {e:#}");
        }
    });
    app.add_action(&in_terminal);

    let whats_new = gio::SimpleAction::new("whats-new", None);
    whats_new.connect_activate(|_, _| {
        let _ = gio::AppInfo::launch_default_for_uri(
            &crate::update::release_url(),
            crate::core::scope::launch_context().as_ref(),
        );
    });
    app.add_action(&whats_new);

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

/// A background update that didn't happen, with the terminal as a way on.
fn notify_update_failure(app: &adw::Application, failure: &crate::update::Failure) {
    use crate::update::Failure;
    let n = gio::Notification::new(match failure {
        Failure::NeedsTerminal(_) => "Parsec can't update in the background",
        Failure::Failed(_) => "Parsec couldn't update",
    });
    n.set_body(Some(&failure.to_string()));
    n.add_button("Update in a terminal", "app.update-in-terminal");
    app.send_notification(Some("update"), &n);
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
                    crate::ui::theme::apply(&cfg);
                    crate::gnome_shell::bridge().set_enabled(cfg.borrow().shell.extension);
                }
            });
            std::mem::forget(monitor);
        }
        Err(e) => tracing::warn!("cannot watch config file: {e}"),
    }
}
