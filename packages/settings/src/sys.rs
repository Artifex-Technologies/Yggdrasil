//! Reading and changing the system, through the same command-line tools Plasma uses. Every
//! parser is a pure function of the tool's output so it can be tested without the tool.

use std::collections::BTreeMap;
use std::process::Command;

/// Run a program and return its standard output, or None if it is missing or failed.
pub fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if out.status.success() {
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        None
    }
}

/// Run a program for its effect; Err carries its error output.
pub fn act(cmd: &str, args: &[&str]) -> Result<(), String> {
    match Command::new(cmd).args(args).output() {
        Ok(o) if o.status.success() => Ok(()),
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => Err(format!("{cmd}: {e}")),
    }
}

/// Start a program and leave it running (an app, a terminal).
pub fn spawn(cmd: &str, args: &[&str]) {
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
    let Some(name) = os_release.lines().find_map(|l| l.strip_prefix("LOGO=")).map(|v| v.trim().trim_matches('"').to_string()) else { return String::new() };
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

pub fn os_name(os_release: &str) -> String {
    let kv: BTreeMap<&str, &str> = os_release
        .lines()
        .filter_map(|l| l.split_once('='))
        .collect();
    kv.get("PRETTY_NAME").or(kv.get("NAME")).map(|v| v.trim_matches('"').to_string()).unwrap_or_default()
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
        let w = Wifi { in_use: f[0] == "*", ssid: f[1].clone(), signal: f[2].parse().unwrap_or(0), secured: !f[3].is_empty() && f[3] != "--" };
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
    pub saved: Vec<SavedNet>,
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
    /// From NetworkManager's cloned-mac-address value. Unset follows the default, which Heimdall makes private on Wi-Fi.
    pub fn from_nm(v: &str, wifi: bool) -> MacMode {
        match v.trim() {
            "stable" | "stable-ssid" => MacMode::Private,
            "random" => MacMode::Random,
            "permanent" | "preserve" => MacMode::Real,
            "" | "--" => if wifi { MacMode::Private } else { MacMode::Real },
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
    parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit())) && u8::from_str_radix(parts[0], 16).map(|b| b & 1 == 0).unwrap_or(false)
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

/// A saved network (a NetworkManager profile, Wi-Fi or wired) and its address setting.
#[derive(Clone, Debug, PartialEq)]
pub struct SavedNet {
    pub name: String,
    pub uuid: String,
    pub wifi: bool,
    pub mac: MacMode,
}

fn saved_nets() -> Vec<SavedNet> {
    let mut out = vec![];
    for line in run("nmcli", &["-t", "-f", "NAME,UUID,TYPE", "connection", "show"]).unwrap_or_default().lines() {
        let f = nm_fields(line);
        if f.len() < 3 {
            continue;
        }
        let wifi = f[2] == "802-11-wireless";
        if !wifi && f[2] != "802-3-ethernet" {
            continue;
        }
        let key = if wifi { "802-11-wireless.cloned-mac-address" } else { "802-3-ethernet.cloned-mac-address" };
        let v = run("nmcli", &["-g", key, "connection", "show", &f[1]]).unwrap_or_default();
        out.push(SavedNet { name: f[0].clone(), uuid: f[1].clone(), wifi, mac: MacMode::from_nm(&v, wifi) });
    }
    out
}

/// Sets which address a saved network sees; it is used from the next time the computer connects to it.
pub fn set_mac(uuid: &str, wifi: bool, mode: &MacMode) -> Result<(), String> {
    if let MacMode::Custom(a) = mode {
        if !valid_mac(a) {
            return Err("that is not a usable address (six pairs like 02:1A:2B:3C:4D:5E)".into());
        }
    }
    let key = if wifi { "802-11-wireless.cloned-mac-address" } else { "802-3-ethernet.cloned-mac-address" };
    act("nmcli", &["connection", "modify", uuid, key, &mode.to_nm()])
}

pub fn network() -> Network {
    Network {
        wifi_on: run("nmcli", &["radio", "wifi"]).map(|s| s.trim() == "enabled").unwrap_or(false),
        devices: net_devices(&run("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).unwrap_or_default()),
        networks: wifi_list(&run("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"]).unwrap_or_default()),
        saved: saved_nets(),
    }
}

#[cfg(test)]
mod mac_tests {
    use super::*;

    #[test]
    fn address_settings_read_write_and_check() {
        assert_eq!(MacMode::from_nm("stable", true), MacMode::Private);
        assert_eq!(MacMode::from_nm("random", true), MacMode::Random);
        assert_eq!(MacMode::from_nm("permanent", true), MacMode::Real);
        assert_eq!(MacMode::from_nm("", true), MacMode::Private, "Wi-Fi defaults to private (Heimdall)");
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
    let paired: Vec<String> = bt_list(&run("bluetoothctl", &["devices", "Paired"]).unwrap_or_default()).into_iter().map(|(m, _)| m).collect();
    bt_list(&run("bluetoothctl", &["devices"]).unwrap_or_default())
        .into_iter()
        .filter(|(m, n)| !paired.contains(m) && !n.is_empty() && n.replace('-', ":") != *m)
        .collect()
}

/// Looks for devices nearby for `secs` seconds (in the background; bt_nearby lists what turned up).
pub fn bt_scan(secs: u32) {
    if let Ok(mut c) = Command::new("bluetoothctl").args(["--timeout", &secs.to_string(), "scan", "on"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// The BlueZ object of a device: /org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF (the first adapter).
pub fn bt_path(mac: &str) -> Option<String> {
    let adapter = std::fs::read_dir("/sys/class/bluetooth").ok()?.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| n.starts_with("hci") && !n.contains(':')).min()?;
    (mac.len() == 17 && mac.chars().all(|c| c.is_ascii_hexdigit() || c == ':')).then(|| format!("/org/bluez/{adapter}/dev_{}", mac.replace(':', "_")))
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
    ask(&["call", "org.bluez", &path, "org.bluez.Device1", "Pair"]).map_err(|e| if e.contains("AlreadyExists") { String::new() } else { format!("Pairing did not work: {e}") }).or_else(|e| if e.is_empty() { Ok(()) } else { Err(e) })?;
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
    pub devices: Vec<BtDevice>,
}

pub fn bluetooth() -> Bluetooth {
    let show = run("bluetoothctl", &["show"]).unwrap_or_default();
    let powered = bt_powered(&show);
    let devices = bt_list(&run("bluetoothctl", &["devices", "Paired"]).unwrap_or_default())
        .into_iter()
        .map(|(mac, name)| {
            let (connected, kind, battery) = bt_info(&run("bluetoothctl", &["info", &mac]).unwrap_or_default());
            BtDevice { mac, name, connected, kind, battery }
        })
        .collect();
    Bluetooth { present: powered.is_some(), powered: powered.unwrap_or(false), devices }
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

// ---------------------------------------------------------------- Displays (kscreen-doctor)

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
    pub enabled: bool,
    pub primary: bool,
    pub scale: f64,
    pub current: String,
    pub modes: Vec<Mode>,
    pub x: i64,
    pub y: i64,
}

impl Output {
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

/// `kscreen-doctor -j`: connected outputs.
pub fn outputs(json: &str) -> Vec<Output> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    v["outputs"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|o| o["connected"].as_bool().unwrap_or(false))
                .map(|o| Output {
                    name: o["name"].as_str().unwrap_or("").to_string(),
                    // Plasma 6.8 dropped the `enabled` key: a connected output with a current mode is in use
                    enabled: o["enabled"].as_bool().unwrap_or_else(|| o["currentModeId"].as_str().map(|m| !m.is_empty()).unwrap_or(false)),
                    primary: o["priority"].as_i64() == Some(1),
                    scale: o["scale"].as_f64().unwrap_or(1.0),
                    current: o["currentModeId"].as_str().map(str::to_string).unwrap_or_else(|| o["currentModeId"].to_string()),
                    modes: o["modes"]
                        .as_array()
                        .map(|m| {
                            m.iter()
                                .map(|m| Mode {
                                    id: m["id"].as_str().map(str::to_string).unwrap_or_else(|| m["id"].to_string()),
                                    width: m["size"]["width"].as_u64().unwrap_or(0) as u32,
                                    height: m["size"]["height"].as_u64().unwrap_or(0) as u32,
                                    refresh: m["refreshRate"].as_f64().unwrap_or(0.0),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    x: o["pos"]["x"].as_i64().unwrap_or(0),
                    y: o["pos"]["y"].as_i64().unwrap_or(0),
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Clone, Debug, Default)]
pub struct Displays {
    pub outputs: Vec<Output>,
    pub tiling: String,
}

pub fn displays() -> Displays {
    let tiling = yggdrasil_conf::tiling::outputs().join(",");
    if hyprland() {
        return Displays { outputs: hypr_outputs(&run("hyprctl", &["-j", "monitors", "all"]).unwrap_or_default()), tiling };
    }
    Displays { outputs: outputs(&run("kscreen-doctor", &["-j"]).unwrap_or_default()), tiling }
}

/// The session's compositor is Hyprland (Yggdrasil's own session).
pub fn hyprland() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

/// `hyprctl -j monitors all`: the screens. Hyprland has no "main" screen: the one at the top left of the space is it.
pub fn hypr_outputs(json: &str) -> Vec<Output> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or(serde_json::Value::Null);
    let mut outs: Vec<Output> = v
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let (w, h, r) = (m["width"].as_u64()? as u32, m["height"].as_u64()? as u32, m["refreshRate"].as_f64().unwrap_or(60.0));
                    let mut modes: Vec<Mode> = m["availableModes"]
                        .as_array()
                        .map(|l| l.iter().filter_map(|x| parse_hypr_mode(x.as_str()?)).collect())
                        .unwrap_or_default();
                    let current = hypr_mode_id(w, h, r);
                    if !modes.iter().any(|x| x.id == current) {
                        modes.push(Mode { id: current.clone(), width: w, height: h, refresh: r });
                    }
                    Some(Output {
                        name: m["name"].as_str()?.to_string(),
                        enabled: !m["disabled"].as_bool().unwrap_or(false),
                        primary: false,
                        scale: m["scale"].as_f64().unwrap_or(1.0),
                        current,
                        modes,
                        x: m["x"].as_i64().unwrap_or(0),
                        y: m["y"].as_i64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(first) = outs.iter_mut().filter(|o| o.enabled).min_by_key(|o| (o.x, o.y)) {
        first.primary = true;
    }
    outs
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

/// The monitor lines of `~/.config/yggdrasil/monitors.lua` for these screens (what Yggdrasil's Hyprland config loads).
pub fn hypr_monitors_lua(outs: &[Output]) -> String {
    let mut t = String::from("-- Written by Settings > Displays. Loaded by Yggdrasil's Hyprland config.\n");
    for o in outs {
        if !o.enabled {
            t.push_str(&format!("hl.monitor({{ output = \"{}\", disabled = true }})\n", o.name));
            continue;
        }
        t.push_str(&format!("hl.monitor({{ output = \"{}\", mode = \"{}\", position = \"{}x{}\", scale = {} }})\n", o.name, o.current, o.x, o.y, o.scale));
    }
    t
}

/// Changes one screen's mode or scale in Hyprland: the whole list is written (from what Hyprland reports now, with the change) and
/// Hyprland reads it again.
pub fn hypr_set(name: &str, mode: Option<&str>, scale: Option<f64>) -> Result<(), String> {
    let mut outs = hypr_outputs(&run("hyprctl", &["-j", "monitors", "all"]).ok_or("Hyprland did not answer")?);
    let o = outs.iter_mut().find(|o| o.name == name).ok_or("that screen is not there")?;
    if let Some(m) = mode {
        o.current = m.to_string();
    }
    if let Some(s) = scale {
        o.scale = s;
    }
    let p = yggdrasil_conf::dirs::config().join("monitors.lua");
    std::fs::write(&p, hypr_monitors_lua(&outs)).map_err(|e| e.to_string())?;
    act("hyprctl", &["reload"])
}

/// Turns tiling on or off for one screen: the shared list (yggdrasil_conf::tiling) and then whoever draws the windows.
pub fn set_tiling(name: &str, on: bool) -> Result<(), String> {
    let mut list = yggdrasil_conf::tiling::outputs();
    list.retain(|n| n != name);
    if on {
        list.push(name.to_string());
    }
    yggdrasil_conf::tiling::set_outputs(&list).map_err(|e| e.to_string())?;
    act("hyprctl", &["reload"])
}

// ---------------------------------------------------------------- Appearance

pub const ACCENTS: [(&str, [u8; 3]); 9] = [
    ("Purple", [124, 84, 214]),
    ("Blue", [61, 132, 245]),
    ("Teal", [38, 162, 158]),
    ("Green", [52, 168, 83]),
    ("Yellow", [230, 176, 30]),
    ("Orange", [240, 120, 40]),
    ("Red", [224, 60, 70]),
    ("Pink", [220, 90, 160]),
    ("White", [255, 255, 255]),
];

/// An icon pack (a freedesktop icon theme) that can be chosen.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IconPack {
    /// The folder name, which is what KDE's settings store.
    pub id: String,
    pub name: String,
    /// Installed by the user (in ~/.local/share/icons), so it can be removed from here.
    pub user: bool,
    /// A few sample icons (folder, files, settings, a browser) to show what the pack looks like.
    pub preview: Vec<std::path::PathBuf>,
}

#[derive(Clone, Debug, Default)]
pub struct Appearance {
    pub icon_packs: Vec<IconPack>,
    /// The id of the pack in use.
    pub icon_theme: String,
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
    Appearance { icon_packs: icon_packs(), icon_theme: current_icon_theme(), dark: look.dark, accent, wallpapers, panel: yggdrasil_conf::PanelConf::load() }
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
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuLimits {
    pub name: String,
    pub default: i64,
    pub min: i64,
    pub max: i64,
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
    });
    let modes = v["limits"].as_object().map(|m| m.iter().map(|(k, l)| (k.clone(), (l["cpu"].as_i64(), l["gpu"].as_i64()))).collect()).unwrap_or_default();
    Some(PowerLimits { gpu, modes })
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

const PPD: [&str; 3] = ["org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles", "org.freedesktop.UPower.PowerProfiles"];

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
    act("busctl", &["set-property", PPD[0], PPD[1], PPD[2], "ActiveProfile", "s", name]).or_else(|_| act("powerprofilesctl", &["set", name]))
}

pub fn power() -> Power {
    let (profiles, active) = profiles_dbus().unwrap_or_else(|| profiles(&run("powerprofilesctl", &["list"]).unwrap_or_default()));
    let limits = if yggdrasil_conf::PanelConf::load().developer { run("/usr/lib/yggdrasil/powerlimit", &["get"]).and_then(|s| parse_power_limits(&s)) } else { None };
    Power {
        profile: active,
        profiles,
        limits,
        idle: read_idle(),
    }
}

/// What to do as the user stays away, in minutes (0 = never), and whether this computer can sleep and hibernate.
#[derive(Clone, Debug, PartialEq)]
pub struct Idle {
    /// The falling characters, until you are back (minutes; 0 = never).
    pub screensaver: i64,
    pub dim: i64,
    /// How bright the screen is while dimmed, in percent.
    pub dim_level: i64,
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
        Idle { screensaver: 0, dim: 5, dim_level: 30, screen_off: 0, lock: 0, sleep: 0, hibernate: 0, can_sleep: true, can_hibernate: true, sleep_note: String::new() }
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
        let Ok(n) = v.trim().parse::<i64>() else { continue };
        match k.trim() {
            "screensaver" => i.screensaver = n / 60,
            "dim" => i.dim = n / 60,
            "dim_level" => i.dim_level = n.clamp(10, 90),
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
        "# Written by Settings > Power. yggdrasil-idle acts on these (seconds of inactivity, 0 = never).\nmode = act\nscreensaver = {}\ndim = {}\ndim_level = {}\ndisplay_off = {}\nlock = {}\nsuspend = {}\nhibernate = {}\n",
        i.screensaver * 60, i.dim * 60, i.dim_level, i.screen_off * 60, i.lock * 60, i.sleep * 60, i.hibernate * 60
    )
}

fn can(what: &str) -> bool {
    run("busctl", &["call", "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager", what]).map(|s| s.contains("\"yes\"") || s.contains("\"challenge\"")).unwrap_or(false)
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

/// Saves the timers and switches KDE's own dimming, screen-off, sleep and lock timers off, so the two never both act.
pub fn write_idle(i: &Idle) -> Result<(), String> {
    let p = idle_conf_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, idle_text(i)).map_err(|e| e.to_string())?;
    // only Plasma has KDE's timers (in Yggdrasil's session there is nothing to switch off)
    if yggdrasil_conf::own_session() {
        return Ok(());
    }
    for state in ["AC", "Battery", "LowBattery"] {
        let _ = act("kwriteconfig6", &["--file", "powerdevilrc", "--group", state, "--group", "Display", "--key", "DimDisplayWhenIdle", "false"]);
        let _ = act("kwriteconfig6", &["--file", "powerdevilrc", "--group", state, "--group", "Display", "--key", "TurnOffDisplayWhenIdle", "false"]);
        let _ = act("kwriteconfig6", &["--file", "powerdevilrc", "--group", state, "--group", "SuspendAndShutdown", "--key", "AutoSuspendAction", "0"]);
    }
    let _ = act("kwriteconfig6", &["--file", "kscreenlockerrc", "--group", "Daemon", "--key", "Autolock", "false"]);
    let _ = act("qdbus6", &["org.kde.Solid.PowerManagement", "/org/kde/Solid/PowerManagement", "org.kde.Solid.PowerManagement.reparseConfiguration"]);
    Ok(())
}

#[cfg(test)]
mod ini_tests {
    use super::*;

    #[test]
    fn reads_and_sets_one_value() {
        let t = "[General]\nAccentColor=1,2,3\n\n[Icons]\nTheme=breeze\n";
        assert_eq!(ini_get(t, "Icons", "Theme").as_deref(), Some("breeze"));
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
        let i = Idle { screensaver: 3, dim: 7, dim_level: 40, screen_off: 15, lock: 10, sleep: 0, hibernate: 120, can_sleep: true, can_hibernate: true, sleep_note: String::new() };
        assert_eq!(parse_idle(&idle_text(&i)), i);
    }

    #[test]
    fn defaults_without_a_file() {
        let i = parse_idle("");
        assert_eq!((i.dim, i.dim_level, i.lock, i.sleep), (5, 30, 0, 0));
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
    pub zones: Vec<String>,
}

/// `timedatectl show`: Key=Value lines.
pub fn time_show(text: &str) -> (String, bool) {
    let kv: BTreeMap<&str, &str> = text.lines().filter_map(|l| l.split_once('=')).collect();
    (kv.get("Timezone").unwrap_or(&"").to_string(), kv.get("NTP").map(|v| *v == "yes").unwrap_or(false))
}

pub fn time() -> Time {
    let (timezone, ntp) = time_show(&run("timedatectl", &["show"]).unwrap_or_default());
    let zones = run("timedatectl", &["list-timezones"]).unwrap_or_default().lines().map(str::to_string).collect();
    Time { timezone, ntp, zones }
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
                    let pkgs: Vec<&str> = c["packages"].as_array().map(|p| p.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
                    let pkgbuild = c["pkgbuild"].as_str();
                    let ok = if !pkgs.is_empty() { pkgs.iter().all(|p| installed(p)) } else { pkgbuild.map(|p| installed(p)).unwrap_or(false) };
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
        let o = hypr_outputs(j);
        let dp1 = o.iter().find(|x| x.name == "DP-1").unwrap();
        assert!(dp1.primary, "the screen at the top left is the main one");
        assert_eq!(dp1.current, "3440x1440@180.00");
        assert_eq!(dp1.current_mode().unwrap().refresh, 180.0);
        assert_eq!(dp1.rates(3440, 1440).len(), 2);
        let lua = hypr_monitors_lua(&o);
        assert!(lua.contains(r#"hl.monitor({ output = "DP-3", mode = "2560x2880@59.95", position = "3440x0", scale = 1.25 })"#), "{lua}");
    }

    #[test]
    fn about_parsers() {
        assert_eq!(os_name("NAME=\"Some OS\"\nPRETTY_NAME=\"Some OS 1\"\n"), "Some OS 1");
        assert_eq!(os_logo("NAME=x\nLOGO=../../etc/passwd\n"), "", "an icon name, never a path");
        let cpu = "processor\t: 0\nmodel name\t: Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz\nprocessor\t: 1\nmodel name\t: Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz\n";
        assert_eq!(cpu_model(cpu), "Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz × 2");
        assert!((mem_gib("MemTotal:       65775396 kB\n") - 62.73).abs() < 0.01);
        let lspci = "00:02.0 \"VGA compatible controller\" \"Intel Corporation\" \"CoffeeLake-S GT2 [UHD Graphics 630]\" -r02 \"\" \"\"\n01:00.0 \"VGA compatible controller\" \"NVIDIA Corporation\" \"TU102 [GeForce RTX 2080 Ti Rev. A]\" -ra1 \"\" \"\"\n00:1f.3 \"Audio device\" \"Intel Corporation\" \"Cannon Lake PCH cAVS\" \"\" \"\"\n";
        assert_eq!(gpus(lspci), vec!["Intel Corporation CoffeeLake-S GT2 [UHD Graphics 630]", "NVIDIA Corporation TU102 [GeForce RTX 2080 Ti Rev. A]"]);
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
        let d = net_devices("eth0:ethernet:connected:Wired connection 1\nlo:loopback:connected (externally):lo\nwlan0:wifi:disconnected:\n");
        assert_eq!(d.len(), 2);
        assert_eq!(d[0].connection, "Wired connection 1");
        assert_eq!(d[1].kind, "wifi");
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
        let (c, k, b) = bt_info("Device 11:22..\n\tName: MX Mechanical\n\tIcon: input-keyboard\n\tConnected: yes\n\tBattery Percentage: 0x5a (90)\n");
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
    fn displays_parser() {
        let j = r#"{"outputs":[{"name":"DP-1","connected":true,"enabled":true,"priority":1,"scale":1,"currentModeId":"2","pos":{"x":0,"y":0},
            "modes":[{"id":"1","size":{"width":3440,"height":1440},"refreshRate":59.97},{"id":"2","size":{"width":3440,"height":1440},"refreshRate":143.97},{"id":"3","size":{"width":2560,"height":1440},"refreshRate":143.97}]},
            {"name":"HDMI-A-2","connected":false,"enabled":false,"modes":[]}]}"#;
        let o = outputs(j);
        assert_eq!(o.len(), 1);
        assert!(o[0].primary);
        assert_eq!(o[0].current_mode().unwrap().refresh, 143.97);
        assert_eq!(o[0].resolutions(), vec![(3440, 1440), (2560, 1440)]);
        assert_eq!(o[0].rates(3440, 1440)[0].id, "2");
    }

    #[test]
    fn power_profiles_parser() {
        let (n, a) = profiles("  performance:\n    CpuDriver:\tintel_pstate\n\n* balanced:\n    CpuDriver:\tintel_pstate\n\n  power-saver:\n");
        assert_eq!(n, vec!["performance", "balanced", "power-saver"]);
        assert_eq!(a, "balanced");
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
        let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(&d).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|x| x == "png" || x == "svg").unwrap_or(false)).collect()).unwrap_or_default();
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
            if theme_key(&index, "Hidden").map(|h| h.eq_ignore_ascii_case("true")).unwrap_or(false) || theme_key(&index, "Directories").is_none() {
                continue;
            }
            // KDE's, GNOME's and the toolkit's own packs are not choices: Yggdrasil's pack is Liki (they stay installed only because KDE programs need them)
            let lower = id.to_lowercase();
            if matches!(id.as_str(), "default" | "hicolor" | "locolor") || lower.starts_with("breeze") || lower.starts_with("klassy") || lower.starts_with("adwaita") || lower == "highcontrast" {
                continue;
            }
            let samples = ["folder", "system-file-manager", "file-manager", "minos", "steam", "spotify", "vivaldi", "firefox", "preferences-system", "web-browser"];
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

/// One value from an INI-style settings file (`[group]`, `key=value`), such as kdeglobals.
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
    std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

/// The icon pack in use (kept in kdeglobals, which the panel, Minos and Plasma all read).
pub fn current_icon_theme() -> String {
    std::fs::read_to_string(config_dir().join("kdeglobals")).ok().and_then(|t| ini_get(&t, "Icons", "Theme")).unwrap_or_default()
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

/// Choose an icon pack for the whole desktop (the panel, Minos, GTK and Qt programs follow it). In Plasma, Plasma's own tool
/// does it (it also tells running KDE programs); in Yggdrasil's session the settings files are written here, no KDE tool needed.
pub fn set_icon_pack(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return Err("not an icon pack name".into());
    }
    if !yggdrasil_conf::own_session() && std::path::Path::new("/usr/lib/plasma-changeicons").exists() {
        return act("/usr/lib/plasma-changeicons", &[id]);
    }
    let cfg = config_dir();
    ini_write(&cfg.join("kdeglobals"), "Icons", "Theme", id)?;
    for gtk in ["gtk-3.0", "gtk-4.0"] {
        ini_write(&cfg.join(gtk).join("settings.ini"), "Settings", "gtk-icon-theme-name", id)?;
    }
    // GTK 4 and portal-aware programs read it from GNOME's settings store, when there is one
    let _ = std::process::Command::new("gsettings").args(["set", "org.gnome.desktop.interface", "icon-theme", id]).stdin(std::process::Stdio::null()).status();
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
            act("bsdtar", &["-xf", &local.to_string_lossy(), "-C", &unpacked.to_string_lossy()]).map_err(|e| format!("Could not open it as an archive: {e}"))?;
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
