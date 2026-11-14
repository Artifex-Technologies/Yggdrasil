//! What NetworkManager sees: the nearby Wi-Fi networks and the network adapters, read from `nmcli -t` (terse, read only; no
//! password is ever on these command lines). Every parser is a pure function of nmcli's output, tested without nmcli.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub in_use: bool,
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
}

/// Split one `nmcli -t` line on unescaped colons (nmcli escapes ':' in values as '\:').
pub fn nm_fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

/// nmcli's SECURITY for a network that asks for a password or a sign-in. "OWE" and "OWE-TM" (Enhanced Open: encrypted, but no
/// password, like many shop and guest networks) are open: before, they showed a lock and asked for a password there is none of.
pub fn nm_secured(security: &str) -> bool {
    let s = security.trim();
    !(s.is_empty() || s == "--" || s.split_whitespace().all(|t| t.starts_with("OWE")))
}

/// `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list`, strongest first, one row per name.
pub fn wifi_list(text: &str) -> Vec<Wifi> {
    let mut best: BTreeMap<String, Wifi> = BTreeMap::new();
    for l in text.lines() {
        let f = nm_fields(l);
        if f.len() < 4 || f[1].is_empty() {
            continue;
        }
        let w = Wifi { in_use: f[0] == "*", ssid: f[1].clone(), signal: f[2].parse().unwrap_or(0), secured: nm_secured(&f[3]) };
        let keep = match best.get(&w.ssid) {
            Some(old) => w.in_use || (!old.in_use && w.signal > old.signal),
            None => true,
        };
        if keep {
            best.insert(w.ssid.clone(), w);
        }
    }
    let mut v: Vec<Wifi> = best.into_values().collect();
    fold_owe_twins(&mut v);
    v.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)));
    v
}

/// The open network an Enhanced Open twin belongs to, by the twin's name: "_owetm_" + the start of the open network's name +
/// digits (an Aruba guest network, "_owetm_Shop Gue2770611103" for "Shop Guest").
pub fn owe_twin_of<'a>(twin: &str, names: impl IntoIterator<Item = &'a str>) -> Option<&'a str> {
    let rest = twin.strip_prefix("_owetm_")?.trim_end_matches(|c: char| c.is_ascii_digit());
    if rest.is_empty() {
        return None;
    }
    names.into_iter().find(|n| *n != twin && n.starts_with(rest))
}

/// One row per network, as a phone shows it: an Enhanced Open network in transition mode is an open network and a hidden,
/// encrypted twin the computer actually joins. The twin's row goes, and when the twin is the one in use, its open network's
/// row says so (it was listed twice, once under the twin's odd name, 2026-11-13).
fn fold_owe_twins(v: &mut Vec<Wifi>) {
    let names: Vec<String> = v.iter().map(|w| w.ssid.clone()).collect();
    let mut keep = vec![true; v.len()];
    for (i, w) in v.clone().iter().enumerate() {
        if let Some(open) = owe_twin_of(&w.ssid, names.iter().map(String::as_str)) {
            keep[i] = false;
            if w.in_use {
                if let Some(o) = v.iter_mut().find(|x| x.ssid == open) {
                    o.in_use = true;
                    o.signal = o.signal.max(w.signal);
                }
            }
        }
    }
    let mut k = keep.into_iter();
    v.retain(|_| k.next().unwrap_or(true));
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetDevice {
    pub device: String,
    pub kind: String,
    pub state: String,
    pub connection: String,
}

/// Whether a wired adapter has a cable in: NetworkManager calls it "unavailable" without one (and leaves alone one it does
/// not manage).
pub fn cable_in(state: &str) -> bool {
    !(state.starts_with("unavailable") || state.starts_with("unmanaged") || state.is_empty())
}

/// `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device`, Ethernet and Wi-Fi only.
pub fn net_devices(text: &str) -> Vec<NetDevice> {
    text.lines()
        .map(nm_fields)
        .filter(|f| f.len() >= 4 && (f[1] == "ethernet" || f[1] == "wifi"))
        .map(|f| NetDevice { device: f[0].clone(), kind: f[1].clone(), state: f[2].clone(), connection: f[3].clone() })
        .collect()
}

/// The adapters, whether Wi-Fi is switched on, and the networks in range: what a page about joining a network needs.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub wifi_on: bool,
    pub devices: Vec<NetDevice>,
    pub networks: Vec<Wifi>,
}

impl Snapshot {
    /// Read through `nmcli` (given its arguments, its output or None), read-only.
    pub fn read(nmcli: impl Fn(&[&str]) -> Option<String>) -> Snapshot {
        Snapshot {
            wifi_on: nmcli(&["radio", "wifi"]).is_some_and(|s| s.trim() == "enabled"),
            devices: net_devices(&nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).unwrap_or_default()),
            networks: wifi_list(&nmcli(&["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"]).unwrap_or_default()),
        }
    }

    /// The adapters and the Wi-Fi switch only, without the list of networks (which can wait for a scan): quick enough to ask
    /// while a window waits for the answer.
    pub fn adapters(nmcli: impl Fn(&[&str]) -> Option<String>) -> Snapshot {
        Snapshot {
            wifi_on: nmcli(&["radio", "wifi"]).is_some_and(|s| s.trim() == "enabled"),
            devices: net_devices(&nmcli(&["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).unwrap_or_default()),
            networks: vec![],
        }
    }

    /// A wired adapter that is connected.
    pub fn wired_up(&self) -> bool {
        self.devices.iter().any(|d| d.kind == "ethernet" && d.state.starts_with("connected"))
    }

    /// A wired adapter with a cable in (connected or not yet).
    pub fn cable(&self) -> bool {
        self.devices.iter().any(|d| d.kind == "ethernet" && cable_in(&d.state))
    }

    /// Whether the computer has a Wi-Fi adapter at all.
    pub fn has_wifi(&self) -> bool {
        self.devices.iter().any(|d| d.kind == "wifi")
    }

    /// The Wi-Fi network the computer is on: the one marked in use, else the connected adapter's profile name.
    pub fn wifi_up(&self) -> Option<String> {
        let dev = self.devices.iter().find(|d| d.kind == "wifi" && d.state.starts_with("connected"))?;
        Some(self.networks.iter().find(|w| w.in_use).map(|w| w.ssid.clone()).unwrap_or_else(|| dev.connection.clone()))
    }
}

/// `nmcli ARGS`: its output, or None when it is missing or failed. Only for reading; nothing secret goes on its command line.
pub fn nmcli(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("nmcli").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wifi_parser_keeps_strongest_and_in_use_first() {
        let t = ":Cafe:40:WPA2\n*:Home\\:5G:70:WPA2\n:Cafe:80:WPA2\n:Open:30:\n::90:WPA2\n";
        let w = wifi_list(t);
        assert_eq!(w.iter().map(|x| x.ssid.as_str()).collect::<Vec<_>>(), vec!["Home:5G", "Cafe", "Open"]);
        assert!(w[0].in_use && w[0].secured);
        assert_eq!(w[1].signal, 80);
        assert!(!w[2].secured);
        // an Enhanced Open twin is folded into its open network, which shows as the one in use
        let t = wifi_list("*:_owetm_Shop Gue2770611103:80:OWE\n:Shop Guest:70:OWE-TM\n:Cafe:40:WPA2\n");
        assert_eq!(t.iter().map(|x| (x.ssid.as_str(), x.in_use)).collect::<Vec<_>>(), vec![("Shop Guest", true), ("Cafe", false)]);
        assert_eq!(owe_twin_of("_owetm_Shop Gue27", ["Cafe", "Shop Guest"]), Some("Shop Guest"));
        assert_eq!(owe_twin_of("Shop Guest", ["Shop Guest"]), None);
        // Enhanced Open (a guest network): no password
        assert!(!nm_secured("OWE") && !nm_secured("OWE-TM") && !nm_secured("--"));
        assert!(nm_secured("WPA2") && nm_secured("WPA2 802.1X") && nm_secured("WPA2 WPA3"));
    }

    #[test]
    fn net_devices_parser() {
        let d =
            net_devices("eth0:ethernet:connected:Wired connection 1\nlo:loopback:connected (externally):lo\nwlan0:wifi:disconnected:\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].connection, "Wired connection 1");
        assert_eq!(d[1].kind, "wifi");
        // Wired shows only with a cable in
        assert!(cable_in("connected") && cable_in("disconnected") && cable_in("connecting (getting IP configuration)"));
        assert!(!cable_in("unavailable") && !cable_in("unmanaged"));
    }

    /// A made-up nmcli: the answers are given, nothing is run.
    fn fake(radio: &'static str, devices: &'static str, wifi: &'static str) -> impl Fn(&[&str]) -> Option<String> {
        move |a: &[&str]| match a {
            ["radio", "wifi"] => Some(radio.to_string()),
            [.., "device"] => Some(devices.to_string()),
            [.., "wifi", "list"] => Some(wifi.to_string()),
            _ => None,
        }
    }

    #[test]
    fn a_snapshot_says_what_is_connected() {
        let s = Snapshot::read(fake("enabled\n", "eth0:ethernet:connected:Wired 1\nwlan0:wifi:disconnected:\n", ":Example-5G:70:WPA2\n"));
        assert!(s.wifi_on && s.wired_up() && s.cable() && s.has_wifi());
        assert_eq!(s.wifi_up(), None);
        assert_eq!(s.networks.len(), 1);
        let s =
            Snapshot::read(fake("enabled\n", "eth0:ethernet:unavailable:\nwlan0:wifi:connected:Example-5G\n", "*:Example-5G:70:WPA2\n"));
        assert!(!s.wired_up() && !s.cable());
        assert_eq!(s.wifi_up().as_deref(), Some("Example-5G"));
        let s = Snapshot::read(fake("disabled\n", "eth0:ethernet:connecting (getting IP configuration):Wired 1\n", ""));
        assert!(!s.wifi_on && !s.wired_up() && s.cable() && !s.has_wifi());
        let a = Snapshot::adapters(fake("enabled\n", "eth0:ethernet:connected:Wired 1\n", ":Example-5G:70:WPA2\n"));
        assert!(a.wired_up() && a.networks.is_empty(), "the adapters alone: no scan list asked for");
        let s = Snapshot::read(|_: &[&str]| None);
        assert_eq!(s, Snapshot::default(), "no nmcli: nothing");
    }
}
