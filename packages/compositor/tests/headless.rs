//! The headless backend, end to end: the compositor is started with `--headless` in a private runtime folder (never the running
//! desktop's), and a small Wayland client in this test talks to it like a panel and a program would.
//!
//! - the outputs appear with the size, scale, name and place asked for (wl_output and xdg-output)
//! - a client maps a layer surface (a bar) and a toplevel (a window), and the compositor lists them
//! - the screenshot is not blank: the bar's red and the window's blue are where they were put
//! - a size or scale change through the control socket reaches the clients (output events, a new configure, the fractional scale)

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
    assert!(layers.iter().any(|l| l == "HEADLESS-1\ttest-bar\tTop\t0,0\t800x40"), "the bar is mapped: {layers:?}");
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
