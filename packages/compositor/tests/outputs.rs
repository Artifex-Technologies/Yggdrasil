//! The system's settings for input and the screens, as the Hyprland session takes them (packages/session/hyprland/yggdrasil.lua):
//! the keyboard layout from the file the installer writes (/etc/X11/xorg.conf.d/00-keyboard.conf; here the test's own etc
//! folder), Settings > Displays' file (~/.config/yggdrasil/monitors.lua, in the form Settings writes it) read through
//! yggdrasil-wm as Settings does, the standard wlr-output-management protocol (list, change, test, refuse), and screens plugged
//! in and out while running, with the windows moving off a screen that goes.
//!
//! Every test starts a compositor of its own in a private folder and has a time limit.

mod common;

use std::ffi::OsString;
use std::time::{Duration, Instant};

use common::*;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols_wlr::output_management::v1::client::zwlr_output_manager_v1::ZwlrOutputManagerV1;
use yggdrasil_wm::Desk;

const RED: u32 = 0xffff0000;
const BLUE: u32 = 0xff0000ff;
const YELLOW: u32 = 0xffffff00;

/// The keyboard file as packages/install-system writes it.
fn x11_keyboard(layout: &str, variant: &str) -> String {
    let v = if variant.is_empty() { String::new() } else { format!("    Option \"XkbVariant\" \"{variant}\"\n") };
    format!("Section \"InputClass\"\n    Identifier \"system-keyboard\"\n    MatchIsKeyboard \"on\"\n    Option \"XkbLayout\" \"{layout}\"\n{v}EndSection\n")
}

fn write_etc(dir: &std::path::Path, file: &str, text: &str) {
    let p = dir.join("etc").join(file);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// yggdrasil-wm as a program inside this compositor finds it (Settings does the same).
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

/// Draw a window again at the size the compositor last gave it, until it is listed at that size.
fn settle(comp: &Compositor, c: &mut Client, w: &Window, app_id: &str, argb: u32) -> WinInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        c.roundtrip();
        let given = c.state.wins[w.index].size;
        let info = comp.window(app_id).unwrap_or_else(|| panic!("{app_id} is listed"));
        if given.0 <= 0 || info.size == given {
            return info;
        }
        c.draw(w, given, argb);
        assert!(Instant::now() < deadline, "{app_id} did not settle at {given:?}: {info:?}");
    }
}

#[test]
fn the_keyboard_layout_is_the_systems() {
    let _l = limit(60, "the system's keyboard layout");
    let comp = Compositor::start_with(&["--size", "800x600"], |dir| {
        write_etc(dir, "X11/xorg.conf.d/00-keyboard.conf", &x11_keyboard("de", "nodeadkeys"));
        write_etc(dir, "vconsole.conf", "KEYMAP=de-latin1-nodeadkeys\nXKBLAYOUT=de\nXKBVARIANT=nodeadkeys\n");
    });
    assert_eq!(comp.must("keyboard"), ["de nodeadkeys"]);
    // a program is given that keymap
    let mut c = comp.connect();
    let _input = c.input();
    c.until("a keymap", |s| s.keymap);
    assert!(
        c.state.keymap_text.contains("xkb_symbols \"pc_de(nodeadkeys)_"),
        "the German keymap: {}",
        &c.state.keymap_text[..400.min(c.state.keymap_text.len())]
    );

    // the system's file changes (`localectl set-x11-keymap fr`): taken within the 2-second look, and programs get the new keymap
    write_etc(&comp.dir, "X11/xorg.conf.d/00-keyboard.conf", &x11_keyboard("fr", ""));
    c.until("the French keymap", |s| s.keymap_text.contains("xkb_symbols \"pc_fr_"));
    assert_eq!(comp.must("keyboard"), ["fr"]);
    assert_eq!(c.state.keymaps, 2);

    // the installer's try-out sets one from outside; it stays while the system's file does not change
    let d = desk(&comp);
    assert!(d.set_keyboard("us", ""));
    std::thread::sleep(Duration::from_millis(2500));
    assert_eq!(comp.must("keyboard"), ["us"], "the 2-second look leaves it alone");
}

#[test]
fn without_the_x11_file_vconsole_conf_then_us() {
    let _l = limit(60, "the keyboard's second and last choice");
    let comp = Compositor::start_with(&["--size", "640x480"], |dir| write_etc(dir, "vconsole.conf", "KEYMAP=fr-latin9\nXKBLAYOUT=fr\n"));
    assert_eq!(comp.must("keyboard"), ["fr"], "vconsole.conf when there is no X11 file");
    drop(comp);
    let comp = Compositor::start(&["--size", "640x480"]);
    assert_eq!(comp.must("keyboard"), ["us"], "neither: US, as the session");
    drop(comp);
    let comp = Compositor::start_with(&["--size", "640x480"], |dir| {
        write_etc(dir, "X11/xorg.conf.d/00-keyboard.conf", &x11_keyboard("no-such-layout", ""))
    });
    assert!(comp.must("keyboard").is_empty(), "a layout xkb cannot load: its default");
    let mut c = comp.connect();
    let _input = c.input();
    c.until("a keymap", |s| s.keymap);
    assert!(c.state.keymap_text.contains("xkb_symbols \"pc_us_"), "programs still get a keymap");
    comp.wait(5, "the reason in the log", |c| c.log_tail(50).contains("could not be loaded"));
}

/// monitors.lua as Settings > Displays writes it (packages/settings: hypr_monitors_lua).
fn monitors_lua(lines: &[&str]) -> String {
    let mut t = String::from("-- Written by Settings > Displays. Loaded by Yggdrasil's Hyprland config.\n");
    for l in lines {
        t.push_str(l);
        t.push('\n');
    }
    t
}

fn write_monitors(dir: &std::path::Path, text: &str) {
    std::fs::write(dir.join("home/.config/yggdrasil/monitors.lua"), text).unwrap();
}

#[test]
fn settings_monitors_file_sets_modes_scales_places_and_screens_off() {
    let _l = limit(60, "Settings > Displays' file");
    let comp = Compositor::start_with(&["--outputs", "3", "--size", "1600x900"], |dir| {
        write_monitors(
            dir,
            &monitors_lua(&[
                "hl.monitor({ output = \"HEADLESS-1\", mode = \"1280x720@60.00\", position = \"0x0\", scale = 1 })",
                "hl.monitor({ output = \"HEADLESS-2\", mode = \"1600x900@60.00\", position = \"-1280x100\", scale = 1.25 })",
                "hl.monitor({ output = \"HEADLESS-3\", disabled = true })",
            ]),
        );
    });
    assert_eq!(
        comp.must("outputs"),
        [
            "HEADLESS-1 1280x720 scale=1 at=0,0 logical=1280x720",
            "HEADLESS-2 1600x900 scale=1.25 at=-1280,100 logical=1280x720",
            "HEADLESS-3 1600x900 scale=1 at=3200,0 logical=1600x900 disabled",
        ]
    );
    // programs see the two that are on, where they are
    let c = comp.connect();
    let seen: Vec<String> = c.state.outputs.iter().map(|o| format!("{} {:?} {:?}", o.name, o.position, o.logical)).collect();
    assert_eq!(seen, ["HEADLESS-1 (0, 0) (1280, 720)", "HEADLESS-2 (-1280, 100) (1280, 720)"]);

    // Settings lists them through yggdrasil-wm: every screen, the one that is off too, with its rate and modes
    let d = desk(&comp);
    let all = d.screens_all();
    let names: Vec<(&str, bool)> = all.iter().map(|s| (s.name.as_str(), s.disabled)).collect();
    assert_eq!(names, [("HEADLESS-1", false), ("HEADLESS-2", false), ("HEADLESS-3", true)]);
    assert_eq!((all[0].width, all[0].height, all[0].refresh), (1280, 720, Some(60.0)));
    assert_eq!(all[0].modes, ["1600x900@60.00Hz", "1280x720@60.00Hz"], "its own mode and the one Settings chose");
    assert_eq!((all[1].x, all[1].y, all[1].scale), (-1280, 100, Some(1.25)));

    // Settings changes them: it writes the file and asks for a reload (as on Hyprland)
    write_monitors(
        &comp.dir,
        &monitors_lua(&[
            "hl.monitor({ output = \"HEADLESS-1\", mode = \"1600x900@60.00\", position = \"0x0\", scale = 1 })",
            "hl.monitor({ output = \"HEADLESS-2\", mode = \"1600x900@60.00\", position = \"-1280x100\", scale = 1.25 })",
            "hl.monitor({ output = \"HEADLESS-3\", mode = \"1600x900@60.00\", position = \"1600x0\", scale = 2 })",
        ]),
    );
    d.reload().unwrap();
    assert_eq!(
        comp.must("outputs"),
        [
            "HEADLESS-1 1600x900 scale=1 at=0,0 logical=1600x900",
            "HEADLESS-2 1600x900 scale=1.25 at=-1280,100 logical=1280x720",
            "HEADLESS-3 1600x900 scale=2 at=1600,0 logical=800x450",
        ]
    );
    assert_eq!(comp.connect().state.outputs.len(), 3, "the screen switched on is offered to programs again");

    // no file: every screen as it is by itself (the command line's size and scale), side by side
    std::fs::remove_file(comp.dir.join("home/.config/yggdrasil/monitors.lua")).unwrap();
    d.reload().unwrap();
    assert_eq!(
        comp.must("outputs"),
        [
            "HEADLESS-1 1600x900 scale=1 at=0,0 logical=1600x900",
            "HEADLESS-2 1600x900 scale=1 at=1600,0 logical=1600x900",
            "HEADLESS-3 1600x900 scale=1 at=3200,0 logical=1600x900",
        ]
    );
}

#[test]
fn programs_list_and_change_the_screens_through_wlr_output_management() {
    let _l = limit(60, "wlr-output-management");
    let comp = Compositor::start(&["--outputs", "2", "--size", "1280x720"]);
    let mut c = comp.connect();
    let mgr: ZwlrOutputManagerV1 = c.globals.bind(&c.qh, 4..=4, ()).expect("wlr-output-management");
    c.until("the heads", |s| s.mgmt.dones >= 1);
    {
        let m = &c.state.mgmt;
        let heads: Vec<(&str, bool, (i32, i32), f64)> = m.heads.iter().map(|h| (h.name.as_str(), h.enabled, h.at, h.scale)).collect();
        assert_eq!(heads, [("HEADLESS-1", true, (0, 0), 1.0), ("HEADLESS-2", true, (1280, 0), 1.0)]);
        assert_eq!(m.current_size("HEADLESS-1"), Some((1280, 720)));
        let modes: Vec<((i32, i32), i32, bool)> = m.modes_of("HEADLESS-2").into_iter().map(|(s, r, p, _)| (s, r, p)).collect();
        assert_eq!(modes, [((1280, 720), 60_000, true)]);
    }

    // a bar on the second screen, and a window opened there (the pointer is there)
    let bar =
        c.layer_with(1, Layer::Top, "test-bar-2", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&bar, RED);
    comp.must("pointer 1900 300");
    let w = c.window("test.on2", "On 2");
    c.draw(&w, (400, 300), BLUE);
    assert_eq!(comp.window("test.on2").unwrap().rect(), (1720, 226, 400, 300), "centred in the second screen's free room");

    // the first screen at another mode and scale, the second off
    let head = |c: &Client, n: &str| c.state.mgmt.head(n).head.clone();
    let (h1, h2) = (head(&c, "HEADLESS-1"), head(&c, "HEADLESS-2"));
    let serial = c.state.mgmt.serial.unwrap();
    let cfg = mgr.create_configuration(serial, &c.qh, 0);
    let ch = cfg.enable_head(&h1, &c.qh, ());
    ch.set_custom_mode(1600, 900, 0);
    ch.set_scale(1.25);
    ch.set_position(0, 0);
    cfg.disable_head(&h2);
    cfg.apply();
    c.until("the answer", |s| s.mgmt.answers.contains_key(&0));
    assert_eq!(c.state.mgmt.answers[&0], "succeeded");
    cfg.destroy();
    assert_eq!(
        comp.must("outputs"),
        ["HEADLESS-1 1600x900 scale=1.25 at=0,0 logical=1280x720", "HEADLESS-2 1280x720 scale=1 at=1280,0 logical=1280x720 disabled"]
    );
    c.until("the changes told", |s| !s.mgmt.head("HEADLESS-2").enabled && s.mgmt.current_size("HEADLESS-1") == Some((1600, 900)));
    assert_eq!(c.state.mgmt.head("HEADLESS-1").scale, 1.25);
    assert!(
        c.state.mgmt.modes_of("HEADLESS-1").iter().any(|(s, r, p, _)| *s == (1600, 900) && *r == 60_000 && !p),
        "the custom mode is listed"
    );
    // the bar of the screen that went is closed; the window moved to the first screen, at its place on it
    c.until("the bar closed", |s| s.layer_closed);
    assert_eq!(comp.window("test.on2").unwrap().rect(), (440, 226, 400, 300));

    // a configuration made for an older state is cancelled; one with every screen off fails; a test changes nothing
    let old = mgr.create_configuration(serial, &c.qh, 1);
    old.enable_head(&h1, &c.qh, ());
    old.enable_head(&h2, &c.qh, ());
    old.apply();
    c.until("cancelled", |s| s.mgmt.answers.contains_key(&1));
    assert_eq!(c.state.mgmt.answers[&1], "cancelled");
    let serial = c.state.mgmt.serial.unwrap();
    let none = mgr.create_configuration(serial, &c.qh, 2);
    none.disable_head(&h1);
    none.disable_head(&h2);
    none.apply();
    c.until("refused", |s| s.mgmt.answers.contains_key(&2));
    assert_eq!(c.state.mgmt.answers[&2], "failed", "at least one screen stays on");
    let test = mgr.create_configuration(serial, &c.qh, 3);
    test.enable_head(&h1, &c.qh, ());
    test.enable_head(&h2, &c.qh, ());
    test.test();
    c.until("tested", |s| s.mgmt.answers.contains_key(&3));
    assert_eq!(c.state.mgmt.answers[&3], "succeeded");
    assert!(comp.must("outputs")[1].ends_with(" disabled"), "a test changes nothing");

    // the second screen on again: to the right of the first, offered to programs again
    let on = mgr.create_configuration(serial, &c.qh, 4);
    on.enable_head(&h1, &c.qh, ());
    on.enable_head(&h2, &c.qh, ());
    on.apply();
    c.until("on again", |s| s.mgmt.answers.contains_key(&4));
    assert_eq!(c.state.mgmt.answers[&4], "succeeded");
    assert_eq!(
        comp.must("outputs"),
        ["HEADLESS-1 1600x900 scale=1.25 at=0,0 logical=1280x720", "HEADLESS-2 1280x720 scale=1 at=1280,0 logical=1280x720"]
    );
    assert_eq!(comp.connect().state.outputs.len(), 2);
}

#[test]
fn screens_plugged_in_and_out_and_the_windows_move_off() {
    let _l = limit(60, "screens that come and go");
    let comp = Compositor::start(&["--size", "1280x720"]);
    let mut c = comp.connect();
    let bar = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&bar, RED);

    // plugged in: the next name, to the right; a program connecting now sees both
    assert_eq!(comp.must("add-output 1024x768"), ["HEADLESS-2"]);
    assert_eq!(comp.must("outputs")[1], "HEADLESS-2 1024x768 scale=1 at=1280,0 logical=1024x768");
    let mut c2 = comp.connect();
    let names: Vec<&str> = c2.state.outputs.iter().map(|o| o.name.as_str()).collect();
    assert_eq!(names, ["HEADLESS-1", "HEADLESS-2"]);

    // on the new screen: a window maximized there, and a floating one in front of it
    comp.must("pointer 1700 300");
    let b = c2.window("test.b", "B");
    c2.draw(&b, (300, 200), YELLOW);
    comp.must("key super+up");
    assert_eq!(settle(&comp, &mut c2, &b, "test.b", YELLOW).rect(), maximized_in((1280, 0, 1024, 768)), "maximized on its own screen");
    let a = c2.window("test.a", "A");
    c2.draw(&a, (400, 300), BLUE);
    assert_eq!(comp.window("test.a").unwrap().rect(), (1592, 234, 400, 300), "centred on the screen the pointer is on");

    // unplugged: the windows go to the first screen, at their place on it, the maximized one maximized there
    comp.must("remove-output HEADLESS-2");
    assert_eq!(comp.must("outputs"), ["HEADLESS-1 1280x720 scale=1 at=0,0 logical=1280x720"]);
    assert_eq!(comp.window("test.a").unwrap().rect(), (312, 234, 400, 300));
    let info = settle(&comp, &mut c2, &b, "test.b", YELLOW);
    assert_eq!(info.rect(), maximized_in((0, 32, 1280, 688)), "maximized in the first screen's free room");
    assert!(info.has("maximized"));
    assert!(comp.ctl("remove-output HEADLESS-1").is_err(), "the last screen stays");
    snapshot!(comp.screenshot("HEADLESS-1"), "outputs-unplugged");

    // plugged back in: a name not used before, and the windows stay where they are
    assert_eq!(comp.must("add-output 1024x768@2"), ["HEADLESS-3"]);
    assert_eq!(comp.must("outputs")[1], "HEADLESS-3 1024x768 scale=2 at=1280,0 logical=512x384");
    assert_eq!(comp.window("test.a").unwrap().rect(), (312, 234, 400, 300));

    // locked, a screen plugged in shows nothing of the session (no lock surface for it yet: black)
    let mut l = comp.connect();
    let mgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).unwrap();
    let lock = mgr.lock(&l.qh, ());
    for (i, o) in l.wl_outputs.clone().iter().enumerate() {
        let s = l.compositor.create_surface(&l.qh, ());
        lock.get_lock_surface(&s, o, &l.qh, i);
    }
    l.until("locked", |s| s.locked);
    assert_eq!(comp.must("add-output 800x600"), ["HEADLESS-4"]);
    comp.must("pointer 2000 100");
    let w = c.window("test.new", "New");
    c.draw(&w, (300, 200), BLUE);
    let pic = comp.screenshot("HEADLESS-4");
    assert_eq!(pic.count((0, 0, 800, 600), (0, 0, 0)), 800 * 600, "all black, the window opened there too");
    lock.unlock_and_destroy();
    l.roundtrip();
    assert_eq!(comp.must("lock"), ["unlocked"]);
}
