//! The window look of Yggdrasil's Hyprland session (packages/session/hyprland/yggdrasil.lua) on the headless backend, drawn in
//! software: the 1-pixel border in the accent colour (80 % in front, faint behind), the corners rounded at 10, the shadow (18
//! wide, the third power), the gaps (8 around a maximized or tiled window, 4 + 4 between tiled ones), nothing of it in full
//! screen; and the animations (open, close, move: 800 ms on Hyprland's default curve), none with "reduce motion". Each is held to
//! a snapshot (tests/snapshots/*.png) and to numbers worked out from the session's values.
//!
//! The animations are looked at with the compositor's clock stopped and moved on by hand (the control socket's `clock`).

mod common;

use std::time::{Duration, Instant};

use common::*;

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
