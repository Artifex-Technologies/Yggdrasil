//! Wi-Fi & Network > Devices: who is on this network, from the security monitor's device list (the `security` integration:
//! `GET /v1/lan/devices` on its `api`, and `devices_scan` to look again now; docs/INTEGRATIONS.md). Settings shows the list,
//! lets the owner rename a device (kept here, per user), say what it is when the list's guess is wrong (kept by the monitor:
//! the integration's `devices_kind`, run through pkexec with the hardware address and the kind, comes back as `owner_kind`)
//! and asks for a look now. Without `devices_scan` there is no page.
//!
//! On a network the monitor does not keep a list for (any network but home), the integration's `devices_look` gives one look
//! when the network's page opens (HEI-74, the owner's decision of 2026-11-22): run as the owner, it listens for a few seconds,
//! sends nothing and keeps nothing, and prints what it heard. Settings keeps that only while the page is open.

use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Device {
    #[serde(default)]
    pub mac: String,
    #[serde(default)]
    pub ip: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub vendor: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub kind: String,
    /// What the owner said it is, over the guess in `kind` (empty: not said).
    #[serde(default)]
    pub owner_kind: String,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub private_mac: bool,
    #[serde(default)]
    pub first_seen: String,
    #[serde(default)]
    pub last_seen: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Net {
    #[serde(default)]
    uuid: String,
    #[serde(default)]
    ssid: String,
    #[serde(default)]
    last_scan: String,
    #[serde(default)]
    devices: Vec<Device>,
}

#[derive(Clone, Debug, Default, Deserialize)]
struct Store {
    #[serde(default)]
    networks: Vec<Net>,
    #[serde(default)]
    status: String,
    #[serde(default)]
    status_uuid: String,
}

/// The list for one connection, as Settings shows it.
#[derive(Clone, Debug, Default)]
pub struct List {
    /// the security monitor's name (the integration's `name`)
    pub monitor: String,
    /// "scanned", "not_trusted", "router_changed", "offline", "waiting" (no scan yet), or "unavailable" (the monitor is not running or too old)
    pub status: String,
    pub ssid: String,
    /// seconds since 1970 (UTC)
    pub last_scan: i64,
    pub devices: Vec<Device>,
    /// A look's multicast DNS: "listened", "not_member" (this computer is not in the group there, so nothing was heard),
    /// "busy" (another program holds the port alone); empty for a kept list.
    pub mdns: String,
    /// How long a look listened, in seconds.
    pub listened: u64,
    /// Why a look did not run ("look_failed").
    pub error: String,
}

/// Parses the API's answer for connection `uuid`. The status is the last scan's; it speaks for this connection only if the
/// last scan was on it.
pub fn parse(body: &str, uuid: &str) -> List {
    let Ok(s) = serde_json::from_str::<Store>(body) else { return List { status: "unavailable".into(), ..Default::default() } };
    let n = s.networks.into_iter().find(|n| n.uuid == uuid);
    let status = if s.status_uuid == uuid && !s.status.is_empty() {
        s.status
    } else if n.is_some() {
        "scanned".into()
    } else if s.status.is_empty() {
        "waiting".into() // the first scan has not run yet
    } else {
        "not_trusted".into()
    };
    let Some(n) = n else { return List { status, ..Default::default() } };
    let mut devices = n.devices;
    devices.sort_by_key(|d| (order(shown_kind(d)), d.name.is_empty(), d.name.to_lowercase(), ip_key(&d.ip)));
    List { status, ssid: n.ssid, last_scan: crate::security::parse_time(&n.last_scan).unwrap_or(0), devices, ..Default::default() }
}

fn order(kind: &str) -> u8 {
    match kind {
        "router" => 0,
        "this" => 1,
        _ => 2,
    }
}

fn ip_key(ip: &str) -> Vec<u32> {
    ip.split('.').filter_map(|p| p.parse().ok()).collect()
}

/// The security integration, when it keeps a device list (it declares `devices_scan`).
fn monitor() -> Option<&'static yggdrasil_conf::integrations::Integration> {
    yggdrasil_conf::integrations::security().filter(|i| i.command("devices_scan").is_some())
}

/// Reads the list for connection `uuid` (a local call, quick). None when no security monitor keeps one.
pub fn load(uuid: &str) -> Option<List> {
    let i = monitor()?;
    let name = i.get("name").unwrap_or("The security monitor").to_string();
    let body = i
        .loopback("api")
        .and_then(|a| crate::sys::run("curl", &["-s", "-m", "3", "--proto", "=http", &format!("http://{a}/v1/lan/devices")]));
    let mut l = match body {
        Some(b) if b.trim_start().starts_with('{') => parse(&b, uuid),
        _ => List { status: "unavailable".into(), ..Default::default() },
    };
    l.monitor = name;
    Some(l)
}

/// Asks the monitor to look again now and waits for it (a few seconds): the integration's `devices_scan` command.
pub fn scan_now() -> Result<(), String> {
    let cmd = monitor().and_then(|i| i.command("devices_scan")).ok_or("there is no device list")?;
    let out = std::process::Command::new(&cmd[0]).args(&cmd[1..]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// A look's answer (`devices_look`).
#[derive(Clone, Debug, Default, Deserialize)]
struct Heard {
    #[serde(default)]
    status: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    mdns: String,
    #[serde(default)]
    listened: u64,
    #[serde(default)]
    devices: Vec<Device>,
}

/// How long a look listens.
pub const LOOK_SECONDS: u64 = 8;

/// Whether the monitor can look at a network it keeps no list for (the integration declares `devices_look`).
pub fn can_look() -> bool {
    monitor().and_then(|i| i.command("devices_look")).is_some()
}

/// The security monitor's name, for a list that is still coming.
pub fn monitor_name() -> String {
    monitor().and_then(|i| i.get("name")).unwrap_or("The security monitor").to_string()
}

/// The look's command line: the integration's `devices_look`, the wait, and the connection. None for a connection id that
/// is not a NetworkManager UUID (hex digits and dashes).
pub fn look_args(cmd: &[String], uuid: &str) -> Option<Vec<String>> {
    let ok = !uuid.is_empty() && uuid.len() <= 64 && uuid.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-');
    (ok && !cmd.is_empty()).then(|| cmd.iter().cloned().chain(["--wait".into(), LOOK_SECONDS.to_string(), uuid.to_string()]).collect())
}

/// A look's answer as a list: status "looked" (every device heard now, at `now`), "offline", or "unavailable" (not an
/// answer). Sorted as the kept lists are.
pub fn parse_look(body: &str, now: i64) -> List {
    let Ok(h) = serde_json::from_str::<Heard>(body.trim()) else { return List { status: "unavailable".into(), ..Default::default() } };
    let status = if h.status == "looked" { "looked" } else { "offline" };
    let mut devices = h.devices;
    devices.sort_by_key(|d| (order(shown_kind(d)), d.name.is_empty(), d.name.to_lowercase(), ip_key(&d.ip)));
    List { status: status.into(), ssid: h.name, last_scan: now, devices, mdns: h.mdns, listened: h.listened, ..Default::default() }
}

/// One look at connection `uuid`, waiting for it (about LOOK_SECONDS).
pub fn look(uuid: &str, now: i64) -> Result<List, String> {
    let cmd = monitor().and_then(|i| i.command("devices_look")).ok_or("The security monitor cannot look at this network.")?;
    let args = look_args(&cmd, uuid).ok_or("Not a connection the security monitor can look at.")?;
    let out =
        std::process::Command::new(&args[0]).args(&args[1..]).stdin(std::process::Stdio::null()).output().map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    let mut l = parse_look(&String::from_utf8_lossy(&out.stdout), now);
    if l.status == "unavailable" {
        return Err("The security monitor is too old to look at this network.".into());
    }
    l.monitor = monitor_name();
    Ok(l)
}

/// What a device in a list is known by: its hardware address, or its address when the list heard it without one.
pub fn key(d: &Device) -> String {
    if d.mac.is_empty() {
        d.ip.clone()
    } else {
        d.mac.clone()
    }
}

/// The kinds the owner may give a device, in the order the choice lists them.
pub const OWNER_KINDS: [&str; 9] = ["tv", "phone", "computer", "console", "speaker", "printer", "smart", "router", "other"];

/// What the device is as shown: the owner's word when it is one of OWNER_KINDS, else the list's guess.
pub fn shown_kind(d: &Device) -> &str {
    if OWNER_KINDS.contains(&d.owner_kind.as_str()) {
        &d.owner_kind
    } else {
        &d.kind
    }
}

/// Whether the monitor can keep the owner's word for what a device is (the integration declares `devices_kind`).
pub fn can_set_kind() -> bool {
    monitor().and_then(|i| i.command("devices_kind")).is_some()
}

/// The arguments for pkexec: the integration's `devices_kind` command, the hardware address and the kind ("guess" takes the
/// owner's word away). None for a kind that is not one of OWNER_KINDS or "guess", or an address that is not six pairs of hex
/// digits with colons.
pub fn kind_args(cmd: &[String], mac: &str, kind: &str) -> Option<Vec<String>> {
    let b = mac.as_bytes();
    let mac_ok = b.len() == 17 && b.iter().enumerate().all(|(i, c)| if i % 3 == 2 { *c == b':' } else { c.is_ascii_hexdigit() });
    let kind_ok = kind == "guess" || OWNER_KINDS.contains(&kind);
    (mac_ok && kind_ok && !cmd.is_empty()).then(|| cmd.iter().cloned().chain([mac.to_ascii_uppercase(), kind.to_string()]).collect())
}

/// Tells the monitor what a device is: its `devices_kind` command through pkexec (polkit asks for the administrator's
/// password). The list is read again afterwards.
pub fn set_kind(mac: &str, kind: &str) -> Result<(), String> {
    let cmd = monitor().and_then(|i| i.command("devices_kind")).ok_or("The security monitor cannot keep what a device is.")?;
    let args = kind_args(&cmd, mac, kind).ok_or("Not a device or a kind the security monitor takes.")?;
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::sys::act("pkexec", &refs).map_err(|e| {
        if e.is_empty() || e.contains("Not authorized") || e.contains("dismissed") {
            "Not changed: the administrator's password was not given.".to_string()
        } else {
            e
        }
    })
}

/// What a kind is called, for people.
pub fn kind_name(kind: &str) -> &'static str {
    match kind {
        "router" => "Router",
        "this" => "This computer",
        "tv" => "TV or streaming stick",
        "speaker" => "Speaker",
        "printer" => "Printer",
        "phone" => "Phone or tablet",
        "computer" => "Computer",
        "console" => "Game console",
        "smart" => "Smart-home device",
        "private" => "Phone, tablet or laptop",
        _ => "Device",
    }
}

/// What a service type means, for people ("_googlecast._tcp" -> "Chromecast"); unknown ones are left out.
pub fn service_name(s: &str) -> Option<&'static str> {
    Some(match s {
        "_googlecast._tcp" => "Chromecast",
        "_airplay._tcp" => "AirPlay",
        "_raop._tcp" => "AirPlay audio",
        "_amzn-wplay._tcp" => "Fire TV casting",
        "_spotify-connect._tcp" => "Spotify Connect",
        "_ipp._tcp" | "_ipps._tcp" | "_printer._tcp" | "_pdl-datastream._tcp" => "Printing",
        "_hap._tcp" => "HomeKit",
        "_matter._tcp" | "_matterd._udp" | "_matterc._udp" => "Matter",
        "_smb._tcp" => "File sharing",
        "_ssh._tcp" => "Remote login (SSH)",
        "_http._tcp" => "Web page",
        "_sonos._tcp" => "Sonos",
        "_companion-link._tcp" => "Apple device",
        "_androidtvremote2._tcp" => "Android TV remote",
        "_meshcop._udp" => "Thread border router",
        "_workstation._tcp" => "Computer",
        _ => return None,
    })
}

/// "3 min ago", "2 h ago", "yesterday", "12 days ago".
pub fn ago(now: i64, t: i64) -> String {
    let s = (now - t).max(0);
    match s {
        0..=89 => "just now".into(),
        90..=3599 => format!("{} min ago", s / 60),
        3600..=86_399 => format!("{} h ago", s / 3600),
        86_400..=172_799 => "yesterday".into(),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// The owner's own names for devices, by hardware address (~/.config/yggdrasil/device-names: "MAC=name" lines).
pub fn names_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("yggdrasil/device-names")
}

pub fn parse_names(text: &str) -> BTreeMap<String, String> {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .map(|(m, n)| (m.trim().to_uppercase(), n.trim().to_string()))
        .filter(|(m, n)| m.len() == 17 && !n.is_empty())
        .collect()
}

pub fn read_names() -> BTreeMap<String, String> {
    parse_names(&std::fs::read_to_string(names_path()).unwrap_or_default())
}

pub fn write_names(names: &BTreeMap<String, String>) -> Result<(), String> {
    let p = names_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let text: String = names.iter().map(|(m, n)| format!("{m}={}\n", n.replace(['\n', '='], " "))).collect();
    std::fs::write(&p, text).map_err(|e| e.to_string())
}

/// The name to show: the owner's own, then what the device or the router said, then the maker, then the kind.
pub fn shown_name(d: &Device, own: &BTreeMap<String, String>) -> String {
    if let Some(n) = own.get(&d.mac.to_uppercase()) {
        return n.clone();
    }
    match shown_kind(d) {
        "router" => return "Router".into(),
        "this" => return "This computer".into(),
        _ => {}
    }
    if !d.name.is_empty() {
        return d.name.clone();
    }
    if !d.vendor.is_empty() {
        return format!("{} device", d.vendor);
    }
    kind_name(shown_kind(d)).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"networks":[{"uuid":"u1","ssid":"Home","last_scan":"2026-11-07T12:00:00Z","devices":[
        {"mac":"00:11:22:33:44:55","ip":"192.168.1.25","name":"Living Room","vendor":"Amazon","kind":"tv","services":["_amzn-wplay._tcp"],"first_seen":"2026-11-07T11:00:00Z","last_seen":"2026-11-07T12:00:00Z"},
        {"mac":"00:11:22:33:44:01","ip":"192.168.1.1","name":"RTR","kind":"router","first_seen":"2026-11-07T11:00:00Z","last_seen":"2026-11-07T12:00:00Z"},
        {"mac":"DA:00:00:00:00:01","ip":"192.168.1.9","kind":"private","private_mac":true,"first_seen":"2026-11-07T11:00:00Z","last_seen":"2026-11-07T11:30:00Z"}
    ]}],"status":"scanned","status_uuid":"u1","status_time":"2026-11-07T12:00:00Z"}"#;

    #[test]
    fn the_list_for_the_connection_router_first() {
        let l = parse(BODY, "u1");
        assert_eq!(l.status, "scanned");
        assert_eq!(l.ssid, "Home");
        assert_eq!(l.devices.len(), 3);
        assert_eq!(l.devices[0].kind, "router");
        assert_eq!(l.devices[1].name, "Living Room", "named devices before unnamed ones");
        assert!(l.last_scan > 0);
        // another connection with no list: not trusted
        assert_eq!(parse(BODY, "u2").status, "not_trusted");
        assert_eq!(parse("not json", "u1").status, "unavailable");
        // the last scan's status on this connection speaks for it
        let b = BODY.replace(r#""status":"scanned""#, r#""status":"router_changed""#);
        assert_eq!(parse(&b, "u1").status, "router_changed");
    }

    #[test]
    fn the_owners_kind_over_the_guess() {
        let b = BODY.replace(r#""kind":"private","private_mac":true"#, r#""kind":"private","owner_kind":"console","private_mac":true"#);
        let l = parse(&b, "u1");
        let d = l.devices.iter().find(|d| d.mac == "DA:00:00:00:00:01").unwrap();
        assert_eq!(shown_kind(d), "console");
        assert_eq!(shown_name(d, &BTreeMap::new()), "Game console");
        let mut odd = d.clone();
        odd.owner_kind = "this".into(); // not a word the owner can give: the guess stands
        assert_eq!(shown_kind(&odd), "private");
        // a device the owner calls a router sorts first, with the router
        let b = BODY.replace(r#""kind":"tv","services""#, r#""kind":"tv","owner_kind":"router","services""#);
        let l = parse(&b, "u1");
        assert_eq!((shown_kind(&l.devices[0]), shown_kind(&l.devices[1])), ("router", "router"));
    }

    #[test]
    fn a_look_heard_now() {
        let body = r#"{"status":"looked","uuid":"0f1e-2d","name":"Cafe","interface":"wlan0","mdns":"listened","listened":8,"devices":[
            {"mac":"","ip":"192.0.2.20","name":"Living TV","names":["Living TV"],"vendor":"","model":"Chromecast","kind":"tv","services":["_googlecast._tcp"],"private_mac":false},
            {"mac":"02:00:00:00:00:01","ip":"192.0.2.1","name":"","names":[],"vendor":"","model":"","kind":"router","services":[],"private_mac":true}]}"#;
        let l = parse_look(body, 1_800_000_000);
        assert_eq!((l.status.as_str(), l.ssid.as_str(), l.mdns.as_str(), l.listened), ("looked", "Cafe", "listened", 8));
        assert_eq!(l.last_scan, 1_800_000_000);
        assert_eq!(l.devices[0].kind, "router", "the router first, as in a kept list");
        assert_eq!(key(&l.devices[1]), "192.0.2.20", "heard without a hardware address: known by its address");
        assert_eq!(key(&l.devices[0]), "02:00:00:00:00:01");
        assert_eq!(parse_look(r#"{"status":"offline","devices":[]}"#, 0).status, "offline");
        assert_eq!(parse_look("usage: heimdalld lan scan|list|kind", 0).status, "unavailable", "an older monitor");
        let cmd = vec!["/usr/bin/heimdalld".to_string(), "lan".into(), "look".into()];
        assert_eq!(
            look_args(&cmd, "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0").unwrap(),
            vec!["/usr/bin/heimdalld", "lan", "look", "--wait", "8", "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"]
        );
        assert_eq!(look_args(&cmd, "--dir"), None);
        assert_eq!(look_args(&cmd, ""), None);
        assert_eq!(look_args(&cmd, "0f1e 2d"), None);
    }

    #[test]
    fn the_kind_command_is_narrow() {
        let cmd = vec!["/usr/bin/heimdalld".to_string(), "lan".into(), "kind".into()];
        assert_eq!(
            kind_args(&cmd, "da:00:00:00:00:01", "tv").unwrap(),
            vec!["/usr/bin/heimdalld", "lan", "kind", "DA:00:00:00:00:01", "tv"]
        );
        assert_eq!(kind_args(&cmd, "DA:00:00:00:00:01", "guess").unwrap().last().unwrap(), "guess");
        assert_eq!(kind_args(&cmd, "DA:00:00:00:00:01", "private"), None);
        assert_eq!(kind_args(&cmd, "DA-00-00-00-00-01", "tv"), None);
        assert_eq!(kind_args(&cmd, "DA:00:00:00:00:01 --dir", "tv"), None);
        assert_eq!(kind_args(&cmd, "--dir=/tmp/x/y/z", "tv"), None);
        assert_eq!(kind_args(&[], "DA:00:00:00:00:01", "tv"), None);
    }

    #[test]
    fn names_shown_and_kept() {
        let l = parse(BODY, "u1");
        let mut own = BTreeMap::new();
        assert_eq!(shown_name(&l.devices[0], &own), "Router");
        assert_eq!(shown_name(&l.devices[2], &own), "Phone, tablet or laptop");
        own.insert("DA:00:00:00:00:01".to_string(), "Sam's phone".to_string());
        assert_eq!(shown_name(&l.devices[2], &own), "Sam's phone");
        let text = "da:00:00:00:00:01 = Sam's phone\nbad line\n00:11:22:33:44:55=\n";
        let parsed = parse_names(text);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed["DA:00:00:00:00:01"], "Sam's phone");
        assert_eq!(ago(1000, 1000 - 30), "just now");
        assert_eq!(ago(10_000, 10_000 - 600), "10 min ago");
        assert_eq!(ago(200_000, 200_000 - 7200), "2 h ago");
        assert_eq!(service_name("_googlecast._tcp"), Some("Chromecast"));
        assert_eq!(service_name("_weird._tcp"), None);
    }
}
