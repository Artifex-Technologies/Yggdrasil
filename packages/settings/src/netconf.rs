//! Wi-Fi & Network > one network's own page, as on a phone: auto-join, private address, IPv4 and IPv6 (automatic or typed
//! in), DNS servers, a proxy, and forgetting it. A network's settings are its NetworkManager profile; they are read from
//! `nmcli -t connection show UUID` and written with `nmcli connection modify`. Parsers and the list of changes are pure
//! functions, tested without nmcli.

use crate::sys::{self, MacMode};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// How an address is got.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum IpMethod {
    /// From the router (DHCP; on IPv6 also the router's announcements).
    Auto,
    /// Typed in.
    Manual,
    /// Not used (IPv6 only).
    Off,
}

/// One IP version's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Ip {
    pub method: IpMethod,
    /// The address with its prefix length (192.168.1.20/24), when typed in.
    pub address: String,
    pub router: String,
    /// DNS servers, separated by commas or spaces; empty: the router's.
    pub dns: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ProxyMode {
    Off,
    /// From a configuration file (PAC) at a web address.
    Auto,
    /// One proxy server (host:port) for everything.
    Manual,
}

/// A saved network's settings.
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub uuid: String,
    pub name: String,
    pub wifi: bool,
    /// The Wi-Fi network's name (empty for wired).
    pub ssid: String,
    pub auto_join: bool,
    pub mac: MacMode,
    pub v4: Ip,
    pub v6: Ip,
    pub proxy: ProxyMode,
    pub pac_url: String,
    pub proxy_server: String,
    /// When it was last connected (NetworkManager's `connection.timestamp`, seconds since 1970; 0: never).
    pub last_connected: i64,
    /// The saved key management (`802-11-wireless-security.key-mgmt`: wpa-psk, sae, wpa-eap, owe ...; empty: open).
    pub key_mgmt: String,
    /// Joined even though it leaves its name out of its beacons.
    pub hidden: bool,
    /// The access points NetworkManager has seen for it (`802-11-wireless.seen-bssids`).
    pub seen_bssids: Vec<String>,
}

/// What a connected network gave the computer (shown, not changed).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Live {
    pub v4: Vec<String>,
    pub router: String,
    pub v6: Vec<String>,
    pub dns: Vec<String>,
    pub hw: String,
    /// The DHCP lease (IPv4), when the address came from one: its length in seconds, the server that gave it, when it ends
    /// (seconds since 1970), and the domain the network calls itself.
    pub lease_secs: Option<u64>,
    pub dhcp_server: String,
    pub lease_ends: Option<i64>,
    pub domain: String,
}

/// The network an address with its prefix is on: "192.168.1.20/24" -> "192.168.1.0/24".
pub fn subnet(addr: &str) -> Option<String> {
    let (a, p) = addr.split_once('/')?;
    let a: Ipv4Addr = a.trim().parse().ok()?;
    let p: u32 = p.trim().parse().ok().filter(|p| *p <= 32)?;
    let mask = if p == 0 { 0 } else { u32::MAX << (32 - p) };
    Some(format!("{}/{p}", Ipv4Addr::from(u32::from(a) & mask)))
}

/// The saved security, for people (NetworkManager's key management; WPA2 and WPA3 transition networks are saved as wpa-psk).
pub fn key_mgmt_label(k: &str) -> &'static str {
    match k {
        "" => "No password (open)",
        "none" => "WEP key",
        "ieee8021x" => "Account (dynamic WEP)",
        "wpa-psk" => "Password (WPA/WPA2-Personal)",
        "sae" => "Password (WPA3-Personal)",
        "wpa-eap" => "Account (802.1X Enterprise)",
        "wpa-eap-suite-b-192" => "Account (WPA3-Enterprise 192-bit)",
        "owe" => "No password (Enhanced Open)",
        _ => "Secured",
    }
}

/// `nmcli -t connection show UUID` (one `key:value` a line, ':' in values escaped) as a map.
pub fn settings_map(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let f = sys::nm_fields(l);
            (f.len() >= 2).then(|| (f[0].clone(), f[1..].join(":")))
        })
        .collect()
}

fn unset(v: &str) -> &str {
    let v = v.trim();
    if v == "--" {
        ""
    } else {
        v
    }
}

/// The first value of a list nmcli writes with commas.
fn first(v: &str) -> String {
    unset(v).split(',').next().unwrap_or("").trim().to_string()
}

fn list(v: &str) -> String {
    unset(v).split(',').map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ")
}

/// The proxy server a one-line PAC script of ours names ("PROXY host:port; DIRECT").
fn pac_server(script: &str) -> Option<String> {
    let at = script.find("PROXY ")? + 6;
    let rest = &script[at..];
    let end = rest.find([';', '"', '\'']).unwrap_or(rest.len());
    Some(rest[..end].trim().to_string()).filter(|s| !s.is_empty())
}

/// The PAC script that sends everything through one proxy (NetworkManager keeps a proxy only as PAC).
fn pac_script(server: &str) -> String {
    format!("function FindProxyForURL(url, host) {{ return \"PROXY {server}; DIRECT\"; }}")
}

pub fn parse(m: &HashMap<String, String>) -> Profile {
    let g = |k: &str| unset(m.get(k).map(String::as_str).unwrap_or("")).to_string();
    let wifi = g("connection.type") == "802-11-wireless";
    let mac_key = if wifi { "802-11-wireless.cloned-mac-address" } else { "802-3-ethernet.cloned-mac-address" };
    let ip = |v: &str| {
        let method = match g(&format!("{v}.method")).as_str() {
            "manual" => IpMethod::Manual,
            "disabled" | "ignore" => IpMethod::Off,
            _ => IpMethod::Auto,
        };
        Ip { method, address: first(&g(&format!("{v}.addresses"))), router: g(&format!("{v}.gateway")), dns: list(&g(&format!("{v}.dns"))) }
    };
    let script = g("proxy.pac-script");
    let (proxy, proxy_server) = match g("proxy.method").as_str() {
        "auto" => match pac_server(&script) {
            Some(s) if g("proxy.pac-url").is_empty() => (ProxyMode::Manual, s),
            _ => (ProxyMode::Auto, String::new()),
        },
        _ => (ProxyMode::Off, String::new()),
    };
    Profile {
        uuid: g("connection.uuid"),
        name: g("connection.id"),
        wifi,
        ssid: g("802-11-wireless.ssid"),
        auto_join: g("connection.autoconnect") != "no",
        mac: MacMode::from_nm(&g(mac_key), wifi),
        v4: ip("ipv4"),
        v6: ip("ipv6"),
        proxy,
        pac_url: g("proxy.pac-url"),
        proxy_server,
        last_connected: g("connection.timestamp").parse().unwrap_or(0),
        key_mgmt: g("802-11-wireless-security.key-mgmt"),
        hidden: g("802-11-wireless.hidden") == "yes",
        seen_bssids: g("802-11-wireless.seen-bssids").split(',').map(str::trim).filter(|b| b.len() == 17).map(str::to_uppercase).collect(),
    }
}

/// `nmcli -t -f IP4.ADDRESS,IP4.GATEWAY,IP4.DNS,IP6.ADDRESS,IP6.DNS,GENERAL.HWADDR,DHCP4 device show DEV`.
pub fn parse_live(text: &str) -> Live {
    let mut l = Live::default();
    for (k, v) in settings_map(text) {
        let v = unset(&v).to_string();
        if v.is_empty() {
            continue;
        }
        let k = k.split('[').next().unwrap_or("").to_string();
        match k.as_str() {
            "IP4.ADDRESS" => l.v4.push(v),
            "IP4.GATEWAY" => l.router = v,
            "IP6.ADDRESS" if !v.starts_with("fe80") => l.v6.push(v),
            "IP4.DNS" | "IP6.DNS" => l.dns.push(v),
            "GENERAL.HWADDR" => l.hw = v,
            // "dhcp_lease_time = 43200"
            "DHCP4.OPTION" => {
                let Some((name, val)) = v.split_once(" = ") else { continue };
                let val = val.trim();
                match name.trim() {
                    "dhcp_lease_time" => l.lease_secs = val.parse().ok(),
                    "dhcp_server_identifier" => l.dhcp_server = val.to_string(),
                    "expiry" => l.lease_ends = val.parse().ok(),
                    "domain_name" => l.domain = val.chars().filter(|c| !c.is_control()).take(64).collect(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    l.v4.sort();
    l.v6.sort();
    l.dns.sort();
    l
}

fn split_list(s: &str) -> Vec<&str> {
    s.split([',', ' ']).map(str::trim).filter(|x| !x.is_empty()).collect()
}

/// An address with its prefix: 192.168.1.20/24 (a bare address takes /24 on IPv4, /64 on IPv6).
fn with_prefix(a: &str, v6: bool) -> Result<String, String> {
    let (addr, prefix) = a.split_once('/').map(|(x, p)| (x.trim(), Some(p.trim()))).unwrap_or((a.trim(), None));
    let max = if v6 { 128 } else { 32 };
    let ok = if v6 { addr.parse::<Ipv6Addr>().is_ok() } else { addr.parse::<Ipv4Addr>().is_ok() };
    if !ok {
        return Err(format!("“{addr}” is not an IPv{} address", if v6 { 6 } else { 4 }));
    }
    let p: u8 = match prefix {
        Some(p) => {
            p.parse().ok().filter(|&n: &u8| n >= 1 && n <= max).ok_or_else(|| format!("“/{p}” is not a prefix length (1 to {max})"))?
        }
        None => {
            if v6 {
                64
            } else {
                24
            }
        }
    };
    Ok(format!("{addr}/{p}"))
}

/// The nmcli settings that turn `old` into `new`, in pairs, or why `new` cannot be used (a typed address that is not one).
pub fn changes(old: &Profile, new: &Profile) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    let mut set = |k: &str, v: String| out.push((k.to_string(), v));
    if old.auto_join != new.auto_join {
        set("connection.autoconnect", if new.auto_join { "yes" } else { "no" }.into());
    }
    if old.mac != new.mac {
        if let MacMode::Custom(a) = &new.mac {
            if !sys::valid_mac(a) {
                return Err("that is not a usable hardware address (six pairs like 02:1A:2B:3C:4D:5E)".into());
            }
        }
        let key = if new.wifi { "802-11-wireless.cloned-mac-address" } else { "802-3-ethernet.cloned-mac-address" };
        set(key, new.mac.to_nm());
    }
    for (v, a, b) in [("ipv4", &old.v4, &new.v4), ("ipv6", &old.v6, &new.v6)] {
        let six = v == "ipv6";
        if a.method != b.method || (b.method == IpMethod::Manual && (a.address != b.address || a.router != b.router)) {
            match b.method {
                IpMethod::Manual => {
                    if b.address.trim().is_empty() {
                        return Err(format!("type the {} address to use, or set it back to Automatic", if six { "IPv6" } else { "IPv4" }));
                    }
                    let addr = with_prefix(&b.address, six)?;
                    let router = b.router.trim();
                    if !router.is_empty()
                        && !matches!(router.parse::<IpAddr>(), Ok(IpAddr::V6(_)) if six)
                        && !matches!(router.parse::<IpAddr>(), Ok(IpAddr::V4(_)) if !six)
                    {
                        return Err(format!("“{router}” is not the router's IPv{} address", if six { 6 } else { 4 }));
                    }
                    set(&format!("{v}.method"), "manual".into());
                    set(&format!("{v}.addresses"), addr);
                    set(&format!("{v}.gateway"), router.to_string());
                }
                IpMethod::Auto => {
                    set(&format!("{v}.method"), "auto".into());
                    set(&format!("{v}.addresses"), String::new());
                    set(&format!("{v}.gateway"), String::new());
                }
                IpMethod::Off => set(&format!("{v}.method"), if six { "disabled" } else { "auto" }.into()),
            }
        }
        if a.dns != b.dns {
            let servers = split_list(&b.dns);
            for s in &servers {
                let ok = match s.parse::<IpAddr>() {
                    Ok(IpAddr::V4(_)) => !six,
                    Ok(IpAddr::V6(_)) => six,
                    Err(_) => false,
                };
                if !ok {
                    return Err(format!("“{s}” is not an IPv{} DNS server address", if six { 6 } else { 4 }));
                }
            }
            set(&format!("{v}.dns"), servers.join(","));
            // servers typed in replace the router's; none: the router's again
            set(&format!("{v}.ignore-auto-dns"), if servers.is_empty() { "no" } else { "yes" }.into());
        }
    }
    if old.proxy != new.proxy || old.pac_url != new.pac_url || old.proxy_server != new.proxy_server {
        match new.proxy {
            ProxyMode::Off => {
                set("proxy.method", "none".into());
                set("proxy.pac-url", String::new());
                set("proxy.pac-script", String::new());
            }
            ProxyMode::Auto => {
                let url = new.pac_url.trim();
                if !(url.starts_with("http://") || url.starts_with("https://") || url.starts_with("file://")) {
                    return Err("the proxy configuration's address starts with http:// or https://".into());
                }
                set("proxy.method", "auto".into());
                set("proxy.pac-url", url.to_string());
                set("proxy.pac-script", String::new());
            }
            ProxyMode::Manual => {
                let s = new.proxy_server.trim();
                let ok = s.rsplit_once(':').is_some_and(|(h, p)| {
                    !h.is_empty()
                        && !h.contains(char::is_whitespace)
                        && !h.contains(['"', '\'', ';'])
                        && p.parse::<u16>().is_ok_and(|n| n > 0)
                });
                if !ok {
                    return Err("type the proxy as server:port (proxy.example.com:8080)".into());
                }
                set("proxy.method", "auto".into());
                set("proxy.pac-url", String::new());
                set("proxy.pac-script", pac_script(s));
            }
        }
    }
    Ok(out)
}

pub fn load(uuid: &str) -> Option<Profile> {
    let text = sys::run("nmcli", &["-t", "connection", "show", uuid])?;
    Some(parse(&settings_map(&text)))
}

pub fn live(device: &str) -> Live {
    parse_live(
        &sys::run(
            "nmcli",
            &["-t", "-f", "IP4.ADDRESS,IP4.GATEWAY,IP4.DNS,IP6.ADDRESS,IP6.DNS,GENERAL.HWADDR,DHCP4", "device", "show", device],
        )
        .unwrap_or_default(),
    )
}

/// Writes the changes; they are used from the next time the computer joins the network.
pub fn save(uuid: &str, pairs: &[(String, String)]) -> Result<(), String> {
    if pairs.is_empty() {
        return Ok(());
    }
    let mut args: Vec<&str> = vec!["connection", "modify", uuid];
    for (k, v) in pairs {
        args.push(k);
        args.push(v);
    }
    sys::act("nmcli", &args)
}

/// Joins again now, so changed settings take effect (the connection drops for a moment).
pub fn rejoin(uuid: &str) -> Result<(), String> {
    sys::act("nmcli", &["connection", "up", uuid])
}

/// Forgets the network: its password and settings go.
pub fn forget(uuid: &str) -> Result<(), String> {
    sys::act("nmcli", &["connection", "delete", uuid])
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "connection.id:Home\nconnection.uuid:1b2c\nconnection.type:802-11-wireless\nconnection.autoconnect:yes\n\
connection.timestamp:1794000000\n802-11-wireless-security.key-mgmt:sae\n802-11-wireless.hidden:no\n\
802-11-wireless.seen-bssids:02\\:00\\:00\\:00\\:00\\:01,02\\:00\\:00\\:00\\:00\\:02\n802-11-wireless.ssid:Home\n802-11-wireless.cloned-mac-address:permanent\nipv4.method:auto\nipv4.dns:\nipv4.addresses:\n\
ipv4.gateway:--\nipv6.method:auto\nipv6.dns:\nipv6.addresses:\nipv6.gateway:--\nproxy.method:none\nproxy.pac-url:\nproxy.pac-script:\n";

    #[test]
    fn a_profile_is_read_from_nmcli() {
        let p = parse(&settings_map(HOME));
        assert_eq!((p.name.as_str(), p.uuid.as_str(), p.wifi, p.ssid.as_str(), p.auto_join), ("Home", "1b2c", true, "Home", true));
        assert_eq!(p.mac, MacMode::Real);
        assert_eq!((p.last_connected, p.key_mgmt.as_str(), p.hidden), (1_794_000_000, "sae", false));
        assert_eq!(p.seen_bssids, vec!["02:00:00:00:00:01".to_string(), "02:00:00:00:00:02".to_string()]);
        assert_eq!(key_mgmt_label(&p.key_mgmt), "Password (WPA3-Personal)");
        assert_eq!(p.v4, Ip { method: IpMethod::Auto, address: String::new(), router: String::new(), dns: String::new() });
        assert_eq!(p.proxy, ProxyMode::Off);
        // typed-in addresses, servers, IPv6 off, and a proxy of ours read back as Manual
        let m = HOME
            .replace("ipv4.method:auto", "ipv4.method:manual")
            .replace("ipv4.addresses:", "ipv4.addresses:192.168.1.20/24")
            .replace("ipv4.gateway:--", "ipv4.gateway:192.168.1.1")
            .replace("ipv4.dns:", "ipv4.dns:1.1.1.1,9.9.9.9")
            .replace("ipv6.method:auto", "ipv6.method:disabled")
            .replace("connection.autoconnect:yes", "connection.autoconnect:no")
            .replace("proxy.method:none", "proxy.method:auto")
            .replace(
                "proxy.pac-script:",
                "proxy.pac-script:function FindProxyForURL(url, host) { return \"PROXY proxy.lan\\:3128; DIRECT\"; }",
            );
        let p = parse(&settings_map(&m));
        assert_eq!(
            (p.v4.method, p.v4.address.as_str(), p.v4.router.as_str(), p.v4.dns.as_str()),
            (IpMethod::Manual, "192.168.1.20/24", "192.168.1.1", "1.1.1.1, 9.9.9.9")
        );
        assert_eq!(p.v6.method, IpMethod::Off);
        assert!(!p.auto_join);
        assert_eq!((p.proxy, p.proxy_server.as_str()), (ProxyMode::Manual, "proxy.lan:3128"));
    }

    #[test]
    fn changes_become_nmcli_settings_and_bad_ones_are_refused() {
        let old = parse(&settings_map(HOME));
        assert!(changes(&old, &old).unwrap().is_empty(), "nothing changed, nothing written");
        let mut new = old.clone();
        new.auto_join = false;
        new.v4 =
            Ip { method: IpMethod::Manual, address: "192.168.1.20".into(), router: "192.168.1.1".into(), dns: "1.1.1.1 9.9.9.9".into() };
        new.v6.method = IpMethod::Off;
        new.proxy = ProxyMode::Manual;
        new.proxy_server = "proxy.lan:3128".into();
        let c: HashMap<String, String> = changes(&old, &new).unwrap().into_iter().collect();
        assert_eq!(c["connection.autoconnect"], "no");
        assert_eq!(c["ipv4.method"], "manual");
        assert_eq!(c["ipv4.addresses"], "192.168.1.20/24", "a bare address takes /24");
        assert_eq!(c["ipv4.gateway"], "192.168.1.1");
        assert_eq!(c["ipv4.dns"], "1.1.1.1,9.9.9.9");
        assert_eq!(c["ipv4.ignore-auto-dns"], "yes");
        assert_eq!(c["ipv6.method"], "disabled");
        assert_eq!(c["proxy.method"], "auto");
        assert_eq!(pac_server(&c["proxy.pac-script"]).as_deref(), Some("proxy.lan:3128"), "the script reads back");
        // back to automatic: the typed address and router are cleared, the router's DNS used again
        let mut back = new.clone();
        back.v4 = old.v4.clone();
        let c: HashMap<String, String> = changes(&new, &back).unwrap().into_iter().collect();
        assert_eq!((c["ipv4.method"].as_str(), c["ipv4.addresses"].as_str(), c["ipv4.gateway"].as_str()), ("auto", "", ""));
        assert_eq!(c["ipv4.ignore-auto-dns"], "no");
        // refused, with the reason
        let bad = |f: &dyn Fn(&mut Profile)| {
            let mut p = old.clone();
            f(&mut p);
            changes(&old, &p).unwrap_err()
        };
        assert!(bad(&|p| p.v4 = Ip { method: IpMethod::Manual, address: "192.168.1.300".into(), ..p.v4.clone() })
            .contains("not an IPv4 address"));
        assert!(bad(&|p| p.v4 = Ip { method: IpMethod::Manual, address: "10.0.0.2/33".into(), ..p.v4.clone() }).contains("prefix length"));
        assert!(bad(&|p| p.v4 = Ip { method: IpMethod::Manual, ..p.v4.clone() }).contains("type the IPv4 address"));
        assert!(bad(&|p| p.v4.dns = "2606:4700::1111".into()).contains("not an IPv4 DNS server"));
        assert!(bad(&|p| p.v6.dns = "dns.google".into()).contains("not an IPv6 DNS server"));
        assert!(bad(&|p| {
            p.proxy = ProxyMode::Manual;
            p.proxy_server = "proxy; DIRECT\"".into()
        })
        .contains("server:port"));
        assert!(bad(&|p| {
            p.proxy = ProxyMode::Auto;
            p.pac_url = "wpad.dat".into()
        })
        .contains("http://"));
        assert!(bad(&|p| p.mac = MacMode::Custom("01:00:00:00:00:00".into())).contains("hardware address"));
    }

    #[test]
    fn what_a_connected_network_gave_is_read() {
        let t =
            "GENERAL.HWADDR:AA\\:BB\\:CC\\:DD\\:EE\\:FF\nIP4.ADDRESS[1]:192.168.1.20/24\nIP4.GATEWAY:192.168.1.1\nIP4.DNS[1]:192.168.1.1\n\
IP6.ADDRESS[1]:2001\\:db8\\:\\:20/64\nIP6.ADDRESS[2]:fe80\\:\\:1/64\nIP6.DNS[1]:--\n\
DHCP4.OPTION[1]:dhcp_lease_time = 43200\nDHCP4.OPTION[2]:dhcp_server_identifier = 192.168.1.1\nDHCP4.OPTION[3]:expiry = 1794100000\n\
DHCP4.OPTION[4]:domain_name = lan\nDHCP4.OPTION[5]:requested_routers = 1\n";
        let l = parse_live(t);
        assert_eq!(
            (l.lease_secs, l.dhcp_server.as_str(), l.lease_ends, l.domain.as_str()),
            (Some(43200), "192.168.1.1", Some(1_794_100_000), "lan")
        );
        assert_eq!(subnet("192.168.1.20/24").as_deref(), Some("192.168.1.0/24"));
        assert_eq!(subnet("10.20.30.40/8").as_deref(), Some("10.0.0.0/8"));
        assert_eq!((subnet("10.0.0.1"), subnet("10.0.0.1/40"), subnet("x/24")), (None, None, None));
        assert_eq!(l.hw, "AA:BB:CC:DD:EE:FF");
        assert_eq!((l.v4, l.router.as_str(), l.dns), (vec!["192.168.1.20/24".to_string()], "192.168.1.1", vec!["192.168.1.1".to_string()]));
        assert_eq!(l.v6, vec!["2001:db8::20/64".to_string()], "the link-local address is left out");
    }
}
