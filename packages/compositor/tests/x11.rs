//! X11 programs through XWayland on the headless backend: what a window manager must do for them. Skipped where XWayland
//! is not installed.

mod common;

use std::time::{Duration, Instant};

use common::*;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConfigureWindowAux, ConnectionExt, CreateWindowAux, EventMask, PropMode, WindowClass};
use x11rb::wrapper::ConnectionExt as _;

/// The compositor with XWayland (tests/common), and the X display it printed.
fn with_xwayland() -> (Compositor, String) {
    with_xwayland_on(&["--size", "640x480"], |_| {})
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

/// What an X11 window asks for before it is shown, which the window manager reads when it is mapped (EWMH, ICCCM): full screen
/// or maximized (`_NET_WM_STATE`: games, programs that restore a maximized window) from its first frame, and back to the place
/// it would have had when that ends; Steam's maximize is ignored, as its requests are, and it may still size itself; a window
/// that asks to start minimized (WM_HINTS) is shown, and told so.
#[test]
fn an_x11_window_opens_as_it_asked_before_it_was_shown() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    use x11rb::protocol::xproto::{ClientMessageEvent, ConfigureWindowAux};
    let _l = limit(60, "what an X11 window asked for before it was shown");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let atom = |n: &[u8]| conn.intern_atom(false, n).unwrap().reply().unwrap().atom;
    let (net_state, full, vert, horz, hidden, wm_state) = (
        atom(b"_NET_WM_STATE"),
        atom(b"_NET_WM_STATE_FULLSCREEN"),
        atom(b"_NET_WM_STATE_MAXIMIZED_VERT"),
        atom(b"_NET_WM_STATE_MAXIMIZED_HORZ"),
        atom(b"_NET_WM_STATE_HIDDEN"),
        atom(b"WM_STATE"),
    );
    // a 300x200 window of this class with these states set before it is mapped, and WM_HINTS' initial state Iconic if asked
    let open = |class: &[u8], states: &[u32], iconic: bool| {
        let win = conn.generate_id().unwrap();
        conn.create_window(0, win, root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, class).unwrap();
        conn.change_property32(PropMode::REPLACE, win, net_state, AtomEnum::ATOM, states).unwrap();
        if iconic {
            // flags StateHint, initial_state IconicState
            conn.change_property32(PropMode::REPLACE, win, AtomEnum::WM_HINTS, AtomEnum::WM_HINTS, &[2, 0, 3, 0, 0, 0, 0, 0, 0]).unwrap();
        }
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        win
    };
    // what the program is told: its _NET_WM_STATE, and its WM_STATE
    let told = |win: u32| {
        let s = conn.get_property(false, win, net_state, AtomEnum::ATOM, 0, 32).unwrap().reply().unwrap();
        let w = conn.get_property(false, win, wm_state, wm_state, 0, 2).unwrap().reply().unwrap();
        (s.value32().map(|v| v.collect::<Vec<u32>>()).unwrap_or_default(), w.value32().and_then(|mut v| v.next()))
    };
    // the program ends a state as programs do: a _NET_WM_STATE remove message to the root
    let end = |win: u32, a: u32, b: u32| {
        let ev = ClientMessageEvent::new(32, win, net_state, [0, a, b, 1, 0]);
        conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, ev).unwrap();
        conn.flush().unwrap();
    };

    let f = open(b"full\0Full\0", &[full], false);
    comp.wait(10, "the window full screen", |c| c.window("Full").is_some_and(|w| w.has("fullscreen") && w.rect() == (0, 0, 800, 600)));
    end(f, full, 0);
    comp.wait(10, "the window back at its own size", |c| c.window("Full").is_some_and(|w| !w.has("fullscreen") && w.size == (300, 200)));

    // (a little short of the screen: the session's gaps)
    let m = open(b"max\0Max\0", &[vert, horz], false);
    comp.wait(10, "the window maximized", |c| c.window("Max").is_some_and(|w| w.has("maximized") && w.size.0 > 700 && w.size.1 > 500));
    end(m, vert, horz);
    comp.wait(10, "the window back at its own size", |c| c.window("Max").is_some_and(|w| !w.has("maximized") && w.size == (300, 200)));

    let s = open(b"steam\0steam\0", &[vert, horz], false);
    comp.wait(10, "Steam's window", |c| c.window("steam").is_some_and(|w| !w.has("maximized")));
    let (states, _) = told(s);
    assert!(!states.contains(&vert) && !states.contains(&horz), "Steam is told it is not maximized: {states:?}");
    conn.configure_window(s, &ConfigureWindowAux::new().width(400).height(300)).unwrap();
    conn.flush().unwrap();
    comp.wait(10, "Steam's window at the size it asked for", |c| c.window("steam").is_some_and(|w| w.size == (400, 300)));

    let i = open(b"iconic\0Iconic\0", &[], true);
    comp.wait(10, "the window shown", |c| c.window("Iconic").is_some_and(|w| !w.has("minimized")));
    let start = Instant::now();
    loop {
        let (states, wm) = told(i);
        if !states.contains(&hidden) && wm == Some(1) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "told it is shown: _NET_WM_STATE {states:?}, WM_STATE {wm:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
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
        bytes.chunks(4).take(4).map(|p| p[3]).collect::<Vec<_>>()
    );
}

#[test]
fn xwayland_starts_when_the_first_x11_program_connects() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "XWayland on the first X11 program");
    // DISPLAY is known at once, and nothing of XWayland runs while no X11 program does
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(xwayland_of(&comp), None, "no Xwayland before an X11 program connects");
    // the first X11 program: XWayland starts, and the window it maps at once is managed (shown, in the window list)
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    assert!(xwayland_of(&comp).is_some(), "Xwayland runs once a program has connected");
    let root = conn.setup().roots[screen].root;
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0x00ff00);
    conn.create_window(0, win, root, 0, 0, 200, 150, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"first\0First\0").unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    let start = Instant::now();
    while comp.window("First").is_none() {
        assert!(start.elapsed() < Duration::from_secs(10), "the first X11 program's window is managed: {:?}", comp.must("windows"));
        std::thread::sleep(Duration::from_millis(50));
    }
    // a second program connects to the same XWayland
    let (second, _) = x11rb::connect(Some(&display)).expect("a second X11 program");
    drop(second);
    let pid = xwayland_of(&comp);
    assert!(pid.is_some());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(xwayland_of(&comp), pid, "one XWayland for every X11 program");
    // when the compositor ends, the display's files go with it (XWayland started, and not)
    let files = |d: &str| {
        let n = d.trim_start_matches(':');
        [format!("/tmp/.X11-unix/X{n}"), format!("/tmp/.X{n}-lock")]
    };
    assert!(files(&display).iter().all(|f| std::path::Path::new(f).exists()));
    drop(conn);
    drop(comp);
    assert!(files(&display).iter().all(|f| !std::path::Path::new(f).exists()), "{display}'s files removed");
    let (idle, d2) = with_xwayland_on(&["--size", "800x600"], |_| {});
    drop(idle);
    assert!(files(&d2).iter().all(|f| !std::path::Path::new(f).exists()), "{d2}'s files removed, XWayland never started");
}

#[test]
fn an_x11_programs_own_icon_is_kept_for_the_dock() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "X11 windows' own icons");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let net_wm_icon = conn.intern_atom(false, b"_NET_WM_ICON").unwrap().reply().unwrap().atom;
    // a 2x2 red icon and a 64x64 blue one, as programs offer several sizes
    let mut icon = vec![2u32, 2];
    icon.extend([0xffff0000u32; 4]);
    icon.extend([64, 64]);
    icon.extend(std::iter::repeat_n(0xff0000ffu32, 64 * 64));
    let window = |class: &[u8], with_icon: bool| {
        let win = conn.generate_id().unwrap();
        conn.create_window(0, win, root, 0, 0, 200, 150, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, class).unwrap();
        if with_icon {
            conn.change_property32(PropMode::REPLACE, win, net_wm_icon, AtomEnum::CARDINAL, &icon).unwrap();
        }
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        win
    };
    let dir = comp.dir.join("run/yggdrasil-window-icons");
    let read_png = |name: &str| -> Option<(u32, u32, Vec<u8>)> {
        let f = std::fs::File::open(dir.join(name)).ok()?;
        let mut r = png::Decoder::new(std::io::BufReader::new(f)).read_info().ok()?;
        let mut buf = vec![0; r.output_buffer_size()];
        let info = r.next_frame(&mut buf).ok()?;
        Some((info.width, info.height, buf[..4].to_vec()))
    };
    let wait = |name: &str| {
        let start = Instant::now();
        loop {
            if let Some(got) = read_png(name) {
                return got;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "{name} kept in {}", dir.display());
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // set before the window shows: kept at once, the 64 one, blue
    window(b"withicon\0Withicon\0", true);
    assert_eq!(wait("Withicon.png"), (64, 64, vec![0, 0, 255, 255]));
    // set after it shows: kept once its title changes
    let late = window(b"lateicon\0Lateicon\0", false);
    std::thread::sleep(Duration::from_millis(300));
    assert!(read_png("Lateicon.png").is_none(), "no icon yet");
    conn.change_property32(PropMode::REPLACE, late, net_wm_icon, AtomEnum::CARDINAL, &icon).unwrap();
    conn.change_property8(PropMode::REPLACE, late, AtomEnum::WM_NAME, AtomEnum::STRING, b"Now with an icon").unwrap();
    conn.flush().unwrap();
    assert_eq!(wait("Lateicon.png").0, 64);
    // no half-written files left beside them
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert!(names.iter().all(|n| !n.starts_with('.')), "{names:?}");
}

#[test]
fn xwayland_starts_again_after_it_went_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(90, "XWayland started again after a crash");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let show = |class: &[u8]| {
        let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
        let root = conn.setup().roots[screen].root;
        let win = conn.generate_id().unwrap();
        conn.create_window(0, win, root, 0, 0, 200, 150, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, class).unwrap();
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        conn
    };
    let wait = |what: &str, done: &dyn Fn() -> bool| {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "{what}: {:?}\n{}", comp.must("windows"), comp.log_tail(10));
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let _before = show(b"before\0Before\0");
    wait("the first window managed", &|| comp.window("Before").is_some());
    let first = xwayland_of(&comp).expect("an Xwayland");
    // XWayland crashes: its windows go, and it waits to start again on the same display
    unsafe { libc::kill(first as i32, libc::SIGKILL) };
    wait("its window gone", &|| comp.window("Before").is_none());
    wait("the crashed one gone", &|| xwayland_of(&comp).is_none());
    let said = format!("it starts again with the next X11 program (DISPLAY={display})");
    wait("said so", &|| comp.log.lock().unwrap().iter().any(|l| l.contains(&said)));
    // the next X11 program, on the same DISPLAY: a new XWayland, and its window managed
    let _after = show(b"after\0After\0");
    wait("the next program's window managed", &|| comp.window("After").is_some());
    let second = xwayland_of(&comp).expect("a new Xwayland");
    assert_ne!(first, second);
}

/// The XSETTINGS an X11 program reads: the manager's window (the owner of _XSETTINGS_S0) and its settings, as name to value
/// (numbers as text), with the serial.
fn xsettings(conn: &impl Connection) -> Option<(u32, std::collections::HashMap<String, String>)> {
    let atom = |n: &[u8]| conn.intern_atom(false, n).ok()?.reply().ok().map(|r| r.atom);
    let (sel, prop) = (atom(b"_XSETTINGS_S0")?, atom(b"_XSETTINGS_SETTINGS")?);
    let owner = conn.get_selection_owner(sel).ok()?.reply().ok()?.owner;
    if owner == 0 {
        return None;
    }
    let b = conn.get_property(false, owner, prop, prop, 0, 1 << 16).ok()?.reply().ok()?.value;
    let u32_at = |i: usize| -> Option<u32> { Some(u32::from_le_bytes(b.get(i..i + 4)?.try_into().ok()?)) };
    let pad = |n: usize| n + (4 - n % 4) % 4;
    let (serial, n) = (u32_at(4)?, u32_at(8)?);
    let mut out = std::collections::HashMap::new();
    let mut i = 12;
    for _ in 0..n {
        let kind = *b.get(i)?;
        let len = u16::from_le_bytes(b.get(i + 2..i + 4)?.try_into().ok()?) as usize;
        let name = String::from_utf8_lossy(b.get(i + 4..i + 4 + len)?).into_owned();
        i += 4 + pad(len) + 4;
        let value = match kind {
            0 => {
                i += 4;
                (u32_at(i - 4)? as i32).to_string()
            }
            1 => {
                let l = u32_at(i)? as usize;
                let v = String::from_utf8_lossy(b.get(i + 4..i + 4 + l)?).into_owned();
                i += 4 + pad(l);
                v
            }
            _ => return None,
        };
        out.insert(name, value);
    }
    Some((serial, out))
}

#[test]
fn x11_programs_get_xsettings_and_their_changes() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "XSETTINGS");
    let ini = |dir: &std::path::Path, theme: &str| {
        let d = dir.join("home/.config/gtk-3.0");
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(
            d.join("settings.ini"),
            format!(
                "[Settings]\ngtk-theme-name={theme}\ngtk-icon-theme-name=Test-Icons\ngtk-font-name=Test Sans 11\ngtk-xft-antialias=1\n"
            ),
        )
        .unwrap();
    };
    let (comp, display) = with_xwayland_on(&["--output", "1920x1200@1.25"], |dir| ini(dir, "Test-Theme"));
    let (conn, _) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let until = |what: &str, ok: &dyn Fn(&std::collections::HashMap<String, String>) -> bool| {
        let start = Instant::now();
        loop {
            if let Some((serial, s)) = xsettings(&conn) {
                if ok(&s) {
                    return (serial, s);
                }
            }
            assert!(start.elapsed() < Duration::from_secs(10), "{what}: {:?}", xsettings(&conn));
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // GTK's own settings, the DPI of 125 % and the pointer
    let (first, s) = until("the settings", &|s| s.contains_key("Net/ThemeName"));
    assert_eq!(s["Net/ThemeName"], "Test-Theme");
    assert_eq!(s["Net/IconThemeName"], "Test-Icons");
    assert_eq!(s["Gtk/FontName"], "Test Sans 11");
    assert_eq!(s["Xft/Antialias"], "1");
    assert_eq!(s["Xft/DPI"], (120 * 1024).to_string(), "96 x 1.25, in 1024ths");
    assert!(s.contains_key("Gtk/CursorThemeName") && s.contains_key("Gtk/CursorThemeSize"), "{s:?}");
    // a new pointer theme: told at once, at X11's scale
    comp.must("cursor-theme Adwaita 32");
    let (second, s) = until("the new pointer", &|s| s.get("Gtk/CursorThemeName").map(String::as_str) == Some("Adwaita"));
    assert_eq!(s["Gtk/CursorThemeSize"], "40");
    assert!(second > first, "a new serial");
    // GTK's theme changed in settings.ini (ygg-theme): told within the compositor's look at its files
    ini(&comp.dir, "Other-Theme");
    until("the new theme", &|s| s.get("Net/ThemeName").map(String::as_str) == Some("Other-Theme"));
}

/// The control socket's number for a window (its `clients` line).
fn client_of(comp: &Compositor, class: &str) -> String {
    let lines = comp.must("clients");
    let line = lines.iter().find(|l| l.split('\t').nth(1) == Some(class)).unwrap_or_else(|| panic!("{class} in {lines:?}"));
    line.split('\t').next().unwrap().to_string()
}

#[test]
fn closing_asks_a_program_that_takes_part_and_ends_one_that_does_not() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "closing X11 windows");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let open = |class: &[u8], polite: bool| {
        let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
        let root = conn.setup().roots[screen].root;
        let atom = |n: &[u8]| conn.intern_atom(false, n).unwrap().reply().unwrap().atom;
        let (protocols, delete) = (atom(b"WM_PROTOCOLS"), atom(b"WM_DELETE_WINDOW"));
        let win = conn.generate_id().unwrap();
        conn.create_window(0, win, root, 0, 0, 200, 150, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, class).unwrap();
        if polite {
            conn.change_property32(PropMode::REPLACE, win, protocols, AtomEnum::ATOM, &[delete]).unwrap();
        }
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        (conn, delete)
    };
    let wait = |what: &str, done: &dyn Fn() -> bool| {
        let start = Instant::now();
        while !done() {
            assert!(start.elapsed() < Duration::from_secs(10), "{what}: {:?}", comp.must("windows"));
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // a program that takes part in WM_DELETE_WINDOW is asked, and stays connected to decide
    let (polite, delete) = open(b"polite\0Polite\0", true);
    wait("the polite window", &|| comp.window("Polite").is_some());
    comp.must(&format!("close {}", client_of(&comp, "Polite")));
    let start = Instant::now();
    let asked = loop {
        match polite.poll_for_event() {
            Ok(Some(x11rb::protocol::Event::ClientMessage(m))) if m.data.as_data32()[0] == delete => break true,
            Ok(_) => {}
            Err(e) => panic!("the polite program was cut off: {e}"),
        }
        assert!(start.elapsed() < Duration::from_secs(10), "no WM_DELETE_WINDOW came");
        std::thread::sleep(Duration::from_millis(20));
    };
    assert!(asked);
    assert!(polite.get_input_focus().unwrap().reply().is_ok(), "still connected: it decides itself");
    // one that does not take part is ended (XKillClient), not left running without a window
    let (rude, _) = open(b"rude\0Rude\0", false);
    wait("the rude window", &|| comp.window("Rude").is_some());
    comp.must(&format!("close {}", client_of(&comp, "Rude")));
    wait("the rude window gone", &|| comp.window("Rude").is_none());
    let start = Instant::now();
    loop {
        if rude.get_input_focus().map(|c| c.reply().is_err()).unwrap_or(true) {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "the program that cannot be asked is still connected");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A program hides its window and shows it again while the pointer rests on it: XWayland gives the window a new surface, and
/// the pointer is in that one from then on, so the program hears it move (a full screen game that does this would otherwise
/// get no mouse at all, as the pointer cannot leave its window).
#[test]
fn an_x11_window_shown_again_under_the_resting_pointer_hears_it_move() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "the pointer in an X11 window shown again");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0x00ff00).event_mask(EventMask::POINTER_MOTION);
    conn.create_window(0, win, root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"prog\0Prog\0").unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    comp.wait(10, "the window managed", |c| c.window("Prog").is_some());
    comp.must(&format!("place {} 100 100 300 200", client_of(&comp, "Prog")));
    // A window is listed as soon as the program shows it; the pointer can be in it only once XWayland has made its surface
    // (then it is active) and drawn it (its green, away from where the pointer goes).
    let shown = |c: &Compositor| {
        c.window("Prog").is_some_and(|w| w.at == (100, 100) && w.states.iter().any(|s| s == "active"))
            && c.screenshot("HEADLESS-1").count((110, 220, 100, 50), (0, 255, 0)) == 100 * 50
    };
    comp.wait(10, "the window drawn where it was placed", shown);
    // the pointer to `at` on the screen: does the program hear it move there, in its window (within 2 s)?
    let heard_at = |at: (i32, i32)| {
        comp.must(&format!("pointer {} {}", at.0, at.1));
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            while let Some(e) = conn.poll_for_event().expect("the X11 connection") {
                if let x11rb::protocol::Event::MotionNotify(m) = e {
                    if m.event == win && (m.event_x as i32, m.event_y as i32) == (at.0 - 100, at.1 - 100) {
                        return true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    };
    assert!(heard_at((250, 200)), "the program hears the pointer in its window");
    // hidden and shown again while the pointer rests on it, back where it was (gone from the screen in between)
    conn.unmap_window(win).unwrap();
    conn.flush().unwrap();
    comp.wait(10, "the window hidden", |c| {
        c.window("Prog").is_none() && c.screenshot("HEADLESS-1").count((110, 220, 100, 50), (0, 255, 0)) == 0
    });
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    comp.wait(10, "the window drawn again where it was", shown);
    // the pointer moves within it: heard
    comp.must("pointer 260 200");
    assert!(heard_at((270, 210)), "the program hears the pointer in its window shown again");
}

#[test]
fn an_x11_menu_is_drawn_where_it_was_made_at_a_fractional_scale() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 menu at 125 %");
    // at 125 % X11 programs draw in the screen's pixels: a menu made at (500, 250), 125 x 125, is there on the screen
    // the compositor's debug lines and XWayland's own messages go into its log, for the report of a failure (YGG-115: it has
    // failed in CI with the menu never drawn, and only there)
    let (comp, display) = with_xwayland_env(&["--output", "1600x900@1.25"], &[("YGGDRASIL_COMPOSITOR_DEBUG", "1")], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let menu = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0xff0000).override_redirect(1);
    conn.create_window(0, menu, root, 500, 250, 125, 125, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.map_window(menu).unwrap();
    conn.flush().unwrap();
    let start = Instant::now();
    loop {
        let shot = comp.screenshot("HEADLESS-1");
        let inside = shot.count((505, 255, 115, 115), (255, 0, 0));
        if inside == 115 * 115 {
            // and nothing of it a quarter further out, where a rounded scale would put it
            assert_eq!(shot.count((640, 320, 40, 40), (255, 0, 0)), 0, "drawn only where it was made");
            break;
        }
        if start.elapsed() >= Duration::from_secs(10) {
            // where the red is, if anywhere: a quarter further out means its place was taken at scale 1, none means not drawn
            let (mut x0, mut y0, mut x1, mut y1, mut n) = (usize::MAX, usize::MAX, 0, 0, 0);
            for y in 0..shot.h {
                for x in 0..shot.w {
                    if shot.at(x, y) == (255, 0, 0) {
                        (x0, y0, x1, y1, n) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y), n + 1);
                    }
                }
            }
            // and what X and the compositor know of it: mapped for X (a map_state of 2 is Viewable), the windows listed
            let state = conn.get_window_attributes(menu).ok().and_then(|c| c.reply().ok()).map(|a| a.map_state);
            panic!(
                "the menu where it was made: {inside} red pixels of {}; red on the screen: {n} pixels in {x0},{y0} to {x1},{y1}; \
                 X's map state of it: {state:?}; windows: {:?}\n{}",
                115 * 115,
                comp.ctl("windows"),
                comp.log_tail(60)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// GTK and Chromium draw their own shadow inside their X window once the window manager says it reads _GTK_FRAME_EXTENTS (how
/// wide the shadow is), as Smithay's does. The window is the part inside the shadow: its X window goes where any X11 window's
/// would, and that part is listed, drawn and clicked exactly where the X server has it. When the shadow goes (GTK drops it when
/// maximized) and comes back, the window keeps its place and size: its X window is placed around it anew. Only the inner edge
/// of the shadow takes clicks (where GTK keeps its resize border); further out they reach what is behind.
#[test]
fn an_x11_window_with_its_own_shadow_is_drawn_and_clicked_where_its_x_window_is() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 window with its own shadow");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let atom = |n: &str| conn.intern_atom(false, n.as_bytes()).unwrap().reply().unwrap().atom;
    let (extents, motif) = (atom("_GTK_FRAME_EXTENTS"), atom("_MOTIF_WM_HINTS"));
    // a 300x200 window in a 20 pixel shadow, as GTK makes one: the shadow green here, the window a red child inside it, with
    // its own title bar (Motif: no decorations), so the compositor draws no controls on it
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0x00ff00).event_mask(EventMask::BUTTON_PRESS);
    conn.create_window(0, win, root, 0, 0, 340, 240, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    let inner = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0xff0000).event_mask(EventMask::BUTTON_PRESS);
    conn.create_window(0, inner, win, 20, 20, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.map_window(inner).unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"x11shadow\0X11shadow\0").unwrap();
    conn.change_property32(PropMode::REPLACE, win, motif, motif, &[2, 0, 0, 0, 0]).unwrap();
    let shadow = |px: u32| {
        // left, right, top, bottom; and the program lays its window out inside the shadow, as GTK does
        conn.change_property32(PropMode::REPLACE, win, extents, AtomEnum::CARDINAL, &[px; 4]).unwrap();
        conn.configure_window(inner, &ConfigureWindowAux::new().x(px as i32).y(px as i32)).unwrap();
        conn.flush().unwrap();
    };
    shadow(20);
    conn.map_window(win).unwrap();
    conn.flush().unwrap();

    // the X window where the X server has it, and its size
    let x_window = || {
        let at = conn.translate_coordinates(win, root, 0, 0).unwrap().reply().unwrap();
        let size = conn.get_geometry(win).unwrap().reply().unwrap();
        (at.dst_x as i32, at.dst_y as i32, size.width as i32, size.height as i32)
    };
    // the window listed at `window` with its X window at `x`; and with `drawn`, its picture there: all of the window red,
    // nothing of the program's shadow (the compositor draws its own)
    let settled = |window: (i32, i32, i32, i32), x: (i32, i32, i32, i32), drawn: bool| {
        let start = Instant::now();
        loop {
            let listed = comp.window("X11shadow").map(|w| w.rect());
            let now = x_window();
            let (red, green) = if drawn {
                let shot = comp.screenshot("HEADLESS-1");
                (shot.fills(window, (255, 0, 0), 10), shot.count((0, 0, 800, 600), (0, 255, 0)))
            } else {
                (true, 0)
            };
            if listed == Some(window) && now == x && red && green == 0 {
                return;
            }
            assert!(
                start.elapsed() < Duration::from_secs(10),
                "listed at {listed:?} (wanted {window:?}), the X window at {now:?} (wanted {x:?}), the window all red: {red}, \
                 {green} green pixels\n{}",
                comp.log_tail(20)
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    // a click at 130,120 lands 50,40 into the window in X too, and at 130,120 of the X screen
    let click_lands = || {
        while conn.poll_for_event().unwrap().is_some() {}
        comp.must("pointer 130 120");
        comp.must("click");
        let start = Instant::now();
        loop {
            match conn.poll_for_event().unwrap() {
                Some(x11rb::protocol::Event::ButtonPress(e)) => {
                    assert_eq!((e.event, e.event_x, e.event_y, e.root_x, e.root_y), (inner, 50, 40, 130, 120), "where the click landed");
                    return;
                }
                Some(_) => {}
                None => {
                    assert!(start.elapsed() < Duration::from_secs(5), "the click reached the window: {}", comp.log_tail(20));
                    std::thread::sleep(Duration::from_millis(20));
                }
            }
        }
    };

    // its X window goes where one without a shadow would (the first window 60,60 in from the screen's corner)
    let window = (80, 80, 300, 200);
    settled(window, (60, 60, 340, 240), true);
    click_lands();
    // the shadow goes: the window stays where it is and as big, its X window is now just the window
    shadow(0);
    settled(window, window, true);
    click_lands();
    // and comes back: the X window is placed around the window again, where it first was
    shadow(20);
    settled(window, (60, 60, 340, 240), true);
    click_lands();

    // maximized, the window fills the free room less the gaps, its shadow around it until the program drops it (as GTK does);
    // restored, it is back where it was and as big, and so is its X window once the shadow is back
    let id = client_of(&comp, "X11shadow");
    comp.must(&format!("maximize {id} on"));
    comp.wait(10, "the window maximized", |c| c.window("X11shadow").is_some_and(|w| w.has("maximized")));
    let room = maximized_in((0, 0, 800, 600));
    settled(room, (room.0 - 20, room.1 - 20, room.2 + 40, room.3 + 40), false);
    shadow(0);
    settled(room, room, false);
    comp.must(&format!("maximize {id} off"));
    settled(window, window, false);
    shadow(20);
    settled(window, (60, 60, 340, 240), true);
    click_lands();

    // Only the edge of the shadow takes clicks, where GTK keeps its resize border (GTK 3 takes them 10 pixels out from its
    // window, GTK 4 12): further out a click reaches what is behind, here a Wayland window. XWayland does not hand on the X
    // input shape that says so.
    let mut c = comp.connect();
    let behind = c.window("test.behind", "Behind");
    c.draw(&behind, (200, 150), 0xff0000ff);
    c.roundtrip();
    comp.wait(10, "the window behind listed", |c| c.window("test.behind").is_some());
    comp.must(&format!("place {} 300 100 200 150", client_of(&comp, "test.behind")));
    let active = |app: &str| comp.must("active").iter().any(|l| l.split('\t').nth(1) == Some(app));
    comp.must(&format!("activate {id}"));
    comp.wait(10, "the X11 window in front", |_| active("X11shadow"));
    // 12 pixels right of the window, in its shadow
    while conn.poll_for_event().unwrap().is_some() {}
    comp.must("pointer 392 150");
    comp.must("click");
    comp.wait(10, "the window behind given the click", |_| active("test.behind"));
    // 5 pixels out it is the X11 window's, in X in its shadow (the first press it gets: the click 12 pixels out sent it none)
    comp.must(&format!("activate {id}"));
    comp.wait(10, "the X11 window in front again", |_| active("X11shadow"));
    comp.must("pointer 385 150");
    comp.must("click");
    let start = Instant::now();
    loop {
        match conn.poll_for_event().unwrap() {
            Some(x11rb::protocol::Event::ButtonPress(e)) => {
                assert_eq!((e.event, e.event_x, e.event_y), (win, 325, 90), "where the click in the shadow landed");
                break;
            }
            Some(_) => {}
            None => {
                assert!(start.elapsed() < Duration::from_secs(5), "the click reached the shadow: {}", comp.log_tail(20));
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

/// A program that draws its own shadow but leaves the window's controls to the compositor (no Motif hints) gets the
/// compositor's grips just outside the window, over its shadow: a press there is the grip's, not the shadow's, and dragging
/// it resizes the window, its X window following with the shadow around it.
#[test]
fn the_grips_of_an_x11_window_lie_over_its_own_shadow() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "grips over an X11 window's shadow");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let extents = conn.intern_atom(false, b"_GTK_FRAME_EXTENTS").unwrap().reply().unwrap().atom;
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0xff0000).event_mask(EventMask::BUTTON_PRESS);
    conn.create_window(0, win, root, 0, 0, 340, 240, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"x11grips\0X11grips\0").unwrap();
    conn.change_property32(PropMode::REPLACE, win, extents, AtomEnum::CARDINAL, &[20; 4]).unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    let x_window = || {
        let at = conn.translate_coordinates(win, root, 0, 0).unwrap().reply().unwrap();
        let size = conn.get_geometry(win).unwrap().reply().unwrap();
        (at.dst_x as i32, at.dst_y as i32, size.width as i32, size.height as i32)
    };
    comp.wait(10, "the window shown", |c| c.window("X11grips").is_some_and(|w| w.rect() == (80, 80, 300, 200)));
    // 3 pixels right of the window: its right grip (the compositor's are 6 pixels wide), dragged 30 pixels further right
    comp.must("pointer 383 180");
    comp.must("press");
    comp.must("pointer 413 180");
    comp.must("release");
    let start = Instant::now();
    loop {
        let (listed, now) = (comp.window("X11grips").map(|w| w.rect()), x_window());
        if listed == Some((80, 80, 330, 200)) && now == (60, 60, 370, 240) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "after the drag: listed at {listed:?}, the X window at {now:?}\n{}",
            comp.log_tail(20)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A program may write anything into _GTK_FRAME_EXTENTS, and Smithay reads each side as 0 to i32::MAX (one above it as none):
/// sides that add up to 2^31, a side of i32::MAX on the left and top or on the right and bottom, sides above i32::MAX, and a
/// shadow far wider than its window, set before the window is shown and on a shown one, then maximized and restored. The
/// compositor must survive each (a debug build: an overflow ends it), and a shadow no program draws is not believed, so the
/// window stays as its X window was made (before, the first two ended the compositor, and the third and the last grew the X
/// window to 65535x65535 and 2000x2000 around a window of no size).
#[test]
fn absurd_x11_shadows_do_not_bring_the_compositor_down() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(120, "absurd X11 shadows");
    let (comp, display) = with_xwayland_on(&["--size", "800x600"], |_| {});
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let extents = conn.intern_atom(false, b"_GTK_FRAME_EXTENTS").unwrap().reply().unwrap().atom;
    let shadow = |win: u32, sides: [u32; 4]| {
        conn.change_property32(PropMode::REPLACE, win, extents, AtomEnum::CARDINAL, &sides).unwrap();
        conn.flush().unwrap();
    };
    let x_window = |win: u32| {
        let at = conn.translate_coordinates(win, root, 0, 0).unwrap().reply().unwrap();
        let size = conn.get_geometry(win).unwrap().reply().unwrap();
        (at.dst_x as i32, at.dst_y as i32, size.width as i32, size.height as i32)
    };
    // a command, and the window's line in `windows` (place, size, states): a compositor that stopped answering ends the test
    // with what it printed last
    let ask = |cmd: &str, after: &str| {
        comp.ctl(cmd).unwrap_or_else(|e| panic!("the compositor ended after {after} ({cmd}: {e}): {}", comp.log_tail(60)))
    };
    let line = |class: &str, after: &str| {
        ask("windows", after).iter().map(|l| l.split('\t').map(String::from).collect::<Vec<_>>()).find(|p| p[0] == class)
    };
    // soon listed at `window` (by default where its X window is), with its X window at `x` (by default anywhere)
    type Rect = Option<(i32, i32, i32, i32)>;
    let settled = |class: &str, win: u32, window: Rect, x: Rect, after: &str| {
        let start = Instant::now();
        loop {
            let listed = line(class, after).map(|p| {
                let (at, size) = (pair(&p[2], ','), pair(&p[3], 'x'));
                (at.0, at.1, size.0, size.1)
            });
            let now = x_window(win);
            if listed.is_some() && listed == Some(window.unwrap_or(now)) && x.is_none_or(|x| x == now) {
                return now;
            }
            assert!(start.elapsed() < Duration::from_secs(10), "after {after}: listed at {listed:?}, the X window at {now:?}");
            std::thread::sleep(Duration::from_millis(50));
        }
    };
    let max = i32::MAX as u32;
    for (i, sides) in [[1 << 30; 4], [max, 0, max, 0], [0, max, 0, max], [u32::MAX; 4], [1000; 4]].into_iter().enumerate() {
        let class = format!("Absurd{i}");
        let win = conn.generate_id().unwrap();
        let aux = CreateWindowAux::new().background_pixel(0xff0000);
        conn.create_window(0, win, root, 0, 0, 340, 240, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
        conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, format!("absurd\0{class}\0").as_bytes())
            .unwrap();
        shadow(win, sides);
        conn.map_window(win).unwrap();
        conn.flush().unwrap();
        let made = settled(&class, win, None, None, &format!("a window shown with the shadow {sides:?}"));
        assert_eq!((made.2, made.3), (340, 240), "the X window as it was made");
        // on the shown window: a real shadow (the X window is placed around the window), then the absurd one again (the X
        // window is just the window again)
        shadow(win, [20; 4]);
        settled(&class, win, Some(made), Some((made.0 - 20, made.1 - 20, 380, 280)), "a real shadow");
        shadow(win, sides);
        settled(&class, win, Some(made), Some(made), &format!("the shadow {sides:?} on a shown window"));
        let id = client_of(&comp, &class);
        ask(&format!("maximize {id} on"), "the window maximized");
        let start = Instant::now();
        while !line(&class, "the window maximized").is_some_and(|p| p[5].split(',').any(|s| s == "maximized")) {
            assert!(start.elapsed() < Duration::from_secs(10), "the window maximized: {}", comp.log_tail(20));
            std::thread::sleep(Duration::from_millis(50));
        }
        settled(&class, win, None, None, "the window maximized");
        ask(&format!("maximize {id} off"), "the window restored");
        settled(&class, win, Some(made), Some(made), "the window restored");
        conn.destroy_window(win).unwrap();
        conn.flush().unwrap();
        comp.wait(10, "the window gone", |c| c.window(&class).is_none());
    }
    std::thread::sleep(Duration::from_millis(300));
    ask("windows", "all of it");
}
