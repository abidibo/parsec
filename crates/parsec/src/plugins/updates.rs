//! Updates for plugins installed from git. Installing records where a
//! plugin came from and which commit it is (`.parsec-origin.json` in its
//! folder). A check asks each remote for its default branch's commit with
//! `git ls-remote`, which downloads nothing; a plugin whose remote moved
//! on has an update. What changed is read from a blobless clone: the
//! commit subjects since the installed one, and the new version.
//!
//! Plugins installed before origins were recorded fall back to their
//! `homepage` with an unknown commit. Their first check compares the
//! installed files with the remote's (by git's own hashes, from a
//! blobless clone) and records the commit when they match; otherwise it
//! offers an update, and updating records it.

use super::{Installed, MANIFEST};
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const ORIGIN: &str = ".parsec-origin.json";
/// Long enough for a slow remote, short enough that a dead one doesn't
/// hold a check up.
const REMOTE_TIMEOUT: Duration = Duration::from_secs(20);
const CLONE_TIMEOUT: Duration = Duration::from_secs(60);
/// Commits listed when the installed one is unknown.
const RECENT: usize = 10;
const MAX_COMMITS: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub url: String,
    /// `None` when installed before Parsec recorded it.
    #[serde(default)]
    pub commit: Option<String>,
}

/// A plugin whose remote has moved past the installed commit.
#[derive(Debug, Clone)]
pub struct Available {
    pub id: String,
    pub name: String,
    pub version: String,
    pub origin: Origin,
    /// The remote's commit when last checked.
    pub head: String,
}

/// What an update brings, read before installing it.
#[derive(Debug, Clone)]
pub struct Changes {
    /// The version in the new plugin.toml, when it sits at the root.
    pub version: Option<String>,
    /// `(short hash, subject)`, newest first.
    pub commits: Vec<(String, String)>,
    /// False when the installed commit is unknown or no longer in the
    /// history: `commits` are then just the latest few.
    pub complete: bool,
}

pub fn record(plugin_dir: &Path, origin: &Origin) -> Result<()> {
    let json = serde_json::to_vec_pretty(origin)?;
    std::fs::write(plugin_dir.join(ORIGIN), json).context("recording the plugin's origin")
}

/// Where `p` updates from, if anywhere.
pub fn origin(p: &Installed) -> Option<Origin> {
    if let Some(o) = std::fs::read(p.dir.join(ORIGIN))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        return Some(o);
    }
    let home = p.manifest.homepage.trim();
    home.starts_with("https://").then(|| Origin {
        url: home.to_string(),
        commit: None,
    })
}

/// The default branch's commit at `url`.
pub fn remote_head(url: &str) -> Result<String> {
    let out = git(None, &["ls-remote", url, "HEAD"], REMOTE_TIMEOUT)?;
    out.split_whitespace()
        .next()
        .filter(|h| h.len() >= 40 && h.chars().all(|c| c.is_ascii_hexdigit()))
        .map(String::from)
        .with_context(|| format!("{url} has no default branch"))
}

/// Each origin's remote commit, by URL, asked side by side. Remotes that
/// can't be reached (offline, gone, private) are logged and left out.
pub fn remote_heads(plugins: &[Installed]) -> BTreeMap<String, String> {
    let mut urls: Vec<String> = plugins.iter().filter_map(origin).map(|o| o.url).collect();
    urls.sort();
    urls.dedup();
    std::thread::scope(|s| {
        let asks: Vec<_> = urls
            .iter()
            .map(|url| s.spawn(move || (url, remote_head(url))))
            .collect();
        asks.into_iter()
            .filter_map(|a| a.join().ok())
            .filter_map(|(url, head)| match head {
                Ok(h) => Some((url.clone(), h)),
                Err(e) => {
                    tracing::warn!(url, "plugin update check failed: {e:#}");
                    None
                }
            })
            .collect()
    })
}

/// For plugins whose installed commit is unknown: when their files are
/// exactly the remote's, record its commit, so they aren't offered an
/// update they already have.
pub fn recognize(plugins: &[Installed], heads: &BTreeMap<String, String>) {
    std::thread::scope(|s| {
        for p in plugins {
            let Some(origin) = origin(p).filter(|o| o.commit.is_none()) else {
                continue;
            };
            let Some(head) = heads.get(&origin.url) else {
                continue;
            };
            s.spawn(move || match same_files(p, &origin.url, head) {
                Ok(true) => {
                    let origin = Origin {
                        commit: Some(head.clone()),
                        ..origin
                    };
                    match record(&p.dir, &origin) {
                        Ok(()) => tracing::info!(id = %p.manifest.id, "plugin matches its remote"),
                        Err(e) => tracing::warn!(id = %p.manifest.id, "{e:#}"),
                    }
                }
                Ok(false) => {}
                Err(e) => tracing::debug!(id = %p.manifest.id, "can't compare with remote: {e:#}"),
            });
        }
    });
}

/// Every file of the remote's `head` is in `p`'s folder, unchanged. Extra
/// local files (caches, a virtualenv) don't count.
fn same_files(p: &Installed, url: &str, head: &str) -> Result<bool> {
    let tmp = Scratch::new()?;
    let repo = tmp.0.to_string_lossy().into_owned();
    git(
        None,
        &[
            "clone",
            "--bare",
            "--quiet",
            "--depth=1",
            "--filter=blob:none",
            url,
            &repo,
        ],
        CLONE_TIMEOUT,
    )?;
    let tree = git(Some(&tmp.0), &["ls-tree", "-r", head], CLONE_TIMEOUT)?;
    let mut remote = Vec::new();
    for line in tree.lines() {
        // "<mode> <type> <hash>\t<path>"
        let Some((meta, path)) = line.split_once('\t') else {
            continue;
        };
        match meta.split_whitespace().collect::<Vec<_>>()[..] {
            [_, "blob", hash] => remote.push((path, hash)),
            _ => return Ok(false),
        }
    }
    if remote.is_empty() || remote.iter().any(|(path, _)| !p.dir.join(path).is_file()) {
        return Ok(false);
    }
    let files: Vec<String> = remote
        .iter()
        .map(|(path, _)| p.dir.join(path).to_string_lossy().into_owned())
        .collect();
    let mut args = vec!["hash-object", "--"];
    args.extend(files.iter().map(String::as_str));
    let local = git(None, &args, CLONE_TIMEOUT)?;
    Ok(local.lines().eq(remote.iter().map(|(_, hash)| *hash)))
}

/// The plugins whose remote commit, from `heads`, isn't the installed one.
pub fn available(plugins: &[Installed], heads: &BTreeMap<String, String>) -> Vec<Available> {
    plugins
        .iter()
        .filter_map(|p| {
            let origin = origin(p)?;
            let head = heads.get(&origin.url)?;
            (origin.commit.as_ref() != Some(head)).then(|| Available {
                id: p.manifest.id.clone(),
                name: p.manifest.name.clone(),
                version: p.manifest.version.clone(),
                origin,
                head: head.clone(),
            })
        })
        .collect()
}

/// Commits and version the remote has beyond `origin.commit`.
pub fn changes(origin: &Origin) -> Result<Changes> {
    let tmp = Scratch::new()?;
    let repo = tmp.0.to_string_lossy().into_owned();
    git(
        None,
        &[
            "clone",
            "--bare",
            "--quiet",
            "--filter=blob:none",
            &origin.url,
            &repo,
        ],
        CLONE_TIMEOUT,
    )?;
    let at = |args: &[&str]| git(Some(&tmp.0), args, CLONE_TIMEOUT);
    let known = origin
        .commit
        .as_deref()
        .filter(|c| at(&["cat-file", "-e", &format!("{c}^{{commit}}")]).is_ok());
    let (range, limit) = match known {
        Some(c) => (format!("{c}..HEAD"), MAX_COMMITS),
        None => ("HEAD".to_string(), RECENT),
    };
    let log = at(&["log", "--format=%h%x09%s", &format!("-n{limit}"), &range])?;
    let commits = log
        .lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(h, s)| (h.to_string(), s.to_string()))
        .collect();
    let version = at(&["show", &format!("HEAD:{MANIFEST}")])
        .ok()
        .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
        .and_then(|t| t.get("version")?.as_str().map(String::from));
    Ok(Changes {
        version,
        commits,
        complete: known.is_some(),
    })
}

/// The commit checked out in `repo`.
pub(super) fn head_of(repo: &Path) -> Option<String> {
    git(Some(repo), &["rev-parse", "HEAD"], REMOTE_TIMEOUT)
        .ok()
        .map(|s| s.trim().to_string())
}

/// Run git without ever prompting, killed after `timeout`. Returns stdout.
/// Every caller reads a few lines at most, so the pipes never fill up.
fn git(dir: Option<&Path>, args: &[&str], timeout: Duration) -> Result<String> {
    let mut cmd = Command::new("git");
    if let Some(dir) = dir {
        cmd.arg("-C").arg(dir);
    }
    let mut child = cmd
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git (is it installed?)")?;
    let deadline = Instant::now() + timeout;
    while child.try_wait()?.is_none() {
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            bail!("git {} timed out", args[0]);
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let last = err
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("");
        bail!("git {} failed: {}", args[0], last.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A temporary folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Result<Self> {
        use std::sync::atomic::{AtomicU32, Ordering};
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "parsec-plugin-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        Ok(Self(path))
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::Manifest;

    /// A repository with a plugin.toml and one commit per subject.
    fn repo(subjects: &[&str]) -> (Scratch, String) {
        let dir = Scratch::new().unwrap();
        std::fs::create_dir_all(&dir.0).unwrap();
        let run = |args: &[&str]| git(Some(&dir.0), args, REMOTE_TIMEOUT).unwrap();
        run(&["init", "--quiet", "--initial-branch=main"]);
        run(&["config", "user.email", "t@t"]);
        run(&["config", "user.name", "t"]);
        for (i, s) in subjects.iter().enumerate() {
            std::fs::write(
                dir.0.join(MANIFEST),
                format!("name = \"T\"\nversion = \"0.{i}.0\"\nexec = \"main.py\"\n"),
            )
            .unwrap();
            run(&["add", "."]);
            run(&["commit", "--quiet", "-m", s]);
        }
        let url = format!("file://{}", dir.0.display());
        (dir, url)
    }

    fn plugin(dir: &Path, homepage: &str) -> Installed {
        Installed {
            manifest: Manifest {
                id: "t".into(),
                name: "T".into(),
                version: "0.1.0".into(),
                homepage: homepage.into(),
                ..Manifest::default()
            },
            dir: dir.to_path_buf(),
        }
    }

    #[test]
    fn origin_is_recorded_or_taken_from_the_homepage() {
        let dir = Scratch::new().unwrap();
        std::fs::create_dir_all(&dir.0).unwrap();
        let p = plugin(&dir.0, "https://github.com/x/y");
        assert_eq!(
            origin(&p),
            Some(Origin {
                url: "https://github.com/x/y".into(),
                commit: None
            })
        );
        let o = Origin {
            url: "git@host:x/y.git".into(),
            commit: Some("abc".into()),
        };
        record(&dir.0, &o).unwrap();
        assert_eq!(origin(&p), Some(o));
        assert_eq!(origin(&plugin(Path::new("/nope"), "")), None);
    }

    #[test]
    fn updates_are_new_commits_and_list_what_changed() {
        let (remote, url) = repo(&["feat: first", "fix: second", "feat: third"]);
        let head = remote_head(&url).unwrap();
        let first = git(Some(&remote.0), &["rev-parse", "HEAD~2"], REMOTE_TIMEOUT)
            .unwrap()
            .trim()
            .to_string();

        let installed = Scratch::new().unwrap();
        std::fs::create_dir_all(&installed.0).unwrap();
        let p = plugin(&installed.0, "");
        let mut o = Origin {
            url: url.clone(),
            commit: Some(head.clone()),
        };
        record(&installed.0, &o).unwrap();
        let heads = remote_heads(std::slice::from_ref(&p));
        assert!(available(std::slice::from_ref(&p), &heads).is_empty());

        o.commit = Some(first);
        record(&installed.0, &o).unwrap();
        let [a] = available(std::slice::from_ref(&p), &heads)
            .try_into()
            .unwrap();
        assert_eq!(a.head, head);

        let c = changes(&a.origin).unwrap();
        assert!(c.complete);
        assert_eq!(c.version.as_deref(), Some("0.2.0"));
        let subjects: Vec<&str> = c.commits.iter().map(|(_, s)| s.as_str()).collect();
        assert_eq!(subjects, ["feat: third", "fix: second"]);

        o.commit = None;
        let c = changes(&o).unwrap();
        assert!(!c.complete);
        assert_eq!(c.commits.len(), 3);
    }

    #[test]
    fn untracked_plugins_matching_the_remote_are_recognized() {
        let (remote, url) = repo(&["feat: first", "fix: second"]);
        let heads = BTreeMap::from([(url.clone(), remote_head(&url).unwrap())]);
        let installed = Scratch::new().unwrap();
        std::fs::create_dir_all(installed.0.join("__pycache__")).unwrap();
        let untracked = Origin {
            url: url.clone(),
            commit: None,
        };
        let p = plugin(&installed.0, "");

        // Behind: the first commit's plugin.toml.
        std::fs::write(
            installed.0.join(MANIFEST),
            "name = \"T\"\nversion = \"0.0.0\"\nexec = \"main.py\"\n",
        )
        .unwrap();
        record(&installed.0, &untracked).unwrap();
        recognize(std::slice::from_ref(&p), &heads);
        assert_eq!(origin(&p).unwrap().commit, None);
        assert_eq!(available(std::slice::from_ref(&p), &heads).len(), 1);

        // Current, with a cache folder of its own.
        std::fs::copy(remote.0.join(MANIFEST), installed.0.join(MANIFEST)).unwrap();
        recognize(std::slice::from_ref(&p), &heads);
        assert_eq!(origin(&p).unwrap().commit.as_ref(), heads.get(&url));
        assert!(available(std::slice::from_ref(&p), &heads).is_empty());
    }

    #[test]
    fn unreachable_remotes_are_skipped() {
        let dir = Scratch::new().unwrap();
        std::fs::create_dir_all(&dir.0).unwrap();
        let o = Origin {
            url: "file:///nowhere/at/all".into(),
            commit: None,
        };
        record(&dir.0, &o).unwrap();
        assert!(remote_heads(&[plugin(&dir.0, "")]).is_empty());
    }
}
