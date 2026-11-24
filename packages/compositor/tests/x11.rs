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
    let comp = Compositor::start_env(&["--size", "640x480"], &[("YGGDRASIL_COMPOSITOR_XWAYLAND", "1")], |_| {});
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
