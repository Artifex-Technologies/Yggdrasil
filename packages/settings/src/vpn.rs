//! Settings > VPN: NetworkManager's VPN and WireGuard connections with a switch each, Tailscale's status and switch when it is
//! installed, and Add VPN Configuration (WireGuard from a pasted or chosen .conf, OpenVPN from an .ovpn file, IKEv2) when
//! NetworkManager has the plugin for it.
//!
//! Only the user interface: the security monitor owns the network's security policy and is not changed here. Keys and
//! passwords go to NetworkManager (a WireGuard key inside the new connection over D-Bus; an IKEv2 password is asked by
//! NetworkManager's agent the first time it connects), never into a file of Settings' own.

use std::collections::HashMap;
use std::path::Path;

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;
use zbus::zvariant::Value;

use crate::{loading, sys, ui, App};

/// One VPN or WireGuard connection NetworkManager has.
#[derive(Clone, Debug, PartialEq)]
pub struct Vpn {
    pub name: String,
    pub uuid: String,
    pub wireguard: bool,
    /// "activated", "activating", "deactivating" or "" (off)
    pub state: String,
}

/// Tailscale, when it is installed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Tailscale {
    /// "Running", "Stopped", "NeedsLogin", "Starting", or empty when its service does not answer.
    pub state: String,
    /// The tailnet's name, when it says.
    pub tailnet: String,
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct State {
    pub vpns: Vec<Vpn>,
    pub tailscale: Option<Tailscale>,
    pub openvpn: bool,
    pub ikev2: bool,
}

pub const LIST_ARGS: [&str; 5] = ["-t", "-f", "NAME,UUID,TYPE,STATE", "connection", "show"];

/// `nmcli -t -f NAME,UUID,TYPE,STATE connection show`: the VPN and WireGuard ones, in nmcli's order (\: is a colon in a name).
pub fn parse_list(text: &str) -> Vec<Vpn> {
    text.lines()
        .filter_map(|l| {
            let f = sys::nm_fields(l);
            let (name, uuid, kind) = (f.first()?, f.get(1)?, f.get(2)?);
            let wireguard = kind == "wireguard";
            (wireguard || kind == "vpn").then(|| Vpn {
                name: name.clone(),
                uuid: uuid.clone(),
                wireguard,
                state: f.get(3).cloned().unwrap_or_default(),
            })
        })
        .collect()
}

/// `tailscale status --json`'s BackendState and the tailnet's name.
pub fn parse_tailscale(json: &str) -> Tailscale {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    Tailscale {
        state: v["BackendState"].as_str().unwrap_or_default().to_string(),
        tailnet: v["CurrentTailnet"]["Name"].as_str().or_else(|| v["MagicDNSSuffix"].as_str()).unwrap_or_default().to_string(),
    }
}

/// Whether a program or NetworkManager plugin is there (with a fixture: listed among its `tools`).
fn installed(tool: &str, path: &str) -> bool {
    match crate::fixture::get() {
        Some(f) => f.tools.iter().any(|t| t == tool),
        None => Path::new(path).exists(),
    }
}

pub fn load() -> State {
    let vpns = parse_list(&sys::run("nmcli", &LIST_ARGS).unwrap_or_default());
    let tailscale = installed("tailscale", "/usr/bin/tailscale")
        .then(|| sys::run("tailscale", &["status", "--json"]).map(|j| parse_tailscale(&j)).unwrap_or_default());
    State {
        vpns,
        tailscale,
        openvpn: installed("networkmanager-openvpn", "/usr/lib/NetworkManager/VPN/nm-openvpn-service.name"),
        ikev2: installed("networkmanager-strongswan", "/usr/lib/NetworkManager/VPN/nm-strongswan-service.name"),
    }
}

/// A WireGuard .conf file, read for NetworkManager.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WgConf {
    pub private_key: String,
    pub addresses: Vec<(String, u32)>,
    pub dns: Vec<String>,
    pub listen_port: Option<u32>,
    pub mtu: Option<u32>,
    pub peers: Vec<WgPeer>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WgPeer {
    pub public_key: String,
    pub preshared_key: Option<String>,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub keepalive: Option<u32>,
}

/// Reads a WireGuard .conf ([Interface] with PrivateKey, Address, DNS, ListenPort, MTU; [Peer]s with PublicKey, PresharedKey,
/// Endpoint, AllowedIPs, PersistentKeepalive). Err says what is missing.
pub fn parse_wg(text: &str) -> Result<WgConf, String> {
    let mut c = WgConf::default();
    let mut section = String::new();
    let list = |v: &str| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect::<Vec<_>>();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = line.trim_matches(['[', ']']).to_ascii_lowercase();
            if section == "peer" {
                c.peers.push(WgPeer::default());
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        match (section.as_str(), k.as_str()) {
            ("interface", "privatekey") => c.private_key = v.to_string(),
            ("interface", "address") => {
                for a in list(v) {
                    let (ip, prefix) = a.split_once('/').unwrap_or((&a, if a.contains(':') { "128" } else { "32" }));
                    c.addresses.push((ip.to_string(), prefix.parse().map_err(|_| format!("Address {a} does not read"))?));
                }
            }
            ("interface", "dns") => c.dns.extend(list(v)),
            ("interface", "listenport") => c.listen_port = v.parse().ok(),
            ("interface", "mtu") => c.mtu = v.parse().ok(),
            ("peer", _) => {
                let Some(p) = c.peers.last_mut() else { continue };
                match k.as_str() {
                    "publickey" => p.public_key = v.to_string(),
                    "presharedkey" => p.preshared_key = Some(v.to_string()),
                    "endpoint" => p.endpoint = Some(v.to_string()),
                    "allowedips" => p.allowed_ips.extend(list(v)),
                    "persistentkeepalive" => p.keepalive = v.parse().ok(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if c.private_key.is_empty() {
        return Err("The file has no PrivateKey in [Interface].".into());
    }
    if c.peers.is_empty() || c.peers.iter().any(|p| p.public_key.is_empty()) {
        return Err("Each [Peer] needs a PublicKey.".into());
    }
    Ok(c)
}

/// NetworkManager's settings for a WireGuard connection named `name` (what AddConnection takes). The keys travel in this
/// message to NetworkManager alone, which keeps them in its own store.
pub fn wg_settings(name: &str, c: &WgConf) -> HashMap<&'static str, HashMap<&'static str, Value<'static>>> {
    let iface: String = name.chars().filter(|ch| ch.is_ascii_alphanumeric() || *ch == '-' || *ch == '_').take(15).collect();
    let iface = if iface.is_empty() { "wg0".to_string() } else { iface };
    let mut conn = HashMap::new();
    conn.insert("id", Value::from(name.to_string()));
    conn.insert("type", Value::from("wireguard"));
    conn.insert("interface-name", Value::from(iface));
    conn.insert("autoconnect", Value::from(false));
    let mut wg = HashMap::new();
    wg.insert("private-key", Value::from(c.private_key.clone()));
    if let Some(p) = c.listen_port {
        wg.insert("listen-port", Value::from(p));
    }
    if let Some(m) = c.mtu {
        wg.insert("mtu", Value::from(m));
    }
    let peers: Vec<HashMap<String, Value<'static>>> = c
        .peers
        .iter()
        .map(|p| {
            let mut m: HashMap<String, Value<'static>> = HashMap::new();
            m.insert("public-key".into(), Value::from(p.public_key.clone()));
            if let Some(k) = &p.preshared_key {
                m.insert("preshared-key".into(), Value::from(k.clone()));
                // stored by NetworkManager with the connection (not asked each time)
                m.insert("preshared-key-flags".into(), Value::from(0u32));
            }
            if let Some(e) = &p.endpoint {
                m.insert("endpoint".into(), Value::from(e.clone()));
            }
            m.insert("allowed-ips".into(), Value::from(p.allowed_ips.clone()));
            if let Some(k) = p.keepalive {
                m.insert("persistent-keepalive".into(), Value::from(k));
            }
            m
        })
        .collect();
    wg.insert("peers", Value::from(peers));
    let ip = |v6: bool| {
        let mut m = HashMap::new();
        let addrs: Vec<HashMap<String, Value<'static>>> = c
            .addresses
            .iter()
            .filter(|(a, _)| a.contains(':') == v6)
            .map(|(a, p)| HashMap::from([("address".to_string(), Value::from(a.clone())), ("prefix".to_string(), Value::from(*p))]))
            .collect();
        let dns: Vec<String> = c.dns.iter().filter(|d| d.contains(':') == v6).cloned().collect();
        if addrs.is_empty() {
            m.insert("method", Value::from(if v6 { "ignore" } else { "disabled" }));
        } else {
            m.insert("method", Value::from("manual"));
            m.insert("address-data", Value::from(addrs));
        }
        if !dns.is_empty() {
            m.insert("dns-data", Value::from(dns));
        }
        m
    };
    HashMap::from([("connection", conn), ("wireguard", wg), ("ipv4", ip(false)), ("ipv6", ip(true))])
}

/// Adds the WireGuard connection to NetworkManager (AddConnection over the system bus).
pub fn add_wireguard(name: &str, conf_text: &str) -> Result<(), String> {
    let c = parse_wg(conf_text)?;
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    let bus = zbus::blocking::Connection::system().map_err(|e| format!("NetworkManager could not be reached: {e}"))?;
    let settings = zbus::blocking::Proxy::new(
        &bus,
        "org.freedesktop.NetworkManager",
        "/org/freedesktop/NetworkManager/Settings",
        "org.freedesktop.NetworkManager.Settings",
    )
    .map_err(|e| e.to_string())?;
    settings
        .call::<_, _, zbus::zvariant::OwnedObjectPath>("AddConnection", &(wg_settings(name, &c),))
        .map(|_| ())
        .map_err(|e| format!("NetworkManager did not take it: {e}"))
}

/// A change to run off the window's thread (adding the configuration).
type Job = Box<dyn FnOnce() -> Result<(), String> + Send>;

/// The kinds Add VPN Configuration offers.
pub const KINDS: [&str; 3] = ["WireGuard", "OpenVPN", "IKEv2"];

/// Add VPN Configuration's form.
#[derive(Default)]
pub struct Form {
    pub kind: usize,
    pub name: String,
    /// WireGuard: the pasted .conf; OpenVPN: unused.
    pub text: String,
    /// A file to import (.conf or .ovpn).
    pub file: String,
    /// IKEv2: the server and the user name (the password is asked by NetworkManager when it first connects).
    pub server: String,
    pub user: String,
}

impl Form {
    /// Wipes what was typed (a pasted .conf holds a private key).
    pub fn clear(&mut self) {
        for s in [&mut self.text, &mut self.name, &mut self.file, &mut self.server, &mut self.user] {
            // overwrite before letting go, as the distress page does with its passwords
            let n = s.len();
            s.clear();
            s.extend(std::iter::repeat_n(' ', n));
            s.clear();
        }
    }
}

impl App {
    /// Settings > VPN.
    pub(crate) fn page_vpn(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(st) = self.data.vpn.clone() else { return loading(ui) };
        if self.vpn_add {
            return self.vpn_add_page(ui, pal, ctx, &st);
        }
        let mut toggle: Option<(String, bool)> = None;
        ui::group(ui, pal, Some("Configurations"), |ui| {
            if st.vpns.is_empty() {
                ui::row(ui, pal, true, "No VPN configurations", Some("Add one below, or install one from your VPN's provider"), |_| {});
            }
            for (i, v) in st.vpns.iter().enumerate() {
                let status = match v.state.as_str() {
                    "activated" => "Connected",
                    "activating" => "Connecting…",
                    "deactivating" => "Disconnecting…",
                    _ => "Not Connected",
                };
                let kind = if v.wireguard { "WireGuard" } else { "VPN" };
                ui::row(ui, pal, i == 0, &v.name, Some(&format!("{kind} · {status}")), |ui| {
                    let mut on = !v.state.is_empty() && v.state != "deactivating";
                    if ui::switch(ui, pal, &mut on).changed() {
                        toggle = Some((v.uuid.clone(), on));
                    }
                });
            }
        });
        ui::group(ui, pal, None, |ui| {
            if ui::nav_row(ui, pal, true, "Add VPN Configuration…", None, |_| {}) {
                self.vpn_add = true;
                self.vpn_form.clear();
                self.vpn_form.kind = 0;
            }
        });
        ui::footnote(
            ui,
            pal,
            "Connections are NetworkManager's; a password a VPN needs is asked when it connects and kept by NetworkManager.",
        );
        let mut ts_toggle: Option<bool> = None;
        if let Some(t) = &st.tailscale {
            ui::group(ui, pal, Some("Tailscale"), |ui| {
                let status = match t.state.as_str() {
                    "Running" if t.tailnet.is_empty() => "Connected".to_string(),
                    "Running" => format!("Connected to {}", t.tailnet),
                    "NeedsLogin" => "Signed out: sign in with `tailscale login` first".into(),
                    "Starting" => "Connecting…".into(),
                    "" => "Its service is not running".into(),
                    _ => "Not Connected".into(),
                };
                ui::row(ui, pal, true, "Tailscale", Some(&status), |ui| {
                    let mut on = t.state == "Running" || t.state == "Starting";
                    ui.add_enabled_ui(matches!(t.state.as_str(), "Running" | "Stopped" | "Starting"), |ui| {
                        if ui::switch(ui, pal, &mut on).changed() {
                            ts_toggle = Some(on);
                        }
                    });
                });
            });
            ui::footnote(
                ui,
                pal,
                "Connects and disconnects with `tailscale up` and `tailscale down`; nothing else of Tailscale's is changed.",
            );
        }
        if let Some((uuid, on)) = toggle {
            self.change(ctx, move || sys::act("nmcli", &["connection", if on { "up" } else { "down" }, "uuid", &uuid]));
        }
        if let Some(on) = ts_toggle {
            self.change(ctx, move || sys::act("tailscale", &[if on { "up" } else { "down" }]));
        }
    }

    /// VPN > Add VPN Configuration.
    fn vpn_add_page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, st: &State) {
        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Type", None, |ui| {
                if let Some(i) = ui::segmented(ui, pal, &KINDS, self.vpn_form.kind) {
                    self.vpn_form.kind = i;
                }
            });
        });
        let f = &mut self.vpn_form;
        let mut add: Option<Result<Job, String>> = None;
        match f.kind {
            0 => {
                ui::group(ui, pal, Some("WireGuard"), |ui| {
                    ui::row(ui, pal, true, "Name", None, |ui| {
                        ui::field(ui, pal, &mut f.name, "Office", 220.0, false);
                    });
                    ui::row(ui, pal, false, "From a file", Some("A .conf from your VPN's provider"), |ui| {
                        ui::field(ui, pal, &mut f.file, "/home/…/wg0.conf", 220.0, false);
                    });
                    ui.add_space(6.0);
                    ui.label(RichText::new("Or paste the configuration").size(12.5).color(pal.dim));
                    egui::Frame::new()
                        .fill(pal.bg)
                        .stroke(egui::Stroke::new(1.0, pal.border))
                        .corner_radius(egui::CornerRadius::same(8))
                        .inner_margin(egui::Margin::symmetric(8, 6))
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut f.text)
                                    .hint_text(
                                        "[Interface]\nPrivateKey = …\nAddress = …\n\n[Peer]\nPublicKey = …\nEndpoint = …\nAllowedIPs = …",
                                    )
                                    .frame(egui::Frame::NONE)
                                    .desired_rows(8)
                                    .desired_width(f32::INFINITY)
                                    .font(egui::TextStyle::Monospace),
                            )
                        });
                    ui.add_space(6.0);
                });
                ui::footnote(ui, pal, "Its private key goes straight to NetworkManager, which keeps it; Settings writes it nowhere.");
                if ui::action(ui, pal, "Add").clicked() {
                    let text = if f.file.trim().is_empty() {
                        Ok(f.text.clone())
                    } else {
                        std::fs::read_to_string(f.file.trim()).map_err(|e| format!("{}: {e}", f.file.trim()))
                    };
                    let name = if f.name.trim().is_empty() {
                        Path::new(f.file.trim()).file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "WireGuard".into())
                    } else {
                        f.name.trim().to_string()
                    };
                    add = Some(text.and_then(|t| {
                        parse_wg(&t)?;
                        Ok(Box::new(move || add_wireguard(&name, &t)) as Job)
                    }));
                }
            }
            1 if !st.openvpn => needs(ui, pal, "OpenVPN", "networkmanager-openvpn"),
            1 => {
                ui::group(ui, pal, Some("OpenVPN"), |ui| {
                    ui::row(ui, pal, true, "Configuration file", Some("The .ovpn file from your VPN's provider"), |ui| {
                        ui::field(ui, pal, &mut f.file, "/home/…/office.ovpn", 240.0, false);
                    });
                });
                ui::footnote(
                    ui,
                    pal,
                    "NetworkManager imports the file into its own store; the password, if any, is asked when it connects.",
                );
                if ui::action(ui, pal, "Import").clicked() {
                    let file = f.file.trim().to_string();
                    add = Some(if Path::new(&file).is_file() {
                        Ok(Box::new(move || sys::act("nmcli", &["connection", "import", "type", "openvpn", "file", &file])))
                    } else {
                        Err("Choose an .ovpn file first.".into())
                    });
                }
            }
            _ if !st.ikev2 => needs(ui, pal, "IKEv2", "networkmanager-strongswan"),
            _ => {
                ui::group(ui, pal, Some("IKEv2"), |ui| {
                    ui::row(ui, pal, true, "Name", None, |ui| {
                        ui::field(ui, pal, &mut f.name, "Office", 220.0, false);
                    });
                    ui::row(ui, pal, false, "Server", None, |ui| {
                        ui::field(ui, pal, &mut f.server, "vpn.example.com", 220.0, false);
                    });
                    ui::row(ui, pal, false, "User name", None, |ui| {
                        ui::field(ui, pal, &mut f.user, "", 220.0, false);
                    });
                });
                ui::footnote(ui, pal, "Signs in with a user name and password (EAP). The password is asked when it first connects and kept by NetworkManager.");
                if ui::action(ui, pal, "Add").clicked() {
                    let (name, server, user) = (f.name.trim().to_string(), f.server.trim().to_string(), f.user.trim().to_string());
                    // vpn.data is "key=value, key=value": a comma or = in them would add settings of their own
                    let odd = |t: &str| t.contains([',', '=']);
                    add = Some(if name.is_empty() || server.is_empty() || user.is_empty() || odd(&server) || odd(&user) {
                        Err("A name, the server and a user name are needed (without commas or =).".into())
                    } else {
                        Ok(Box::new(move || {
                            let data = format!("address={server}, method=eap, user={user}, virtual=yes, encap=no, ipcomp=no");
                            sys::act(
                                "nmcli",
                                &[
                                    "connection",
                                    "add",
                                    "type",
                                    "vpn",
                                    "vpn-type",
                                    "strongswan",
                                    "con-name",
                                    &name,
                                    "vpn.data",
                                    &data,
                                    "autoconnect",
                                    "no",
                                ],
                            )
                        }))
                    });
                }
            }
        }
        match add {
            Some(Ok(job)) => {
                self.vpn_form.clear();
                self.vpn_add = false;
                self.change(ctx, job);
            }
            Some(Err(e)) => self.status = e,
            None => {}
        }
    }
}

/// A kind whose NetworkManager plugin is not installed: what it needs.
fn needs(ui: &mut egui::Ui, pal: &Palette, kind: &str, package: &str) {
    ui::group(ui, pal, Some(kind), |ui| {
        ui::row(
            ui,
            pal,
            true,
            &format!("{kind} needs NetworkManager's {kind} plugin"),
            Some(&format!("Install the {package} package (Agora, or your package manager), then open this page again")),
            |_| {},
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_has_the_vpn_and_wireguard_connections() {
        let text = "Home Wi-Fi:6b0c1f2e-0000-4000-8000-00000000a001:802-11-wireless:activated\n\
                    Office:6b0c1f2e-0000-4000-8000-00000000b001:vpn:\n\
                    Travel\\: home:6b0c1f2e-0000-4000-8000-00000000b002:wireguard:activated\n\
                    lo:6b0c1f2e-0000-4000-8000-00000000c001:loopback:activated\n";
        let v = parse_list(text);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].name.as_str(), v[0].wireguard, v[0].state.as_str()), ("Office", false, ""));
        assert_eq!((v[1].name.as_str(), v[1].wireguard, v[1].state.as_str()), ("Travel: home", true, "activated"));
    }

    #[test]
    fn tailscale_status_reads() {
        let t = parse_tailscale(r#"{"BackendState":"Running","CurrentTailnet":{"Name":"example.ts.net"}}"#);
        assert_eq!((t.state.as_str(), t.tailnet.as_str()), ("Running", "example.ts.net"));
        assert_eq!(parse_tailscale("not json").state, "");
    }

    /// A made-up configuration (the keys are 32 zero bytes and a test pattern, not anyone's).
    const CONF: &str = "[Interface]\n\
        PrivateKey = AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=\n\
        Address = 192.0.2.2/32, 2001:db8::2/128 # both\n\
        DNS = 192.0.2.1\n\
        \n\
        [Peer]\n\
        PublicKey = AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=\n\
        Endpoint = vpn.example.com:51820\n\
        AllowedIPs = 0.0.0.0/0, ::/0\n\
        PersistentKeepalive = 25\n";

    #[test]
    fn a_wireguard_conf_becomes_networkmanager_settings() {
        let c = parse_wg(CONF).unwrap();
        assert_eq!(c.addresses, [("192.0.2.2".to_string(), 32), ("2001:db8::2".to_string(), 128)]);
        assert_eq!(c.peers.len(), 1);
        assert_eq!(c.peers[0].allowed_ips, ["0.0.0.0/0", "::/0"]);
        assert_eq!(c.peers[0].keepalive, Some(25));
        let s = wg_settings("Travel home!", &c);
        assert_eq!(s["connection"]["type"], Value::from("wireguard"));
        assert_eq!(s["connection"]["interface-name"], Value::from("Travelhome"), "a name the kernel takes");
        assert_eq!(s["ipv4"]["method"], Value::from("manual"));
        assert_eq!(s["ipv6"]["method"], Value::from("manual"));
        assert_eq!(s["ipv4"]["dns-data"], Value::from(vec!["192.0.2.1".to_string()]));
        assert!(!s["ipv6"].contains_key("dns-data"));
        assert!(parse_wg("[Interface]\nAddress = 10.0.0.2/32\n").unwrap_err().contains("PrivateKey"));
        assert!(parse_wg("[Interface]\nPrivateKey = x\n[Peer]\nEndpoint = a:1\n").unwrap_err().contains("PublicKey"));
    }
}
