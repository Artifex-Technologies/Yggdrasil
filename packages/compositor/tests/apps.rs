//! Yggdrasil's own programs on the headless compositor: the panel (bar, dock, wallpaper, notifications, the start menu, its desktop
//! buttons and window zones), Settings (an egui window drawn in software), the lock screen (in its test-auth build) and the
//! session's window keys yggdrasil-snap and yggdrasil-close, with a terminal stand-in of the tests' own (tests/common's window,
//! which records the keys it gets).
//!
//! The programs are the ones built from this repository, named by variables (nothing is looked up in PATH):
//!   YGGDRASIL_TEST_PANEL     target/debug/yggdrasil-panel
//!   YGGDRASIL_TEST_SETTINGS  target/debug/yggdrasil-settings
//!   YGGDRASIL_TEST_LOCK      yggdrasil-lock built with `--features test-auth` (refused otherwise: a real build would check the
//!                            password with PAM)
//!   YGGDRASIL_TEST_SNAP      target/debug/yggdrasil-snap (packages/wm)
//!   YGGDRASIL_TEST_CLOSE     target/debug/yggdrasil-close (packages/wm)
//!   YGGDRASIL_TEST_LOGOUT    target/debug/yggdrasil-logout (packages/wm); it runs with stand-ins for systemd-cat, notify-send,
//!                            loginctl and systemctl as its only PATH, so it never locks or stops anything of the real desktop
//! A test whose program is not named is skipped, unless YGGDRASIL_TEST_APPS_REQUIRED is set (CI's compositor-headless job).
//! tests/apps-podman.sh runs these in a container with no devices and no network.
//!
//! Each program runs with nothing of the running desktop: an empty environment but for PATH=/usr/bin, the compositor's private
//! runtime folder and home, its Wayland socket, and a session bus of the test's own (no service activation). It dies with the test.

mod common;

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::*;
use wayland_client::Proxy;

const BLUE: u32 = 0xff0000ff;
const RED: u32 = 0xffff0000;
const RGB_BLUE: (u8, u8, u8) = (0, 0, 255);

/// A program named by a variable, or None (the test is skipped) unless every program is required.
fn program(var: &str) -> Option<PathBuf> {
    match std::env::var_os(var).map(PathBuf::from) {
        Some(p) if p.is_file() => Some(p),
        Some(p) => panic!("{var}={}: no such program", p.display()),
        None => {
            assert!(std::env::var_os("YGGDRASIL_TEST_APPS_REQUIRED").is_none(), "{var} is not set");
            eprintln!("skipped: {var} is not set");
            None
        }
    }
}

/// A session bus of the test's own: no service files, so nothing installed is ever started by it.
struct Bus {
    child: Child,
    address: String,
}

impl Bus {
    fn start(dir: &Path) -> Bus {
        let socket = dir.join("bus");
        let conf = dir.join("bus.conf");
        std::fs::write(
            &conf,
            format!(
                r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}</listen>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
                socket.display()
            ),
        )
        .unwrap();
        let mut cmd = Command::new("/usr/bin/dbus-daemon");
        cmd.arg(format!("--config-file={}", conf.display())).arg("--nofork").env_clear().stdout(Stdio::null()).stderr(Stdio::null());
        let child = dies_with_test(&mut cmd).spawn().expect("dbus-daemon (the dbus package) runs");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !socket.exists() {
            assert!(Instant::now() < deadline, "the test's session bus did not start");
            std::thread::sleep(Duration::from_millis(20));
        }
        Bus { child, address: format!("unix:path={}", socket.display()) }
    }

    /// busctl on this bus: the answer's text.
    fn call(&self, args: &[&str]) -> Result<String, String> {
        let mut cmd = Command::new("/usr/bin/busctl");
        cmd.arg(format!("--address={}", self.address)).args(args).env_clear().env("PATH", "/usr/bin");
        let out = dies_with_test(&mut cmd).output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).into_owned())
        }
    }

    fn has(&self, name: &str) -> bool {
        self.call(&["status", name]).is_ok()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// One of Yggdrasil's programs inside the compositor.
struct App {
    child: Child,
    log: PathBuf,
}

impl App {
    fn start(comp: &Compositor, bin: &Path, bus: Option<&Bus>, env: &[(&str, &str)]) -> App {
        let name = bin.file_name().unwrap().to_string_lossy().into_owned();
        let log = comp.dir.join(format!("{name}.log"));
        let home = comp.dir.join("home");
        let mut cmd = Command::new(bin);
        cmd.env_clear()
            .env("PATH", "/usr/bin")
            .env("HOME", &home)
            .env("USER", "yggdrasil-test")
            .env("LANG", "C.UTF-8")
            .env("RUST_BACKTRACE", "1")
            .env("XDG_RUNTIME_DIR", comp.dir.join("run"))
            .env("WAYLAND_DISPLAY", comp.wayland.file_name().unwrap())
            // no bus at all unless the test gives its own: never the desktop's
            .env(
                "DBUS_SESSION_BUS_ADDRESS",
                bus.map(|b| b.address.clone()).unwrap_or_else(|| format!("unix:path={}/no-bus", comp.dir.display())),
            )
            .envs(env.iter().copied())
            .stdin(Stdio::null())
            .stdout(std::fs::File::create(&log).unwrap())
            .stderr(std::fs::File::options().append(true).open(&log).unwrap());
        let child = dies_with_test(&mut cmd).spawn().unwrap_or_else(|e| panic!("{name} starts: {e}"));
        App { child, log }
    }

    fn log(&self) -> String {
        let mut s = String::new();
        let _ = std::fs::File::open(&self.log).and_then(|mut f| f.read_to_string(&mut s));
        s
    }

    fn running(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Wait for it to end on its own: its exit code.
    fn wait_exit(&mut self, secs: u64) -> Option<i32> {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < deadline {
            if let Ok(Some(st)) = self.child.try_wait() {
                return st.code();
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Wait until a program's surfaces are listed, failing with its log.
fn wait_for(comp: &Compositor, app: &mut App, secs: u64, what: &str, done: impl Fn(&Compositor) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(secs);
    while !done(comp) {
        if !app.running() || Instant::now() > deadline {
            panic!(
                "{what}: {}\nlayers: {:?}\nwindows: {:?}\nits log:\n{}\ncompositor log:\n{}",
                if app.running() { "timed out" } else { "the program ended" },
                comp.ctl("layers"),
                comp.ctl("windows"),
                app.log(),
                comp.log_tail(20)
            );
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn layer<'a>(layers: &'a [LayerInfo], ns: &str) -> Option<&'a LayerInfo> {
    layers.iter().find(|l| l.namespace == ns)
}

/// The room the compositor leaves free for windows on an output (its `free` list), checked against the bar and the dock: below
/// the bar, above the dock.
fn free_room(comp: &Compositor, output: &str) -> (i32, i32, i32, i32) {
    let line = comp.must("free").into_iter().find(|l| l.starts_with(&format!("{output}\t"))).expect("the output's free room");
    let p: Vec<&str> = line.split('\t').collect();
    let (at, size) = (pair(p[1], ','), pair(p[2], 'x'));
    let layers = comp.layers();
    let bar = layer(&layers, "yggdrasil-panel").expect("the bar");
    let dock = layer(&layers, "yggdrasil-dock").expect("the dock");
    assert_eq!(at.1, bar.at.1 + bar.size.1, "the free room starts under the bar");
    assert!(at.1 + size.1 <= dock.at.1, "the free room ends above the dock: {line} {dock:?}");
    (at.0, at.1, size.0, size.1)
}

/// Re-draw the stand-in window at whatever size it was last given, until it is listed at that size.
fn settle(comp: &Compositor, c: &mut Client, w: &Window, app_id: &str, argb: u32) -> WinInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        c.roundtrip();
        let given = c.state.wins[w.index].size;
        let info = comp.window(app_id).expect("the stand-in is listed");
        if given.0 <= 0 || info.size == given {
            return info;
        }
        assert!(Instant::now() < deadline, "{app_id} did not settle at {given:?}: {info:?}");
        c.draw(w, given, argb);
    }
}

#[test]
fn the_panel_draws_its_bar_dock_and_wallpaper_and_windows_keep_clear_of_them() {
    let Some(panel) = program("YGGDRASIL_TEST_PANEL") else { return };
    let _l = limit(150, "panel");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let bg = background(&comp.screenshot("HEADLESS-1"));
    let bus = Bus::start(&comp.dir);
    // a wallpaper of the test's own (the panel draws one in Yggdrasil's own session when one is chosen)
    let home = comp.dir.join("home");
    std::fs::create_dir_all(home.join(".config/yggdrasil")).unwrap();
    let wall = home.join("wallpaper.png");
    {
        let mut enc = png::Encoder::new(std::fs::File::create(&wall).unwrap(), 64, 64);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        let px: Vec<u8> = (0..64 * 64).flat_map(|i| [40u8, 120, (i % 64 * 4) as u8]).collect();
        enc.write_header().unwrap().write_image_data(&px).unwrap();
    }
    std::fs::write(home.join(".config/yggdrasil/wallpaper"), format!("{}\n", wall.display())).unwrap();
    let mut p = App::start(&comp, &panel, Some(&bus), &[("YGGDRASIL_A11Y", "off"), ("XDG_CURRENT_DESKTOP", "Yggdrasil")]);
    wait_for(&comp, &mut p, 60, "the bar, the dock and the wallpaper", |c| {
        let l = c.layers();
        ["yggdrasil-panel", "yggdrasil-dock", "yggdrasil-wallpaper"].iter().all(|ns| layer(&l, ns).is_some_and(|x| x.size.0 > 0))
    });
    let layers = comp.layers();
    let bar = layer(&layers, "yggdrasil-panel").unwrap();
    assert_eq!((bar.output.as_str(), bar.layer.as_str(), bar.at, bar.size.0), ("HEADLESS-1", "Top", (0, 0), 1600), "{bar:?}");
    let wall = layer(&layers, "yggdrasil-wallpaper").unwrap();
    assert_eq!((wall.layer.as_str(), wall.at, wall.size), ("Background", (0, 0), (1600, 900)));
    let dock = layer(&layers, "yggdrasil-dock").unwrap();
    assert_eq!(dock.layer, "Top");
    assert!(dock.at.1 + dock.size.1 <= 900 && dock.at.1 > 450, "the dock at the bottom: {dock:?}");
    let room = free_room(&comp, "HEADLESS-1");

    // drawn: the bar, the dock and the wallpaper are pictures, not the compositor's empty background
    std::thread::sleep(Duration::from_millis(500));
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.share_not((0, 0, 1600, bar.size.1), bg) > 0.5, "the bar is drawn");
    assert!(pic.share_not((dock.at.0, dock.at.1, dock.size.0, dock.size.1), bg) > 0.5, "the dock is drawn");
    assert!(pic.share_not(room, bg) > 0.9, "the wallpaper is drawn");

    // the panel serves notifications and its own name on the test's bus
    let deadline = Instant::now() + Duration::from_secs(30);
    while !(bus.has("org.freedesktop.Notifications") && bus.has("org.yggdrasil.Panel")) {
        assert!(Instant::now() < deadline, "the panel's bus names: {}", p.log());
        std::thread::sleep(Duration::from_millis(100));
    }

    // a window (the terminal stand-in) opens centred in the room the bar and the dock leave, and maximizes to exactly that room
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    let t = c.window("test.terminal", "Terminal");
    c.draw(&t, (640, 400), BLUE);
    let info = comp.window("test.terminal").unwrap();
    let want = (room.0 + (room.2 - 640) / 2, room.1 + (room.3 - 400) / 2);
    assert!((info.at.0 - want.0).abs() <= 1 && (info.at.1 - want.1).abs() <= 1, "centred in {room:?}: {info:?}");
    comp.must("key super+Up");
    let info = settle(&comp, &mut c, &t, "test.terminal", BLUE);
    assert_eq!(info.rect(), room, "maximized to the room the bar and dock leave");
    comp.must("key super+Up");
    settle(&comp, &mut c, &t, "test.terminal", BLUE);

    // a notification shows as a toast and leaves the keyboard where it was
    let id = bus
        .call(&[
            "call",
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Notify",
            "susssasa{sv}i",
            "headless-test",
            "0",
            "",
            "Headless",
            "A notification on the test compositor",
            "0",
            "0",
            "5000",
        ])
        .expect("Notify");
    assert!(id.starts_with("u "), "a notification id: {id}");
    wait_for(&comp, &mut p, 10, "the toast", |c| layer(&c.layers(), "yggdrasil-toast").is_some());
    let toast = layer(&comp.layers(), "yggdrasil-toast").cloned().unwrap();
    assert_eq!(toast.layer, "Overlay");
    assert!(toast.at.0 + toast.size.0 <= 1600 && toast.at.1 + toast.size.1 <= 900, "on the screen: {toast:?}");
    assert_eq!(comp.focus(), "window\ttest.terminal\tTerminal", "a toast does not take the keyboard");

    // the start menu takes the keyboard at once (exclusive), and gives it back when it closes
    bus.call(&["call", "org.yggdrasil.Panel", "/Panel", "org.yggdrasil.Panel", "ToggleStart"]).expect("ToggleStart");
    comp.wait(10, "the start menu with the keyboard", |c| c.focus().starts_with("layer\tyggdrasil-popup\t"));
    // (modifier keys such as the Super of Super+Up reach the window too, as on Hyprland: only the others count here)
    let typed = |c: &Client| c.state.keys.iter().filter(|(_, k)| ![29, 42, 54, 56, 97, 100, 125, 126].contains(k)).count();
    let before = typed(&c);
    comp.must("type x");
    c.roundtrip();
    assert_eq!(typed(&c), before, "keys go to the menu, not the window");
    bus.call(&["call", "org.yggdrasil.Panel", "/Panel", "org.yggdrasil.Panel", "ToggleStart"]).expect("ToggleStart");
    comp.wait(10, "the keyboard back on the window", |c| c.focus() == "window\ttest.terminal\tTerminal");
    comp.must("type ls");
    c.roundtrip();
    let last: Vec<(bool, u32)> = c.state.keys.iter().rev().take(2).rev().map(|(s, k)| (s == &t.surface.id(), *k)).collect();
    assert_eq!(last, [(true, 38), (true, 31)], "typing reaches the window again");

    // a smaller screen: the bar and the wallpaper follow it
    comp.must("resize HEADLESS-1 1280x720");
    comp.wait(10, "the bar at the new width", |c| layer(&c.layers(), "yggdrasil-panel").is_some_and(|b| b.size.0 == 1280));
    comp.wait(10, "the wallpaper at the new size", |c| layer(&c.layers(), "yggdrasil-wallpaper").is_some_and(|w| w.size == (1280, 720)));
    assert!(p.running(), "the panel is still running: {}", p.log());
}

#[test]
fn settings_opens_centred_maximizes_and_closes() {
    let Some(settings) = program("YGGDRASIL_TEST_SETTINGS") else { return };
    let _l = limit(240, "settings");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    // a bar and a dock of the test's own (the panel test covers the real ones)
    let bar = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&bar, RED);
    let dock = c.layer_with(0, Layer::Top, "test-dock", Anchor::Bottom, (600, 64), 64, KeyboardInteractivity::None);
    c.draw_layer(&dock, RED);
    let room = (0, 32, 1600, 804);
    // drawn in software: Mesa's llvmpipe through EGL on shared memory, no GPU (and none is opened)
    let soft = [
        ("LIBGL_ALWAYS_SOFTWARE", "1"),
        ("__EGL_VENDOR_LIBRARY_FILENAMES", "/usr/share/glvnd/egl_vendor.d/50_mesa.json"),
        ("WGPU_BACKEND", "gl"),
    ];
    let bus = Bus::start(&comp.dir);
    let mut s = App::start(&comp, &settings, Some(&bus), &soft);
    wait_for(&comp, &mut s, 120, "the Settings window, drawn", |c| c.window("yggdrasil-settings").is_some_and(|w| w.size.0 > 0));
    let info = comp.window("yggdrasil-settings").unwrap();
    assert_eq!(info.title, "Settings");
    assert_eq!(info.size, (1060, 720), "the size it asks for");
    assert_eq!(info.at, (270, 74), "centred in the free room {room:?}");
    assert!(info.has("floating") && info.has("active"), "{info:?}");
    assert_eq!(comp.focus(), "window\tyggdrasil-settings\tSettings");

    // its picture: drawn, not blank (many colours in its rectangle)
    std::thread::sleep(Duration::from_millis(1500));
    let pic = comp.screenshot("HEADLESS-1");
    let mut colours = std::collections::HashSet::new();
    for y in (info.at.1..info.at.1 + 720).step_by(7) {
        for x in (info.at.0..info.at.0 + 1060).step_by(7) {
            colours.insert(pic.at(x as usize, y as usize));
        }
    }
    assert!(colours.len() > 8, "Settings drew its page ({} colours)", colours.len());

    // Super+Up: maximized to the free room; again: back at its size, centred
    comp.must("key super+Up");
    wait_for(&comp, &mut s, 30, "Settings maximized", |c| {
        c.window("yggdrasil-settings").is_some_and(|w| w.rect() == room && w.has("maximized"))
    });
    comp.must("key super+Up");
    wait_for(&comp, &mut s, 30, "Settings back at its size", |c| {
        c.window("yggdrasil-settings").is_some_and(|w| w.rect() == (270, 74, 1060, 720) && !w.has("maximized"))
    });

    // Super+Q closes it (the session's Super+Q / Alt+F4)
    comp.must("key super+q");
    assert_eq!(s.wait_exit(30), Some(0), "Settings ends when its window is closed: {}", s.log());
    assert!(comp.window("yggdrasil-settings").is_none());
}

#[test]
fn the_lock_screen_covers_both_outputs_and_unlocks_with_the_password() {
    let Some(lock) = program("YGGDRASIL_TEST_LOCK") else { return };
    // never type into a lock screen that asks PAM: only the test-auth build compares with the variable below
    let bytes = std::fs::read(&lock).unwrap();
    assert!(
        bytes.windows(28).any(|w| w == b"YGGDRASIL_LOCK_TEST_PASSWORD"),
        "{} is not a test-auth build (cargo build --features test-auth): refused",
        lock.display()
    );
    let _l = limit(120, "lock");
    let comp = Compositor::start(&["--output", "1280x720", "--output", "1024x768@1.5"]);
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    let t = c.window("test.terminal", "Terminal");
    c.draw(&t, (500, 300), BLUE);
    c.roundtrip();
    assert_eq!(c.state.kb_focus, Some(t.surface.id()));

    let mut l = App::start(&comp, &lock, None, &[("YGGDRASIL_LOCK_TEST_PASSWORD", "correct horse")]);
    wait_for(&comp, &mut l, 60, "locked, both outputs drawn", |c| {
        let s = c.must("lock");
        s.len() == 3 && s[0] == "locked" && s[1..].iter().all(|x| !x.ends_with("\t0x0"))
    });
    assert_eq!(comp.must("lock"), ["locked", "HEADLESS-1\t1280x720", "HEADLESS-2\t683x512"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    while !l.log().contains("locked") {
        assert!(Instant::now() < deadline, "the lock says \"locked\" once the compositor confirmed it");
        std::thread::sleep(Duration::from_millis(50));
    }
    std::thread::sleep(Duration::from_millis(500));
    for (o, (w, h)) in [("HEADLESS-1", (1280, 720)), ("HEADLESS-2", (1024, 768))] {
        let pic = comp.screenshot(o);
        assert_eq!(pic.count((0, 0, w, h), RGB_BLUE), 0, "nothing of the session on {o}");
        assert!(pic.share_not((0, 0, w, h), (0, 0, 0)) > 0.9, "the lock screen is drawn on {o}");
    }
    assert_eq!(comp.focus(), "lock\tHEADLESS-1");

    // a wrong password keeps it locked; nothing typed reaches the window behind
    comp.must("type wrong");
    comp.must("key Return");
    std::thread::sleep(Duration::from_millis(800));
    assert!(l.running(), "still locked after a wrong password: {}", l.log());
    assert_eq!(comp.must("lock")[0], "locked");
    // the right one ends it (exit 0), and the keyboard goes back to the window
    comp.must("type correct horse");
    comp.must("key Return");
    assert_eq!(l.wait_exit(20), Some(0), "the lock ends after the right password: {}", l.log());
    assert_eq!(comp.must("lock"), ["unlocked"]);
    assert_eq!(comp.focus(), "window\ttest.terminal\tTerminal");
    c.roundtrip();
    assert!(c.state.keys.is_empty(), "no key typed while locked reached the window: {:?}", c.state.keys);
    comp.must("type ok");
    c.roundtrip();
    assert_eq!(c.state.keys.len(), 2, "typing reaches the window again");
    assert!(comp.screenshot("HEADLESS-1").count((0, 0, 1280, 720), RGB_BLUE) > 0);
}

/// A bar (32, reserved) and a dock (64, reserved) of the test's own, as the panel makes them.
fn bar_and_dock(c: &mut Client) -> (LayerSurf, LayerSurf) {
    let bar = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&bar, RED);
    let dock = c.layer_with(0, Layer::Top, "test-dock", Anchor::Bottom, (600, 64), 64, KeyboardInteractivity::None);
    c.draw_layer(&dock, 0xff00ff00);
    (bar, dock)
}

/// One of the session's window keys, run once inside the compositor as its shortcut would (a Hyprland is named in the environment
/// but not there: the compositor's own socket is found first). Its exit code.
fn session_key(comp: &Compositor, bin: &Path, arg: Option<&str>) -> i32 {
    let home = comp.dir.join("home");
    let mut cmd = Command::new(bin);
    cmd.env_clear()
        .env("PATH", "/usr/bin")
        .env("HOME", &home)
        .env("XDG_RUNTIME_DIR", comp.dir.join("run"))
        .env("WAYLAND_DISPLAY", comp.wayland.file_name().unwrap())
        .env("HYPRLAND_INSTANCE_SIGNATURE", "no-hyprland-here")
        .args(arg)
        .stdin(Stdio::null());
    dies_with_test(&mut cmd).status().expect("the session key runs").code().unwrap_or(-1)
}

#[test]
fn the_session_keys_snap_and_close_windows_on_it() {
    let (Some(snap), Some(close)) = (program("YGGDRASIL_TEST_SNAP"), program("YGGDRASIL_TEST_CLOSE")) else { return };
    let _l = limit(90, "snap and close");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _layers = bar_and_dock(&mut c);
    let t = c.window("test.terminal", "Terminal");
    c.draw(&t, (640, 400), BLUE);

    // Super+Left/Right and the top and bottom halves: the room the bar and the dock leave is 0,32 1600x804
    for (side, rect) in
        [("left", (0, 32, 800, 804)), ("right", (800, 32, 800, 804)), ("top", (0, 32, 1600, 402)), ("bottom", (0, 434, 1600, 402))]
    {
        assert_eq!(session_key(&comp, &snap, Some(side)), 0, "{side}");
        assert_eq!(settle(&comp, &mut c, &t, "test.terminal", BLUE).rect(), rect, "{side}");
    }
    // Super+Up: maximized into that room, again: back
    assert_eq!(session_key(&comp, &snap, Some("up")), 0);
    let info = settle(&comp, &mut c, &t, "test.terminal", BLUE);
    assert!(info.has("maximized") && info.rect() == (0, 32, 1600, 804), "{info:?}");
    assert_eq!(session_key(&comp, &snap, Some("up")), 0);
    assert!(!settle(&comp, &mut c, &t, "test.terminal", BLUE).has("maximized"));
    // Super+Down: minimized
    assert_eq!(session_key(&comp, &snap, Some("down")), 0);
    comp.wait(10, "the terminal minimized", |c| c.window("test.terminal").is_some_and(|w| w.has("minimized")));
    assert_eq!(session_key(&comp, &snap, Some("sideways")), 2, "a wrong side is refused");

    // Super+Q: a program that keeps running (keep-running) is minimized; any other is asked to close
    let conf = comp.dir.join("home/.config/yggdrasil");
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(conf.join("keep-running"), "# music keeps playing\ntest.player\n").unwrap();
    let p = c.window("test.player", "Player");
    c.draw(&p, (500, 300), BLUE);
    assert_eq!(session_key(&comp, &close, None), 0);
    comp.wait(10, "the player minimized", |c| c.window("test.player").is_some_and(|w| w.has("minimized")));
    c.roundtrip();
    assert!(!c.state.wins[p.index].close_asked, "the player keeps running");
    let o = c.window("test.other", "Other");
    c.draw(&o, (500, 300), BLUE);
    assert_eq!(session_key(&comp, &close, None), 0);
    c.until("the other window asked to close", |s| s.wins[o.index].close_asked);
    assert!(!c.state.wins[t.index].close_asked && !c.state.wins[p.index].close_asked);
}

#[test]
fn the_panels_desktop_buttons_and_window_zones_work_on_it() {
    let Some(panel) = program("YGGDRASIL_TEST_PANEL") else { return };
    let _l = limit(150, "the panel's desktops and zones");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let bus = Bus::start(&comp.dir);
    // one layout on the zones sheet (halves), so where its zones are drawn is known
    let conf = comp.dir.join("home/.config/yggdrasil");
    std::fs::create_dir_all(&conf).unwrap();
    std::fs::write(conf.join("zones.conf"), "enabled = yes\nlayouts = halves\ngap = 8\n").unwrap();
    // the zones' trail (zones.log) is kept where the folder exists
    std::fs::create_dir_all(comp.dir.join("home/.local/state/yggdrasil")).unwrap();
    let mut p = App::start(&comp, &panel, Some(&bus), &[("YGGDRASIL_A11Y", "off"), ("XDG_CURRENT_DESKTOP", "Yggdrasil")]);
    wait_for(&comp, &mut p, 60, "the bar and the dock", |c| {
        let l = c.layers();
        ["yggdrasil-panel", "yggdrasil-dock"].iter().all(|ns| layer(&l, ns).is_some_and(|x| x.size.0 > 0))
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while !bus.has("org.yggdrasil.Panel") {
        assert!(Instant::now() < deadline, "the panel's bus name: {}", p.log());
        std::thread::sleep(Duration::from_millis(100));
    }
    let room = free_room(&comp, "HEADLESS-1");
    let bar_h = layer(&comp.layers(), "yggdrasil-panel").unwrap().size.1;
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    let t = c.window("test.terminal", "Terminal");
    c.draw(&t, (640, 400), BLUE);

    // the bar's desktops (scale 1): the start button at 8 (30 wide), then the desktops' pill at 44, its slots 22 wide from 48,
    // and "+" after the last. One desktop: "+" is at 70..92. A click makes desktop 2 and shows it.
    let y = bar_h / 2;
    comp.must(&format!("pointer 81 {y}"));
    comp.must("click");
    comp.wait(10, "desktop 2 shown from the bar's +", |c| c.must("workspace") == ["2"]);
    // the bar reads the desktops again within two seconds: 1 and 2, the second shown; the first slot shows desktop 1
    std::thread::sleep(Duration::from_millis(2600));
    comp.must(&format!("pointer 59 {y}"));
    comp.must("click");
    comp.wait(10, "desktop 1 shown from its button", |c| c.must("workspace") == ["1"]);
    comp.wait(10, "the terminal has the keyboard again", |c| c.focus() == "window\ttest.terminal\tTerminal");

    // Super+Shift+Z (the panel's ZonesPick): the sheet opens for the window in front on its screen
    bus.call(&["call", "org.yggdrasil.Panel", "/Panel", "org.yggdrasil.Panel", "ZonesPick"]).expect("ZonesPick");
    wait_for(&comp, &mut p, 10, "the zones sheet", |c| layer(&c.layers(), "yggdrasil-popup").is_some_and(|l| l.size.0 > 0));
    let sheet = layer(&comp.layers(), "yggdrasil-popup").cloned().unwrap();
    let log = std::fs::read_to_string(comp.dir.join("home/.local/state/yggdrasil/zones.log")).unwrap_or_default();
    assert!(log.contains("sheet open (Pick) for ") && log.contains(" on HEADLESS-1"), "the zones' trail: {log}");
    // the card is centred at the top of the sheet: 160 wide for one layout, its thumbnail 14 in and 42 down, the left zone 3 in
    let left_zone = (sheet.at.0 + (sheet.size.0 - 160) / 2 + 14 + 3 + 20, sheet.at.1 + 42 + 3 + 20);
    std::thread::sleep(Duration::from_millis(300));
    comp.must(&format!("pointer {} {}", left_zone.0, left_zone.1));
    comp.must("click");
    let want = yggdrasil_conf::zones::place((0.0, 0.0, 0.5, 1.0), room, 8);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = settle(&comp, &mut c, &t, "test.terminal", BLUE);
        if info.rect() == want {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the terminal in the left zone {want:?}: {info:?}\nthe zones' trail: {}\npanel: {}",
            std::fs::read_to_string(comp.dir.join("home/.local/state/yggdrasil/zones.log")).unwrap_or_default(),
            p.log()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(p.running(), "the panel is still running: {}", p.log());
}

/// Stand-ins for the programs yggdrasil-logout runs, the only ones on its PATH: each writes down how it was called.
fn logout_stand_ins(comp: &Compositor) -> (PathBuf, PathBuf) {
    let bin = comp.dir.join("stand-ins");
    std::fs::create_dir_all(&bin).unwrap();
    let log = comp.dir.join("calls");
    for t in ["systemd-cat", "notify-send", "loginctl", "systemctl"] {
        let p = bin.join(t);
        let l = log.display();
        std::fs::write(
            &p,
            format!("#!/bin/sh\nprintf '%s' '{t}' >> '{l}'\nfor a in \"$@\"; do printf ' [%s]' \"$a\" >> '{l}'; done\necho >> '{l}'\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    (bin, log)
}

fn start_logout(comp: &Compositor, bin: &Path, path: &Path, arg: &str) -> Child {
    let mut cmd = Command::new(bin);
    cmd.env_clear()
        .env("PATH", path)
        .env("HOME", comp.dir.join("home"))
        .env("XDG_RUNTIME_DIR", comp.dir.join("run"))
        .env("WAYLAND_DISPLAY", comp.wayland.file_name().unwrap())
        .env("HYPRLAND_INSTANCE_SIGNATURE", "no-hyprland-here")
        .arg(arg)
        .stdin(Stdio::null());
    dies_with_test(&mut cmd).spawn().expect("yggdrasil-logout starts")
}

#[test]
fn logout_closes_every_window_then_ends_the_session() {
    let Some(logout) = program("YGGDRASIL_TEST_LOGOUT") else { return };
    let _l = limit(60, "logout");
    let mut comp = Compositor::start(&["--size", "1280x720"]);
    let (path, calls) = logout_stand_ins(&comp);
    let mut c = comp.connect();
    let a = c.window("test.editor", "notes.txt - Editor");
    c.draw(&a, (400, 300), BLUE);
    let b = c.window("test.player", "Player");
    c.draw(&b, (400, 300), BLUE);
    let mut child = start_logout(&comp, &logout, &path, "poweroff");
    // each program closes when asked (as one with nothing to save does)
    let mut open = vec![a, b];
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        // (the compositor ends with the session: a closed connection is no failure here)
        let _ = c.queue.roundtrip(&mut c.state);
        open.retain(|w| {
            let asked = c.state.wins[w.index].close_asked;
            if asked {
                w.toplevel.destroy();
                w.xdg.destroy();
                w.surface.destroy();
            }
            !asked
        });
        let _ = c.conn.flush();
        if let Ok(Some(st)) = child.try_wait() {
            break st;
        }
        assert!(Instant::now() < deadline, "yggdrasil-logout ends");
        std::thread::sleep(Duration::from_millis(30));
    };
    assert_eq!(status.code(), Some(0));
    assert!(open.is_empty(), "every window was asked to close");
    assert_eq!(
        std::fs::read_to_string(&calls).unwrap(),
        "systemd-cat [-t] [yggdrasil-session] [echo] [poweroff: every window closed]\n",
        "no Hyprland portal to stop here"
    );
    assert_eq!(std::fs::read_to_string(comp.dir.join("run/yggdrasil-after-session")).unwrap(), "poweroff\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    while comp.child.try_wait().ok().flatten().is_none() {
        assert!(Instant::now() < deadline, "the compositor ends with the session");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn logout_with_a_window_that_stays_locks_instead() {
    let Some(logout) = program("YGGDRASIL_TEST_LOGOUT") else { return };
    let _l = limit(60, "logout, a window stays");
    let comp = Compositor::start(&["--size", "1280x720"]);
    let (path, calls) = logout_stand_ins(&comp);
    let mut c = comp.connect();
    let a = c.window("test.editor", "notes.txt - Editor");
    c.draw(&a, (400, 300), BLUE);
    let mut child = start_logout(&comp, &logout, &path, "logout");
    let st = child.wait().unwrap();
    assert_eq!(st.code(), Some(1));
    c.roundtrip();
    assert!(c.state.wins[a.index].close_asked, "it was asked");
    let log = std::fs::read_to_string(&calls).unwrap();
    assert_eq!(
        log,
        "systemd-cat [-t] [yggdrasil-session] [echo] [logout stopped: still open: notes.txt - Editor]\n\
         notify-send [-a] [Desktop] [-u] [critical] [-i] [dialog-warning] [Still signed in] [notes.txt - Editor did not close (it may be asking whether to save). The screen was locked instead; close it, then try again.]\n\
         loginctl [lock-session]\n"
    );
    assert!(comp.window("test.editor").is_some(), "the session goes on");
}
