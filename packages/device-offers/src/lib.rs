//! Device software: which of the operating system's optional parts this computer's hardware asks for, and whether they are
//! installed. The installer ticks them on its Optional software page; yggdrasil-device-offers offers the missing ones later, when
//! the installed system comes online or such a device is plugged in (see README.md).
//!
//! The list is the OS's own (its `extras` file, Labyrinth's installer-extras.json). An entry is asked for by:
//! - `chosen_if_usb`: a USB device plugged in now, by exact "vendor:product" ("06cb:00fc"), or by maker ("1532"). A maker alone
//!   counts only for a device of the entry's `usb_kind` when it gives one: "keyboard", "mouse" or "input" (either; the HID boot
//!   interfaces every keyboard and mouse has), "printer" (USB printer class), "gamepad" (an Xbox controller's own interface). So a
//!   Corsair fan controller is not a Corsair keyboard, and a HyperX mouse (HP's vendor id) is not an HP printer.
//! - `chosen_if_machine`: what the computer is (Hardware::matches).
//!
//! Everything here reads files and answers; nothing runs a program.

use serde_json::{Map, Value};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A USB device plugged in: its ids (lowercase hex) and the kinds of device its interfaces say it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Usb {
    pub vendor: String,
    pub product: String,
    /// "keyboard", "mouse", "printer", "gamepad" (see interface_kind), sorted
    pub kinds: Vec<&'static str>,
}

impl Usb {
    pub fn new(vendor: &str, product: &str, kinds: &[&'static str]) -> Usb {
        let mut kinds = kinds.to_vec();
        kinds.sort_unstable();
        kinds.dedup();
        Usb { vendor: vendor.to_ascii_lowercase(), product: product.to_ascii_lowercase(), kinds }
    }

    /// Of this `usb_kind` ("input": a keyboard or a mouse). A kind this program does not know is never matched.
    pub fn is(&self, kind: &str) -> bool {
        match kind {
            "input" => self.kinds.iter().any(|k| *k == "keyboard" || *k == "mouse"),
            k => self.kinds.contains(&k),
        }
    }
}

/// What a USB interface is, from its class, subclass and protocol (two hex digits each, as /sys shows them).
pub fn interface_kind(class: &str, subclass: &str, protocol: &str) -> Option<&'static str> {
    match (class, subclass, protocol) {
        // HID with the boot protocol: what BIOSes talk to, so every keyboard and mouse (and their wireless receivers) has one
        ("03", "01", "01") => Some("keyboard"),
        ("03", "01", "02") => Some("mouse"),
        ("07", _, _) => Some("printer"),
        // Xbox 360 (5d) and Xbox One/Series (47) controllers and their wireless adapters: vendor-specific, these subclasses
        ("ff", "5d", _) | ("ff", "47", _) => Some("gamepad"),
        _ => None,
    }
}

/// The computer, for the `chosen_if_usb` and `chosen_if_machine` rules. Read from /sys and /proc (Hardware::read; a made-up tree in
/// tests).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hardware {
    /// The USB devices plugged in now, sorted by their ids.
    pub usb: Vec<Usb>,
    /// /proc/cpuinfo's vendor_id: "GenuineIntel", "AuthenticAMD" (none on most ARM computers).
    pub cpu_vendor: String,
    /// A battery that runs the computer (power_supply of type Battery, not a mouse's or a pen's: those say scope Device).
    pub battery: bool,
    /// An input device with a tablet-mode switch (SW_TABLET_MODE): a 2-in-1 that folds into a tablet.
    pub tablet_switch: bool,
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn text(p: PathBuf) -> Option<String> {
    std::fs::read_to_string(p).map(|s| s.trim().to_lowercase()).ok()
}

impl Hardware {
    /// The computer as seen under `root` ("/" outside tests).
    pub fn read(root: &Path) -> Hardware {
        // devices ("1-2": idVendor, idProduct) and their interfaces ("1-2:1.0": bInterfaceClass ...), both links in one folder
        let mut usb: Vec<(String, Usb)> = vec![];
        let mut kinds: Vec<(String, &'static str)> = vec![];
        for d in entries(&root.join("sys/bus/usb/devices")) {
            let name = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Some(v) = text(d.join("idVendor")) {
                usb.push((name, Usb::new(&v, &text(d.join("idProduct")).unwrap_or_default(), &[])));
            } else if let Some((dev, _)) = name.split_once(':') {
                let f = |n: &str| text(d.join(n)).unwrap_or_default();
                if let Some(k) = interface_kind(&f("bInterfaceClass"), &f("bInterfaceSubClass"), &f("bInterfaceProtocol")) {
                    kinds.push((dev.to_string(), k));
                }
            }
        }
        for (dev, k) in kinds {
            if let Some((_, u)) = usb.iter_mut().find(|(n, _)| *n == dev) {
                u.kinds.push(k);
                u.kinds.sort_unstable();
                u.kinds.dedup();
            }
        }
        let mut usb: Vec<Usb> = usb.into_iter().map(|(_, u)| u).collect();
        usb.sort_by(|a, b| (&a.vendor, &a.product).cmp(&(&b.vendor, &b.product)));
        let cpu_vendor = std::fs::read_to_string(root.join("proc/cpuinfo"))
            .ok()
            .and_then(|t| t.lines().find(|l| l.starts_with("vendor_id")).and_then(|l| l.split(':').nth(1)).map(|v| v.trim().to_string()))
            .unwrap_or_default();
        let battery = entries(&root.join("sys/class/power_supply"))
            .into_iter()
            .any(|d| text(d.join("type")).as_deref() == Some("battery") && text(d.join("scope")).as_deref() != Some("device"));
        let tablet_switch = entries(&root.join("sys/class/input"))
            .into_iter()
            .any(|d| text(d.join("capabilities/sw")).is_some_and(|b| sw_has_tablet_mode(&b)));
        Hardware { usb, cpu_vendor, battery, tablet_switch }
    }

    /// `chosen_if_usb` with the entry's `usb_kind`: a device plugged in now is one of `ids` ("vvvv:pppp" exactly; "vvvv" any device
    /// of that maker, of `kind` when one is given). Case does not matter.
    pub fn plugged(&self, ids: &Value, kind: Option<&str>) -> bool {
        ids.as_array().is_some_and(|ids| {
            ids.iter().filter_map(Value::as_str).map(str::to_ascii_lowercase).any(|x| match x.split_once(':') {
                Some((v, p)) => self.usb.iter().any(|u| u.vendor == v && u.product == p),
                None => self.usb.iter().any(|u| u.vendor == x && kind.is_none_or(|k| u.is(k))),
            })
        })
    }

    /// `chosen_if_machine`: every key it gives holds. "cpu_vendor" (as /proc/cpuinfo says it), "battery" and "tablet_switch"
    /// (true or false), "absent" (an absolute path that must not exist: a place where the software would refuse to run). A
    /// key this program does not know never ticks the entry; neither does an empty rule.
    pub fn matches(&self, rule: &Map<String, Value>, exists: &dyn Fn(&str) -> bool) -> bool {
        !rule.is_empty()
            && rule.iter().all(|(k, v)| match k.as_str() {
                "cpu_vendor" => v.as_str() == Some(self.cpu_vendor.as_str()),
                "battery" => v.as_bool() == Some(self.battery),
                "tablet_switch" => v.as_bool() == Some(self.tablet_switch),
                "absent" => v.as_str().is_some_and(|p| p.starts_with('/') && !exists(p)),
                _ => false,
            })
    }
}

/// An input device's switch bitmap (capabilities/sw: hex words, the lowest last) has SW_TABLET_MODE (bit 1).
pub fn sw_has_tablet_mode(bits: &str) -> bool {
    bits.split_whitespace().last().and_then(|w| u64::from_str_radix(w, 16).ok()).is_some_and(|v| v & 0b10 != 0)
}

/// An entry of the OS's list is asked for by this computer: a device plugged in (`chosen_if_usb`, `usb_kind`) or what the computer
/// is (`chosen_if_machine`). `exists` answers for paths (the file system, outside tests).
pub fn wanted(e: &Value, hw: &Hardware, exists: &dyn Fn(&str) -> bool) -> bool {
    hw.plugged(&e["chosen_if_usb"], e["usb_kind"].as_str()) || e["chosen_if_machine"].as_object().is_some_and(|r| hw.matches(r, exists))
}

/// The OS's components (its `components` file: `{"components": [{"id", "packages", "aur", "pkgbuild"}, ...]}`): each id with every
/// package it installs, as Agora counts them.
pub fn component_packages(text: &str) -> Vec<(String, Vec<String>)> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let names = |x: &Value| -> Vec<String> {
        x.as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    v["components"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    let id = c["id"].as_str()?.to_string();
                    let mut p = names(&c["packages"]);
                    p.extend(names(&c["aur"]));
                    p.extend(c["pkgbuild"].as_str().map(str::to_string));
                    Some((id, p))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A component is installed when it names packages and every one of them is installed (Agora's rule). None: no such component.
pub fn component_installed(comps: &[(String, Vec<String>)], id: &str, installed: &HashSet<String>) -> Option<bool> {
    comps.iter().find(|c| c.0 == id).map(|c| !c.1.is_empty() && c.1.iter().all(|p| installed.contains(p)))
}

/// The component ids the OS still installs by itself for `user` (its first-login list, `{"user", "components"}`); nothing when the
/// list is someone else's or cannot be read.
pub fn pending(text: &str, user: &str) -> HashSet<String> {
    let v: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    if v["user"].as_str() != Some(user) {
        return HashSet::new();
    }
    v["components"].as_array().map(|a| a.iter().filter_map(|c| c.as_str().map(str::to_string)).collect()).unwrap_or_default()
}

/// Something to offer: an entry of the OS's list and the components of it to install.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    pub id: String,
    pub name: String,
    pub note: String,
    pub components: Vec<String>,
}

/// What to offer now: the entries this computer asks for (wanted) whose components are known and not all installed. Left out:
/// the ids in `leave` (said no to, or offered already), and components in `pending` (the OS installs those by itself at the
/// first login; an entry with nothing else missing is left to it). In the list's order.
pub fn offers(
    extras: &str,
    comps: &[(String, Vec<String>)],
    hw: &Hardware,
    exists: &dyn Fn(&str) -> bool,
    installed: &HashSet<String>,
    leave: &HashSet<String>,
    pending: &HashSet<String>,
) -> Vec<Offer> {
    let v: Value = serde_json::from_str(extras).unwrap_or(Value::Null);
    let Some(list) = v["extras"].as_array() else { return vec![] };
    list.iter()
        .filter_map(|e| {
            let id = e["id"].as_str()?.to_string();
            if leave.contains(&id) || !wanted(e, hw, exists) {
                return None;
            }
            let ids: Vec<String> = e["components"].as_array()?.iter().filter_map(|c| c.as_str().map(str::to_string)).collect();
            let mut missing = vec![];
            for c in &ids {
                match component_installed(comps, c, installed) {
                    None => return None, // the OS lists a component it has no recipe for: nothing to install it with
                    Some(false) if !pending.contains(c) => missing.push(c.clone()),
                    Some(_) => {}
                }
            }
            if missing.is_empty() {
                return None;
            }
            Some(Offer {
                name: e["name"].as_str().unwrap_or(&id).to_string(),
                note: e["note"].as_str().unwrap_or("").to_string(),
                components: missing,
                id,
            })
        })
        .collect()
}

/// A default route that is not the loopback's in the kernel's routing tables (/proc/net/route, /proc/net/ipv6_route under `root`):
/// how to tell whether the computer could be online when NetworkManager is not there to say.
pub fn default_route(root: &Path) -> bool {
    let v4 = std::fs::read_to_string(root.join("proc/net/route")).unwrap_or_default();
    let v4 = v4.lines().skip(1).any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        // Iface Destination Gateway Flags RefCnt Use Metric Mask ...; RTF_UP is flag 1
        f.len() > 7 && f[0] != "lo" && f[1] == "00000000" && f[7] == "00000000" && u32::from_str_radix(f[3], 16).is_ok_and(|x| x & 1 != 0)
    });
    let v6 = std::fs::read_to_string(root.join("proc/net/ipv6_route")).unwrap_or_default();
    let v6 = v6.lines().any(|l| {
        let f: Vec<&str> = l.split_whitespace().collect();
        // destination, its prefix length, ..., flags (field 8), device (last); RTF_UP 1, and not a reject route (0x200)
        f.len() == 10
            && f[9] != "lo"
            && f[0].chars().all(|c| c == '0')
            && f[1] == "00"
            && u32::from_str_radix(f[8], 16).is_ok_and(|x| x & 1 != 0 && x & 0x200 == 0)
    });
    v4 || v6
}

/// Ids in a plain list file, one per line.
pub fn read_ids(path: &Path) -> HashSet<String> {
    std::fs::read_to_string(path)
        .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect())
        .unwrap_or_default()
}

/// Adds ids to such a list (the folder is made when missing).
pub fn add_ids(path: &Path, ids: &[String]) -> std::io::Result<()> {
    let mut all: Vec<String> = read_ids(path).into_iter().collect();
    all.extend(ids.iter().cloned());
    all.sort();
    all.dedup();
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, all.iter().map(|i| format!("{i}\n")).collect::<String>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A made-up computer under a temporary folder: USB devices with their interfaces, /proc/cpuinfo, power supplies, input switches.
    pub struct Root(pub PathBuf);

    impl Root {
        pub fn new(name: &str) -> Root {
            let d = std::env::temp_dir().join(format!("ygg-device-offers-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Root(d)
        }
        pub fn put(&self, p: &str, t: &str) {
            let f = self.0.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, format!("{t}\n")).unwrap();
        }
        /// A USB device "bus-port" with its interfaces (class, subclass, protocol).
        pub fn usb(&self, at: &str, vendor: &str, product: &str, interfaces: &[(&str, &str, &str)]) {
            self.put(&format!("sys/bus/usb/devices/{at}/idVendor"), vendor);
            self.put(&format!("sys/bus/usb/devices/{at}/idProduct"), product);
            for (i, (c, s, p)) in interfaces.iter().enumerate() {
                let f = format!("sys/bus/usb/devices/{at}:1.{i}");
                self.put(&format!("{f}/bInterfaceClass"), c);
                self.put(&format!("{f}/bInterfaceSubClass"), s);
                self.put(&format!("{f}/bInterfaceProtocol"), p);
            }
        }
    }

    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const KEYBOARD: (&str, &str, &str) = ("03", "01", "01");
    const MOUSE: (&str, &str, &str) = ("03", "01", "02");
    const HID: (&str, &str, &str) = ("03", "00", "00");

    #[test]
    fn the_computer_is_read_from_sys_and_proc() {
        let r = Root::new("x1");
        r.usb("usb1", "1d6b", "0002", &[("09", "00", "00")]);
        r.usb("1-3", "06CB", "00FC", &[("ff", "00", "00")]);
        r.put("proc/cpuinfo", "processor\t: 0\nvendor_id\t: GenuineIntel\ncpu family\t: 6");
        r.put("sys/class/power_supply/AC/type", "Mains");
        r.put("sys/class/power_supply/BAT0/type", "Battery");
        r.put("sys/class/power_supply/BAT0/scope", "System");
        r.put("sys/class/input/input0/capabilities/sw", "0");
        r.put("sys/class/input/input1/capabilities/sw", "2");
        let hw = Hardware::read(&r.0);
        assert_eq!(hw.usb, [Usb::new("06cb", "00fc", &[]), Usb::new("1d6b", "0002", &[])], "ids lowercase, sorted");
        assert_eq!((hw.cpu_vendor.as_str(), hw.battery, hw.tablet_switch), ("GenuineIntel", true, true));

        let desk = Root::new("desk");
        desk.put("proc/cpuinfo", "vendor_id\t: AuthenticAMD");
        desk.put("sys/class/power_supply/hidpp_battery_0/type", "Battery");
        desk.put("sys/class/power_supply/hidpp_battery_0/scope", "Device");
        desk.put("sys/class/input/input0/capabilities/sw", "1 0");
        let hw = Hardware::read(&desk.0);
        assert_eq!(
            (hw.cpu_vendor.as_str(), hw.battery, hw.tablet_switch),
            ("AuthenticAMD", false, false),
            "a mouse's battery is not the computer's; the lid switch (bit 0) is not a tablet switch"
        );
        assert_eq!(Hardware::read(&r.0.join("nothing")), Hardware::default(), "nothing to read: nothing known");
        assert!(sw_has_tablet_mode("3") && sw_has_tablet_mode("0 2") && !sw_has_tablet_mode("2 0") && !sw_has_tablet_mode("zz"));
    }

    #[test]
    fn each_device_knows_what_its_interfaces_are() {
        let r = Root::new("kinds");
        // a wireless receiver with a keyboard and a mouse interface and a vendor one; a fan controller (plain HID); a printer;
        // an Xbox One controller; a webcam (video)
        r.usb("1-1", "046d", "c548", &[KEYBOARD, MOUSE, HID]);
        r.usb("1-2", "1b1c", "0c10", &[HID]);
        r.usb("1-3", "03f0", "c511", &[("07", "01", "02"), ("ff", "00", "00")]);
        r.usb("1-4", "045e", "02ea", &[("ff", "47", "d0")]);
        r.usb("1-5", "046d", "085c", &[("0e", "01", "00"), ("0e", "02", "00")]);
        let hw = Hardware::read(&r.0);
        let kinds: Vec<(&str, Vec<&str>)> = hw.usb.iter().map(|u| (u.product.as_str(), u.kinds.clone())).collect();
        assert_eq!(
            kinds,
            [("c511", vec!["printer"]), ("02ea", vec!["gamepad"]), ("085c", vec![]), ("c548", vec!["keyboard", "mouse"]), ("0c10", vec![]),]
        );
        assert!(hw.usb[3].is("input") && hw.usb[3].is("mouse") && !hw.usb[3].is("printer") && !hw.usb[3].is("colour"));
        assert_eq!(interface_kind("03", "00", "00"), None, "HID without the boot protocol: a controller, a dial, a fan hub");
    }

    fn device_list() -> Value {
        json!({"extras": [
            {"id": "razer", "name": "Razer", "components": ["razer"], "chosen_if_usb": ["1532"]},
            {"id": "ckb-next", "name": "Corsair keyboards and mice", "components": ["ckb-next"], "chosen_if_usb": ["1b1c"], "usb_kind": "input"},
            {"id": "gaming-mice", "name": "Gaming mice", "components": ["gaming-mice"], "chosen_if_usb": ["046d", "258a:0036"], "usb_kind": "mouse"},
            {"id": "hp", "name": "HP printers", "components": ["hp"], "chosen_if_usb": ["03f0"], "usb_kind": "printer"},
            {"id": "fingerprint", "name": "Fingerprint reader", "components": ["fingerprint"], "chosen_if_usb": ["06CB:00FC"]},
            {"id": "thermald", "name": "Cooling", "components": ["thermald"], "chosen_if_machine": {"cpu_vendor": "GenuineIntel", "battery": true, "absent": "/sys/devices/platform/thinkpad_acpi/dytc_lapmode"}},
            {"id": "osk", "components": ["osk"], "chosen_if_machine": {"tablet_switch": true}},
            {"id": "odd", "components": ["osk"], "chosen_if_machine": {"tablet_switch": true, "colour": "red"}},
            {"id": "empty", "components": ["osk"], "chosen_if_machine": {}},
            {"id": "sarina", "name": "Sarina", "components": ["sarina"], "chosen_if_exists": "/usr/bin/sarina"}
        ]})
    }

    fn wanted_ids(hw: &Hardware, exists: &dyn Fn(&str) -> bool) -> Vec<String> {
        device_list()["extras"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| wanted(e, hw, exists))
            .map(|e| e["id"].as_str().unwrap().to_string())
            .collect()
    }

    #[test]
    fn a_maker_counts_only_for_its_kind_of_device_an_exact_id_always() {
        let none = &|_: &str| false;
        let hw = |usb: Vec<Usb>| Hardware { usb, ..Default::default() };
        assert_eq!(wanted_ids(&hw(vec![Usb::new("1532", "0084", &[])]), none), ["razer"], "no kind asked: any device of the maker");
        assert!(wanted_ids(&hw(vec![Usb::new("1b1c", "0c10", &[])]), none).is_empty(), "a Corsair fan controller is no keyboard");
        assert_eq!(wanted_ids(&hw(vec![Usb::new("1B1C", "1b7d", &["keyboard"])]), none), ["ckb-next"]);
        assert!(
            wanted_ids(&hw(vec![Usb::new("03f0", "0d84", &["keyboard", "mouse"])]), none).is_empty(),
            "a HyperX keyboard is no HP printer"
        );
        assert_eq!(wanted_ids(&hw(vec![Usb::new("03f0", "c511", &["printer"])]), none), ["hp"]);
        assert!(wanted_ids(&hw(vec![Usb::new("046d", "085c", &[])]), none).is_empty(), "a Logitech webcam is no gaming mouse");
        assert_eq!(wanted_ids(&hw(vec![Usb::new("046d", "c548", &["keyboard", "mouse"])]), none), ["gaming-mice"]);
        assert_eq!(wanted_ids(&hw(vec![Usb::new("258a", "0036", &[])]), none), ["gaming-mice"], "an exact id needs no kind");
        assert!(wanted_ids(&hw(vec![Usb::new("258a", "0001", &["mouse"])]), none).is_empty(), "nor does that maker's other mouse count");
        assert!(wanted_ids(&Hardware::default(), none).is_empty(), "nothing plugged in: nothing asked for");
    }

    #[test]
    fn laptop_software_is_asked_for_by_what_the_computer_is() {
        let yoga =
            Hardware { usb: vec![Usb::new("06cb", "00fc", &[])], cpu_vendor: "GenuineIntel".into(), battery: true, tablet_switch: true };
        assert_eq!(
            wanted_ids(&yoga, &|_| false),
            ["fingerprint", "thermald", "osk"],
            "an unknown rule key or an empty rule asks for nothing"
        );
        assert_eq!(
            wanted_ids(&yoga, &|p| p.ends_with("dytc_lapmode")),
            ["fingerprint", "osk"],
            "a ThinkPad whose firmware runs the cooling"
        );
        let other_reader = Hardware { usb: vec![Usb::new("06cb", "00a2", &[])], ..yoga.clone() };
        assert!(!wanted_ids(&other_reader, &|_| false).contains(&"fingerprint".to_string()), "a reader libfprint cannot drive");
        assert!(
            wanted_ids(&Hardware { cpu_vendor: "GenuineIntel".into(), ..Default::default() }, &|_| false).is_empty(),
            "an Intel desktop"
        );
    }

    fn comps() -> Vec<(String, Vec<String>)> {
        component_packages(
            r#"{"components": [{"id": "razer", "packages": ["openrazer-daemon", "dkms"], "aur": ["polychromatic"]},
                {"id": "ckb-next", "packages": ["ckb-next"]}, {"id": "gaming-mice", "packages": ["piper"]},
                {"id": "thermald", "packages": ["thermald"]}, {"id": "nothing"}, {"id": "own", "pkgbuild": "own-git"}, {"no": "id"}]}"#,
        )
    }

    #[test]
    fn components_count_as_agora_counts_them() {
        let c = comps();
        assert_eq!(c.len(), 6);
        assert_eq!(c[0].1, ["openrazer-daemon", "dkms", "polychromatic"], "repository and AUR packages");
        assert_eq!(c[5].1, ["own-git"], "its own build");
        let have: HashSet<String> = ["openrazer-daemon", "dkms"].iter().map(|s| s.to_string()).collect();
        assert_eq!(component_installed(&c, "razer", &have), Some(false), "Polychromatic is missing");
        let have: HashSet<String> = ["openrazer-daemon", "dkms", "polychromatic"].iter().map(|s| s.to_string()).collect();
        assert_eq!(component_installed(&c, "razer", &have), Some(true));
        assert_eq!(component_installed(&c, "nothing", &have), Some(false), "a component that names nothing is never installed");
        assert_eq!(component_installed(&c, "steam", &have), None);
        assert!(component_packages("not json").is_empty());
    }

    #[test]
    fn what_is_offered() {
        let list = device_list().to_string();
        let c = comps();
        let hw = Hardware {
            usb: vec![Usb::new("1532", "0084", &["mouse"]), Usb::new("1b1c", "1b7d", &["keyboard"]), Usb::new("046d", "c548", &["mouse"])],
            cpu_vendor: "GenuineIntel".into(),
            battery: true,
            ..Default::default()
        };
        let none = HashSet::new();
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<HashSet<String>>();
        let ids = |o: Vec<Offer>| o.into_iter().map(|o| (o.id, o.components)).collect::<Vec<_>>();
        let got = offers(&list, &c, &hw, &|_| false, &none, &none, &none);
        assert_eq!(got[0], Offer { id: "razer".into(), name: "Razer".into(), note: String::new(), components: vec!["razer".into()] });
        assert_eq!(
            ids(got),
            [
                ("razer".into(), vec!["razer".to_string()]),
                ("ckb-next".into(), vec!["ckb-next".to_string()]),
                ("gaming-mice".into(), vec!["gaming-mice".to_string()]),
                ("thermald".into(), vec!["thermald".to_string()]),
            ],
            "in the list's order; Sarina is not the hardware's business"
        );
        let installed = set(&["ckb-next", "piper"]);
        let got = offers(&list, &c, &hw, &|_| false, &installed, &set(&["thermald"]), &set(&["razer"]));
        assert!(got.is_empty(), "installed, said no to, or the first login's: {got:?}");
        let fp = Hardware { usb: vec![Usb::new("06cb", "00fc", &[])], ..Default::default() };
        assert!(offers(&list, &c, &fp, &|_| false, &none, &none, &none).is_empty(), "no recipe for the component: nothing to offer");
        assert!(offers("{}", &c, &hw, &|_| false, &none, &none, &none).is_empty());
        assert!(offers("not json", &c, &hw, &|_| false, &none, &none, &none).is_empty());
    }

    #[test]
    fn the_first_login_list_is_only_its_users() {
        let t = r#"{"user": "ada", "components": ["razer", "sarina", 5]}"#;
        assert_eq!(pending(t, "ada"), ["razer", "sarina"].iter().map(|s| s.to_string()).collect());
        assert!(pending(t, "bob").is_empty());
        assert!(pending("", "ada").is_empty());
    }

    #[test]
    fn a_default_route_says_the_computer_could_be_online() {
        let r = Root::new("routes");
        assert!(!default_route(&r.0), "no tables: no route");
        r.put(
            "proc/net/route",
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nlo\t00000000\t00000000\t0001\t0\t0\t0\t00000000\t0\t0\t0\nwlan0\t0010A8C0\t00000000\t0001\t0\t0\t600\t00FFFFFF\t0\t0\t0",
        );
        assert!(!default_route(&r.0), "only the local network and a loopback default");
        r.put(
            "proc/net/route",
            "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT\nwlan0\t00000000\t0100A8C0\t0003\t0\t0\t600\t00000000\t0\t0\t0",
        );
        assert!(default_route(&r.0));
        r.put("proc/net/route", "Iface\tDestination\tGateway \tFlags\tRefCnt\tUse\tMetric\tMask\t\tMTU\tWindow\tIRTT");
        let zero = "00000000000000000000000000000000";
        r.put("proc/net/ipv6_route", &format!("{zero} 00 {zero} 00 {zero} ffffffff 00000001 00000000 00200200       lo"));
        assert!(!default_route(&r.0), "the loopback's reject route");
        r.put(
            "proc/net/ipv6_route",
            &format!("{zero} 00 {zero} 00 fe800000000000000000000000000001 00000400 00000001 00000000 00450003   enp5s0"),
        );
        assert!(default_route(&r.0));
    }

    #[test]
    fn id_lists_are_kept_sorted_and_once() {
        let r = Root::new("ids");
        let f = r.0.join("a/b/never");
        assert!(read_ids(&f).is_empty());
        add_ids(&f, &["razer".into(), "ckb-next".into()]).unwrap();
        add_ids(&f, &["razer".into()]).unwrap();
        assert_eq!(std::fs::read_to_string(&f).unwrap(), "ckb-next\nrazer\n");
        assert_eq!(read_ids(&f), ["ckb-next", "razer"].iter().map(|s| s.to_string()).collect());
    }
}
