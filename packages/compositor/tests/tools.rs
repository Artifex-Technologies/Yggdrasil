//! Yggdrasil's own tools on this compositor through yggdrasil-wm (packages/wm), the layer they share with the Hyprland session:
//! it finds this compositor by its control socket and reads and acts on it as the panel (desktops, zones), yggdrasil-snap,
//! yggdrasil-close and yggdrasil-idle (the pointer) do. The programs themselves are run in tests/apps.rs.
//!
//! Every test starts a compositor of its own in a private folder and has a time limit; the layer is given that folder and the
//! compositor's Wayland socket, never the running desktop's.

mod common;

use std::ffi::OsString;
use std::time::{Duration, Instant};

use common::*;
use yggdrasil_wm::{Desk, Desktops, MonitorRef};

const BLUE: u32 = 0xff0000ff;
const GREEN: u32 = 0xff00ff00;
const RED: u32 = 0xffff0000;

// xdg_toplevel states
const MAXIMIZED: u32 = 1;

/// The layer as a program inside this compositor finds it. A Hyprland signature is given too, of a Hyprland that is not
/// there: the compositor's own socket is found first.
fn desk(comp: &Compositor) -> Desk {
    let run = comp.dir.join("run").into_os_string();
    let display: OsString = comp.wayland.file_name().unwrap().into();
    let d = Desk::from_env(|k| match k {
        "XDG_RUNTIME_DIR" => Some(run.clone()),
        "WAYLAND_DISPLAY" => Some(display.clone()),
        "HYPRLAND_INSTANCE_SIGNATURE" => Some("no-hyprland-here".into()),
        _ => None,
    })
    .expect("the compositor is found");
    assert!(matches!(d, Desk::Own(_)), "{d:?}");
    d
}

/// A bar (32, reserved) and a dock (64, reserved) as the panel makes them.
fn bar_and_dock(c: &mut Client) -> (LayerSurf, LayerSurf) {
    let bar = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&bar, RED);
    let dock = c.layer_with(0, Layer::Top, "test-dock", Anchor::Bottom, (600, 64), 64, KeyboardInteractivity::None);
    c.draw_layer(&dock, GREEN);
    (bar, dock)
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
        assert!(Instant::now() < deadline, "{app_id} did not settle at {given:?}: {info:?}");
        c.draw(w, given, argb);
    }
}

#[test]
fn the_tools_read_the_windows_screens_and_desktops() {
    let _l = limit(60, "reading through the layer");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _layers = bar_and_dock(&mut c);
    let a = c.window("test.a", "A window");
    c.draw(&a, (400, 300), BLUE);
    let b = c.window("test.b", "B window");
    c.draw(&b, (500, 400), BLUE);
    let d = desk(&comp);

    let wins = d.windows();
    assert_eq!(
        wins.iter().map(|w| (w.class.as_str(), w.title.as_str())).collect::<Vec<_>>(),
        [("test.a", "A window"), ("test.b", "B window")]
    );
    let info = comp.window("test.a").unwrap();
    assert_eq!(wins[0].at, Some((info.at.0 as f64, info.at.1 as f64)));
    assert_eq!(wins[0].size, Some((400.0, 300.0)));
    assert_eq!((wins[0].workspace, wins[0].floating, wins[0].minimized), (Some(1), Some(true), false));
    assert_eq!(wins[0].monitor, Some(MonitorRef::Name("HEADLESS-1".into())));
    assert_ne!(wins[0].id, wins[1].id);
    assert_eq!(d.active_window(true).map(|w| w.class), Some("test.b".into()), "the newest window has the keyboard");
    assert_eq!(d.find("test.a", "renamed").map(|w| w.id), Some(wins[0].id.clone()), "the only window of its app");

    let screens = d.screens();
    assert_eq!(screens.len(), 1);
    let s = &screens[0];
    assert_eq!(
        (s.name.as_str(), s.x, s.y, s.width, s.height, s.scale, s.reserved),
        ("HEADLESS-1", 0, 0, 1600, 900, Some(1.0), [0, 32, 0, 64])
    );
    assert!(s.focused && s.is(&MonitorRef::Name("HEADLESS-1".into())));
    let free: Vec<String> = comp.must("free");
    assert_eq!(free, ["HEADLESS-1\t0,32\t1600x804"]);
    assert_eq!(s.monitor().free_area(), (0, 32, 1600, 804), "the zones' free room is the compositor's");
    assert_eq!(d.focused_screen().map(|s| s.name), Some("HEADLESS-1".into()));

    comp.must("pointer 100 200");
    assert_eq!(d.cursor(), Some((100.0, 200.0)));
    assert_eq!(d.super_held(), Some(false));

    // the desktops: the one shown, and those holding a window
    assert_eq!(d.desktops(), Desktops { ids: vec![1], at: 0 });
    assert_eq!(d.max_desktops(), Some(4));
    d.show_desktop("2");
    assert_eq!(comp.must("workspace"), ["2"]);
    assert_eq!(d.desktops(), Desktops { ids: vec![1, 2], at: 1 }, "an empty desktop exists while it is shown");
    d.show_desktop("1");
    assert_eq!(comp.must("workspace"), ["1"]);
    d.send_to_desktop(&wins[0].id, "3");
    comp.wait(10, "A on desktop 3", |c| c.window("test.a").is_some_and(|w| w.workspace == 3));
    assert_eq!(d.desktops(), Desktops { ids: vec![1, 3], at: 0 });
    d.show_desktop("5");
    d.show_desktop("special:minimized");
    assert_eq!(comp.must("workspace"), ["1"], "there are four desktops; nothing else is shown");

    // the dock's own minimize and restore are wlr-foreign-toplevel's here: the layer leaves them to it
    assert!(!d.minimize_app("test.b", "B window") && !d.restore_app("test.b", "B window"));
    assert!(!d.focus_direction('l'), "no tiling screens of Hyprland's here");
    assert!(comp.window("test.b").is_some_and(|w| !w.has("minimized")));
}

#[test]
fn the_tools_place_maximize_minimize_and_close_windows() {
    let _l = limit(60, "acting through the layer");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _layers = bar_and_dock(&mut c);
    let a = c.window("test.a", "A");
    c.draw(&a, (400, 300), BLUE);
    let d = desk(&comp);
    let id = d.active_window(false).unwrap().id;

    // a zone (the panel's window zones), and a half (yggdrasil-snap)
    let answers = d.place(&id, true, (8, 40, 784, 788));
    assert_eq!(answers.iter().map(|(_, a)| a.as_deref()).collect::<Vec<_>>(), [Some("ok")]);
    assert_eq!(settle(&comp, &mut c, &a, "test.a", BLUE).rect(), (8, 40, 784, 788));
    assert!(d.snap_active((800, 32, 800, 804)));
    assert_eq!(settle(&comp, &mut c, &a, "test.a", BLUE).rect(), (800, 32, 800, 804));
    // a place far outside every screen is refused
    assert_eq!(d.place(&id, true, (0, 0, 0, 10))[0].1.as_deref().map(|a| a.contains("size")), Some(true));
    assert!(d.place("not-a-number", true, (0, 0, 10, 10)).is_empty());

    // Super+Up twice (yggdrasil-snap up): maximized to the free room, then back
    assert!(d.maximize_active());
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(info.has("maximized") && c.state.wins[a.index].states.contains(&MAXIMIZED), "{info:?}");
    assert_eq!(info.rect(), (0, 32, 1600, 804));
    assert!(d.maximize_active());
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(!info.has("maximized"), "{info:?}");
    // a maximized window placed in a zone is a plain window there
    assert!(d.maximize_active());
    settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(d.snap_active((0, 32, 800, 804)));
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(!info.has("maximized") && info.rect() == (0, 32, 800, 804), "{info:?}");

    // Super+Down (yggdrasil-snap down) and the keep-running programs' close: minimized
    assert!(d.minimize_active());
    comp.wait(10, "A minimized", |c| c.window("test.a").is_some_and(|w| w.has("minimized")));
    assert!(d.windows()[0].minimized);
    assert_eq!(d.active_window(false), None, "a minimized window does not have the keyboard");
    assert!(!d.close_active(), "nothing in front to close");

    // yggdrasil-close: the window is asked to close (the program may still ask to save first)
    let b = c.window("test.b", "B");
    c.draw(&b, (300, 200), BLUE);
    assert!(d.close_active());
    c.until("B asked to close", |s| s.wins[b.index].close_asked);
    assert!(!c.state.wins[a.index].close_asked);

    // yggdrasil-idle hides the pointer while the screen is dimmed, and puts it back
    assert_eq!(d.pointer_timeout(), Some(0.0));
    d.set_pointer_timeout(0.25);
    assert_eq!(d.pointer_timeout(), Some(0.25));
    assert_eq!(comp.must("pointer-timeout"), ["0.25"]);
    d.set_pointer_timeout(0.0);
    assert_eq!(d.pointer_timeout(), Some(0.0));
    assert!(comp.ctl("pointer-timeout -1").is_err() && comp.ctl("pointer-timeout nan").is_err());
}

#[test]
fn a_tiled_window_is_floated_where_it_is_put() {
    let _l = limit(60, "placing a tiled window");
    let comp = Compositor::start_with(&["--size", "1600x900"], |dir| {
        let conf = dir.join("home/.config/yggdrasil");
        std::fs::create_dir_all(&conf).unwrap();
        std::fs::write(conf.join("workspaces.conf"), "default = tiling\n").unwrap();
    });
    let mut c = comp.connect();
    let a = c.window("test.a", "A");
    c.draw(&a, (400, 300), BLUE);
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(info.has("tiled"), "the workspace tiles: {info:?}");
    let d = desk(&comp);
    let w = d.active_window(false).unwrap();
    assert_eq!(w.floating, Some(false));
    d.place(&w.id, false, (100, 100, 600, 400));
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(info.has("floating") && info.rect() == (100, 100, 600, 400), "{info:?}");
}

#[test]
fn a_window_title_cannot_add_lines_to_the_answers() {
    let _l = limit(30, "titles");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let a = c.window("test.a", "two\nlines\tand a tab");
    c.draw(&a, (200, 100), BLUE);
    let d = desk(&comp);
    let wins = d.windows();
    assert_eq!(wins.len(), 1);
    assert_eq!(wins[0].title, "two lines and a tab");
    assert_eq!(comp.windows().len(), 1);
}
