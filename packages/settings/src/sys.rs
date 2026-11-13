//! Reading and changing the system, through the usual command-line tools (nmcli, bluetoothctl, hyprctl ...). Every
//! parser is a pure function of the tool's output so it can be tested without the tool.

use std::collections::BTreeMap;
use std::process::Command;

/// Run a program and return its standard output, or None if it is missing or failed. (A debug build started with a fixture
/// answers from it and runs nothing: fixture.rs.)
pub fn run(cmd: &str, args: &[&str]) -> Option<String> {
    if let Some(f) = crate::fixture::get() {
        return f.answer(cmd, args);
    }
    let out = Command::new(cmd).args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Run a program for its effect; Err carries its error output.
pub fn act(cmd: &str, args: &[&str]) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    match Command::new(cmd).args(args).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(format!("{cmd}: {e}")),
    }
}

/// Start a program and leave it running (an app, a terminal).
pub fn spawn(cmd: &str, args: &[&str]) {
    if crate::fixture::get().is_some() {
        return;
    }
    let _ = Command::new(cmd).args(args).spawn();
}

// ---------------------------------------------------------------- About

#[derive(Clone, Debug, Default, PartialEq)]
pub struct About {
    pub os: String,
    /// The system's logo (LOGO in /etc/os-release, an icon name), as a file; empty when there is none.
    pub logo: String,
    pub hostname: String,
    pub cpu: String,
    pub memory_gib: f64,
    pub gpus: Vec<String>,
    pub kernel: String,
    pub disk_total: u64,
    pub disk_free: u64,
}

/// The file of the icon named by LOGO in /etc/os-release (the standard icon folders), if there is one.
pub fn os_logo(os_release: &str) -> String {
    let Some(name) = os_release.lines().find_map(|l| l.strip_prefix("LOGO=")).map(|v| v.trim().trim_matches('"').to_string()) else {
        return String::new();
    };
    if name.is_empty() || name.contains('/') {
        return String::new();
    }
    ["256x256/apps", "512x512/apps", "128x128/apps", "scalable/apps"]
        .iter()
        .flat_map(|d| [format!("/usr/share/icons/hicolor/{d}/{name}.png"), format!("/usr/share/icons/hicolor/{d}/{name}.svg")])
        .chain([format!("/usr/share/pixmaps/{name}.png"), format!("/usr/share/pixmaps/{name}.svg")])
        .find(|p| std::path::Path::new(p).is_file())
        .unwrap_or_default()
}

/// The system's name with its version, as About shows it: "Labyrinth OS 1.0" from PRETTY_NAME (or NAME) and VERSION. A
/// PRETTY_NAME that already names the version ("Fedora Linux 40 (Workstation Edition)") is shown as it is.
pub fn os_name(os_release: &str) -> String {
    let kv: BTreeMap<&str, &str> = os_release
        .lines()
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim().trim_matches('"').trim_matches('\'')))
        .collect();
    let name = kv.get("PRETTY_NAME").or(kv.get("NAME")).copied().unwrap_or_default();
    let version = kv.get("VERSION").copied().unwrap_or_default();
    let id = kv.get("VERSION_ID").copied().unwrap_or_default();
    let named = [version, id].iter().any(|v| !v.is_empty() && name.contains(v));
    if name.is_empty() || version.is_empty() || named {
        name.to_string()
    } else {
        format!("{name} {version}")
    }
}

pub fn cpu_model(cpuinfo: &str) -> String {
    let model = cpuinfo
        .lines()
        .find_map(|l| l.strip_prefix("model name").and_then(|r| r.split_once(':')).map(|(_, v)| v.trim().to_string()))
        .unwrap_or_default();
    let threads = cpuinfo.lines().filter(|l| l.starts_with("processor")).count();
    if threads > 0 && !model.is_empty() {
        format!("{model} × {threads}")
    } else {
        model
    }
}

pub fn mem_gib(meminfo: &str) -> f64 {
    meminfo
        .lines()
        .find_map(|l| l.strip_prefix("MemTotal:"))
        .and_then(|r| r.split_whitespace().next())
        .and_then(|kb| kb.parse::<f64>().ok())
        .map(|kb| kb / 1024.0 / 1024.0)
        .unwrap_or(0.0)
}

/// GPU names from `lspci -mm` (VGA and 3D controllers).
pub fn gpus(lspci_mm: &str) -> Vec<String> {
    lspci_mm
        .lines()
        .filter(|l| l.contains("\"VGA compatible controller\"") || l.contains("\"3D controller\"") || l.contains("\"Display controller\""))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('"').collect();
            // slot "class" "vendor" "device" ...  -> fields 1,3,5 are class, vendor, device
            if f.len() > 5 {
                Some(format!("{} {}", f[3], f[5]).trim().to_string())
            } else {
                None
            }
        })
        .collect()
}

/// Total and free bytes of the filesystem holding / from `df -B1 --output=size,avail /`.
pub fn disk(df: &str) -> (u64, u64) {
    df.lines()
        .nth(1)
        .map(|l| {
            let v: Vec<u64> = l.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            (v.first().copied().unwrap_or(0), v.get(1).copied().unwrap_or(0))
        })
        .unwrap_or((0, 0))
}

pub fn about() -> About {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    let (disk_total, disk_free) = disk(&run("df", &["-B1", "--output=size,avail", "/"]).unwrap_or_default());
    About {
        os: os_name(&read("/etc/os-release")),
        logo: os_logo(&read("/etc/os-release")),
        hostname: read("/etc/hostname").trim().to_string(),
        cpu: cpu_model(&read("/proc/cpuinfo")),
        memory_gib: mem_gib(&read("/proc/meminfo")),
        gpus: gpus(&run("lspci", &["-mm"]).unwrap_or_default()),
        kernel: run("uname", &["-r"]).unwrap_or_default().trim().to_string(),
        disk_total,
        disk_free,
    }
}

// ---------------------------------------------------------------- Network (NetworkManager)

#[derive(Clone, Debug, PartialEq)]
pub struct Wifi {
    pub in_use: bool,
    pub ssid: String,
    pub signal: u8,
    pub secured: bool,
}

/// Split one `nmcli -t` line on unescaped colons (nmcli escapes ':' in values as '\:').
pub fn nm_fields(line: &str) -> Vec<String> {
    let mut out = vec![String::new()];
    let mut esc = false;
    for c in line.chars() {
        if esc {
            out.last_mut().unwrap().push(c);
            esc = false;
        } else if c == '\\' {
            esc = true;
        } else if c == ':' {
            out.push(String::new());
        } else {
            out.last_mut().unwrap().push(c);
        }
    }
    out
}

/// `nmcli -t -f IN-USE,SSID,SIGNAL,SECURITY device wifi list`, strongest first, one row per name.
pub fn wifi_list(text: &str) -> Vec<Wifi> {
    let mut best: BTreeMap<String, Wifi> = BTreeMap::new();
    for l in text.lines() {
        let f = nm_fields(l);
        if f.len() < 4 || f[1].is_empty() {
            continue;
        }
        let w =
            Wifi { in_use: f[0] == "*", ssid: f[1].clone(), signal: f[2].parse().unwrap_or(0), secured: !f[3].is_empty() && f[3] != "--" };
        let keep = match best.get(&w.ssid) {
            Some(old) => w.in_use || (!old.in_use && w.signal > old.signal),
            None => true,
        };
        if keep {
            best.insert(w.ssid.clone(), w);
        }
    }
    let mut v: Vec<Wifi> = best.into_values().collect();
    v.sort_by(|a, b| b.in_use.cmp(&a.in_use).then(b.signal.cmp(&a.signal)));
    v
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetDevice {
    pub device: String,
    pub kind: String,
    pub state: String,
    pub connection: String,
}

/// Whether a wired adapter has a cable in: NetworkManager calls it "unavailable" without one (and leaves alone one it does
/// not manage).
pub fn cable_in(state: &str) -> bool {
    !(state.starts_with("unavailable") || state.starts_with("unmanaged") || state.is_empty())
}

/// `nmcli -t -f DEVICE,TYPE,STATE,CONNECTION device`, Ethernet and Wi-Fi only.
pub fn net_devices(text: &str) -> Vec<NetDevice> {
    text.lines()
        .map(nm_fields)
        .filter(|f| f.len() >= 4 && (f[1] == "ethernet" || f[1] == "wifi"))
        .map(|f| NetDevice { device: f[0].clone(), kind: f[1].clone(), state: f[2].clone(), connection: f[3].clone() })
        .collect()
}

#[derive(Clone, Debug, Default)]
pub struct Network {
    pub wifi_on: bool,
    pub devices: Vec<NetDevice>,
    pub networks: Vec<Wifi>,
    pub saved: Vec<crate::netconf::Profile>,
    /// What each connected profile (by its id) was given: addresses, router, DNS.
    pub live: std::collections::HashMap<String, crate::netconf::Live>,
    /// The security monitor's device list for the connection that has the router (its id, and the list), when it keeps one.
    pub lan: Option<(String, crate::devices::List)>,
    /// What the radio last heard: the kernel's scan list, read without sending anything, grouped by name
    /// (yggdrasil-wifiscan). Empty when Wi-Fi is off or the list could not be read (`air_error` says why). Shared, not
    /// copied, when the page takes its copy of the data each frame.
    pub air: std::sync::Arc<Vec<yggdrasil_wifiscan::Network>>,
    pub air_error: String,
    /// The makers of the access points shown, by the first three bytes of their addresses (the IEEE list, offline).
    pub makers: std::sync::Arc<std::collections::HashMap<[u8; 3], String>>,
    /// What Settings remembers between visits: when each access point was first and last heard, each network's last lease.
    pub memory: std::sync::Arc<crate::wifimem::Memory>,
}

impl Network {
    /// What the radio heard of the network with this name (NetworkManager's name for it).
    pub fn air_for(&self, ssid: &str) -> Option<&yggdrasil_wifiscan::Network> {
        if ssid.is_empty() {
            return None;
        }
        self.air
            .iter()
            .find(|a| !a.hidden() && a.ssid == ssid.as_bytes())
            .or_else(|| self.air.iter().find(|a| !a.hidden() && a.name() == ssid))
    }

    /// Who made an access point ("" when not known: a made-up address, or not in the list).
    pub fn maker(&self, mac: &[u8; 6]) -> &str {
        self.makers.get(&[mac[0], mac[1], mac[2]]).map(String::as_str).unwrap_or("")
    }
}

/// The kernel's last scan results on the Wi-Fi adapters (or a fixture's made-up ones), grouped by name.
fn air(devices: &[String]) -> (Vec<yggdrasil_wifiscan::Network>, String) {
    if let Some(f) = crate::fixture::get() {
        return (yggdrasil_wifiscan::group(yggdrasil_wifiscan::parse_fixture(&f.scan)), String::new());
    }
    let mut all = Vec::new();
    let mut err = String::new();
    for d in devices {
        match yggdrasil_wifiscan::nl80211::scan_dump(d) {
            Ok(v) => all.extend(v),
            Err(e) => err = e.to_string(),
        }
    }
    (yggdrasil_wifiscan::group(all), if devices.is_empty() { String::new() } else { err })
}

/// The makers of these address prefixes, from the IEEE list on disk. Each prefix is looked up once per run (the list is a
/// few megabytes).
fn makers(want: Vec<[u8; 3]>) -> std::collections::HashMap<[u8; 3], String> {
    static CACHE: std::sync::Mutex<BTreeMap<[u8; 3], Option<String>>> = std::sync::Mutex::new(BTreeMap::new());
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let mut missing: Vec<[u8; 3]> = want.iter().filter(|p| !cache.contains_key(*p)).copied().collect();
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        let found = yggdrasil_wifiscan::oui::makers(std::path::Path::new(yggdrasil_wifiscan::oui::OUI_PATH), &missing);
        for p in missing {
            cache.insert(p, found.get(&p).cloned());
        }
    }
    want.into_iter().filter_map(|p| cache.get(&p).cloned().flatten().map(|n| (p, n))).collect()
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// Which hardware (MAC) address the computer shows a network, as on phones.
#[derive(Clone, Debug, PartialEq)]
pub enum MacMode {
    /// A random address of its own for this network, the same every time (NetworkManager's "stable"). The default.
    Private,
    /// A new random address every time it connects.
    Random,
    /// The adapter's real address.
    Real,
    /// One typed in.
    Custom(String),
}

impl MacMode {
    /// From NetworkManager's cloned-mac-address value. Unset follows the default, which the security setup makes private on Wi-Fi.
    pub fn from_nm(v: &str, wifi: bool) -> MacMode {
        match v.trim() {
            "stable" | "stable-ssid" => MacMode::Private,
            "random" => MacMode::Random,
            "permanent" | "preserve" => MacMode::Real,
            "" | "--" => {
                if wifi {
                    MacMode::Private
                } else {
                    MacMode::Real
                }
            }
            a if valid_mac(a) => MacMode::Custom(a.to_uppercase()),
            _ => MacMode::Private,
        }
    }

    pub fn to_nm(&self) -> String {
        match self {
            MacMode::Private => "stable".into(),
            MacMode::Random => "random".into(),
            MacMode::Real => "permanent".into(),
            MacMode::Custom(a) => a.to_uppercase(),
        }
    }
}

/// A usable address to type in: six pairs of hex digits, and not a group (multicast) address.
pub fn valid_mac(a: &str) -> bool {
    let parts: Vec<&str> = a.split(':').collect();
    parts.len() == 6
        && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
        && u8::from_str_radix(parts[0], 16).map(|b| b & 1 == 0).unwrap_or(false)
}

/// A random address of the kind phones use: locally administered and not a group address, so it cannot clash with a real
/// adapter's.
pub fn random_mac() -> String {
    let mut b = [0u8; 6];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut b)).is_err() {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        b.copy_from_slice(&t.to_le_bytes()[..6]);
    }
    b[0] = (b[0] & 0xfc) | 0x02;
    b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(":")
}

/// The saved networks (NetworkManager profiles, Wi-Fi and wired) with their settings.
fn saved_nets() -> Vec<crate::netconf::Profile> {
    let mut out = vec![];
    for line in run("nmcli", &["-t", "-f", "NAME,UUID,TYPE", "connection", "show"]).unwrap_or_default().lines() {
        let f = nm_fields(line);
        if f.len() < 3 || (f[2] != "802-11-wireless" && f[2] != "802-3-ethernet") {
            continue;
        }
        if let Some(p) = crate::netconf::load(&f[1]) {
            out.push(p);
        }
    }
    out
}

/// The connected profiles and the device each is on (`nmcli -t -f UUID,DEVICE connection show --active`).
pub fn active_profiles(text: &str) -> Vec<(String, String)> {
    text.lines().map(nm_fields).filter(|f| f.len() >= 2 && !f[1].is_empty()).map(|f| (f[0].clone(), f[1].clone())).collect()
}

pub fn network() -> Network {
    let mut n = Network {
        wifi_on: run("nmcli", &["radio", "wifi"]).map(|s| s.trim() == "enabled").unwrap_or(false),
        devices: net_devices(&run("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).unwrap_or_default()),
        networks: wifi_list(&run("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"]).unwrap_or_default()),
        saved: saved_nets(),
        live: active_profiles(&run("nmcli", &["-t", "-f", "UUID,DEVICE", "connection", "show", "--active"]).unwrap_or_default())
            .into_iter()
            .map(|(uuid, dev)| (uuid, crate::netconf::live(&dev)))
            .collect(),
        lan: None,
        air: Default::default(),
        air_error: String::new(),
        makers: Default::default(),
        memory: Default::default(),
    };
    // the connection to the router (not a VPN or Tailscale's tunnel)
    let home = n.live.iter().filter(|(_, l)| !l.router.is_empty()).map(|(u, _)| u.clone()).min();
    n.lan = home.and_then(|u| crate::devices::load(&u).map(|l| (u, l)));
    if n.wifi_on {
        let devs: Vec<String> = n.devices.iter().filter(|d| d.kind == "wifi").map(|d| d.device.clone()).collect();
        let (heard, err) = air(&devs);
        n.air = std::sync::Arc::new(heard);
        n.air_error = err;
    }
    remember(&mut n, unix_now());
    // the makers of the access points (made-up addresses have none) and of the vendor extras they announce
    let seen_before = n.saved.iter().flat_map(|p| p.seen_bssids.iter()).filter_map(|b| yggdrasil_wifiscan::parse_mac(b));
    let heard = n.air.iter().flat_map(|a| a.aps.iter()).map(|a| a.bss.bssid);
    let mut want: Vec<[u8; 3]> =
        heard.chain(seen_before).filter(|m| !yggdrasil_wifiscan::oui::is_local(m)).map(|m| [m[0], m[1], m[2]]).collect();
    want.extend(n.air.iter().flat_map(|a| a.aps.iter()).flat_map(|a| a.info.vendors.iter()).map(|v| v.oui));
    want.sort();
    want.dedup();
    n.makers = std::sync::Arc::new(makers(want));
    n
}

/// Notes what was heard and the saved networks' leases in Settings' memory (wifimem.rs), and keeps it in `n` for the pages.
/// With a fixture it is read, never written.
fn remember(n: &mut Network, now: i64) {
    let mut m = crate::wifimem::load();
    let mut write = false;
    for ap in n.air.iter().flat_map(|a| a.aps.iter()) {
        let ago = i64::from(ap.bss.seen_ms_ago.unwrap_or(0)) / 1000;
        write |= m.note_seen(&yggdrasil_wifiscan::mac_text(&ap.bss.bssid), now - ago);
    }
    for p in &n.saved {
        if let Some(l) = n.live.get(&p.uuid).filter(|l| !l.v4.is_empty() || !l.router.is_empty()) {
            write |= m.note_lease(&p.uuid, crate::wifimem::Lease::from_live(l, now));
        }
    }
    let saved: Vec<&str> = n.saved.iter().map(|p| p.uuid.as_str()).collect();
    write |= m.prune(now, &saved);
    if write && crate::fixture::get().is_none() {
        let _ = crate::wifimem::save(&m);
    }
    n.memory = std::sync::Arc::new(m);
}

#[cfg(test)]
mod mac_tests {
    use super::*;

    #[test]
    fn address_settings_read_write_and_check() {
        assert_eq!(MacMode::from_nm("stable", true), MacMode::Private);
        assert_eq!(MacMode::from_nm("random", true), MacMode::Random);
        assert_eq!(MacMode::from_nm("permanent", true), MacMode::Real);
        assert_eq!(MacMode::from_nm("", true), MacMode::Private, "Wi-Fi defaults to private");
        assert_eq!(MacMode::from_nm("", false), MacMode::Real, "wired keeps the real one by default");
        assert_eq!(MacMode::from_nm("02:1a:2b:3c:4d:5e", true), MacMode::Custom("02:1A:2B:3C:4D:5E".into()));
        assert_eq!(MacMode::Custom("02:1a:2b:3c:4d:5e".into()).to_nm(), "02:1A:2B:3C:4D:5E");
        assert!(valid_mac("02:1A:2B:3C:4D:5E"));
        assert!(!valid_mac("01:1A:2B:3C:4D:5E"), "a group address");
        assert!(!valid_mac("02:1A:2B:3C:4D"), "five pairs");
        assert!(!valid_mac("02:1A:2B:3C:4D:ZZ"));
        for _ in 0..20 {
            let r = random_mac();
            assert!(valid_mac(&r), "{r}");
            assert_eq!(u8::from_str_radix(&r[..2], 16).unwrap() & 0x03, 0x02, "locally administered, not a group: {r}");
        }
        assert_ne!(random_mac(), random_mac());
    }
}

// ---------------------------------------------------------------- Bluetooth (bluetoothctl)

#[derive(Clone, Debug, PartialEq)]
pub struct BtDevice {
    pub mac: String,
    pub name: String,
    pub connected: bool,
    pub kind: String,
    pub battery: Option<u8>,
}

/// `bluetoothctl devices [Paired]`: "Device AA:BB:.. Name".
pub fn bt_list(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| l.strip_prefix("Device "))
        .filter_map(|r| r.split_once(' ').map(|(m, n)| (m.to_string(), n.trim().to_string())))
        .collect()
}

/// Devices found nearby that are not paired yet (bluetoothctl lists both; nameless ones, shown only as their address, are left
/// out: nothing to recognise them by).
pub fn bt_nearby() -> Vec<(String, String)> {
    let paired: Vec<String> =
        bt_list(&run("bluetoothctl", &["devices", "Paired"]).unwrap_or_default()).into_iter().map(|(m, _)| m).collect();
    bt_list(&run("bluetoothctl", &["devices"]).unwrap_or_default())
        .into_iter()
        .filter(|(m, n)| !paired.contains(m) && !n.is_empty() && n.replace('-', ":") != *m)
        .collect()
}

/// Looks for devices nearby for `secs` seconds (in the background; bt_nearby lists what turned up).
pub fn bt_scan(secs: u32) {
    if let Ok(mut c) = Command::new("bluetoothctl")
        .args(["--timeout", &secs.to_string(), "scan", "on"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// The BlueZ object of a device: /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF (the first adapter).
pub fn bt_path(mac: &str) -> Option<String> {
    let adapter = std::fs::read_dir("/sys/class/bluetooth")
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.starts_with("hci") && !n.contains(':'))
        .min()?;
    (mac.len() == 17 && mac.chars().all(|c| c.is_ascii_hexdigit() || c == ':'))
        .then(|| format!("/org/bluez/{adapter}/dev_{}", mac.replace(':', "_")))
}

/// Pairs with a device, trusts it (it may connect by itself later) and connects. A code to compare or type is asked by
/// Yggdrasil's own agent (yggdrasil-polkit is BlueZ's default agent), not here.
pub fn bt_pair(mac: &str) -> Result<(), String> {
    let path = bt_path(mac).ok_or("not a device address")?;
    let ask = |args: &[&str]| -> Result<(), String> {
        let o = Command::new("busctl").args(["--timeout=90"]).args(args).output().map_err(|e| e.to_string())?;
        if o.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
        }
    };
    ask(&["call", "org.bluez", &path, "org.bluez.Device1", "Pair"])
        .map_err(|e| if e.contains("AlreadyExists") { String::new() } else { format!("Pairing did not work: {e}") })
        .or_else(|e| if e.is_empty() { Ok(()) } else { Err(e) })?;
    ask(&["set-property", "org.bluez", &path, "org.bluez.Device1", "Trusted", "b", "true"])?;
    ask(&["call", "org.bluez", &path, "org.bluez.Device1", "Connect"]).map_err(|e| format!("Paired, but it did not connect yet: {e}"))
}

/// `bluetoothctl info MAC`: (connected, icon kind, battery %).
pub fn bt_info(text: &str) -> (bool, String, Option<u8>) {
    let mut connected = false;
    let mut kind = String::new();
    let mut battery = None;
    for l in text.lines().map(str::trim) {
        if let Some(v) = l.strip_prefix("Connected:") {
            connected = v.trim() == "yes";
        } else if let Some(v) = l.strip_prefix("Icon:") {
            kind = v.trim().to_string();
        } else if let Some(v) = l.strip_prefix("Battery Percentage:") {
            // "0x5a (90)"
            battery = v.split('(').nth(1).and_then(|x| x.trim_end_matches(')').trim().parse().ok());
        }
    }
    (connected, kind, battery)
}

pub fn bt_powered(show: &str) -> Option<bool> {
    show.lines().map(str::trim).find_map(|l| l.strip_prefix("Powered:")).map(|v| v.trim() == "yes")
}

#[derive(Clone, Debug, Default)]
pub struct Bluetooth {
    pub present: bool,
    pub powered: bool,
    /// Paired devices other than game controllers.
    pub devices: Vec<BtDevice>,
    /// Game controllers: the ones connected now (USB or Bluetooth, from the kernel), the paired Bluetooth ones (connected or
    /// not), and the lights chosen for them (shared with the Controllers plugin, which puts them back on every connection).
    pub pads: Vec<gamepads::Pad>,
    pub paired_pads: Vec<BtDevice>,
    pub pad_settings: gamepads::Settings,
}

pub fn bluetooth() -> Bluetooth {
    let show = run("bluetoothctl", &["show"]).unwrap_or_default();
    let powered = bt_powered(&show);
    let (paired_pads, devices): (Vec<BtDevice>, Vec<BtDevice>) = bt_list(&run("bluetoothctl", &["devices", "Paired"]).unwrap_or_default())
        .into_iter()
        .map(|(mac, name)| {
            let (connected, kind, battery) = bt_info(&run("bluetoothctl", &["info", &mac]).unwrap_or_default());
            BtDevice { mac, name, connected, kind, battery }
        })
        .partition(|d| d.kind == "input-gaming");
    Bluetooth {
        present: powered.is_some(),
        powered: powered.unwrap_or(false),
        devices,
        pads: gamepads::list(),
        paired_pads,
        pad_settings: gamepads::Settings::load(),
    }
}

// ---------------------------------------------------------------- Sound (pactl, PipeWire)

#[derive(Clone, Debug, PartialEq)]
pub struct AudioDevice {
    pub name: String,
    pub description: String,
    pub volume: u32,
    pub muted: bool,
}

/// `pactl -f json list sinks` (or sources; monitors of outputs are left out).
pub fn audio_devices(json: &str) -> Vec<AudioDevice> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    v.as_array()
        .map(|a| {
            a.iter()
                .filter(|d| d["monitor_of_sink"].is_null() || d["monitor_of_sink"] == "n/a")
                .filter(|d| !d["name"].as_str().unwrap_or("").ends_with(".monitor"))
                .map(|d| {
                    let vol = d["volume"]
                        .as_object()
                        .and_then(|o| o.values().next())
                        .and_then(|c| c["value_percent"].as_str())
                        .and_then(|p| p.trim_end_matches('%').trim().parse().ok())
                        .unwrap_or(0);
                    AudioDevice {
                        name: d["name"].as_str().unwrap_or("").to_string(),
                        description: d["description"].as_str().unwrap_or("").to_string(),
                        volume: vol,
                        muted: d["mute"].as_bool().unwrap_or(false),
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
pub struct Sound {
    pub outputs: Vec<AudioDevice>,
    pub inputs: Vec<AudioDevice>,
    pub default_output: String,
    pub default_input: String,
}

pub fn sound() -> Sound {
    Sound {
        outputs: audio_devices(&run("pactl", &["-f", "json", "list", "sinks"]).unwrap_or_default()),
        inputs: audio_devices(&run("pactl", &["-f", "json", "list", "sources"]).unwrap_or_default()),
        default_output: run("pactl", &["get-default-sink"]).unwrap_or_default().trim().to_string(),
        default_input: run("pactl", &["get-default-source"]).unwrap_or_default().trim().to_string(),
    }
}

// ---------------------------------------------------------------- Displays (Hyprland and Yggdrasil's own compositor)

#[derive(Clone, Debug, PartialEq)]
pub struct Mode {
    pub id: String,
    pub width: u32,
    pub height: u32,
    pub refresh: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Output {
    pub name: String,
    /// The monitor's maker and model (empty when the compositor does not say): the main display is chosen by them.
    pub make: String,
    pub model: String,
    pub enabled: bool,
    /// The main display (yggdrasil_conf::display): the dock, notifications and the start menu are there.
    pub primary: bool,
    pub scale: f64,
    pub current: String,
    pub modes: Vec<Mode>,
    pub x: i64,
    pub y: i64,
    /// Turned: 0 none, 1 90°, 2 180°, 3 270° (wl_output's transforms; 4 to 7 are those flipped).
    pub transform: u32,
    /// The screen it shows a copy of (Duplicate), when it does.
    pub mirror_of: Option<String>,
    /// Variable refresh rate as monitors.lua sets it (0 off, 1 on, 2 for full-screen games); None: the session's own setting.
    pub vrr: Option<u8>,
}

impl Output {
    /// Its size in the space of all screens (logical pixels): the mode over the scale, turned when it is turned. A screen that is
    /// off and says no mode takes its largest one.
    pub fn logical(&self) -> (i32, i32) {
        let (w, h) = match self.current_mode().filter(|m| m.width > 0 && m.height > 0) {
            Some(m) => (m.width, m.height),
            None => self.resolutions().first().copied().unwrap_or((1920, 1080)),
        };
        let s = if self.scale.is_finite() && self.scale > 0.1 { self.scale } else { 1.0 };
        let (w, h) = ((w as f64 / s).round() as i32, (h as f64 / s).round() as i32);
        if self.transform % 2 == 1 {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// A mode to switch it on with: the one it has, else its largest at its fastest rate.
    pub fn mode_to_use(&self) -> String {
        match self.current_mode().filter(|m| m.width > 0 && m.height > 0) {
            Some(m) => m.id.clone(),
            None => self
                .resolutions()
                .first()
                .and_then(|&(w, h)| self.rates(w, h).first().map(|m| m.id.clone()))
                .unwrap_or_else(|| self.current.clone()),
        }
    }

    /// As the main display's choice sees it.
    pub fn seen(&self) -> yggdrasil_conf::display::Seen<'_> {
        yggdrasil_conf::display::Seen { connector: &self.name, make: &self.make, model: &self.model, x: self.x as i32, y: self.y as i32 }
    }

    pub fn current_mode(&self) -> Option<&Mode> {
        self.modes.iter().find(|m| m.id == self.current)
    }
    /// Distinct resolutions, largest first.
    pub fn resolutions(&self) -> Vec<(u32, u32)> {
        let mut r: Vec<(u32, u32)> = self.modes.iter().map(|m| (m.width, m.height)).collect();
        r.sort_by(|a, b| (b.0 * b.1).cmp(&(a.0 * a.1)).then(b.0.cmp(&a.0)));
        r.dedup();
        r
    }
    /// Modes of one resolution, fastest first.
    pub fn rates(&self, w: u32, h: u32) -> Vec<&Mode> {
        let mut r: Vec<&Mode> = self.modes.iter().filter(|m| m.width == w && m.height == h).collect();
        r.sort_by(|a, b| b.refresh.partial_cmp(&a.refresh).unwrap_or(std::cmp::Ordering::Equal));
        r
    }
}

#[derive(Clone, Debug, Default)]
pub struct Displays {
    pub outputs: Vec<Output>,
    pub tiling: String,
    pub caps: Caps,
}

/// What the compositor can do beyond modes, scales, places and switching a screen off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Caps {
    /// Turn a screen (portrait).
    pub transform: bool,
    /// Show a copy of another screen (Duplicate).
    pub mirror: bool,
    /// Variable refresh rate per screen.
    pub vrr: bool,
}

/// Hyprland does all of it (a fixture stands in for Hyprland); Yggdrasil's own compositor not yet.
fn caps() -> Caps {
    let hypr = crate::fixture::get().is_some() || hyprland() && !matches!(desk(), Some(yggdrasil_wm::Desk::Own(_)));
    Caps { transform: hypr, mirror: hypr, vrr: hypr }
}

pub fn displays() -> Displays {
    let tiling = yggdrasil_conf::tiling::outputs().join(",");
    Displays { outputs: current_outputs().unwrap_or_default(), tiling, caps: caps() }
}

/// The screens now, with the main display marked and what monitors.lua adds (variable refresh rate).
fn current_outputs() -> Option<Vec<Output>> {
    let main = yggdrasil_conf::display::load();
    let mut outs = outputs_from(&screens_all()?, main.as_ref());
    let lua = std::fs::read_to_string(monitors_path()).unwrap_or_default();
    for o in &mut outs {
        o.vrr = lua_value(&lua, &o.name, "vrr").and_then(|v| v.parse().ok()).filter(|v: &u8| *v <= 2);
    }
    Some(outs)
}

pub fn monitors_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("monitors.lua")
}

/// A value of the last `hl.monitor({ output = "NAME", ... })` line naming a screen (the form Settings writes, one per line).
fn lua_value(lua: &str, output: &str, key: &str) -> Option<String> {
    let named = format!("output = \"{output}\"");
    let line = lua.lines().rev().find(|l| l.trim_start().starts_with("hl.monitor(") && l.contains(&named))?;
    let inside = line.split_once('{')?.1.rsplit_once('}')?.0;
    inside.split(',').filter_map(|p| p.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().trim_matches('"').to_string())
}

/// The compositor (yggdrasil-wm: Hyprland or Yggdrasil's own), unless a made-up system (a fixture) stands in for the real one.
pub fn desk() -> Option<yggdrasil_wm::Desk> {
    if crate::fixture::get().is_some() {
        return None;
    }
    yggdrasil_wm::Desk::detect()
}

/// Every screen, switched-off ones too, with the modes it offers: from the compositor (a fixture answers it as `hyprctl -j
/// monitors all`).
fn screens_all() -> Option<Vec<yggdrasil_wm::Screen>> {
    if let Some(f) = crate::fixture::get() {
        return f.answer("hyprctl", &["-j", "monitors", "all"]).map(|j| yggdrasil_wm::hyprland::parse_monitors(&j));
    }
    yggdrasil_wm::Desk::detect().map(|d| d.screens_all())
}

/// The compositor reads its settings again (after Settings wrote them).
pub fn reload() -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    desk().ok_or("No compositor answered")?.reload()
}

/// The session's compositor is Hyprland (Yggdrasil's own session).
pub fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// The screens as the compositor lists them. Neither compositor has a "main" screen of its own: it is the one chosen here
/// (`main`, when that monitor is on), else the one at the top left of the space. A screen that does not say its refresh rate is
/// taken as 60 Hz, and its mode now is always among its modes.
pub fn outputs_from(screens: &[yggdrasil_wm::Screen], main: Option<&yggdrasil_conf::display::MainDisplay>) -> Vec<Output> {
    let mut outs: Vec<Output> = screens
        .iter()
        .map(|m| {
            let (w, h, r) = (m.width, m.height, m.refresh.unwrap_or(60.0));
            let mut modes: Vec<Mode> = m.modes.iter().filter_map(|x| parse_hypr_mode(x)).collect();
            let current = hypr_mode_id(w, h, r);
            if !modes.iter().any(|x| x.id == current) {
                modes.push(Mode { id: current.clone(), width: w, height: h, refresh: r });
            }
            Output {
                name: m.name.clone(),
                make: m.make.clone(),
                model: m.model.clone(),
                enabled: !m.disabled,
                primary: false,
                scale: m.scale.unwrap_or(1.0),
                current,
                modes,
                x: m.x as i64,
                y: m.y as i64,
                transform: m.transform,
                mirror_of: m.mirror_of.clone(),
                vrr: None,
            }
        })
        .collect();
    let on: Vec<usize> = (0..outs.len()).filter(|&i| outs[i].enabled && outs[i].mirror_of.is_none()).collect();
    let seen: Vec<yggdrasil_conf::display::Seen> = on.iter().map(|&i| outs[i].seen()).collect();
    if let Some(i) = yggdrasil_conf::display::pick(main, &seen).map(|k| on[k]) {
        outs[i].primary = true;
    }
    outs
}

/// Makes this monitor the main display: the dock, notifications and the start menu move there (the panel watches the file), and
/// it stays the main one on another port. Yggdrasil's own compositor reads it again too (its main screen: X11 programs' primary).
pub fn set_main_display(o: &Output) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    yggdrasil_conf::display::save(&yggdrasil_conf::display::MainDisplay::of(&o.seen())).map_err(|e| e.to_string())?;
    match desk() {
        Some(d @ yggdrasil_wm::Desk::Own(_)) => d.reload(),
        _ => Ok(()),
    }
}

fn hypr_mode_id(w: u32, h: u32, r: f64) -> String {
    format!("{w}x{h}@{r:.2}")
}

/// "3440x1440@180.00Hz" -> a mode.
fn parse_hypr_mode(s: &str) -> Option<Mode> {
    let (size, rate) = s.trim_end_matches("Hz").split_once('@')?;
    let (w, h) = size.split_once('x')?;
    let (w, h, r) = (w.parse().ok()?, h.parse().ok()?, rate.parse::<f64>().ok()?);
    Some(Mode { id: hypr_mode_id(w, h, r), width: w, height: h, refresh: r })
}

/// The monitor lines of `~/.config/yggdrasil/monitors.lua` for these screens (what Yggdrasil's Hyprland config and its own
/// compositor load; the compositor leaves out what it cannot do yet: mirror, transform, vrr).
pub fn hypr_monitors_lua(outs: &[Output]) -> String {
    let mut t = String::from("-- Written by Settings > Displays. Loaded by Yggdrasil's Hyprland config and its own compositor.\n");
    for o in outs {
        if !o.enabled {
            t.push_str(&format!("hl.monitor({{ output = \"{}\", disabled = true }})\n", o.name));
            continue;
        }
        let place = match &o.mirror_of {
            Some(of) => format!("position = \"auto\", scale = {}, mirror = \"{of}\"", o.scale),
            None => format!("position = \"{}x{}\", scale = {}", o.x, o.y, o.scale),
        };
        let mut extra = String::new();
        if o.transform != 0 {
            extra.push_str(&format!(", transform = {}", o.transform));
        }
        if let Some(v) = o.vrr {
            extra.push_str(&format!(", vrr = {v}"));
        }
        t.push_str(&format!("hl.monitor({{ output = \"{}\", mode = \"{}\", {place}{extra} }})\n", o.name, o.mode_to_use()));
    }
    t
}

/// monitors.lua before a change, to put back (None: there was none).
pub type Snapshot = Option<String>;

/// Changes the screens: `change` gets them as the compositor reports them now (the main display marked) and changes them; the
/// whole list is written to ~/.config/yggdrasil/monitors.lua and the compositor reads it again. Hyprland's session config and
/// Yggdrasil's own compositor both load that file on reload, so this is the same on both. The file as it was comes back, for
/// "Keep these display settings?".
pub fn set_screens(change: impl FnOnce(&mut Vec<Output>) -> Result<(), String>) -> Result<Snapshot, String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    let mut outs = current_outputs().ok_or("The compositor did not answer")?;
    change(&mut outs)?;
    let path = monitors_path();
    let before = std::fs::read_to_string(&path).ok();
    write_file(&path, &hypr_monitors_lua(&outs))?;
    reload()?;
    Ok(before)
}

/// Puts monitors.lua back as it was before a change (removed when there was none) and has the compositor read it.
pub fn restore_screens(before: Snapshot) -> Result<(), String> {
    let path = monitors_path();
    match before {
        Some(t) => write_file(&path, &t)?,
        None => {
            let _ = std::fs::remove_file(&path);
        }
    }
    reload()
}

/// Written beside and renamed: the compositor never reads half a file.
fn write_file(path: &std::path::Path, text: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("lua.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Shows each screen's number large on it for a few seconds (the panel draws them): "DP-1=1,DP-3=2".
pub fn identify(numbers: &[(String, u32)]) -> Result<(), String> {
    let list = numbers.iter().map(|(n, k)| format!("{n}={k}")).collect::<Vec<_>>().join(",");
    act("busctl", &["--user", "call", "org.yggdrasil.Panel", "/Panel", "org.yggdrasil.Panel", "Identify", "s", &list])
}

/// Turns tiling on or off for one screen: the shared list (yggdrasil_conf::tiling) and then whoever draws the windows.
pub fn set_tiling(name: &str, on: bool) -> Result<(), String> {
    let mut list = yggdrasil_conf::tiling::outputs();
    list.retain(|n| n != name);
    if on {
        list.push(name.to_string());
    }
    yggdrasil_conf::tiling::set_outputs(&list).map_err(|e| e.to_string())?;
    reload()
}

// ---------------------------------------------------------------- Appearance

pub const ACCENTS: [(&str, [u8; 3]); 10] = [
    ("Purple", [124, 84, 214]),
    ("Blue", [61, 132, 245]),
    ("Teal", [38, 162, 158]),
    ("Green", [52, 168, 83]),
    ("Yellow", [230, 176, 30]),
    ("Orange", [240, 120, 40]),
    ("Red", [224, 60, 70]),
    // Rosewood #65000b (2026-11-07), BlackArch's kind of deep red, beside LykanOS's teal; on a dark style its text is lifted
    ("Rosewood", [101, 0, 11]),
    ("Pink", [220, 90, 160]),
    ("White", [255, 255, 255]),
];

/// An icon pack (a freedesktop icon theme) that can be chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconPack {
    /// The folder name, which is what the settings store.
    pub id: String,
    pub name: String,
    /// Installed by the user (in ~/.local/share/icons), so it can be removed from here.
    pub user: bool,
    /// A few sample icons (folder, files, settings, a browser) to show what the pack looks like.
    pub preview: Vec<std::path::PathBuf>,
}

/// A mouse pointer theme (an X cursor theme: a folder with a cursors/ folder in it) that can be chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CursorTheme {
    /// The folder name, which is what the settings store.
    pub id: String,
    pub name: String,
    /// Installed by the user (in ~/.local/share/icons or ~/.icons).
    pub user: bool,
}

#[derive(Clone, Debug, Default)]
pub struct Appearance {
    pub icon_packs: Vec<IconPack>,
    /// The id of the pack in use.
    pub icon_theme: String,
    pub cursors: Vec<CursorTheme>,
    /// The id of the pointer theme in use.
    pub cursor: String,
    pub dark: bool,
    pub accent: Option<[u8; 3]>,
    pub wallpapers: Vec<std::path::PathBuf>,
    pub panel: yggdrasil_conf::PanelConf,
}

pub fn appearance() -> Appearance {
    let look = yggdrasil_conf::appearance::load();
    let accent = look.accent;
    let mut wallpapers = vec![];
    for dir in yggdrasil_conf::integrations::wallpaper_dirs() {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                // Plasma wallpaper packages keep their pictures in contents/images.
                let p = if p.is_dir() { best_image(&p.join("contents/images")).unwrap_or(p) } else { p };
                if p.extension().map(|x| matches!(x.to_str(), Some("png" | "jpg" | "jpeg" | "webp"))).unwrap_or(false) {
                    wallpapers.push(p);
                }
            }
        }
    }
    wallpapers.sort();
    Appearance {
        icon_packs: icon_packs(),
        icon_theme: current_icon_theme(),
        cursors: cursor_themes(),
        cursor: current_cursor(),
        dark: look.dark,
        accent,
        wallpapers,
        panel: yggdrasil_conf::PanelConf::load(),
    }
}

/// The largest picture in a Plasma wallpaper package's images folder (named like 3840x2160.png).
fn best_image(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(u64, std::path::PathBuf)> = None;
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        let stem = p.file_stem()?.to_string_lossy().to_string();
        let px = stem.split_once('x').and_then(|(w, h)| Some(w.parse::<u64>().ok()? * h.parse::<u64>().ok()?)).unwrap_or(0);
        if best.as_ref().map(|b| px > b.0).unwrap_or(true) {
            best = Some((px, p));
        }
    }
    best.map(|b| b.1)
}

// ---------------------------------------------------------------- Power

#[derive(Clone, Debug, Default)]
pub struct Power {
    pub profile: String,
    pub profiles: Vec<String>,
    /// The idle timers, in ~/.config/yggdrasil/idle.conf (yggdrasil-idle acts on them).
    pub idle: Idle,
    /// What the hardware allows and what is set for each mode (Developer options only).
    pub limits: Option<PowerLimits>,
    /// The graphics card unlock (Developer options, an NVIDIA card only).
    pub unlock: Option<GpuUnlock>,
    /// A laptop's battery; None on a computer without one (the Battery section is left out).
    pub battery: Option<Battery>,
    /// A laptop's fans and processor temperature, shown (not set: the firmware runs them) on a computer with a battery.
    pub laptop_cooling: Option<LaptopCooling>,
}

/// What a laptop's firmware is doing with its fans.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LaptopCooling {
    /// Each fan's speed in RPM, in hwmon's order.
    pub fans: Vec<i64>,
    /// The processor's temperature in °C (coretemp's package sensor, or k10temp's Tctl).
    pub cpu_temp: Option<i64>,
    /// The firmware's thermal profile (/sys/firmware/acpi/platform_profile: low-power, balanced, performance), which
    /// power-profiles-daemon switches with the power mode.
    pub thermal: Option<String>,
}

/// The processor's sensors: the only temperatures read (an NVMe's or a disk's wakes it, and the page reads every few seconds).
const CPU_HWMON: [&str; 2] = ["coretemp", "k10temp"];
/// A graphics card's drivers: their fans are the card's, not the laptop's, and reading them can wake a sleeping card.
const GPU_HWMON: [&str; 4] = ["amdgpu", "radeon", "nouveau", "xe"];

/// One hwmon folder's (name, fan speeds, the processor's temperature in °C).
pub fn read_hwmon(dir: &std::path::Path) -> (String, Vec<i64>, Option<i64>) {
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().map(|s| s.trim().to_string());
    let name = read("name").unwrap_or_default();
    if GPU_HWMON.contains(&name.as_str()) {
        return (name, vec![], None);
    }
    let fans = (1..=8).filter_map(|i| read(&format!("fan{i}_input")).and_then(|v| v.parse().ok())).collect();
    let temp = if CPU_HWMON.contains(&name.as_str()) {
        read("temp1_input").and_then(|v| v.parse::<i64>().ok()).map(|m| (m as f64 / 1000.0).round() as i64)
    } else {
        None
    };
    (name, fans, temp)
}

/// The fans to show: the laptop vendor's driver (thinkpad, dell_smm, asus...) rather than ACPI's generic copy of the same fan.
pub fn laptop_cooling_from(hwmons: &[(String, Vec<i64>, Option<i64>)], thermal: Option<String>) -> Option<LaptopCooling> {
    let vendor = hwmons.iter().find(|(n, f, _)| !f.is_empty() && n != "acpi_fan");
    let fans = vendor.or_else(|| hwmons.iter().find(|(_, f, _)| !f.is_empty())).map(|(_, f, _)| f.clone()).unwrap_or_default();
    let cpu_temp = hwmons.iter().find(|(n, _, t)| CPU_HWMON.contains(&n.as_str()) && t.is_some()).and_then(|(_, _, t)| *t);
    (!fans.is_empty() || cpu_temp.is_some()).then_some(LaptopCooling { fans, cpu_temp, thermal })
}

fn laptop_cooling() -> Option<LaptopCooling> {
    let mut dirs: Vec<std::path::PathBuf> = std::fs::read_dir("/sys/class/hwmon").ok()?.flatten().map(|e| e.path()).collect();
    dirs.sort();
    let hwmons: Vec<_> = dirs.iter().map(|d| read_hwmon(d)).collect();
    let thermal = std::fs::read_to_string("/sys/firmware/acpi/platform_profile").ok().map(|s| s.trim().to_string());
    laptop_cooling_from(&hwmons, thermal)
}

/// `/usr/lib/yggdrasil/battery json`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Battery {
    pub percent: i64,
    /// "2 h 10 min left", "Charging: full in 45 min"
    pub status: String,
    pub health: Option<i64>,
    pub cycles: Option<i64>,
    /// The charge limit in percent, when the battery has one.
    pub limit: Option<i64>,
}

pub fn parse_battery(json: &str) -> Option<Battery> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    v["present"].as_bool().filter(|p| *p)?;
    Some(Battery {
        percent: v["percent"].as_i64().unwrap_or(0),
        status: v["status"].as_str().unwrap_or_default().to_string(),
        health: v["health"].as_i64(),
        cycles: v["cycles"].as_i64(),
        limit: v["limit"].as_i64(),
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuLimits {
    pub name: String,
    pub default: i64,
    pub min: i64,
    pub max: i64,
    /// While the unlock is tried and measured, no mode may go above this (the card's default, or lower after a hot start).
    pub hold: Option<i64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PowerLimits {
    pub gpu: Option<GpuLimits>,
    /// Per mode: (processor percent, graphics watts), each None when not limited.
    pub modes: std::collections::BTreeMap<String, (Option<i64>, Option<i64>)>,
}

/// Parse `powerlimit get` (/usr/lib/yggdrasil/powerlimit).
pub fn parse_power_limits(json: &str) -> Option<PowerLimits> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let gpu = v["gpu"].as_object().map(|g| GpuLimits {
        name: g.get("name").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        default: g.get("default").and_then(|x| x.as_i64()).unwrap_or(0),
        min: g.get("min").and_then(|x| x.as_i64()).unwrap_or(0),
        max: g.get("max").and_then(|x| x.as_i64()).unwrap_or(0),
        hold: g.get("hold").and_then(|x| x.as_i64()),
    });
    let modes = v["limits"]
        .as_object()
        .map(|m| m.iter().map(|(k, l)| (k.clone(), (l["cpu"].as_i64(), l["gpu"].as_i64()))).collect())
        .unwrap_or_default();
    Some(PowerLimits { gpu, modes })
}

/// What `gpuunlock status --json` says (/usr/lib/yggdrasil/gpuunlock; the plan is Labyrinth OS docs/GPU_UNLOCK.md).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuUnlock {
    /// unsupported, supported, armed, unlocked, failed, throttled, secure-boot or no-nvidia
    pub state: String,
    pub reason: String,
    /// Why arming cannot start yet on a supported card (its boot program is not built, the boot menu cannot do one try here).
    pub blocker: Option<String>,
    pub since: Option<String>,
    pub ratio: Option<f64>,
    pub hold: bool,
}

impl GpuUnlock {
    /// Whether there is an unlock row at all: never for a card that is not in the support table (the reason stays in the
    /// helper's answer and its log).
    pub fn offered(&self) -> bool {
        !matches!(self.state.as_str(), "unsupported" | "no-nvidia" | "")
    }
}

pub fn parse_gpu_unlock(json: &str) -> Option<GpuUnlock> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let s = |k: &str| v[k].as_str().map(String::from);
    Some(GpuUnlock {
        state: s("state")?,
        reason: s("reason").unwrap_or_default(),
        blocker: s("arm_blocker"),
        since: s("since"),
        ratio: v["ratio"].as_f64(),
        hold: v["power_hold"].as_bool().unwrap_or(false),
    })
}

/// The helper's answer. A debug build reads it from $YGGDRASIL_GPUUNLOCK_FIXTURE instead, for tests and screenshots; release
/// builds ignore that, and arming checks everything again as root.
pub fn gpu_unlock() -> Option<GpuUnlock> {
    if cfg!(debug_assertions) {
        if let Ok(path) = std::env::var("YGGDRASIL_GPUUNLOCK_FIXTURE") {
            return std::fs::read_to_string(path).ok().and_then(|s| parse_gpu_unlock(&s));
        }
    }
    run("/usr/lib/yggdrasil/gpuunlock", &["status", "--json"]).and_then(|s| parse_gpu_unlock(&s))
}

/// The newest backup of Sarina on Proton Drive (its folder's name, a date), shown before arming the unlock as a reminder.
pub fn sarina_backup() -> Option<String> {
    let dir = std::path::Path::new(&std::env::var("HOME").ok()?).join("ProtonDrive/Documents/Github/Sarina/backups");
    std::fs::read_dir(dir).ok()?.flatten().filter(|e| e.path().is_dir()).filter_map(|e| e.file_name().into_string().ok()).max()
}

/// `powerprofilesctl list`: profile names ("* performance:" marks the active one).
pub fn profiles(text: &str) -> (Vec<String>, String) {
    let mut names = vec![];
    let mut active = String::new();
    for l in text.lines() {
        let t = l.trim();
        if t.ends_with(':') && !t.contains(' ') || t.starts_with("* ") && t.ends_with(':') {
            let name = t.trim_start_matches("* ").trim_end_matches(':').to_string();
            if t.starts_with("* ") {
                active = name.clone();
            }
            names.push(name);
        }
    }
    (names, active)
}

const PPD: [&str; 3] =
    ["org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles"];

/// The power modes and the active one, read from power-profiles-daemon over D-Bus. (`powerprofilesctl` is a Python program whose
/// interpreter aborts at exit now and then, which left core dumps; it is only the fallback.)
fn profiles_dbus() -> Option<(Vec<String>, String)> {
    let list = run("busctl", &["--json=short", "get-property", PPD[0], PPD[1], PPD[2], "Profiles"])?;
    let v: serde_json::Value = serde_json::from_str(&list).ok()?;
    let names: Vec<String> = v["data"].as_array()?.iter().filter_map(|p| p["Profile"]["data"].as_str().map(String::from)).collect();
    let active = run("busctl", &["--json=short", "get-property", PPD[0], PPD[1], PPD[2], "ActiveProfile"])
        .and_then(|a| serde_json::from_str::<serde_json::Value>(&a).ok())
        .and_then(|a| a["data"].as_str().map(String::from))
        .unwrap_or_default();
    if names.is_empty() {
        None
    } else {
        Some((names, active))
    }
}

/// Switches the power mode over D-Bus (the same permission check as powerprofilesctl).
pub fn set_profile(name: &str) -> Result<(), String> {
    act("busctl", &["set-property", PPD[0], PPD[1], PPD[2], "ActiveProfile", "s", name])
        .or_else(|_| act("powerprofilesctl", &["set", name]))
}

pub fn power() -> Power {
    let (profiles, active) = profiles_dbus().unwrap_or_else(|| profiles(&run("powerprofilesctl", &["list"]).unwrap_or_default()));
    let limits = if yggdrasil_conf::PanelConf::load().developer {
        run("/usr/lib/yggdrasil/powerlimit", &["get"]).and_then(|s| parse_power_limits(&s))
    } else {
        None
    };
    let unlock = if limits.as_ref().is_some_and(|l| l.gpu.is_some()) { gpu_unlock() } else { None };
    let battery = run("/usr/lib/yggdrasil/battery", &["json"]).and_then(|s| parse_battery(&s));
    let laptop_cooling = if battery.is_some() { laptop_cooling() } else { None };
    Power { profile: active, profiles, limits, unlock, idle: read_idle(), battery, laptop_cooling }
}

/// What to do as the user stays away, in minutes (0 = never), and whether this computer can sleep and hibernate.
#[derive(Clone, Debug, PartialEq)]
pub struct Idle {
    /// The falling characters, until you are back (minutes; 0 = never).
    pub screensaver: i64,
    pub dim: i64,
    /// How bright the screen is while dimmed, in percent.
    pub dim_level: i64,
    /// Keyboard and mouse lights go off while the screen is dimmed, off or showing the screensaver.
    pub lights_off: bool,
    /// The screen covered in black, the pointer hidden (minutes; 0 = never). Unlike turning the screen off, the monitor stays
    /// connected, so nothing has to come back from an unplug.
    pub blank: i64,
    pub screen_off: i64,
    pub lock: i64,
    pub sleep: i64,
    pub hibernate: i64,
    pub can_sleep: bool,
    pub can_hibernate: bool,
    /// Why sleeping is not possible, when something switched it off (a systemd setting).
    pub sleep_note: String,
}

impl Default for Idle {
    fn default() -> Self {
        Idle {
            screensaver: 0,
            dim: 5,
            dim_level: 30,
            lights_off: true,
            blank: 0,
            screen_off: 0,
            lock: 0,
            sleep: 0,
            hibernate: 0,
            can_sleep: true,
            can_hibernate: true,
            sleep_note: String::new(),
        }
    }
}

pub fn idle_conf_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("idle.conf")
}

/// The timers from idle.conf (seconds there, minutes here). Without the file the defaults apply: dim after 5 minutes, nothing else.
pub fn parse_idle(text: &str) -> Idle {
    let mut i = Idle::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        if k.trim() == "lights_off" {
            i.lights_off = !matches!(v.trim(), "0" | "no" | "false");
            continue;
        }
        let Ok(n) = v.trim().parse::<i64>() else { continue };
        match k.trim() {
            "screensaver" => i.screensaver = n / 60,
            "dim" => i.dim = n / 60,
            "dim_level" => i.dim_level = n.clamp(10, 90),
            "blank" => i.blank = n / 60,
            "display_off" => i.screen_off = n / 60,
            "lock" => i.lock = n / 60,
            "suspend" => i.sleep = n / 60,
            "hibernate" => i.hibernate = n / 60,
            _ => {}
        }
    }
    i
}

pub fn idle_text(i: &Idle) -> String {
    format!(
        "# Written by Settings > Power (lights_off: Peripherals > Keyboard). yggdrasil-idle acts on these (seconds of inactivity, 0 = never).\nmode = act\nscreensaver = {}\ndim = {}\ndim_level = {}\nlights_off = {}\nblank = {}\ndisplay_off = {}\nlock = {}\nsuspend = {}\nhibernate = {}\n",
        i.screensaver * 60, i.dim * 60, i.dim_level, i.lights_off as u8, i.blank * 60, i.screen_off * 60, i.lock * 60, i.sleep * 60, i.hibernate * 60
    )
}

fn can(what: &str) -> bool {
    run("busctl", &["call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", what])
        .map(|s| s.contains("\"yes\"") || s.contains("\"challenge\""))
        .unwrap_or(false)
}

pub fn read_idle() -> Idle {
    let mut i = std::fs::read_to_string(idle_conf_path()).map(|t| parse_idle(&t)).unwrap_or_default();
    i.can_sleep = can("CanSuspend");
    i.can_hibernate = can("CanHibernate");
    if !i.can_sleep || !i.can_hibernate {
        // a drop-in such as /etc/systemd/sleep.conf.d/10-never-sleep.conf that says AllowSuspend=no
        if let Ok(rd) = std::fs::read_dir("/etc/systemd/sleep.conf.d") {
            for e in rd.flatten() {
                let t = std::fs::read_to_string(e.path()).unwrap_or_default();
                if t.lines().any(|l| l.trim().starts_with("AllowSuspend=no") || l.trim().starts_with("AllowHibernation=no")) {
                    i.sleep_note = format!("Switched off by {}", e.path().display());
                }
            }
        }
    }
    i
}

/// Saves the timers (yggdrasil-idle reads them).
/// "Keyboard and mouse lights off while dimmed" (Settings > Peripherals > Keyboard): idle.conf's `lights_off`.
pub fn lights_off() -> bool {
    std::fs::read_to_string(idle_conf_path()).map(|t| parse_idle(&t).lights_off).unwrap_or(Idle::default().lights_off)
}

/// Sets `lights_off` alone; the timers in idle.conf stay as they are.
pub fn set_lights_off(on: bool) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    let mut i = std::fs::read_to_string(idle_conf_path()).map(|t| parse_idle(&t)).unwrap_or_default();
    i.lights_off = on;
    write_idle(&i)
}

pub fn write_idle(i: &Idle) -> Result<(), String> {
    let p = idle_conf_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, idle_text(i)).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod ini_tests {
    use super::*;

    #[test]
    fn reads_and_sets_one_value() {
        let t = "[General]\nAccentColor=1,2,3\n\n[Icons]\nTheme=Papirus\n";
        assert_eq!(ini_get(t, "Icons", "Theme").as_deref(), Some("Papirus"));
        assert_eq!(ini_get(t, "General", "Theme"), None);
        let s = ini_set(t, "Icons", "Theme", "Liki");
        assert_eq!(ini_get(&s, "Icons", "Theme").as_deref(), Some("Liki"));
        assert_eq!(ini_get(&s, "General", "AccentColor").as_deref(), Some("1,2,3"), "the rest stays");
        let s = ini_set("[General]\nA=1\n", "Icons", "Theme", "Liki");
        assert_eq!(s, "[General]\nA=1\n\n[Icons]\nTheme=Liki\n");
        let s = ini_set("[Icons]\n[Other]\nB=2\n", "Icons", "Theme", "X");
        assert_eq!(ini_get(&s, "Icons", "Theme").as_deref(), Some("X"));
        assert_eq!(ini_get(&s, "Other", "B").as_deref(), Some("2"));
        assert_eq!(ini_set("", "Settings", "gtk-icon-theme-name", "Liki"), "[Settings]\ngtk-icon-theme-name=Liki\n");
    }
}

#[cfg(test)]
mod idle_tests {
    use super::*;

    #[test]
    fn round_trip() {
        let i = Idle {
            screensaver: 3,
            dim: 7,
            dim_level: 40,
            lights_off: false,
            blank: 20,
            screen_off: 15,
            lock: 10,
            sleep: 0,
            hibernate: 120,
            can_sleep: true,
            can_hibernate: true,
            sleep_note: String::new(),
        };
        assert_eq!(parse_idle(&idle_text(&i)), i);
    }

    #[test]
    fn defaults_without_a_file() {
        let i = parse_idle("");
        assert_eq!((i.dim, i.dim_level, i.lock, i.sleep, i.lights_off), (5, 30, 0, 0, true));
    }
}

// ---------------------------------------------------------------- Firewall, date & time

pub fn firewall_on() -> bool {
    run("systemctl", &["is-active", "ufw"]).map(|s| s.trim() == "active").unwrap_or(false)
}

#[derive(Clone, Debug, Default)]
pub struct Time {
    pub timezone: String,
    pub ntp: bool,
    /// "Set time zone automatically": the user timer yggdrasil-autozone.timer is enabled.
    pub autozone: bool,
    /// The time zones to choose from (read once: zone.tab and the tz database do not change while Settings is open).
    pub catalog: std::sync::Arc<yggdrasil_tzpicker::Catalog>,
}

/// `timedatectl show`: Key=Value lines.
pub fn time_show(text: &str) -> (String, bool) {
    let kv: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
    (kv.get("Timezone").unwrap_or(&"").to_string(), kv.get("NTP").map(|v| *v == "yes").unwrap_or(false))
}

pub fn time() -> Time {
    let (timezone, ntp) = time_show(&run("timedatectl", &["show"]).unwrap_or_default());
    static CATALOG: std::sync::OnceLock<std::sync::Arc<yggdrasil_tzpicker::Catalog>> = std::sync::OnceLock::new();
    let catalog = CATALOG.get_or_init(|| std::sync::Arc::new(yggdrasil_tzpicker::Catalog::system(&timezone))).clone();
    let autozone = run("systemctl", &["--user", "is-enabled", "yggdrasil-autozone.timer"]).is_some_and(|s| s.trim() == "enabled");
    Time { timezone, ntp, autozone, catalog }
}

// ---------------------------------------------------------------- Apps (the operating system's optional software)

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub description: String,
    pub installed: bool,
}

/// The operating system's optional software (the components file its packages integration names), marked installed when all its
/// repo packages are (pacman -Q).
pub fn components(json: &str, installed: &dyn Fn(&str) -> bool) -> Vec<Component> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    v["components"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|c| {
                    let pkgs: Vec<&str> =
                        c["packages"].as_array().map(|p| p.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                    let pkgbuild = c["pkgbuild"].as_str();
                    let ok = if !pkgs.is_empty() { pkgs.iter().all(|p| installed(p)) } else { pkgbuild.map(installed).unwrap_or(false) };
                    Component {
                        id: c["id"].as_str().unwrap_or("").to_string(),
                        name: c["name"].as_str().unwrap_or("").to_string(),
                        description: c["description"].as_str().unwrap_or("").to_string(),
                        installed: ok,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn apps() -> Vec<Component> {
    let packages = yggdrasil_conf::integrations::first("packages");
    if packages.as_ref().and_then(|i| i.command("component")).is_none() {
        return vec![];
    }
    let json = packages.and_then(|i| i.path("components")).and_then(|p| std::fs::read_to_string(p).ok()).unwrap_or_default();
    components(&json, &|p| run("pacman", &["-Q", p]).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprland_screens_and_their_settings_file() {
        let j = r#"[{"name":"DP-3","width":2560,"height":2880,"refreshRate":59.95,"x":3440,"y":0,"scale":1.25,"disabled":false,"availableModes":["2560x2880@59.95Hz","1920x1080@60.00Hz"]},
                   {"name":"DP-1","width":3440,"height":1440,"refreshRate":180.0,"x":0,"y":0,"scale":1.0,"disabled":false,"availableModes":["3440x1440@180.00Hz","3440x1440@60.00Hz"]}]"#;
        let o = outputs_from(&yggdrasil_wm::hyprland::parse_monitors(j), None);
        let dp1 = o.iter().find(|x| x.name == "DP-1").unwrap();
        assert!(dp1.primary, "the screen at the top left is the main one");
        assert_eq!(dp1.current, "3440x1440@180.00");
        assert_eq!(dp1.current_mode().unwrap().refresh, 180.0);
        assert_eq!(dp1.rates(3440, 1440).len(), 2);
        // a screen of Yggdrasil's own compositor says no rate and no modes: 60 Hz, its one mode
        let own =
            outputs_from(&[yggdrasil_wm::own::parse_screen(0, "HEADLESS-1\t0,0\t1600x900\t1\t0,32,0,64\tfocused", Some(1)).unwrap()], None);
        assert_eq!((own[0].current.as_str(), own[0].modes.len(), own[0].primary), ("1600x900@60.00", 1, true));
        let lua = hypr_monitors_lua(&o);
        assert!(lua.contains(r#"hl.monitor({ output = "DP-3", mode = "2560x2880@59.95", position = "3440x0", scale = 1.25 })"#), "{lua}");
    }

    #[test]
    fn the_main_display_is_the_chosen_monitor_when_it_is_on() {
        use yggdrasil_conf::display::MainDisplay;
        // made-up monitors: a wide one at the left, a tall one at its right
        let j = |tall_port: &str, tall_on: bool| {
            format!(
                r#"[{{"name":"DP-1","make":"Example Display","model":"UW34","width":3440,"height":1440,"x":0,"y":0,"disabled":false}},
                   {{"name":"{tall_port}","make":"Sample Screens","model":"Tall27","width":2560,"height":2880,"x":3440,"y":0,"disabled":{}}}]"#,
                !tall_on
            )
        };
        let main = |o: &[Output]| o.iter().find(|x| x.primary).map(|x| x.name.clone());
        let screens = yggdrasil_wm::hyprland::parse_monitors(&j("DP-3", true));
        let o = outputs_from(&screens, None);
        assert_eq!(main(&o).as_deref(), Some("DP-1"), "no choice: the top left");
        let tall = o.iter().find(|x| x.name == "DP-3").unwrap();
        assert_eq!((tall.make.as_str(), tall.model.as_str()), ("Sample Screens", "Tall27"));
        let c = MainDisplay::of(&tall.seen());
        assert_eq!(main(&outputs_from(&screens, Some(&c))).as_deref(), Some("DP-3"), "the choice is kept");
        let moved = yggdrasil_wm::hyprland::parse_monitors(&j("HDMI-A-1", true));
        assert_eq!(main(&outputs_from(&moved, Some(&c))).as_deref(), Some("HDMI-A-1"), "and moves with the monitor");
        let off = yggdrasil_wm::hyprland::parse_monitors(&j("DP-3", false));
        assert_eq!(main(&outputs_from(&off, Some(&c))).as_deref(), Some("DP-1"), "switched off: the top left again");
    }

    #[test]
    fn copies_turns_vrr_and_screens_off_in_monitors_lua() {
        use crate::displays::tests::screen;
        let wide = Output { vrr: Some(2), ..screen("DP-1", (3440, 1440), (0, 0)) };
        let tall = Output { transform: 1, ..screen("DP-3", (2560, 2880), (3440, 0)) };
        let copy = Output { mirror_of: Some("DP-1".into()), ..screen("HDMI-A-1", (1920, 1080), (0, 0)) };
        let off = Output { enabled: false, ..screen("HDMI-A-2", (1920, 1080), (0, 0)) };
        let lua = hypr_monitors_lua(&[wide, tall, copy, off]);
        assert_eq!(
            lua.lines().skip(1).collect::<Vec<_>>(),
            [
                r#"hl.monitor({ output = "DP-1", mode = "3440x1440@60.00", position = "0x0", scale = 1, vrr = 2 })"#,
                r#"hl.monitor({ output = "DP-3", mode = "2560x2880@60.00", position = "3440x0", scale = 1, transform = 1 })"#,
                r#"hl.monitor({ output = "HDMI-A-1", mode = "1920x1080@60.00", position = "auto", scale = 1, mirror = "DP-1" })"#,
                r#"hl.monitor({ output = "HDMI-A-2", disabled = true })"#,
            ]
        );
        assert_eq!(lua_value(&lua, "DP-1", "vrr").as_deref(), Some("2"), "read back for the page");
        assert_eq!(lua_value(&lua, "DP-3", "vrr"), None);
        assert_eq!(lua_value(&lua, "HDMI-A-1", "mirror").as_deref(), Some("DP-1"));
        assert_eq!(
            lua_value("hl.monitor({ output = \"DP-1\", vrr = 1 })\nhl.monitor({ output = \"DP-1\", vrr = 0 })\n", "DP-1", "vrr").as_deref(),
            Some("0"),
            "the last line wins"
        );
    }

    #[test]
    fn the_screenshot_fixtures_have_three_displays_and_one() {
        let monitors = |fixture: &str| {
            let v: serde_json::Value = serde_json::from_str(fixture).unwrap();
            outputs_from(&yggdrasil_wm::hyprland::parse_monitors(v["run"]["hyprctl -j monitors all"].as_str().unwrap()), None)
        };
        let three = monitors(include_str!("../tests/fixtures/system.json"));
        assert_eq!(three.len(), 3);
        assert_eq!(three.iter().filter(|o| !o.enabled).count(), 1, "one of them disconnected");
        assert!(crate::displays::several(&three), "the arrangement box");
        let one = monitors(include_str!("../tests/fixtures/displays-single.json"));
        assert!(one.len() == 1 && !crate::displays::several(&one), "one display: no box, no Disconnect or Duplicate");
    }

    #[test]
    fn about_parsers() {
        assert_eq!(os_name("NAME=\"Some OS\"\nPRETTY_NAME=\"Some OS 1\"\n"), "Some OS 1");
        let labyrinth =
            "NAME=\"Labyrinth OS\"\nPRETTY_NAME=\"Labyrinth OS\"\nID=labyrinth\nVERSION=\"1.0\"\nVERSION_ID=1.0.0\nBUILD_ID=rolling\n";
        assert_eq!(os_name(labyrinth), "Labyrinth OS 1.0", "the system version from VERSION");
        assert_eq!(
            os_name("NAME=\"Labyrinth OS\"\nVERSION=\"1.1.1\"\nVERSION_ID=1.1.1\n"),
            "Labyrinth OS 1.1.1",
            "NAME when there is no PRETTY_NAME"
        );
        assert_eq!(
            os_name("NAME=\"Labyrinth OS\"\nPRETTY_NAME=\"Labyrinth OS\"\nID=labyrinth\n"),
            "Labyrinth OS",
            "an os-release without a version (before 1.0)"
        );
        let fedora = "NAME=\"Fedora Linux\"\nVERSION=\"40 (Workstation Edition)\"\nVERSION_ID=40\nPRETTY_NAME=\"Fedora Linux 40 (Workstation Edition)\"\n";
        assert_eq!(os_name(fedora), "Fedora Linux 40 (Workstation Edition)", "a PRETTY_NAME that names its version is not repeated");
        assert_eq!(os_name("NAME=\"Arch Linux\"\nPRETTY_NAME=\"Arch Linux\"\nBUILD_ID=rolling\n"), "Arch Linux");
        assert_eq!(os_logo("NAME=x\nLOGO=../../etc/passwd\n"), "", "an icon name, never a path");
        let cpu = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz\n";
        assert_eq!(cpu_model(cpu), "Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz × 2");
        assert!((mem_gib("MemTotal:       65775396 kB\n") - 62.73).abs() < 0.01);
        let lspci = "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"CoffeeLake-S GT2 [UHD Graphics 630]\" -r02 \"\" \"\"\n01:00.0 \"VGA compatible controller\" \"NVIDIA Corporation\" \"TU102 [GeForce RTX 2080 Ti Rev. A]\" -ra1 \"\" \"\"\n00:1f.3 \"Audio device\" \"Intel Corporation\" \"Cannon Lake PCH cAVS\" \"\" \"\"\n";
        assert_eq!(
            gpus(lspci),
            vec!["Intel Corporation CoffeeLake-S GT2 [UHD Graphics 630]", "NVIDIA Corporation TU102 [GeForce RTX 2080 Ti Rev. A]"]
        );
        assert_eq!(disk("     1B-blocks         Avail\n1000000000000 400000000000\n"), (1_000_000_000_000, 400_000_000_000));
    }

    #[test]
    fn wifi_parser_keeps_strongest_and_in_use_first() {
        let t = ":Cafe:40:WPA2\n*:Home\\:5G:70:WPA2\n:Cafe:80:WPA2\n:Open:30:\n::90:WPA2\n";
        let w = wifi_list(t);
        assert_eq!(w.iter().map(|x| x.ssid.as_str()).collect::<Vec<_>>(), vec!["Home:5G", "Cafe", "Open"]);
        assert!(w[0].in_use && w[0].secured);
        assert_eq!(w[1].signal, 80);
        assert!(!w[2].secured);
    }

    #[test]
    fn net_devices_parser() {
        let d =
            net_devices("eth0:ethernet:connected:Wired connection 1\nlo:loopback:connected (externally):lo\nwlan0:wifi:disconnected:\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].connection, "Wired connection 1");
        assert_eq!(d[1].kind, "wifi");
        // Wired shows only with a cable in
        assert!(cable_in("connected") && cable_in("disconnected") && cable_in("connecting (getting IP configuration)"));
        assert!(!cable_in("unavailable") && !cable_in("unmanaged"));
    }

    #[test]
    fn bluetooth_device_paths() {
        if std::path::Path::new("/sys/class/bluetooth/hci0").exists() {
            assert_eq!(bt_path("AA:BB:CC:DD:EE:0F").as_deref(), Some("/org/bluez/hci0/dev_AA_BB_CC_DD_EE_0F"));
        }
        assert_eq!(bt_path("not a mac"), None);
        assert_eq!(bt_path("AA:BB:CC:DD:EE:0F; rm"), None);
    }

    #[test]
    fn bluetooth_parsers() {
        assert_eq!(bt_list("Device 11:22:33:44:55:66 MX Mechanical\nDevice AA:BB:CC:DD:EE:FF Razer Basilisk\n").len(), 2);
        let (c, k, b) =
            bt_info("Device 11:22..\n\tName: MX Mechanical\n\tIcon: input-keyboard\n\tConnected: yes\n\tBattery Percentage: 0x5a (90)\n");
        assert!(c);
        assert_eq!(k, "input-keyboard");
        assert_eq!(b, Some(90));
        assert_eq!(bt_powered("Controller ..\n\tPowered: yes\n"), Some(true));
        assert_eq!(bt_powered(""), None);
    }

    #[test]
    fn audio_parser() {
        let j = r#"[{"name":"alsa_output.pci.analog","description":"Speakers","mute":false,"volume":{"front-left":{"value":32768,"value_percent":"50%","db":"-18 dB"}},"monitor_of_sink":null},
                    {"name":"alsa_output.pci.analog.monitor","description":"Monitor of Speakers","mute":false,"volume":{},"monitor_of_sink":"alsa_output.pci.analog"}]"#;
        let d = audio_devices(j);
        assert_eq!(d.len(), 1);
        assert_eq!(d[0].volume, 50);
        assert_eq!(d[0].description, "Speakers");
    }

    #[test]
    fn power_profiles_parser() {
        let (n, a) =
            profiles("  performance:\n    CpuDriver:\tintel_pstate\n\n* balanced:\n    CpuDriver:\tintel_pstate\n\n  power-saver:\n");
        assert_eq!(n, vec!["performance", "balanced", "power-saver"]);
        assert_eq!(a, "balanced");
    }

    #[test]
    fn only_the_processors_temperature_is_read_and_a_graphics_cards_fan_is_not_the_laptops() {
        let root = std::env::temp_dir().join(format!("ygg-hwmon-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mk = |d: &str, files: &[(&str, &str)]| {
            std::fs::create_dir_all(root.join(d)).unwrap();
            for (f, v) in files {
                std::fs::write(root.join(d).join(f), format!("{v}\n")).unwrap();
            }
            read_hwmon(&root.join(d))
        };
        // reading an NVMe's or a disk's temperature wakes it, and the page reads every few seconds
        assert_eq!(mk("hwmon0", &[("name", "nvme"), ("temp1_input", "38850")]), ("nvme".to_string(), vec![], None));
        assert_eq!(mk("hwmon1", &[("name", "drivetemp"), ("temp1_input", "31000")]).2, None);
        assert_eq!(mk("hwmon2", &[("name", "coretemp"), ("temp1_input", "57400")]).2, Some(57));
        // a dGPU's hwmon (sorted before the vendor driver's) is not the laptop's fan
        let gpu = mk("hwmon3", &[("name", "amdgpu"), ("fan1_input", "2100"), ("temp1_input", "45000")]);
        assert_eq!((gpu.1.len(), gpu.2), (0, None));
        let tp = mk("hwmon4", &[("name", "thinkpad"), ("fan1_input", "6329"), ("fan2_input", "5917")]);
        assert_eq!(tp.1, vec![6329, 5917]);
        let c = laptop_cooling_from(&[gpu, tp], None).unwrap();
        assert_eq!(c.fans, vec![6329, 5917]);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_laptops_fans_come_from_its_vendor_driver() {
        let h = |n: &str, f: &[i64], t: Option<i64>| (n.to_string(), f.to_vec(), t);
        let hwmons = [
            h("acpi_fan", &[6300], None),
            h("acpitz", &[], Some(56)),
            h("thinkpad", &[6329, 5917], Some(56)),
            h("coretemp", &[], Some(57)),
        ];
        let c = laptop_cooling_from(&hwmons, Some("balanced".into())).unwrap();
        assert_eq!((c.fans, c.cpu_temp, c.thermal.as_deref()), (vec![6329, 5917], Some(57), Some("balanced")));
        assert_eq!(laptop_cooling_from(&[h("nvme", &[], None)], None), None);
    }

    #[test]
    fn battery_parser() {
        let b = parse_battery(r#"{"present":true,"percent":76,"status":"Charging: full in 59 min","health":86,"cycles":209,"limit":80}"#);
        assert_eq!(b.map(|b| (b.percent, b.health, b.limit)), Some((76, Some(86), Some(80))));
        assert_eq!(parse_battery(r#"{"present":false}"#), None);
    }

    #[test]
    fn time_parser() {
        assert_eq!(time_show("Timezone=America/Denver\nLocalRTC=no\nNTP=yes\n"), ("America/Denver".to_string(), true));
    }

    #[test]
    fn components_parser() {
        let j = r#"{"components":[{"id":"steam","name":"Steam","description":"Games","packages":["steam","gamemode"]},{"id":"sarina","name":"Sarina","description":"Agent","pkgbuild":"sarina-git"}]}"#;
        let c = components(j, &|p| p == "steam" || p == "gamemode");
        assert!(c[0].installed);
        assert!(!c[1].installed);
    }
}

// ---------------------------------------------------------------- Icon packs

fn user_icons_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share/icons"))
}

/// `Key=value` from the `[Icon Theme]` section of an index.theme.
fn theme_key(index: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in index.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            inside = l == "[Icon Theme]";
        } else if inside {
            if let Some((k, v)) = l.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// One sample icon from a pack, by name (the pack's own files only, not what it inherits).
fn theme_icon(theme: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for ctx in ["places", "apps", "devices", "mimetypes"] {
        for size in ["scalable", "512", "256", "128", "96", "64", "48", "32"] {
            for dir in [theme.join(ctx).join(size), theme.join(size).join(ctx), theme.join(format!("{size}x{size}")).join(ctx)] {
                for ext in ["svg", "png"] {
                    let p = dir.join(format!("{name}.{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

/// Up to `n` icon files from a pack, whatever it has (by size folder, then name), for a preview.
fn any_icons(theme: &std::path::Path, n: usize) -> Vec<std::path::PathBuf> {
    let mut out = vec![];
    let mut dirs: Vec<std::path::PathBuf> = vec![];
    if let Ok(rd) = std::fs::read_dir(theme) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                dirs.push(p.clone());
                if let Ok(sub) = std::fs::read_dir(&p) {
                    dirs.extend(sub.flatten().map(|s| s.path()).filter(|s| s.is_dir()));
                }
            }
        }
    }
    dirs.sort();
    for d in dirs {
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&d)
            .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "png" || x == "svg").unwrap_or(false)).collect())
            .unwrap_or_default();
        files.sort();
        for f in files {
            out.push(f);
            if out.len() >= n {
                return out;
            }
        }
    }
    out
}

/// Every icon pack on the system and in the user's own folder, user ones first. Cursor themes, hidden
/// themes and the empty base themes are left out.
pub fn icon_packs() -> Vec<IconPack> {
    let mut out: Vec<IconPack> = vec![];
    let roots = [(user_icons_dir(), true), (Some(std::path::PathBuf::from("/usr/share/icons")), false)];
    for (root, user) in roots {
        let Some(root) = root else { continue };
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let id = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let Ok(index) = std::fs::read_to_string(d.join("index.theme")) else { continue };
            if theme_key(&index, "Hidden").map(|h| h.eq_ignore_ascii_case("true")).unwrap_or(false)
                || theme_key(&index, "Directories").is_none()
            {
                continue;
            }
            // KDE's, GNOME's and the toolkit's own packs are not choices: Yggdrasil's pack is Liki (they stay installed only because KDE programs need them)
            let lower = id.to_lowercase();
            if matches!(id.as_str(), "default" | "hicolor" | "locolor")
                || lower.starts_with("breeze")
                || lower.starts_with("klassy")
                || lower.starts_with("adwaita")
                || lower == "highcontrast"
            {
                continue;
            }
            let samples = [
                "folder",
                "system-file-manager",
                "file-manager",
                "minos",
                "steam",
                "spotify",
                "vivaldi",
                "firefox",
                "preferences-system",
                "web-browser",
            ];
            if let Some(have) = out.iter_mut().find(|p| p.id == id) {
                // the same pack is in the user's folder and the system's (the user's adds to it): borrow samples from the system copy
                for n in samples {
                    if have.preview.len() >= 4 {
                        break;
                    }
                    if let Some(p) = theme_icon(&d, n) {
                        if !have.preview.contains(&p) {
                            have.preview.push(p);
                        }
                    }
                }
                continue;
            }
            let name = theme_key(&index, "Name").filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            let mut preview: Vec<std::path::PathBuf> = samples.iter().filter_map(|n| theme_icon(&d, n)).take(4).collect();
            if preview.len() < 4 {
                // a pack with none of the usual sample names (an imported Android pack): show whatever it has
                for p in any_icons(&d, 4 - preview.len()) {
                    if !preview.contains(&p) {
                        preview.push(p);
                    }
                }
            }
            out.push(IconPack { id, name, user, preview });
        }
    }
    out
}

/// One value from an INI-style settings file (`[group]`, `key=value`), such as GTK's settings.ini.
pub fn ini_get(text: &str, group: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            inside = t == format!("[{group}]");
        } else if inside {
            if let Some((k, v)) = t.split_once('=') {
                if k.trim() == key {
                    return Some(v.trim().to_string());
                }
            }
        }
    }
    None
}

/// The same file's text with one value set (the group and the key are added when missing; everything else is kept).
pub fn ini_set(text: &str, group: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> = vec![];
    let (mut inside, mut done, mut seen_group) = (false, false, false);
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with('[') {
            if inside && !done {
                out.push(format!("{key}={value}"));
                done = true;
            }
            inside = t == format!("[{group}]");
            seen_group |= inside;
        } else if inside && !done {
            if let Some((k, _)) = t.split_once('=') {
                if k.trim() == key {
                    out.push(format!("{key}={value}"));
                    done = true;
                    continue;
                }
            }
        }
        out.push(line.to_string());
    }
    if !done {
        if !seen_group {
            if out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
                out.push(String::new());
            }
            out.push(format!("[{group}]"));
        }
        out.push(format!("{key}={value}"));
    }
    out.join("\n") + "\n"
}

/// ~/.config (or $XDG_CONFIG_HOME).
pub fn config_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// Every pointer theme installed, Yggdrasil's own first, then the user's, then the system's. "default" (only a pointer to
/// another theme) is left out.
pub fn cursor_themes() -> Vec<CursorTheme> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let roots =
        [(user_icons_dir(), true), (home.map(|h| h.join(".icons")), true), (Some(std::path::PathBuf::from("/usr/share/icons")), false)];
    let mut out: Vec<CursorTheme> = vec![];
    for (root, user) in roots {
        let Some(root) = root else { continue };
        let Ok(rd) = std::fs::read_dir(&root) else { continue };
        let mut dirs: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.join("cursors").is_dir()).collect();
        dirs.sort();
        for d in dirs {
            let id = d.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if id == "default" || id.starts_with('.') || out.iter().any(|c| c.id == id) {
                continue;
            }
            let name = std::fs::read_to_string(d.join("index.theme"))
                .ok()
                .and_then(|t| theme_key(&t, "Name"))
                .filter(|n| !n.trim().is_empty())
                .unwrap_or_else(|| id.clone());
            out.push(CursorTheme { id, name, user });
        }
    }
    out.sort_by_key(|c| (c.id != DEFAULT_CURSOR, !c.user));
    out
}

pub const DEFAULT_CURSOR: &str = "Yggdrasil";

/// The pointer theme in use: `cursor` in ~/.config/yggdrasil/appearance.conf (ygg-theme writes it), Yggdrasil's own when none.
pub fn current_cursor() -> String {
    let text = std::fs::read_to_string(config_dir().join("yggdrasil/appearance.conf")).unwrap_or_default();
    cursor_in(&text).unwrap_or_else(|| DEFAULT_CURSOR.to_string())
}

fn cursor_in(text: &str) -> Option<String> {
    text.lines()
        .filter_map(|l| l.split('#').next()?.split_once('='))
        .find(|(k, _)| k.trim() == "cursor")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod screen_tests {
    //! Settings > Displays on Yggdrasil's own compositor: a stand-in for its control socket in a runtime folder of the test's own
    //! (the desktop's is never named, and Hyprland's variable is taken away), and monitors.lua written in a temporary config
    //! folder.
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_screen_is_changed_on_our_own_compositor_as_on_hyprland() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::env::temp_dir().join(format!("ygg-settings-screens-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("run")).unwrap();
        let listener = UnixListener::bind(d.join("run/yggdrasil-compositor-wayland-test.ctl")).unwrap();
        let asked: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = asked.clone();
        std::thread::spawn(move || {
            for c in listener.incoming() {
                let Ok(mut c) = c else { break };
                let mut line = String::new();
                if BufReader::new(&c).read_line(&mut line).is_err() {
                    continue;
                }
                let cmd = line.trim().to_string();
                let answer = match cmd.as_str() {
                    "workspaces" => "1\t1\tshown\nok\n",
                    "screens all" => {
                        "HEADLESS-1\t0,0\t1280x720\t1\t0,0,0,0\tfocused\t60.00\t1280x720@60.00Hz,1920x1080@60.00Hz\ton\n\
                         HEADLESS-2\t1280,0\t1600x900\t1.5\t0,0,0,0\t-\t59.95\t1600x900@59.95Hz\ton\nok\n"
                    }
                    _ => "ok\n",
                };
                log.lock().unwrap().push(cmd);
                let _ = c.write_all(answer.as_bytes());
            }
        });
        let saved: Vec<(&str, Option<std::ffi::OsString>)> =
            ["XDG_RUNTIME_DIR", "WAYLAND_DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "XDG_CONFIG_HOME"]
                .iter()
                .map(|k| (*k, std::env::var_os(k)))
                .collect();
        std::env::set_var("XDG_RUNTIME_DIR", d.join("run"));
        std::env::set_var("WAYLAND_DISPLAY", "wayland-test");
        std::env::remove_var("HYPRLAND_INSTANCE_SIGNATURE");
        std::env::set_var("XDG_CONFIG_HOME", d.join("config"));
        assert!(matches!(super::desk(), Some(yggdrasil_wm::Desk::Own(_))), "the stand-in is the compositor found");
        let set = |name: &str, mode: Option<&str>, scale: Option<f64>| {
            let (name, mode) = (name.to_string(), mode.map(str::to_string));
            super::set_screens(move |outs| {
                let o = outs.iter_mut().find(|o| o.name == name).ok_or("that screen is not there")?;
                if let Some(m) = mode {
                    o.current = m;
                }
                if let Some(s) = scale {
                    o.scale = s;
                }
                Ok(())
            })
        };
        let mode = set("HEADLESS-1", Some("1920x1080@60.00"), None);
        let before = std::fs::read_to_string(d.join("config/yggdrasil/monitors.lua")).ok();
        let scale = set("HEADLESS-2", None, Some(2.0));
        let missing = set("HDMI-A-9", None, Some(1.0));
        let lua = std::fs::read_to_string(d.join("config/yggdrasil/monitors.lua")).unwrap_or_default();
        // "Keep these display settings?" not answered: the file as it was before the first change (none) comes back
        let undone = super::restore_screens(None);
        let gone = !d.join("config/yggdrasil/monitors.lua").exists();
        for (k, v) in saved {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        let _ = std::fs::remove_dir_all(&d);
        assert_eq!(mode, Ok(None), "no longer refused on our own compositor; there was no file before");
        assert_eq!(scale, Ok(before), "the file as it was comes back, to put back");
        assert!(missing.is_err());
        assert_eq!(
            lua.lines().skip(1).collect::<Vec<_>>(),
            [
                r#"hl.monitor({ output = "HEADLESS-1", mode = "1280x720@60.00", position = "0x0", scale = 1 })"#,
                r#"hl.monitor({ output = "HEADLESS-2", mode = "1600x900@59.95", position = "1280x0", scale = 2 })"#,
            ],
            "the last change holds what the compositor reports now, with the change; the form both compositors read"
        );
        assert!(undone.is_ok() && gone, "put back: no file, as before");
        let asked = asked.lock().unwrap().clone();
        assert_eq!(asked.iter().filter(|c| *c == "reload").count(), 3, "read again after each change and the undo: {asked:?}");
    }
}

#[cfg(test)]
mod cursor_tests {
    #[test]
    fn cursor_from_appearance() {
        assert_eq!(super::cursor_in("style = dark\ncursor = macOS\n").as_deref(), Some("macOS"));
        assert_eq!(super::cursor_in("style = dark\n# cursor = x\n"), None);
    }
}

/// Switch the mouse pointer for the whole desktop (ygg-theme writes every place programs read it from, and the compositor
/// switches at once).
pub fn set_cursor(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return Err("not a pointer theme name".into());
    }
    act("ygg-theme", &["cursor", id])
}

/// The icon pack in use (`icons` in ~/.config/yggdrasil/appearance.conf; the panel, Minos and ygg-theme read it too).
pub fn current_icon_theme() -> String {
    yggdrasil_conf::appearance::icon_pack().unwrap_or_default()
}

fn ini_write(path: &std::path::Path, group: &str, key: &str, value: &str) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let tmp = path.with_extension("ygg-tmp");
    std::fs::write(&tmp, ini_set(&old, group, key, value)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Choose an icon pack for the whole desktop (the panel, Minos, GTK and Qt programs follow it): the desktop's own record, then
/// GTK's settings here; `ygg-theme apply` tells the rest (X11 programs, and KDE-library programs when any are installed).
pub fn set_icon_pack(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return Err("not an icon pack name".into());
    }
    let cfg = config_dir();
    yggdrasil_conf::appearance::set_icon_pack(id).map_err(|e| e.to_string())?;
    for gtk in ["gtk-3.0", "gtk-4.0"] {
        ini_write(&cfg.join(gtk).join("settings.ini"), "Settings", "gtk-icon-theme-name", id)?;
    }
    // GTK 4 and portal-aware programs read it from GNOME's settings store, when there is one
    let _ = std::process::Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "icon-theme", id])
        .stdin(std::process::Stdio::null())
        .status();
    let _ = std::process::Command::new("ygg-theme")
        .arg("apply")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .status();
    Ok(())
}

fn safe_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.chars().all(|c| c.is_alphanumeric() || matches!(c, '-' | '_' | '.' | ' ' | '+'))
}

fn copy_tree(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let (from, to) = (e.path(), dst.join(e.file_name()));
        let t = e.file_type()?;
        if t.is_dir() {
            copy_tree(&from, &to)?;
        } else if t.is_file() {
            std::fs::copy(&from, &to)?;
        } else if t.is_symlink() {
            // a link inside the pack stays a link only when it points inside the pack (relative); others are skipped
            if let Ok(target) = std::fs::read_link(&from) {
                if target.is_relative() && !target.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
                    let _ = std::os::unix::fs::symlink(target, &to);
                }
            }
        }
    }
    Ok(())
}

/// Install an icon pack from a folder, an archive (.tar.gz, .tar.xz, .tar.bz2, .zip) or an https link to one.
/// It goes into ~/.local/share/icons, so nothing needs root. Returns the names of the packs installed.
pub fn install_icon_pack(src: &str) -> Result<Vec<String>, String> {
    let src = src.trim();
    let dest_root = user_icons_dir().ok_or("no home folder")?;
    let tmp = std::env::temp_dir().join(format!("yggdrasil-icons-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let result = (|| -> Result<Vec<String>, String> {
        let local: std::path::PathBuf = if src.starts_with("https://") {
            let file = tmp.join("download");
            act("curl", &["-fsSL", "--max-time", "300", "--max-filesize", "500000000", "-o", &file.to_string_lossy(), src])?;
            file
        } else if let Some(rest) = src.strip_prefix("~/") {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(rest)
        } else if let Some(rest) = src.strip_prefix("file://") {
            std::path::PathBuf::from(rest)
        } else {
            std::path::PathBuf::from(src)
        };
        if !local.exists() {
            return Err("That path does not exist.".into());
        }
        let unpacked = tmp.join("unpacked");
        std::fs::create_dir_all(&unpacked).map_err(|e| e.to_string())?;
        if local.is_dir() {
            copy_tree(&local, &unpacked.join(local.file_name().unwrap_or_default())).map_err(|e| e.to_string())?;
        } else if local.extension().map(|e| e.eq_ignore_ascii_case("apk")).unwrap_or(false) {
            // an Android icon pack: only read (never run) and turned into a theme for the installed programs
            act("liki-icon-import", &[&local.to_string_lossy(), "--out", &unpacked.to_string_lossy()])?;
        } else {
            // bsdtar reads zip and every tar flavour, and refuses paths that climb out of the folder
            act("bsdtar", &["-xf", &local.to_string_lossy(), "-C", &unpacked.to_string_lossy()])
                .map_err(|e| format!("Could not open it as an archive: {e}"))?;
        }
        // packs are the folders with an index.theme: at the top, or one level down (an archive of several packs)
        let mut found: Vec<std::path::PathBuf> = vec![];
        let mut stack = vec![(unpacked.clone(), 0)];
        while let Some((d, depth)) = stack.pop() {
            if d != unpacked && d.join("index.theme").is_file() {
                found.push(d);
                continue;
            }
            if depth < 2 {
                if let Ok(rd) = std::fs::read_dir(&d) {
                    stack.extend(rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).map(|p| (p, depth + 1)));
                }
            }
        }
        if found.is_empty() {
            return Err("No icon pack found there (an icon pack is a folder with an index.theme file).".into());
        }
        std::fs::create_dir_all(&dest_root).map_err(|e| e.to_string())?;
        let mut names = vec![];
        for pack in found {
            let name = pack.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if !safe_name(&name) {
                return Err(format!("The folder name {name:?} is not usable as an icon pack name."));
            }
            let target = dest_root.join(&name);
            if target.exists() {
                std::fs::remove_dir_all(&target).map_err(|e| e.to_string())?;
            }
            copy_tree(&pack, &target).map_err(|e| e.to_string())?;
            names.push(name);
        }
        let _ = Command::new("gtk-update-icon-cache").arg("-q").arg("-t").arg(&dest_root).status();
        Ok(names)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    result
}

/// Remove an icon pack the user installed (never a system one).
pub fn remove_icon_pack(id: &str) -> Result<(), String> {
    if !safe_name(id) {
        return Err("not an icon pack name".into());
    }
    let dir = user_icons_dir().ok_or("no home folder")?.join(id);
    let meta = std::fs::symlink_metadata(&dir).map_err(|_| "That pack is not in your own icons folder.".to_string())?;
    if !meta.is_dir() {
        return Err("That pack is not in your own icons folder.".into());
    }
    std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())
}

#[cfg(test)]
mod limit_tests {
    use super::*;

    #[test]
    fn power_limits_parse() {
        let l = parse_power_limits(r#"{"cpu_max_khz":5000000,"gpu":{"name":"RTX","watts":260,"default":260,"min":130,"max":290},"active":"balanced","limits":{"balanced":{"cpu":80,"gpu":200},"power-saver":{"cpu":50}}}"#).unwrap();
        assert_eq!(l.gpu.as_ref().unwrap().max, 290);
        assert_eq!(l.modes["balanced"], (Some(80), Some(200)));
        assert_eq!(l.modes["power-saver"], (Some(50), None));
        assert!(parse_power_limits(r#"{"gpu":null,"limits":{}}"#).unwrap().gpu.is_none());
        assert!(parse_power_limits("nope").is_none());
        assert_eq!(l.gpu.as_ref().unwrap().hold, None);
        let held = parse_power_limits(
            r#"{"gpu":{"name":"RTX","watts":260,"default":260,"min":130,"max":290,"hold":260},"limits":{"performance":{"gpu":290}}}"#,
        )
        .unwrap();
        assert_eq!(held.gpu.unwrap().hold, Some(260), "the slider stops at the held watts");
    }

    #[test]
    fn gpu_unlock_parse_and_when_a_row_is_shown() {
        // this computer: an MSI card that is not in the support table, so no row; the reason is kept for the log
        let msi = parse_gpu_unlock(r#"{"gpu":{"name":"NVIDIA GeForce RTX 2080 Ti","device":"10de:1e07","subsystem":"1462:3711","vbios":"90.02.17.00.3E"},"secure_boot":false,"state":"unsupported","reason":"No tested entry for MSI 1462:3711 with VBIOS 90.02.17.00.3E (tested: ASUS 1043:866a, VBIOS 90.02.17.00.47)","arm_blocker":null,"last_attempt":null,"since":null,"ratio":null,"power_hold":false,"log":"/var/lib/yggdrasil/gpu-unlock/log"}"#).unwrap();
        assert!(!msi.offered());
        assert!(msi.reason.contains("1462:3711") && msi.reason.contains("90.02.17.00.3E"));
        let state = |s: &str| {
            parse_gpu_unlock(&format!(
                r#"{{"state":"{s}","reason":"r","arm_blocker":null,"since":"2026-10-27T09:12:00","ratio":2.0,"power_hold":true}}"#
            ))
            .unwrap()
        };
        assert!(!state("no-nvidia").offered());
        for s in ["supported", "armed", "unlocked", "failed", "throttled", "secure-boot"] {
            assert!(state(s).offered(), "{s}");
        }
        let u = state("unlocked");
        assert_eq!((u.ratio, u.hold, u.since.as_deref()), (Some(2.0), true, Some("2026-10-27T09:12:00")));
        let blocked = parse_gpu_unlock(r#"{"state":"supported","reason":"r","arm_blocker":"not built"}"#).unwrap();
        assert_eq!(blocked.blocker.as_deref(), Some("not built"));
        assert!(parse_gpu_unlock(r#"{"reason":"no state"}"#).is_none());
        assert!(parse_gpu_unlock("nope").is_none());
    }
}

#[cfg(test)]
mod icon_tests {
    use super::*;

    #[test]
    fn index_keys_and_names() {
        let idx = "[Icon Theme]\nName=My Pack\nDirectories=apps/scalable\nHidden=false\n[apps/scalable]\nSize=128\n";
        assert_eq!(theme_key(idx, "Name").as_deref(), Some("My Pack"));
        assert_eq!(theme_key(idx, "Directories").as_deref(), Some("apps/scalable"));
        assert_eq!(theme_key(idx, "Size"), None, "only the [Icon Theme] section counts");
        assert!(safe_name("MacTahoe-purple") && safe_name("Pack 2.0") && !safe_name("../x") && !safe_name(".hidden") && !safe_name("a/b"));
    }

    #[test]
    fn a_folder_pack_installs_into_the_home_folder_and_can_be_removed() {
        let _g = crate::ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let home = std::env::temp_dir().join(format!("lf-icons-home-{}", std::process::id()));
        let src = std::env::temp_dir().join(format!("lf-icons-src-{}/TestPack", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(src.join("apps/scalable")).unwrap();
        std::fs::write(src.join("index.theme"), "[Icon Theme]\nName=Test Pack\nDirectories=apps/scalable\n").unwrap();
        std::fs::write(src.join("apps/scalable/x.svg"), "<svg xmlns='http://www.w3.org/2000/svg'/>").unwrap();
        std::env::set_var("HOME", &home);
        assert_eq!(install_icon_pack(&src.to_string_lossy()).unwrap(), vec!["TestPack".to_string()]);
        assert!(home.join(".local/share/icons/TestPack/apps/scalable/x.svg").is_file());
        let packs = icon_packs();
        assert!(packs.iter().any(|p| p.id == "TestPack" && p.user && p.name == "Test Pack"));
        assert!(install_icon_pack("/nonexistent/path").is_err());
        remove_icon_pack("TestPack").unwrap();
        assert!(!home.join(".local/share/icons/TestPack").exists());
        assert!(remove_icon_pack("../etc").is_err());
    }
}

/// Build the Liki pack (app logos from Simple Icons, in the chosen shape and colouring) into the
/// user's icons folder, by running liki-build. `shape` is glyph, squircle, rounded, circle or hexagon;
/// `style` is brand, dark or purple.
pub fn build_brand_pack(shape: &str, style: &str) -> Result<(), String> {
    if !["glyph", "squircle", "rounded", "circle", "hexagon"].contains(&shape) || !["brand", "dark", "purple"].contains(&style) {
        return Err("unknown shape or style".into());
    }
    act("liki-build", &["--shape", shape, "--style", style])
}
