//! The GNOME custom shortcut that toggles Parsec, read and written through
//! GSettings so the settings window can show and change it in place.

use adw::prelude::*;
use gtk::gio;
use gtk::glib;

const MEDIA_KEYS: &str = "org.gnome.settings-daemon.plugins.media-keys";
const BINDING_SCHEMA: &str = "org.gnome.settings-daemon.plugins.media-keys.custom-keybinding";
const PATH: &str = "/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/parsec/";

fn schema_exists(id: &str) -> bool {
    gio::SettingsSchemaSource::default()
        .and_then(|s| s.lookup(id, true))
        .is_some()
}

/// The command the shortcut should run: this binary, `toggle`.
fn toggle_command() -> String {
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "parsec".into());
    format!("{exe} toggle")
}

/// Make sure the custom shortcut entry exists and points at us.
fn ensure_entry() -> Option<gio::Settings> {
    if !schema_exists(MEDIA_KEYS) || !schema_exists(BINDING_SCHEMA) {
        return None;
    }
    let media = gio::Settings::new(MEDIA_KEYS);
    let mut list: Vec<String> = media
        .strv("custom-keybindings")
        .iter()
        .map(|s| s.to_string())
        .collect();
    if !list.iter().any(|p| p == PATH) {
        list.push(PATH.to_string());
        let refs: Vec<&str> = list.iter().map(String::as_str).collect();
        let _ = media.set_strv("custom-keybindings", refs.as_slice());
    }
    let binding = gio::Settings::with_path(BINDING_SCHEMA, PATH);
    if binding.string("name").is_empty() {
        let _ = binding.set_string("name", "Parsec");
    }
    let _ = binding.set_string("command", &toggle_command());
    gio::Settings::sync();
    Some(binding)
}

/// Current accelerator in GTK notation, e.g. `<Control>space`, if set.
pub fn current() -> Option<String> {
    if !schema_exists(BINDING_SCHEMA) {
        return None;
    }
    let binding = gio::Settings::with_path(BINDING_SCHEMA, PATH);
    let b = binding.string("binding").to_string();
    (!b.is_empty()).then_some(b)
}

/// Human label for an accelerator, e.g. `Ctrl+Space`.
pub fn label(accel: &str) -> String {
    match gtk::accelerator_parse(accel) {
        Some((key, mods)) => gtk::accelerator_get_label(key, mods).to_string(),
        None => accel.to_string(),
    }
}

pub fn set(accel: &str) -> bool {
    let Some(binding) = ensure_entry() else {
        return false;
    };
    let ok = binding.set_string("binding", accel).is_ok();
    gio::Settings::sync();
    tracing::info!(accel, "hotkey set");
    ok
}

/// A dialog that captures the next key combination and saves it.
pub fn capture(parent: &impl IsA<gtk::Window>, on_done: impl Fn(Option<String>) + 'static) {
    let dialog = adw::Window::builder()
        .transient_for(parent)
        .modal(true)
        .default_width(380)
        .default_height(180)
        .title("Set hotkey")
        .build();
    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .build();
    content.append(
        &gtk::Label::builder()
            .label("Press the new shortcut")
            .css_classes(["title-2"])
            .build(),
    );
    content.append(
        &gtk::Label::builder()
            .label("Use at least one modifier, for example Ctrl, Alt or Super.\nEsc cancels.")
            .justify(gtk::Justification::Center)
            .css_classes(["dim-label"])
            .build(),
    );
    dialog.set_content(Some(&content));

    let on_done = std::rc::Rc::new(on_done);
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    keys.connect_key_pressed(glib::clone!(
        #[weak]
        dialog,
        #[upgrade_or]
        glib::Propagation::Stop,
        move |_, key, _, state| {
            use gtk::gdk::{Key, ModifierType};
            if key == Key::Escape {
                dialog.close();
                on_done(None);
                return glib::Propagation::Stop;
            }
            let modifier_keys = [
                Key::Control_L,
                Key::Control_R,
                Key::Shift_L,
                Key::Shift_R,
                Key::Alt_L,
                Key::Alt_R,
                Key::Super_L,
                Key::Super_R,
                Key::Meta_L,
                Key::Meta_R,
                Key::Hyper_L,
                Key::Hyper_R,
                Key::ISO_Level3_Shift,
                Key::Caps_Lock,
            ];
            if modifier_keys.contains(&key) {
                return glib::Propagation::Stop;
            }
            let mods = state
                & (ModifierType::CONTROL_MASK
                    | ModifierType::ALT_MASK
                    | ModifierType::SHIFT_MASK
                    | ModifierType::SUPER_MASK);
            if mods.is_empty() {
                return glib::Propagation::Stop; // a bare key would hijack typing
            }
            let accel = gtk::accelerator_name(key, mods).to_string();
            let saved = set(&accel);
            dialog.close();
            on_done(saved.then_some(accel));
            glib::Propagation::Stop
        }
    ));
    dialog.add_controller(keys);
    dialog.present();
}
