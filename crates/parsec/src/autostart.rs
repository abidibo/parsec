//! Start-at-login via an XDG autostart entry pointing at this executable.

use crate::brand;
use std::path::PathBuf;

pub fn path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("autostart")
        .join(format!("{}.desktop", brand::APP_ID))
}

pub fn is_enabled() -> bool {
    path().is_file()
}

pub fn set_enabled(on: bool) -> std::io::Result<()> {
    let p = path();
    if on {
        let exe = std::env::current_exe()?;
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(&p, brand::desktop_entry(&exe.to_string_lossy()))?;
        tracing::info!(path = %p.display(), "autostart enabled");
    } else if p.exists() {
        std::fs::remove_file(&p)?;
        tracing::info!("autostart disabled");
    }
    Ok(())
}
