//! The pointer the compositor draws (gap item 7): the cursor theme as `ygg-theme cursor` stores and sets it (appearance.conf, and
//! the running compositor through yggdrasil-wm), its sizes at a screen's scale, a program's own cursor surface and the shapes
//! programs ask for by name (cursor-shape-v1), hidden while the screen is dimmed (yggdrasil-idle's pointer timeout through
//! yggdrasil-wm), and moved onto a screen that stays when its screen goes. The pointer is drawn over a screen's picture as a
//! hardware cursor plane is: `screenshot NAME FILE pointer`, grim -c and the copies that ask for it show it; plain copies do not.
//!
//! The themes are the test's own (small X cursor files written here, one colour per shape and size); Yggdrasil's own theme, as
//! yggdrasil-theme's ygg-cursors draws it, is drawn when YGGDRASIL_TEST_CURSORS names a folder holding it (tests/apps-podman.sh
//! and CI's compositor-headless job make one).

mod common;

use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use common::*;
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::Shape, wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use yggdrasil_wm::Desk;

const BLUE: u32 = 0xff0000ff;
const RGB_RED: (u8, u8, u8) = (255, 0, 0);
const RGB_GREEN: (u8, u8, u8) = (0, 255, 0);
const RGB_MAGENTA: (u8, u8, u8) = (255, 0, 255);
const RGB_CYAN: (u8, u8, u8) = (0, 255, 255);
const RGB_YELLOW: (u8, u8, u8) = (255, 255, 0);

/// An X cursor file: one square picture per (nominal size, colour), its hotspot an eighth of the way in.
fn xcursor(images: &[(u32, u32)]) -> Vec<u8> {
    let mut out = vec![];
    let put = |v: &mut Vec<u8>, x: u32| v.extend_from_slice(&x.to_le_bytes());
    let n = images.len() as u32;
    out.extend_from_slice(b"Xcur");
    put(&mut out, 16);
    put(&mut out, 0x1_0000);
    put(&mut out, n);
    let mut pos = 16 + n * 12;
    for (size, _) in images {
        put(&mut out, 0xfffd_0002);
        put(&mut out, *size);
        put(&mut out, pos);
        pos += 36 + size * size * 4;
    }
    for (size, argb) in images {
        for v in [36, 0xfffd_0002, *size, 1, *size, *size, size / 8, size / 8, 0] {
            put(&mut out, v);
        }
        for _ in 0..size * size {
            put(&mut out, *argb);
        }
    }
    out
}

/// A cursor theme of the test's own in home/.local/share/icons: each shape's pictures (nominal size, colour).
fn theme(dir: &Path, name: &str, shapes: &[(&str, &[(u32, u32)])]) {
    let d = dir.join("home/.local/share/icons").join(name).join("cursors");
    std::fs::create_dir_all(&d).unwrap();
    std::fs::write(d.parent().unwrap().join("index.theme"), format!("[Icon Theme]\nName={name}\n")).unwrap();
    for (shape, images) in shapes {
        std::fs::write(d.join(shape), xcursor(images)).unwrap();
    }
}

/// Two themes: TestPointer (the default arrow red at 24 and green at 48, the text beam cyan) and TestPointer2 (yellow).
fn themes(dir: &Path) {
    theme(dir, "TestPointer", &[("default", &[(24, 0xffff0000), (48, 0xff00ff00)]), ("text", &[(24, 0xff00ffff), (48, 0xff00ffff)])]);
    theme(dir, "TestPointer2", &[("default", &[(24, 0xffffff00), (32, 0xffffff00)])]);
}

fn desk(comp: &Compositor) -> Desk {
    let run = comp.dir.join("run").into_os_string();
    let display: OsString = comp.wayland.file_name().unwrap().into();
    let d = Desk::from_env(|k| match k {
        "XDG_RUNTIME_DIR" => Some(run.clone()),
        "WAYLAND_DISPLAY" => Some(display.clone()),
        _ => None,
    })
    .expect("the compositor is found");
    assert!(matches!(d, Desk::Own(_)), "{d:?}");
    d
}

/// The screen with the pointer painted over it.
fn shot_with_pointer(comp: &Compositor, output: &str) -> Picture {
    let path = comp.dir.join(format!("pointer-{output}.png"));
    comp.must(&format!("screenshot {output} {} pointer", path.display()));
    Picture::load(&path)
}

/// Where a colour's pixels are: (left, top, width, height), or None when there are none.
fn found(pic: &Picture, colour: (u8, u8, u8)) -> Option<(i32, i32, i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, -1, -1);
    for y in 0..pic.h {
        for x in 0..pic.w {
            if pic.at(x, y) == colour {
                (x0, y0, x1, y1) = (x0.min(x as i32), y0.min(y as i32), x1.max(x as i32), y1.max(y as i32));
            }
        }
    }
    (x1 >= 0).then(|| (x0, y0, x1 - x0 + 1, y1 - y0 + 1))
}

#[test]
fn the_pointer_is_drawn_with_the_theme_ygg_theme_chose() {
    let _l = limit(60, "the pointer's theme");
    // appearance.conf as ygg-theme writes it, and the session's size
    let comp = Compositor::start_env(&["--output", "640x480", "--output", "640x480@2"], &[("XCURSOR_SIZE", "24")], |dir| {
        themes(dir);
        std::fs::write(dir.join("home/.config/yggdrasil/appearance.conf"), "style = dark\naccent = 124,84,214\ncursor = TestPointer\n")
            .unwrap();
    });
    comp.must("pointer 300 200");
    assert_eq!(comp.must("pointer-look"), ["theme\tTestPointer\t24\tdefault"]);
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    assert_eq!(found(&pic, RGB_RED), Some((297, 197, 24, 24)), "the theme's 24 picture, its hotspot (3, 3) on the pointer");
    snapshot!(pic, "pointer-theme");
    // the screen's own picture (and every copy that does not ask for the pointer) has none
    assert_eq!(found(&comp.screenshot("HEADLESS-1"), RGB_RED), None);

    // on the screen at scale 2 the theme's 48 picture, at twice the place
    comp.must("pointer 900 100");
    let pic = shot_with_pointer(&comp, "HEADLESS-2");
    assert_eq!(found(&pic, RGB_GREEN), Some((514, 194, 48, 48)), "(900 - 640) * 2 - 6, 100 * 2 - 6");
    assert_eq!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_RED), None, "not on the other screen");

    // `ygg-theme cursor` on the running compositor (yggdrasil-wm's set_cursor): another theme and size, at once
    assert!(desk(&comp).set_cursor("TestPointer2", 32));
    assert_eq!(comp.must("pointer-look"), ["theme\tTestPointer2\t32\tdefault"]);
    comp.must("pointer 100 100");
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    assert_eq!(found(&pic, RGB_YELLOW), Some((96, 96, 32, 32)), "its 32 picture");
    // a theme that is not installed: the plain arrow
    assert!(desk(&comp).set_cursor("NoSuchTheme", 24));
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    let arrow = found(&pic, (255, 255, 255)).expect("a white arrow");
    let outline = found(&pic, (0, 0, 0)).expect("outlined in black");
    assert!((100..=102).contains(&outline.0) && (100..=102).contains(&outline.1), "its tip on the pointer: {outline:?}");
    assert!(arrow.0 > outline.0 && arrow.2 < outline.2 && (20..=26).contains(&outline.3), "white inside, 24 tall: {arrow:?} {outline:?}");

    // grim -c paints the pointer in (wlr-screencopy's overlay_cursor), plain grim does not
    let grim = Path::new("/usr/bin/grim");
    if !grim.exists() {
        assert!(std::env::var_os("YGGDRASIL_TEST_APPS_REQUIRED").is_none(), "grim is not installed");
        return;
    }
    for (args, with) in [(&["-c"][..], true), (&[][..], false)] {
        let file = comp.dir.join(format!("grim-{with}.png"));
        let mut cmd = std::process::Command::new(grim);
        cmd.args(args).args(["-o", "HEADLESS-1"]).arg(&file).env_clear();
        cmd.env("XDG_RUNTIME_DIR", comp.dir.join("run")).env("WAYLAND_DISPLAY", comp.wayland.file_name().unwrap());
        let out = dies_with_test(&mut cmd).output().expect("grim runs");
        assert!(out.status.success(), "grim: {}", String::from_utf8_lossy(&out.stderr));
        let g = Picture::load(&file);
        let want = if with { shot_with_pointer(&comp, "HEADLESS-1") } else { comp.screenshot("HEADLESS-1") };
        assert!(g.rgb == want.rgb, "grim {args:?}: the screen {} the pointer", if with { "with" } else { "without" });
    }
}

#[test]
fn a_programs_own_pointer_and_named_shapes_are_drawn() {
    let _l = limit(60, "a program's pointer");
    let comp = Compositor::start_with(&["--size", "640x480"], |dir| {
        themes(dir);
        std::fs::write(dir.join("home/.config/yggdrasil/appearance.conf"), "cursor = TestPointer\n").unwrap();
    });
    let mut c = comp.connect();
    let (_k, pointer) = c.input();
    let w = c.window("test.w", "W");
    c.draw(&w, (300, 200), BLUE);
    let info = comp.window("test.w").unwrap();
    let (px, py) = (info.at.0 + 150, info.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    c.until("the pointer on the window", |s| s.pointer_on == Some(wayland_client::Proxy::id(&w.surface)));

    // its own picture: 16 x 16 magenta, hotspot (2, 3)
    let cs = c.compositor.create_surface(&c.qh, ());
    let b = c.buffer(16, 16, 0xffff00ff);
    cs.attach(Some(&b), 0, 0);
    cs.damage_buffer(0, 0, 16, 16);
    pointer.set_cursor(c.state.pointer_serial, Some(&cs), 2, 3);
    cs.commit();
    c.roundtrip();
    assert_eq!(comp.must("pointer-look"), ["surface\t16x16\t2,3"]);
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    assert_eq!(found(&pic, RGB_MAGENTA), Some((px - 2, py - 3, 16, 16)));
    snapshot!(pic, "pointer-program");

    // a shape by name (cursor-shape-v1): the theme's text beam
    let shapes: WpCursorShapeManagerV1 = c.globals.bind(&c.qh, 1..=1, ()).expect("cursor-shape");
    let device = shapes.get_pointer(&pointer, &c.qh, ());
    device.set_shape(c.state.pointer_serial, Shape::Text);
    c.roundtrip();
    assert_eq!(comp.must("pointer-look"), ["theme\tTestPointer\t24\ttext"]);
    assert_eq!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_CYAN), Some((px - 3, py - 3, 24, 24)));

    // the program hides it
    pointer.set_cursor(c.state.pointer_serial, None, 0, 0);
    c.roundtrip();
    assert_eq!(comp.must("pointer-look"), ["hidden\tprogram"]);
    assert!(shot_with_pointer(&comp, "HEADLESS-1").rgb == comp.screenshot("HEADLESS-1").rgb, "nothing drawn");

    // off the window, over nothing: the default arrow
    comp.must("pointer 20 20");
    assert_eq!(comp.must("pointer-look"), ["theme\tTestPointer\t24\tdefault"]);
    assert_eq!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_RED), Some((17, 17, 24, 24)));

    // a screen's copy asked with paint_cursors (ext-image-copy-capture) has the pointer, one without does not
    let mut p = comp.connect();
    for cursors in [true, false] {
        let (i, s) = p.capture_output(0, cursors);
        let f = p.begin_copy(i, &s, (640, 480), 1);
        let (pic, _) = p.end_copy(f, 5.0).expect("the copy");
        assert_eq!(found(&pic, RGB_RED).is_some(), cursors, "paint_cursors {cursors}");
    }
}

#[test]
fn the_pointer_hides_while_the_screen_is_dimmed() {
    let _l = limit(60, "the pointer while dimmed");
    let comp = Compositor::start_with(&["--size", "640x480"], |dir| {
        themes(dir);
        std::fs::write(dir.join("home/.config/yggdrasil/appearance.conf"), "cursor = TestPointer\n").unwrap();
    });
    let d = desk(&comp);
    comp.must("pointer 200 200");
    assert!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_RED).is_some());
    // yggdrasil-idle dims the screen: it sets the pointer timeout to 0.25 s through yggdrasil-wm, as on Hyprland
    d.set_pointer_timeout(0.25);
    assert_eq!(d.pointer_timeout(), Some(0.25));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(comp.must("pointer-look"), ["hidden\tresting"]);
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    assert_eq!(found(&pic, RGB_RED), None, "hidden while it rests");
    // moving it shows it at once, and it hides again after resting
    comp.must("pointer 220 210");
    assert_eq!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_RED), Some((217, 207, 24, 24)));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_RED), None);
    // the screen wakes: yggdrasil-idle puts the timeout back to 0 and the pointer shows where it was, unmoved
    d.set_pointer_timeout(0.0);
    assert_eq!(found(&shot_with_pointer(&comp, "HEADLESS-1"), RGB_RED), Some((217, 207, 24, 24)));
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(comp.must("pointer-look"), ["theme\tTestPointer\t24\tdefault"], "0: never hidden");
}

#[test]
fn the_pointer_moves_onto_a_screen_that_stays() {
    let _l = limit(60, "the pointer when its screen goes");
    let comp = Compositor::start_with(&["--outputs", "2", "--size", "640x480"], |dir| {
        themes(dir);
        std::fs::write(dir.join("home/.config/yggdrasil/appearance.conf"), "cursor = TestPointer\n").unwrap();
    });
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (200, 100), BLUE);
    // on the second screen, which is unplugged: to the middle of the first
    comp.must("pointer 1000 300");
    comp.must("remove-output HEADLESS-2");
    assert_eq!(comp.must("cursor"), ["320 240"]);
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    assert_eq!(found(&pic, RGB_RED), Some((317, 237, 24, 24)));
    snapshot!(pic, "pointer-screen-gone");
    // a screen that gets smaller keeps the pointer, inside it
    comp.must("pointer 600 450");
    comp.must("resize HEADLESS-1 480x360");
    assert_eq!(comp.must("cursor"), ["479 359"]);
    // a screen plugged in leaves it where it is
    comp.must("add-output 640x480");
    assert_eq!(comp.must("cursor"), ["479 359"]);
}

#[test]
fn yggdrasils_own_pointer_is_drawn() {
    let Some(dir) = std::env::var_os("YGGDRASIL_TEST_CURSORS") else {
        assert!(std::env::var_os("YGGDRASIL_TEST_APPS_REQUIRED").is_none(), "YGGDRASIL_TEST_CURSORS is not set");
        eprintln!("skipped: YGGDRASIL_TEST_CURSORS is not set (a folder with Yggdrasil's cursor theme, from ygg-cursors)");
        return;
    };
    let theme = Path::new(&dir).join("Yggdrasil");
    assert!(theme.join("cursors/default").is_file(), "{}: no Yggdrasil/cursors/default", theme.display());
    let _l = limit(60, "Yggdrasil's pointer");
    // no appearance.conf choice: Yggdrasil's own theme, at the session's size
    let comp = Compositor::start_env(&["--size", "640x480"], &[("XCURSOR_SIZE", "24")], |d| {
        let icons = d.join("home/.local/share/icons");
        std::fs::create_dir_all(&icons).unwrap();
        std::os::unix::fs::symlink(&theme, icons.join("Yggdrasil")).unwrap();
    });
    comp.must("pointer 100 100");
    assert_eq!(comp.must("pointer-look"), ["theme\tYggdrasil\t24\tdefault"]);
    let pic = shot_with_pointer(&comp, "HEADLESS-1");
    let plain = comp.screenshot("HEADLESS-1");
    let changed: Vec<(usize, usize)> =
        (0..480).flat_map(|y| (0..640).map(move |x| (x, y))).filter(|&(x, y)| pic.at(x, y) != plain.at(x, y)).collect();
    assert!(changed.len() > 100, "the pointer is drawn: {} pixels", changed.len());
    assert!(changed.iter().all(|&(x, y)| (90..130).contains(&x) && (90..130).contains(&y)), "around the pointer only");
    assert!(changed.iter().any(|&(x, y)| pic.at(x, y) == (255, 255, 255)), "white inside");
    snapshot!(pic, "pointer-yggdrasil");
}
