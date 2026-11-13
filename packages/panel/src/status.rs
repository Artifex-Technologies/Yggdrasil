//! What the top bar shows besides the clock: the desktops (workspaces), the network and Bluetooth.
//! Read every couple of seconds on a worker thread, so the bar never waits on them: the desktops from the compositor
//! (yggdrasil-wm), the rest with the system's own tools. The parsing is separate from the commands so it can be tested.

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

pub fn poll() -> Status {
    let (desktops, current) = desktops(yggdrasil_wm::Desk::detect().as_ref());
    let net = parse_net(&out("nmcli", &["-t", "-f", "TYPE,STATE", "device"]), &out("nmcli", &["-t", "-f", "IN-USE,SIGNAL", "dev", "wifi"]));
    let bt = parse_bt(&out("timeout", &["2", "bluetoothctl", "show"]), &out("timeout", &["2", "bluetoothctl", "devices", "Connected"]));
    Status { desktops, current, net, bt }
}

/// NetworkManager's connectivity check (`nmcli -t -f CONNECTIVITY general`): "full", "limited", "portal", "none", "unknown".
pub fn connectivity() -> String {
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
        assert_eq!(portal_page("https://guest.example.com/terms?x=1\n"), Some("https://guest.example.com/terms?x=1"));
        assert_eq!(portal_page(""), None);
        assert_eq!(portal_page("javascript:alert(1)"), None);
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
