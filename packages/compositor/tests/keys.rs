//! The shortcut table of Yggdrasil's Hyprland session (packages/session/hyprland/yggdrasil.lua) on the headless backend: every
//! key that starts a program starts the same command line as there (stand-ins on the compositor's PATH write down how they were
//! called; nothing real runs), the window keys do what the session's yggdrasil-close and yggdrasil-snap do, a modifier tapped on
//! its own (Super: the start menu, Ctrl: Find my mouse) runs on its release and still reaches the program, held volume keys
//! repeat, Super+Shift+M shows the minimized windows, and the control socket lists the bindings with the session's words.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use common::*;

const BLUE: u32 = 0xff0000ff;
const YELLOW: u32 = 0xffffff00;

/// The programs the session's shortcuts start; each stand-in writes its name and arguments to `called` in the test's folder.
const PROGRAMS: [&str; 13] = [
    "yggdrasil-panelctl",
    "yggdrasil-clipboard",
    "minos",
    "yggdrasil-settings",
    "ghostty",
    "loginctl",
    "yggdrasil-taskmanager",
    "yggdrasil-shot",
    "yggdrasil-colorpicker",
    "yggdrasil-zoom",
    "yggdrasil-screen-reader",
    "yggdrasil-mediakey",
    "yggdrasil-lock",
];

/// A compositor whose PATH holds only the stand-ins (a command it would find nowhere else does nothing).
fn with_stand_ins(args: &[&str]) -> (Compositor, PathBuf) {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let bin = std::env::temp_dir().join(format!("ygg-keys-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&bin);
    std::fs::create_dir_all(&bin).unwrap();
    let log = bin.join("called");
    for p in PROGRAMS {
        let f = bin.join(p);
        // shell built-ins only: nothing else is on this PATH
        std::fs::write(&f, format!("#!/bin/sh\necho \"${{0##*/}} $*\" >> '{}'\n", log.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    let comp = Compositor::start_env(args, &[("PATH", bin.to_str().unwrap())], |_| {});
    (comp, log)
}

fn calls(log: &Path) -> Vec<String> {
    std::fs::read_to_string(log).unwrap_or_default().lines().map(|l| l.trim_end().to_string()).collect()
}

/// Press `keys` (a combination for the control socket's `key`) and wait until one more call was written down: that call.
fn runs(comp: &Compositor, log: &Path, keys: &str) -> String {
    let before = calls(log).len();
    comp.must(&format!("key {keys}"));
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let now = calls(log);
        if now.len() > before {
            std::thread::sleep(Duration::from_millis(30));
            let now = calls(log);
            assert_eq!(now.len(), before + 1, "{keys} ran one command: {:?}", &now[before..]);
            return now[before].clone();
        }
        assert!(Instant::now() < deadline, "{keys} ran nothing");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Press `keys` and make sure nothing ran.
fn runs_nothing(comp: &Compositor, log: &Path, keys: &str) {
    let before = calls(log).len();
    comp.must(&format!("key {keys}"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(calls(log)[before..], [] as [String; 0], "{keys} runs nothing");
}

#[test]
fn every_key_runs_what_it_runs_in_the_session() {
    let _l = limit(90, "the session's keys");
    let (comp, log) = with_stand_ins(&["--size", "800x600"]);
    for (keys, called) in [
        ("super+n", "yggdrasil-panelctl notifications"),
        ("super+s", "yggdrasil-panelctl ask-press"),
        ("ctrl+alt+s", "yggdrasil-panelctl ask-press"),
        ("super+v", "yggdrasil-clipboard"),
        ("super+c", "yggdrasil-panelctl calendar"),
        ("super+e", "minos"),
        ("super+i", "yggdrasil-settings"),
        ("super+Return", "ghostty"),
        ("super+l", "loginctl lock-session"),
        ("ctrl+shift+Escape", "yggdrasil-taskmanager"),
        ("Print", "yggdrasil-shot"),
        ("super+shift+s", "yggdrasil-shot"),
        ("super+shift+c", "yggdrasil-colorpicker"),
        ("super+slash", "yggdrasil-panelctl shortcuts"),
        ("super+alt+equal", "yggdrasil-zoom in"),
        ("super+alt+minus", "yggdrasil-zoom out"),
        ("super+alt+0", "yggdrasil-zoom off"),
        ("super+alt+s", "yggdrasil-screen-reader"),
        ("super+shift+z", "yggdrasil-panelctl zones"),
        ("XF86AudioRaiseVolume", "yggdrasil-mediakey volume-up"),
        ("XF86AudioLowerVolume", "yggdrasil-mediakey volume-down"),
        ("XF86AudioMute", "yggdrasil-mediakey mute"),
        ("XF86AudioMicMute", "yggdrasil-mediakey mic-mute"),
        ("XF86AudioPlay", "yggdrasil-mediakey play-pause"),
        ("XF86AudioPause", "yggdrasil-mediakey play-pause"),
        ("XF86AudioNext", "yggdrasil-mediakey next"),
        ("XF86AudioPrev", "yggdrasil-mediakey previous"),
        ("XF86AudioStop", "yggdrasil-mediakey stop"),
        ("XF86MonBrightnessUp", "yggdrasil-mediakey brightness-up"),
        ("XF86MonBrightnessDown", "yggdrasil-mediakey brightness-down"),
        // a modifier on its own: Super is the start menu, either Ctrl a tap of Find my mouse
        ("super", "yggdrasil-panelctl start"),
        ("ctrl", "yggdrasil-panelctl cursor-tap"),
        ("rctrl", "yggdrasil-panelctl cursor-tap"),
    ] {
        assert_eq!(runs(&comp, &log, keys), called, "{keys}");
    }
    // not a tap: another key between the press and the release
    runs_nothing(&comp, &log, "super+ctrl");
    runs_nothing(&comp, &log, "ctrl+a");
    runs_nothing(&comp, &log, "super+x");
}

#[test]
fn taps_and_untaken_keys_still_reach_the_program() {
    let _l = limit(60, "taps reach the program");
    let (comp, log) = with_stand_ins(&["--size", "800x600"]);
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    let w = c.window("test.w", "W");
    c.draw(&w, (300, 200), BLUE);
    c.until("the keyboard", |s| s.kb_focus.is_some());
    assert_eq!(runs(&comp, &log, "super"), "yggdrasil-panelctl start");
    assert_eq!(runs(&comp, &log, "ctrl"), "yggdrasil-panelctl cursor-tap");
    comp.must("key ctrl+c");
    c.roundtrip();
    let keys: Vec<u32> = c.state.keys.iter().map(|(_, k)| *k).collect();
    // Super and Ctrl on their own (their binds do not take the key: Hyprland's non_consuming), then Ctrl+C for the program
    assert_eq!(keys, [125, 29, 29, 46], "{keys:?}");
    // a shortcut takes its key: Super+N's N never reaches the program
    let before = c.state.keys.len();
    assert_eq!(runs(&comp, &log, "super+n"), "yggdrasil-panelctl notifications");
    c.roundtrip();
    let keys: Vec<u32> = c.state.keys[before..].iter().map(|(_, k)| *k).collect();
    assert_eq!(keys, [125]);
}

#[test]
fn held_volume_and_magnifier_keys_repeat() {
    let _l = limit(60, "repeat");
    let (comp, log) = with_stand_ins(&["--size", "800x600"]);
    let count = |what: &str| calls(&log).iter().filter(|l| *l == what).count();
    let hold = |keys: &str, ms: u64| {
        comp.must(&format!("key-down {keys}"));
        std::thread::sleep(Duration::from_millis(ms));
        comp.must(&format!("key-up {keys}"));
        std::thread::sleep(Duration::from_millis(300));
    };
    // 1.5 s: once at once, then after 600 ms 25 times a second (about 23 in all)
    hold("XF86AudioRaiseVolume", 1500);
    let n = count("yggdrasil-mediakey volume-up");
    assert!((12..=30).contains(&n), "volume up ran {n} times");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(count("yggdrasil-mediakey volume-up"), n, "and stopped when let go");
    // a short press does not repeat; a key that does not repeat (mute) runs once however long it is held
    hold("XF86AudioLowerVolume", 100);
    assert_eq!(count("yggdrasil-mediakey volume-down"), 1);
    hold("XF86AudioMute", 1200);
    assert_eq!(count("yggdrasil-mediakey mute"), 1);
    // the magnifier's zoom in repeats too; another key pressed meanwhile stops it
    comp.must("key-down super+alt+equal");
    std::thread::sleep(Duration::from_millis(1000));
    comp.must("key a");
    let n = count("yggdrasil-zoom in");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(count("yggdrasil-zoom in"), n, "a key pressed meanwhile ends the repeat");
    comp.must("key-up super+alt+equal");
    assert!(n >= 5, "zoom in ran {n} times");
}

#[test]
fn the_window_keys_close_minimize_and_show_the_minimized_windows() {
    let _l = limit(60, "window keys");
    let (comp, _log) = with_stand_ins(&["--size", "1280x720"]);
    let mut c = comp.connect();
    let a = c.window("test.a", "A");
    c.draw(&a, (300, 200), BLUE);
    let b = c.window("test.b", "B");
    c.draw(&b, (300, 200), YELLOW);
    // Super+W, Alt+F4 and Super+Q ask the window in front to close (the session's yggdrasil-close)
    for keys in ["super+w", "alt+F4", "super+q"] {
        c.state.wins[b.index].close_asked = false;
        comp.must(&format!("key {keys}"));
        c.until(keys, |s| s.wins[b.index].close_asked);
        assert!(!c.state.wins[a.index].close_asked);
    }
    // a program that keeps running (keep-running) is minimized instead
    std::fs::write(comp.dir.join("home/.config/yggdrasil/keep-running"), "test.b\n").unwrap();
    c.state.wins[b.index].close_asked = false;
    comp.must("key super+q");
    comp.wait(10, "B minimized", |c| c.window("test.b").is_some_and(|w| w.has("minimized")));
    c.roundtrip();
    assert!(!c.state.wins[b.index].close_asked, "it keeps running");
    // Super+Down minimizes at once, maximized or not (the session's yggdrasil-snap down)
    comp.must("key super+Up");
    // (a program answers the new size with a picture at it)
    c.roundtrip();
    c.draw(&a, (300, 200), BLUE);
    comp.wait(10, "A maximized", |c| c.window("test.a").is_some_and(|w| w.has("maximized")));
    comp.must("key super+Down");
    comp.wait(10, "A minimized", |c| c.window("test.a").is_some_and(|w| w.has("minimized")));
    assert_eq!(comp.focus(), "none");
    // (back, and at its own size again, for the next part: a maximized window would hide the other one)
    let id_of = |app: &str| {
        comp.must("clients").iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap().split('\t').next().unwrap().to_string()
    };
    comp.must(&format!("activate {}", id_of("test.a")));
    comp.must("key super+Up");
    let deadline = Instant::now() + Duration::from_secs(10);
    while comp.window("test.a").is_some_and(|w| w.has("maximized")) {
        assert!(Instant::now() < deadline, "A back from maximized");
        c.roundtrip();
        c.draw(&a, (300, 200), BLUE);
        std::thread::sleep(Duration::from_millis(20));
    }
    comp.must(&format!("place {} 20 20 300 200", id_of("test.a")));
    c.roundtrip();
    c.draw(&a, (300, 200), BLUE);
    comp.wait(10, "A in the corner", |c| c.window("test.a").is_some_and(|w| w.rect() == (20, 20, 300, 200)));
    comp.must("key super+Down");
    comp.wait(10, "A minimized", |c| c.window("test.a").is_some_and(|w| w.has("minimized")));

    // Super+Shift+M: both shown over the desktop, still minimized; again: out of sight
    comp.must("key super+shift+m");
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.count((0, 0, 1280, 720), (0, 0, 255)) > 0 && pic.count((0, 0, 1280, 720), (255, 255, 0)) > 0, "both shown");
    assert!(comp.windows().iter().all(|w| w.has("minimized")), "{:?}", comp.windows());
    assert!(comp.focus().starts_with("window\t"), "one of them has the keyboard: {}", comp.focus());
    comp.must("key super+shift+m");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 1280, 720), (0, 0, 255)) + pic.count((0, 0, 1280, 720), (255, 255, 0)), 0, "both gone");
    // shown again, one brought forward (the dock, activate) stays when the rest go
    comp.must("key super+shift+m");
    comp.must(&format!("activate {}", id_of("test.a")));
    assert!(!comp.window("test.a").unwrap().has("minimized"));
    comp.must("key super+shift+m");
    assert!(!comp.window("test.a").unwrap().has("minimized") && comp.window("test.b").unwrap().has("minimized"));
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.count((0, 0, 1280, 720), (0, 0, 255)) > 0 && pic.count((0, 0, 1280, 720), (255, 255, 0)) == 0);
}

#[test]
fn the_bindings_are_listed_with_the_sessions_words() {
    let _l = limit(30, "the bindings' list");
    let comp = Compositor::start(&["--size", "800x600"]);
    let lines = comp.must("binds");
    assert!(lines.contains(&"64\tSuper_L\tStart menu\trelease".to_string()), "{lines:?}");
    assert!(lines.contains(&"0\tXF86AudioRaiseVolume\t\trepeat,locked".to_string()));
    assert!(lines.contains(&"72\tequal\tMagnifier: zoom in around the pointer\trepeat".to_string()));
    assert!(lines.contains(&"64\tmouse:272\tMove the window\t-".to_string()));
    // the lists Settings and the panel's card make from them: the same as from the session's own file
    let session = include_str!("../../session/hyprland/yggdrasil.lua");
    let theirs = yggdrasil_conf::shortcuts::list(&yggdrasil_conf::shortcuts::lua_binds(session));
    let run = comp.dir.join("run").into_os_string();
    let display: std::ffi::OsString = comp.wayland.file_name().unwrap().into();
    let desk = yggdrasil_wm::Desk::from_env(|k| match k {
        "XDG_RUNTIME_DIR" => Some(run.clone()),
        "WAYLAND_DISPLAY" => Some(display.clone()),
        _ => None,
    })
    .expect("the compositor is found");
    let binds = desk.binds().expect("the bindings");
    assert!(binds.iter().any(|b| b.key == "Super_L" && b.release));
    let key = |s: &yggdrasil_conf::shortcuts::Shortcut| (s.keys.clone(), s.what.clone(), s.group);
    let mut ours: Vec<_> = yggdrasil_conf::shortcuts::list(&binds).iter().map(key).collect();
    let mut theirs: Vec<_> = theirs.iter().map(key).collect();
    ours.sort();
    theirs.sort();
    assert_eq!(ours, theirs);
    assert_eq!(ours.len(), 56);
}
