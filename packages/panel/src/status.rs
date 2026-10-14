//! What the top bar shows besides the clock: the desktops (workspaces), the network and Bluetooth.
//! Read with the system's own tools every couple of seconds on a worker thread, so the bar never
//! waits on them. The parsing is separate from the commands so it can be tested.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Net {
    /// No connection.
    #[default]
    Off,
    /// Wi-Fi, with its signal strength in percent.
    Wifi(u8),
    Wired,
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
    std::process::Command::new(cmd).args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// The desktops from `busctl --json=short get-property ... desktops` (type a(uss): position, id, name).
pub fn parse_desktops(json: &str) -> Vec<Desktop> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    let list = match &v["data"] {
        Value::Array(a) if a.len() == 1 && a[0].is_array() => a[0].as_array().cloned().unwrap_or_default(),
        Value::Array(a) => a.clone(),
        _ => vec![],
    };
    list.iter()
        .filter_map(|e| {
            let e = e.as_array()?;
            Some(Desktop { id: e.get(1)?.as_str()?.to_string(), name: e.get(2)?.as_str()?.to_string() })
        })
        .collect()
}

/// The id of the current desktop from `busctl --json=short get-property ... current` (type s).
pub fn parse_current(json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json).ok()?;
    v["data"].as_str().or_else(|| v["data"][0].as_str()).map(str::to_string)
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

pub fn poll() -> Status {
    let desktops = parse_desktops(&out("busctl", &["--user", "--json=short", "get-property", "org.kde.KWin", "/VirtualDesktopManager", "org.kde.KWin.VirtualDesktopManager", "desktops"]));
    let current_id = parse_current(&out("busctl", &["--user", "--json=short", "get-property", "org.kde.KWin", "/VirtualDesktopManager", "org.kde.KWin.VirtualDesktopManager", "current"]));
    let current = current_id.and_then(|id| desktops.iter().position(|d| d.id == id)).unwrap_or(0);
    let net = parse_net(&out("nmcli", &["-t", "-f", "TYPE,STATE", "device"]), &out("nmcli", &["-t", "-f", "IN-USE,SIGNAL", "dev", "wifi"]));
    let bt = parse_bt(&out("timeout", &["2", "bluetoothctl", "show"]), &out("timeout", &["2", "bluetoothctl", "devices", "Connected"]));
    Status { desktops, current, net, bt }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_and_the_current_one_are_read() {
        let d = parse_desktops(r#"{"type":"a(uss)","data":[[[0,"aaa","Desktop 1"],[1,"bbb","Desktop 2"]]]}"#);
        assert_eq!(d.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["aaa", "bbb"]);
        assert_eq!(d[1].name, "Desktop 2");
        assert_eq!(parse_current(r#"{"type":"s","data":"bbb"}"#).as_deref(), Some("bbb"));
        assert!(parse_desktops("nonsense").is_empty());
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
