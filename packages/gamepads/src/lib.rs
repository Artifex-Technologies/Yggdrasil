//! Game controllers, read from the kernel (sysfs): which are connected, how (USB or Bluetooth), their battery, and their lights.
//! The kernel drives them itself (hid-playstation for the DualShock 4 and DualSense, hid-sony on older kernels, xpad, hid-microsoft or
//! xpadneo for Xbox controllers, hid-nintendo); this only reads what those drivers publish and writes the LED files they offer.
//!
//! Used by Settings (Bluetooth > Controllers) and by the Controllers top bar plugin (packages/controllers).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

mod rumble;
pub use rumble::rumble;

/// Which controller it is (from its USB/Bluetooth vendor and product ids).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Model {
    DualShock4,
    DualSense,
    DualSenseEdge,
    Xbox,
    SwitchPro,
    Other,
}

impl Model {
    pub fn from_ids(vendor: u16, product: u16) -> Model {
        match (vendor, product) {
            (0x054c, 0x05c4 | 0x09cc | 0x0ba0) => Model::DualShock4,
            (0x054c, 0x0ce6) => Model::DualSense,
            (0x054c, 0x0df2) => Model::DualSenseEdge,
            (0x045e, _) => Model::Xbox,
            (0x057e, 0x2009) => Model::SwitchPro,
            _ => Model::Other,
        }
    }

    /// A short name for people: "DualShock 4 (PS4)".
    pub fn label(self) -> &'static str {
        match self {
            Model::DualShock4 => "DualShock 4 (PS4)",
            Model::DualSense => "DualSense (PS5)",
            Model::DualSenseEdge => "DualSense Edge (PS5)",
            Model::Xbox => "Xbox controller",
            Model::SwitchPro => "Switch Pro Controller",
            Model::Other => "Controller",
        }
    }

    /// How to put it in pairing mode.
    pub fn pairing_hint(self) -> &'static str {
        match self {
            Model::DualShock4 => "Hold Share and the PS button until the light bar flashes quickly.",
            Model::DualSense | Model::DualSenseEdge => "Hold Create and the PS button until the light around the touchpad flashes.",
            Model::Xbox => "Turn it on, then hold the pairing button on top until the Xbox button flashes quickly.",
            Model::SwitchPro => "Hold the small sync button on top until the player lights run back and forth.",
            Model::Other => "Put the controller in pairing mode (see its manual).",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Connection {
    Usb,
    Bluetooth,
    Other,
}

impl Connection {
    pub fn label(self) -> &'static str {
        match self {
            Connection::Usb => "USB",
            Connection::Bluetooth => "Bluetooth",
            Connection::Other => "Connected",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Battery {
    /// 0-100, when the driver gives a number.
    pub percent: Option<u8>,
    /// "Critical" | "Low" | "Normal" | "High" | "Full", when it gives a level instead (or as well).
    pub level: Option<String>,
    /// "Charging" | "Discharging" | "Full" | "Not charging" | "Unknown".
    pub status: String,
}

impl Battery {
    pub fn charging(&self) -> bool {
        self.status == "Charging"
    }
    pub fn full(&self) -> bool {
        self.status == "Full"
    }
    /// A percentage to compare and draw: the number, else one worked out from the level.
    pub fn estimate(&self) -> Option<u8> {
        self.percent.or_else(|| {
            Some(match self.level.as_deref()? {
                "Critical" => 5,
                "Low" => 20,
                "Normal" => 50,
                "High" => 80,
                "Full" => 100,
                _ => return None,
            })
        })
    }
    /// "80%", "Low", "Charging 80%", "Full".
    pub fn describe(&self) -> String {
        let amount = self.percent.map(|p| format!("{p}%")).or_else(|| self.level.clone()).unwrap_or_else(|| "?".into());
        if self.full() {
            "Full".into()
        } else if self.charging() {
            format!("Charging {amount}")
        } else {
            amount
        }
    }
}

/// The controller's lights the kernel offers as LED files.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Leds {
    /// One RGB LED (`*:rgb:indicator`, files multi_intensity and brightness): DualShock 4 and DualSense on hid-playstation.
    pub lightbar: Option<PathBuf>,
    /// Separate red, green and blue LEDs (DualShock 4 on the older hid-sony).
    pub rgb: Option<[PathBuf; 3]>,
    /// The player indicator lights, in order (DualSense: five).
    pub players: Vec<PathBuf>,
    /// We may write them (Labyrinth's udev rule opens them to the user).
    pub writable: bool,
}

impl Leds {
    pub fn has_lightbar(&self) -> bool {
        self.lightbar.is_some() || self.rgb.is_some()
    }
}

/// A connected controller.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Pad {
    pub name: String,
    pub model: Model,
    pub connection: Connection,
    /// The Bluetooth address (lower case) or the USB serial, when the driver reports one.
    pub uniq: String,
    pub vendor: u16,
    pub product: u16,
    /// The kernel driver of the HID device ("playstation", "sony", "xpadneo", "microsoft", "nintendo"), or the input driver ("xpad").
    pub driver: String,
    /// /dev/input/eventN of its buttons and sticks (rumble goes there).
    pub event: Option<PathBuf>,
    pub battery: Option<Battery>,
    pub leds: Leds,
}

impl Pad {
    /// The Bluetooth address, when connected over Bluetooth.
    pub fn mac(&self) -> Option<&str> {
        (self.connection == Connection::Bluetooth && self.uniq.len() == 17).then_some(self.uniq.as_str())
    }
    /// What to call it: its model for the known ones ("DualShock 4 (PS4)"), else the name the driver gives.
    pub fn title(&self) -> String {
        if self.model == Model::Other { self.name.clone() } else { self.model.label().to_string() }
    }
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).map(|s| s.trim().to_string()).unwrap_or_default()
}

fn hex(s: &str) -> u16 {
    u16::from_str_radix(s.trim(), 16).unwrap_or(0)
}

fn writable(p: &Path) -> bool {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    CString::new(p.as_os_str().as_bytes()).map(|c| unsafe { libc::access(c.as_ptr(), libc::W_OK) } == 0).unwrap_or(false)
}

fn children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|r| r.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
}

/// Is this input device a joystick? It has a jsN node, or udev marked it (ID_INPUT_JOYSTICK).
fn is_joystick(root: &Path, input: &Path) -> bool {
    if children(input).iter().any(|c| name_of(c).starts_with("js")) {
        return true;
    }
    children(input).iter().filter(|c| name_of(c).starts_with("event")).any(|ev| {
        let dev = read(&ev.join("dev"));
        !dev.is_empty() && read(&root.join(format!("run/udev/data/c{dev}"))).lines().any(|l| l == "E:ID_INPUT_JOYSTICK=1")
    })
}

/// The connected controllers. `root` is "/" on a real system (tests give a made-up tree).
pub fn list_at(root: &Path) -> Vec<Pad> {
    let mut pads: Vec<Pad> = vec![];
    for input in children(&root.join("sys/class/input")) {
        if !name_of(&input).starts_with("input") || !is_joystick(root, &input) {
            continue;
        }
        let name = read(&input.join("name"));
        // a controller's motion sensors and touchpad are input devices of their own; only the gamepad itself counts
        if name.ends_with("Motion Sensors") || name.ends_with("Touchpad") {
            continue;
        }
        let (vendor, product) = (hex(&read(&input.join("id/vendor"))), hex(&read(&input.join("id/product"))));
        let connection = match hex(&read(&input.join("id/bustype"))) {
            0x03 => Connection::Usb,
            0x05 => Connection::Bluetooth,
            _ => Connection::Other,
        };
        let uniq = read(&input.join("uniq")).to_lowercase();
        // the device the input hangs off: for a HID controller its HID device, which also holds its battery and LEDs
        let parent = std::fs::canonicalize(input.join("device")).unwrap_or_default();
        let hid = if parent.as_os_str().is_empty() { None } else { Some(parent) };
        let driver = hid
            .as_ref()
            .and_then(|h| std::fs::read_link(h.join("driver")).ok())
            .map(|d| name_of(&d))
            .unwrap_or_default();
        let event = children(&input).into_iter().find(|c| name_of(c).starts_with("event")).map(|e| PathBuf::from("/dev/input").join(name_of(&e)));
        let battery = hid.as_ref().and_then(|h| battery_in(&h.join("power_supply")));
        let leds = hid.as_ref().map(|h| leds_in(&h.join("leds"))).unwrap_or_default();
        let pad = Pad { name, model: Model::from_ids(vendor, product), connection, uniq, vendor, product, driver, event, battery, leds };
        // one controller can show more than one joystick device; keep the first
        if !pads.iter().any(|p| !p.uniq.is_empty() && p.uniq == pad.uniq && p.vendor == pad.vendor) {
            pads.push(pad);
        }
    }
    pads
}

/// The connected controllers.
pub fn list() -> Vec<Pad> {
    list_at(Path::new("/"))
}

fn battery_in(dir: &Path) -> Option<Battery> {
    let ps = children(dir).into_iter().next()?;
    let cap = read(&ps.join("capacity"));
    let level = read(&ps.join("capacity_level"));
    let status = read(&ps.join("status"));
    let percent = cap.parse::<u8>().ok().map(|p| p.min(100));
    let level = (!level.is_empty() && level != "Unknown").then_some(level);
    if percent.is_none() && level.is_none() {
        return None;
    }
    Some(Battery { percent, level, status: if status.is_empty() { "Unknown".into() } else { status } })
}

fn leds_in(dir: &Path) -> Leds {
    let mut l = Leds::default();
    let mut rgb: [Option<PathBuf>; 3] = [None, None, None];
    for led in children(dir) {
        let n = name_of(&led);
        if n.ends_with(":rgb:indicator") {
            l.lightbar = Some(led.clone());
        } else if n.contains(":player-") {
            l.players.push(led.clone());
        } else if n.ends_with(":red") {
            rgb[0] = Some(led.clone());
        } else if n.ends_with(":green") {
            rgb[1] = Some(led.clone());
        } else if n.ends_with(":blue") {
            rgb[2] = Some(led.clone());
        }
    }
    l.players.sort_by_key(|p| name_of(p).rsplit('-').next().and_then(|n| n.parse::<u32>().ok()).unwrap_or(0));
    if let [Some(r), Some(g), Some(b)] = rgb {
        l.rgb = Some([r, g, b]);
    }
    let any = l.lightbar.as_ref().map(|p| p.join("brightness")).or_else(|| l.rgb.as_ref().map(|r| r[0].join("brightness"))).or_else(|| l.players.first().map(|p| p.join("brightness")));
    l.writable = any.is_some_and(|p| writable(&p));
    l
}

fn write(p: &Path, v: &str) -> Result<(), String> {
    std::fs::write(p, v).map_err(|e| format!("{}: {e}", p.display()))
}

/// Sets the light bar's colour (black turns it off).
pub fn set_lightbar(pad: &Pad, rgb: [u8; 3]) -> Result<(), String> {
    if let Some(led) = &pad.leds.lightbar {
        write(&led.join("multi_intensity"), &format!("{} {} {}", rgb[0], rgb[1], rgb[2]))?;
        let max = read(&led.join("max_brightness"));
        return write(&led.join("brightness"), if rgb == [0, 0, 0] { "0" } else if max.is_empty() { "255" } else { &max });
    }
    if let Some(leds) = &pad.leds.rgb {
        for (led, v) in leds.iter().zip(rgb) {
            write(&led.join("brightness"), &v.to_string())?;
        }
        return Ok(());
    }
    Err("this controller has no light bar the system can set".into())
}

/// Which of five player lights are on for player `n` (0 = none), as the PS5 shows them.
pub fn player_pattern(n: u8) -> [bool; 5] {
    let bits: u8 = match n {
        0 => 0b00000,
        1 => 0b00100,
        2 => 0b01010,
        3 => 0b10101,
        4 => 0b11011,
        _ => 0b11111,
    };
    std::array::from_fn(|i| bits & (1 << i) != 0)
}

/// Shows player `n` (0-5) on the player lights.
pub fn set_player(pad: &Pad, n: u8) -> Result<(), String> {
    if pad.leds.players.is_empty() {
        return Err("this controller has no player lights the system can set".into());
    }
    let pattern = player_pattern(n.min(5));
    for (i, led) in pad.leds.players.iter().enumerate() {
        write(&led.join("brightness"), if pattern.get(i).copied().unwrap_or(false) { "1" } else { "0" })?;
    }
    Ok(())
}

/// "#7c54d6" -> [124, 84, 214].
pub fn parse_color(s: &str) -> Option<[u8; 3]> {
    let h = s.trim().strip_prefix('#')?;
    if h.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(h, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn color_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

// ---------------------------------------------------------------- Bluetooth (bluetoothctl)

fn bluetoothctl(args: &[&str]) -> Option<String> {
    let out = std::process::Command::new("bluetoothctl").args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string())
}

fn valid_mac(mac: &str) -> bool {
    mac.len() == 17 && mac.split(':').all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// Disconnects a Bluetooth controller (it turns itself off).
pub fn disconnect(mac: &str) -> Result<(), String> {
    if !valid_mac(mac) {
        return Err("not a Bluetooth address".into());
    }
    bluetoothctl(&["disconnect", mac]).map(|_| ()).ok_or_else(|| "bluetoothctl could not disconnect it".into())
}

/// `bluetoothctl info` says the device is a game controller.
pub fn info_is_gamepad(info: &str) -> bool {
    info.lines().any(|l| l.trim() == "Icon: input-gaming")
}

/// A paired Bluetooth controller (connected or not).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct PairedPad {
    pub mac: String,
    pub name: String,
    pub connected: bool,
}

/// The paired Bluetooth devices that are game controllers.
pub fn paired_gamepads() -> Vec<PairedPad> {
    let list = bluetoothctl(&["devices", "Paired"]).unwrap_or_default();
    list.lines()
        .filter_map(|l| {
            let rest = l.strip_prefix("Device ")?;
            let (mac, name) = rest.split_once(' ')?;
            Some((mac.to_string(), name.to_string()))
        })
        .filter_map(|(mac, name)| {
            let info = bluetoothctl(&["info", &mac])?;
            info_is_gamepad(&info).then(|| PairedPad { connected: info.lines().any(|l| l.trim() == "Connected: yes"), mac: mac.to_lowercase(), name })
        })
        .collect()
}

// ---------------------------------------------------------------- settings

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PadSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lightbar: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<u8>,
}

/// ~/.config/yggdrasil/controllers.json
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    /// The light bar colour for every controller ("#rrggbb"; "#000000" off); none leaves the controller's own.
    #[serde(default)]
    pub lightbar: Option<String>,
    /// Per controller (by Bluetooth address or serial): its own colour and player number.
    #[serde(default)]
    pub per_pad: BTreeMap<String, PadSettings>,
    /// Notifications: connected, disconnected, battery low, charged.
    #[serde(default = "yes")]
    pub notify: bool,
    /// "Battery low" at or under this percentage.
    #[serde(default = "twenty")]
    pub low_at: u8,
}

fn yes() -> bool {
    true
}
fn twenty() -> u8 {
    20
}

impl Default for Settings {
    fn default() -> Self {
        Settings { lightbar: None, per_pad: BTreeMap::new(), notify: true, low_at: 20 }
    }
}

impl Settings {
    pub fn path() -> PathBuf {
        yggdrasil_conf::dirs::config().join("controllers.json")
    }

    pub fn load_from(p: &Path) -> Settings {
        std::fs::read_to_string(p).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn load() -> Settings {
        Self::load_from(&Self::path())
    }

    pub fn save_to(&self, p: &Path) -> std::io::Result<()> {
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self).unwrap_or_default())?;
        std::fs::rename(tmp, p)
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&Self::path())
    }

    /// The colour this controller should show: its own, else the one for all.
    pub fn lightbar_for(&self, pad: &Pad) -> Option<[u8; 3]> {
        self.per_pad.get(&pad.uniq).and_then(|p| p.lightbar.as_deref()).or(self.lightbar.as_deref()).and_then(parse_color)
    }

    pub fn player_for(&self, pad: &Pad) -> Option<u8> {
        self.per_pad.get(&pad.uniq).and_then(|p| p.player)
    }

    /// Puts the saved lights back on a controller (the kernel resets them every time it connects). Only what can be written.
    pub fn apply(&self, pad: &Pad) {
        if !pad.leds.writable {
            return;
        }
        if let Some(c) = self.lightbar_for(pad) {
            let _ = set_lightbar(pad, c);
        }
        if let Some(n) = self.player_for(pad) {
            let _ = set_player(pad, n);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn put(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    /// A made-up sysfs with a DualShock 4 over Bluetooth (hid-playstation: battery, lightbar), its motion sensors, a keyboard,
    /// and an Xbox controller over USB (xpad: no battery, joystick known from udev only).
    fn tree() -> PathBuf {
        let root = std::env::temp_dir().join(format!("gamepads-test-{}-{:?}", std::process::id(), std::thread::current().id()));
        let _ = std::fs::remove_dir_all(&root);
        let hid = root.join("sys/devices/virtual/bt/hci0/0005:054C:09CC.0003");
        put(&hid.join("power_supply/ps-controller-battery-a4:ae:12:00:11:22/capacity"), "75\n");
        put(&hid.join("power_supply/ps-controller-battery-a4:ae:12:00:11:22/status"), "Discharging\n");
        put(&hid.join("leds/input20:rgb:indicator/multi_intensity"), "0 0 64\n");
        put(&hid.join("leds/input20:rgb:indicator/brightness"), "255\n");
        put(&hid.join("leds/input20:rgb:indicator/max_brightness"), "255\n");
        std::fs::create_dir_all(root.join("sys/bus/hid/drivers/playstation")).unwrap();
        symlink(root.join("sys/bus/hid/drivers/playstation"), hid.join("driver")).unwrap();
        let classes = root.join("sys/class/input");
        std::fs::create_dir_all(&classes).unwrap();
        for (n, name, js) in [("input20", "Wireless Controller", true), ("input21", "Wireless Controller Motion Sensors", true)] {
            let d = hid.join("input").join(n);
            put(&d.join("name"), name);
            put(&d.join("uniq"), "A4:AE:12:00:11:22");
            put(&d.join("id/vendor"), "054c");
            put(&d.join("id/product"), "09cc");
            put(&d.join("id/bustype"), "0005");
            std::fs::create_dir_all(d.join(if js { "js0" } else { "x" })).unwrap();
            std::fs::create_dir_all(d.join("event7")).unwrap();
            symlink(&hid, d.join("device")).unwrap();
            symlink(&d, classes.join(n)).unwrap();
        }
        // a keyboard: not a joystick
        let kb = root.join("sys/devices/platform/i8042/input/input3");
        put(&kb.join("name"), "AT keyboard");
        std::fs::create_dir_all(kb.join("event3")).unwrap();
        symlink(&kb, classes.join("input3")).unwrap();
        // an Xbox controller on xpad (USB), joystick known from udev's data only
        let usb = root.join("sys/devices/pci0000:00/usb1/1-2/1-2:1.0");
        let x = usb.join("input/input30");
        put(&x.join("name"), "Microsoft X-Box One pad");
        put(&x.join("id/vendor"), "045e");
        put(&x.join("id/product"), "02ea");
        put(&x.join("id/bustype"), "0003");
        put(&x.join("event9/dev"), "13:73");
        put(&root.join("run/udev/data/c13:73"), "E:ID_INPUT=1\nE:ID_INPUT_JOYSTICK=1\n");
        symlink(&usb, x.join("device")).unwrap();
        symlink(&x, classes.join("input30")).unwrap();
        root
    }

    #[test]
    fn controllers_are_found_with_battery_and_lights() {
        let root = tree();
        let pads = list_at(&root);
        assert_eq!(pads.len(), 2, "{pads:#?}");
        let ds4 = &pads[0];
        assert_eq!(ds4.model, Model::DualShock4);
        assert_eq!(ds4.connection, Connection::Bluetooth);
        assert_eq!(ds4.mac(), Some("a4:ae:12:00:11:22"));
        assert_eq!(ds4.driver, "playstation");
        assert_eq!(ds4.event.as_deref(), Some(Path::new("/dev/input/event7")));
        let b = ds4.battery.as_ref().unwrap();
        assert_eq!((b.percent, b.describe().as_str(), b.estimate()), (Some(75), "75%", Some(75)));
        assert!(ds4.leds.has_lightbar());
        assert!(ds4.leds.writable, "the test's own files are writable");
        let xbox = &pads[1];
        assert_eq!((xbox.model, xbox.connection, xbox.battery.is_none(), xbox.mac()), (Model::Xbox, Connection::Usb, true, None));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lights_are_written() {
        let root = tree();
        let pads = list_at(&root);
        set_lightbar(&pads[0], [124, 84, 214]).unwrap();
        let led = pads[0].leds.lightbar.clone().unwrap();
        assert_eq!(read(&led.join("multi_intensity")), "124 84 214");
        set_lightbar(&pads[0], [0, 0, 0]).unwrap();
        assert_eq!(read(&led.join("brightness")), "0");
        assert!(set_player(&pads[0], 2).is_err(), "a DualShock 4 has no player lights");
        assert!(set_lightbar(&pads[1], [1, 2, 3]).is_err());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn battery_levels_and_states_read() {
        let d = std::env::temp_dir().join(format!("gamepads-bat-{}", std::process::id()));
        put(&d.join("hid-x-battery/capacity_level"), "Low\n");
        put(&d.join("hid-x-battery/status"), "Charging\n");
        let b = battery_in(&d).unwrap();
        assert_eq!((b.percent, b.level.as_deref(), b.estimate(), b.describe().as_str()), (None, Some("Low"), Some(20), "Charging Low"));
        std::fs::remove_dir_all(&d).ok();
        assert!(battery_in(&d).is_none());
    }

    #[test]
    fn player_patterns_and_colours() {
        assert_eq!(player_pattern(1), [false, false, true, false, false]);
        assert_eq!(player_pattern(4), [true, true, false, true, true]);
        assert_eq!(player_pattern(9), [true; 5]);
        assert_eq!(parse_color("#7c54d6"), Some([124, 84, 214]));
        assert_eq!(parse_color("7c54d6"), None);
        assert_eq!(color_hex([124, 84, 214]), "#7c54d6");
        assert!(info_is_gamepad("Device AA\n\tName: Wireless Controller\n\tIcon: input-gaming\n"));
        assert!(!info_is_gamepad("\tIcon: audio-headset\n"));
        assert!(valid_mac("a4:ae:12:00:11:22") && !valid_mac("a4:ae:12:00:11:22; rm") && !valid_mac("--help"));
    }

    #[test]
    fn settings_round_trip_and_defaults() {
        let p = std::env::temp_dir().join(format!("gamepads-settings-{}/controllers.json", std::process::id()));
        let s = Settings::load_from(&p);
        assert_eq!((s.notify, s.low_at, s.lightbar.is_none()), (true, 20, true));
        let mut s = s;
        s.lightbar = Some("#7c54d6".into());
        s.per_pad.insert("a4:ae:12:00:11:22".into(), PadSettings { lightbar: Some("#ff0000".into()), player: Some(2) });
        s.save_to(&p).unwrap();
        let back = Settings::load_from(&p);
        assert_eq!(back, s);
        let root = tree();
        let pads = list_at(&root);
        assert_eq!(back.lightbar_for(&pads[0]), Some([255, 0, 0]), "its own colour wins");
        assert_eq!(back.lightbar_for(&pads[1]), Some([124, 84, 214]));
        std::fs::remove_dir_all(p.parent().unwrap()).ok();
        std::fs::remove_dir_all(&root).ok();
    }
}
