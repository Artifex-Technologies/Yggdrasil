//! The mouse and keyboard settings (Settings > Peripherals): pointer speed, acceleration, scrolling, which side the main button
//! is on, and key repeat; for every mouse, and per mouse. They are kept in `~/.config/yggdrasil/input.conf`, which the session
//! reads at every start and reload (packages/session/hyprland/yggdrasil.lua), so they survive restarts; `Desk::apply_input`
//! gives them to the running compositor at once.
//!
//! The file, `key = value` lines (a value that is not understood is left out, so a hand-edited file never breaks the session):
//!
//! ```text
//! sensitivity = -0.25        pointer speed, -1 (slowest) to 1 (fastest); 0 is the mouse's own
//! accel_profile = flat       flat (no acceleration) or adaptive; without it, libinput's default (adaptive)
//! scroll_factor = 1.5        how far a scroll goes, 0.1 to 5 (1: as the mouse says)
//! natural_scroll = 1         the content follows the wheel, as on a touchpad
//! left_handed = 0            the main button on the right
//! repeat_rate = 25           keys a second while one is held, 1 to 100
//! repeat_delay = 600         milliseconds before a held key repeats, 100 to 2000
//! [example-gaming-mouse]    one mouse, by the compositor's name for it (without the -1, -2 ... of its other parts)
//! sensitivity = -0.5         what that mouse does differently (the pointer keys above)
//! ```
//!
//! Yggdrasil's own compositor (packages/compositor) reads the same file with `parse` here, at start and on `reload` (which
//! `apply_input` asks it for), and lists its devices on its control socket's `devices`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Settings > Peripherals' note on Yggdrasil's own compositor, from before it took these settings (2026-11-12); `apply_input`
/// no longer answers it. Goes once Settings no longer shows it.
pub const NOT_YET: &str = "Kept. Yggdrasil's own compositor does not take mouse and keyboard settings yet; Hyprland does.";

pub const DEFAULT_REPEAT_RATE: u32 = 25;
pub const DEFAULT_REPEAT_DELAY: u32 = 600;
pub const SPEED: std::ops::RangeInclusive<f64> = -1.0..=1.0;
pub const SCROLL: std::ops::RangeInclusive<f64> = 0.1..=5.0;
pub const REPEAT_RATE: std::ops::RangeInclusive<u32> = 1..=100;
pub const REPEAT_DELAY: std::ops::RangeInclusive<u32> = 100..=2000;
/// A mouse's other parts the compositor lists as NAME-1, NAME-2 ...: a mouse's settings are given to these as well.
pub const PARTS: u32 = 9;

/// Pointer acceleration: libinput's two profiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accel {
    /// The pointer goes as far as the mouse, whatever the speed (most gamers' choice).
    Flat,
    /// Faster movements go further.
    Adaptive,
}

impl Accel {
    pub fn word(self) -> &'static str {
        match self {
            Accel::Flat => "flat",
            Accel::Adaptive => "adaptive",
        }
    }
}

/// The pointer settings; None leaves it to the default (for a single mouse: to the settings for every mouse).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Pointer {
    pub sensitivity: Option<f64>,
    pub accel: Option<Accel>,
    pub scroll_factor: Option<f64>,
    pub natural_scroll: Option<bool>,
    pub left_handed: Option<bool>,
}

impl Pointer {
    pub fn is_empty(&self) -> bool {
        *self == Pointer::default()
    }

    /// This mouse's settings over `all`'s: what it does in the end.
    pub fn over(&self, all: &Pointer) -> Pointer {
        Pointer {
            sensitivity: self.sensitivity.or(all.sensitivity),
            accel: self.accel.or(all.accel),
            scroll_factor: self.scroll_factor.or(all.scroll_factor),
            natural_scroll: self.natural_scroll.or(all.natural_scroll),
            left_handed: self.left_handed.or(all.left_handed),
        }
    }

    /// Takes one `key = value` line of the file (a key that is not a pointer setting is skipped; a value not understood unsets it).
    fn set(&mut self, key: &str, value: &str) {
        match key {
            "sensitivity" => self.sensitivity = number(value, SPEED),
            "scroll_factor" => self.scroll_factor = number(value, SCROLL),
            "accel_profile" => {
                self.accel = match value {
                    "flat" => Some(Accel::Flat),
                    "adaptive" => Some(Accel::Adaptive),
                    _ => None,
                }
            }
            "natural_scroll" => self.natural_scroll = yes_no(value),
            "left_handed" => self.left_handed = yes_no(value),
            _ => {}
        }
    }

    fn lines(&self, out: &mut String) {
        if let Some(v) = self.sensitivity {
            out.push_str(&format!("sensitivity = {}\n", short(v)));
        }
        if let Some(a) = self.accel {
            out.push_str(&format!("accel_profile = {}\n", a.word()));
        }
        if let Some(v) = self.scroll_factor {
            out.push_str(&format!("scroll_factor = {}\n", short(v)));
        }
        if let Some(b) = self.natural_scroll {
            out.push_str(&format!("natural_scroll = {}\n", b as u8));
        }
        if let Some(b) = self.left_handed {
            out.push_str(&format!("left_handed = {}\n", b as u8));
        }
    }

    /// The Lua table fields Hyprland takes (`input = { ... }`, `hl.device({ ... })`); with `defaults`, a setting that is not
    /// given is put back to Hyprland's default (a change made earlier in the running session is undone).
    fn lua_fields(&self, defaults: bool, out: &mut Vec<String>) {
        let num = |v: Option<f64>, d: f64| v.or(defaults.then_some(d)).map(short);
        if let Some(v) = num(self.sensitivity, 0.0) {
            out.push(format!("sensitivity = {v}"));
        }
        match (self.accel, defaults) {
            (Some(a), _) => out.push(format!("accel_profile = \"{}\"", a.word())),
            (None, true) => out.push("accel_profile = \"\"".into()),
            (None, false) => {}
        }
        if let Some(v) = num(self.scroll_factor, 1.0) {
            out.push(format!("scroll_factor = {v}"));
        }
        for (name, v) in [("natural_scroll", self.natural_scroll), ("left_handed", self.left_handed)] {
            if let Some(b) = v.or(defaults.then_some(false)) {
                out.push(format!("{name} = {b}"));
            }
        }
    }
}

/// Everything in input.conf.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputConf {
    /// Every mouse.
    pub all: Pointer,
    pub repeat_rate: Option<u32>,
    pub repeat_delay: Option<u32>,
    /// One mouse's own settings, by its name (`group_name`).
    pub devices: BTreeMap<String, Pointer>,
}

/// A name the compositor gives an input device that may go into the file and into Lua text as it is: lower-case letters,
/// digits and `-_.:+()`, at most 128.
pub fn device_name_ok(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-_.:+()".contains(c))
}

/// The mouse a device belongs to: Hyprland names a device's parts NAME, NAME-1, NAME-2 ... (a mouse is often a pointer and a
/// keyboard for its extra buttons).
pub fn group_name(name: &str) -> &str {
    match name.rsplit_once('-') {
        Some((base, n)) if !base.is_empty() && !n.is_empty() && n.len() <= 2 && n.chars().all(|c| c.is_ascii_digit()) => base,
        _ => name,
    }
}

/// A name to show for a device name: "example-gaming-mouse" is "Example Gaming Mouse".
pub fn pretty(name: &str) -> String {
    group_name(name)
        .split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next().map(|f| f.to_uppercase().chain(c).collect::<String>()).unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn number(v: &str, range: std::ops::RangeInclusive<f64>) -> Option<f64> {
    v.parse::<f64>().ok().filter(|x| x.is_finite()).map(|x| x.clamp(*range.start(), *range.end()))
}

fn whole(v: &str, range: std::ops::RangeInclusive<u32>) -> Option<u32> {
    v.parse::<u32>().ok().map(|x| x.clamp(*range.start(), *range.end()))
}

fn yes_no(v: &str) -> Option<bool> {
    match v {
        "1" | "yes" | "true" | "on" => Some(true),
        "0" | "no" | "false" | "off" => Some(false),
        _ => None,
    }
}

/// A number as short as it can be written (two decimals at most): 0.5, -0.25, 1.
fn short(v: f64) -> String {
    let s = format!("{:.2}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

impl InputConf {
    pub fn parse(text: &str) -> InputConf {
        let mut c = InputConf::default();
        let mut device: Option<String> = None;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                let name = name.trim();
                // a section whose name is refused: its lines are skipped (not taken as every mouse's)
                device = Some(if device_name_ok(name) { name.to_string() } else { String::new() });
                if let Some(d) = device.as_ref().filter(|d| !d.is_empty()) {
                    c.devices.entry(d.clone()).or_default();
                }
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match (&device, k) {
                (None, "repeat_rate") => c.repeat_rate = whole(v, REPEAT_RATE),
                (None, "repeat_delay") => c.repeat_delay = whole(v, REPEAT_DELAY),
                (None, _) => {
                    c.all.set(k, v);
                }
                (Some(d), _) => {
                    if let Some(p) = c.devices.get_mut(d) {
                        p.set(k, v);
                    }
                }
            }
        }
        c.devices.retain(|_, p| !p.is_empty());
        c
    }

    pub fn text(&self) -> String {
        let mut out = String::from(
            "# Written by Settings > Peripherals; read by Yggdrasil's session at every start and reload.\n# The form is in Yggdrasil's packages/wm/src/input.rs.\n",
        );
        self.all.lines(&mut out);
        if let Some(r) = self.repeat_rate {
            out.push_str(&format!("repeat_rate = {r}\n"));
        }
        if let Some(d) = self.repeat_delay {
            out.push_str(&format!("repeat_delay = {d}\n"));
        }
        for (name, p) in self.devices.iter().filter(|(n, p)| device_name_ok(n) && !p.is_empty()) {
            out.push_str(&format!("\n[{name}]\n"));
            p.lines(&mut out);
        }
        out
    }

    /// The file: ~/.config/yggdrasil/input.conf.
    pub fn path() -> PathBuf {
        yggdrasil_conf::dirs::config().join("input.conf")
    }

    pub fn load() -> InputConf {
        InputConf::load_from(&InputConf::path())
    }

    /// Nothing there (or unreadable) is no settings.
    pub fn load_from(path: &Path) -> InputConf {
        std::fs::read_to_string(path).map(|t| InputConf::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        self.save_to(&InputConf::path())
    }

    /// Written next to the file and renamed over it, so the session never reads half a file.
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        let err = |e: std::io::Error| format!("{}: {e}", path.display());
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(err)?;
        }
        let tmp = path.with_extension("conf.new");
        std::fs::write(&tmp, self.text()).map_err(err)?;
        std::fs::rename(&tmp, path).map_err(err)
    }

    /// The settings for every device, as Hyprland's Lua takes them at run time: what is not set goes back to Hyprland's default.
    pub fn hyprland_lua(&self) -> String {
        let mut f = vec![];
        self.all.lua_fields(true, &mut f);
        f.push(format!("repeat_rate = {}", self.repeat_rate.unwrap_or(DEFAULT_REPEAT_RATE)));
        f.push(format!("repeat_delay = {}", self.repeat_delay.unwrap_or(DEFAULT_REPEAT_DELAY)));
        format!("hl.config({{ input = {{ {} }} }})", f.join(", "))
    }

    /// One mouse's settings as `hl.device` calls, for it and its other parts (NAME-1 ... NAME-9); empty for a name that may not
    /// go into Lua.
    pub fn hyprland_device_lua(name: &str, p: &Pointer) -> Vec<String> {
        if !device_name_ok(name) || p.is_empty() {
            return vec![];
        }
        let mut f = vec![];
        p.lua_fields(false, &mut f);
        std::iter::once(name.to_string())
            .chain((1..=PARTS).map(|n| format!("{name}-{n}")))
            .map(|n| format!("hl.device({{ name = \"{n}\", {} }})", f.join(", ")))
            .collect()
    }
}

/// An input device the compositor has.
#[derive(Clone, Debug, PartialEq)]
pub struct InputDevice {
    pub name: String,
    pub kind: DeviceKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Pointer,
    Keyboard,
}

/// The mice to offer settings for: one per mouse (its parts together), by name, without touchpads (they have their own
/// settings) and without names that may not go into the file.
pub fn mice(devices: &[InputDevice]) -> Vec<String> {
    let mut v: Vec<String> = devices
        .iter()
        .filter(|d| d.kind == DeviceKind::Pointer && !d.name.contains("touchpad"))
        .map(|d| group_name(&d.name).to_string())
        .filter(|n| device_name_ok(n))
        .collect();
    v.sort();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_file_is_read_and_written_back_the_same() {
        let text = "sensitivity = -0.25\naccel_profile = flat\nscroll_factor = 1.5\nnatural_scroll = 1\nleft_handed = no\n\
                    repeat_rate = 30\nrepeat_delay = 400\n\n[example-gaming-mouse]\nsensitivity = -0.5\naccel_profile = adaptive\n";
        let c = InputConf::parse(text);
        assert_eq!(c.all.sensitivity, Some(-0.25));
        assert_eq!(c.all.accel, Some(Accel::Flat));
        assert_eq!((c.all.natural_scroll, c.all.left_handed), (Some(true), Some(false)));
        assert_eq!((c.repeat_rate, c.repeat_delay), (Some(30), Some(400)));
        let m = &c.devices["example-gaming-mouse"];
        assert_eq!((m.sensitivity, m.accel, m.scroll_factor), (Some(-0.5), Some(Accel::Adaptive), None));
        assert_eq!(m.over(&c.all).scroll_factor, Some(1.5), "what a mouse does not set comes from every mouse's");
        assert_eq!(InputConf::parse(&c.text()), c);
        assert!(c.text().contains("\n[example-gaming-mouse]\nsensitivity = -0.5\naccel_profile = adaptive\n"));
    }

    #[test]
    fn what_is_not_understood_is_left_out() {
        let c = InputConf::parse(
            "sensitivity = 7\nscroll_factor = nan\naccel_profile = fast\nrepeat_rate = 0\nrepeat_delay = 99999\nleft_handed = maybe\n\
             [Bad Name\"]\nsensitivity = 0.5\n[ok-mouse]\nrepeat_rate = 5\nwhatever = 1\n[empty]\n",
        );
        assert_eq!(c.all.sensitivity, Some(1.0), "clamped to the range");
        assert_eq!((c.all.scroll_factor, c.all.accel, c.all.left_handed), (None, None, None));
        assert_eq!((c.repeat_rate, c.repeat_delay), (Some(1), Some(2000)));
        assert!(c.devices.is_empty(), "a refused name's lines are skipped; key repeat is for every keyboard; empty sections go");
    }

    #[test]
    fn hyprland_is_given_every_setting_or_its_default() {
        let mut c = InputConf::default();
        assert_eq!(
            c.hyprland_lua(),
            "hl.config({ input = { sensitivity = 0, accel_profile = \"\", scroll_factor = 1, natural_scroll = false, \
             left_handed = false, repeat_rate = 25, repeat_delay = 600 } })"
        );
        c.all = Pointer { sensitivity: Some(0.333), accel: Some(Accel::Flat), ..Default::default() };
        assert!(c.hyprland_lua().starts_with("hl.config({ input = { sensitivity = 0.33, accel_profile = \"flat\", "));
        let p = Pointer { left_handed: Some(true), ..Default::default() };
        let lua = InputConf::hyprland_device_lua("my-mouse", &p);
        assert_eq!(lua.len(), 1 + PARTS as usize);
        assert_eq!(lua[0], "hl.device({ name = \"my-mouse\", left_handed = true })");
        assert_eq!(lua[2], "hl.device({ name = \"my-mouse-2\", left_handed = true })");
        assert!(InputConf::hyprland_device_lua("x\" }) os.exit() --", &p).is_empty(), "only a plain name reaches the Lua");
    }

    #[test]
    fn a_mouses_parts_are_one_mouse() {
        assert_eq!(group_name("example-gaming-mouse-2"), "example-gaming-mouse");
        assert_eq!(group_name("example-gaming-mouse"), "example-gaming-mouse");
        assert_eq!(group_name("logitech-g502-hero-gaming-mouse"), "logitech-g502-hero-gaming-mouse");
        assert_eq!(pretty("example-gaming-mouse-1"), "Example Gaming Mouse");
        let d = |n: &str, kind| InputDevice { name: n.into(), kind };
        let list = [
            d("example-mouse", DeviceKind::Pointer),
            d("example-mouse-2", DeviceKind::Pointer),
            d("example-mouse-1", DeviceKind::Keyboard),
            d("example-touchpad", DeviceKind::Pointer),
            d("example-keyboard", DeviceKind::Keyboard),
        ];
        assert_eq!(mice(&list), ["example-mouse"]);
    }

    #[test]
    fn saved_into_a_folder_of_its_own() {
        let d = std::env::temp_dir().join(format!("ygg-wm-input-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("yggdrasil/input.conf");
        assert_eq!(InputConf::load_from(&p), InputConf::default(), "no file: no settings");
        let mut c = InputConf { repeat_delay: Some(300), ..Default::default() };
        c.devices.insert("example-mouse".into(), Pointer { natural_scroll: Some(true), ..Default::default() });
        c.save_to(&p).unwrap();
        assert_eq!(InputConf::load_from(&p), c);
        assert!(!p.with_extension("conf.new").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
