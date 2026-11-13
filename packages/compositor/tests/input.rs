//! Settings > Peripherals' mouse and keyboard settings on the compositor: `~/.config/yggdrasil/input.conf` (yggdrasil-wm's form,
//! the file the Hyprland session reads) taken at start and again when Settings applies a change (yggdrasil-wm's `apply_input`:
//! a reload here). Key repeat as programs are told it; the headless virtual mouse's speed, left-handed buttons, scroll factor
//! and natural scrolling, every mouse's and in a section of its own; the devices as yggdrasil-wm lists them for Settings.

mod common;

use std::ffi::OsString;

use common::*;
use yggdrasil_wm::input::{DeviceKind, InputConf};
use yggdrasil_wm::Desk;

const BLUE: u32 = 0xff0000ff;
const LEFT: u32 = 0x110;
const RIGHT: u32 = 0x111;
const VERTICAL: u32 = 0;

fn desk(comp: &Compositor) -> Desk {
    let run = comp.dir.join("run").into_os_string();
    let display: OsString = comp.wayland.file_name().unwrap().into();
    let d = Desk::from_env(|k| match k {
        "XDG_RUNTIME_DIR" => Some(run.clone()),
        "WAYLAND_DISPLAY" => Some(display.clone()),
        _ => None,
    })
    .expect("the compositor is found");
    assert!(matches!(d, Desk::Own(_)), "{d:?}");
    d
}

#[test]
fn input_conf_is_taken_at_start_and_when_settings_applies_it() {
    let _l = limit(60, "input.conf");
    // as Settings > Peripherals writes it: every mouse a little faster and scrolling naturally, keys repeating sooner; the
    // headless mouse slower, flat and left-handed in a section of its own
    let conf = InputConf::parse(
        "sensitivity = 0.5\nnatural_scroll = 1\nscroll_factor = 2\nrepeat_rate = 40\nrepeat_delay = 300\n\n[yggdrasil-headless-mouse]\nsensitivity = -0.5\naccel_profile = flat\nleft_handed = 1\n",
    );
    let comp = Compositor::start_with(&["--size", "800x600"], |dir| {
        conf.save_to(&dir.join("home/.config/yggdrasil/input.conf")).unwrap();
    });
    assert_eq!(
        comp.must("input"),
        [
            "yggdrasil-headless-keyboard\trepeat_rate=40\trepeat_delay=300",
            "yggdrasil-headless-mouse\tsensitivity=-0.5\taccel_profile=flat\tscroll_factor=2\tnatural_scroll=1\tleft_handed=1"
        ]
    );
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    assert_eq!(c.state.repeat_info, Some((40, 300)), "programs are told the key repeat");
    let w = c.window("test.w", "W");
    c.draw(&w, (400, 300), BLUE);
    let info = comp.window("test.w").unwrap();

    // the mouse at half speed: 100 to the right moves the pointer 50
    comp.must(&format!("pointer {} {}", info.at.0 + 100, info.at.1 + 100));
    comp.must("pointer-move 100 40");
    assert_eq!(comp.must("cursor"), [format!("{} {}", info.at.0 + 150, info.at.1 + 120)]);
    c.until("the motion", |s| s.pointer_at == (150.0, 120.0));
    // left-handed: its left button is the right one
    comp.must("click left");
    comp.must("click right");
    c.until("two clicks", |s| s.buttons.len() == 2);
    assert_eq!(c.state.buttons, [RIGHT, LEFT]);
    // natural scrolling at twice the distance: a turn down scrolls up, twice as far
    comp.must("scroll 10");
    c.until("the scroll", |s| !s.axis.is_empty());
    assert_eq!(c.state.axis, [(VERTICAL, -20.0)]);
    // the pointer stays on the screens
    comp.must("pointer-move -60000 0");
    assert_eq!(comp.must("cursor")[0].split(' ').next(), Some("0"));

    // Settings > Peripherals changes them: saved, then applied through yggdrasil-wm (a reload here)
    let d = desk(&comp);
    let devices = d.input_devices().expect("the devices");
    assert!(devices.iter().any(|x| x.name == "yggdrasil-headless-mouse" && x.kind == DeviceKind::Pointer), "{devices:?}");
    assert_eq!(yggdrasil_wm::input::mice(&devices), ["yggdrasil-headless-mouse"], "Settings offers the mouse");
    let now = InputConf::parse("repeat_rate = 30\nrepeat_delay = 500\n");
    now.save_to(&comp.dir.join("home/.config/yggdrasil/input.conf")).unwrap();
    d.apply_input(&conf, &now).expect("applied at once");
    c.until("the new key repeat", |s| s.repeat_info == Some((30, 500)));
    assert_eq!(
        comp.must("input")[1],
        "yggdrasil-headless-mouse\tsensitivity=0\taccel_profile=default\tscroll_factor=1\tnatural_scroll=0\tleft_handed=0"
    );
    comp.must(&format!("pointer {} {}", info.at.0 + 100, info.at.1 + 100));
    comp.must("click left");
    c.until("a third click", |s| s.buttons.len() == 3);
    assert_eq!(c.state.buttons[2], LEFT, "right-handed again");
    // no file: the defaults (Hyprland's)
    std::fs::remove_file(comp.dir.join("home/.config/yggdrasil/input.conf")).unwrap();
    d.apply_input(&now, &InputConf::default()).unwrap();
    c.until("the default key repeat", |s| s.repeat_info == Some((25, 600)));
}
