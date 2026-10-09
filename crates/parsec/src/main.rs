mod app;
mod autostart;
mod brand;
mod config;
mod core;
mod detect;
mod providers;
mod ui;

use gtk::prelude::*;

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "parsec=info".into()),
        )
        .with_target(false)
        .init();

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "parsec - a personal GNOME launcher\n\n\
             usage:\n  parsec               start the daemon, or toggle the window if it runs\n  \
             parsec query <text>  run a search once and print results (debugging)\n  \
             parsec config        print the effective configuration and its path\n  \
             parsec settings      open the settings window\n  \
             parsec --background  start the daemon without showing the window\n  \
             parsec toggle        same as plain `parsec`, reads better in keybindings"
        );
        return glib::ExitCode::SUCCESS;
    }
    if args.get(1).map(String::as_str) == Some("config") {
        println!("# {}", config::Config::path().display());
        print!("{}", config::Config::load().to_commented_toml());
        return glib::ExitCode::SUCCESS;
    }
    if args.get(1).map(String::as_str) == Some("query") {
        let text = args[2..].join(" ");
        return query_once(&text);
    }
    let background = args.iter().any(|a| a == "--background");
    let settings = args.get(1).map(String::as_str) == Some("settings");

    let app = app::build(background, settings);
    if settings {
        // With a daemon running, hand it the action instead of activating.
        if app.register(gtk::gio::Cancellable::NONE).is_ok() && app.is_remote() {
            app.activate_action("preferences", None);
            return glib::ExitCode::SUCCESS;
        }
        // Otherwise this process becomes the daemon and opens settings.
    }
    // GApplication would otherwise try to parse our flags.
    app.run_with_args::<&str>(&[])
}

use gtk::glib;

/// Run one search through the engine with no window, for debugging providers.
fn query_once(text: &str) -> glib::ExitCode {
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(config::Config::load()));
    let engine = core::Engine::new(providers::all(cfg, false));
    let started = std::time::Instant::now();
    let items = glib::MainContext::default().block_on(engine.search(text));
    for (i, item) in items.iter().enumerate() {
        let actions: Vec<&str> = item.actions.iter().map(|a| a.label.as_str()).collect();
        println!(
            "{:>2}. {:<40} {:<40} [{}]",
            i + 1,
            item.title,
            item.subtitle.as_deref().unwrap_or(""),
            actions.join(" | ")
        );
    }
    eprintln!("{} results in {:?}", items.len(), started.elapsed());
    glib::ExitCode::SUCCESS
}
