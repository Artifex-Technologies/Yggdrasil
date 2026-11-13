//! What the top bar shows besides the clock: the desktops (workspaces), the network and Bluetooth.
//! Read on a worker thread when they change, so the bar never waits on them and nothing is read while nothing happens: the
//! desktops from the compositor (yggdrasil-wm) after its events say they moved, the network and Bluetooth with the system's own
//! tools after NetworkManager or BlueZ signal a change on the system bus. The parsing is separate from the commands so it can be
//! tested.

use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Net {
    /// No connection.
    #[default]
    Off,
    /// Wi-Fi, with its signal strength in percent.
    Wifi(u8),
    Wired,
}

impl Net {
    /// The bar's icon for it (the signal shows in four steps).
    pub fn icon(self) -> &'static str {
        match self {
            Net::Off => "network-offline-symbolic",
            Net::Wired => "network-wired-symbolic",
            Net::Wifi(sig) if sig >= 75 => "network-wireless-signal-excellent-symbolic",
            Net::Wifi(sig) if sig >= 50 => "network-wireless-signal-good-symbolic",
            Net::Wifi(sig) if sig >= 25 => "network-wireless-signal-ok-symbolic",
            Net::Wifi(_) => "network-wireless-signal-weak-symbolic",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Bt {
    /// No Bluetooth adapter.
    #[default]
    Missing,
    Off,
    On,
    Connected,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Desktop {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Status {
    pub desktops: Vec<Desktop>,
    pub current: usize,
    pub net: Net,
    pub bt: Bt,
}

/// The most desktops the "+" will make.
pub const MAX_DESKTOPS: usize = 9;

fn out(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

/// The desktops the compositor has (the numbered workspaces that hold a window or are shown), in order, and the position of the
/// one in front.
pub fn desktops(d: Option<&yggdrasil_wm::Desk>) -> (Vec<Desktop>, usize) {
    let found = d.map(|d| d.desktops()).unwrap_or_default();
    (found.ids.iter().map(|id| Desktop { id: id.to_string(), name: format!("Desktop {id}") }).collect(), found.at)
}

/// `nmcli -t -f TYPE,STATE device` and `nmcli -t -f IN-USE,SIGNAL dev wifi`.
pub fn parse_net(devices: &str, wifi: &str) -> Net {
    let up = |t: &str| devices.lines().any(|l| l.trim() == format!("{t}:connected"));
    if up("wifi") {
        let signal = wifi.lines().find_map(|l| l.strip_prefix("*:")).and_then(|s| s.trim().parse::<u8>().ok()).unwrap_or(60);
        Net::Wifi(signal)
    } else if up("ethernet") {
        Net::Wired
    } else {
        Net::Off
    }
}

/// `bluetoothctl show` (empty when there is no adapter) and `bluetoothctl devices Connected`.
pub fn parse_bt(show: &str, connected: &str) -> Bt {
    if show.trim().is_empty() || !show.contains("Controller") {
        Bt::Missing
    } else if !show.lines().any(|l| l.trim() == "Powered: yes") {
        Bt::Off
    } else if connected.lines().any(|l| l.trim_start().starts_with("Device ")) {
        Bt::Connected
    } else {
        Bt::On
    }
}

fn read_net() -> Net {
    parse_net(&out("nmcli", &["-t", "-f", "TYPE,STATE", "device"]), &out("nmcli", &["-t", "-f", "IN-USE,SIGNAL", "dev", "wifi"]))
}

fn read_bt() -> Bt {
    parse_bt(&out("timeout", &["2", "bluetoothctl", "show"]), &out("timeout", &["2", "bluetoothctl", "devices", "Connected"]))
}

/// What a change asks to be read again.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Parts {
    pub desktops: bool,
    pub net: bool,
    pub bt: bool,
    /// The Wi-Fi in use has this signal strength now (from NetworkManager's signal itself: nothing to run).
    pub signal: Option<u8>,
}

impl Parts {
    const ALL: Parts = Parts { desktops: true, net: true, bt: true, signal: None };
    fn add(&mut self, o: Parts) {
        self.desktops |= o.desktops;
        self.net |= o.net;
        self.bt |= o.bt;
        self.signal = o.signal.or(self.signal);
    }
}

/// Whether two readings look the same in the bar (the Wi-Fi signal shows in four steps).
fn looks_same(a: &Status, b: &Status) -> bool {
    a.desktops == b.desktops && a.current == b.current && a.bt == b.bt && a.net.icon() == b.net.icon()
}

/// A burst of signals (NetworkManager says several things as a connection comes up) is read once it has been quiet this long,
/// and at least this often while it goes on.
const QUIET: Duration = Duration::from_millis(150);
const LONGEST: Duration = Duration::from_secs(1);

/// Reads the desktops, the network and Bluetooth at the start, and again only when something says they changed; sends each
/// reading that differs from the one before. A network that wants a sign-in first (a shop's guest Wi-Fi) gets one notification
/// with a Sign in button, and is preferred until it is online, so the portal's drop after the terms does not end on another
/// network.
pub fn spawn(tx: calloop::channel::Sender<Status>) {
    let (wake, woken) = mpsc::channel::<Parts>();
    {
        let wake = wake.clone();
        std::thread::spawn(move || watch_system_bus(wake));
    }
    std::thread::spawn(move || watch_compositor(wake));
    std::thread::spawn(move || {
        let desk = yggdrasil_wm::Desk::detect();
        let mut now = Status::default();
        let mut connectivity = String::new();
        let mut signing_in: Option<String> = None;
        let mut parts = Parts::ALL;
        let mut sent: Option<Status> = None;
        loop {
            if parts.desktops {
                (now.desktops, now.current) = desktops(desk.as_ref());
            }
            if parts.net {
                now.net = read_net();
                let c = connectivity_now();
                if portal_appeared(&connectivity, &c) {
                    let active = out("nmcli", &["-t", "-f", "UUID,TYPE", "connection", "show", "--active"]);
                    if let Some(u) = active_wifi(&active) {
                        prefer_while_signing_in(&u, true);
                        signing_in = Some(u);
                    }
                    ask_portal_sign_in();
                }
                if c == "full" {
                    if let Some(u) = signing_in.take() {
                        prefer_while_signing_in(&u, false);
                    }
                }
                connectivity = c;
            }
            if parts.bt {
                now.bt = read_bt();
            }
            if let (Some(sig), false, Net::Wifi(_)) = (parts.signal, parts.net, now.net) {
                now.net = Net::Wifi(sig);
            }
            if !sent.as_ref().is_some_and(|s| looks_same(s, &now)) {
                if tx.send(now.clone()).is_err() {
                    return;
                }
                sent = Some(now.clone());
            }
            // wait for the next change, then for its burst to settle
            let Ok(first) = woken.recv() else { return };
            parts = first;
            let until = Instant::now() + LONGEST;
            while let Ok(p) = woken.recv_timeout(QUIET) {
                parts.add(p);
                if Instant::now() >= until {
                    break;
                }
            }
        }
    });
}

/// The system bus's signals from NetworkManager and BlueZ (and either of them starting or stopping), as the parts to read again.
/// Blocks on the bus; besides the Wi-Fi in use (asked once, then followed through the signals), nothing is asked of it.
fn watch_system_bus(wake: mpsc::Sender<Parts>) {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedObjectPath, OwnedValue};
    let Ok(conn) = crate::bus::system().and_then(|b| b.build()) else { return };
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
    let rules = [
        "type='signal',sender='org.freedesktop.NetworkManager'",
        "type='signal',sender='org.bluez'",
        "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.freedesktop.NetworkManager'",
        "type='signal',sender='org.freedesktop.DBus',member='NameOwnerChanged',arg0='org.bluez'",
    ];
    for r in rules {
        let Ok(rule) = zbus::MatchRule::try_from(r) else { return };
        if dbus.add_match_rule(rule).is_err() {
            return;
        }
    }
    // the access point each Wi-Fi device uses: its strength is the bar's signal
    let mut active: HashMap<String, String> = active_access_points(&conn);
    for msg in zbus::blocking::MessageIterator::from(&conn).flatten() {
        let hdr = msg.header();
        if hdr.message_type() != zbus::message::Type::Signal {
            continue;
        }
        let path = hdr.path().map(|p| p.as_str().to_string()).unwrap_or_default();
        let member = hdr.member().map(|m| m.as_str()).unwrap_or("");
        let none = Parts::default();
        let parts = match member {
            "NameOwnerChanged" => match msg.body().deserialize::<(String, String, String)>() {
                Ok((name, _, _)) if name == "org.bluez" => Parts { bt: true, ..none },
                Ok((name, _, _)) if name == "org.freedesktop.NetworkManager" => {
                    active = active_access_points(&conn);
                    Parts { net: true, ..none }
                }
                _ => continue,
            },
            "PropertiesChanged" => {
                let Ok((iface, changed, _)) = msg.body().deserialize::<(String, HashMap<String, OwnedValue>, Vec<String>)>() else {
                    continue;
                };
                if let Some(ap) = changed.get("ActiveAccessPoint").and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok()) {
                    active.insert(path.clone(), ap.to_string());
                }
                let strength = changed.get("Strength").and_then(|v| u8::try_from(v).ok());
                let keys: Vec<&str> = changed.keys().map(String::as_str).collect();
                let used: Vec<&str> = active.values().map(String::as_str).collect();
                signal_parts(&path, &iface, &keys, strength, &used)
            }
            "InterfacesAdded" | "InterfacesRemoved" => {
                let ifaces: Vec<String> = match member {
                    "InterfacesAdded" => msg
                        .body()
                        .deserialize::<(OwnedObjectPath, HashMap<String, HashMap<String, OwnedValue>>)>()
                        .map(|(_, m)| m.into_keys().collect())
                        .unwrap_or_default(),
                    _ => msg.body().deserialize::<(OwnedObjectPath, Vec<String>)>().map(|(_, v)| v).unwrap_or_default(),
                };
                ifaces.iter().map(|i| signal_parts(&path, i, &[], None, &[])).fold(none, |mut a, p| {
                    a.add(p);
                    a
                })
            }
            // NetworkManager's own StateChanged and the like
            _ => signal_parts(&path, "", &[], None, &[]),
        };
        if parts != none && wake.send(parts).is_err() {
            return;
        }
    }
}

/// The access point each Wi-Fi device of NetworkManager's uses now (device path to access point path).
fn active_access_points(conn: &zbus::blocking::Connection) -> std::collections::HashMap<String, String> {
    use zbus::zvariant::OwnedObjectPath;
    let nm = "org.freedesktop.NetworkManager";
    let Ok(p) = zbus::blocking::Proxy::new(conn, nm, "/org/freedesktop/NetworkManager", nm) else { return Default::default() };
    let devices: Vec<OwnedObjectPath> = p.call("GetDevices", &()).unwrap_or_default();
    devices
        .into_iter()
        .filter_map(|d| {
            let w = zbus::blocking::Proxy::new(conn, nm, d.clone(), "org.freedesktop.NetworkManager.Device.Wireless").ok()?;
            let ap: OwnedObjectPath = w.get_property("ActiveAccessPoint").ok()?;
            Some((d.to_string(), ap.to_string()))
        })
        .collect()
}

/// Which parts a signal can change, by the object it is about, its interface and the properties that changed (none: the object
/// came or went, or another signal), given the access points in use. What shows nowhere wakes nothing: counters, bit rates, scans,
/// the strength of networks not in use, Bluetooth devices seen while searching. The strength of the one in use is passed on as
/// it is.
pub fn signal_parts(path: &str, iface: &str, keys: &[&str], strength: Option<u8>, in_use: &[&str]) -> Parts {
    let none = Parts::default();
    let net = Parts { net: true, ..none };
    let bt = Parts { bt: true, ..none };
    if iface.starts_with("org.bluez.") {
        return match iface {
            "org.bluez.Adapter1" if keys.is_empty() || keys.contains(&"Powered") => bt,
            "org.bluez.Device1" if keys.contains(&"Connected") => bt,
            _ => none,
        };
    }
    if path.starts_with("/org/bluez") {
        return none;
    }
    match iface {
        "org.freedesktop.NetworkManager.Device.Statistics" => none,
        "org.freedesktop.NetworkManager.Device.Wireless" if !keys.is_empty() => {
            if keys.contains(&"ActiveAccessPoint") {
                net
            } else {
                none
            }
        }
        "org.freedesktop.NetworkManager.AccessPoint" if !keys.is_empty() => match strength {
            Some(s) if in_use.contains(&path) => Parts { signal: Some(s), ..none },
            _ => none,
        },
        _ if path.starts_with("/org/freedesktop") => net,
        _ => none,
    }
}

/// The compositor's events (yggdrasil-wm): the desktops are read again when one can have moved them. Blocks on the connection;
/// if the compositor restarts, it is waited for and listened to again.
fn watch_compositor(wake: mpsc::Sender<Parts>) {
    use std::io::BufRead;
    let Some(desk) = yggdrasil_wm::Desk::detect() else { return };
    loop {
        if let Some(stream) = desk.events() {
            let mut lines = std::io::BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                // one event per line; a line past 64 KB is not one (and is not kept growing)
                match std::io::Read::take(&mut lines, 64 * 1024).read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) if !line.ends_with('\n') => break,
                    Ok(_) => {
                        if yggdrasil_wm::moves_desktops(line.trim_end())
                            && wake.send(Parts { desktops: true, ..Default::default() }).is_err()
                        {
                            return;
                        }
                    }
                }
            }
        }
        // the compositor went away (or is restarting): try again in a while, and read what it has then
        std::thread::sleep(Duration::from_secs(5));
        if wake.send(Parts { desktops: true, ..Default::default() }).is_err() {
            return;
        }
    }
}

/// NetworkManager's connectivity check (`nmcli -t -f CONNECTIVITY general`): "full", "limited", "portal", "none", "unknown".
fn connectivity_now() -> String {
    out("nmcli", &["-t", "-f", "CONNECTIVITY", "general"]).trim().to_string()
}

/// An address that a sign-in page catches: plain http (a portal cannot redirect https), and NetworkManager's own check, so a
/// network that has already let you in shows only "NetworkManager is online".
pub const PORTAL_PROBE: &str = "http://ping.archlinux.org/nm-check.txt";

/// The sign-in page the network sent the check to (curl's redirect_url), if it is a web address.
pub fn portal_page(redirect: &str) -> Option<&str> {
    let r = redirect.trim();
    ((r.starts_with("https://") || r.starts_with("http://")) && !r.contains(char::is_whitespace)).then_some(r)
}

/// The Wi-Fi connection in use (its NetworkManager id), from `nmcli -t -f UUID,TYPE connection show --active`.
pub fn active_wifi(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.split_once(':').filter(|(_, t)| t.trim() == "802-11-wireless").map(|(u, _)| u.to_string()))
}

/// While a network's sign-in page is open, NetworkManager prefers that network: many portals drop the connection once when the
/// terms are accepted, and the computer then joined another saved network (a phone's hotspot) instead of coming back. Its usual
/// priority returns once it is online (`restore`).
pub fn prefer_while_signing_in(uuid: &str, on: bool) {
    let prio = if on { "100" } else { "0" };
    let _ = std::process::Command::new("nmcli")
        .args(["connection", "modify", uuid, "connection.autoconnect-priority", prio])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Whether this reading is a new sign-in page: the network just started asking (it did not in the reading before).
pub fn portal_appeared(before: &str, now: &str) -> bool {
    now == "portal" && before != "portal"
}

/// A guest or shop network that wants you to sign in or accept its terms first (a captive portal): a notification with a Sign in
/// button that opens the page in the browser, as a phone does. Runs on a thread of its own (it waits for the button).
pub fn ask_portal_sign_in() {
    std::thread::spawn(|| {
        let answer = std::process::Command::new("notify-send")
            .args([
                "-a",
                "Wi-Fi",
                "-i",
                "network-wireless-symbolic",
                "--action=open=Sign in",
                "--wait",
                "Sign in to this Wi-Fi network",
                "It asks you to sign in or accept its terms before it lets you online.",
            ])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        if answer.is_ok_and(|o| String::from_utf8_lossy(&o.stdout).trim() == "open") {
            // the sign-in page's own address, asked of the network: opening the plain-http check in the browser could be upgraded
            // to https first and meet the portal's stand-in certificate ("not private") instead of its page
            let asked = out("curl", &["-s", "-o", "/dev/null", "--max-time", "5", "-w", "%{redirect_url}", PORTAL_PROBE]);
            let page = portal_page(&asked).unwrap_or(PORTAL_PROBE).to_string();
            let _ = std::process::Command::new("xdg-open").arg(page).stdin(std::process::Stdio::null()).spawn();
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_page_is_asked_about_once_when_it_appears() {
        assert!(portal_appeared("full", "portal"));
        assert!(portal_appeared("", "portal"));
        assert!(!portal_appeared("portal", "portal"), "not again while it stays");
        assert!(!portal_appeared("portal", "full"));
        assert_eq!(active_wifi("abc-1:802-11-wireless\nlo-1:loopback\n"), Some("abc-1".into()));
        assert_eq!(active_wifi("lo-1:loopback\n"), None);
        assert_eq!(portal_page("https://guest.example.com/terms?x=1\n"), Some("https://guest.example.com/terms?x=1"));
        assert_eq!(portal_page(""), None);
        assert_eq!(portal_page("javascript:alert(1)"), None);
    }

    #[test]
    fn only_signals_that_show_wake_the_reading() {
        let none = Parts::default();
        let net = Parts { net: true, ..none };
        let bt = Parts { bt: true, ..none };
        let sig = |p: &str, i: &str, k: &[&str]| signal_parts(p, i, k, None, &[]);
        let (ap, other) = ("/org/freedesktop/NetworkManager/AccessPoint/1", "/org/freedesktop/NetworkManager/AccessPoint/650");
        let wifi = "org.freedesktop.NetworkManager.Device.Wireless";
        let dev = "/org/freedesktop/NetworkManager/Devices/3";
        assert_eq!(sig("/org/freedesktop/NetworkManager", "org.freedesktop.NetworkManager", &["Connectivity"]), net);
        assert_eq!(sig(dev, "", &[]), net, "StateChanged");
        // what a Wi-Fi PC's NetworkManager says every few seconds (seen 2026-11-13): none of it is worth a reading
        assert_eq!(sig(dev, wifi, &["Bitrate"]), none);
        assert_eq!(sig(dev, wifi, &["LastScan"]), none);
        assert_eq!(sig(dev, wifi, &["ActiveAccessPoint"]), net, "another network joined");
        let ap_sig = |p: &str, k: &[&str], s| signal_parts(p, "org.freedesktop.NetworkManager.AccessPoint", k, s, &[ap]);
        assert_eq!(ap_sig(ap, &["Strength"], Some(63)), Parts { signal: Some(63), ..none }, "the one in use: its strength, no command");
        assert_eq!(ap_sig(other, &["Strength", "LastSeen"], Some(52)), none, "a network not in use");
        assert_eq!(ap_sig(ap, &["LastSeen"], None), none, "a scan's stamp");
        assert_eq!(sig(dev, "org.freedesktop.NetworkManager.Device.Statistics", &["RxBytes"]), none);
        assert_eq!(sig("/org/bluez/hci0", "org.bluez.Adapter1", &["Powered"]), bt);
        assert_eq!(sig("/org/bluez/hci0", "org.bluez.Adapter1", &["Discovering"]), none);
        assert_eq!(sig("/", "org.bluez.Adapter1", &[]), bt, "an adapter plugged in or out");
        assert_eq!(sig("/org/bluez/hci0/dev_11", "org.bluez.Device1", &["RSSI"]), none, "a device seen while searching");
        assert_eq!(sig("/org/bluez/hci0/dev_11", "org.bluez.Device1", &["Connected"]), bt);
        assert_eq!(sig("/", "org.bluez.Device1", &[]), none, "a device found");
        assert_eq!(sig("/org/bluez/hci0", "", &[]), none);
    }

    #[test]
    fn a_reading_is_sent_when_the_bar_would_change() {
        let a = Status { net: Net::Wifi(63), ..Default::default() };
        assert!(looks_same(&a, &Status { net: Net::Wifi(67), ..a.clone() }), "the same step of the signal");
        assert!(!looks_same(&a, &Status { net: Net::Wifi(80), ..a.clone() }));
        assert!(!looks_same(&a, &Status { bt: Bt::On, ..a.clone() }));
        assert!(!looks_same(&a, &Status { current: 1, ..a.clone() }));
    }

    #[test]
    fn no_compositor_no_desktops() {
        assert_eq!(desktops(None), (vec![], 0));
    }

    #[test]
    fn network_prefers_wifi_with_its_signal() {
        assert_eq!(parse_net("wifi:connected\nethernet:unavailable\n", " :40\n*:78\n"), Net::Wifi(78));
        assert_eq!(parse_net("ethernet:connected\nwifi:disconnected\n", ""), Net::Wired);
        assert_eq!(parse_net("wifi:disconnected\n", ""), Net::Off);
    }

    #[test]
    fn bluetooth_states() {
        let on = "Controller AA:BB (public)\n\tPowered: yes\n";
        assert_eq!(parse_bt("", ""), Bt::Missing);
        assert_eq!(parse_bt("Controller AA:BB\n\tPowered: no\n", ""), Bt::Off);
        assert_eq!(parse_bt(on, ""), Bt::On);
        assert_eq!(parse_bt(on, "Device 11:22 Headphones\n"), Bt::Connected);
    }
}
