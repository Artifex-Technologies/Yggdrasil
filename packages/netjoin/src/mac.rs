//! Hardware addresses (MAC), for networks that reserve or only allow known addresses: each adapter's real (permanent) address
//! and the one it has now, read from NetworkManager over D-Bus (read only), and whether the system gives each Wi-Fi network a
//! private address of its own instead (NetworkManager's `wifi.cloned-mac-address` default in its configuration), in which case
//! the address a network sees is not known before joining, unless the real one is asked for (`join::Request::real_address`).

use std::path::Path;

use zbus::blocking::{proxy, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::OwnedObjectPath;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const WIRED: &str = "org.freedesktop.NetworkManager.Device.Wired";

/// One network adapter.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Adapter {
    /// Its interface name (wlan0, enp3s0).
    pub device: String,
    /// "wifi" or "ethernet".
    pub kind: String,
    /// The address it has now (while scanning, a Wi-Fi adapter may show a random one).
    pub address: String,
    /// Its own, permanent address (empty when NetworkManager does not know it).
    pub permanent: String,
    /// Plugged in by USB.
    pub usb: bool,
    pub driver: String,
    /// What a USB adapter calls itself ("Realtek 802.11ac WLAN Adapter"), from /sys.
    pub product: String,
}

impl Adapter {
    /// The real address: the permanent one, else the current one.
    pub fn real(&self) -> &str {
        if self.permanent.is_empty() {
            &self.address
        } else {
            &self.permanent
        }
    }

    /// How a page names it: "Wi-Fi (wlan0, built in)", "USB Wi-Fi: Realtek 802.11ac WLAN Adapter (wlan1)".
    pub fn label(&self) -> String {
        let what = if self.kind == "wifi" { "Wi-Fi" } else { "Wired" };
        match (self.usb, self.product.is_empty()) {
            (true, false) => format!("USB {what}: {} ({})", self.product, self.device),
            (true, true) => format!("USB {what} ({})", self.device),
            (false, _) => format!("{what} ({}, built in)", self.device),
        }
    }
}

fn object<'a>(bus: &'a Connection, path: &'a str, iface: &'a str) -> Result<Proxy<'a>, String> {
    proxy::Builder::new(bus)
        .destination(NM)
        .and_then(|b| b.path(path))
        .and_then(|b| b.interface(iface))
        .map(|b| b.cache_properties(CacheProperties::No))
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())
}

/// The wired and Wi-Fi adapters NetworkManager has, with their addresses (read only). `product` names a USB adapter by its
/// interface (from /sys outside the tests).
pub fn adapters_on(bus: &Connection, product: &dyn Fn(&str) -> String) -> Result<Vec<Adapter>, String> {
    let nm = object(bus, NM_PATH, NM)?;
    let devices: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).map_err(|e| e.to_string())?;
    let mut out = vec![];
    for d in devices {
        let dev = object(bus, d.as_str(), DEVICE)?;
        let (kind, iface) = match dev.get_property::<u32>("DeviceType").unwrap_or(0) {
            1 => ("ethernet", WIRED),
            2 => ("wifi", WIRELESS),
            _ => continue,
        };
        let text = |p: &Proxy, name: &str| p.get_property::<String>(name).unwrap_or_default();
        let device = text(&dev, "Interface");
        let usb = text(&dev, "Path").contains("-usb-");
        out.push(Adapter {
            kind: kind.into(),
            address: text(&dev, "HwAddress").to_uppercase(),
            permanent: object(bus, d.as_str(), iface).map(|p| text(&p, "PermHwAddress")).unwrap_or_default().to_uppercase(),
            usb,
            driver: text(&dev, "Driver"),
            product: if usb { product(&device) } else { String::new() },
            device,
        });
    }
    Ok(out)
}

/// What a join form says about the address a network will see, for each Wi-Fi adapter: its label, and the address, or None
/// when the system gives each network a private address of its own (made as it joins, so not known before) and the real one
/// was not asked for.
pub fn seen_by_network(adapters: &[Adapter], private: bool, real_address: bool) -> Vec<(String, Option<String>)> {
    adapters
        .iter()
        .filter(|a| a.kind == "wifi")
        .map(|a| (a.label(), (!private || real_address).then(|| a.real().to_string()).filter(|m| !m.is_empty())))
        .collect()
}

/// The adapters, from NetworkManager on the system bus (none when it cannot be reached).
pub fn adapters() -> Vec<Adapter> {
    let Ok(bus) = Connection::system() else { return vec![] };
    adapters_on(&bus, &|iface| usb_product(Path::new("/sys/class/net"), iface)).unwrap_or_default()
}

/// A USB network adapter's own name: the USB device's "manufacturer" and "product" (the parent of the interface's device).
pub fn usb_product(class_net: &Path, iface: &str) -> String {
    if iface.is_empty() || iface.contains('/') || iface.starts_with('.') {
        return String::new();
    }
    let Ok(dev) = std::fs::canonicalize(class_net.join(iface).join("device")) else { return String::new() };
    let Some(usb) = dev.parent() else { return String::new() };
    let read = |n: &str| std::fs::read_to_string(usb.join(n)).map(|s| s.trim().to_string()).unwrap_or_default();
    let (maker, product) = (read("manufacturer"), read("product"));
    if maker.is_empty() || product.is_empty() || product.starts_with(&maker) {
        product
    } else {
        format!("{maker} {product}")
    }
}

/// Whether NetworkManager's configuration gives Wi-Fi networks a private address by default (`wifi.cloned-mac-address` stable,
/// stable-ssid or random in a `[connection*]` section that applies to Wi-Fi). `files`: the configuration's texts from the
/// lowest priority to the highest (NetworkManager.conf, then conf.d by file name). As NetworkManager reads them: the
/// highest-priority file that sets it wins; within a file, the first matching section, `[connection]` itself last.
pub fn private_wifi_address(files: &[String]) -> bool {
    for text in files.iter().rev() {
        let mut sections: Vec<(String, Vec<(String, String)>)> = vec![];
        for line in text.lines() {
            let l = line.trim();
            if l.starts_with('#') || l.starts_with(';') || l.is_empty() {
                continue;
            }
            if let Some(name) = l.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                sections.push((name.trim().to_string(), vec![]));
            } else if let (Some((k, v)), Some(last)) = (l.split_once('='), sections.last_mut()) {
                last.1.push((k.trim().to_string(), v.trim().to_string()));
            }
        }
        let mut ordered: Vec<&(String, Vec<(String, String)>)> =
            sections.iter().filter(|(n, _)| n.starts_with("connection-") || n == "connection").collect();
        // [connection] is considered after the named ones
        ordered.sort_by_key(|(n, _)| n == "connection");
        for (_, keys) in ordered {
            let get = |k: &str| keys.iter().rev().find(|(kk, _)| kk == k).map(|(_, v)| v.as_str());
            let applies = get("match-device").is_none_or(|m| m.split(',').any(|t| matches!(t.trim(), "type:wifi" | "*")));
            if let (true, Some(v)) = (applies, get("wifi.cloned-mac-address")) {
                return matches!(v, "stable" | "stable-ssid" | "random");
            }
        }
    }
    false
}

/// `private_wifi_address` for this computer's NetworkManager configuration: NetworkManager.conf, then the conf.d files from
/// /usr/lib, /run and /etc by name (a file in /etc hiding one of the same name in /usr/lib).
pub fn nm_private_wifi() -> bool {
    private_wifi_address(&nm_config(Path::new("/")))
}

/// The configuration's texts under `root`, lowest priority first.
pub fn nm_config(root: &Path) -> Vec<String> {
    let mut named: std::collections::BTreeMap<String, std::path::PathBuf> = Default::default();
    for dir in ["usr/lib/NetworkManager/conf.d", "run/NetworkManager/conf.d", "etc/NetworkManager/conf.d"] {
        for e in std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.ends_with(".conf") {
                named.insert(name, e.path());
            }
        }
    }
    let main = std::fs::read_to_string(root.join("etc/NetworkManager/NetworkManager.conf")).unwrap_or_default();
    std::iter::once(main).chain(named.values().filter_map(|p| std::fs::read_to_string(p).ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_nm::*;

    #[test]
    fn each_adapter_with_its_real_and_current_address() {
        let w = world(vec![ACTIVATED], 0, WPA2);
        let a = adapters_on(&w.bus, &|i| if i == "wlan1" { "Example Wireless USB".into() } else { String::new() }).unwrap();
        assert_eq!(
            a.iter().map(|x| (x.device.as_str(), x.kind.as_str(), x.usb)).collect::<Vec<_>>(),
            [("eth0", "ethernet", false), ("wlan0", "wifi", false), ("wlan1", "wifi", true)]
        );
        assert_eq!(
            (a[1].address.as_str(), a[1].real()),
            ("02:00:00:00:00:99", "02:00:00:00:00:01"),
            "scanning: the real one is the permanent one"
        );
        assert_eq!(a[1].label(), "Wi-Fi (wlan0, built in)");
        assert_eq!(a[2].label(), "USB Wi-Fi: Example Wireless USB (wlan1)");
        assert_eq!(a[0].real(), "02:00:00:00:00:10");
        let unknown = Adapter { address: "02:00:00:00:00:05".into(), ..Adapter::default() };
        assert_eq!(unknown.real(), "02:00:00:00:00:05", "no permanent address known: the current one");
    }

    #[test]
    fn the_address_a_network_will_see() {
        let wifi = |d: &str, perm: &str, usb| Adapter {
            device: d.into(),
            kind: "wifi".into(),
            address: "02:00:00:00:00:99".into(),
            permanent: perm.into(),
            usb,
            ..Adapter::default()
        };
        let wired = Adapter { device: "eth0".into(), kind: "ethernet".into(), address: "02:00:00:00:00:10".into(), ..Adapter::default() };
        let all = [wired, wifi("wlan0", "02:00:00:00:00:01", false), wifi("wlan1", "02:00:00:00:00:02", true)];
        assert_eq!(
            seen_by_network(&all, false, false),
            [
                ("Wi-Fi (wlan0, built in)".to_string(), Some("02:00:00:00:00:01".to_string())),
                ("USB Wi-Fi (wlan1)".to_string(), Some("02:00:00:00:00:02".to_string()))
            ]
        );
        assert!(seen_by_network(&all, true, false).iter().all(|(_, m)| m.is_none()), "a private address: not known before joining");
        assert_eq!(seen_by_network(&all, true, true)[0].1.as_deref(), Some("02:00:00:00:00:01"), "the real one, when asked for");
        assert!(seen_by_network(&[], false, false).is_empty());
    }

    #[test]
    fn a_usb_adapter_is_named_from_sys() {
        let d = std::env::temp_dir().join(format!("netjoin-sys-{}", std::process::id()));
        let usb = d.join("devices/usb1/1-2");
        std::fs::create_dir_all(usb.join("1-2:1.0")).unwrap();
        std::fs::create_dir_all(d.join("class/net/wlan1")).unwrap();
        std::fs::write(usb.join("manufacturer"), "Example\n").unwrap();
        std::fs::write(usb.join("product"), "Wireless USB Adapter\n").unwrap();
        std::os::unix::fs::symlink(usb.join("1-2:1.0"), d.join("class/net/wlan1/device")).unwrap();
        assert_eq!(usb_product(&d.join("class/net"), "wlan1"), "Example Wireless USB Adapter");
        assert_eq!(usb_product(&d.join("class/net"), "wlan9"), "");
        assert_eq!(usb_product(&d.join("class/net"), "../x"), "", "an interface name, never a path");
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn whether_wifi_networks_get_a_private_address() {
        let f = |v: &[&str]| private_wifi_address(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert!(!f(&[]), "NetworkManager's own default: the adapter's address");
        // a security package's file (as Heimdall installs one)
        let stable = "# note\n[connection-wifi-privacy]\nmatch-device=type:wifi\nwifi.cloned-mac-address=stable\n";
        assert!(f(&["", stable]));
        assert!(f(&["[connection]\nwifi.cloned-mac-address=random\n"]));
        assert!(!f(&["[connection]\nwifi.cloned-mac-address=permanent\n"]));
        assert!(!f(&[stable, "[connection-mine]\nwifi.cloned-mac-address=preserve\n"]), "a later file wins");
        assert!(f(&["[connection-mine]\nwifi.cloned-mac-address=preserve\n", stable]));
        assert!(!f(&["[connection-eth]\nmatch-device=type:ethernet\nwifi.cloned-mac-address=random\n"]), "not for Wi-Fi");
        assert!(
            f(&["[connection]\nwifi.cloned-mac-address=permanent\n[connection-wifi]\nwifi.cloned-mac-address=stable\n"]),
            "[connection] is considered last"
        );
        assert!(!f(&["[main]\nwifi.cloned-mac-address=random\n"]), "only [connection*] sections");
        assert!(!f(&["[connection]\n#wifi.cloned-mac-address=random\n"]));
    }

    #[test]
    fn the_configuration_files_in_order() {
        let d = std::env::temp_dir().join(format!("netjoin-conf-{}", std::process::id()));
        for (p, t) in [
            ("usr/lib/NetworkManager/conf.d/50-privacy.conf", "lib-50"),
            ("usr/lib/NetworkManager/conf.d/10-a.conf", "lib-10"),
            ("etc/NetworkManager/conf.d/50-privacy.conf", "etc-50"),
            ("etc/NetworkManager/conf.d/notes.txt", "ignored"),
            ("etc/NetworkManager/NetworkManager.conf", "main"),
        ] {
            std::fs::create_dir_all(d.join(p).parent().unwrap()).unwrap();
            std::fs::write(d.join(p), t).unwrap();
        }
        assert_eq!(nm_config(&d), ["main", "lib-10", "etc-50"], "/etc hides /usr/lib's file of the same name");
        std::fs::remove_dir_all(&d).unwrap();
    }
}
