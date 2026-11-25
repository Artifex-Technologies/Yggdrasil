//! The window look of Yggdrasil's Hyprland session (packages/session/hyprland/yggdrasil.lua) on the headless backend, drawn in
//! software: the 1-pixel border in the accent colour (80 % in front, faint behind), the corners rounded at 10, the shadow (18
//! wide, the third power), the gaps (8 around a maximized or tiled window, 4 + 4 between tiled ones), nothing of it in full
//! screen; and the animations (open, close, move: 800 ms on Hyprland's default curve), none with "reduce motion" nor for a window
//! moved or resized by hand (the pointer or a finger). Each is held to a snapshot (tests/snapshots/*.png) and to numbers worked
//! out from the session's values.
//!
//! The animations are looked at with the compositor's clock stopped and moved on by hand (the control socket's `clock`).

mod common;

use std::time::{Duration, Instant};

use common::*;
use wayland_client::protocol::wl_shm;
use wayland_client::Proxy;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;

const RED: u32 = 0xffff0000;
const GREEN: u32 = 0xff00ff00;
const BLUE: u32 = 0xff0000ff;
const YELLOW: u32 = 0xffffff00;
const RGB_BLUE: (u8, u8, u8) = (0, 0, 255);
const RGB_YELLOW: (u8, u8, u8) = (255, 255, 0);
/// the compositor's background (0.05, 0.05, 0.08), as pixman makes it
const BG: (u8, u8, u8) = (12, 12, 20);
/// Yggdrasil's purple, the accent when none is chosen
const ACCENT: (f64, f64, f64) = (124.0, 84.0, 214.0);

/// The shadow's darkness `d` pixels out from the border (0.27 x (1 - d/18)^3), for a pixel's middle.
fn shadow(d: f64) -> f64 {
    if d >= 18.0 {
        0.0
    } else {
        (0x44 as f64 / 255.0) * (1.0 - d / 18.0).powi(3)
    }
}

/// `over` at `a` over `under`.
fn blend(over: (f64, f64, f64), a: f64, under: (f64, f64, f64)) -> (f64, f64, f64) {
    (over.0 * a + under.0 * (1.0 - a), over.1 * a + under.1 * (1.0 - a), over.2 * a + under.2 * (1.0 - a))
}

fn f((r, g, b): (u8, u8, u8)) -> (f64, f64, f64) {
    (r as f64, g as f64, b as f64)
}

fn blend_u8(over: (u8, u8, u8), a: f64, under: (u8, u8, u8)) -> (u8, u8, u8) {
    let (r, g, b) = blend(f(over), a, f(under));
    (r.round() as u8, g.round() as u8, b.round() as u8)
}

/// A pixel within `by` of a colour worked out.
fn near(have: (u8, u8, u8), want: (f64, f64, f64), by: f64) -> bool {
    let d = |a: u8, b: f64| (a as f64 - b).abs();
    d(have.0, want.0) <= by && d(have.1, want.1) <= by && d(have.2, want.2) <= by
}

/// Draw a window again at the size the compositor last gave it until the compositor lists it at that size.
fn settle(comp: &Compositor, c: &mut Client, w: &Window, app_id: &str, argb: u32) -> WinInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        c.roundtrip();
        let given = c.state.wins[w.index].size;
        let now = comp.window(app_id).map(|i| i.size).unwrap_or((100, 100));
        c.draw(w, now, argb);
        c.roundtrip();
        let info = comp.window(app_id).unwrap();
        if given == (0, 0) || info.size == given {
            return info;
        }
        assert!(Instant::now() < deadline, "{app_id} never reached its size: {info:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A window's number for the control socket's desktop commands.
fn client_id(comp: &Compositor, app: &str) -> String {
    comp.must("clients").iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap().split('\t').next().unwrap().to_string()
}

#[test]
fn border_corners_shadow_and_the_accent() {
    let _l = limit(60, "the window look");
    let comp = Compositor::start(&["--size", "640x400"]);
    let mut c = comp.connect();
    let b = c.window("test.behind", "Behind");
    c.draw(&b, (300, 160), YELLOW);
    comp.must(&format!("place {} 40 40 300 160", client_id(&comp, "test.behind")));
    settle(&comp, &mut c, &b, "test.behind", YELLOW);
    let a = c.window("test.front", "Front");
    c.draw(&a, (260, 180), BLUE);
    let (x, y, w, h) = comp.window("test.front").unwrap().rect();
    assert_eq!((x, y, w, h), (190, 110, 260, 180), "centred");
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "look-dark");

    // the picture fills its rectangle but for the corners, which are cut round
    assert!(pic.fills((x, y, w, h), RGB_BLUE, 10));
    assert_ne!(pic.at(x as usize, y as usize), RGB_BLUE, "the corner pixel is cut");
    assert_ne!(pic.at((x + 1) as usize, (y + 1) as usize), RGB_BLUE);
    assert_eq!(pic.at((x + 3) as usize, (y + 3) as usize), RGB_BLUE, "inside the curve");
    assert_ne!(pic.at((x + w - 1) as usize, (y + h - 1) as usize), RGB_BLUE);
    // the border, one pixel outside: the accent at 80 % (the shadow begins outside it); where nothing is behind
    let under = f(BG);
    let border = blend(ACCENT, 0.8, under);
    for (px, py) in [(x + w * 3 / 4, y - 1), (x + w / 2, y + h), (x - 1, y + h * 3 / 4), (x + w, y + h / 2)] {
        let p = pic.at(px as usize, py as usize);
        assert!(near(p, border, 3.0), "border at {px},{py}: {p:?}, want {border:?}");
    }
    // the shadow fading out over 18 pixels beyond the border (`d` pixels out, measured at the pixel's middle)
    for d in [1, 4, 9, 14] {
        let want = blend((0.0, 0.0, 0.0), shadow(d as f64 - 0.5), f(BG));
        let p = pic.at((x + w + d) as usize, (y + h / 2) as usize);
        assert!(near(p, want, 2.0), "shadow {d} out: {p:?}, want {want:?}");
    }
    assert_eq!(pic.at((x + w + 1 + 18) as usize, (y + h / 2) as usize), BG, "nothing past the shadow");
    // the window behind: a faint border (white at 8 %), and the front one's shadow over it
    let (bx, by, bw, _) = comp.window("test.behind").unwrap().rect();
    let faint = blend((255.0, 255.0, 255.0), 0x14 as f64 / 255.0, under);
    let p = pic.at((bx + bw / 4) as usize, (by - 1) as usize);
    assert!(near(p, faint, 3.0), "the faint border: {p:?}, want {faint:?}");
    let shaded = blend((0.0, 0.0, 0.0), shadow(4.5), f(RGB_YELLOW));
    let p = pic.at((x - 1 - 5) as usize, (y + 20) as usize);
    assert!(near(p, shaded, 3.0), "the front window's shadow on the one behind: {p:?}, want {shaded:?}");

    // another accent and the light style (Settings > Appearance): read again on the next look at the files
    std::fs::write(comp.dir.join("home/.config/yggdrasil/appearance.conf"), "style = light\naccent = 200,40,40\n").unwrap();
    comp.must("reload");
    c.draw(&a, (260, 180), BLUE);
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "look-light-accent");
    let border = blend((200.0, 40.0, 40.0), 0.8, under);
    let p = pic.at((x + w * 3 / 4) as usize, (y - 1) as usize);
    assert!(near(p, border, 3.0), "the new accent: {p:?}, want {border:?}");
    let faint = blend((0.0, 0.0, 0.0), 0x18 as f64 / 255.0, under);
    let p = pic.at((bx + bw / 4) as usize, (by - 1) as usize);
    assert!(near(p, faint, 3.0), "the light style's faint border: {p:?}, want {faint:?}");
}

#[test]
fn gaps_around_maximized_and_tiled_windows_and_nothing_in_full_screen() {
    let _l = limit(60, "gaps");
    let comp = Compositor::start_with(&["--size", "640x400"], |dir| {
        std::fs::write(dir.join("home/.config/yggdrasil/workspaces.conf"), "2 = tiling\n").unwrap();
    });
    let mut c = comp.connect();
    let bar = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 24), 24, KeyboardInteractivity::None);
    c.draw_layer(&bar, 0xff303040);
    let m = c.window("test.max", "Max");
    c.draw(&m, (200, 100), BLUE);
    // maximized: the free room (0,24 640x376) less the outer gap (8) and the border (1) all round
    comp.must("key super+Up");
    let info = settle(&comp, &mut c, &m, "test.max", BLUE);
    assert_eq!(info.rect(), (9, 33, 622, 358), "{info:?}");
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "look-maximized");
    assert!(pic.fills(info.rect(), RGB_BLUE, 10));
    let p = pic.at(4, 200);
    assert!(near(p, f(blend_u8((0, 0, 0), shadow(3.5), BG)), 1.0), "the gap shows the shadow over the background: {p:?}");
    // full screen: the whole output, square corners, no border or shadow
    comp.must("key super+f");
    let info = settle(&comp, &mut c, &m, "test.max", BLUE);
    assert_eq!(info.rect(), (0, 0, 640, 400));
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 640, 400), RGB_BLUE), 640 * 400, "every pixel, corners too");
    comp.must("key super+f");
    settle(&comp, &mut c, &m, "test.max", BLUE);

    // a tiling desktop: two windows 8 from the edges and 4 + 4 apart, each inside its border
    comp.must("key super+ctrl+2");
    let t1 = c.window("test.t1", "T1");
    c.draw(&t1, (100, 100), BLUE);
    settle(&comp, &mut c, &t1, "test.t1", BLUE);
    let t2 = c.window("test.t2", "T2");
    c.draw(&t2, (100, 100), YELLOW);
    let i2 = settle(&comp, &mut c, &t2, "test.t2", YELLOW);
    let i1 = settle(&comp, &mut c, &t1, "test.t1", BLUE);
    assert_eq!(i1.rect(), (9, 33, 306, 358), "{i1:?}");
    assert_eq!(i2.rect(), (325, 33, 306, 358), "{i2:?}");
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "look-tiled");
    assert!(pic.fills(i2.rect(), RGB_YELLOW, 10));
    // the one in front (the newer) casts its shadow over the other's right side, as on Hyprland
    assert!(pic.fills((9, 33, 306 - 18, 358), RGB_BLUE, 10));
    assert_eq!(pic.at(314, 200), blend_u8((0, 0, 0), shadow(9.5), RGB_BLUE));
    // between them: border, 8 pixels of gap (shadows over the background), border
    for gx in 315..325 {
        let p = pic.at(gx, 200);
        assert!(p != RGB_BLUE && p != RGB_YELLOW, "the gap at {gx}: {p:?}");
    }
}

/// A full screen window is drawn exactly as the program made it, pixel for pixel: that is when its buffer may go to the screen
/// directly on the hardware backend (look::Change), so the composited frame and the direct one show the same. Four quarters of
/// different colours show a picture moved, stretched, rounded or cut by a pixel. Nothing is drawn again while it does not
/// change.
#[test]
fn a_full_screen_window_is_drawn_as_the_program_made_it_and_then_rests() {
    let _l = limit(60, "full screen as made");
    let comp = Compositor::start(&["--size", "640x400"]);
    let mut c = comp.connect();
    let w = c.window("test.game", "Game");
    c.draw(&w, (200, 100), BLUE);
    w.toplevel.set_fullscreen(None);
    c.until("the full screen size", |s| s.wins[w.index].size == (640, 400));
    let quarters = [((0, 0, 320, 200), RED), ((320, 0, 320, 200), GREEN), ((0, 200, 320, 200), BLUE), ((320, 200, 320, 200), YELLOW)];
    let (pool, file) = c.pool(640 * 400 * 4);
    let mut data = vec![0u8; 640 * 400 * 4];
    for ((x, y, qw, qh), argb) in quarters {
        for row in y..y + qh {
            for col in x..x + qw {
                let at = ((row * 640 + col) * 4) as usize;
                data[at..at + 4].copy_from_slice(&argb.to_le_bytes());
            }
        }
    }
    use std::io::Write;
    (&file).write_all(&data).unwrap();
    let buffer = pool.create_buffer(0, 640, 400, 640 * 4, wl_shm::Format::Argb8888, &c.qh, ());
    w.surface.attach(Some(&buffer), 0, 0);
    w.surface.damage_buffer(0, 0, 640, 400);
    w.surface.commit();
    c.roundtrip();
    comp.wait(10, "the window in full screen", |comp| {
        comp.window("test.game").is_some_and(|i| i.has("fullscreen") && i.rect() == (0, 0, 640, 400))
    });
    let pic = comp.screenshot("HEADLESS-1");
    for ((x, y, qw, qh), argb) in quarters {
        let [b, g, r, _] = argb.to_le_bytes();
        assert_eq!(pic.count((x, y, qw, qh), (r, g, b)), (qw * qh) as usize, "the quarter at {x},{y}, its corner pixels too");
    }
    // at rest: no pass over the screen in a second
    let passes = || -> u64 { comp.must("frames")[1].strip_prefix("passes ").unwrap().parse().unwrap() };
    std::thread::sleep(Duration::from_millis(300));
    let before = passes();
    std::thread::sleep(Duration::from_millis(1000));
    assert_eq!(passes(), before, "nothing drawn while the full screen window does not change");
}

/// A compositor with the animations on (no "reduce motion") and its clock stopped.
fn animated(args: &[&str]) -> Compositor {
    let comp = Compositor::start_with(args, |dir| {
        std::fs::remove_file(dir.join("home/.config/yggdrasil/panel.conf")).unwrap();
    });
    comp.must("clock stop");
    comp
}

#[test]
fn a_window_opens_moves_and_closes_with_the_sessions_animations() {
    let _l = limit(60, "animations");
    let comp = animated(&["--size", "640x400"]);
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (240, 160), BLUE);
    let r = comp.window("test.w").unwrap().rect();
    assert_eq!(r, (200, 120, 240, 160));
    // opening: nothing yet, then growing from its middle and fading in, then all of it
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 640, 400), BG), 640 * 400, "at its start a window is not seen");
    comp.must("clock +40");
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "open-40ms");
    assert!(pic.count((0, 0, 640, 400), BG) < 640 * 400, "it shows");
    assert_eq!(pic.at(201, 200), BG, "not yet at its edges");
    assert_ne!(pic.at(320, 200), BG, "but at its middle");
    assert_eq!(pic.count((0, 0, 640, 400), RGB_BLUE), 0, "still see-through");
    comp.must("clock +800");
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.fills(r, RGB_BLUE, 10), "all of it once the 800 ms are over");

    // moving (maximize): it slides and grows from where it was
    comp.must("key super+Up");
    c.roundtrip();
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.fills(r, RGB_BLUE, 10), "it starts where it was");
    comp.must("clock +60");
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "maximize-60ms");
    assert_eq!(pic.at(320, 200), RGB_BLUE);
    assert_ne!(pic.at(15, 200), RGB_BLUE, "not yet at the left edge");
    assert_eq!(pic.at(150, 200), RGB_BLUE, "but on its way there");
    assert_eq!(pic.at(500, 300), RGB_BLUE, "its picture stretched over all of it while the program draws the new size");
    let info = settle(&comp, &mut c, &w, "test.w", BLUE);
    assert_eq!(info.rect(), (9, 9, 622, 382));
    comp.must("clock +800");
    assert!(comp.screenshot("HEADLESS-1").fills(info.rect(), RGB_BLUE, 10));

    // closing: its last picture shrinks to its middle and fades
    w.toplevel.destroy();
    w.xdg.destroy();
    w.surface.destroy();
    c.roundtrip();
    let deadline = Instant::now() + Duration::from_secs(5);
    while comp.window("test.w").is_some() {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(20));
    }
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.fills(info.rect(), RGB_BLUE, 10), "at the start of its closing it is all there");
    comp.must("clock +50");
    let pic = comp.screenshot("HEADLESS-1");
    snapshot!(pic, "close-50ms");
    assert_eq!(pic.at(20, 200), BG, "smaller");
    assert_ne!(pic.at(320, 200), BG, "around its middle");
    assert_eq!(pic.count((0, 0, 640, 400), RGB_BLUE), 0, "fading");
    comp.must("clock +800");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 640, 400), BG), 640 * 400, "gone");
    assert_eq!(comp.must("clock")[0].split(' ').next(), Some("stopped"));
}

#[test]
fn reduce_motion_draws_every_window_at_once() {
    let _l = limit(60, "reduce motion");
    // the tests' compositors start with "reduce motion" on (panel.conf): nothing moves however the clock stands
    let comp = Compositor::start(&["--size", "640x400"]);
    comp.must("clock stop");
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (240, 160), BLUE);
    assert!(comp.screenshot("HEADLESS-1").fills((200, 120, 240, 160), RGB_BLUE, 10), "there at once");
    // switched off in Settings (panel.conf): the next window animates
    std::fs::write(comp.dir.join("home/.config/yggdrasil/panel.conf"), "reduce_motion = no\n").unwrap();
    comp.must("reload");
    let v = c.window("test.v", "V");
    c.draw(&v, (100, 60), YELLOW);
    assert_eq!(comp.screenshot("HEADLESS-1").count((0, 0, 640, 400), RGB_YELLOW), 0, "the new one opens slowly");
}

/// A window moved or resized by hand (its program's own move and resize, with the button held or a finger down on it) is drawn
/// where the pointer or the finger puts it at every frame, however the clock stands: no animation then, as on Hyprland.
#[test]
fn a_window_moved_or_resized_by_hand_is_drawn_under_the_hand_at_once() {
    let _l = limit(60, "moved by hand");
    let comp = animated(&["--size", "1000x600"]);
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    let w = c.window("test.w", "W");
    c.draw(&w, (300, 200), BLUE);
    comp.must(&format!("place {} 50 100 300 200", client_id(&comp, "test.w")));
    c.roundtrip();
    // its opening starts with the first frame drawn (one now), and is over 800 ms later
    let _ = comp.screenshot("HEADLESS-1");
    comp.must("clock +800");
    assert!(comp.screenshot("HEADLESS-1").fills((50, 100, 300, 200), RGB_BLUE, 10), "opened where it was put");

    // its program's own move with the button held on it: under the pointer at once, and the place it left bare
    comp.must("pointer 150 200");
    c.until("the pointer on the window", |st| st.pointer_on == Some(w.surface.id()));
    c.state.button_serial = 0;
    comp.must("press");
    c.until("the press", |st| st.button_serial != 0);
    w.toplevel._move(&c.seat, c.state.button_serial);
    c.roundtrip();
    comp.must("pointer 250 200");
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.fills((150, 100, 300, 200), RGB_BLUE, 10), "under the pointer at once");
    assert_eq!(pic.count((60, 120, 50, 160), BG), 50 * 160, "gone from where it was");
    comp.must("release");

    // the same with a finger (the first touch gives the seat its touch), and it stays there when the finger lifts
    comp.must("touch-down 0 960 560");
    comp.must("touch-up 0");
    c.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = c.touch();
    let finger_on = |c: &mut Client, (x, y): (i32, i32)| {
        c.state.touch_serial = 0;
        comp.must(&format!("touch-down 1 {x} {y}"));
        c.until("the finger down", |st| st.touch_serial != 0);
        c.state.touch_serial
    };
    let serial = finger_on(&mut c, (250, 200));
    w.toplevel._move(&c.seat, serial);
    c.roundtrip();
    comp.must("touch-move 1 350 200");
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.fills((250, 100, 300, 200), RGB_BLUE, 10), "under the finger at once");
    assert_eq!(pic.count((160, 120, 50, 160), BG), 50 * 160, "gone from where it was");
    comp.must("touch-up 1");
    assert!(comp.screenshot("HEADLESS-1").fills((250, 100, 300, 200), RGB_BLUE, 10), "and there when the finger lifts");

    // resized with a finger from its bottom right corner: its picture stretched at once to the size the finger gives it
    let serial = finger_on(&mut c, (530, 280));
    w.toplevel.resize(&c.seat, serial, ResizeEdge::BottomRight);
    c.roundtrip();
    comp.must("touch-move 1 590 320");
    c.until("asked to take the size the finger gives it", |st| st.wins[w.index].size == (360, 240));
    // (less its outermost pixel all round, which the stretch blends with what is beside it)
    assert!(comp.screenshot("HEADLESS-1").fills((251, 101, 358, 238), RGB_BLUE, 10), "that size at once");
    comp.must("touch-up 1");
}

/// CPU time (every thread's, in microseconds) and memory (resident and proportional, kB) of a process.
fn cost(pid: u32) -> (u64, u64, u64) {
    let mut ns = 0u64;
    for t in std::fs::read_dir(format!("/proc/{pid}/task")).unwrap().flatten() {
        let s = std::fs::read_to_string(t.path().join("schedstat")).unwrap_or_default();
        ns += s.split_whitespace().next().and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
    }
    let kb = |file: &str, key: &str| {
        std::fs::read_to_string(format!("/proc/{pid}/{file}"))
            .unwrap()
            .lines()
            .find(|l| l.starts_with(key))
            .and_then(|l| l.split_whitespace().nth(1).map(|v| v.parse::<u64>().unwrap()))
            .unwrap_or(0)
    };
    (ns / 1000, kb("status", "VmRSS:"), kb("smaps_rollup", "Pss:"))
}

/// The compositor's cost with a bar, a dock and three windows: idle for 10 s, then the window in front drawn 60 times, a window's
/// picture taken 10 times (the dock's previews), then
/// (YGGDRASIL_MEASURE_ANIMATIONS=1: with the animations on) a window opening and closing. Not part of the normal run (it only
/// measures; best with a release build in YGGDRASIL_COMPOSITOR): `cargo test --test look -- --ignored --nocapture idle_cost`.
#[test]
#[ignore]
fn idle_cost() {
    let _l = limit(90, "idle cost");
    let animations = std::env::var_os("YGGDRASIL_MEASURE_ANIMATIONS").is_some();
    let comp = Compositor::start_with(&["--size", "1920x1080"], |dir| {
        if animations {
            std::fs::remove_file(dir.join("home/.config/yggdrasil/panel.conf")).unwrap();
        }
    });
    let mut c = comp.connect();
    let bar = c.layer_with(0, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 28), 28, KeyboardInteractivity::None);
    c.draw_layer(&bar, 0xff202030);
    let dock = c.layer_with(0, Layer::Top, "test-dock", Anchor::Bottom, (640, 70), 70, KeyboardInteractivity::None);
    c.draw_layer(&dock, 0xff303040);
    let mut wins = vec![];
    for (i, colour) in [BLUE, YELLOW, BLUE].into_iter().enumerate() {
        let w = c.window(&format!("test.w{i}"), "W");
        c.draw(&w, (800, 600), colour);
        wins.push(w);
    }
    std::thread::sleep(Duration::from_secs(2));
    c.roundtrip();
    let pid = comp.child.id();
    let (t0, _, _) = cost(pid);
    let start = Instant::now();
    std::thread::sleep(Duration::from_secs(10));
    let (t1, rss, pss) = cost(pid);
    let secs = start.elapsed().as_secs_f64();
    println!(
        "idle: {:.3} % of a core over {secs:.1} s ({} us), memory {rss} kB resident, {pss} kB proportional",
        (t1 - t0) as f64 / secs / 10_000.0,
        t1 - t0
    );
    // the window in front drawn again 60 times, a frame each: the compositor's time per frame
    let size = comp.window("test.w2").unwrap().size;
    let pictures = [c.buffer(size.0, size.1, YELLOW), c.buffer(size.0, size.1, BLUE)];
    let (t2, _, _) = cost(pid);
    for i in 0..60 {
        wins[2].surface.attach(Some(&pictures[i % 2]), 0, 0);
        wins[2].surface.damage_buffer(0, 0, size.0, size.1);
        wins[2].surface.commit();
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(20));
    }
    c.roundtrip();
    std::thread::sleep(Duration::from_millis(100));
    let (t3, rss2, pss2) = cost(pid);
    println!("an 800x600 window drawn 60 times: {} us a frame, memory {rss2} kB resident, {pss2} kB proportional", (t3 - t2) / 60);
    // the dock's window list takes a picture of each window about once a second while it is open (ext-image-copy-capture, a
    // session each time, as the panel does): the compositor's time per picture
    let mut p = comp.connect();
    let _list = p.toplevel_list();
    let handle = p.listed("test.w0").expect("listed").handle;
    let (t4, _, _) = cost(pid);
    for _ in 0..10 {
        let (i, s) = p.capture_window(&handle);
        let size = p.state.caps[i].size.expect("a size");
        let f = p.begin_copy(i, &s, size, 0);
        p.end_copy(f, 5.0).expect("a picture");
        s.destroy();
    }
    let (t5, rss3, pss3) = cost(pid);
    println!("an 800x600 window's picture taken 10 times: {} us each, memory {rss3} kB resident, {pss3} kB proportional", (t5 - t4) / 10);
    if animations {
        // a window opening (800 ms of frames), then closing
        let (t4, _, _) = cost(pid);
        let w = c.window("test.new", "New");
        c.draw(&w, (800, 600), YELLOW);
        std::thread::sleep(Duration::from_millis(1200));
        let (t5, rss3, pss3) = cost(pid);
        println!("a window opening: {} us in all, memory {rss3} kB resident, {pss3} kB proportional", t5 - t4);
        w.toplevel.destroy();
        w.xdg.destroy();
        w.surface.destroy();
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(1200));
        let (t6, rss4, pss4) = cost(pid);
        println!("and closing: {} us in all, memory {rss4} kB resident, {pss4} kB proportional", t6 - t5);
        let (t7, _, _) = cost(pid);
        std::thread::sleep(Duration::from_secs(5));
        let (t8, _, _) = cost(pid);
        println!("idle again after: {} us over 5 s", t8 - t7);
    }
}

/// The magnifier (YGG-6): `zoom 2` draws the screen under the pointer twice as large about the pointer (the point under it stays
/// where it is). A layer's picture, a window's picture, its rounded clip and its border all grow together; `zoom 1` puts the
/// screen back as it was, pixel for pixel.
#[test]
fn the_magnifier_draws_the_screen_larger_about_the_pointer() {
    let _l = limit(60, "the magnifier");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    // a yellow bar across the top (a layer: no border, no rounding) and a blue window at 100,200, 200x150
    let (bar, _ls) = c.layer(40);
    let yellow = c.buffer(800, 40, YELLOW);
    bar.attach(Some(&yellow), 0, 0);
    bar.damage_buffer(0, 0, 800, 40);
    bar.commit();
    let w = c.window("test.zoom", "Zoom");
    c.draw(&w, (200, 150), BLUE);
    comp.must(&format!("place {} 100 200 200 150", client_id(&comp, "test.zoom")));
    settle(&comp, &mut c, &w, "test.zoom", BLUE);
    let (x, y, ww, wh) = comp.window("test.zoom").unwrap().rect();
    assert_eq!((x, y, ww, wh), (100, 200, 200, 150));
    let before = comp.screenshot("HEADLESS-1");
    assert_eq!(before.count((0, 0, 800, 600), RGB_YELLOW), 800 * 40);
    assert!(before.fills((100, 200, 200, 150), RGB_BLUE, 10));
    assert_eq!(comp.must("zoom"), ["1"]);

    // the pointer on the window's middle (200,275), zoom 2: the window covers 0..400 x 125..425, cut round at twice the radius
    comp.must("pointer 200 275");
    comp.must("zoom 2");
    assert_eq!(comp.must("zoom"), ["2"]);
    let pic = comp.screenshot("HEADLESS-1");
    // (its outermost row and column are smoothed by the scaling: one pixel in from the edge it is solid)
    assert!(pic.fills((1, 126, 398, 298), RGB_BLUE, 20), "the picture and its clip grew together");
    assert_eq!(pic.count((0, 0, 800, 600), RGB_BLUE), pic.count((0, 125, 400, 300), RGB_BLUE), "nothing blue outside it");
    assert_ne!(pic.at(399, 424), RGB_BLUE, "its corner is cut, twice as round");
    assert_eq!(pic.at(200, 275), RGB_BLUE, "the point under the pointer stays");
    // the bar, 0..40 high, now -275*2+275.. : off the top of the screen
    assert_eq!(pic.count((0, 0, 800, 600), RGB_YELLOW), 0);

    // the pointer near the top (400,20): the bar grows to 60 rows, across the whole width
    comp.must("pointer 400 20");
    let pic = comp.screenshot("HEADLESS-1");
    // (rows -20..60 drawn, the screen shows 0..60; the last one smoothed by the scaling)
    assert_eq!(pic.count((0, 0, 800, 59), RGB_YELLOW), 800 * 59);
    assert_eq!(pic.count((0, 60, 800, 540), RGB_YELLOW), 0);

    // off: the screen as it was
    comp.must("zoom 1");
    let after = comp.screenshot("HEADLESS-1");
    assert_eq!(after.count((0, 0, 800, 600), RGB_YELLOW), 800 * 40);
    assert_eq!(after.count((0, 0, 800, 600), RGB_BLUE), before.count((0, 0, 800, 600), RGB_BLUE));
    assert!(comp.ctl("zoom 9").is_err() && comp.ctl("zoom 0.5").is_err() && comp.ctl("zoom x").is_err());
}
