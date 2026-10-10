//! `ps <name>`: your own processes from /proc, heaviest first. Enter asks
//! the process to quit (SIGTERM); Tab offers SIGKILL, a live view in your
//! terminal, and the PID. Other users' processes and kernel threads are
//! left out: you could not signal them anyway.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use std::cell::RefCell;
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

const CACHE_FOR: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    /// Average CPU share over the process lifetime, percent.
    pub cpu: f64,
    /// Resident memory, bytes.
    pub rss: u64,
}

pub struct ProcessesProvider {
    cfg: SharedConfig,
    cache: RefCell<Option<(Instant, Vec<Proc>)>>,
}

impl ProcessesProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            cache: RefCell::new(None),
        }
    }

    async fn procs(&self) -> Vec<Proc> {
        if let Some((at, list)) = self.cache.borrow().as_ref() {
            if at.elapsed() < CACHE_FOR {
                return list.clone();
            }
        }
        let list = gio::spawn_blocking(scan).await.unwrap_or_default();
        *self.cache.borrow_mut() = Some((Instant::now(), list.clone()));
        list
    }
}

/// Fields of /proc/[pid]/stat after the parenthesised name:
/// (utime, stime, starttime) in clock ticks.
pub fn parse_stat(stat: &str) -> Option<(String, u64, u64, u64)> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat[open + 1..close].to_string();
    let rest: Vec<&str> = stat[close + 1..].split_whitespace().collect();
    // rest[0] is the state; utime is field 14, stime 15, starttime 22 in
    // the 1-based numbering of proc(5), where the name is field 2.
    let field = |n: usize| rest.get(n - 3).and_then(|s| s.parse::<u64>().ok());
    Some((name, field(14)?, field(15)?, field(22)?))
}

fn scan() -> Vec<Proc> {
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let self_pid = std::process::id();
    let ticks = clock_ticks();
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0);
    let page = page_size();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in dir.flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == self_pid {
            continue;
        }
        let path = entry.path();
        if me.is_some() && std::fs::metadata(&path).map(|m| m.uid()).ok() != me {
            continue;
        }
        let Some(p) = read_proc(&path, pid, ticks, uptime, page) else {
            continue;
        };
        out.push(p);
    }
    out.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.rss.cmp(&a.rss))
    });
    out
}

fn read_proc(path: &Path, pid: u32, ticks: f64, uptime: f64, page: u64) -> Option<Proc> {
    let cmdline_raw = std::fs::read(path.join("cmdline")).ok()?;
    if cmdline_raw.is_empty() {
        return None; // kernel thread or zombie
    }
    let cmdline: String = cmdline_raw
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect::<Vec<_>>()
        .join(" ");
    let stat = std::fs::read_to_string(path.join("stat")).ok()?;
    let (name, utime, stime, start) = parse_stat(&stat)?;
    let alive = uptime - start as f64 / ticks;
    let cpu = if alive > 0.5 {
        ((utime + stime) as f64 / ticks) / alive * 100.0
    } else {
        0.0
    };
    let rss = std::fs::read_to_string(path.join("statm"))
        .ok()
        .and_then(|s| s.split_whitespace().nth(1)?.parse::<u64>().ok())
        .unwrap_or(0)
        * page;
    Some(Proc {
        pid,
        name,
        cmdline,
        cpu,
        rss,
    })
}

fn clock_ticks() -> f64 {
    std::process::Command::new("getconf")
        .arg("CLK_TCK")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<f64>()
                .ok()
        })
        .unwrap_or(100.0)
}

fn page_size() -> u64 {
    std::process::Command::new("getconf")
        .arg("PAGESIZE")
        .output()
        .ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(4096)
}

pub fn human_bytes(b: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    let mut v = b as f64;
    let mut i = 0;
    while v >= 1024.0 && i < UNITS.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{b} B")
    } else {
        format!("{v:.1} {}", UNITS[i])
    }
}

fn argv(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|s| s.to_string()).collect()
}

fn actions(cfg: &SharedConfig, p: &Proc) -> Vec<Action> {
    let pid = p.pid.to_string();
    let viewer = if crate::detect::which("htop").is_some() {
        vec!["htop".to_string(), "-p".into(), pid.clone()]
    } else {
        vec!["top".to_string(), "-p".into(), pid.clone()]
    };
    let home = dirs::home_dir()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_else(|| "/".into());
    vec![
        Action {
            label: "Terminate".into(),
            kind: ActionKind::Command(argv(&["kill", "-TERM", &pid])),
        },
        Action {
            label: "Kill".into(),
            kind: ActionKind::Command(argv(&["kill", "-KILL", &pid])),
        },
        Action {
            label: "Watch".into(),
            kind: ActionKind::Command(cfg.borrow().terminal_command(&home, &viewer)),
        },
        Action {
            label: "Copy PID".into(),
            kind: ActionKind::CopyText(pid),
        },
    ]
}

#[async_trait(?Send)]
impl Provider for ProcessesProvider {
    fn id(&self) -> &'static str {
        "processes"
    }

    fn title(&self) -> String {
        "Processes".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.processes.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let procs = self.procs().await;
        let n = procs.len() as u32;
        procs
            .iter()
            .enumerate()
            .filter_map(|(i, p)| {
                let score = if q.is_empty() {
                    n - i as u32
                } else {
                    // The name is what people type; a command line is
                    // long enough to fuzzy-match almost anything.
                    let by_name = q.score(&p.name);
                    let by_cmd = q.score(&p.cmdline).map(|s| s / 3);
                    by_name.into_iter().chain(by_cmd).max()?
                };
                Some(Item {
                    id: format!("proc:{}", p.name),
                    title: p.name.clone(),
                    subtitle: Some(format!(
                        "{} · {:.1}% cpu · {} · {}",
                        p.pid,
                        p.cpu,
                        human_bytes(p.rss),
                        p.cmdline
                    )),
                    icon: Icon::Named("utilities-system-monitor-symbolic".into()),
                    score,
                    actions: actions(&self.cfg, p),
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stat_with_spaces_in_the_name() {
        let line = "42 (Web Content) S 1 1 1 0 -1 4194304 129 0 0 0 7 3 0 0 20 0 1 0 563823 8511488 480 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30 31 32 33";
        let (name, utime, stime, start) = parse_stat(line).unwrap();
        assert_eq!(name, "Web Content");
        assert_eq!((utime, stime, start), (7, 3, 563823));
    }

    #[test]
    fn bytes_are_readable() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(3 * 1024 * 1024), "3.0 MiB");
    }

    #[test]
    fn scan_finds_ourselves_absent_but_others_present() {
        let list = scan();
        assert!(list.iter().all(|p| p.pid != std::process::id()));
        assert!(!list.is_empty());
    }
}
