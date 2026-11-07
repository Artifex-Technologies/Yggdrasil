//! Wi-Fi & Network > Devices: who is on this network, from Heimdall's device list (`GET /v1/lan/devices`, Heimdall's
//! docs/DEVICES.md). Heimdall looks every 5 minutes, only on networks the owner trusted; Settings shows the list, lets the
//! owner rename a device (kept here, per user) and asks for a scan now.

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
    /// "scanned", "not_trusted", "router_changed", "offline", "waiting" (no scan yet), or "unavailable" (Heimdall not running or too old)
    pub status: String,
    pub ssid: String,
    /// seconds since 1970 (UTC)
    pub last_scan: i64,
    pub devices: Vec<Device>,
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
    devices.sort_by_key(|d| (order(&d.kind), d.name.is_empty(), d.name.to_lowercase(), ip_key(&d.ip)));
    List { status, ssid: n.ssid, last_scan: crate::security::parse_time(&n.last_scan).unwrap_or(0), devices }
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

/// Reads Heimdall's list for connection `uuid` (a local call, quick; empty when Heimdall is not there).
pub fn load(uuid: &str) -> List {
    let api = yggdrasil_conf::integrations::security().and_then(|i| i.loopback("api"));
    let body = api.and_then(|a| crate::sys::run("curl", &["-s", "-m", "3", "--proto", "=http", &format!("http://{a}/v1/lan/devices")]));
    match body {
        Some(b) if b.trim_start().starts_with('{') => parse(&b, uuid),
        _ => List { status: "unavailable".into(), ..Default::default() },
    }
}

/// Starts Heimdall's scan and waits for it (a few seconds). A polkit rule lets an administrator start this one unit.
pub fn scan_now() -> Result<(), String> {
    crate::sys::act("systemctl", &["start", "heimdall-lan-scan.service"])
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
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).unwrap_or_else(|| {
        std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
    });
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
    match d.kind.as_str() {
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
    kind_name(&d.kind).into()
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
