//! Name, logo and version in one place.

use gtk::glib;

pub const NAME: &str = "Parsec";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
pub const TAGLINE: &str = "A personal launcher for GNOME";
pub const APP_ID: &str = "org.abidibo.Parsec";
pub const LOGO_SVG: &[u8] = include_bytes!("../data/logo.svg");

/// The logo as an image widget at the given size.
pub fn logo(pixel_size: i32) -> gtk::Image {
    let image = gtk::Image::builder().pixel_size(pixel_size).build();
    match gtk::gdk::Texture::from_bytes(&glib::Bytes::from_static(LOGO_SVG)) {
        Ok(texture) => image.set_paintable(Some(&texture)),
        Err(e) => {
            tracing::warn!("logo failed to load: {e}");
            image.set_icon_name(Some("system-search-symbolic"));
        }
    }
    image
}

/// Desktop entry used for autostart. `exec` is the binary to run.
pub fn desktop_entry(exec: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={NAME}\nComment={TAGLINE}\n\
         Exec={exec} --background\nIcon={APP_ID}\nTerminal=false\nCategories=Utility;\n\
         NoDisplay=true\nX-GNOME-Autostart-enabled=true\n"
    )
}
