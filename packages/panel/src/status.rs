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

/// The desktops from Hyprland's `hyprctl -j workspaces` and `-j activeworkspace`: the numbered workspaces that exist (they do while
/// they hold a window or are shown) and the one in front, in order, and the position of the one in front. The hidden ones
/// ("special:minimized") are not desktops.
pub fn parse_desktops(workspaces: &str, active: &str) -> (Vec<Desktop>, usize) {
    let mut ids: Vec<i64> = serde_json::from_str::<Value>(workspaces)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|w| w["id"].as_i64())
        .filter(|&id| id >= 1)
        .collect();
    let current = serde_json::from_str::<Value>(active).ok().and_then(|v| v["id"].as_i64()).filter(|&id| id >= 1);
    ids.extend(current);
    ids.sort_unstable();
    ids.dedup();
    let at = current.and_then(|c| ids.iter().position(|&id| id == c)).unwrap_or(0);
    (ids.iter().map(|id| Desktop { id: id.to_string(), name: format!("Desktop {id}") }).collect(), at)
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
    let (desktops, current) = parse_desktops(&out("hyprctl", &["-j", "workspaces"]), &out("hyprctl", &["-j", "activeworkspace"]));
    let net = parse_net(&out("nmcli", &["-t", "-f", "TYPE,STATE", "device"]), &out("nmcli", &["-t", "-f", "IN-USE,SIGNAL", "dev", "wifi"]));
    let bt = parse_bt(&out("timeout", &["2", "bluetoothctl", "show"]), &out("timeout", &["2", "bluetoothctl", "devices", "Connected"]));
    Status { desktops, current, net, bt }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn desktops_and_the_current_one_are_read() {
        let ws = r#"[{"id":3,"name":"3","windows":1},{"id":1,"name":"1","windows":3},{"id":-98,"name":"special:minimized","windows":1}]"#;
        let (d, at) = parse_desktops(ws, r#"{"id":2,"name":"2"}"#);
        assert_eq!(d.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["1", "2", "3"], "the shown one is a desktop even while empty; hidden ones are not");
        assert_eq!((d[1].name.as_str(), at), ("Desktop 2", 1));
        assert_eq!(parse_desktops("nonsense", "").0.len(), 0);
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
