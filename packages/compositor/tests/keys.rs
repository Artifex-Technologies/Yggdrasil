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
fn the_shortcuts_come_before_a_panel_that_has_the_keyboard() {
    let _l = limit(60, "shortcuts over a panel");
    let (comp, log) = with_stand_ins(&["--size", "1280x720"]);
    let mut c = comp.connect();
    let _input = c.input();
    let a = c.window("test.a", "A");
    c.draw(&a, (300, 200), BLUE);
    let b = c.window("test.b", "B");
    c.draw(&b, (300, 200), YELLOW);
    comp.wait(10, "B has the keyboard", |c| c.focus() == "window\ttest.b\tB");

    // a panel that takes the keyboard exclusively (the start menu): the keys that are no shortcut go to it...
    let menu = c.layer_with(0, Layer::Overlay, "test-menu", Anchor::Top | Anchor::Left, (300, 400), 0, KeyboardInteractivity::Exclusive);
    c.draw_layer(&menu, YELLOW);
    comp.wait(10, "the menu has the keyboard", |c| c.focus() == "layer\ttest-menu\tHEADLESS-1");
    // ...but the compositor's own shortcuts work, on the window that was active under it
    assert_eq!(runs(&comp, &log, "super+Return"), "ghostty");
    comp.must("key super+q");
    c.until("Super+Q closes B under the menu", |s| s.wins[b.index].close_asked);
    assert!(!c.state.wins[a.index].close_asked);
    comp.must("key super+Up");
    // (xdg_toplevel's maximized state is 1)
    c.until("Super+Up maximizes B", |s| s.wins[b.index].states.contains(&1));
    // Alt+Tab switches, and the window it picks gets the keyboard
    comp.must("key alt+Tab");
    comp.wait(10, "A picked", |c| c.focus() == "window\ttest.a\tA");

    // an on-demand panel that was clicked (a pop-up): the same
    menu.layer.destroy();
    menu.surface.destroy();
    let pop = c.layer_with(0, Layer::Overlay, "test-pop", Anchor::Top | Anchor::Right, (300, 200), 0, KeyboardInteractivity::OnDemand);
    c.draw_layer(&pop, YELLOW);
    comp.must("pointer 1200 20");
    comp.must("click");
    comp.wait(10, "the pop-up has the keyboard", |c| c.focus() == "layer\ttest-pop\tHEADLESS-1");
    c.state.wins[a.index].close_asked = false;
    comp.must("key super+q");
    c.until("Super+Q closes A under the pop-up", |s| s.wins[a.index].close_asked);
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
    assert_eq!(ours.len(), 61);
}

// ------------------------------------------------------------------ the window switcher (Alt+Tab)

const RED: u32 = 0xffff0000;
const GREEN: u32 = 0xff00ff00;
const MAGENTA: u32 = 0xffff00ff;
const CYAN: u32 = 0xff00ffff;

/// A compositor for the switcher's tests: no fonts (fontconfig finds none, so no text is drawn and the pictures are the same on
/// every machine), Yggdrasil's purple chosen as the accent, and an icon for the app `test.b` (a red square in a ring).
fn switcher_compositor(size: &str) -> Compositor {
    let fonts = std::env::temp_dir().join(format!("ygg-keys-nofonts-{}.conf", std::process::id()));
    std::fs::write(&fonts, "<?xml version=\"1.0\"?><fontconfig></fontconfig>\n").unwrap();
    Compositor::start_env(&["--size", size], &[("FONTCONFIG_FILE", fonts.to_str().unwrap())], |dir| {
        std::fs::write(dir.join("home/.config/yggdrasil/appearance.conf"), "style = dark\naccent = 124,84,214\n").unwrap();
        let icons = dir.join("home/.local/share/icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(
            icons.join("test.b.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><circle cx="12" cy="12" r="11" fill="white"/><rect x="6" y="6" width="12" height="12" fill="red"/></svg>"#,
        )
        .unwrap();
    })
}

/// A card of the switcher as the control socket lists it: chosen, the app ids of the card's windows, its place and size.
type CardLine = (bool, String, Option<(i32, i32, i32, i32)>);

fn cards(comp: &Compositor) -> Vec<CardLine> {
    comp.must("switcher")
        .iter()
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            let place = (f.len() >= 4 && !f[2].is_empty()).then(|| {
                let (x, y) = pair(f[2], ',');
                let (w, h) = pair(f[3], 'x');
                (x, y, w, h)
            });
            (f[0] == "*", f[1].to_string(), place)
        })
        .collect()
}

/// The app ids in the switcher's order, and the chosen card's.
fn order(comp: &Compositor) -> (Vec<String>, String) {
    let c = cards(comp);
    let chosen = c.iter().find(|(on, _, _)| *on).map(|(_, a, _)| a.clone()).unwrap_or_default();
    (c.into_iter().map(|(_, a, _)| a).collect(), chosen)
}

fn focused_app(comp: &Compositor) -> String {
    comp.focus().split('\t').nth(1).unwrap_or("").to_string()
}

fn id_of(comp: &Compositor, app: &str) -> String {
    comp.must("clients").iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap().split('\t').next().unwrap().to_string()
}

fn frames_drawn(comp: &Compositor) -> u64 {
    comp.must("frames")[0].strip_prefix("drawn ").unwrap().parse().unwrap()
}

#[test]
fn alt_tab_goes_through_the_windows_most_recent_first() {
    let _l = limit(90, "Alt+Tab");
    let comp = switcher_compositor("1280x720");
    let mut c = comp.connect();
    let _input = c.input();
    let mut wins = vec![];
    for (app, colour) in [("test.a", BLUE), ("test.b", YELLOW), ("test.c", RED), ("test.d", GREEN)] {
        let w = c.window(app, app);
        c.draw(&w, (400, 300), colour);
        wins.push(w);
    }
    comp.wait(10, "D has the keyboard", |c| focused_app(c) == "test.d");
    let apps = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    // Alt+Tab: the windows used last first, the one before the window in front chosen
    comp.must("key-down alt");
    comp.must("key Tab");
    assert_eq!(order(&comp), (apps(&["test.d", "test.c", "test.b", "test.a"]), "test.c".into()));
    // Tab moves on and goes round, Shift+Tab goes back (round the start too)
    comp.must("key Tab");
    assert_eq!(order(&comp).1, "test.b");
    comp.must("key Tab");
    comp.must("key Tab");
    assert_eq!(order(&comp).1, "test.d", "round the end");
    comp.must("key shift+Tab");
    assert_eq!(order(&comp).1, "test.a", "Shift+Tab round the start");
    comp.must("key shift+Tab");
    assert_eq!(order(&comp).1, "test.b");
    // the keys go to the switcher, not to the window
    assert_eq!(focused_app(&comp), "test.d");
    // letting go of Alt switches
    comp.must("key-up alt");
    assert!(cards(&comp).is_empty(), "closed");
    assert_eq!(focused_app(&comp), "test.b");
    assert_eq!(comp.windows().last().unwrap().app_id, "test.b", "raised in front");

    // a quick Alt+Tab goes back to the window used before
    comp.must("key alt+Tab");
    assert_eq!(focused_app(&comp), "test.d");

    // Alt+Shift+Tab opens on the window used longest ago; Escape cancels and nothing changes, also when Alt is let go after
    comp.must("key-down alt");
    comp.must("key shift+Tab");
    assert_eq!(order(&comp), (apps(&["test.d", "test.b", "test.c", "test.a"]), "test.a".into()));
    comp.must("key Escape");
    assert!(cards(&comp).is_empty(), "Escape closes it");
    comp.must("key-up alt");
    assert_eq!(focused_app(&comp), "test.d");

    // the arrows move in the grid, Enter picks
    comp.must("key-down alt");
    comp.must("key Tab");
    comp.must("key Right");
    assert_eq!(order(&comp).1, "test.c");
    comp.must("key Left");
    comp.must("key Left");
    assert_eq!(order(&comp).1, "test.d");
    comp.must("key Left");
    assert_eq!(order(&comp).1, "test.a", "Left round the start");
    comp.must("key Return");
    assert!(cards(&comp).is_empty());
    assert_eq!(focused_app(&comp), "test.a");
    comp.must("key-up alt");
    assert_eq!(focused_app(&comp), "test.a", "Alt let go after Enter switches nothing more");
}

#[test]
fn alt_tab_brings_back_a_minimized_window_and_takes_a_click() {
    let _l = limit(90, "Alt+Tab, minimized and clicked");
    let comp = switcher_compositor("1280x720");
    let mut c = comp.connect();
    let _input = c.input();
    for (app, colour) in [("test.a", BLUE), ("test.b", YELLOW), ("test.c", RED)] {
        let w = c.window(app, app);
        c.draw(&w, (400, 300), colour);
    }
    comp.wait(10, "C has the keyboard", |c| focused_app(c) == "test.c");
    comp.must(&format!("minimize {}", id_of(&comp, "test.a")));
    comp.wait(10, "A minimized", |c| c.window("test.a").is_some_and(|w| w.has("minimized")));

    // a minimized window is a card too; choosing it brings it back with the keyboard
    comp.must("key-down alt");
    comp.must("key Tab");
    let (apps, _) = order(&comp);
    assert!(apps.contains(&"test.a".to_string()), "{apps:?}");
    while order(&comp).1 != "test.a" {
        comp.must("key Tab");
    }
    comp.must("key-up alt");
    comp.wait(10, "A back", |c| c.window("test.a").is_some_and(|w| !w.has("minimized")));
    assert_eq!(focused_app(&comp), "test.a");
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.count((0, 0, 1280, 720), (0, 0, 255)) > 100_000, "A is drawn again");

    // a click on a card switches to it (Alt still held: letting go after switches nothing more)
    comp.must("key-down alt");
    comp.must("key Tab");
    let cs = cards(&comp);
    let (_, _, place) = cs.iter().find(|(_, a, _)| a == "test.b").expect("B's card").clone();
    let (x, y, w, h) = place.expect("B's card is shown");
    comp.must(&format!("pointer {} {}", x + w / 2, y + h / 2));
    comp.must("click");
    assert!(cards(&comp).is_empty(), "the click closed it");
    assert_eq!(focused_app(&comp), "test.b");
    comp.must("key-up alt");
    assert_eq!(focused_app(&comp), "test.b");
    // a click outside the panel closes it and switches nothing
    comp.must("key-down alt");
    comp.must("key Tab");
    comp.must("pointer 2 2");
    comp.must("click");
    assert!(cards(&comp).is_empty());
    comp.must("key-up alt");
    assert_eq!(focused_app(&comp), "test.b");
}

#[test]
fn snapped_windows_are_one_group_card_and_come_forward_together() {
    let _l = limit(90, "group cards");
    let comp = switcher_compositor("1280x720");
    let mut c = comp.connect();
    let _input = c.input();
    let a = c.window("test.a", "Calculator");
    c.draw(&a, (400, 300), BLUE);
    let b = c.window("test.b", "Notes");
    c.draw(&b, (400, 300), YELLOW);
    let cc = c.window("test.c", "Terminal");
    c.draw(&cc, (500, 300), RED);
    let d = c.window("test.d", "Tall");
    c.draw(&d, (220, 420), MAGENTA);
    let e = c.window("test.e", "Wide");
    c.draw(&e, (640, 220), CYAN);
    // A to the left half, B to the right one: a group
    let snap = |c: &mut common::Client, w: &common::Window, app: &str, keys: &str, colour: u32| {
        comp.must(&format!("activate {}", id_of(&comp, app)));
        comp.must(&format!("key {keys}"));
        c.roundtrip();
        c.draw(w, (400, 300), colour);
        let side = if keys.ends_with("Left") { "left" } else { "right" };
        comp.wait(10, side, |comp| comp.window(app).is_some_and(|w| w.has(side)));
    };
    snap(&mut c, &a, "test.a", "super+Left", BLUE);
    snap(&mut c, &b, "test.b", "super+Right", YELLOW);
    comp.must(&format!("activate {}", id_of(&comp, "test.e")));
    comp.wait(10, "E in front", |c| focused_app(c) == "test.e");

    comp.must("key-down alt");
    comp.must("key Tab");
    let (apps, chosen) = order(&comp);
    assert_eq!(apps, ["test.e", "test.b,test.a", "test.d", "test.c"], "the snapped pair is one card, B (used last) first");
    assert_eq!(chosen, "test.b,test.a");
    let cs = cards(&comp);
    let group = cs[1].2.unwrap();
    assert!(group.2 > cs[2].2.unwrap().2, "the group's card is wider than a tall window's: {cs:?}");
    // the live pictures: each window's own colour on its card, the group's two side by side
    let pic = comp.screenshot("HEADLESS-1");
    let inside = |p: (i32, i32, i32, i32)| (p.0 + 8, p.1 + 8 + 28, p.2 - 16, p.3 - 16 - 28);
    let (gx, gy, gw, gh) = inside(group);
    assert!(pic.count((gx, gy, gw / 2, gh), (0, 0, 255)) > 500, "A on the left of the group's card");
    assert!(pic.count((gx + gw / 2, gy, gw / 2, gh), (255, 255, 0)) > 500, "B on the right");
    assert!(pic.count(inside(cs[2].2.unwrap()), (255, 0, 255)) > 500, "D on its card");
    snapshot!(pic, "switcher-grid");

    // choosing the group brings both forward, B with the keyboard
    comp.must("key-up alt");
    comp.wait(10, "B in front", |c| focused_app(c) == "test.b");
    let mut ws: Vec<String> = comp.windows().iter().rev().take(2).map(|w| w.app_id.clone()).collect();
    ws.sort();
    assert_eq!(ws, ["test.a", "test.b"], "the two in front: {:?}", comp.windows());
}

#[test]
fn the_switcher_draws_only_when_something_changes_and_nothing_once_closed() {
    let _l = limit(90, "the switcher's frames");
    let comp = switcher_compositor("1280x720");
    let mut c = comp.connect();
    let _input = c.input();
    let a = c.window("test.a", "A");
    c.draw(&a, (400, 300), BLUE);
    let b = c.window("test.b", "B");
    c.draw(&b, (400, 300), YELLOW);
    comp.wait(10, "B has the keyboard", |c| focused_app(c) == "test.b");
    let settle = || {
        let mut last = frames_drawn(&comp);
        loop {
            std::thread::sleep(Duration::from_millis(150));
            let now = frames_drawn(&comp);
            if now == last {
                return now;
            }
            last = now;
        }
    };
    let idle = settle();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(frames_drawn(&comp), idle, "nothing drawn while nothing changes");

    // open: drawn once, then nothing while the windows stay as they are
    comp.must("key-down alt");
    comp.must("key Tab");
    let open = settle();
    assert!(open > idle, "the switcher was drawn");
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(frames_drawn(&comp), open, "an open switcher over still windows draws nothing more");
    // a window that draws anew shows on its card (live)
    let card = cards(&comp).into_iter().find(|(_, a, _)| a == "test.a").unwrap().2.unwrap();
    let inside = (card.0 + 8, card.1 + 36, card.2 - 16, card.3 - 44);
    assert!(comp.screenshot("HEADLESS-1").count(inside, (0, 0, 255)) > 500);
    c.draw(&a, (400, 300), GREEN);
    comp.wait(10, "A's card green", |comp| comp.screenshot("HEADLESS-1").count(inside, (0, 255, 0)) > 500);
    let after = settle();
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(frames_drawn(&comp), after);

    // closed (Escape, then Alt let go): one frame to take it away, then nothing
    comp.must("key Escape");
    comp.must("key-up alt");
    let closed = settle();
    assert!(closed > after, "drawn once more to take it away");
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(frames_drawn(&comp), closed, "nothing drawn once closed");
}

#[test]
fn a_steam_games_card_shows_its_shortcuts_icon() {
    let _l = limit(60, "a Steam game's card");
    let fonts = std::env::temp_dir().join(format!("ygg-keys-nofonts-steam-{}.conf", std::process::id()));
    std::fs::write(&fonts, "<?xml version=\"1.0\"?><fontconfig></fontconfig>\n").unwrap();
    // the shortcut Steam makes for a game, and the game's icon Steam saves (magenta here)
    let comp = Compositor::start_env(&["--size", "1280x720"], &[("FONTCONFIG_FILE", fonts.to_str().unwrap())], |dir| {
        let apps = dir.join("home/.local/share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(
            apps.join("Fallout 4.desktop"),
            "[Desktop Entry]\nType=Application\nName=Fallout 4\nExec=steam steam://rungameid/377160\nIcon=steam_icon_377160\n",
        )
        .unwrap();
        let icons = dir.join("home/.local/share/icons/hicolor/scalable/apps");
        std::fs::create_dir_all(&icons).unwrap();
        std::fs::write(
            icons.join("steam_icon_377160.svg"),
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"><rect width="24" height="24" fill="magenta"/></svg>"#,
        )
        .unwrap();
    });
    let mut c = comp.connect();
    let _input = c.input();
    // a game under Proton: its window class is steam_app_<number>
    let game = c.window("steam_app_377160", "Fallout4");
    c.draw(&game, (400, 300), BLUE);
    let other = c.window("test.a", "A");
    c.draw(&other, (400, 300), GREEN);
    comp.wait(10, "A has the keyboard", |c| focused_app(c) == "test.a");
    comp.must("key-down alt");
    comp.must("key Tab");
    assert_eq!(order(&comp).0, ["test.a", "steam_app_377160"]);
    let pic = comp.screenshot("HEADLESS-1");
    let all = (0, 0, pic.w as i32, pic.h as i32);
    assert!(pic.count(all, (255, 0, 255)) > 100, "the game's card has the shortcut's icon");
    comp.must("key-up alt");
}
