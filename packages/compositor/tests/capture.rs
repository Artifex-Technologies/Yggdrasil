//! The window list and window pictures through the standard protocols the panel's dock uses for its previews (gap item 6):
//! ext-foreign-toplevel-list, ext-image-capture-source and ext-image-copy-capture, with the tests' own small client (tests/common)
//! speaking them as packages/panel/src/capture.rs does. The panel itself takes its previews on the compositor in tests/apps.rs.

mod common;

use std::io::Write;
use std::time::Duration;

use common::*;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;

const BLUE: u32 = 0xff0000ff;
const RED: u32 = 0xffff0000;
const GREEN: u32 = 0xff00ff00;
const YELLOW: u32 = 0xffffff00;
const RGB_RED: (u8, u8, u8) = (255, 0, 0);
const RGB_GREEN: (u8, u8, u8) = (0, 255, 0);
const RGB_BLUE: (u8, u8, u8) = (0, 0, 255);
const RGB_YELLOW: (u8, u8, u8) = (255, 255, 0);

const ARGB: u32 = 0;
const XRGB: u32 = 1;
/// ext_image_copy_capture_frame_v1.failure_reason
const BUFFER_CONSTRAINTS: u32 = 1;
const STOPPED: u32 = 2;

/// A buffer whose left half is one colour and right half another.
fn halves(c: &Client, w: i32, h: i32, left: u32, right: u32) -> WlBuffer {
    let (pool, f) = c.pool(w * h * 4);
    let data: Vec<u8> = (0..w * h).flat_map(|i| if i % w < w / 2 { left.to_le_bytes() } else { right.to_le_bytes() }).collect();
    (&f).write_all(&data).unwrap();
    let b = pool.create_buffer(0, w, h, w * 4, wayland_client::protocol::wl_shm::Format::Argb8888, &c.qh, ());
    pool.destroy();
    b
}

/// The control socket's number for a window (its `clients` line).
fn client_id(comp: &Compositor, app_id: &str) -> String {
    let lines = comp.must("clients");
    let line = lines.iter().find(|l| l.split('\t').nth(1) == Some(app_id)).unwrap_or_else(|| panic!("{app_id} in {lines:?}"));
    line.split('\t').next().unwrap().to_string()
}

#[test]
fn windows_are_listed_and_copied_on_their_own() {
    let _l = limit(60, "window list and copies");
    let comp = Compositor::start(&["--size", "1280x720"]);
    let mut c = comp.connect();
    // window A: red on the left, green on the right; window B, larger, opens over it and covers it
    let a = c.window("test.a", "A");
    let buf = halves(&c, 300, 200, RED, GREEN);
    a.surface.attach(Some(&buf), 0, 0);
    a.surface.damage_buffer(0, 0, 300, 200);
    a.surface.commit();
    c.roundtrip();
    let b = c.window("test.b", "B");
    c.draw(&b, (500, 400), BLUE);
    let ia = comp.window("test.a").unwrap();
    let shot = comp.screenshot("HEADLESS-1");
    assert_eq!(shot.count((ia.at.0, ia.at.1, 300, 200), RGB_RED), 0, "A is covered on the screen: {ia:?}");

    // the list names both, with their titles and app ids, and follows a new title
    let mut p = comp.connect();
    let _list = p.toplevel_list();
    let la = p.listed("test.a").expect("A is listed");
    let lb = p.listed("test.b").expect("B is listed");
    assert_eq!((la.title.as_str(), lb.title.as_str()), ("A", "B"));
    assert!(la.done >= 1 && !la.identifier.is_empty() && la.identifier != lb.identifier, "{la:?} {lb:?}");
    a.toplevel.set_title("A, renamed".into());
    a.surface.commit();
    c.roundtrip();
    comp.must("windows");
    p.until("the new title", |s| s.listed.iter().any(|l| l.app_id == "test.a" && l.title == "A, renamed"));

    // A's own picture, covered as it is: its size, both formats, and exactly its pixels
    let (ia_cap, session) = p.capture_window(&la.handle);
    let cap = p.state.caps[ia_cap].clone();
    assert_eq!(cap.size, Some((300, 200)));
    assert!(cap.formats.contains(&ARGB) && cap.formats.contains(&XRGB), "{:?}", cap.formats);
    let f = p.begin_copy(ia_cap, &session, (300, 200), ARGB);
    let (pic, bytes) = p.end_copy(f, 5.0).expect("A's copy");
    assert_eq!(pic.count((0, 0, 150, 200), RGB_RED), 150 * 200, "the left half is red");
    assert_eq!(pic.count((150, 0, 150, 200), RGB_GREEN), 150 * 200, "the right half is green");
    assert!(bytes.chunks(4).all(|px| px[3] == 255), "opaque");
    assert_eq!(p.state.caps[ia_cap].transform, Some(0), "normal");
    assert_eq!(p.state.caps[ia_cap].damage, vec![(0, 0, 300, 200)]);
    snapshot!(pic, "capture-window");

    // the next copy waits for A to change, then shows the change
    let f = p.begin_copy(ia_cap, &session, (300, 200), XRGB);
    std::thread::sleep(Duration::from_millis(300));
    p.roundtrip();
    assert!(!p.state.caps[ia_cap].ready, "no copy before A changes");
    c.draw(&a, (300, 200), YELLOW);
    let (pic, _) = p.end_copy(f, 5.0).expect("the copy after the change");
    assert_eq!(pic.count((0, 0, 300, 200), RGB_YELLOW), 300 * 200);

    // a buffer of another size fails (the program is told the size again with every new session)
    let f = p.begin_copy(ia_cap, &session, (100, 100), ARGB);
    assert_eq!(p.end_copy(f, 5.0).err(), Some(Some(BUFFER_CONSTRAINTS)));

    // minimized, A still gives its picture (the dock's card shows it)
    comp.must(&format!("minimize {}", client_id(&comp, "test.a")));
    assert!(comp.window("test.a").unwrap().has("minimized"));
    let (i2, s2) = p.capture_window(&la.handle);
    let f = p.begin_copy(i2, &s2, (300, 200), ARGB);
    let (pic, _) = p.end_copy(f, 5.0).expect("a minimized window's copy");
    assert_eq!(pic.count((0, 0, 300, 200), RGB_YELLOW), 300 * 200);

    // B closes: the list says so, and its session stops
    let (ib, sb) = p.capture_window(&lb.handle);
    b.toplevel.destroy();
    b.xdg.destroy();
    b.surface.destroy();
    c.roundtrip();
    comp.must("windows");
    p.until("B closed in the list", |s| s.listed.iter().any(|l| l.app_id == "test.b" && l.closed));
    p.until("B's session stopped", |s| s.caps[ib].stopped);
    let f = p.begin_copy(ib, &sb, (500, 400), ARGB);
    assert_eq!(p.end_copy(f, 5.0).err(), Some(Some(STOPPED)));
}

#[test]
fn a_window_is_copied_at_its_screens_scale_and_never_while_locked() {
    let _l = limit(60, "copies at scale 2, locked");
    let comp = Compositor::start(&["--output", "1280x720@2"]);
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (200, 100), BLUE);
    let mut p = comp.connect();
    let _list = p.toplevel_list();
    let lw = p.listed("test.w").unwrap_or_else(|| panic!("{:?} {:?}", p.state.listed, comp.windows()));
    let (i, s) = p.capture_window(&lw.handle);
    assert_eq!(p.state.caps[i].size, Some((400, 200)), "the window's size in the screen's pixels");
    let f = p.begin_copy(i, &s, (400, 200), XRGB);
    let (pic, _) = p.end_copy(f, 5.0).expect("the copy");
    // (a picture drawn at scale 1 is stretched to 2 with a smooth filter: its outermost pixels are blended)
    assert!(pic.count((0, 0, 400, 200), RGB_BLUE) >= 400 * 200 * 97 / 100, "blue");
    assert_eq!(pic.count((2, 2, 396, 196), RGB_BLUE), 396 * 196, "blue inside its edge");

    // the window grows: the next copy fails with the new size told, and one of that size works
    let b = c.buffer(300, 150, GREEN);
    w.surface.attach(Some(&b), 0, 0);
    w.surface.damage_buffer(0, 0, 300, 150);
    w.surface.commit();
    c.roundtrip();
    let f = p.begin_copy(i, &s, (400, 200), XRGB);
    assert_eq!(p.end_copy(f, 5.0).err(), Some(Some(BUFFER_CONSTRAINTS)));
    assert_eq!(p.state.caps[i].size, Some((600, 300)));
    let f = p.begin_copy(i, &s, (600, 300), XRGB);
    let (pic, _) = p.end_copy(f, 5.0).expect("the copy at the new size");
    assert_eq!(pic.count((2, 2, 596, 296), RGB_GREEN), 596 * 296);

    // while the session is locked no window is copied
    let mut l = comp.connect();
    let mgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).expect("ext-session-lock");
    let lock = mgr.lock(&l.qh, ());
    let ls = l.compositor.create_surface(&l.qh, ());
    let _lsurf = lock.get_lock_surface(&ls, &l.wl_outputs[0].clone(), &l.qh, 0);
    l.until("locked", |s| s.locked);
    c.draw(&w, (300, 150), YELLOW);
    let f = p.begin_copy(i, &s, (600, 300), XRGB);
    assert!(p.end_copy(f, 5.0).is_err(), "no copy while locked");
    // a screen's copy shows the lock (black here: the locker drew nothing), not the window
    let (io, so) = p.capture_output(0, false);
    assert_eq!(p.state.caps[io].size, Some((1280, 720)));
    let f = p.begin_copy(io, &so, (1280, 720), XRGB);
    let (pic, _) = p.end_copy(f, 5.0).expect("the screen's copy");
    assert_eq!(pic.count((0, 0, 1280, 720), RGB_YELLOW), 0, "nothing of the session shows");
}

#[test]
fn a_screen_is_copied_as_the_screenshot_shows_it() {
    let _l = limit(60, "a screen's copy");
    let comp = Compositor::start(&["--outputs", "2", "--size", "640x480"]);
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (200, 100), BLUE);
    let mut p = comp.connect();
    let (i, s) = p.capture_output(0, false);
    assert_eq!(p.state.caps[i].size, Some((640, 480)));
    let f = p.begin_copy(i, &s, (640, 480), XRGB);
    let (pic, _) = p.end_copy(f, 5.0).expect("the copy");
    let shot = comp.screenshot("HEADLESS-1");
    assert!(pic.rgb == shot.rgb, "the copy is the screenshot");
    assert!(pic.count((0, 0, 640, 480), RGB_BLUE) > 0);
    // the second screen goes: its session stops
    let (i2, _s2) = p.capture_output(1, false);
    assert_eq!(p.state.caps[i2].size, Some((640, 480)));
    comp.must("remove-output HEADLESS-2");
    p.until("the second screen's session stopped", |s| s.caps[i2].stopped);
}
