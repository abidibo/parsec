//! `kp <name>`: entries from a KeePass database (.kdbx), read with the pure
//! Rust `keepass` crate. Unlocked with the master password typed in the
//! launcher, kept decrypted in memory only, locked again after a while.
//! Passwords copy as secrets: excluded from the clipboard history and
//! cleared from the clipboard after a delay. Nothing is logged or written.

use super::SharedConfig;
use crate::config;
use crate::core::{Action, ActionKind, Icon, Item, Prompt, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use gtk::prelude::*;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Instant;

#[derive(Clone)]
struct Secret {
    title: String,
    username: String,
    password: String,
    url: String,
    group: String,
}

enum State {
    Locked,
    Unlocked {
        entries: Vec<Secret>,
        since: Instant,
    },
}

pub struct KeepassProvider {
    cfg: SharedConfig,
    state: Rc<RefCell<State>>,
}

impl KeepassProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            state: Rc::new(RefCell::new(State::Locked)),
        }
    }

    fn database(&self) -> Option<PathBuf> {
        let p = self.cfg.borrow().keepass.database.clone();
        (!p.trim().is_empty()).then(|| config::expand_home(&p))
    }

    /// Drop the entries once the lock timeout has passed.
    fn enforce_lock(&self) {
        let lock_after = self.cfg.borrow().keepass.lock_after_secs;
        let expired = matches!(
            &*self.state.borrow(),
            State::Unlocked { since, .. } if since.elapsed().as_secs() > lock_after
        );
        if expired {
            tracing::info!("keepass: auto-locked");
            *self.state.borrow_mut() = State::Locked;
        }
    }

    fn unlock_item(&self, db: &Path, verb: &str) -> Item {
        let name = db
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| "database".into());
        let state = self.state.clone();
        let cfg = self.cfg.clone();
        let db_path = db.to_path_buf();
        let prompt = Prompt {
            title: format!("Master password for {name}"),
            secret: true,
            restore: format!("{verb} "),
            submit: Rc::new(move |password: String| {
                let state = state.clone();
                let db = db_path.clone();
                let (key_file, skip) = {
                    let c = cfg.borrow();
                    let k = c.keepass.key_file.clone();
                    (
                        (!k.trim().is_empty()).then(|| config::expand_home(&k)),
                        c.keepass.skip_groups.clone(),
                    )
                };
                Box::pin(async move {
                    let entries = gio::spawn_blocking(move || {
                        open(&db, &password, key_file.as_deref(), &skip)
                    })
                    .await
                    .map_err(|_| "unlock thread failed".to_string())??;
                    tracing::info!(count = entries.len(), "keepass: unlocked");
                    *state.borrow_mut() = State::Unlocked {
                        entries,
                        since: Instant::now(),
                    };
                    Ok(())
                })
            }),
        };
        Item {
            id: "keepass:unlock".into(),
            title: format!("Unlock {name}"),
            subtitle: Some(config::abbreviate_home(db)),
            icon: Icon::Named("channel-secure-symbolic".into()),
            score: 1000,
            actions: vec![Action {
                label: "Unlock".into(),
                kind: ActionKind::Prompt(prompt),
            }],
        }
    }

    fn lock_item(&self) -> Item {
        let state = self.state.clone();
        Item {
            id: "keepass:lock".into(),
            title: "Lock database".into(),
            subtitle: Some("Forget the decrypted entries now".into()),
            icon: Icon::Named("channel-secure-symbolic".into()),
            score: 1,
            actions: vec![Action {
                label: "Lock".into(),
                kind: ActionKind::Callback(Rc::new(move || {
                    *state.borrow_mut() = State::Locked;
                    tracing::info!("keepass: locked");
                    Ok(())
                })),
            }],
        }
    }

    fn entry_item(&self, e: &Secret, score: u32) -> Item {
        let clear = self.cfg.borrow().keepass.clipboard_clear_secs;
        let mut actions = vec![Action {
            label: "Copy password".into(),
            kind: ActionKind::CopySecret {
                text: e.password.clone(),
                clear_after_secs: clear,
            },
        }];
        if !e.username.is_empty() {
            actions.push(Action {
                label: "Copy username".into(),
                kind: ActionKind::CopyText(e.username.clone()),
            });
        }
        if !e.url.is_empty() {
            let url = if e.url.contains("://") {
                e.url.clone()
            } else {
                format!("https://{}", e.url)
            };
            actions.push(Action {
                label: "Open URL".into(),
                kind: ActionKind::OpenUri(url),
            });
        }
        let mut subtitle = Vec::new();
        if !e.username.is_empty() {
            subtitle.push(e.username.clone());
        }
        if !e.group.is_empty() {
            subtitle.push(e.group.clone());
        }
        Item {
            // Hashed: the frecency file must not list account names.
            id: format!(
                "keepass:{:016x}",
                crate::core::secrets::hash(&format!("{}\n{}", e.title, e.username))
            ),
            title: e.title.clone(),
            subtitle: (!subtitle.is_empty()).then(|| subtitle.join(" · ")),
            icon: Icon::Named("dialog-password-symbolic".into()),
            score,
            actions,
        }
    }
}

/// Decrypt the database and extract what the launcher needs. Runs on a
/// worker thread: key derivation can take a second.
fn open(
    db: &Path,
    password: &str,
    key_file: Option<&Path>,
    skip_groups: &[String],
) -> Result<Vec<Secret>, String> {
    use keepass::{Database, DatabaseKey};
    let mut file = std::fs::File::open(db).map_err(|e| format!("Cannot open database: {e}"))?;
    let mut key = DatabaseKey::new().with_password(password);
    if let Some(kf) = key_file {
        let mut f = std::fs::File::open(kf).map_err(|e| format!("Cannot open key file: {e}"))?;
        key = key
            .with_keyfile(&mut f)
            .map_err(|e| format!("Bad key file: {e}"))?;
    }
    let database = Database::open(&mut file, key).map_err(|e| {
        use keepass::error::DatabaseOpenError as E;
        // KDBX 4 reports a bad key as such; KDBX 3 only notices when the
        // decrypted stream fails padding or block hash checks.
        let msg = e.to_string().to_lowercase();
        let wrong_key = matches!(e, E::Key(_))
            || msg.contains("padding")
            || msg.contains("mismatch")
            || msg.contains("incorrect key");
        if wrong_key {
            "Wrong password".to_string()
        } else {
            format!("Cannot read database: {e}")
        }
    })?;
    let bin = database.meta.recyclebin_uuid;
    let skip: Vec<String> = skip_groups
        .iter()
        .map(|g| g.trim().to_lowercase())
        .collect();
    let mut out: Vec<Secret> = database
        .iter_all_entries()
        .filter(|e| {
            // Skip the recycle bin (by header id or by name) and anything
            // inside it. Walk by id: a GroupRef borrows its parent ref, so
            // refs can't be chained.
            let mut gid = Some(e.parent().id());
            while let Some(id) = gid {
                if bin.is_some_and(|b| id.uuid() == b) {
                    return false;
                }
                let Some(group) = database.group(id) else {
                    break;
                };
                if skip.contains(&group.name.trim().to_lowercase()) {
                    return false;
                }
                gid = group.parent().map(|p| p.id());
            }
            true
        })
        .map(|e| Secret {
            title: e.get_title().unwrap_or("").to_string(),
            username: e.get_username().unwrap_or("").to_string(),
            password: e.get_password().unwrap_or("").to_string(),
            url: e.get_url().unwrap_or("").to_string(),
            group: e.parent().name.clone(),
        })
        .filter(|s| !s.title.is_empty() || !s.username.is_empty())
        .collect();
    out.sort_by(|a, b| a.title.to_lowercase().cmp(&b.title.to_lowercase()));
    Ok(out)
}

#[async_trait(?Send)]
impl Provider for KeepassProvider {
    fn id(&self) -> &'static str {
        "keepass"
    }

    fn prefix(&self) -> Option<String> {
        Some(self.cfg.borrow().verbs.keepass.clone())
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let verb = self.cfg.borrow().verbs.keepass.clone();
        let Some(db) = self.database() else {
            return vec![Item {
                id: "keepass:configure".into(),
                title: "No KeePass database configured".into(),
                subtitle: Some("Set the .kdbx path in Settings › Providers".into()),
                icon: Icon::Named("dialog-warning-symbolic".into()),
                score: 1,
                actions: vec![Action {
                    label: "Open Settings".into(),
                    kind: ActionKind::Callback(Rc::new(|| {
                        if let Some(app) = gio::Application::default() {
                            app.activate_action("preferences", None);
                        }
                        Ok(())
                    })),
                }],
            }];
        };
        if !db.is_file() {
            return vec![Item {
                id: "keepass:missing".into(),
                title: "KeePass database not found".into(),
                subtitle: Some(config::abbreviate_home(&db)),
                icon: Icon::Named("dialog-warning-symbolic".into()),
                score: 1,
                actions: vec![Action {
                    label: "Open Settings".into(),
                    kind: ActionKind::Callback(Rc::new(|| {
                        if let Some(app) = gio::Application::default() {
                            app.activate_action("preferences", None);
                        }
                        Ok(())
                    })),
                }],
            }];
        }
        self.enforce_lock();
        let state = self.state.borrow();
        match &*state {
            State::Locked => vec![self.unlock_item(&db, &verb)],
            State::Unlocked { entries, .. } => {
                let mut items: Vec<Item> = entries
                    .iter()
                    .filter_map(|e| {
                        let score = if q.is_empty() {
                            1
                        } else {
                            q.score_any([e.title.as_str(), e.username.as_str(), e.url.as_str()])?
                        };
                        Some(self.entry_item(e, score))
                    })
                    .collect();
                if q.is_empty() || q.score("lock").is_some() {
                    items.push(self.lock_item());
                }
                items
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Created with keepassxc-cli, master password "parsec-test", two entries.
    fn fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/test.kdbx")
    }

    #[test]
    fn opens_with_the_right_password() {
        // `Secret` has no Debug on purpose (never print passwords), so no expect().
        let entries = match open(&fixture(), "parsec-test", None, &[]) {
            Ok(e) => e,
            Err(msg) => panic!("unlock failed: {msg}"),
        };
        let titles: Vec<&str> = entries.iter().map(|e| e.title.as_str()).collect();
        assert_eq!(titles, ["Example Site", "Other"]);
        let site = &entries[0];
        assert_eq!(site.username, "alice");
        assert_eq!(site.password, "s3cret");
        assert_eq!(site.url, "https://example.com");
    }

    #[test]
    fn wrong_password_is_reported_plainly() {
        let err = open(&fixture(), "nope", None, &[])
            .err()
            .unwrap_or_default();
        assert_eq!(err, "Wrong password");
    }

    #[test]
    fn skip_groups_hides_by_name() {
        // Both fixture entries live in the root group; learn its name, then
        // hide it (case-insensitively) and expect nothing back.
        let all = open(&fixture(), "parsec-test", None, &[]).unwrap_or_else(|m| panic!("{m}"));
        let group = all[0].group.to_uppercase();
        let entries =
            open(&fixture(), "parsec-test", None, &[group]).unwrap_or_else(|m| panic!("{m}"));
        assert!(entries.is_empty());
    }

    #[test]
    fn missing_file_is_reported() {
        let err = open(Path::new("/nonexistent.kdbx"), "x", None, &[])
            .err()
            .unwrap_or_default();
        assert!(err.starts_with("Cannot open database"));
    }
}
