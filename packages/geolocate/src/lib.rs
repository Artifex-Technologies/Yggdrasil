//! Where this computer roughly is, for Weather's "Automatic" place and Settings > Date & Time > "Set time zone automatically".
//!
//! From the IP address only. The request to [beaconDB](https://beacondb.net/) (an open, non-profit location service) is the
//! body `{"considerIp":true}` and nothing else, and beaconDB places the address the request comes from (its IP-to-city
//! database: about 25 km at best, often only the city or the region). No Wi-Fi networks and no hardware IDs: nothing here
//! scans, reads or sends nearby networks, their IDs (BSSIDs) or their signal. Until 2026-11-12 both features sent the BSSIDs
//! and signal of the networks NetworkManager last saw; I asked for that to stop. The answer is rounded to two decimals
//! (about 1 km) before anything else sees it.
//!
//! While a VPN is on, the address is the VPN server's, and the place would be wherever that server is: then nothing is asked
//! and [`locate`] says why ([`Error::Vpn`]). The checks only read: NetworkManager's active connections, and which interface
//! the kernel would send internet traffic through (`ip route get`, a lookup in the routing tables that sends nothing).

use std::path::Path;
use std::time::Duration;

pub const BEACONDB_URL: &str = "https://api.beacondb.net/v1/geolocate";

/// All that is sent to beaconDB: "place the IP address this request comes from". No networks, no hardware IDs.
pub const REQUEST_BODY: &str = r#"{"considerIp":true}"#;

/// Coordinates, rounded to two decimals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    pub lat: f64,
    pub lon: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// A VPN is on (the reason): nothing was asked, as the place would be the VPN server's.
    Vpn(String),
    /// beaconDB could not be reached, or did not know the address.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Vpn(why) => write!(f, "location is off while a VPN is on ({why})"),
            Error::Failed(e) => f.write_str(e),
        }
    }
}

/// Two decimals: about 1 km.
pub fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// beaconDB's answer: {"location": {"lat", "lng"}, "accuracy", "fallback": "ipf"} or {"error": {...}}, rounded.
pub fn parse_answer(text: &str) -> Result<Place, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "beaconDB gave an answer that is not JSON".to_string())?;
    if let Some(m) = v["error"]["message"].as_str() {
        return Err(format!("beaconDB: {m}"));
    }
    match (v["location"]["lat"].as_f64(), v["location"]["lng"].as_f64()) {
        (Some(lat), Some(lon)) if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) => {
            Ok(Place { lat: round2(lat), lon: round2(lon) })
        }
        _ => Err("beaconDB did not know where this IP address is".into()),
    }
}

/// The interface in `ip -o route get ADDRESS`'s answer ("192.0.2.1 via 198.51.100.1 dev wlan0 src 198.51.100.5 uid 1000 ...").
pub fn route_dev(out: &str) -> Option<&str> {
    let mut words = out.split_whitespace();
    words.find(|w| *w == "dev")?;
    words.next()
}

/// "tun" (tun/tap: OpenVPN, Tailscale, most VPN apps) or "WireGuard" when the interface `dev` is a tunnel; `net` is
/// /sys/class/net.
pub fn tunnel_kind(net: &Path, dev: &str) -> Option<&'static str> {
    if dev.is_empty() || dev.len() > 15 || dev.contains('/') || dev.starts_with('.') {
        return None;
    }
    let d = net.join(dev);
    if d.join("tun_flags").exists() {
        return Some("tun");
    }
    let uevent = std::fs::read_to_string(d.join("uevent")).unwrap_or_default();
    uevent.lines().any(|l| l.trim() == "DEVTYPE=wireguard").then_some("WireGuard")
}

/// Why the IP address would be a VPN's, if it would: `nm_types` is `nmcli -t -f TYPE connection show --active`, `egress`
/// the interfaces internet traffic leaves through (IPv4 and IPv6). A NetworkManager "vpn" (OpenVPN, IPsec, WireGuard
/// plugins ...) or "wireguard" connection counts as soon as it is active. A "tun" one only when internet traffic goes
/// through it: Tailscale shows up as one, and without an exit node it carries only the tailnet, so the address stays the
/// computer's own (with an exit node, traffic leaves through it and it counts).
pub fn vpn_reason(nm_types: &str, egress: &[&str], net: &Path) -> Option<String> {
    for t in nm_types.lines().map(str::trim) {
        match t {
            "vpn" => return Some("a VPN connection is active".into()),
            "wireguard" => return Some("a WireGuard connection is active".into()),
            _ => {}
        }
    }
    egress.iter().find_map(|dev| tunnel_kind(net, dev).map(|k| format!("internet traffic goes through {dev}, a {k} tunnel")))
}

fn out(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default()
}

/// The VPN check on this computer (read-only). The two addresses are documentation addresses (RFC 5737, RFC 3849): `ip route
/// get` only looks them up in the routing tables, so the answer is the interface the default route uses; nothing is sent.
pub fn vpn() -> Option<String> {
    let nm = out("nmcli", &["-t", "-f", "TYPE", "connection", "show", "--active"]);
    let v4 = out("ip", &["-o", "-4", "route", "get", "192.0.2.1"]);
    let v6 = out("ip", &["-o", "-6", "route", "get", "2001:db8::1"]);
    let egress: Vec<&str> = [route_dev(&v4), route_dev(&v6)].into_iter().flatten().collect();
    vpn_reason(&nm, &egress, Path::new("/sys/class/net"))
}

/// Where this computer is, from its IP address through beaconDB; nothing is asked while a VPN is on.
pub fn locate(user_agent: &str) -> Result<Place, Error> {
    locate_with(vpn, BEACONDB_URL, user_agent)
}

fn locate_with(vpn: impl FnOnce() -> Option<String>, url: &str, user_agent: &str) -> Result<Place, Error> {
    if let Some(why) = vpn() {
        return Err(Error::Vpn(why));
    }
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(false).build().into();
    let mut resp = agent
        .post(url)
        .header("User-Agent", user_agent)
        .header("Content-Type", "application/json")
        .send(REQUEST_BODY.as_bytes())
        .map_err(|e| Error::Failed(format!("beaconDB could not be reached: {e}")))?;
    let text = resp.body_mut().read_to_string().map_err(|e| Error::Failed(e.to_string()))?;
    parse_answer(&text).map_err(Error::Failed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    /// beaconDB's answer when it placed the IP address (the shape its geolocate handler writes for its IP fallback).
    const IP_ANSWER: &str = r#"{"license":"IP geolocation data sourced from IP to City Lite by DB-IP, licensed under CC BY 4.0.","location":{"lat":48.85661,"lng":2.35222},"accuracy":25000,"fallback":"ipf"}"#;

    /// A stand-in for beaconDB on this computer's loopback: answers one request with `answer` and hands back the request it got
    /// (head and body), so the test sees exactly what would leave the computer.
    fn stand_in(answer: &'static str) -> (String, std::thread::JoinHandle<String>) {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1/geolocate", l.local_addr().unwrap());
        let h = std::thread::spawn(move || {
            let (s, _) = l.accept().unwrap();
            let mut r = BufReader::new(s);
            let mut head = String::new();
            let mut len = 0;
            loop {
                let mut line = String::new();
                r.read_line(&mut line).unwrap();
                if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
                head.push_str(&line);
                if line == "\r\n" || line.is_empty() {
                    break;
                }
            }
            let mut body = vec![0; len];
            r.read_exact(&mut body).unwrap();
            let reply = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{answer}",
                answer.len()
            );
            r.get_mut().write_all(reply.as_bytes()).unwrap();
            head + &String::from_utf8(body).unwrap()
        });
        (url, h)
    }

    #[test]
    fn only_the_ip_address_is_asked_about() {
        let v: serde_json::Value = serde_json::from_str(REQUEST_BODY).unwrap();
        assert_eq!(v, serde_json::json!({"considerIp": true}));
        let (url, h) = stand_in(IP_ANSWER);
        assert_eq!(locate_with(|| None, &url, "test"), Ok(Place { lat: 48.86, lon: 2.35 }));
        let sent = h.join().unwrap();
        assert!(sent.starts_with("POST /v1/geolocate "), "{sent}");
        assert!(sent.ends_with("\r\n\r\n{\"considerIp\":true}"), "{sent}");
        for word in ["wifiAccessPoints", "macAddress", "signalStrength", "cellTowers", "bluetoothBeacons"] {
            assert!(!sent.contains(word), "{word} in {sent}");
        }
    }

    #[test]
    fn nothing_is_asked_while_a_vpn_is_on() {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        let url = format!("http://{}/v1/geolocate", l.local_addr().unwrap());
        let r = locate_with(|| Some("a WireGuard connection is active".into()), &url, "test");
        assert_eq!(r, Err(Error::Vpn("a WireGuard connection is active".into())));
        assert!(r.unwrap_err().to_string().starts_with("location is off while a VPN is on"));
        assert_eq!(l.accept().unwrap_err().kind(), std::io::ErrorKind::WouldBlock, "the stand-in was called");
    }

    #[test]
    fn answers_are_rounded_to_about_a_kilometre() {
        assert_eq!(parse_answer(IP_ANSWER), Ok(Place { lat: 48.86, lon: 2.35 }));
        assert!(parse_answer(r#"{"error":{"code":404,"message":"Not found"}}"#).unwrap_err().contains("Not found"));
        assert!(parse_answer("<html>").is_err());
        assert!(parse_answer(r#"{"location":{"lat":999,"lng":0}}"#).is_err());
        assert_eq!(round2(39.73921), 39.74);
    }

    #[test]
    fn route_lookups_name_the_interface() {
        assert_eq!(route_dev("192.0.2.1 via 198.51.100.1 dev wlan0 src 198.51.100.5 uid 1000 \\    cache "), Some("wlan0"));
        assert_eq!(route_dev("2001:db8::1 from :: via fe80::1 dev wg0 proto static src 2001:db8::2 metric 1024 pref medium"), Some("wg0"));
        assert_eq!(route_dev(""), None);
        assert_eq!(route_dev("RTNETLINK answers: Network is unreachable"), None);
    }

    #[test]
    fn vpns_and_tunnels_are_noticed() {
        // a made-up /sys/class/net: a Wi-Fi card, a tun device and a WireGuard one
        let net = std::env::temp_dir().join(format!("geolocate-net-{}", std::process::id()));
        for (dev, file, text) in [
            ("wlan0", "uevent", "DEVTYPE=wlan\nINTERFACE=wlan0\n"),
            ("tun0", "tun_flags", "0x1001\n"),
            ("wg0", "uevent", "DEVTYPE=wireguard\nINTERFACE=wg0\n"),
        ] {
            std::fs::create_dir_all(net.join(dev)).unwrap();
            std::fs::write(net.join(dev).join(file), text).unwrap();
        }
        assert_eq!(vpn_reason("802-11-wireless\nloopback\n", &["wlan0"], &net), None);
        assert!(vpn_reason("802-11-wireless\nvpn\n", &["wlan0"], &net).unwrap().contains("VPN"));
        assert!(vpn_reason("wireguard\n", &["wlan0"], &net).unwrap().contains("WireGuard"));
        // Tailscale without an exit node: a "tun" connection, but internet traffic still leaves through the Wi-Fi
        assert_eq!(vpn_reason("802-11-wireless\ntun\n", &["wlan0"], &net), None);
        assert_eq!(vpn_reason("802-11-wireless\ntun\n", &["tun0"], &net).unwrap(), "internet traffic goes through tun0, a tun tunnel");
        // a tunnel NetworkManager does not know about (wg-quick, a VPN app), by IPv6 only
        assert!(vpn_reason("", &["wlan0", "wg0"], &net).unwrap().contains("wg0, a WireGuard tunnel"));
        assert_eq!(tunnel_kind(&net, "../wg0"), None);
        assert_eq!(tunnel_kind(&net, "missing0"), None);
        std::fs::remove_dir_all(&net).ok();
    }
}
