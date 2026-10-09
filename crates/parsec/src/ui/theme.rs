//! Dark and light palettes, GNOME accent sync, applied as `@define-color`
//! overrides in a dedicated CSS provider. The base stylesheet in `window`
//! carries the dark defaults; this provider wins over it and the user's
//! style.css wins over both.

use crate::config::SharedConfig;
use adw::prelude::*;
use gtk::gio;
use gtk::glib;
use std::cell::RefCell;

thread_local! {
    static PROVIDER: RefCell<Option<gtk::CssProvider>> = const { RefCell::new(None) };
}

pub const DEFAULT_ACCENT: &str = "#8b7cff";

/// libadwaita's accent palette, keyed by the GNOME 47+ setting value.
fn gnome_accent(name: &str) -> Option<&'static str> {
    Some(match name {
        "blue" => "#3584e4",
        "teal" => "#2190a4",
        "green" => "#3a944a",
        "yellow" => "#c88800",
        "orange" => "#ed5b00",
        "red" => "#e62d42",
        "pink" => "#d56199",
        "purple" => "#9141ac",
        "slate" => "#6f8396",
        _ => return None,
    })
}

/// GNOME's accent colour if this desktop has the setting (GNOME 47+).
fn system_accent() -> Option<String> {
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup("org.gnome.desktop.interface", true)?;
    if !schema.has_key("accent-color") {
        return None;
    }
    let settings = gio::Settings::new("org.gnome.desktop.interface");
    gnome_accent(settings.string("accent-color").as_str()).map(String::from)
}

fn is_hex(s: &str) -> bool {
    let s = s.trim();
    (s.len() == 7 || s.len() == 4)
        && s.starts_with('#')
        && s[1..].chars().all(|c| c.is_ascii_hexdigit())
}

fn resolve_accent(setting: &str) -> String {
    let s = setting.trim();
    if is_hex(s) {
        return s.to_string();
    }
    system_accent().unwrap_or_else(|| DEFAULT_ACCENT.to_string())
}

fn palette(dark: bool, accent: &str) -> String {
    if dark {
        format!(
            "@define-color parsec_accent {accent};
@define-color parsec_bg rgba(24, 24, 30, 0.985);
@define-color parsec_border rgba(255, 255, 255, 0.09);
@define-color parsec_fg #f2f2f5;
@define-color parsec_dim rgba(242, 242, 245, 0.50);
@define-color parsec_row_hover rgba(255, 255, 255, 0.045);
@define-color parsec_row_selected rgba(255, 255, 255, 0.085);
@define-color parsec_key_bg rgba(255, 255, 255, 0.10);
@define-color parsec_chip_bg rgba(255, 255, 255, 0.06);
@define-color parsec_shadow rgba(0, 0, 0, 0.55);
@define-color parsec_shadow_soft rgba(0, 0, 0, 0.35);
@define-color parsec_inner_highlight rgba(255, 255, 255, 0.05);
@define-color tile_apps rgba(255, 255, 255, 0.06);
@define-color tile_projects #5b9cff;
@define-color tile_shell #b0b7c3;
@define-color tile_github #c58bff;
@define-color tile_clipboard #4fd1a1;
@define-color tile_keepass #ffb454;
@define-color tile_shortcuts #ff7a9a;
@define-color tile_plugins #6ad4ff;
@define-color tile_files #f0c674;
@define-color tile_ssh #7ee0c8;
@define-color tile_docker #4aa8ff;
@define-color tile_services #ff9f7a;
@define-color tile_parsec {accent};
"
        )
    } else {
        format!(
            "@define-color parsec_accent {accent};
@define-color parsec_bg rgba(250, 250, 252, 0.985);
@define-color parsec_border rgba(0, 0, 0, 0.09);
@define-color parsec_fg #1c1c22;
@define-color parsec_dim rgba(28, 28, 34, 0.55);
@define-color parsec_row_hover rgba(0, 0, 0, 0.04);
@define-color parsec_row_selected rgba(0, 0, 0, 0.07);
@define-color parsec_key_bg rgba(0, 0, 0, 0.08);
@define-color parsec_chip_bg rgba(0, 0, 0, 0.05);
@define-color parsec_shadow rgba(0, 0, 0, 0.28);
@define-color parsec_shadow_soft rgba(0, 0, 0, 0.14);
@define-color parsec_inner_highlight rgba(255, 255, 255, 0.8);
@define-color tile_apps rgba(0, 0, 0, 0.05);
@define-color tile_projects #2f6fdb;
@define-color tile_shell #5b6472;
@define-color tile_github #7c3fbf;
@define-color tile_clipboard #1f9a6c;
@define-color tile_keepass #c77a00;
@define-color tile_shortcuts #d8426e;
@define-color tile_plugins #1c8fc2;
@define-color tile_files #b8860b;
@define-color tile_ssh #1f9a8a;
@define-color tile_docker #1e73d8;
@define-color tile_services #d9663a;
@define-color tile_parsec {accent};
"
        )
    }
}

/// Apply the configured theme and accent. Safe to call repeatedly.
pub fn apply(cfg: &SharedConfig) {
    let (theme, accent) = {
        let c = cfg.borrow();
        (c.appearance.theme.clone(), c.appearance.accent.clone())
    };
    let manager = adw::StyleManager::default();
    let dark = match theme.as_str() {
        "dark" => {
            manager.set_color_scheme(adw::ColorScheme::ForceDark);
            true
        }
        "light" => {
            manager.set_color_scheme(adw::ColorScheme::ForceLight);
            false
        }
        _ => {
            manager.set_color_scheme(adw::ColorScheme::Default);
            manager.is_dark()
        }
    };
    let css = palette(dark, &resolve_accent(&accent));
    let Some(display) = gtk::gdk::Display::default() else {
        return;
    };
    PROVIDER.with(|slot| {
        let mut slot = slot.borrow_mut();
        let provider = slot.get_or_insert_with(|| {
            let p = gtk::CssProvider::new();
            // Above the base stylesheet, below the user's style.css.
            gtk::style_context_add_provider_for_display(
                &display,
                &p,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1,
            );
            p
        });
        provider.load_from_string(&css);
    });
    tracing::debug!(theme, dark, "theme applied");
}

/// Re-apply when the system dark preference or accent changes.
pub fn watch(cfg: SharedConfig) {
    let manager = adw::StyleManager::default();
    manager.connect_dark_notify(glib::clone!(
        #[strong]
        cfg,
        move |_| apply(&cfg)
    ));
    if let Some(source) = gio::SettingsSchemaSource::default() {
        if let Some(schema) = source.lookup("org.gnome.desktop.interface", true) {
            if schema.has_key("accent-color") {
                let settings = gio::Settings::new("org.gnome.desktop.interface");
                settings.connect_changed(
                    Some("accent-color"),
                    glib::clone!(
                        #[strong]
                        cfg,
                        move |_, _| apply(&cfg)
                    ),
                );
                std::mem::forget(settings);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_validation() {
        assert!(is_hex("#8b7cff"));
        assert!(is_hex("#fff"));
        assert!(!is_hex("8b7cff"));
        assert!(!is_hex("#ggg"));
        assert!(!is_hex("system"));
    }

    #[test]
    fn accent_names() {
        assert_eq!(gnome_accent("blue"), Some("#3584e4"));
        assert_eq!(gnome_accent("mauve"), None);
    }
}
