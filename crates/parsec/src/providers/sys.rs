//! `sys`: the things you would otherwise reach through GNOME's quick
//! settings menu or a terminal. Toggles read their state when you type, so
//! the row tells you where you are and the action says what Enter does.
//! Session commands that end your work (log out, power off) go through
//! `gnome-session-quit`, which asks first.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Browse, Icon, Item, Provider, Query};
use crate::detect::which;
use async_trait::async_trait;
use gtk::gio;
use gtk::prelude::*;
use std::process::Command;
use std::rc::Rc;

pub struct SysProvider {
    cfg: SharedConfig,
}

impl SysProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self { cfg }
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn cmd(parts: &[&str]) -> ActionKind {
    ActionKind::Command(argv(parts))
}

fn settings(schema: &str) -> Option<gio::Settings> {
    gio::SettingsSchemaSource::default()?
        .lookup(schema, true)
        .map(|_| gio::Settings::new(schema))
}

fn gsetting_bool(schema: &str, key: &str) -> Option<bool> {
    Some(settings(schema)?.boolean(key))
}

fn set_gsetting_bool(schema: &'static str, key: &'static str, value: bool) -> ActionKind {
    ActionKind::Callback(Rc::new(move || {
        let s = settings(schema).ok_or_else(|| anyhow::anyhow!("no schema {schema}"))?;
        s.set_boolean(key, value)?;
        gio::Settings::sync();
        Ok(())
    }))
}

/// First line of a command's output, trimmed, if it ran.
fn output(parts: &[&str]) -> Option<String> {
    let (bin, args) = parts.split_first()?;
    which(bin)?;
    let out = Command::new(bin).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .map(|l| l.trim().to_string())
}

/// What a toggle row shows and does.
struct Toggle {
    id: &'static str,
    title: &'static str,
    icon: &'static str,
    keywords: &'static [&'static str],
    on: Option<bool>,
    turn_on: ActionKind,
    turn_off: ActionKind,
}

/// An item plus the words it should also match on.
type Entry = (Item, Vec<&'static str>);

fn toggle_item(t: Toggle) -> Option<Entry> {
    let on = t.on?;
    let (state, label, kind) = if on {
        ("On", "Turn off", t.turn_off)
    } else {
        ("Off", "Turn on", t.turn_on)
    };
    Some((
        Item {
            id: format!("sys:{}", t.id),
            title: format!("{}: {state}", t.title),
            subtitle: Some(format!("Enter: {}", label.to_lowercase())),
            icon: Icon::Named(t.icon.into()),
            score: 1,
            actions: vec![Action {
                label: label.into(),
                kind,
            }],
        },
        t.keywords.to_vec(),
    ))
}

/// States that need a child process, gathered off the main thread.
#[derive(Default, Clone)]
struct Probed {
    wifi: Option<bool>,
    bluetooth: Option<bool>,
    muted: Option<bool>,
    mic_muted: Option<bool>,
}

fn probe() -> Probed {
    Probed {
        wifi: output(&["nmcli", "radio", "wifi"]).map(|s| s == "enabled"),
        bluetooth: which("bluetoothctl").and_then(|_| {
            let out = Command::new("bluetoothctl").arg("show").output().ok()?;
            let text = String::from_utf8_lossy(&out.stdout);
            text.lines()
                .find(|l| l.trim_start().starts_with("Powered:"))
                .map(|l| l.contains("yes"))
        }),
        muted: output(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SINK@"])
            .map(|s| s.contains("MUTED")),
        mic_muted: output(&["wpctl", "get-volume", "@DEFAULT_AUDIO_SOURCE@"])
            .map(|s| s.contains("MUTED")),
    }
}

fn toggles(p: &Probed) -> Vec<Entry> {
    const IFACE: &str = "org.gnome.desktop.interface";
    const NOTIF: &str = "org.gnome.desktop.notifications";
    const COLOR: &str = "org.gnome.settings-daemon.plugins.color";
    let dark = settings(IFACE).map(|s| s.string("color-scheme") == "prefer-dark");
    let set_scheme = |value: &'static str| {
        ActionKind::Callback(Rc::new(move || {
            let s = settings(IFACE).ok_or_else(|| anyhow::anyhow!("no interface schema"))?;
            s.set_string("color-scheme", value)?;
            gio::Settings::sync();
            Ok(())
        }))
    };
    [
        Toggle {
            id: "dark",
            title: "Dark style",
            icon: "weather-clear-night-symbolic",
            keywords: &["dark mode", "light", "theme", "appearance"],
            on: dark,
            turn_on: set_scheme("prefer-dark"),
            turn_off: set_scheme("default"),
        },
        Toggle {
            id: "dnd",
            title: "Do not disturb",
            icon: "notifications-disabled-symbolic",
            keywords: &["notifications", "dnd", "quiet", "focus"],
            on: gsetting_bool(NOTIF, "show-banners").map(|b| !b),
            turn_on: set_gsetting_bool(NOTIF, "show-banners", false),
            turn_off: set_gsetting_bool(NOTIF, "show-banners", true),
        },
        Toggle {
            id: "nightlight",
            title: "Night light",
            icon: "night-light-symbolic",
            keywords: &["blue light", "warm", "display"],
            on: gsetting_bool(COLOR, "night-light-enabled"),
            turn_on: set_gsetting_bool(COLOR, "night-light-enabled", true),
            turn_off: set_gsetting_bool(COLOR, "night-light-enabled", false),
        },
        Toggle {
            id: "wifi",
            title: "Wi-Fi",
            icon: "network-wireless-symbolic",
            keywords: &["wireless", "network", "wlan"],
            on: p.wifi,
            turn_on: cmd(&["nmcli", "radio", "wifi", "on"]),
            turn_off: cmd(&["nmcli", "radio", "wifi", "off"]),
        },
        Toggle {
            id: "bluetooth",
            title: "Bluetooth",
            icon: "bluetooth-active-symbolic",
            keywords: &["bt", "headphones", "wireless"],
            on: p.bluetooth,
            turn_on: cmd(&["bluetoothctl", "power", "on"]),
            turn_off: cmd(&["bluetoothctl", "power", "off"]),
        },
        Toggle {
            id: "mute",
            title: "Mute sound",
            icon: "audio-volume-muted-symbolic",
            keywords: &["volume", "audio", "speaker", "silence"],
            on: p.muted,
            turn_on: cmd(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "1"]),
            turn_off: cmd(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "0"]),
        },
        Toggle {
            id: "micmute",
            title: "Mute microphone",
            icon: "microphone-disabled-symbolic",
            keywords: &["mic", "input", "audio", "meeting"],
            on: p.mic_muted,
            turn_on: cmd(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SOURCE@", "1"]),
            turn_off: cmd(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SOURCE@", "0"]),
        },
    ]
    .into_iter()
    .filter_map(toggle_item)
    .collect()
}

/// One-shot commands. Each is dropped when its program is missing.
fn commands() -> Vec<Entry> {
    struct C {
        id: &'static str,
        title: &'static str,
        subtitle: &'static str,
        icon: &'static str,
        keywords: &'static [&'static str],
        label: &'static str,
        argv: &'static [&'static str],
    }
    let list = [
        C {
            id: "lock",
            title: "Lock screen",
            subtitle: "Lock the session now",
            icon: "system-lock-screen-symbolic",
            keywords: &["lock", "away"],
            label: "Lock",
            argv: &["loginctl", "lock-session"],
        },
        C {
            id: "suspend",
            title: "Suspend",
            subtitle: "Sleep until a key is pressed",
            icon: "weather-clear-night-symbolic",
            keywords: &["sleep", "standby"],
            label: "Suspend",
            argv: &["systemctl", "suspend"],
        },
        C {
            id: "logout",
            title: "Log out",
            subtitle: "GNOME asks before ending the session",
            icon: "system-log-out-symbolic",
            keywords: &["sign out", "session"],
            label: "Log out…",
            argv: &["gnome-session-quit", "--logout"],
        },
        C {
            id: "reboot",
            title: "Restart",
            subtitle: "GNOME asks before restarting",
            icon: "system-reboot-symbolic",
            keywords: &["reboot"],
            label: "Restart…",
            argv: &["gnome-session-quit", "--reboot"],
        },
        C {
            id: "poweroff",
            title: "Power off",
            subtitle: "GNOME asks before shutting down",
            icon: "system-shutdown-symbolic",
            keywords: &["shutdown", "halt", "turn off"],
            label: "Power off…",
            argv: &["gnome-session-quit", "--power-off"],
        },
        C {
            id: "trash",
            title: "Empty trash",
            subtitle: "Delete everything in the trash",
            icon: "user-trash-symbolic",
            keywords: &["bin", "rubbish", "delete"],
            label: "Empty",
            argv: &["gio", "trash", "--empty"],
        },
    ];
    let mut items: Vec<Entry> = list
        .into_iter()
        .filter(|c| which(c.argv[0]).is_some())
        .map(|c| {
            (
                Item {
                    id: format!("sys:{}", c.id),
                    title: c.title.into(),
                    subtitle: Some(c.subtitle.into()),
                    icon: Icon::Named(c.icon.into()),
                    score: 1,
                    actions: vec![Action {
                        label: c.label.into(),
                        kind: ActionKind::Command(argv(c.argv)),
                    }],
                },
                c.keywords.to_vec(),
            )
        })
        .collect();

    // Screenshot: GNOME's own capture UI, through the Shell.
    items.push((
        Item {
            id: "sys:screenshot".into(),
            title: "Screenshot".into(),
            subtitle: Some("Open GNOME's screenshot tool".into()),
            icon: Icon::Named("camera-photo-symbolic".into()),
            score: 1,
            actions: vec![Action {
                label: "Capture".into(),
                kind: ActionKind::Callback(Rc::new(|| {
                    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
                    bus.call(
                        Some("org.gnome.Shell.Screenshot"),
                        "/org/gnome/Shell/Screenshot",
                        "org.gnome.Shell.Screenshot",
                        "InteractiveScreenshot",
                        None,
                        None,
                        gio::DBusCallFlags::NONE,
                        2000,
                        gio::Cancellable::NONE,
                        |r| {
                            if let Err(e) = r {
                                tracing::warn!("screenshot call failed: {e}");
                            }
                        },
                    );
                    Ok(())
                })),
            }],
        },
        vec!["capture", "screen", "grab"],
    ));

    // GNOME Settings panels, as a list to drill into.
    if which("gnome-control-center").is_some() {
        items.push((
            Item {
                id: "sys:settings".into(),
                title: "GNOME Settings".into(),
                subtitle: Some("Open a settings panel".into()),
                icon: Icon::Named("preferences-system-symbolic".into()),
                score: 1,
                actions: vec![
                    Action {
                        label: "Panels".into(),
                        kind: ActionKind::Browse(Browse::new("Settings", || async {
                            PANELS
                                .iter()
                                .map(|(panel, title)| Item {
                                    id: format!("sys:panel:{panel}"),
                                    title: (*title).into(),
                                    subtitle: None,
                                    icon: Icon::Named("preferences-system-symbolic".into()),
                                    score: 1,
                                    actions: vec![Action {
                                        label: "Open".into(),
                                        kind: cmd(&["gnome-control-center", panel]),
                                    }],
                                })
                                .collect()
                        })),
                    },
                    Action {
                        label: "Open".into(),
                        kind: cmd(&["gnome-control-center"]),
                    },
                ],
            },
            vec!["control center", "preferences", "panel"],
        ));
    }
    items
}

const PANELS: &[(&str, &str)] = &[
    ("wifi", "Wi-Fi"),
    ("network", "Network"),
    ("bluetooth", "Bluetooth"),
    ("display", "Displays"),
    ("sound", "Sound"),
    ("power", "Power"),
    ("multitasking", "Multitasking"),
    ("appearance", "Appearance"),
    ("notifications", "Notifications"),
    ("search", "Search"),
    ("applications", "Apps"),
    ("online-accounts", "Online Accounts"),
    ("privacy", "Privacy & Security"),
    ("sharing", "Sharing"),
    ("mouse", "Mouse & Touchpad"),
    ("keyboard", "Keyboard"),
    ("printers", "Printers"),
    ("color", "Color"),
    ("region", "Region & Language"),
    ("universal-access", "Accessibility"),
    ("user-accounts", "Users"),
    ("datetime", "Date & Time"),
    ("info-overview", "About"),
];

#[async_trait(?Send)]
impl Provider for SysProvider {
    fn id(&self) -> &'static str {
        "sys"
    }

    fn title(&self) -> String {
        "System".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.system.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let probed = gio::spawn_blocking(probe).await.unwrap_or_default();
        let mut entries = toggles(&probed);
        entries.extend(commands());
        let n = entries.len() as u32;
        entries
            .into_iter()
            .enumerate()
            .filter_map(|(i, (mut it, keywords))| {
                it.score = if q.is_empty() {
                    n - i as u32
                } else {
                    q.score_any(
                        [it.title.as_str()]
                            .into_iter()
                            .chain(keywords.iter().copied()),
                    )?
                };
                Some(it)
            })
            .collect()
    }
}
