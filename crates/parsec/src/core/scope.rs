//! Every program Parsec starts gets its own systemd scope, as GNOME Shell
//! does for the dash. Without one it stays in Parsec's scope: the system
//! counts its memory as Parsec's, systemd-oomd would kill it together with
//! the launcher, and nothing can tell which app it is from its cgroup.
//!
//! The scope is named the freedesktop way, `app-parsec-<app id>-<pid>.scope`
//! in `app.slice`, so tools that read the app id from the cgroup (portals,
//! system monitors) see the right one. Moving happens right after the fork,
//! over D-Bus; a child the app forks in that instant stays behind, the same
//! race GNOME Shell lives with. A process that exits before systemd moves
//! it leaves an empty scope systemd never collects, so a scope still empty
//! a moment later is stopped. Anything that fails (no systemd user manager,
//! cgroup v1) only logs: the program still runs.

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// A launch context for GIO launches whose children move to their own
/// scope. D-Bus-activated apps report no pid and are left alone: the bus
/// already started them in a unit of their own.
pub fn launch_context() -> Option<gtk::gdk::AppLaunchContext> {
    let ctx = gtk::gdk::Display::default()?.app_launch_context();
    ctx.connect_launched(|_, info, platform_data| {
        let pid = glib::VariantDict::new(Some(platform_data)).lookup::<i32>("pid");
        let Ok(Some(pid)) = pid else {
            return;
        };
        let id = info.id().map(|id| id.to_string()).unwrap_or_default();
        let id = id.strip_suffix(".desktop").unwrap_or(&id);
        adopt(pid as u32, id);
    });
    Some(ctx)
}

/// Move `pid` into a new scope named after `app_id`. Returns at once; the
/// D-Bus call completes on the main loop.
pub fn adopt(pid: u32, app_id: &str) {
    if pid <= 1 {
        return;
    }
    let unit = unit_name(app_id, pid);
    glib::spawn_future_local(async move {
        if let Err(e) = start_scope(&unit, pid).await {
            tracing::debug!(%unit, "no scope for launched process: {e}");
        }
    });
}

/// The app id for a spawned argv: its program's file name.
pub fn app_id_for(program: &str) -> &str {
    program.rsplit('/').next().unwrap_or(program)
}

/// How long a scope may stay empty before it counts as left behind.
const EMPTY_CHECK_SECS: u32 = 2;

async fn start_scope(unit: &str, pid: u32) -> Result<(), glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    manager_call(&bus, "StartTransientUnit", start_args(unit, pid)).await?;
    tracing::debug!(%unit, "launched process moved to its own scope");

    glib::timeout_future_seconds(EMPTY_CHECK_SECS).await;
    let Some(procs) = scope_procs_file(unit) else {
        return Ok(());
    };
    // Missing: systemd collected it after its processes ended. Empty: the
    // process was gone before the move. Only then is stopping it harmless.
    if std::fs::read_to_string(procs).is_ok_and(|p| p.trim().is_empty()) {
        manager_call(&bus, "StopUnit", (unit, "replace").to_variant()).await?;
        tracing::debug!(%unit, "stopped a scope its process never reached");
    }
    Ok(())
}

async fn manager_call(
    bus: &gio::DBusConnection,
    method: &str,
    args: glib::Variant,
) -> Result<glib::Variant, glib::Error> {
    bus.call_future(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        method,
        Some(&args),
        Some(glib::VariantTy::new("(o)").unwrap()),
        gio::DBusCallFlags::NONE,
        2000,
    )
    .await
}

/// `cgroup.procs` of a scope in our user manager's `app.slice`.
fn scope_procs_file(unit: &str) -> Option<std::path::PathBuf> {
    let own = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let manager = user_manager_cgroup(&own)?;
    Some(format!("/sys/fs/cgroup{manager}/app.slice/{unit}/cgroup.procs").into())
}

/// The `user@<uid>.service` part of a cgroup v2 path from `/proc/self/cgroup`.
fn user_manager_cgroup(proc_cgroup: &str) -> Option<&str> {
    let path = proc_cgroup.lines().find_map(|l| l.strip_prefix("0::"))?;
    let at = path.find("/user@")?;
    let end = at + path[at..].find(".service")? + ".service".len();
    Some(&path[..end])
}

/// `StartTransientUnit(name, mode, properties, aux)`.
fn start_args(unit: &str, pid: u32) -> glib::Variant {
    let props: Vec<(String, glib::Variant)> = vec![
        ("PIDs".into(), vec![pid].to_variant()),
        ("Slice".into(), "app.slice".to_variant()),
        ("CollectMode".into(), "inactive-or-failed".to_variant()),
        (
            "Description".into(),
            "Application launched by Parsec".to_variant(),
        ),
    ];
    let aux: Vec<(String, Vec<(String, glib::Variant)>)> = Vec::new();
    (unit, "fail", props, aux).to_variant()
}

fn unit_name(app_id: &str, pid: u32) -> String {
    let id = if app_id.is_empty() { "unknown" } else { app_id };
    format!("app-parsec-{}-{pid}.scope", escape(id))
}

/// systemd's unit name escaping (`systemd-escape`): ASCII letters, digits,
/// `:`, `_` and `.` stay, a leading `.` and every other byte become `\xNN`.
/// Dashes are escaped too, since they separate the name's parts.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for (i, b) in s.bytes().enumerate() {
        let keep = b.is_ascii_alphanumeric() || b == b':' || b == b'_' || (b == b'.' && i > 0);
        if keep {
            out.push(b as char);
        } else {
            out.push_str(&format!("\\x{b:02x}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_names_follow_gnome() {
        assert_eq!(
            unit_name("org.gnome.Nautilus", 42),
            "app-parsec-org.gnome.Nautilus-42.scope"
        );
        assert_eq!(
            unit_name("google-chrome", 7),
            "app-parsec-google\\x2dchrome-7.scope"
        );
        assert_eq!(unit_name("", 7), "app-parsec-unknown-7.scope");
    }

    #[test]
    fn escape_matches_systemd() {
        assert_eq!(escape(".hidden"), "\\x2ehidden");
        assert_eq!(escape("a b/é"), "a\\x20b\\x2f\\xc3\\xa9");
    }

    #[test]
    fn start_args_match_systemd_signature() {
        let args = start_args("app-parsec-x-1.scope", 1);
        assert_eq!(args.type_().as_str(), "(ssa(sv)a(sa(sv)))");
    }

    #[test]
    fn finds_user_manager_cgroup() {
        let own =
            "0::/user.slice/user-1000.slice/user@1000.service/app.slice/app-gnome-x-1.scope\n";
        assert_eq!(
            user_manager_cgroup(own),
            Some("/user.slice/user-1000.slice/user@1000.service")
        );
        assert_eq!(user_manager_cgroup("0::/system.slice/foo.service\n"), None);
        assert_eq!(user_manager_cgroup("1:name=systemd:/user.slice\n"), None);
    }

    #[test]
    fn app_id_is_program_name() {
        assert_eq!(app_id_for("/usr/bin/wezterm"), "wezterm");
        assert_eq!(app_id_for("kitty"), "kitty");
    }
}
