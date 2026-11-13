//! Settings > Peripherals' mouse and keyboard settings: `~/.config/yggdrasil/input.conf`, read with yggdrasil-wm's own parser
//! (`yggdrasil_wm::input::InputConf`, where the file's form is written down), the same file the Hyprland session reads. Taken at
//! start and on `reload` (yggdrasil-wm's `apply_input` asks for one after Settings saved the file) and the 2-second look when
//! the file changed.
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

use yggdrasil_wm::input::{self, Accel, InputConf};

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

/// A mouse's settings from the file (by its name; its other parts NAME-1, NAME-2 share them).
pub fn mouse(conf: &InputConf, device: &str) -> Mouse {
    let own = conf.devices.get(input::group_name(device)).cloned().unwrap_or_default();
    let p = own.over(&conf.all);
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

/// How far the pointer goes for a movement of the mouse (libinput's flat profile: times 1 + speed).
pub fn motion(m: &Mouse, (dx, dy): (f64, f64)) -> (f64, f64) {
    let f = 1.0 + m.speed.clamp(-1.0, 1.0);
    (dx * f, dy * f)
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
}
