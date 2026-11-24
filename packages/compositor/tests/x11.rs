//! X11 programs through XWayland on the headless backend: what a window manager must do for them. Skipped where XWayland
//! is not installed.

mod common;

use std::time::{Duration, Instant};

use common::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, CreateWindowAux, EventMask, PropMode, WindowClass};
use x11rb::wrapper::ConnectionExt as _;

fn xwayland_installed() -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join("Xwayland").is_file()))
}

/// The compositor with XWayland, and the X display it printed.
fn with_xwayland() -> (Compositor, String) {
    with_xwayland_on(&["--size", "640x480"], |_| {})
}

/// The same, on the screens of `args`, with `setup` run on the test's folder first.
fn with_xwayland_on(args: &[&str], setup: impl FnOnce(&std::path::Path)) -> (Compositor, String) {
    let comp = Compositor::start_env(args, &[("YGGDRASIL_COMPOSITOR_XWAYLAND", "1")], setup);
    let start = Instant::now();
    loop {
        let found = comp.log.lock().unwrap().iter().find_map(|l| l.strip_prefix("DISPLAY=").map(str::to_string));
        if let Some(d) = found {
            return (comp, d);
        }
        assert!(start.elapsed() < Duration::from_secs(20), "XWayland did not start: {}", comp.log_tail(20));
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn a_window_that_waits_to_be_told_it_has_the_focus_is_made_active_and_given_it() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 window's focus");
    let (comp, display) = with_xwayland();
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).unwrap().reply().unwrap().atom;
    let (protocols, take_focus, delete, active) =
        (atom("WM_PROTOCOLS"), atom("WM_TAKE_FOCUS"), atom("WM_DELETE_WINDOW"), atom("_NET_ACTIVE_WINDOW"));

    // a window as Wine makes them: "globally active" (its input hint False, WM_TAKE_FOCUS among its protocols), and it
    // never sets the focus itself: it waits to be told it is the active window
    let win = conn.generate_id().unwrap();
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win,
        root,
        0,
        0,
        300,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new().background_pixel(0x0000ff).event_mask(EventMask::FOCUS_CHANGE),
    )
    .unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"game\0test.wine\0").unwrap();
    conn.change_property32(PropMode::REPLACE, win, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, &[1, 0, 1, 0, 0, 0, 0, 0, 0]).unwrap();
    conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete, take_focus]).unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();

    comp.wait(10, "the window listed", |c| c.window("test.wine").is_some());
    comp.wait(10, "the window has the keyboard", |c| c.focus().contains("test.wine"));
    let start = Instant::now();
    let (mut is_active, mut has_focus) = (false, false);
    while start.elapsed() < Duration::from_secs(5) && !(is_active && has_focus) {
        let a = conn.get_property(false, root, active, AtomEnum::WINDOW, 0, 1).unwrap().reply().unwrap();
        is_active = a.value32().and_then(|mut v| v.next()) == Some(win);
        has_focus = conn.get_input_focus().unwrap().reply().unwrap().focus == win;
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(is_active, "_NET_ACTIVE_WINDOW names the window");
    assert!(has_focus, "the window has the X input focus");
}

/// X11 programs show the desktop's pointer (YGG-10): the compositor sets Xcursor.theme and Xcursor.size in XWayland's X
/// resources (libXcursor reads them), with Xft.dpi beside them, and sets them again when the pointer's theme changes.
#[test]
fn x11_programs_are_told_the_pointers_theme_and_size() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "the X11 pointer theme");
    let (comp, display) = with_xwayland();
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let resources = || {
        let r = conn.get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16).unwrap().reply().unwrap();
        String::from_utf8_lossy(&r.value).into_owned()
    };
    let wait_for = |want: &[&str]| {
        let start = Instant::now();
        loop {
            let r = resources();
            if want.iter().all(|w| r.lines().any(|l| l == *w)) {
                return r;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "waited for {want:?}; the resources are {r:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // at the start: some theme, a size, and the DPI of scale 1
    let r = wait_for(&["Xft.dpi:\t96"]);
    assert!(r.lines().any(|l| l.starts_with("Xcursor.theme:\t")) && r.lines().any(|l| l.starts_with("Xcursor.size:\t")), "{r:?}");
    // ygg-theme sets another on the running compositor: X11 programs are told, each name once, Xft.dpi kept
    comp.must("cursor-theme Adwaita 32");
    let r = wait_for(&["Xcursor.theme:\tAdwaita", "Xcursor.size:\t32"]);
    assert_eq!(r.lines().filter(|l| l.starts_with("Xcursor.theme:")).count(), 1, "{r:?}");
    assert!(r.lines().any(|l| l == "Xft.dpi:\t96"), "{r:?}");
}

/// The X screen's size and the Xft.dpi X11 programs are told, on a 1920x1200 screen at 125 %.
fn x11_size_and_dpi(stretch: bool) -> ((u16, u16), String) {
    let (_comp, display) = with_xwayland_on(&["--output", "1920x1200@1.25"], |dir| {
        if stretch {
            std::fs::write(dir.join("home/.config/yggdrasil/xwayland.conf"), "stretch = true\n").unwrap();
        }
    });
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = &conn.setup().roots[screen];
    let size = (root.width_in_pixels, root.height_in_pixels);
    let start = Instant::now();
    loop {
        let r = conn.get_property(false, root.root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 16).unwrap().reply().unwrap();
        let text = String::from_utf8_lossy(&r.value).into_owned();
        if let Some(dpi) = text.lines().find_map(|l| l.strip_prefix("Xft.dpi:\t")) {
            return (size, dpi.to_string());
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no Xft.dpi in {text:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn x11_programs_draw_at_the_screens_resolution_unless_stretched() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(90, "X11 at the screen's resolution");
    // XWayland is told the screen in pixels and the DPI of 125 %: a game is offered 1920x1200, a GTK program keeps its size
    assert_eq!(x11_size_and_dpi(false), ((1920, 1200), "120".to_string()));
    // stretch = true in xwayland.conf: the old way, the scaled-down size stretched to the screen, the DPI of scale 1
    assert_eq!(x11_size_and_dpi(true), ((1536, 960), "96".to_string()));
}

#[test]
fn an_x11_windows_picture_is_copied_for_the_docks_preview() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 window's copy");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    // a red window: the X server paints its background, XWayland hands the picture over
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0xff0000).event_mask(EventMask::EXPOSURE);
    conn.create_window(0, win, root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"x11copy\0X11copy\0").unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_NAME, AtomEnum::STRING, b"Copied").unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    let start = Instant::now();
    while comp.window("X11copy").is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "the X11 window opens: {:?}", comp.must("windows"));
        std::thread::sleep(Duration::from_millis(50));
    }
    let on_screen = comp.window("X11copy").unwrap();
    let shot = comp.screenshot("HEADLESS-1");
    assert!(shot.count((on_screen.at.0 + 20, on_screen.at.1 + 20, 100, 100), (255, 0, 0)) > 9000, "red on the screen: {on_screen:?}");

    // the dock's way: the window list, then a copy of the window alone
    let mut p = comp.connect();
    let _list = p.toplevel_list();
    let listed = p.listed("X11copy").expect("the X11 window is listed");
    assert_eq!(listed.title, "Copied");
    let (i, session) = p.capture_window(&listed.handle);
    let size = p.state.caps[i].size.expect("a size for the copy");
    assert_eq!(size, (300, 200), "the window's own size");
    let f = p.begin_copy(i, &session, size, 0);
    let (pic, bytes) = p.end_copy(f, 5.0).expect("the X11 window's copy");
    assert_eq!(pic.count((0, 0, 300, 200), (255, 0, 0)), 300 * 200, "all of it red, as on the screen");
    let clear = bytes.chunks(4).filter(|px| px[3] != 255).count();
    assert_eq!(
        clear,
        0,
        "opaque, as the panel draws it (alpha of the first pixels: {:?})",
        &bytes.chunks(4).take(4).map(|p| p[3]).collect::<Vec<_>>()
    );
}
