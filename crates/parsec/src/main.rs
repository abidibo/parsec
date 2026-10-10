mod app;
mod autostart;
mod brand;
mod config;
mod core;
mod detect;
mod gnome_shell;
mod plugins;
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
             parsec plugin list | install <zip|dir|git-url> | remove <id>\n  \
             parsec extension status | install | remove   (GNOME Shell extension)\n  \
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
    if args.get(1).map(String::as_str) == Some("plugin") {
        return plugin_command(&args[2..]);
    }
    if args.get(1).map(String::as_str) == Some("extension") {
        return extension_command(&args[2..]);
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

fn plugin_command(args: &[String]) -> glib::ExitCode {
    match args.first().map(String::as_str) {
        Some("list") => {
            let all = plugins::installed();
            if all.is_empty() {
                println!("no plugins in {}", plugins::dir().display());
            }
            for p in all {
                let m = &p.manifest;
                println!(
                    "{:<16} {:<8} {:<12} {}",
                    m.id,
                    m.version,
                    m.keywords.join(","),
                    m.description
                );
            }
            glib::ExitCode::SUCCESS
        }
        Some("install") => match args.get(1) {
            Some(src) => match plugins::install(src) {
                Ok(p) => {
                    println!(
                        "installed {} {} into {}",
                        p.manifest.id,
                        p.manifest.version,
                        p.dir.display()
                    );
                    glib::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e:#}");
                    glib::ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("usage: parsec plugin install <zip|dir|git-url>");
                glib::ExitCode::FAILURE
            }
        },
        Some("remove") => match args.get(1) {
            Some(id) => match plugins::remove(id) {
                Ok(()) => glib::ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("error: {e:#}");
                    glib::ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("usage: parsec plugin remove <id>");
                glib::ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: parsec plugin list | install <zip|dir|git-url> | remove <id>");
            glib::ExitCode::FAILURE
        }
    }
}

fn extension_command(args: &[String]) -> glib::ExitCode {
    match args.first().map(String::as_str) {
        Some("status") => {
            // Ask the bus so "running" is accurate.
            glib::MainContext::default().block_on(gnome_shell::bridge().connect());
            println!("{}", gnome_shell::status().describe());
            println!("folder: {}", gnome_shell::dir().display());
            glib::ExitCode::SUCCESS
        }
        Some("install") => {
            glib::MainContext::default().block_on(gnome_shell::bridge().connect());
            match gnome_shell::install() {
                Ok(needs_logout) => {
                    println!("installed into {}", gnome_shell::dir().display());
                    if needs_logout {
                        println!("log out and back in so GNOME Shell loads it");
                    }
                    glib::ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("error: {e:#}");
                    glib::ExitCode::FAILURE
                }
            }
        }
        Some("remove") => match gnome_shell::remove() {
            Ok(()) => glib::ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("error: {e:#}");
                glib::ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("usage: parsec extension status | install | remove");
            glib::ExitCode::FAILURE
        }
    }
}

/// Run one search through the engine with no window, for debugging providers.
fn query_once(text: &str) -> glib::ExitCode {
    let cfg = std::rc::Rc::new(std::cell::RefCell::new(config::Config::load()));
    let bridge = gnome_shell::bridge();
    bridge.set_enabled(cfg.borrow().shell.extension);
    let engine = core::Engine::new(providers::all(cfg, false));
    let started = std::time::Instant::now();
    let items = glib::MainContext::default().block_on(async {
        bridge.connect().await;
        engine.search(text).await
    });
    for (i, hit) in items.iter().enumerate() {
        let item = &hit.item;
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
