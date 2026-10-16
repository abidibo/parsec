//! `ps <name>`: your own processes from /proc, heaviest first. Enter asks
//! the process to quit (SIGTERM); Tab offers SIGKILL, a live view in your
//! terminal, and the PID. Other users' processes and kernel threads are
//! left out: you could not signal them anyway.
//!
//! `ps :3000` looks processes up by the TCP port they listen on, from
//! /proc/net/tcp{,6} matched against the sockets in /proc/[pid]/fd. Other
//! users' fds are not readable, so their ports are named after the Docker
//! container that publishes them or, failing that, the user that owns them.

use super::infra::{container_actions, list_containers, published_ports, Container};
use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::Path;
use std::time::{Duration, Instant};

const CACHE_FOR: Duration = Duration::from_secs(2);
const DOCKER_CACHE_FOR: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub name: String,
    pub cmdline: String,
    /// Average CPU share over the process lifetime, percent.
    pub cpu: f64,
    /// Resident memory, bytes.
    pub rss: u64,
    /// TCP ports it listens on, sorted.
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    procs: Vec<Proc>,
    /// Listening ports whose socket belongs to none of our processes,
    /// with the name of the user that owns the socket.
    foreign: Vec<(u16, String)>,
}

/// Who answers on a port, as far as we can tell.
enum Owner<'a> {
    Proc(&'a Proc),
    Container(&'a Container),
    User(&'a str),
}

pub struct ProcessesProvider {
    cfg: SharedConfig,
    cache: RefCell<Option<(Instant, Snapshot)>>,
    docker: bool,
    containers: RefCell<Option<(Instant, Vec<Container>)>>,
}

impl ProcessesProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            cache: RefCell::new(None),
            docker: crate::detect::which("docker").is_some(),
            containers: RefCell::new(None),
        }
    }

    /// Only asked for by port queries: `docker ps` is too slow for every
    /// keystroke of a name search.
    async fn containers(&self) -> Vec<Container> {
        if !self.docker {
            return Vec::new();
        }
        if let Some((at, list)) = self.containers.borrow().as_ref() {
            if at.elapsed() < DOCKER_CACHE_FOR {
                return list.clone();
            }
        }
        let list = gio::spawn_blocking(list_containers)
            .await
            .ok()
            .flatten()
            .unwrap_or_default();
        *self.containers.borrow_mut() = Some((Instant::now(), list.clone()));
        list
    }

    async fn snapshot(&self) -> Snapshot {
        if let Some((at, snap)) = self.cache.borrow().as_ref() {
            if at.elapsed() < CACHE_FOR {
                return snap.clone();
            }
        }
        let snap = gio::spawn_blocking(scan).await.unwrap_or_default();
        *self.cache.borrow_mut() = Some((Instant::now(), snap.clone()));
        snap
    }

    async fn by_port(&self, wanted: &str, snap: &Snapshot) -> Vec<Item> {
        if !wanted.chars().all(|c| c.is_ascii_digit()) {
            return Vec::new();
        }
        let containers = if snap.foreign.is_empty() {
            Vec::new()
        } else {
            self.containers().await
        };
        let ours: HashSet<u16> = snap.procs.iter().flat_map(|p| p.ports.clone()).collect();
        // Published ports are listed even without a socket: with Docker's
        // userland proxy off, iptables forwards them and nothing listens.
        let published: Vec<(u16, &Container)> = containers
            .iter()
            .flat_map(|c| published_ports(&c.ports).into_iter().map(move |p| (p, c)))
            .filter(|(port, _)| !ours.contains(port))
            .collect();
        let docker_ports: HashSet<u16> = published.iter().map(|(p, _)| *p).collect();
        let mut rows: Vec<(u16, Owner)> = snap
            .procs
            .iter()
            .flat_map(|p| p.ports.iter().map(move |&port| (port, Owner::Proc(p))))
            .chain(
                published
                    .into_iter()
                    .map(|(port, c)| (port, Owner::Container(c))),
            )
            .chain(
                snap.foreign
                    .iter()
                    .filter(|(port, _)| !docker_ports.contains(port))
                    .map(|(port, user)| (*port, Owner::User(user))),
            )
            .filter(|(port, _)| port.to_string().starts_with(wanted))
            .collect();
        rows.sort_by_key(|(port, _)| *port);
        let n = rows.len() as u32;
        let cfg = self.cfg.borrow();
        rows.into_iter()
            .enumerate()
            .map(|(i, (port, owner))| {
                let exact = port.to_string() == wanted;
                let score = n - i as u32 + if exact { n } else { 0 };
                let open = Action {
                    label: "Open in browser".into(),
                    kind: ActionKind::OpenUri(format!("http://localhost:{port}")),
                };
                let (title, subtitle, actions) = match owner {
                    Owner::Proc(p) => {
                        let mut acts = actions(&self.cfg, p);
                        acts.insert(1, open);
                        (
                            format!(":{port} · {}", p.name),
                            format!("{} · {}", p.pid, p.cmdline),
                            acts,
                        )
                    }
                    Owner::Container(c) => {
                        let mut acts = container_actions(&cfg, c);
                        acts.insert(0, open);
                        (
                            format!(":{port} · {}", c.names),
                            format!("docker · {} · {}", c.image, c.status),
                            acts,
                        )
                    }
                    Owner::User(user) => (
                        format!(":{port} · {user}"),
                        format!("listening as {user}; only root can see which process"),
                        vec![open],
                    ),
                };
                Item {
                    output: None,
                    preview: None,
                    body: None,
                    id: format!("port:{port}"),
                    title,
                    subtitle: Some(subtitle),
                    icon: Icon::Named("network-server-symbolic".into()),
                    score,
                    actions,
                }
            })
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Listener {
    pub port: u16,
    pub uid: u32,
}

/// Listening sockets in /proc/net/tcp or tcp6 format, by inode.
pub fn parse_listening(table: &str) -> Vec<(u64, Listener)> {
    const LISTEN: &str = "0A";
    table
        .lines()
        .skip(1)
        .filter_map(|line| {
            let f: Vec<&str> = line.split_whitespace().collect();
            if f.get(3) != Some(&LISTEN) {
                return None;
            }
            let port = u16::from_str_radix(f.get(1)?.rsplit(':').next()?, 16).ok()?;
            let uid = f.get(7)?.parse::<u32>().ok()?;
            let inode = f.get(9)?.parse::<u64>().ok()?;
            (inode != 0).then_some((inode, Listener { port, uid }))
        })
        .collect()
}

fn listening() -> HashMap<u64, Listener> {
    ["/proc/net/tcp", "/proc/net/tcp6"]
        .iter()
        .filter_map(|f| std::fs::read_to_string(f).ok())
        .flat_map(|t| parse_listening(&t))
        .collect()
}

/// The inode of an fd link target like `socket:[12345]`.
pub fn socket_inode(target: &str) -> Option<u64> {
    target
        .strip_prefix("socket:[")?
        .strip_suffix(']')?
        .parse()
        .ok()
}

/// User names by uid, from /etc/passwd.
pub fn parse_passwd(text: &str) -> HashMap<u32, String> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?;
            let uid = f.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

/// Ports among `listening` that the process at `path` holds a socket for.
fn ports_of(path: &Path, listening: &HashMap<u64, Listener>, seen: &mut HashSet<u64>) -> Vec<u16> {
    let Ok(fds) = std::fs::read_dir(path.join("fd")) else {
        return Vec::new();
    };
    let mut ports: Vec<u16> = fds
        .flatten()
        .filter_map(|fd| {
            let target = std::fs::read_link(fd.path()).ok()?;
            let inode = socket_inode(target.to_str()?)?;
            let port = listening.get(&inode)?.port;
            seen.insert(inode);
            Some(port)
        })
        .collect();
    ports.sort_unstable();
    ports.dedup();
    ports
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

fn scan() -> Snapshot {
    let me = std::fs::metadata("/proc/self").map(|m| m.uid()).ok();
    let self_pid = std::process::id();
    let ticks = clock_ticks();
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|s| s.split_whitespace().next()?.parse::<f64>().ok())
        .unwrap_or(0.0);
    let page = page_size();
    let Ok(dir) = std::fs::read_dir("/proc") else {
        return Snapshot::default();
    };
    let sockets = listening();
    let mut seen = HashSet::new();
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
        let Some(mut p) = read_proc(&path, pid, ticks, uptime, page) else {
            continue;
        };
        if !sockets.is_empty() {
            p.ports = ports_of(&path, &sockets, &mut seen);
        }
        out.push(p);
    }
    out.sort_by(|a, b| {
        b.cpu
            .partial_cmp(&a.cpu)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| b.rss.cmp(&a.rss))
    });
    // A port can have a socket of ours on IPv4 and someone else's on IPv6;
    // the owner we know about wins.
    let ours: HashSet<u16> = out.iter().flat_map(|p| p.ports.iter().copied()).collect();
    let users = std::fs::read_to_string("/etc/passwd")
        .map(|t| parse_passwd(&t))
        .unwrap_or_default();
    let mut foreign: Vec<(u16, String)> = sockets
        .iter()
        .filter(|(inode, l)| !seen.contains(inode) && !ours.contains(&l.port))
        .map(|(_, l)| {
            let user = users.get(&l.uid).cloned();
            (l.port, user.unwrap_or_else(|| format!("uid {}", l.uid)))
        })
        .collect();
    foreign.sort();
    foreign.dedup_by_key(|(port, _)| *port);
    Snapshot {
        procs: out,
        foreign,
    }
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
        ports: Vec::new(),
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
        let snap = self.snapshot().await;
        if let Some(port) = q.text.trim().strip_prefix(':') {
            return self.by_port(port.trim(), &snap).await;
        }
        let procs = &snap.procs;
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
                let ports: String = p.ports.iter().map(|port| format!(":{port} · ")).collect();
                Some(Item {
                    output: None,
                    preview: None,
                    body: None,
                    id: format!("proc:{}", p.name),
                    title: p.name.clone(),
                    subtitle: Some(format!(
                        "{} · {:.1}% cpu · {} · {}{}",
                        p.pid,
                        p.cpu,
                        human_bytes(p.rss),
                        ports,
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
    fn listening_sockets_only() {
        let table = "  sl  local_address rem_address   st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 0100007F:0BB8 00000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 41234 1 0000000000000000 100 0 0 10 0
   1: 0100007F:0BB8 0100007F:D431 01 00000000:00000000 00:00000000 00000000  1000        0 41299 1 0000000000000000 20 4 30 10 -1
   2: 00000000:1538 00000000:0000 0A 00000000:00000000 00:00000000 00000000   120        0 18821 1 0000000000000000 100 0 0 10 0";
        let l = |port, uid| Listener { port, uid };
        assert_eq!(
            parse_listening(table),
            vec![(41234, l(3000, 1000)), (18821, l(5432, 120))]
        );
        let v6 = "  sl  local_address                         remote_address                        st tx_queue rx_queue tr tm->when retrnsmt   uid  timeout inode
   0: 00000000000000000000000000000000:2405 00000000000000000000000000000000:0000 0A 00000000:00000000 00:00000000 00000000  1000        0 52001 1 0000000000000000 100 0 0 10 0";
        assert_eq!(parse_listening(v6), vec![(52001, l(9221, 1000))]);
    }

    #[test]
    fn passwd_names() {
        let users = parse_passwd("root:x:0:0:root:/root:/bin/bash\nollama:x:996:986::/usr/share/ollama:/bin/false\n# junk\n");
        assert_eq!(users.get(&0).map(String::as_str), Some("root"));
        assert_eq!(users.get(&996).map(String::as_str), Some("ollama"));
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn socket_links() {
        assert_eq!(socket_inode("socket:[41234]"), Some(41234));
        assert_eq!(socket_inode("pipe:[41234]"), None);
        assert_eq!(socket_inode("/dev/null"), None);
    }

    #[test]
    fn finds_our_own_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let mut seen = HashSet::new();
        let ports = ports_of(Path::new("/proc/self"), &listening(), &mut seen);
        assert!(ports.contains(&port), "{port} not in {ports:?}");
    }

    #[test]
    fn scan_finds_ourselves_absent_but_others_present() {
        let list = scan().procs;
        assert!(list.iter().all(|p| p.pid != std::process::id()));
        assert!(!list.is_empty());
    }
}
