//! The headless backend, end to end: the compositor is started with `--headless` in a private runtime folder (never the running
//! desktop's), and a small Wayland client in this test talks to it like a panel and a program would.
//!
//! - the outputs appear with the size, scale, name and place asked for (wl_output and xdg-output)
//! - a client maps a layer surface (a bar) and a toplevel (a window), and the compositor lists them
//! - the screenshot is not blank: the bar's red and the window's blue are where they were put
//! - a size or scale change through the control socket reaches the clients (output events, a new configure, the fractional scale)
//! - the config files are read again as soon as one changes, and an idle compositor neither draws nor wakes up meanwhile

mod common;

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use common::*;

// ------------------------------------------------------------------ the tests

#[test]
fn outputs_appear_with_the_size_and_scale_asked_for() {
    let _l = limit(60, "outputs");
    let comp = Compositor::start(&["--output", "1600x900", "--output", "1280x720@1.5"]);
    let c = comp.connect();
    let outs = &c.state.outputs;
    assert_eq!(outs.len(), 2, "two outputs: {outs:?}");
    assert_eq!(outs[0].name, "HEADLESS-1");
    assert_eq!(outs[0].mode, (1600, 900));
    assert_eq!(outs[0].scale, 1);
    assert_eq!(outs[0].logical, (1600, 900));
    assert_eq!(outs[0].position, (0, 0));
    assert_eq!(outs[1].name, "HEADLESS-2");
    assert_eq!(outs[1].mode, (1280, 720));
    assert_eq!(outs[1].scale, 2, "wl_output rounds 1.5 up");
    assert_eq!(outs[1].logical, (853, 480), "xdg-output: 1280x720 at 1.5");
    assert_eq!(outs[1].position, (1600, 0), "side by side");

    let listed = comp.ctl("outputs").unwrap();
    assert_eq!(listed, ["HEADLESS-1 1600x900 scale=1 at=0,0 logical=1600x900", "HEADLESS-2 1280x720 scale=1.5 at=1600,0 logical=853x480"]);

    // several outputs of one size
    let comp = Compositor::start(&["--outputs", "3", "--size", "640x480", "--scale", "2"]);
    let c = comp.connect();
    assert_eq!(c.state.outputs.len(), 3);
    assert!(c.state.outputs.iter().all(|o| o.mode == (640, 480) && o.scale == 2 && o.logical == (320, 240)));
    assert_eq!(c.state.outputs[2].position, (640, 0));
}

#[test]
fn a_panel_and_a_window_are_mapped_and_drawn() {
    let _l = limit(60, "panel and window");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();

    // the bar: red, 800x40 across the top
    let (bar, _ls) = c.layer(40);
    assert_eq!(c.state.layer_sizes[0], (800, 40));
    let red = c.buffer(800, 40, 0xffff0000);
    bar.attach(Some(&red), 0, 0);
    bar.damage_buffer(0, 0, 800, 40);
    bar.commit();

    // the window: blue, 200x150
    let win = c.window("org.yggdrasil.test", "Headless test");
    c.draw(&win, (200, 150), 0xff0000ff);
    c.roundtrip();

    let layers = comp.ctl("layers").unwrap();
    assert!(layers.iter().any(|l| l == "HEADLESS-1\ttest-bar\tTop\t0,0\t800x40\tnone"), "the bar is mapped: {layers:?}");
    let windows = comp.ctl("windows").unwrap();
    let win = windows
        .iter()
        .find(|l| l.starts_with("org.yggdrasil.test\tHeadless test\t"))
        .unwrap_or_else(|| panic!("the window is mapped: {windows:?}"));
    let parts: Vec<&str> = win.split('\t').collect();
    let (x, y) = pair(parts[2], ',');
    assert_eq!(pair(parts[3], 'x'), (200, 150));

    let shot = comp.dir.join("shot.png");
    comp.ctl(&format!("screenshot HEADLESS-1 {}", shot.display())).unwrap();
    let pic = Picture::load(&shot);
    assert_eq!((pic.w, pic.h), (800, 600));
    assert_eq!(pic.at(400, 20), (255, 0, 0), "the bar's red at the top");
    assert_eq!(pic.at(5, 5), (255, 0, 0));
    let (cx, cy) = ((x + 100) as usize, (y + 75) as usize);
    assert!(cy > 40 && cy < 600, "the window is below the bar: {x},{y}");
    assert_eq!(pic.at(cx, cy), (0, 0, 255), "the window's blue at its middle ({cx},{cy})");
    // the background elsewhere: neither of the two
    let bg = pic.at(790, 590);
    assert!(bg != (255, 0, 0) && bg != (0, 0, 255), "the background shows where nothing is: {bg:?}");

    // errors are answered, not ignored
    assert!(comp.ctl("screenshot HEADLESS-9 /nowhere.png").unwrap_err().contains("no output"));
    assert!(comp.ctl("frobnicate").unwrap_err().contains("unknown command"));
}

#[test]
fn a_resize_and_a_scale_change_reach_the_clients() {
    let _l = limit(60, "resize and scale");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let (_bg, _ls) = c.layer(0);
    assert_eq!(c.state.layer_sizes.last(), Some(&(800, 600)));
    assert_eq!(c.state.preferred_scale, Some(120), "fractional scale 1.0 (in 120ths)");

    comp.ctl("resize HEADLESS-1 1024x768").unwrap();
    c.until("the new size", |s| s.layer_sizes.last() == Some(&(1024, 768)));
    assert_eq!(c.state.outputs[0].mode, (1024, 768));
    assert_eq!(c.state.outputs[0].logical, (1024, 768));
    assert_eq!(comp.ctl("outputs").unwrap(), ["HEADLESS-1 1024x768 scale=1 at=0,0 logical=1024x768"]);

    comp.ctl("scale HEADLESS-1 2").unwrap();
    c.until("the new scale", |s| s.layer_sizes.last() == Some(&(512, 384)));
    c.until("the preferred scale", |s| s.preferred_scale == Some(240));
    assert_eq!(c.state.outputs[0].scale, 2);
    assert_eq!(c.state.outputs[0].logical, (512, 384));

    comp.ctl("scale HEADLESS-1 1.25").unwrap();
    c.until("a fractional scale", |s| s.preferred_scale == Some(150));
    assert!(comp.ctl("resize HEADLESS-1 huge").is_err());
    assert!(!c.state.closed);
}

#[test]
fn the_screenshot_follows_the_scale() {
    let _l = limit(60, "screenshot scale");
    // a bar 30 logical pixels high on an output at scale 2 covers 60 rows of the picture, all of its width
    let comp = Compositor::start(&["--size", "640x480", "--scale", "2"]);
    let mut c = comp.connect();
    let (bar, _ls) = c.layer(30);
    assert_eq!(c.state.layer_sizes[0], (320, 30));
    let green = c.buffer(640, 60, 0xff00ff00);
    bar.set_buffer_scale(2);
    bar.attach(Some(&green), 0, 0);
    bar.damage_buffer(0, 0, 640, 60);
    bar.commit();
    c.roundtrip();
    let shot = comp.dir.join("scaled.png");
    comp.ctl(&format!("screenshot HEADLESS-1 {}", shot.display())).unwrap();
    let pic = Picture::load(&shot);
    assert_eq!((pic.w, pic.h), (640, 480));
    assert_eq!(pic.at(639, 59), (0, 255, 0));
    assert_eq!(pic.at(0, 0), (0, 255, 0));
    assert_ne!(pic.at(320, 61), (0, 255, 0), "nothing of the bar below its 60 rows");
}

#[test]
fn quit_ends_it_and_leaves_no_socket_behind() {
    let _l = limit(60, "quit");
    let mut comp = Compositor::start(&[]);
    comp.ctl("quit").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while comp.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "it did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!comp.wayland.exists(), "the Wayland socket is removed");
    assert!(!comp.ctl.exists(), "the control socket is removed");
}

#[test]
fn no_runtime_folder_is_refused() {
    // (the desktop's own /run/user/UID is refused too: a unit test in headless.rs, so this test never starts anything there)
    let out = Command::new(env!("CARGO_BIN_EXE_yggdrasil-compositor"))
        .arg("--headless")
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("XDG_RUNTIME_DIR"));
}

#[test]
fn other_programs_see_the_workspaces_and_switch_them() {
    let _l = limit(60, "ext-workspace");
    let comp = Compositor::start(&["--output", "1280x720", "--output", "800x600"]);
    let mut c = comp.connect();
    let m = c.workspaces();
    let active = |s: &State| s.ws.iter().filter(|w| w.active).map(|w| w.name.clone()).collect::<Vec<_>>();
    let ws = &c.state.ws;
    assert_eq!(ws.iter().map(|w| w.name.as_str()).collect::<Vec<_>>(), ["1", "2", "3", "4"]);
    assert!(ws.iter().all(|w| w.can_activate && w.in_group && !w.id.is_empty() && w.coordinates.len() == 4), "{ws:?}");
    assert_eq!(active(&c.state), ["1"]);
    assert_eq!(c.state.ws_group_caps, Some(1), "a program may ask for a new workspace (create_workspace)");
    assert_eq!(c.state.ws_group_outputs, 2, "one group on both screens");
    assert!(c.state.ws_done >= 1);
    // asked through the protocol: done at the commit, not before
    let third = c.state.ws[2].handle.clone();
    third.activate();
    c.roundtrip();
    assert_eq!(comp.must("workspace"), ["1"], "nothing before the commit");
    m.commit();
    c.until("workspace 3 active", |s| active(s) == ["3"]);
    assert_eq!(comp.must("workspace"), ["3"]);
    // switched in the compositor (a shortcut, the dock): told as well
    comp.must("workspace 2");
    c.until("workspace 2 active", |s| active(s) == ["2"]);
    let names = |s: &State| s.ws.iter().map(|w| w.name.clone()).collect::<Vec<_>>();

    // a fifth workspace (YGG-9): asked for through the group, made at the commit, then shown
    c.state.ws_group.clone().unwrap().create_workspace("anything".into());
    m.commit();
    c.until("workspace 5 announced", |s| names(s).len() == 5);
    assert_eq!(names(&c.state), ["1", "2", "3", "4", "5"], "named by its number, not as asked");
    assert!(c.state.ws[4].in_group && c.state.ws[4].can_activate && !c.state.ws[4].active);
    assert_eq!(comp.must("workspaces").len(), 5);
    c.state.ws[4].handle.activate();
    m.commit();
    c.until("workspace 5 active", |s| active(s) == ["5"]);
    assert_eq!(comp.must("workspace"), ["5"]);
    // left empty: it goes again, and the program is told
    comp.must("workspace 1");
    c.until("workspace 5 removed", |s| names(s).len() == 4 && active(s) == ["1"]);
    assert_eq!(c.state.ws_removed, 1);

    // made by switching there (Super+Ctrl+7, the control socket): 5 to 7 exist while a window is on 7
    comp.must("workspace 7");
    c.until("workspaces 5 to 7", |s| names(s).len() == 7 && active(s) == ["7"]);
    let w = c.window("test.seven", "Seven");
    c.draw(&w, (300, 200), 0xff0000ff);
    comp.must("workspace 1");
    c.roundtrip();
    assert_eq!(comp.must("workspaces").len(), 7, "7 holds a window, so it and the empty ones before it stay");
    // the window gone (closed while on 7, not shown), leaving 7 empty: 7, 6 and 5 go at the next switch
    w.toplevel.destroy();
    w.xdg.destroy();
    w.surface.destroy();
    c.roundtrip();
    comp.must("workspace 2");
    c.until("back to four", |s| names(s).len() == 4 && active(s) == ["2"]);
    assert!(comp.ctl("workspace 10").is_err(), "nine at most");
}

/// The passes over the screens since the start (`frames`): one follows every wake-up that may change what is drawn.
fn passes(comp: &Compositor) -> u64 {
    comp.must("frames")[1].strip_prefix("passes ").unwrap().parse().unwrap()
}

/// Over `ms` of nothing happening: the passes over the screens, and how often the compositor's main thread woke up (its
/// voluntary context switches: each one a wait in its event loop that ended).
fn idle_for(comp: &Compositor, ms: u64) -> (u64, u64) {
    let pid = comp.child.id();
    let woke = || {
        let st = std::fs::read_to_string(format!("/proc/{pid}/task/{pid}/status")).unwrap();
        st.lines().find_map(|l| l.strip_prefix("voluntary_ctxt_switches:")).and_then(|v| v.trim().parse::<u64>().ok()).unwrap()
    };
    let before = passes(comp);
    // (it has just answered: back in its wait well before this)
    std::thread::sleep(Duration::from_millis(200));
    let woke_before = woke();
    std::thread::sleep(Duration::from_millis(ms));
    let woken = woke() - woke_before;
    (passes(comp) - before, woken)
}

/// A resting pointer hides after `pointer-timeout` (yggdrasil-idle sets one while the screen is dimmed) with the next frame. The
/// compositor sleeps until something changes, so a timer of its own makes that frame: one pass once the pointer has rested
/// its time, then none; and no timer at all while the timeout is 0 (the screen bright).
#[test]
fn a_resting_pointer_hides_with_a_frame_of_its_own() {
    let _l = limit(30, "the pointer's timeout");
    let comp = Compositor::start(&["--size", "800x600"]);
    comp.must("pointer 100 100");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(idle_for(&comp, 1200).0, 0, "no pass while the pointer rests with no timeout");
    comp.must("pointer-timeout 0.6");
    comp.must("pointer 200 200");
    // the move's own pass has come and gone; the pointer hides 0.6 s after it moved
    std::thread::sleep(Duration::from_millis(200));
    let moved = passes(&comp);
    std::thread::sleep(Duration::from_millis(800));
    let hid = passes(&comp);
    assert!(hid > moved, "a pass when the pointer hid ({moved} then {hid})");
    assert_eq!(idle_for(&comp, 1200).0, 0, "none after it hid");
}

/// The config files are watched (configs.rs, YGG-118), no longer looked at every 2 s: an idle compositor sleeps, without one
/// pass over the screens or one wake-up, until one of them changes, and the change is in force at once (2 s are allowed here,
/// for a busy machine), however it is saved: written in place, a new file renamed over it (as Settings and most editors save),
/// in folders made after the start, through a link into another folder, removed. Every other file reload_config reads wakes
/// it as well; another file in the same folders draws nothing.
#[test]
fn an_idle_compositor_sleeps_until_a_config_file_changes() {
    let _l = limit(60, "the config files' watch");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (400, 300), 0xff0000ff);
    comp.wait(10, "the window", |c| c.window("test.w").is_some());
    let mut last = passes(&comp);
    loop {
        std::thread::sleep(Duration::from_millis(150));
        let now = passes(&comp);
        if now == last {
            break;
        }
        last = now;
    }
    // (before, the look at the files every 2 s woke it, with a pass over the screens each time)
    assert_eq!(idle_for(&comp, 2500), (0, 0), "passes and wake-ups in 2.5 s with nothing going on");

    let (conf, etc) = (comp.dir.join("home/.config/yggdrasil"), comp.dir.join("etc"));
    let soon = |what: &str, done: &dyn Fn(&Compositor) -> bool| comp.wait(2, what, done);
    let bound = |comp: &Compositor, key: &str| comp.must("binds").iter().any(|l| l.split('\t').nth(1) == Some(key));
    // written in place (a script, an editor that writes over its file): shortcuts.conf's key
    std::fs::write(conf.join("shortcuts.conf"), "Super+F9 = exec test-nine\n").unwrap();
    soon("Super+F9 from shortcuts.conf", &|c| bound(c, "F9"));
    // a new file renamed over the old one, as Settings and most editors save: input.conf's key repeat
    std::fs::write(conf.join(".input.conf.new"), "repeat_rate = 30\nrepeat_delay = 500\n").unwrap();
    std::fs::rename(conf.join(".input.conf.new"), conf.join("input.conf")).unwrap();
    soon("input.conf's key repeat", &|c| c.must("input")[0] == "yggdrasil-headless-keyboard\trepeat_rate=30\trepeat_delay=500");
    // in folders made after the start (the test's own /etc was not there): the system's keyboard layout
    std::fs::create_dir_all(etc.join("X11/xorg.conf.d")).unwrap();
    std::fs::write(etc.join("X11/xorg.conf.d/00-keyboard.conf"), "Section \"InputClass\"\n    Option \"XkbLayout\" \"de\"\nEndSection\n")
        .unwrap();
    soon("the German layout from the new folders", &|c| c.must("keyboard") == ["de"]);
    // a link into a dotfiles folder: the file it leads to counts
    let dots = comp.dir.join("dots");
    std::fs::create_dir_all(&dots).unwrap();
    std::fs::write(dots.join("shortcuts.conf"), "Super+F10 = exec test-ten\n").unwrap();
    std::fs::remove_file(conf.join("shortcuts.conf")).unwrap();
    std::os::unix::fs::symlink(dots.join("shortcuts.conf"), conf.join("shortcuts.conf")).unwrap();
    soon("Super+F10 through the link", &|c| bound(c, "F10") && !bound(c, "F9"));
    std::fs::write(dots.join("shortcuts.conf"), "Super+F11 = exec test-eleven\n").unwrap();
    soon("Super+F11 written where the link leads", &|c| bound(c, "F11") && !bound(c, "F10"));
    // removed: the built-in table again
    std::fs::remove_file(conf.join("shortcuts.conf")).unwrap();
    soon("the built-in shortcuts", &|c| !bound(c, "F11"));

    // every other file reload_config reads (configs::files) wakes it to read them again, a pass over the screens after it (each
    // written empty, or as it was: nothing on the screen changes)
    for (file, text) in [
        (conf.join("workspaces.conf"), ""),
        (comp.dir.join("home/.config/gtk-3.0/settings.ini"), ""),
        (conf.join("opaque.conf"), ""),
        (conf.join("xwayland.conf"), ""),
        (etc.join("vconsole.conf"), ""),
        (conf.join("tiling"), ""),
        (conf.join("display.conf"), ""),
        (conf.join("monitors.lua"), ""),
        (conf.join("appearance.conf"), ""),
        (conf.join("panel.conf"), "reduce_motion = yes\n"),
    ] {
        let before = passes(&comp);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, text).unwrap();
        soon(&format!("{} read again", file.display()), &|c| passes(c) > before);
    }
    // another file in the same folders: read from the watch and dropped
    let before = passes(&comp);
    std::fs::write(conf.join("weather.json"), "{}").unwrap();
    std::fs::write(etc.join("locale.conf"), "LANG=C.UTF-8\n").unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(passes(&comp), before, "another file in the same folders draws nothing");
    assert_eq!(idle_for(&comp, 2500), (0, 0), "passes and wake-ups in 2.5 s with nothing going on, after all that");
}

/// SIGTERM, SIGINT and SIGHUP end it cleanly, as on the hardware backend: its loop stops, and its Wayland socket, the socket's
/// lock and the control socket are removed (YGG-10: a `wayland-N` file was left after SIGTERM).
#[test]
fn a_signal_ends_it_and_leaves_no_socket_behind() {
    let _l = limit(60, "the end signals");
    for sig in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
        let mut comp = Compositor::start(&["--size", "320x240"]);
        let run = comp.wayland.parent().unwrap().to_path_buf();
        let ours = |run: &std::path::Path| -> Vec<String> {
            std::fs::read_dir(run)
                .unwrap()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.starts_with("wayland-") || n.ends_with(".ctl"))
                .collect()
        };
        assert_eq!(ours(&run).len(), 3, "the socket, its lock and the control socket: {:?}", ours(&run));
        // SAFETY: a signal to the child this test started
        unsafe { libc::kill(comp.child.id() as i32, sig) };
        let status = comp.child.wait().unwrap();
        assert!(status.success(), "signal {sig}: it ended by itself, not killed ({status:?})");
        assert!(ours(&run).is_empty(), "signal {sig}: left behind {:?}", ours(&run));
    }
}
