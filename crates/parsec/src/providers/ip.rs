//! `ip`: this machine's addresses. The local one of each network card, the
//! public one websites see, and active VPNs, with whether they carry all
//! traffic or only some. `ip wifi`, `ip vpn`, `ip public` filter. Enter
//! copies the address.
//!
//! Interfaces are told apart by what the kernel says they are, not by
//! name: a `device` link means a real card (Wi-Fi when it has `wireless`),
//! `tun_flags` or a WireGuard DEVTYPE means a VPN, anything else (bridges,
//! veth pairs, Docker, loopback) is plumbing and stays hidden. Addresses
//! and routes come from iproute2's JSON output; VPN names from
//! NetworkManager when it's there.
//!
//! The public address takes a request to a lookup service, so it happens
//! only under the verb, is cached for a few minutes per route, and can be
//! turned off with `[ip] public_lookup = false`.

use super::SharedConfig;
use crate::core::{Action, ActionKind, Icon, Item, Provider, Query};
use async_trait::async_trait;
use gtk::gio;
use std::cell::RefCell;
use std::net::IpAddr;
use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

const PUBLIC_V4: &str = "https://api.ipify.org";
const PUBLIC_V6: &str = "https://api6.ipify.org";
const PUBLIC_TTL: Duration = Duration::from_secs(300);
/// Addresses used only to ask the kernel which route reaches the internet.
const PROBE_V4: &str = "1.1.1.1";
const PROBE_V6: &str = "2606:4700:4700::1111";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Wifi,
    Ethernet,
    Vpn,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Iface {
    name: String,
    kind: Kind,
    /// IPv4 addresses with their prefix length.
    v4: Vec<(String, u8)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Route {
    dev: String,
    gateway: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct Snapshot {
    ifaces: Vec<Iface>,
    route_v4: Option<Route>,
    route_v6: Option<Route>,
    vpn_names: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct Public {
    v4: Option<String>,
    v6: Option<String>,
}

/// What kind of interface `name` is, from its sysfs entry; `None` for
/// the ones not worth showing.
fn classify(sys: &Path) -> Option<Kind> {
    if sys.join("tun_flags").exists() {
        return Some(Kind::Vpn);
    }
    let uevent = std::fs::read_to_string(sys.join("uevent")).unwrap_or_default();
    if uevent.lines().any(|l| l == "DEVTYPE=wireguard") {
        return Some(Kind::Vpn);
    }
    if sys.join("device").exists() {
        let wifi = sys.join("wireless").exists() || sys.join("phy80211").exists();
        return Some(if wifi { Kind::Wifi } else { Kind::Ethernet });
    }
    None
}

/// `ip -j addr`: each interface with its global IPv4 addresses.
fn parse_addrs(json: &str) -> Vec<(String, Vec<(String, u8)>)> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).unwrap_or_default();
    list.iter()
        .filter_map(|i| {
            let name = i["ifname"].as_str()?.to_string();
            let v4 = i["addr_info"]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|a| a["family"] == "inet" && a["scope"] == "global")
                .filter_map(|a| {
                    Some((
                        a["local"].as_str()?.to_string(),
                        a["prefixlen"].as_u64().unwrap_or(32) as u8,
                    ))
                })
                .collect();
            Some((name, v4))
        })
        .collect()
}

/// `ip -j route get <addr>`: the interface and gateway that reach it.
fn parse_route(json: &str) -> Option<Route> {
    let list: Vec<serde_json::Value> = serde_json::from_str(json).ok()?;
    let r = list.first()?;
    Some(Route {
        dev: r["dev"].as_str()?.to_string(),
        gateway: r["gateway"].as_str().map(String::from),
    })
}

/// `nmcli -t -f TYPE,NAME connection show --active`: names of the VPNs.
/// The name comes last, as it's the field that can hold an escaped colon.
fn parse_vpn_names(text: &str) -> Vec<String> {
    text.lines()
        .filter_map(|l| l.split_once(':'))
        .filter(|(kind, _)| matches!(*kind, "vpn" | "wireguard"))
        .map(|(_, name)| name.replace("\\:", ":"))
        .collect()
}

fn output(program: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(program).args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Interfaces, routes and VPN names. Blocking: runs `ip` and `nmcli`.
fn snapshot() -> Snapshot {
    let sys = Path::new("/sys/class/net");
    let ifaces = output("ip", &["-j", "addr", "show"])
        .map(|j| parse_addrs(&j))
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(name, v4)| {
            let kind = classify(&sys.join(&name))?;
            Some(Iface { name, kind, v4 })
        })
        .collect();
    let route = |args: &[&str]| output("ip", args).and_then(|j| parse_route(&j));
    Snapshot {
        ifaces,
        route_v4: route(&["-j", "route", "get", PROBE_V4]),
        route_v6: route(&["-j", "-6", "route", "get", PROBE_V6]),
        vpn_names: output(
            "nmcli",
            &["-t", "-f", "TYPE,NAME", "connection", "show", "--active"],
        )
        .map(|t| parse_vpn_names(&t))
        .unwrap_or_default(),
    }
}

/// Ask the lookup service, only over the families that have a route.
/// Blocking: network.
fn fetch_public(v4: bool, v6: bool) -> Public {
    let agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(3)))
        .user_agent(format!("parsec/{}", crate::brand::VERSION))
        .build()
        .new_agent();
    let get = |url: &str| -> Option<String> {
        let text = agent
            .get(url)
            .call()
            .ok()?
            .body_mut()
            .read_to_string()
            .ok()?;
        let text = text.trim().to_string();
        text.parse::<IpAddr>().ok().map(|_| text)
    };
    Public {
        v4: v4.then(|| get(PUBLIC_V4)).flatten(),
        v6: v6.then(|| get(PUBLIC_V6)).flatten(),
    }
}

pub struct IpProvider {
    cfg: SharedConfig,
    /// The last lookup, the route it was made over, and when.
    public: RefCell<Option<(Instant, String, Public)>>,
}

impl IpProvider {
    pub fn new(cfg: SharedConfig) -> Self {
        Self {
            cfg,
            public: RefCell::new(None),
        }
    }

    /// The public addresses, cached until the route changes or it's stale.
    async fn public(&self, snap: &Snapshot) -> Public {
        let key = format!("{:?}{:?}", snap.route_v4, snap.route_v6);
        if let Some((at, k, p)) = &*self.public.borrow() {
            if *k == key && at.elapsed() < PUBLIC_TTL {
                return p.clone();
            }
        }
        let (v4, v6) = (snap.route_v4.is_some(), snap.route_v6.is_some());
        let found = gio::spawn_blocking(move || fetch_public(v4, v6))
            .await
            .unwrap_or_default();
        // A failed lookup isn't cached, so the next `ip` tries again.
        if found.v4.is_some() || found.v6.is_some() {
            *self.public.borrow_mut() = Some((Instant::now(), key, found.clone()));
        }
        found
    }
}

fn copy_actions(addr: &str, extra: Option<Action>) -> Vec<Action> {
    let mut actions = vec![Action {
        label: "Copy".into(),
        kind: ActionKind::CopyText(addr.to_string()),
    }];
    actions.extend(extra);
    actions
}

fn row(id: String, title: &str, subtitle: String, icon: &str, actions: Vec<Action>) -> Item {
    Item {
        output: None,
        preview: None,
        id,
        title: title.to_string(),
        subtitle: Some(subtitle),
        body: None,
        icon: Icon::Named(icon.into()),
        score: 1,
        actions,
    }
}

/// The rows, most useful first: the card that reaches the internet, the
/// public addresses, VPNs, then any other card.
fn items(snap: &Snapshot, public: &Public) -> Vec<Item> {
    let mut out = Vec::new();
    let internet = snap.route_v4.as_ref().map(|r| r.dev.as_str());
    let mut cards: Vec<&Iface> = snap.ifaces.iter().filter(|i| i.kind != Kind::Vpn).collect();
    cards.sort_by_key(|i| Some(i.name.as_str()) != internet);
    let mut cards = cards.into_iter();

    let card_rows = |i: &Iface, out: &mut Vec<Item>| {
        let (what, icon) = match i.kind {
            Kind::Wifi => ("Wi-Fi", "network-wireless-symbolic"),
            _ => ("Ethernet", "network-wired-symbolic"),
        };
        for (addr, prefix) in &i.v4 {
            let mut sub = format!("Local · {what} {}", i.name);
            if let Some(r) = snap.route_v4.as_ref().filter(|r| r.dev == i.name) {
                if let Some(gw) = &r.gateway {
                    sub.push_str(&format!(" · gateway {gw}"));
                }
            }
            let cidr = Action {
                label: "Copy with prefix".into(),
                kind: ActionKind::CopyText(format!("{addr}/{prefix}")),
            };
            out.push(row(
                format!("ip:local:{}", i.name),
                addr,
                sub,
                icon,
                copy_actions(addr, Some(cidr)),
            ));
        }
    };

    if let Some(first) = cards.next() {
        card_rows(first, &mut out);
    }
    for (addr, family) in [(&public.v4, "IPv4"), (&public.v6, "IPv6")] {
        if let Some(addr) = addr {
            let lookup = Action {
                label: "Look up on ipinfo.io".into(),
                kind: ActionKind::OpenUri(format!("https://ipinfo.io/{addr}")),
            };
            out.push(row(
                format!("ip:public:{family}"),
                addr,
                format!("Public {family} · what websites see"),
                "network-workgroup-symbolic",
                copy_actions(addr, Some(lookup)),
            ));
        }
    }
    for vpn in snap.ifaces.iter().filter(|i| i.kind == Kind::Vpn) {
        let traffic = if internet == Some(vpn.name.as_str()) {
            "all traffic"
        } else {
            "only some traffic"
        };
        let name = if snap.vpn_names.is_empty() {
            String::new()
        } else {
            format!("{} · ", snap.vpn_names.join(", "))
        };
        for (addr, _) in &vpn.v4 {
            out.push(row(
                format!("ip:vpn:{}", vpn.name),
                addr,
                format!("VPN · {name}{} · {traffic}", vpn.name),
                "network-vpn-symbolic",
                copy_actions(addr, None),
            ));
        }
    }
    for card in cards {
        card_rows(card, &mut out);
    }
    if out.is_empty() {
        out.push(row(
            "ip:offline".into(),
            "No network connection",
            "No network card has an address".into(),
            "network-offline-symbolic",
            vec![Action {
                label: String::new(),
                kind: ActionKind::Callback(std::rc::Rc::new(|| Ok(()))),
            }],
        ));
    }
    out
}

#[async_trait(?Send)]
impl Provider for IpProvider {
    fn id(&self) -> &'static str {
        "ip"
    }

    fn title(&self) -> String {
        "IP addresses".into()
    }

    fn prefixes(&self) -> Vec<String> {
        vec![self.cfg.borrow().verbs.ip.clone()]
    }

    async fn query(&self, q: &Query<'_>) -> Vec<Item> {
        let snap = gio::spawn_blocking(snapshot).await.unwrap_or_default();
        let lookup = self.cfg.borrow().ip.public_lookup;
        let public = if lookup && (snap.route_v4.is_some() || snap.route_v6.is_some()) {
            self.public(&snap).await
        } else {
            Public::default()
        };
        let rows = items(&snap, &public);
        let total = rows.len() as u32;
        rows.into_iter()
            .enumerate()
            .filter_map(|(i, mut item)| {
                item.score = if q.is_empty() {
                    total - i as u32
                } else {
                    let sub = item.subtitle.as_deref().unwrap_or("");
                    q.score_any([item.title.as_str(), sub])?
                };
                Some(item)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap() -> Snapshot {
        Snapshot {
            ifaces: vec![
                Iface {
                    name: "eth1".into(),
                    kind: Kind::Ethernet,
                    v4: vec![("10.0.0.5".into(), 8)],
                },
                Iface {
                    name: "wlan0".into(),
                    kind: Kind::Wifi,
                    v4: vec![("192.168.1.20".into(), 24)],
                },
                Iface {
                    name: "tun0".into(),
                    kind: Kind::Vpn,
                    v4: vec![("10.8.0.2".into(), 24)],
                },
            ],
            route_v4: Some(Route {
                dev: "wlan0".into(),
                gateway: Some("192.168.1.1".into()),
            }),
            route_v6: None,
            vpn_names: vec!["Office".into()],
        }
    }

    #[test]
    fn rows_put_the_internet_card_and_public_address_first() {
        let public = Public {
            v4: Some("203.0.113.7".into()),
            v6: None,
        };
        let rows = items(&snap(), &public);
        let titles: Vec<&str> = rows.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(
            titles,
            ["192.168.1.20", "203.0.113.7", "10.8.0.2", "10.0.0.5"]
        );
        assert_eq!(
            rows[0].subtitle.as_deref(),
            Some("Local · Wi-Fi wlan0 · gateway 192.168.1.1")
        );
        assert_eq!(
            rows[2].subtitle.as_deref(),
            Some("VPN · Office · tun0 · only some traffic")
        );
    }

    #[test]
    fn a_vpn_on_the_internet_route_carries_all_traffic() {
        let mut s = snap();
        s.route_v4 = Some(Route {
            dev: "tun0".into(),
            gateway: None,
        });
        let rows = items(&s, &Public::default());
        let vpn = rows.iter().find(|r| r.id == "ip:vpn:tun0").unwrap();
        assert!(vpn.subtitle.as_deref().unwrap().ends_with("all traffic"));
        assert!(!vpn.subtitle.as_deref().unwrap().contains("only"));
    }

    #[test]
    fn nothing_connected_says_so() {
        let rows = items(&Snapshot::default(), &Public::default());
        assert_eq!(rows[0].title, "No network connection");
    }

    #[test]
    fn iproute2_json() {
        let addrs = parse_addrs(
            r#"[{"ifname":"lo","addr_info":[{"family":"inet","local":"127.0.0.1","prefixlen":8,"scope":"host"}]},
                {"ifname":"wlan0","addr_info":[{"family":"inet","local":"192.168.1.20","prefixlen":24,"scope":"global"},
                                              {"family":"inet6","local":"fe80::1","prefixlen":64,"scope":"link"}]}]"#,
        );
        assert_eq!(addrs[0], ("lo".to_string(), vec![]));
        assert_eq!(addrs[1].1, vec![("192.168.1.20".to_string(), 24)]);
        let route = parse_route(
            r#"[{"dst":"1.1.1.1","gateway":"192.168.1.1","dev":"wlan0","prefsrc":"192.168.1.20"}]"#,
        );
        assert_eq!(
            route,
            Some(Route {
                dev: "wlan0".into(),
                gateway: Some("192.168.1.1".into())
            })
        );
        assert_eq!(parse_route("[]"), None);
    }

    #[test]
    fn vpn_names_from_nmcli() {
        let names = parse_vpn_names(
            "802-11-wireless:Home\nvpn:Office\\: Milan\nwireguard:wg-home\nbridge:br0\n",
        );
        assert_eq!(names, ["Office: Milan", "wg-home"]);
    }

    #[test]
    fn interfaces_are_told_apart_by_sysfs() {
        let root = std::env::temp_dir().join(format!("parsec-ip-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let make = |name: &str, files: &[&str], uevent: &str| {
            let d = root.join(name);
            std::fs::create_dir_all(&d).unwrap();
            for f in files {
                std::fs::create_dir_all(d.join(f)).unwrap();
            }
            std::fs::write(d.join("uevent"), uevent).unwrap();
            d
        };
        let wifi = make("wlan0", &["device", "wireless"], "INTERFACE=wlan0\n");
        let eth = make("eth0", &["device"], "INTERFACE=eth0\n");
        let tun = make("tun0", &["tun_flags"], "INTERFACE=tun0\n");
        let wg = make("wg0", &[], "DEVTYPE=wireguard\nINTERFACE=wg0\n");
        let bridge = make("br0", &["bridge"], "DEVTYPE=bridge\n");
        assert_eq!(classify(&wifi), Some(Kind::Wifi));
        assert_eq!(classify(&eth), Some(Kind::Ethernet));
        assert_eq!(classify(&tun), Some(Kind::Vpn));
        assert_eq!(classify(&wg), Some(Kind::Vpn));
        assert_eq!(classify(&bridge), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}
