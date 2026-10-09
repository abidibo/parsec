//! Runs installed plugins and turns their answers into items.
//!
//! One long-lived process per plugin, JSON lines on stdin/stdout. Every
//! message Parsec sends gets exactly one reply line. A plugin that fails
//! to answer within the timeout is killed and restarted on the next query;
//! after a few failures in a row it is left alone until Parsec restarts.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use crate::plugins::{self, Installed};
use async_trait::async_trait;
use futures_util::future::{select, Either};
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;
use serde::Deserialize;
use serde_json::{json, Value};
use std::cell::{Cell, RefCell};
use std::ffi::OsStr;
use std::rc::Rc;
use std::time::{Duration, SystemTime};

const QUERY_TIMEOUT: Duration = Duration::from_millis(3000);
const INIT_TIMEOUT: Duration = Duration::from_millis(5000);
const MAX_FAILURES: u32 = 3;

struct Running {
    proc: gio::Subprocess,
    stdin: gio::OutputStream,
    stdout: gio::InputStream,
    buf: Vec<u8>,
}

struct Plugin {
    info: Installed,
    running: Option<Running>,
    failures: u32,
    next_id: u64,
    /// One request at a time per plugin; the reply stream is sequential.
    busy: Rc<Cell<bool>>,
}

impl Plugin {
    fn new(info: Installed) -> Self {
        Self {
            info,
            running: None,
            failures: 0,
            next_id: 1,
            busy: Rc::new(Cell::new(false)),
        }
    }

    fn start(&mut self) -> bool {
        let exec = self.info.exec_path();
        let argv = [exec.as_os_str()];
        let launcher = gio::SubprocessLauncher::new(
            gio::SubprocessFlags::STDIN_PIPE | gio::SubprocessFlags::STDOUT_PIPE,
        );
        launcher.set_cwd(&self.info.dir);
        launcher.setenv(
            OsStr::new("PARSEC_PLUGIN_DIR"),
            self.info.dir.as_os_str(),
            true,
        );
        let proc = match launcher.spawn(&argv) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(plugin = %self.info.manifest.id, "cannot start: {e}");
                self.failures += 1;
                return false;
            }
        };
        let (Some(stdin), Some(stdout)) = (proc.stdin_pipe(), proc.stdout_pipe()) else {
            return false;
        };
        self.running = Some(Running {
            proc,
            stdin,
            stdout,
            buf: Vec::new(),
        });
        tracing::info!(plugin = %self.info.manifest.id, "started");
        true
    }

    fn stop(&mut self) {
        if let Some(r) = self.running.take() {
            r.proc.force_exit();
        }
    }

    fn fail(&mut self, why: &str) {
        tracing::warn!(plugin = %self.info.manifest.id, "{why}");
        self.failures += 1;
        self.stop();
    }
}

async fn write_line(r: &Running, value: &Value) -> Result<(), String> {
    let mut line = value.to_string();
    line.push('\n');
    r.stdin
        .write_all_future(line.into_bytes(), glib::Priority::DEFAULT)
        .await
        .map(|_| ())
        .map_err(|(_, e)| e.to_string())
}

/// Next line from the plugin, or `None` at EOF.
async fn read_line(r: &mut Running) -> Option<String> {
    loop {
        if let Some(pos) = r.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = r.buf.drain(..=pos).collect();
            return Some(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        let chunk = vec![0u8; 64 * 1024];
        match r.stdout.read_future(chunk, glib::Priority::DEFAULT).await {
            Ok((_, 0)) => return None,
            Ok((b, n)) => r.buf.extend_from_slice(&b[..n]),
            Err(_) => return None,
        }
    }
}

/// Send one message and wait for its reply, with a timeout.
async fn roundtrip(r: &mut Running, msg: &Value, timeout: Duration) -> Result<Value, String> {
    write_line(r, msg).await?;
    let read = Box::pin(read_line(r));
    let clock = glib::timeout_future(timeout);
    match select(read, clock).await {
        Either::Left((Some(line), _)) => {
            serde_json::from_str(&line).map_err(|e| format!("bad JSON reply: {e}"))
        }
        Either::Left((None, _)) => Err("plugin exited".into()),
        Either::Right(_) => Err(format!("no reply within {}ms", timeout.as_millis())),
    }
}

#[derive(Deserialize)]
struct Reply {
    #[serde(default)]
    items: Vec<ReplyItem>,
}

#[derive(Deserialize)]
struct ReplyItem {
    title: String,
    #[serde(default)]
    subtitle: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    score: Option<u32>,
    #[serde(default)]
    actions: Vec<ReplyAction>,
}

#[derive(Deserialize)]
struct ReplyAction {
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    open: Option<String>,
    #[serde(default)]
    copy: Option<String>,
    #[serde(default)]
    copy_secret: Option<String>,
    #[serde(default)]
    run: Option<Vec<String>>,
    #[serde(default)]
    callback: Option<Value>,
}

pub struct PluginHost {
    cfg: SharedConfig,
    plugins: RefCell<Vec<Rc<RefCell<Plugin>>>>,
    /// (plugins dir mtime, disabled list) the current set was built from.
    built_from: RefCell<(Option<SystemTime>, Vec<String>)>,
}

impl PluginHost {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            plugins: RefCell::new(Vec::new()),
            built_from: RefCell::new((None, Vec::new())),
        }
    }

    /// Reload the plugin set when the directory or the disabled list changed.
    fn refresh(&self) {
        let mtime = std::fs::metadata(plugins::dir())
            .and_then(|m| m.modified())
            .ok();
        let disabled = self.cfg.borrow().plugins.disabled.clone();
        if *self.built_from.borrow() == (mtime, disabled.clone()) {
            return;
        }
        let wanted: Vec<Installed> = plugins::installed()
            .into_iter()
            .filter(|p| !disabled.contains(&p.manifest.id))
            .collect();
        let mut current = self.plugins.borrow_mut();
        // Stop what is gone or changed, keep what is unchanged.
        current.retain(|p| {
            let keep = {
                let p = p.borrow();
                wanted.iter().any(|w| {
                    w.manifest.id == p.info.manifest.id
                        && w.manifest.version == p.info.manifest.version
                        && w.manifest.exec == p.info.manifest.exec
                })
            };
            if !keep {
                p.borrow_mut().stop();
            }
            keep
        });
        for w in wanted {
            let known = current
                .iter()
                .any(|p| p.borrow().info.manifest.id == w.manifest.id);
            if !known {
                current.push(Rc::new(RefCell::new(Plugin::new(w))));
            }
        }
        tracing::debug!(count = current.len(), "plugins loaded");
        *self.built_from.borrow_mut() = (mtime, disabled);
    }

    async fn ask(&self, plugin: &Rc<RefCell<Plugin>>, q: &Query<'_>) -> Vec<Item> {
        // Serialize requests to this plugin.
        let busy = plugin.borrow().busy.clone();
        while busy.get() {
            glib::timeout_future(Duration::from_millis(5)).await;
        }
        busy.set(true);
        let result = self.ask_inner(plugin, q).await;
        busy.set(false);
        result
    }

    async fn ask_inner(&self, plugin: &Rc<RefCell<Plugin>>, q: &Query<'_>) -> Vec<Item> {
        if plugin.borrow().failures >= MAX_FAILURES {
            return Vec::new();
        }
        // Start (and init) on first use.
        let needs_start = plugin.borrow().running.is_none();
        if needs_start {
            let (ok, init) = {
                let mut p = plugin.borrow_mut();
                let ok = p.start();
                let init = json!({
                    "type": "init",
                    "version": crate::brand::VERSION,
                    "config": p.info.manifest.config,
                });
                (ok, init)
            };
            if !ok {
                return Vec::new();
            }
            let mut p = plugin.borrow_mut();
            let Some(r) = p.running.as_mut() else {
                return Vec::new();
            };
            if let Err(e) = roundtrip(r, &init, INIT_TIMEOUT).await {
                p.fail(&format!("init failed: {e}"));
                return Vec::new();
            }
        }
        let (id, msg) = {
            let mut p = plugin.borrow_mut();
            let id = p.next_id;
            p.next_id += 1;
            (
                id,
                json!({"type": "query", "id": id, "text": q.text, "keyword": q.verb}),
            )
        };
        let mut p = plugin.borrow_mut();
        let Some(r) = p.running.as_mut() else {
            return Vec::new();
        };
        let reply = match roundtrip(r, &msg, QUERY_TIMEOUT).await {
            Ok(v) => v,
            Err(e) => {
                p.fail(&format!("query failed: {e}"));
                return Vec::new();
            }
        };
        if reply.get("id").and_then(Value::as_u64) != Some(id) {
            p.fail("reply id mismatch");
            return Vec::new();
        }
        p.failures = 0;
        let parsed: Reply = match serde_json::from_value(reply) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(plugin = %p.info.manifest.id, "bad results: {e}");
                return Vec::new();
            }
        };
        let info = p.info.clone();
        drop(p);
        parsed
            .items
            .into_iter()
            .map(|it| self.item(plugin, &info, it, q.text))
            .collect()
    }

    fn item(
        &self,
        plugin: &Rc<RefCell<Plugin>>,
        info: &Installed,
        it: ReplyItem,
        text: &str,
    ) -> Item {
        let icon = match it.icon.as_deref().map(str::trim) {
            None | Some("") => info.icon(),
            Some(name) => {
                let path = info.dir.join(name);
                if path.is_file() {
                    Icon::Path(path)
                } else {
                    Icon::Named(name.to_string())
                }
            }
        };
        let item_id = it.id.unwrap_or_else(|| it.title.clone());
        let mut actions: Vec<Action> = it
            .actions
            .into_iter()
            .filter_map(|a| {
                let label = a.label.unwrap_or_default();
                let kind = if let Some(url) = a.open {
                    ActionKind::OpenUri(url)
                } else if let Some(t) = a.copy {
                    ActionKind::CopyText(t)
                } else if let Some(t) = a.copy_secret {
                    ActionKind::CopySecret {
                        text: t,
                        clear_after_secs: 15,
                    }
                } else if let Some(argv) = a.run {
                    ActionKind::Command(argv)
                } else if let Some(data) = a.callback {
                    let plugin = plugin.clone();
                    let msg =
                        json!({"type": "activate", "item": item_id, "data": data, "text": text});
                    ActionKind::Callback(Rc::new(move || {
                        let plugin = plugin.clone();
                        let msg = msg.clone();
                        glib::spawn_future_local(async move {
                            let busy = plugin.borrow().busy.clone();
                            while busy.get() {
                                glib::timeout_future(Duration::from_millis(5)).await;
                            }
                            busy.set(true);
                            let mut p = plugin.borrow_mut();
                            if let Some(r) = p.running.as_mut() {
                                if let Err(e) = roundtrip(r, &msg, QUERY_TIMEOUT).await {
                                    p.fail(&format!("activate failed: {e}"));
                                }
                            }
                            busy.set(false);
                        });
                        Ok(())
                    }))
                } else {
                    return None;
                };
                Some(Action {
                    label: if label.is_empty() {
                        "Run".into()
                    } else {
                        label
                    },
                    kind,
                })
            })
            .collect();
        if actions.is_empty() {
            // An item with nothing to do still shows; Enter just closes.
            actions.push(Action {
                label: String::new(),
                kind: ActionKind::Callback(Rc::new(|| Ok(()))),
            });
        }
        Item {
            id: format!("plugin:{}:{}", info.manifest.id, item_id),
            title: it.title,
            subtitle: it.subtitle.filter(|s| !s.is_empty()),
            icon,
            score: it.score.unwrap_or(500),
            actions,
        }
    }
}

#[async_trait(?Send)]
impl Provider for PluginHost {
    fn id(&self) -> &'static str {
        "plugins"
    }

    fn title(&self) -> String {
        "Plugins".into()
    }

    fn verb_label(&self, verb: &str) -> String {
        self.plugins
            .borrow()
            .iter()
            .find(|p| p.borrow().info.manifest.keywords.iter().any(|k| k == verb))
            .map(|p| p.borrow().info.manifest.name.clone())
            .unwrap_or_else(|| "Plugin".into())
    }

    fn prefixes(&self) -> Vec<String> {
        self.refresh();
        self.plugins
            .borrow()
            .iter()
            .flat_map(|p| p.borrow().info.manifest.keywords.clone())
            .collect()
    }

    fn accepts_unprefixed(&self) -> bool {
        self.refresh();
        self.plugins
            .borrow()
            .iter()
            .any(|p| p.borrow().info.manifest.keywords.is_empty())
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        self.refresh();
        let targets: Vec<Rc<RefCell<Plugin>>> = self
            .plugins
            .borrow()
            .iter()
            .filter(|p| {
                let kws = &p.borrow().info.manifest.keywords;
                match q.verb {
                    Some(v) => kws.iter().any(|k| k == v),
                    None => kws.is_empty(),
                }
            })
            .cloned()
            .collect();
        let mut items = Vec::new();
        for plugin in &targets {
            items.extend(self.ask(plugin, q).await);
        }
        items
    }
}
