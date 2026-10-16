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
    pub hostname: String,
    pub cpu: String,
    pub memory_gib: f64,
    pub gpus: Vec<String>,
    pub kernel: String,
    pub disk_total: u64,
    pub disk_free: u64,
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
}

pub fn network() -> Network {
    Network {
        wifi_on: run("nmcli", &["radio", "wifi"]).map(|s| s.trim() == "enabled").unwrap_or(false),
        devices: net_devices(&run("nmcli", &["-t", "-f", "DEVICE,TYPE,STATE,CONNECTION", "device"]).unwrap_or_default()),
        networks: wifi_list(&run("nmcli", &["-t", "-f", "IN-USE,SSID,SIGNAL,SECURITY", "device", "wifi", "list"]).unwrap_or_default()),
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
                    enabled: o["enabled"].as_bool().unwrap_or(false),
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
    Displays {
        outputs: outputs(&run("kscreen-doctor", &["-j"]).unwrap_or_default()),
        tiling: run("kreadconfig6", &["--file", "kwinrc", "--group", "Script-labyrinth-screen-keys", "--key", "tilingOutputs"])
            .unwrap_or_default()
            .trim()
            .to_string(),
    }
}

// ---------------------------------------------------------------- Appearance

pub const ACCENTS: [(&str, [u8; 3]); 8] = [
    ("Labyrinth purple", [124, 84, 214]),
    ("Blue", [61, 132, 245]),
    ("Teal", [38, 162, 158]),
    ("Green", [52, 168, 83]),
    ("Yellow", [230, 176, 30]),
    ("Orange", [240, 120, 40]),
    ("Red", [224, 60, 70]),
    ("Pink", [220, 90, 160]),
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
    /// Labyrinth's own top bar and dock are in use (labyrinth-panel-mode).
    pub panel_on: bool,
    pub panel: labyrinth_conf::PanelConf,
}

pub fn appearance() -> Appearance {
    let scheme = run("kreadconfig6", &["--file", "kdeglobals", "--group", "General", "--key", "ColorScheme"]).unwrap_or_default();
    let accent = run("kreadconfig6", &["--file", "kdeglobals", "--group", "General", "--key", "AccentColor"])
        .and_then(|s| labyrinth_ui::theme::parse_kde_accent(&format!("[General]
AccentColor={}", s.trim())));
    let mut wallpapers = vec![];
    for dir in ["/usr/share/labyrinth/wallpapers", "/usr/share/wallpapers"] {
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
    let panel_on = run("labyrinth-panel-mode", &["status"]).map(|s| s.trim() == "labyrinth-panel").unwrap_or(false);
    Appearance { icon_packs: icon_packs(), icon_theme: run("kreadconfig6", &["--file", "kdeglobals", "--group", "Icons", "--key", "Theme"]).map(|s| s.trim().to_string()).unwrap_or_default(), dark: scheme.trim().to_lowercase().contains("dark") || scheme.trim().is_empty(), accent, wallpapers, panel_on, panel: labyrinth_conf::PanelConf::load() }
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
    pub screen_off_min: i64,
    pub sleep_min: i64,
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

pub fn power() -> Power {
    let (profiles, active) = profiles(&run("powerprofilesctl", &["list"]).unwrap_or_default());
    let read = |group: &str, key: &str, default: i64| {
        run("kreadconfig6", &["--file", "powerdevilrc", "--group", "AC", "--group", group, "--key", key, "--default", &default.to_string()])
            .and_then(|s| s.trim().parse::<i64>().ok())
            .unwrap_or(default)
    };
    Power {
        profile: active,
        profiles,
        screen_off_min: read("Display", "TurnOffDisplayIdleTimeoutSec", 600) / 60,
        sleep_min: read("SuspendAndShutdown", "AutoSuspendIdleTimeoutSec", 0) / 60,
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

// ---------------------------------------------------------------- Apps (labyrinth-apps)

#[derive(Clone, Debug, PartialEq)]
pub struct Component {
    pub id: String,
    pub name: String,
    pub description: String,
    pub installed: bool,
}

/// Labyrinth's optional software from components.json, marked installed when all its repo
/// packages are (pacman -Q).
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
    let json = std::fs::read_to_string("/usr/share/labyrinth/components.json").unwrap_or_default();
    components(&json, &|p| run("pacman", &["-Q", p]).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn about_parsers() {
        assert_eq!(os_name("NAME=\"Labyrinth OS\"\nPRETTY_NAME=\"Labyrinth OS\"\n"), "Labyrinth OS");
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
            if matches!(id.as_str(), "default" | "hicolor" | "locolor") || out.iter().any(|p| p.id == id) {
                continue;
            }
            let name = theme_key(&index, "Name").filter(|n| !n.is_empty()).unwrap_or_else(|| id.clone());
            let preview = ["folder", "system-file-manager", "file-manager", "minos", "preferences-system", "web-browser"].iter().filter_map(|n| theme_icon(&d, n)).take(4).collect();
            out.push(IconPack { id, name, user, preview });
        }
    }
    out
}

/// Choose an icon pack for the whole desktop (KDE, the panel and Minos follow it).
pub fn set_icon_pack(id: &str) -> Result<(), String> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') {
        return Err("not an icon pack name".into());
    }
    act("/usr/lib/plasma-changeicons", &[id])
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
    let tmp = std::env::temp_dir().join(format!("labyrinth-icons-{}", std::process::id()));
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
