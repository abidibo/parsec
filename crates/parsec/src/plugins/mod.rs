//! Plugin packages on disk: manifest, discovery, install and removal.
//! Pure filesystem code, shared by the daemon, the settings window and the
//! `parsec plugin` subcommands. Running plugins is `providers::plugin_host`.
//!
//! A plugin is a directory under `~/.local/share/parsec/plugins/<id>/` with
//! a `plugin.toml` and an executable. See README › Plugins for the protocol.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub name: String,
    /// Directory name by default. Letters, digits, `-` and `_`.
    pub id: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub homepage: String,
    /// Trigger words. Empty = the plugin sees every query.
    pub keywords: Vec<String>,
    /// Executable, relative to the plugin directory.
    pub exec: String,
    /// Icon: theme name or image file relative to the plugin directory.
    pub icon: String,
    /// Free-form settings handed to the plugin in the `init` message.
    pub config: BTreeMap<String, toml::Value>,
}

impl Default for Manifest {
    fn default() -> Self {
        Self {
            name: String::new(),
            id: String::new(),
            version: "0.0.0".into(),
            description: String::new(),
            author: String::new(),
            homepage: String::new(),
            keywords: Vec::new(),
            exec: "main".into(),
            icon: String::new(),
            config: BTreeMap::new(),
        }
    }
}

/// An installed plugin: its manifest and where it lives.
#[derive(Debug, Clone)]
pub struct Installed {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Installed {
    pub fn exec_path(&self) -> PathBuf {
        self.dir.join(&self.manifest.exec)
    }

    pub fn icon(&self) -> crate::core::Icon {
        let icon = self.manifest.icon.trim();
        if icon.is_empty() {
            return crate::core::Icon::Named("application-x-addon-symbolic".into());
        }
        let path = self.dir.join(icon);
        if path.is_file() {
            crate::core::Icon::Path(path)
        } else {
            crate::core::Icon::Named(icon.to_string())
        }
    }
}

pub const MANIFEST: &str = "plugin.toml";

pub fn dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("parsec")
        .join("plugins")
}

/// Every plugin directory with a readable manifest, sorted by id.
pub fn installed() -> Vec<Installed> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir()) else {
        return out;
    };
    for entry in rd.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match read_manifest(&path) {
            Ok(manifest) => out.push(Installed {
                manifest,
                dir: path,
            }),
            Err(e) => tracing::warn!(dir = %path.display(), "skipping plugin: {e:#}"),
        }
    }
    out.sort_by(|a, b| a.manifest.id.cmp(&b.manifest.id));
    out
}

pub fn read_manifest(plugin_dir: &Path) -> Result<Manifest> {
    let text = std::fs::read_to_string(plugin_dir.join(MANIFEST))
        .with_context(|| format!("no {MANIFEST} in {}", plugin_dir.display()))?;
    let mut m: Manifest = toml::from_str(&text).context("invalid plugin.toml")?;
    if m.id.trim().is_empty() {
        m.id = plugin_dir
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default();
    }
    m.id = sanitize_id(&m.id);
    if m.id.is_empty() {
        bail!("plugin has no usable id");
    }
    if m.name.trim().is_empty() {
        m.name = m.id.clone();
    }
    if m.exec.trim().is_empty() {
        bail!("plugin.toml has no exec");
    }
    if !plugin_dir.join(&m.exec).is_file() {
        bail!("executable {} not found", m.exec);
    }
    Ok(m)
}

fn sanitize_id(raw: &str) -> String {
    raw.trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

/// Install from a `.zip`, a directory, or a git URL. Returns the manifest.
/// An existing plugin with the same id is replaced.
pub fn install(source: &str) -> Result<Installed> {
    let source = source.trim();
    let path = crate::config::expand_home(source);
    if path.is_dir() {
        return install_dir(&path);
    }
    if path.is_file() {
        return install_zip(&path);
    }
    if looks_like_git(source) {
        return install_git(source);
    }
    bail!("{source}: not a zip file, a directory, or a git URL")
}

fn looks_like_git(s: &str) -> bool {
    s.starts_with("https://")
        || s.starts_with("http://")
        || s.starts_with("git@")
        || s.starts_with("ssh://")
        || s.ends_with(".git")
}

fn install_zip(zip_path: &Path) -> Result<Installed> {
    let file = std::fs::File::open(zip_path).context("opening zip")?;
    let mut archive = zip::ZipArchive::new(file).context("reading zip")?;
    let staging = staging_dir()?;
    archive.extract(&staging).context("extracting zip")?;
    finish_install(&staging)
}

fn install_dir(src: &Path) -> Result<Installed> {
    let staging = staging_dir()?;
    copy_tree(src, &staging)?;
    finish_install(&staging)
}

fn install_git(url: &str) -> Result<Installed> {
    let staging = staging_dir()?;
    let status = std::process::Command::new("git")
        .args(["clone", "--depth", "1", "--quiet", url])
        .arg(&staging)
        .status()
        .context("running git (is it installed?)")?;
    if !status.success() {
        let _ = std::fs::remove_dir_all(&staging);
        bail!("git clone failed for {url}");
    }
    let _ = std::fs::remove_dir_all(staging.join(".git"));
    finish_install(&staging)
}

/// Locate the manifest in the staged tree (root, or a single subfolder as
/// zips and git checkouts usually have), validate, move into place.
fn finish_install(staging: &Path) -> Result<Installed> {
    let root =
        find_root(staging).ok_or_else(|| anyhow::anyhow!("no {MANIFEST} found in the package"))?;
    let manifest = match read_manifest(&root) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_dir_all(staging);
            return Err(e);
        }
    };
    make_executable(&root.join(&manifest.exec))?;
    let target = dir().join(&manifest.id);
    if target.exists() {
        std::fs::remove_dir_all(&target).context("replacing previous version")?;
    }
    std::fs::rename(&root, &target).or_else(|_| {
        copy_tree(&root, &target)
            .and_then(|_| std::fs::remove_dir_all(&root).context("cleaning staging"))
    })?;
    let _ = std::fs::remove_dir_all(staging);
    tracing::info!(id = %manifest.id, version = %manifest.version, "plugin installed");
    Ok(Installed {
        manifest,
        dir: target,
    })
}

fn find_root(staging: &Path) -> Option<PathBuf> {
    if staging.join(MANIFEST).is_file() {
        return Some(staging.to_path_buf());
    }
    let subdirs: Vec<PathBuf> = std::fs::read_dir(staging)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && !p.file_name().is_some_and(|n| n == "__MACOSX"))
        .collect();
    subdirs.into_iter().find(|d| d.join(MANIFEST).is_file())
}

fn staging_dir() -> Result<PathBuf> {
    let base = dir();
    std::fs::create_dir_all(&base).context("creating plugins directory")?;
    let staging = base.join(format!(".staging-{}", std::process::id()));
    if staging.exists() {
        std::fs::remove_dir_all(&staging)?;
    }
    std::fs::create_dir_all(&staging)?;
    Ok(staging)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)?.flatten() {
        let from = entry.path();
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            if entry.file_name() == ".git" {
                continue;
            }
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).with_context(|| format!("copying {}", from.display()))?;
        }
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(perms.mode() | 0o111);
    std::fs::set_permissions(path, perms)?;
    Ok(())
}

pub fn remove(id: &str) -> Result<()> {
    let id = sanitize_id(id);
    let target = dir().join(&id);
    if !target.join(MANIFEST).is_file() {
        bail!("no plugin named {id}");
    }
    std::fs::remove_dir_all(&target).context("removing plugin")?;
    tracing::info!(id = %id, "plugin removed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_sanitized() {
        assert_eq!(sanitize_id(" My Plugin! "), "my-plugin");
        assert_eq!(sanitize_id("calc_v2"), "calc_v2");
        assert_eq!(sanitize_id("---"), "");
    }

    #[test]
    fn git_detection() {
        assert!(looks_like_git("https://github.com/x/y"));
        assert!(looks_like_git("git@github.com:x/y.git"));
        assert!(!looks_like_git("~/Downloads/plugin.zip"));
    }

    #[test]
    fn manifest_from_example_plugin() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/plugins/calc");
        let m = read_manifest(&dir).unwrap();
        assert_eq!(m.id, "calc");
        assert_eq!(m.keywords, ["="]);
        assert_eq!(m.exec, "main.py");
    }
}
