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
        bytes.chunks(4).take(4).map(|p| p[3]).collect::<Vec<_>>()
    );
}

/// The compositor's Xwayland process, if one runs.
fn xwayland_of(comp: &Compositor) -> Option<u32> {
    let parent = comp.child.id().to_string();
    std::fs::read_dir("/proc").ok()?.filter_map(Result::ok).find_map(|e| {
        let stat = std::fs::read_to_string(e.path().join("stat")).ok()?;
        // pid (comm) state ppid ...
        let (head, rest) = stat.rsplit_once(')')?;
        let comm = head.split_once('(')?.1;
        let ppid = rest.split_whitespace().nth(1)?;
        (comm == "Xwayland" && ppid == parent && !rest.trim_start().starts_with('Z'))
            .then(|| e.file_name().to_string_lossy().parse().ok())?
    })
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

#[test]
fn an_x11_menu_is_drawn_where_it_was_made_at_a_fractional_scale() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 menu at 125 %");
    // at 125 % X11 programs draw in the screen's pixels: a menu made at (500, 250), 125 x 125, is there on the screen
    let (comp, display) = with_xwayland_on(&["--output", "1600x900@1.25"], |_| {});
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
        assert!(start.elapsed() < Duration::from_secs(10), "the menu where it was made: {inside} red pixels of {}", 115 * 115);
        std::thread::sleep(Duration::from_millis(50));
    }
}
