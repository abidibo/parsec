//! Where a browser keeps its profile, and reading history and bookmarks out
//! of it. Two families cover nearly every browser: Chromium (`History`, a
//! SQLite file, plus a JSON `Bookmarks` file) and Firefox (`places.sqlite`
//! for both).
//!
//! A running browser keeps its database locked, so we copy it, with its
//! journal or WAL, into a private folder under `$XDG_RUNTIME_DIR` (memory
//! backed), read the copy and delete it. Nothing from it is written to disk.

use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    Chromium,
    Firefox,
}

/// One way a browser can be installed: its config root, relative to the
/// home folder, and the desktop ids it launches with.
struct Install {
    key: &'static str,
    name: &'static str,
    family: Family,
    root: &'static str,
    desktop_ids: &'static [&'static str],
}

const INSTALLS: &[Install] = &[
    Install {
        key: "brave",
        name: "Brave",
        family: Family::Chromium,
        root: ".config/BraveSoftware/Brave-Browser",
        desktop_ids: &["brave-browser.desktop", "brave_brave.desktop"],
    },
    Install {
        key: "brave",
        name: "Brave",
        family: Family::Chromium,
        root: ".var/app/com.brave.Browser/config/BraveSoftware/Brave-Browser",
        desktop_ids: &["com.brave.Browser.desktop"],
    },
    Install {
        key: "chrome",
        name: "Chrome",
        family: Family::Chromium,
        root: ".config/google-chrome",
        desktop_ids: &["google-chrome.desktop"],
    },
    Install {
        key: "chrome",
        name: "Chrome",
        family: Family::Chromium,
        root: ".var/app/com.google.Chrome/config/google-chrome",
        desktop_ids: &["com.google.Chrome.desktop"],
    },
    Install {
        key: "chromium",
        name: "Chromium",
        family: Family::Chromium,
        root: ".config/chromium",
        desktop_ids: &["chromium.desktop", "chromium-browser.desktop"],
    },
    Install {
        key: "chromium",
        name: "Chromium",
        family: Family::Chromium,
        root: "snap/chromium/common/chromium",
        desktop_ids: &["chromium_chromium.desktop"],
    },
    Install {
        key: "chromium",
        name: "Chromium",
        family: Family::Chromium,
        root: ".var/app/org.chromium.Chromium/config/chromium",
        desktop_ids: &["org.chromium.Chromium.desktop"],
    },
    Install {
        key: "vivaldi",
        name: "Vivaldi",
        family: Family::Chromium,
        root: ".config/vivaldi",
        desktop_ids: &["vivaldi-stable.desktop"],
    },
    Install {
        key: "edge",
        name: "Edge",
        family: Family::Chromium,
        root: ".config/microsoft-edge",
        desktop_ids: &["microsoft-edge.desktop"],
    },
    Install {
        key: "firefox",
        name: "Firefox",
        family: Family::Firefox,
        root: ".mozilla/firefox",
        desktop_ids: &["firefox.desktop", "firefox-esr.desktop"],
    },
    Install {
        key: "firefox",
        name: "Firefox",
        family: Family::Firefox,
        root: ".config/mozilla/firefox",
        desktop_ids: &["firefox.desktop"],
    },
    Install {
        key: "firefox",
        name: "Firefox",
        family: Family::Firefox,
        root: "snap/firefox/common/.mozilla/firefox",
        desktop_ids: &["firefox_firefox.desktop"],
    },
    Install {
        key: "firefox",
        name: "Firefox",
        family: Family::Firefox,
        root: ".var/app/org.mozilla.firefox/.mozilla/firefox",
        desktop_ids: &["org.mozilla.firefox.desktop"],
    },
    Install {
        key: "librewolf",
        name: "LibreWolf",
        family: Family::Firefox,
        root: ".librewolf",
        desktop_ids: &["librewolf.desktop"],
    },
    Install {
        key: "librewolf",
        name: "LibreWolf",
        family: Family::Firefox,
        root: ".var/app/io.gitlab.librewolf-community/.librewolf",
        desktop_ids: &["io.gitlab.librewolf-community.desktop"],
    },
    Install {
        key: "zen",
        name: "Zen",
        family: Family::Firefox,
        root: ".zen",
        desktop_ids: &["zen.desktop"],
    },
    Install {
        key: "zen",
        name: "Zen",
        family: Family::Firefox,
        root: ".var/app/app.zen_browser.zen/.zen",
        desktop_ids: &["app.zen_browser.zen.desktop"],
    },
];

/// Names accepted in `[browser] name`, besides "auto".
pub fn known() -> Vec<&'static str> {
    let mut keys: Vec<&'static str> = INSTALLS.iter().map(|i| i.key).collect();
    keys.dedup();
    keys
}

/// A profile to read, and how to open what's in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub family: Family,
    pub profile: PathBuf,
    /// Desktop ids to open links with, preferred first.
    pub desktop_ids: Vec<String>,
}

impl Source {
    fn db(&self) -> PathBuf {
        match self.family {
            Family::Chromium => self.profile.join("History"),
            Family::Firefox => self.profile.join("places.sqlite"),
        }
    }

    /// Modification times of everything read, to tell when to read again.
    pub fn stamp(&self) -> Vec<Option<SystemTime>> {
        let db = self.db();
        let mut files = vec![db.clone(), sibling(&db, "-wal")];
        if self.family == Family::Chromium {
            files.push(self.profile.join("Bookmarks"));
        }
        files
            .iter()
            .map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok())
            .collect()
    }
}

/// Pick the profile to read. `name` is "auto" or one of [`known`];
/// `default_app` is the desktop id of the default browser, used by "auto".
/// `profile` names a profile (folder or display name; empty = the one in
/// use). `path`, a profile folder, wins over both.
pub fn resolve(name: &str, profile: &str, path: &str, default_app: Option<&str>) -> Option<Source> {
    let home = dirs::home_dir()?;
    let auto = name.is_empty() || name == "auto";
    let wanted: Vec<&Install> = if auto {
        let by_default: Vec<&Install> = INSTALLS
            .iter()
            .filter(|i| default_app.is_some_and(|d| i.desktop_ids.contains(&d)))
            .collect();
        if by_default.is_empty() {
            INSTALLS.iter().collect()
        } else {
            by_default
        }
    } else {
        INSTALLS.iter().filter(|i| i.key == name).collect()
    };

    if !path.is_empty() {
        let dir = crate::config::expand_home(path);
        let family = if dir.join("History").is_file() {
            Family::Chromium
        } else if dir.join("places.sqlite").is_file() {
            Family::Firefox
        } else {
            return None;
        };
        let install = wanted.iter().find(|i| i.family == family);
        return Some(Source {
            name: install.map_or("Browser", |i| i.name).to_string(),
            family,
            profile: dir,
            desktop_ids: desktop_ids(install.copied(), default_app),
        });
    }

    // Several installs can exist (native and Flatpak): read the one used last.
    wanted
        .into_iter()
        .filter_map(|i| {
            let root = home.join(i.root);
            let dir = match i.family {
                Family::Chromium => chromium_profile(&root, profile)?,
                Family::Firefox => firefox_profile(&root, profile)?,
            };
            let src = Source {
                name: i.name.to_string(),
                family: i.family,
                profile: dir,
                desktop_ids: desktop_ids(Some(i), default_app),
            };
            let used = std::fs::metadata(src.db())
                .and_then(|m| m.modified())
                .ok()?;
            Some((used, src))
        })
        .max_by_key(|(used, _)| *used)
        .map(|(_, src)| src)
}

fn desktop_ids(install: Option<&Install>, default_app: Option<&str>) -> Vec<String> {
    let mut ids: Vec<String> = Vec::new();
    if let Some(d) = default_app.filter(|d| install.is_none_or(|i| i.desktop_ids.contains(d))) {
        ids.push(d.to_string());
    }
    for id in install.map_or(&[][..], |i| i.desktop_ids) {
        if !ids.iter().any(|x| x == id) {
            ids.push(id.to_string());
        }
    }
    ids
}

/// `Default`, `Profile 1`... chosen by folder or display name, else the one
/// `Local State` says was used last.
fn chromium_profile(root: &Path, wanted: &str) -> Option<PathBuf> {
    let state: serde_json::Value = std::fs::read(root.join("Local State"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let dir = if wanted.is_empty() {
        state["profile"]["last_used"]
            .as_str()
            .unwrap_or("Default")
            .to_string()
    } else {
        state["profile"]["info_cache"]
            .as_object()
            .and_then(|cache| {
                cache
                    .iter()
                    .find(|(_, v)| v["name"].as_str() == Some(wanted))
                    .map(|(k, _)| k.clone())
            })
            .unwrap_or_else(|| wanted.to_string())
    };
    let dir = root.join(dir);
    dir.join("History").is_file().then_some(dir)
}

/// From `profiles.ini`: the named profile, else the one the newest install
/// uses, else the one marked default.
fn firefox_profile(root: &Path, wanted: &str) -> Option<PathBuf> {
    let ini = std::fs::read_to_string(root.join("profiles.ini")).ok()?;
    let sections = parse_ini(&ini);
    let resolve = |p: &str| {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            root.join(p)
        }
    };
    let ok = |p: PathBuf| p.join("places.sqlite").is_file().then_some(p);
    let profiles = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Profile"));
    if !wanted.is_empty() {
        return profiles
            .clone()
            .find(|(_, kv)| {
                kv.get("Name").map(String::as_str) == Some(wanted)
                    || kv
                        .get("Path")
                        .is_some_and(|p| p == wanted || p.ends_with(&format!("/{wanted}")))
            })
            .and_then(|(_, kv)| ok(resolve(kv.get("Path")?)));
    }
    let by_install = sections
        .iter()
        .filter(|(name, _)| name.starts_with("Install"))
        .filter_map(|(_, kv)| ok(resolve(kv.get("Default")?)))
        .max_by_key(|p| {
            std::fs::metadata(p.join("places.sqlite"))
                .and_then(|m| m.modified())
                .ok()
        });
    by_install
        .or_else(|| {
            profiles
                .clone()
                .find(|(_, kv)| kv.get("Default").map(String::as_str) == Some("1"))
                .and_then(|(_, kv)| ok(resolve(kv.get("Path")?)))
        })
        .or_else(|| {
            profiles
                .clone()
                .find_map(|(_, kv)| ok(resolve(kv.get("Path")?)))
        })
}

fn parse_ini(text: &str) -> Vec<(String, HashMap<String, String>)> {
    let mut out: Vec<(String, HashMap<String, String>)> = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            out.push((name.to_string(), HashMap::new()));
        } else if let (Some((k, v)), Some((_, kv))) = (line.split_once('='), out.last_mut()) {
            kv.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    out
}

// ---------------------------------------------------------------- reading

/// One page, from history, bookmarks or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub url: String,
    pub title: String,
    pub visits: u32,
    /// Unix seconds of the last visit.
    pub last: Option<u64>,
    /// `Some(folder)` for a bookmark; the folder may be empty.
    pub folder: Option<String>,
}

/// Recent history, `limit` pages at most, merged with every bookmark.
/// Blocking: copies and queries the database. Call off the main thread.
pub fn read(src: &Source, limit: usize) -> Result<Vec<Entry>> {
    let snap = Snapshot::take(&src.db())?;
    let conn = Connection::open_with_flags(
        &snap.db,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("opening history copy")?;
    let (history, bookmarks) = match src.family {
        Family::Chromium => (
            chromium_history(&conn, limit)?,
            std::fs::read(src.profile.join("Bookmarks"))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .map(|v| chromium_bookmarks(&v))
                .unwrap_or_default(),
        ),
        Family::Firefox => (firefox_history(&conn, limit)?, firefox_bookmarks(&conn)?),
    };
    Ok(merge(history, bookmarks))
}

/// Microseconds since 1601, Chromium's clock, to Unix seconds.
const CHROMIUM_EPOCH_OFFSET: i64 = 11_644_473_600;

fn chromium_history(conn: &Connection, limit: usize) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count, last_visit_time FROM urls \
         WHERE hidden = 0 AND last_visit_time > 0 \
         ORDER BY last_visit_time DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        let t: i64 = r.get(3)?;
        Ok(Entry {
            url: r.get(0)?,
            title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            visits: r.get::<_, i64>(2)?.max(0) as u32,
            last: u64::try_from(t / 1_000_000 - CHROMIUM_EPOCH_OFFSET).ok(),
            folder: None,
        })
    })?;
    Ok(rows
        .filter_map(|r| r.ok())
        .filter(|e| openable(&e.url))
        .collect())
}

/// Walk the `Bookmarks` JSON: `roots` holds the bar, "other" and mobile
/// folders, each a tree of `folder` and `url` nodes.
fn chromium_bookmarks(json: &serde_json::Value) -> Vec<Entry> {
    fn walk(node: &serde_json::Value, folder: &str, out: &mut Vec<Entry>) {
        match node["type"].as_str() {
            Some("url") => {
                let url = node["url"].as_str().unwrap_or_default();
                if openable(url) {
                    out.push(Entry {
                        url: url.to_string(),
                        title: node["name"].as_str().unwrap_or_default().to_string(),
                        visits: 0,
                        last: None,
                        folder: Some(folder.to_string()),
                    });
                }
            }
            Some("folder") => {
                let name = node["name"].as_str().unwrap_or_default();
                let path = if folder.is_empty() {
                    name.to_string()
                } else {
                    format!("{folder} › {name}")
                };
                for child in node["children"].as_array().into_iter().flatten() {
                    walk(child, &path, out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    if let Some(roots) = json["roots"].as_object() {
        for root in roots.values() {
            walk(root, "", &mut out);
        }
    }
    out
}

fn firefox_history(conn: &Connection, limit: usize) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT url, title, visit_count, last_visit_date FROM moz_places \
         WHERE hidden = 0 AND last_visit_date IS NOT NULL \
         ORDER BY last_visit_date DESC LIMIT ?1",
    )?;
    let rows = stmt.query_map([limit as i64], |r| {
        Ok(Entry {
            url: r.get(0)?,
            title: r.get::<_, Option<String>>(1)?.unwrap_or_default(),
            visits: r.get::<_, i64>(2)?.max(0) as u32,
            last: u64::try_from(r.get::<_, i64>(3)? / 1_000_000).ok(),
            folder: None,
        })
    })?;
    Ok(rows
        .filter_map(|r| r.ok())
        .filter(|e| openable(&e.url))
        .collect())
}

fn firefox_bookmarks(conn: &Connection) -> Result<Vec<Entry>> {
    let mut stmt = conn.prepare(
        "SELECT p.url, COALESCE(b.title, p.title, ''), COALESCE(f.title, ''), f.guid \
         FROM moz_bookmarks b JOIN moz_places p ON p.id = b.fk \
         LEFT JOIN moz_bookmarks f ON f.id = b.parent \
         WHERE b.type = 1",
    )?;
    let rows = stmt.query_map([], |r| {
        let folder: String = r.get(2)?;
        let guid: Option<String> = r.get(3)?;
        // The built-in roots carry internal names; show what Firefox shows.
        let folder = match guid.as_deref() {
            Some("toolbar_____") => "Bookmarks Toolbar".to_string(),
            Some("menu________") => "Bookmarks Menu".to_string(),
            Some("unfiled_____") => "Other Bookmarks".to_string(),
            Some("mobile______") => "Mobile Bookmarks".to_string(),
            _ => folder,
        };
        Ok(Entry {
            url: r.get(0)?,
            title: r.get(1)?,
            visits: 0,
            last: None,
            folder: Some(folder),
        })
    })?;
    Ok(rows
        .filter_map(|r| r.ok())
        .filter(|e| openable(&e.url))
        .collect())
}

/// Pages worth opening from a launcher: no `chrome://`, `place:`,
/// `javascript:` or data URLs.
fn openable(url: &str) -> bool {
    ["https://", "http://", "file://"]
        .iter()
        .any(|p| url.starts_with(p))
}

/// History in recency order, bookmarked pages flagged in place (keeping the
/// bookmark's name, which you chose), then bookmarks never visited.
pub fn merge(history: Vec<Entry>, bookmarks: Vec<Entry>) -> Vec<Entry> {
    let mut marks: HashMap<String, Entry> = HashMap::new();
    let mut order: Vec<String> = Vec::new();
    for b in bookmarks {
        if !marks.contains_key(&b.url) {
            order.push(b.url.clone());
            marks.insert(b.url.clone(), b);
        }
    }
    let mut out: Vec<Entry> = history
        .into_iter()
        .map(|mut e| {
            if let Some(b) = marks.remove(&e.url) {
                if !b.title.is_empty() {
                    e.title = b.title;
                }
                e.folder = b.folder;
            }
            e
        })
        .collect();
    out.extend(order.into_iter().filter_map(|u| marks.remove(&u)));
    out
}

/// A private copy of a database and its journal or WAL, removed on drop.
struct Snapshot {
    dir: PathBuf,
    db: PathBuf,
}

impl Snapshot {
    fn take(src: &Path) -> Result<Self> {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
        let base = dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("parsec");
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&base)
            .context("creating snapshot folder")?;
        let dir = base.join(format!("browser-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(&dir)
            .context("creating snapshot folder")?;
        let snap = Self {
            db: dir.join("db.sqlite"),
            dir,
        };
        std::fs::copy(src, &snap.db).with_context(|| format!("copying {}", src.display()))?;
        let _ = std::fs::set_permissions(&snap.db, std::fs::Permissions::from_mode(0o600));
        // A hot journal is rolled back on open, a WAL replayed: either way
        // the copy reads as a consistent database.
        for suffix in ["-journal", "-wal"] {
            let from = sibling(src, suffix);
            if from.is_file() {
                let _ = std::fs::copy(&from, sibling(&snap.db, suffix));
            }
        }
        Ok(snap)
    }
}

impl Drop for Snapshot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(suffix);
    PathBuf::from(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(url: &str, title: &str, folder: Option<&str>) -> Entry {
        Entry {
            url: url.into(),
            title: title.into(),
            visits: 1,
            last: None,
            folder: folder.map(String::from),
        }
    }

    #[test]
    fn merge_flags_visited_bookmarks_and_appends_the_rest() {
        let history = vec![
            entry("https://a", "A page", None),
            entry("https://b", "B", None),
        ];
        let marks = vec![
            entry("https://b", "My B", Some("Dev")),
            entry("https://c", "C", Some("")),
            entry("https://c", "C again", Some("Other")),
        ];
        let got = merge(history, marks);
        assert_eq!(got.len(), 3);
        assert_eq!(got[1].title, "My B");
        assert_eq!(got[1].folder.as_deref(), Some("Dev"));
        assert_eq!(got[2].url, "https://c");
        assert_eq!(got[2].title, "C");
    }

    #[test]
    fn chromium_bookmark_tree() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{"roots":{"bookmark_bar":{"type":"folder","name":"Bookmarks bar","children":[
                {"type":"url","name":"Parsec","url":"https://github.com/abidibo/parsec"},
                {"type":"folder","name":"Dev","children":[
                    {"type":"url","name":"Docs","url":"https://docs.rs"},
                    {"type":"url","name":"Bookmarklet","url":"javascript:alert(1)"}]}]}}}"#,
        )
        .unwrap();
        let got = chromium_bookmarks(&json);
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].folder.as_deref(), Some("Bookmarks bar"));
        assert_eq!(got[1].folder.as_deref(), Some("Bookmarks bar › Dev"));
    }

    #[test]
    fn firefox_profiles_ini() {
        let dir = std::env::temp_dir().join(format!("parsec-ff-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for p in ["a.default", "b.work"] {
            std::fs::create_dir_all(dir.join(p)).unwrap();
            std::fs::write(dir.join(p).join("places.sqlite"), b"").unwrap();
        }
        std::fs::write(
            dir.join("profiles.ini"),
            "[Profile0]\nName=default\nIsRelative=1\nPath=a.default\nDefault=1\n\n\
             [Profile1]\nName=work\nIsRelative=1\nPath=b.work\n\n\
             [Install4F96D1932A9F858E]\nDefault=b.work\nLocked=1\n",
        )
        .unwrap();
        assert_eq!(firefox_profile(&dir, ""), Some(dir.join("b.work")));
        assert_eq!(
            firefox_profile(&dir, "default"),
            Some(dir.join("a.default"))
        );
        assert_eq!(firefox_profile(&dir, "nope"), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn firefox_history_and_bookmarks_from_sqlite() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_places (id INTEGER PRIMARY KEY, url TEXT, title TEXT, \
                 visit_count INTEGER, last_visit_date INTEGER, hidden INTEGER);
             CREATE TABLE moz_bookmarks (id INTEGER PRIMARY KEY, type INTEGER, fk INTEGER, \
                 parent INTEGER, title TEXT, guid TEXT);
             INSERT INTO moz_places VALUES (1, 'https://a.example', 'A', 3, 2000000000, 0);
             INSERT INTO moz_places VALUES (2, 'https://b.example', 'B', 1, 1000000000, 0);
             INSERT INTO moz_places VALUES (3, 'place:sort=8', NULL, 0, NULL, 0);
             INSERT INTO moz_bookmarks VALUES (10, 2, NULL, 1, 'toolbar', 'toolbar_____');
             INSERT INTO moz_bookmarks VALUES (11, 1, 2, 10, 'Bee', 'x');
             INSERT INTO moz_bookmarks VALUES (12, 1, 3, 10, 'Smart', 'y');",
        )
        .unwrap();
        let got = merge(
            firefox_history(&conn, 10).unwrap(),
            firefox_bookmarks(&conn).unwrap(),
        );
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].url, "https://a.example");
        assert_eq!(got[0].last, Some(2000));
        assert_eq!(got[1].title, "Bee");
        assert_eq!(got[1].folder.as_deref(), Some("Bookmarks Toolbar"));
    }
}
