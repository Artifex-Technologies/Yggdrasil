//! Settings > Peripherals' mouse and keyboard settings: `~/.config/yggdrasil/input.conf`, read with yggdrasil-wm's own parser
//! (`yggdrasil_wm::input::InputConf`, where the file's form is written down), the same file the Hyprland session reads. Taken at
//! start, when the file changes (the config files' watch, configs.rs) and on `reload` (yggdrasil-wm's `apply_input` asks for
//! one after Settings saved the file).
//!
//! - **Key repeat** (`repeat_rate`, `repeat_delay`): what programs are told (wl_keyboard.repeat_info, at once to every keyboard)
//!   and the repeat of the compositor's own held shortcuts (volume, brightness, the magnifier).
//! - **The mouse** (`sensitivity`, `accel_profile`, `scroll_factor`, `natural_scroll`, `left_handed`), for every mouse and per
//!   mouse (`[name]` sections, by the compositor's name for the device; its own settings over every mouse's). Headless there is
//!   one virtual mouse, `yggdrasil-headless-mouse`, moved and clicked from the control socket (`pointer-move`, `scroll`, `click`):
//!   its speed as libinput's flat profile works it out (the movement times 1 + sensitivity), the scroll factor and natural
//!   scrolling as Hyprland applies them, the main button on the right when left-handed. The adaptive profile's curve is
//!   libinput's own: on the hardware backend libinput is told the speed and profile; headless an adaptive mouse moves as a flat
//!   one at that speed.
//! - **Touchpads and TrackPoints** (hardware backend; `settings`): a device's kind decides what it takes. A touchpad: tap to
//!   click, tap and drag, drag lock, disable while typing, the click method, middle-button emulation, the scroll method, and
//!   natural scrolling and the scroll factor of its own (`[touchpad]`, not every mouse's), with the Hyprland session's
//!   defaults (taps click and drag, nothing while typing, the bottom corners are its buttons, natural scrolling). A TrackPoint
//!   (by its name, and a touchpad's companion "-mouse" device, which carries a ThinkPad's TrackPoint buttons): its speed from
//!   `[trackpoint]`, and it scrolls while its middle button is held, as the session sets it. A device's own section comes first.
//! - **Device names**: as Hyprland names them (lower case, spaces as `-`, `-1`, `-2` ... after a name already taken), so
//!   input.conf's sections and Settings > Peripherals mean the same device on both.

use smithay::backend::input::InputTime;
use yggdrasil_wm::input::{self, Accel, ClickMethod, InputConf, Pointer, ScrollMethod};

/// The headless backend's devices (the control socket's input): their names as `devices` lists them.
pub const HEADLESS_MOUSE: &str = "yggdrasil-headless-mouse";
pub const HEADLESS_KEYBOARD: &str = "yggdrasil-headless-keyboard";

/// What a mouse does in the end: its own settings over every mouse's, the defaults for the rest.
#[derive(Clone, Debug, PartialEq)]
pub struct Mouse {
    /// -1 (slowest) to 1 (fastest); 0 is the mouse's own speed
    pub speed: f64,
    /// None: libinput's default (adaptive)
    pub accel: Option<Accel>,
    pub scroll: f64,
    pub natural: bool,
    pub left_handed: bool,
}

/// What a device is, which decides the settings it takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Mouse,
    Touchpad,
    TrackPoint,
    Keyboard,
    /// a touchscreen
    Touch,
    /// a pen (a tablet tool: the screen's own, or a drawing tablet)
    Pen,
    /// the lid and the tablet-mode switch
    Switch,
    Other,
}

impl Kind {
    pub fn word(self) -> &'static str {
        match self {
            Kind::Mouse => "pointer",
            Kind::Touchpad => "touchpad",
            Kind::TrackPoint => "trackpoint",
            Kind::Keyboard => "keyboard",
            Kind::Touch => "touch",
            Kind::Pen => "tablet",
            Kind::Switch => "switch",
            Kind::Other => "other",
        }
    }

    /// A pointing device (Settings > Peripherals lists these as "pointer", touchpads too, as Hyprland's "mice").
    pub fn points(self) -> bool {
        matches!(self, Kind::Mouse | Kind::Touchpad | Kind::TrackPoint)
    }
}

/// One input device the compositor has: libinput's name for it, its name here (as Hyprland's) and its kind.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub raw: String,
    pub name: String,
    pub kind: Kind,
}

/// The compositor's name for a device, as Hyprland makes it: lower case, spaces (and line breaks) as `-`; a name already taken
/// gets `-1`, `-2` ... after it ("ELAN067B:00 04F3:31F8 Touchpad" is "elan067b:00-04f3:31f8-touchpad").
pub fn device_name(raw: &str, taken: &[String]) -> String {
    let base: String = raw.trim().chars().map(|c| if c == ' ' || c == '\n' { '-' } else { c.to_ascii_lowercase() }).collect();
    let mut name = base.clone();
    let mut n = 0;
    while taken.contains(&name) {
        n += 1;
        name = format!("{base}-{n}");
    }
    name
}

/// What libinput says a device can do, for `kind_of`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Caps {
    pub pointer: bool,
    pub keyboard: bool,
    pub touch: bool,
    pub tablet_tool: bool,
    pub switch: bool,
    /// libinput's tap finger count: more than 0 only on a touchpad
    pub tap_fingers: u32,
}

/// A device's kind from its capabilities and name. `touchpads`: the names of the touchpads there are (a touchpad's companion
/// "NAME-mouse" device is a TrackPoint's buttons, as in the session's rule).
pub fn kind_of(name: &str, caps: Caps, touchpads: &[String]) -> Kind {
    if caps.switch {
        Kind::Switch
    } else if caps.tablet_tool {
        Kind::Pen
    } else if caps.touch {
        Kind::Touch
    } else if caps.pointer && caps.tap_fingers > 0 {
        Kind::Touchpad
    } else if caps.pointer
        && (name.contains("trackpoint")
            || name.strip_suffix("-mouse").is_some_and(|base| touchpads.iter().any(|t| t.strip_suffix("-touchpad") == Some(base))))
    {
        Kind::TrackPoint
    } else if caps.pointer {
        Kind::Mouse
    } else if caps.keyboard {
        Kind::Keyboard
    } else {
        Kind::Other
    }
}

/// The settings a device takes in the end: its own section, then its kind's ([touchpad], [trackpoint]), then every mouse's (a
/// touchpad without their scrolling), then the kind's defaults (the Hyprland session's).
pub fn settings(conf: &InputConf, device: &str, kind: Kind) -> Pointer {
    let own = conf.devices.get(input::group_name(device)).cloned().unwrap_or_default();
    let section = match kind {
        Kind::Touchpad => conf.devices.get(input::TOUCHPADS),
        Kind::TrackPoint => conf.devices.get(input::TRACKPOINTS),
        _ => None,
    };
    let all = if kind == Kind::Touchpad { conf.all.touchpad_part() } else { conf.all.clone() };
    let defaults = match kind {
        // Hyprland's own touchpad defaults and the session's natural scrolling
        Kind::Touchpad => Pointer {
            natural_scroll: Some(true),
            tap_to_click: Some(true),
            tap_and_drag: Some(true),
            drag_lock: Some(false),
            disable_while_typing: Some(true),
            click_method: Some(ClickMethod::ButtonAreas),
            middle_button_emulation: Some(false),
            ..Default::default()
        },
        // the session: a TrackPoint scrolls while its middle button is held
        Kind::TrackPoint => Pointer { scroll_method: Some(ScrollMethod::OnButtonDown), scroll_button: Some(0x112), ..Default::default() },
        _ => Pointer::default(),
    };
    own.over(&section.cloned().unwrap_or_default().over(&all.over(&defaults)))
}

/// A mouse's settings from the file (by its name; its other parts NAME-1, NAME-2 share them).
pub fn mouse(conf: &InputConf, device: &str) -> Mouse {
    mouse_of(&settings(conf, device, Kind::Mouse))
}

/// The pointer part of a device's settings.
pub fn mouse_of(p: &Pointer) -> Mouse {
    Mouse {
        speed: p.sensitivity.unwrap_or(0.0),
        accel: p.accel,
        scroll: p.scroll_factor.unwrap_or(1.0),
        natural: p.natural_scroll.unwrap_or(false),
        left_handed: p.left_handed.unwrap_or(false),
    }
}

/// Keys a second and milliseconds before a held key repeats.
pub fn repeat(conf: &InputConf) -> (i32, i32) {
    (conf.repeat_rate.unwrap_or(input::DEFAULT_REPEAT_RATE) as i32, conf.repeat_delay.unwrap_or(input::DEFAULT_REPEAT_DELAY) as i32)
}

/// The button a press of `code` is (left-handed: the left and right buttons swap).
pub fn button(m: &Mouse, code: u32) -> u32 {
    match (m.left_handed, code) {
        (true, 0x110) => 0x111,
        (true, 0x111) => 0x110,
        _ => code,
    }
}

/// How far a scroll of `v` goes: the factor, and the other way round with natural scrolling.
pub fn scroll(m: &Mouse, v: f64) -> f64 {
    v * m.scroll * if m.natural { -1.0 } else { 1.0 }
}

/// One scroll event as programs get it (wl_pointer's axis frame). `h` and `v` are libinput's value for the horizontal and the
/// vertical axis (None: that axis is not in the event) and its wheel steps (120 a notch); `factor` is input.conf's scroll
/// factor. A touchpad (finger) tells that an axis stopped with a value of 0 on it; only then is axis_stop sent for that axis.
/// An axis that is not in the event did not stop: a vertical two-finger scroll is not a stop of the horizontal axis on every
/// frame (the laptop, 2026-11-19).
pub fn axis_frame(
    time: InputTime,
    source: smithay::backend::input::AxisSource,
    factor: f64,
    h: (Option<f64>, Option<f64>),
    v: (Option<f64>, Option<f64>),
) -> smithay::input::pointer::AxisFrame {
    use smithay::backend::input::{Axis, AxisSource};
    let mut frame = smithay::input::pointer::AxisFrame::new(time).source(source);
    for (a, (amount, v120)) in [(Axis::Horizontal, h), (Axis::Vertical, v)] {
        let value = factor * amount.unwrap_or_else(|| v120.unwrap_or(0.0) * 15.0 / 120.0);
        if value != 0.0 {
            frame = frame.value(a, value);
            if let Some(d) = v120 {
                frame = frame.v120(a, d as i32);
            }
        }
        if source == AxisSource::Finger && amount == Some(0.0) {
            frame = frame.stop(a);
        }
    }
    frame
}

/// How far the pointer goes for a movement of the mouse (libinput's flat profile: times 1 + speed).
pub fn motion(m: &Mouse, (dx, dy): (f64, f64)) -> (f64, f64) {
    let f = 1.0 + m.speed.clamp(-1.0, 1.0);
    (dx * f, dy * f)
}

/// One line for the hardware backend's `input`: a device, its kind and the settings in force for it (libinput's default where
/// nothing is set: "default").
pub fn describe_device(d: &DeviceInfo, conf: &InputConf) -> String {
    let mut line = format!("{}\t{}", d.name, d.kind.word());
    if !d.kind.points() {
        return line;
    }
    let p = settings(conf, &d.name, d.kind);
    line.push('\t');
    line.push_str(&describe(&d.name, &mouse_of(&p)).split_once('\t').map(|x| x.1.to_string()).unwrap_or_default());
    if d.kind == Kind::Mouse {
        return line;
    }
    let b = |v: Option<bool>| v.map(|b| (b as u8).to_string()).unwrap_or_else(|| "default".into());
    line.push_str(&format!(
        "\ttap_to_click={}\ttap_and_drag={}\tdrag_lock={}\tdisable_while_typing={}\tclick_method={}\tmiddle_button_emulation={}\tscroll_method={}\tscroll_button={}",
        b(p.tap_to_click),
        b(p.tap_and_drag),
        b(p.drag_lock),
        b(p.disable_while_typing),
        p.click_method.map(ClickMethod::word).unwrap_or("default"),
        b(p.middle_button_emulation),
        p.scroll_method.map(ScrollMethod::word).unwrap_or("default"),
        p.scroll_button.map(|v| v.to_string()).unwrap_or_else(|| "default".into()),
    ));
    line
}

/// One line for the control socket's `input`: a mouse's settings in force.
pub fn describe(name: &str, m: &Mouse) -> String {
    format!(
        "{name}\tsensitivity={}\taccel_profile={}\tscroll_factor={}\tnatural_scroll={}\tleft_handed={}",
        m.speed,
        m.accel.map(Accel::word).unwrap_or("default"),
        m.scroll,
        m.natural as u8,
        m.left_handed as u8
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_touchpad_scroll_stops_only_the_axis_that_stopped() {
        use smithay::backend::input::AxisSource;
        // a vertical two-finger scroll: the horizontal axis is not in the event, and it did not stop
        let f = super::axis_frame(
            smithay::backend::input::InputTime::from_millis(10),
            AxisSource::Finger,
            1.0,
            (None, None),
            (Some(-3.5), None),
        );
        assert_eq!((f.axis, f.stop, f.source), ((0.0, -3.5), (false, false), Some(AxisSource::Finger)));
        // the fingers lifted: libinput's 0 on the vertical axis is its stop
        let f = super::axis_frame(
            smithay::backend::input::InputTime::from_millis(20),
            AxisSource::Finger,
            1.0,
            (None, None),
            (Some(0.0), None),
        );
        assert_eq!(f.stop, (false, true));
        let f = super::axis_frame(
            smithay::backend::input::InputTime::from_millis(20),
            AxisSource::Finger,
            1.0,
            (Some(0.0), None),
            (Some(0.0), None),
        );
        assert_eq!(f.stop, (true, true), "both stopped");
        // a wheel never stops; its notches are kept, the value comes from them when libinput gives none, times the factor
        let f = super::axis_frame(
            smithay::backend::input::InputTime::from_millis(30),
            AxisSource::Wheel,
            2.0,
            (None, None),
            (None, Some(120.0)),
        );
        assert_eq!((f.axis, f.v120, f.stop), ((0.0, 30.0), Some((0, 120)), (false, false)));
        let f = super::axis_frame(
            smithay::backend::input::InputTime::from_millis(30),
            AxisSource::Wheel,
            1.0,
            (None, None),
            (Some(0.0), Some(0.0)),
        );
        assert_eq!(f.stop, (false, false));
    }

    use super::*;

    #[test]
    fn a_mouse_takes_its_own_section_over_every_mouses() {
        let conf = InputConf::parse(
            "sensitivity = 0.5\nnatural_scroll = 1\nrepeat_rate = 40\nrepeat_delay = 300\n[yggdrasil-headless-mouse]\nsensitivity = -0.5\naccel_profile = flat\nleft_handed = 1\n",
        );
        let m = mouse(&conf, HEADLESS_MOUSE);
        assert_eq!(m, Mouse { speed: -0.5, accel: Some(Accel::Flat), scroll: 1.0, natural: true, left_handed: true });
        assert_eq!(mouse(&conf, "another-mouse-1").speed, 0.5, "another mouse: every mouse's");
        assert_eq!(repeat(&conf), (40, 300));
        assert_eq!(repeat(&InputConf::default()), (25, 600), "Hyprland's defaults");
        assert_eq!(button(&m, 0x110), 0x111);
        assert_eq!(button(&m, 0x112), 0x112);
        assert_eq!(scroll(&m, 10.0), -10.0);
        assert_eq!(motion(&m, (100.0, -40.0)), (50.0, -20.0));
    }

    /// This laptop's devices as libinput names them (a ThinkPad X1 Yoga): Hyprland's names for them and their kinds.
    #[test]
    fn devices_are_named_as_hyprland_names_them_and_known_by_kind() {
        assert_eq!(device_name("ELAN067B:00 04F3:31F8 Touchpad", &[]), "elan067b:00-04f3:31f8-touchpad");
        assert_eq!(device_name("TPPS/2 Elan TrackPoint", &[]), "tpps/2-elan-trackpoint");
        let taken = vec!["wacom-hid-5349-pen".to_string()];
        assert_eq!(device_name("Wacom HID 5349 Pen", &taken), "wacom-hid-5349-pen-1", "a second device of the same name");
        let pad = Caps { pointer: true, tap_fingers: 3, ..Default::default() };
        let pointer = Caps { pointer: true, ..Default::default() };
        let pads = vec!["elan067b:00-04f3:31f8-touchpad".to_string()];
        assert_eq!(kind_of("elan067b:00-04f3:31f8-touchpad", pad, &pads), Kind::Touchpad);
        assert_eq!(kind_of("tpps/2-elan-trackpoint", pointer, &pads), Kind::TrackPoint);
        assert_eq!(kind_of("elan067b:00-04f3:31f8-mouse", pointer, &pads), Kind::TrackPoint, "the touchpad's companion");
        assert_eq!(kind_of("example-usb-mouse", pointer, &pads), Kind::Mouse);
        assert_eq!(kind_of("wacom-hid-5349-finger", Caps { touch: true, ..Default::default() }, &pads), Kind::Touch);
        assert_eq!(kind_of("wacom-hid-5349-pen", Caps { tablet_tool: true, ..Default::default() }, &pads), Kind::Pen);
        assert_eq!(kind_of("lid-switch", Caps { switch: true, ..Default::default() }, &pads), Kind::Switch);
        assert_eq!(kind_of("at-translated-set-2-keyboard", Caps { keyboard: true, ..Default::default() }, &pads), Kind::Keyboard);
    }

    #[test]
    fn a_touchpad_takes_its_own_settings_with_the_sessions_defaults() {
        let pad = "elan067b:00-04f3:31f8-touchpad";
        // nothing in the file: Hyprland's touchpad defaults and the session's natural scrolling
        let p = settings(&InputConf::default(), pad, Kind::Touchpad);
        assert_eq!(
            (p.tap_to_click, p.tap_and_drag, p.drag_lock, p.disable_while_typing),
            (Some(true), Some(true), Some(false), Some(true))
        );
        assert_eq!(
            (p.click_method, p.middle_button_emulation, p.natural_scroll),
            (Some(ClickMethod::ButtonAreas), Some(false), Some(true))
        );
        assert_eq!(p.scroll_method, None, "libinput's own (two fingers)");
        // every mouse's natural scrolling and scroll factor are a mouse's; their speed is every device's
        let conf = InputConf::parse(
            "sensitivity = 0.3\nnatural_scroll = 0\nscroll_factor = 3\n[touchpad]\ntap_to_click = 0\nclick_method = clickfinger\n\
             scroll_method = edge\n[elan067b:00-04f3:31f8-touchpad]\ntap_to_click = 1\n[trackpoint]\nsensitivity = -0.6\n",
        );
        let p = settings(&conf, pad, Kind::Touchpad);
        assert_eq!((p.sensitivity, p.natural_scroll, p.scroll_factor), (Some(0.3), Some(true), None));
        assert_eq!(
            (p.tap_to_click, p.click_method, p.scroll_method),
            (Some(true), Some(ClickMethod::Clickfinger), Some(ScrollMethod::Edge)),
            "its own section first"
        );
        let m = settings(&conf, "example-usb-mouse", Kind::Mouse);
        assert_eq!(
            (m.natural_scroll, m.scroll_factor, m.tap_to_click),
            (Some(false), Some(3.0), None),
            "a mouse takes no touchpad section"
        );
        // the TrackPoint: its own speed, scrolling on the middle button
        let t = settings(&conf, "tpps/2-elan-trackpoint", Kind::TrackPoint);
        assert_eq!((t.sensitivity, t.scroll_method, t.scroll_button), (Some(-0.6), Some(ScrollMethod::OnButtonDown), Some(0x112)));
        assert_eq!(mouse_of(&t).speed, -0.6);
        let d = DeviceInfo { raw: "x".into(), name: pad.into(), kind: Kind::Touchpad };
        let line = describe_device(&d, &conf);
        assert!(line.starts_with(&format!("{pad}\ttouchpad\tsensitivity=0.3\t")), "{line}");
        assert!(
            line.contains("\ttap_to_click=1\t")
                && line.contains("\tclick_method=clickfinger\t")
                && line.ends_with("\tscroll_button=default"),
            "{line}"
        );
    }
}
