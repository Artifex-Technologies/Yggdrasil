//! What Yggdrasil relies on from its compositor (on Hyprland today: packages/session/hyprland/yggdrasil.lua), checked on the
//! headless backend with the tests' own small clients (tests/common): the bar and the dock as layer-shell surfaces with their
//! reserved room, windows floating and centred in the free room and kept clear of the bar and the dock, maximize, snapping and
//! full screen, workspaces, the keyboard's focus, the lock covering every output, screen copies, screens off and on, and idle.
//!
//! Every test starts a compositor of its own in a private folder and has a time limit. Keys and clicks are made inside the
//! compositor through its control socket (no input device).

mod common;

use std::os::unix::fs::FileExt;
use std::time::{Duration, Instant};

use common::*;
use wayland_client::Proxy;
use wayland_protocols::ext::idle_notify::v1::client::ext_idle_notifier_v1::ExtIdleNotifierV1;
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1, zwlr_output_power_v1::Mode,
};
use wayland_protocols_wlr::screencopy::v1::client::zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1;

const RED: u32 = 0xffff0000;
const GREEN: u32 = 0xff00ff00;
const BLUE: u32 = 0xff0000ff;
const YELLOW: u32 = 0xffffff00;
const RGB_RED: (u8, u8, u8) = (255, 0, 0);
const RGB_GREEN: (u8, u8, u8) = (0, 255, 0);
const RGB_BLUE: (u8, u8, u8) = (0, 0, 255);
const RGB_YELLOW: (u8, u8, u8) = (255, 255, 0);

// xdg_toplevel states
const MAXIMIZED: u32 = 1;
const FULLSCREEN: u32 = 2;
const TILED_LEFT: u32 = 5;

// kernel key codes, as wl_keyboard reports them
const KEY_L: u32 = 38;
const KEY_S: u32 = 31;
const KEY_H: u32 = 35;
const KEY_I: u32 = 23;

/// Modifier keys: they reach the program with the keyboard, as on Hyprland (a shortcut takes only the key that completes it).
const MODIFIERS: [u32; 8] = [29, 42, 54, 56, 97, 100, 125, 126];

/// The keys a client got, without the modifiers.
fn typed(keys: &[(wayland_client::backend::ObjectId, u32)]) -> Vec<(wayland_client::backend::ObjectId, u32)> {
    keys.iter().filter(|(_, k)| !MODIFIERS.contains(k)).cloned().collect()
}

const BAR_H: u32 = 32;
const DOCK: (u32, u32) = (600, 64);

/// A bar (red, Top layer, across the top, reserving its height) and a dock (green, Top layer, centred at the bottom, reserving its
/// height), as the panel makes them.
fn bar_and_dock(c: &mut Client) -> (LayerSurf, LayerSurf) {
    let bar = c.layer_with(
        0,
        Layer::Top,
        "test-bar",
        Anchor::Top | Anchor::Left | Anchor::Right,
        (0, BAR_H),
        BAR_H as i32,
        KeyboardInteractivity::None,
    );
    c.draw_layer(&bar, RED);
    let dock = c.layer_with(0, Layer::Top, "test-dock", Anchor::Bottom, DOCK, DOCK.1 as i32, KeyboardInteractivity::None);
    c.draw_layer(&dock, GREEN);
    (bar, dock)
}

/// Draw a window again at the size the compositor last gave it (a program answers each configure with a commit), until the
/// compositor lists it at that size.
fn settle(comp: &Compositor, c: &mut Client, w: &Window, app_id: &str, argb: u32) -> WinInfo {
    let deadline = Instant::now() + Duration::from_secs(10);
    c.roundtrip();
    let now = comp.window(app_id).map(|i| i.size).unwrap_or((100, 100));
    c.draw(w, now, argb);
    loop {
        c.roundtrip();
        let given = c.state.wins[w.index].size;
        let info = comp.window(app_id).unwrap_or_else(|| panic!("{app_id} is listed"));
        if given.0 > 0 && given.1 > 0 && info.size != given {
            c.draw(w, given, argb);
        } else if Instant::now() > deadline || given.0 <= 0 || info.size == given {
            return info;
        }
        assert!(Instant::now() < deadline, "{app_id} did not settle at {given:?}: {info:?}");
    }
}

#[test]
fn the_bar_and_dock_reserve_their_room_and_windows_open_centred_in_the_rest() {
    let _l = limit(60, "layers and placement");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _ = bar_and_dock(&mut c);
    let layers = comp.layers();
    let bar = layers.iter().find(|l| l.namespace == "test-bar").unwrap();
    assert_eq!((bar.layer.as_str(), bar.at, bar.size), ("Top", (0, 0), (1600, 32)), "the bar across the top");
    let dock = layers.iter().find(|l| l.namespace == "test-dock").unwrap();
    assert_eq!((dock.at, dock.size), ((500, 836), (600, 64)), "the dock centred at the bottom");

    // a pop-up that keeps out of the reserved room (exclusive zone 0, as the panel's menus) starts under the bar
    let pop =
        c.layer_with(0, Layer::Overlay, "test-popup", Anchor::Top | Anchor::Left | Anchor::Right, (0, 300), 0, KeyboardInteractivity::None);
    assert_eq!(c.state.layer_configures[&pop.index].last(), Some(&(1600, 300)));
    c.draw_layer(&pop, YELLOW);
    let popup = comp.layers().into_iter().find(|l| l.namespace == "test-popup").unwrap();
    assert_eq!(popup.at, (0, 32), "under the bar, not over it");
    pop.layer.destroy();
    pop.surface.destroy();
    c.roundtrip();

    // a window opens floating and centred in the free room (0,32 1600x804), as Hyprland's "float, center" and keep-clear do
    let a = c.window("test.a", "A");
    assert_eq!(c.state.wins[a.index].size, (0, 0), "a floating window chooses its own size");
    c.draw(&a, (400, 300), BLUE);
    let info = comp.window("test.a").unwrap();
    assert_eq!(info.rect(), (600, 284, 400, 300), "centred in the free room: {info:?}");
    assert!(info.has("floating") && info.has("active"), "{info:?}");

    // one that asks for more than the free room is shortened to it
    let b = c.window("test.b", "B");
    c.draw(&b, (1600, 900), YELLOW);
    assert_eq!(c.state.wins[b.index].size, (1600, 804), "told the free room's size");
    let info = settle(&comp, &mut c, &b, "test.b", YELLOW);
    assert_eq!(info.rect(), (0, 32, 1600, 804), "{info:?}");

    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.at(800, 10), RGB_RED, "the bar over everything at the top");
    assert_eq!(pic.at(800, 870), RGB_GREEN, "the dock at the bottom");
    assert_eq!(pic.at(800, 450), RGB_YELLOW, "the window in front");
    assert_eq!(pic.count((0, 32, 1600, 804), RGB_YELLOW), 1600 * 804, "the window fills the free room exactly");
}

#[test]
fn windows_keep_clear_of_a_dock_that_appears_and_come_back_from_maximize_at_a_normal_size() {
    let _l = limit(60, "keep clear");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let bar = c.layer_with(
        0,
        Layer::Top,
        "test-bar",
        Anchor::Top | Anchor::Left | Anchor::Right,
        (0, BAR_H),
        BAR_H as i32,
        KeyboardInteractivity::None,
    );
    c.draw_layer(&bar, RED);
    // free room 0,32 1600x868: an 800x840 window opens at 400,46 and reaches 886, under where the dock will be
    let w = c.window("test.tall", "Tall");
    c.draw(&w, (800, 840), BLUE);
    assert_eq!(comp.window("test.tall").unwrap().rect(), (400, 46, 800, 840));
    // and one that asks for all of the screen gets the free room
    let big = c.window("test.big", "Big");
    c.draw(&big, (1600, 900), YELLOW);
    assert_eq!(settle(&comp, &mut c, &big, "test.big", YELLOW).rect(), (0, 32, 1600, 868));
    let dock = c.layer_with(0, Layer::Top, "test-dock", Anchor::Bottom, DOCK, DOCK.1 as i32, KeyboardInteractivity::None);
    c.draw_layer(&dock, GREEN);
    // the dock's room is 836..900: the window is moved up and shortened to the 804 rows left (keep-clear.lua's fit)
    let info = settle(&comp, &mut c, &w, "test.tall", BLUE);
    assert_eq!(info.rect(), (400, 32, 800, 804), "{info:?}");
    let info = settle(&comp, &mut c, &big, "test.big", YELLOW);
    assert_eq!(info.rect(), (0, 32, 1600, 804), "{info:?}");

    // maximize (the program asks), then back: a window as big as the free room comes back at three quarters of it, centred
    big.toplevel.set_maximized();
    c.roundtrip();
    assert!(c.state.wins[big.index].states.contains(&MAXIMIZED));
    let info = settle(&comp, &mut c, &big, "test.big", YELLOW);
    assert_eq!(info.rect(), (0, 32, 1600, 804), "maximized to the free room: {info:?}");
    assert!(info.has("maximized"));
    big.toplevel.unset_maximized();
    c.roundtrip();
    assert!(!c.state.wins[big.index].states.contains(&MAXIMIZED));
    let info = settle(&comp, &mut c, &big, "test.big", YELLOW);
    assert_eq!(info.rect(), (200, 133, 1200, 603), "a normal size, centred (keep-clear.lua's normal): {info:?}");

    // a small window comes back where it was
    let s = c.window("test.small", "Small");
    c.draw(&s, (400, 300), YELLOW);
    let before = comp.window("test.small").unwrap().rect();
    s.toplevel.set_maximized();
    c.roundtrip();
    settle(&comp, &mut c, &s, "test.small", YELLOW);
    s.toplevel.unset_maximized();
    c.roundtrip();
    let info = settle(&comp, &mut c, &s, "test.small", YELLOW);
    assert_eq!(info.rect(), before);
}

#[test]
fn shortcuts_maximize_snap_restore_and_minimize() {
    let _l = limit(60, "shortcuts");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _ = bar_and_dock(&mut c);
    let w = c.window("test.w", "W");
    c.draw(&w, (400, 300), BLUE);
    assert_eq!(comp.focus(), "window\ttest.w\tW");

    // Super+Up maximizes to the free room, again restores (the session's yggdrasil-snap up)
    comp.must("key super+Up");
    c.roundtrip();
    assert!(c.state.wins[w.index].states.contains(&MAXIMIZED));
    assert_eq!(settle(&comp, &mut c, &w, "test.w", BLUE).rect(), (0, 32, 1600, 804));
    comp.must("key super+Up");
    c.roundtrip();
    assert_eq!(settle(&comp, &mut c, &w, "test.w", BLUE).rect(), (600, 284, 400, 300));

    // Super+Left: the left half of the free room; Super+Down restores, again minimizes
    comp.must("key super+Left");
    c.roundtrip();
    assert!(c.state.wins[w.index].states.contains(&TILED_LEFT));
    let info = settle(&comp, &mut c, &w, "test.w", BLUE);
    assert_eq!(info.rect(), (0, 32, 800, 804));
    assert!(info.has("left"));
    comp.must("key super+Down");
    c.roundtrip();
    assert_eq!(settle(&comp, &mut c, &w, "test.w", BLUE).rect(), (600, 284, 400, 300));
    comp.must("key super+Down");
    let info = comp.window("test.w").unwrap();
    assert!(info.has("minimized"), "{info:?}");
    assert_eq!(comp.focus(), "none");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((600, 284, 400, 300), RGB_BLUE), 0, "a minimized window is not drawn");
}

#[test]
fn a_full_screen_window_covers_the_bar_and_the_dock() {
    let _l = limit(60, "full screen");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let _ = bar_and_dock(&mut c);
    let w = c.window("test.game", "Game");
    c.draw(&w, (640, 480), BLUE);
    w.toplevel.set_fullscreen(None);
    c.roundtrip();
    assert!(c.state.wins[w.index].states.contains(&FULLSCREEN));
    assert_eq!(c.state.wins[w.index].size, (1600, 900));
    let info = settle(&comp, &mut c, &w, "test.game", BLUE);
    assert_eq!(info.rect(), (0, 0, 1600, 900));
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 1600, 900), RGB_BLUE), 1600 * 900, "nothing of the bar or the dock over a full screen window");
    w.toplevel.unset_fullscreen();
    c.roundtrip();
    let info = settle(&comp, &mut c, &w, "test.game", BLUE);
    assert_eq!(info.rect(), (480, 194, 640, 480), "back where it was");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.at(800, 10), RGB_RED, "the bar is back in front");
}

#[test]
fn workspaces_switch_and_take_windows_along() {
    let _l = limit(60, "workspaces");
    let comp = Compositor::start(&["--size", "1280x720"]);
    let mut c = comp.connect();
    let a = c.window("test.a", "A");
    c.draw(&a, (400, 300), BLUE);
    assert_eq!(comp.must("workspace"), ["1"]);

    // Super+Ctrl+2: the second desktop, empty; A stays on the first
    comp.must("key super+ctrl+2");
    assert_eq!(comp.must("workspace"), ["2"]);
    let info = comp.window("test.a").unwrap();
    assert_eq!(info.workspace, 1, "{info:?}");
    assert_eq!(comp.focus(), "none", "nothing to type into on an empty desktop");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 1280, 720), RGB_BLUE), 0, "A is not shown on the second desktop");

    // a window opened here belongs here; Super+Ctrl+Shift+1 sends it to the first
    let b = c.window("test.b", "B");
    c.draw(&b, (300, 200), YELLOW);
    assert_eq!(comp.window("test.b").unwrap().workspace, 2);
    assert_eq!(comp.focus(), "window\ttest.b\tB");
    comp.must("key super+ctrl+shift+1");
    assert_eq!(comp.window("test.b").unwrap().workspace, 1);
    assert_eq!(comp.focus(), "none");

    // Super+Ctrl+Left goes back to the first: both there, the keyboard on the one used last
    comp.must("key super+ctrl+Left");
    assert_eq!(comp.must("workspace"), ["1"]);
    let pic = comp.screenshot("HEADLESS-1");
    assert!(pic.count((0, 0, 1280, 720), RGB_BLUE) > 0 && pic.count((0, 0, 1280, 720), RGB_YELLOW) > 0, "A and B both shown");
    assert!(comp.focus().starts_with("window\t"), "a window has the keyboard: {}", comp.focus());
    assert!(comp.windows().iter().all(|w| w.workspace == 1));
}

#[test]
fn the_keyboard_follows_clicks_new_windows_and_an_exclusive_panel() {
    let _l = limit(60, "keyboard focus");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    assert!(c.state.keymap, "a keymap is sent");
    let a = c.window("test.a", "A");
    c.draw(&a, (600, 400), BLUE);
    c.roundtrip();
    assert_eq!(c.state.kb_focus, Some(a.surface.id()), "a new window gets the keyboard");

    // the terminal stand-in: typing reaches the window with the keyboard
    comp.must("type ls");
    c.roundtrip();
    assert_eq!(c.state.keys, [(a.surface.id(), KEY_L), (a.surface.id(), KEY_S)]);

    // a second window takes it; snapped to the left half, A shows to its right
    let b = c.window("test.b", "B");
    c.draw(&b, (300, 200), YELLOW);
    c.roundtrip();
    assert_eq!(c.state.kb_focus, Some(b.surface.id()));
    comp.must("key super+Left");
    c.roundtrip();
    settle(&comp, &mut c, &b, "test.b", YELLOW);
    let ra = comp.window("test.a").unwrap().rect();
    // a click on A (right of B) gives it the keyboard and raises it
    comp.must(&format!("pointer {} {}", ra.0 + ra.2 - 20, ra.1 + 20));
    comp.must("click");
    c.roundtrip();
    assert_eq!(c.state.kb_focus, Some(a.surface.id()));
    assert_eq!(comp.focus(), "window\ttest.a\tA");
    assert!(comp.window("test.a").unwrap().has("active"));
    comp.must("type hi");
    c.roundtrip();
    assert_eq!(&typed(&c.state.keys)[2..], [(a.surface.id(), KEY_H), (a.surface.id(), KEY_I)]);
    // and back to B by a click on it
    comp.must("pointer 20 400");
    comp.must("click");
    c.roundtrip();
    assert_eq!(c.state.kb_focus, Some(b.surface.id()));

    // a panel that asks for the keyboard exclusively (the start menu) gets it as soon as it shows...
    let menu = c.layer_with(0, Layer::Overlay, "test-menu", Anchor::Top | Anchor::Left, (300, 400), 0, KeyboardInteractivity::Exclusive);
    c.draw_layer(&menu, GREEN);
    c.roundtrip();
    assert_eq!(comp.focus(), "layer\ttest-menu\tHEADLESS-1");
    assert_eq!(c.state.kb_focus, Some(menu.surface.id()));
    comp.must("type s");
    c.roundtrip();
    assert_eq!(c.state.keys.last(), Some(&(menu.surface.id(), KEY_S)));
    // ...one that asks on demand (a notification) does not take it
    let toast = c.layer_with(0, Layer::Overlay, "test-toast", Anchor::Top | Anchor::Right, (300, 80), 0, KeyboardInteractivity::OnDemand);
    c.draw_layer(&toast, YELLOW);
    assert_eq!(comp.focus(), "layer\ttest-menu\tHEADLESS-1");
    // and when the menu closes, the keyboard goes back to the window (the session's layer.closed rule)
    menu.layer.destroy();
    menu.surface.destroy();
    c.roundtrip();
    c.roundtrip();
    assert_eq!(comp.focus(), "window\ttest.b\tB");
    assert_eq!(c.state.kb_focus, Some(b.surface.id()));
}

#[test]
fn the_lock_covers_every_output_and_takes_all_input_until_unlocked() {
    let _l = limit(60, "session lock");
    let comp = Compositor::start(&["--output", "1280x720", "--output", "1024x768@2"]);
    let mut c = comp.connect();
    let (_k, _p) = c.input();
    let w = c.window("test.secret", "Secret");
    c.draw(&w, (400, 300), BLUE);
    c.roundtrip();
    assert_eq!(c.state.kb_focus, Some(w.surface.id()));

    // the locker: a client of its own, one lock surface per output
    let mut l = comp.connect();
    let (_lk, _lp) = l.input();
    let mgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).expect("ext-session-lock");
    let lock = mgr.lock(&l.qh, ());
    let mut surfaces = vec![];
    for (i, o) in l.wl_outputs.clone().iter().enumerate() {
        let s = l.compositor.create_surface(&l.qh, ());
        let ls = lock.get_lock_surface(&s, o, &l.qh, i);
        surfaces.push((s, ls));
    }
    l.until("the lock surfaces' sizes", |s| s.lock_sizes.len() == 2);
    assert_eq!(l.state.lock_sizes[&0], (1280, 720));
    assert_eq!(l.state.lock_sizes[&1], (512, 384), "logical size at scale 2");
    l.until("locked", |s| s.locked);
    for (i, (s, _)) in surfaces.iter().enumerate() {
        let (w, h) = l.state.lock_sizes[&i];
        let scale = if i == 1 { 2 } else { 1 };
        let b = l.buffer(w as i32 * scale, h as i32 * scale, RED);
        s.set_buffer_scale(scale);
        s.attach(Some(&b), 0, 0);
        s.damage_buffer(0, 0, w as i32 * scale, h as i32 * scale);
        s.commit();
    }
    l.roundtrip();
    assert_eq!(comp.must("lock"), ["locked", "HEADLESS-1\t1280x720", "HEADLESS-2\t512x384"]);

    // every output shows the lock, nothing of the session
    let one = comp.screenshot("HEADLESS-1");
    assert_eq!(one.count((0, 0, 1280, 720), RGB_RED), 1280 * 720);
    let two = comp.screenshot("HEADLESS-2");
    assert_eq!(two.count((0, 0, 1024, 768), RGB_RED), 1024 * 768);

    // the keyboard is the lock's: keys and clicks never reach the window
    assert_eq!(comp.focus(), "lock\tHEADLESS-1");
    c.roundtrip();
    assert_ne!(c.state.kb_focus, Some(w.surface.id()), "the window lost the keyboard");
    comp.must("type hi");
    comp.must("pointer 640 360");
    comp.must("click");
    comp.must("key super+ctrl+2");
    c.roundtrip();
    l.roundtrip();
    assert!(c.state.keys.is_empty(), "no key reached the window: {:?}", c.state.keys);
    // the lock got every key, Super+Ctrl+2 too: no shortcut runs while locked (on Hyprland only binds marked "locked" do: the
    // media keys)
    assert_eq!(l.state.keys.iter().map(|(_, k)| *k).collect::<Vec<_>>(), [KEY_H, KEY_I, 125, 29, 3]);
    assert_eq!(comp.must("workspace"), ["1"], "Super+Ctrl+2 did nothing while locked");

    // a second locker is refused
    let mut l2 = comp.connect();
    let mgr2: ExtSessionLockManagerV1 = l2.globals.bind(&l2.qh, 1..=1, ()).unwrap();
    let _lock2 = mgr2.lock(&l2.qh, ());
    l2.until("the second lock refused", |s| s.lock_finished);
    assert!(!l2.state.locked);

    // unlock: the session shows again and the keyboard goes back to the window
    lock.unlock_and_destroy();
    l.roundtrip();
    assert_eq!(comp.must("lock"), ["unlocked"]);
    let one = comp.screenshot("HEADLESS-1");
    assert!(one.count((0, 0, 1280, 720), RGB_BLUE) > 0);
    assert_eq!(comp.focus(), "window\ttest.secret\tSecret");
}

#[test]
fn a_locker_that_dies_leaves_the_screen_locked() {
    let _l = limit(60, "lock crash");
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let w = c.window("test.secret", "Secret");
    c.draw(&w, (400, 300), BLUE);
    {
        let mut l = comp.connect();
        let mgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).unwrap();
        let lock = mgr.lock(&l.qh, ());
        let s = l.compositor.create_surface(&l.qh, ());
        let _ls = lock.get_lock_surface(&s, &l.wl_outputs[0], &l.qh, 0);
        l.until("locked", |s| s.locked && !s.lock_sizes.is_empty());
        // the connection ends here without unlocking (a crash)
    }
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(comp.must("lock")[0], "locked", "the session stays locked (ext-session-lock requires it)");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 800, 600), RGB_BLUE), 0, "nothing of the session shows");
    c.roundtrip();

    // a new locker may take over (yggdrasil-idle starts one again; Hyprland's allow_session_lock_restore) and unlock
    let mut l = comp.connect();
    let mgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).unwrap();
    let lock = mgr.lock(&l.qh, ());
    let s = l.compositor.create_surface(&l.qh, ());
    let _ls = lock.get_lock_surface(&s, &l.wl_outputs[0], &l.qh, 0);
    l.until("the new locker holds the lock", |s| s.locked && !s.lock_sizes.is_empty());
    let b = l.buffer(800, 600, RED);
    s.attach(Some(&b), 0, 0);
    s.commit();
    l.roundtrip();
    assert_eq!(comp.must("lock"), ["locked", "HEADLESS-1\t800x600"]);
    lock.unlock_and_destroy();
    l.roundtrip();
    assert_eq!(comp.must("lock"), ["unlocked"]);
    assert!(comp.screenshot("HEADLESS-1").count((0, 0, 800, 600), RGB_BLUE) > 0);
}

/// One screen copy through wlr-screencopy: the picture, and the copy's events.
fn copy(
    c: &mut Client,
    mgr: &ZwlrScreencopyManagerV1,
    output: usize,
    region: Option<(i32, i32, i32, i32)>,
    with_damage: bool,
) -> (Picture, CopyState) {
    let i = c.state.copies.len();
    c.state.copies.push(CopyState::default());
    let o = c.wl_outputs[output].clone();
    let frame = match region {
        None => mgr.capture_output(0, &o, &c.qh, i),
        Some((x, y, w, h)) => mgr.capture_output_region(0, &o, x, y, w, h, &c.qh, i),
    };
    c.until("the copy's buffer size", |s| s.copies[i].buffer_done || s.copies[i].failed);
    let (fmt, w, h, stride) = c.state.copies[i].buffer.expect("a buffer size");
    assert_eq!(fmt, 1, "XRGB8888");
    let (pool, file) = c.pool((stride * h) as i32);
    let buf = pool.create_buffer(0, w as i32, h as i32, stride as i32, wayland_client::protocol::wl_shm::Format::Xrgb8888, &c.qh, ());
    pool.destroy();
    if with_damage {
        frame.copy_with_damage(&buf);
    } else {
        frame.copy(&buf);
    }
    c.until("the copy", |s| s.copies[i].ready || s.copies[i].failed);
    assert!(!c.state.copies[i].failed, "the copy failed");
    let mut data = vec![0u8; (stride * h) as usize];
    file.read_exact_at(&mut data, 0).unwrap();
    frame.destroy();
    buf.destroy();
    (Picture::from_xrgb(w as usize, h as usize, stride as usize, &data), c.state.copies[i].clone())
}

#[test]
fn screen_copies_return_what_is_shown() {
    let _l = limit(90, "screencopy");
    let comp = Compositor::start(&["--output", "1280x720", "--output", "800x600@2"]);
    let mut c = comp.connect();
    let bar = c.layer_with(
        0,
        Layer::Top,
        "test-bar",
        Anchor::Top | Anchor::Left | Anchor::Right,
        (0, BAR_H),
        BAR_H as i32,
        KeyboardInteractivity::None,
    );
    c.draw_layer(&bar, RED);
    let w = c.window("test.w", "W");
    c.draw(&w, (400, 300), BLUE);
    let mgr: ZwlrScreencopyManagerV1 = c.globals.bind(&c.qh, 1..=3, ()).expect("wlr-screencopy");

    // the whole output: the same picture as the compositor's own screenshot
    let (pic, st) = copy(&mut c, &mgr, 0, None, false);
    assert_eq!((pic.w, pic.h), (1280, 720));
    assert_eq!(st.buffer, Some((1, 1280, 720, 1280 * 4)));
    let shot = comp.screenshot("HEADLESS-1");
    assert!(pic.rgb == shot.rgb, "the copy is the screen");
    assert_eq!(pic.at(10, 10), RGB_RED);

    // a part of it: the window's rectangle, all blue
    let r = comp.window("test.w").unwrap().rect();
    let (part, _) = copy(&mut c, &mgr, 0, Some(r), false);
    assert_eq!((part.w, part.h), (400, 300));
    assert_eq!(part.count((0, 0, 400, 300), RGB_BLUE), 400 * 300);

    // a part of the scale 2 output is twice as many pixels each way
    let (half, _) = copy(&mut c, &mgr, 1, Some((10, 10, 100, 50)), false);
    assert_eq!((half.w, half.h), (200, 100));

    // with damage: the first copy at once, the next only after something changed
    let (_, first) = copy(&mut c, &mgr, 0, None, true);
    assert!(!first.damage.is_empty());
    let i = c.state.copies.len();
    c.state.copies.push(CopyState::default());
    let frame = mgr.capture_output(0, &c.wl_outputs[0], &c.qh, i);
    c.until("the buffer size", |s| s.copies[i].buffer_done);
    let (pool, file) = c.pool(1280 * 720 * 4);
    let buf = pool.create_buffer(0, 1280, 720, 1280 * 4, wayland_client::protocol::wl_shm::Format::Xrgb8888, &c.qh, ());
    frame.copy_with_damage(&buf);
    for _ in 0..10 {
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!c.state.copies[i].ready, "no change, no copy yet");
    c.draw(&w, (400, 300), YELLOW);
    c.until("the copy after a change", |s| s.copies[i].ready);
    let mut data = vec![0u8; 1280 * 720 * 4];
    file.read_exact_at(&mut data, 0).unwrap();
    let after = Picture::from_xrgb(1280, 720, 1280 * 4, &data);
    assert_eq!(after.count(r, RGB_YELLOW), 400 * 300, "the change is in the copy");

    // while locked, a copy shows the lock and nothing else
    let mut l = comp.connect();
    let lmgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).unwrap();
    let lock = lmgr.lock(&l.qh, ());
    let s = l.compositor.create_surface(&l.qh, ());
    let _ls = lock.get_lock_surface(&s, &l.wl_outputs[0], &l.qh, 0);
    l.until("locked", |s| s.locked && s.lock_sizes.contains_key(&0));
    let (pic, _) = copy(&mut c, &mgr, 0, None, false);
    assert_eq!(pic.count((0, 0, 1280, 720), RGB_YELLOW), 0, "the window does not show through the lock");
    assert_eq!(pic.count((0, 0, 1280, 720), RGB_RED), 0, "nor the bar");
}

#[test]
fn a_copy_into_a_wrong_buffer_is_a_protocol_error() {
    let _l = limit(30, "screencopy error");
    let comp = Compositor::start(&["--size", "640x480"]);
    let mut c = comp.connect();
    let mgr: ZwlrScreencopyManagerV1 = c.globals.bind(&c.qh, 1..=3, ()).unwrap();
    c.state.copies.push(CopyState::default());
    let frame = mgr.capture_output(0, &c.wl_outputs[0], &c.qh, 0);
    c.until("the buffer size", |s| s.copies[0].buffer_done);
    let (pool, _f) = c.pool(100 * 100 * 4);
    let buf = pool.create_buffer(0, 100, 100, 400, wayland_client::protocol::wl_shm::Format::Xrgb8888, &c.qh, ());
    frame.copy(&buf);
    assert!(c.queue.roundtrip(&mut c.state).is_err(), "a buffer of the wrong size ends the connection");
    // the compositor goes on
    assert_eq!(comp.must("outputs").len(), 1);
}

#[test]
fn grim_takes_the_same_screenshot() {
    let _l = limit(60, "grim");
    let grim = std::path::Path::new("/usr/bin/grim");
    if !grim.exists() {
        assert!(std::env::var_os("YGGDRASIL_TEST_APPS_REQUIRED").is_none(), "grim is not installed");
        eprintln!("skipped: /usr/bin/grim is not installed");
        return;
    }
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (300, 200), BLUE);
    let file = comp.dir.join("grim.png");
    let mut cmd = std::process::Command::new(grim);
    cmd.args(["-o", "HEADLESS-1"]).arg(&file).env_clear();
    cmd.env("XDG_RUNTIME_DIR", comp.dir.join("run"))
        .env("WAYLAND_DISPLAY", comp.wayland.file_name().unwrap())
        .env("HOME", comp.dir.join("home"));
    let out = dies_with_test(&mut cmd).output().expect("grim runs");
    assert!(out.status.success(), "grim: {}", String::from_utf8_lossy(&out.stderr));
    let pic = Picture::load(&file);
    let shot = comp.screenshot("HEADLESS-1");
    assert_eq!((pic.w, pic.h), (800, 600));
    assert!(pic.rgb == shot.rgb, "grim's picture is the screen");
    assert_eq!(pic.count((250, 200, 300, 200), RGB_BLUE), 300 * 200);
}

#[test]
fn screens_go_off_and_on_through_output_power() {
    let _l = limit(30, "output power");
    let comp = Compositor::start(&["--outputs", "2", "--size", "640x480"]);
    let mut c = comp.connect();
    let w = c.window("test.w", "W");
    c.draw(&w, (200, 100), BLUE);
    let mgr: ZwlrOutputPowerManagerV1 = c.globals.bind(&c.qh, 1..=1, ()).expect("wlr-output-power");
    let p0 = mgr.get_output_power(&c.wl_outputs[0], &c.qh, 0);
    let _p1 = mgr.get_output_power(&c.wl_outputs[1], &c.qh, 1);
    c.until("the modes", |s| s.power.len() == 2);
    assert_eq!(c.state.power[&0], 1, "on at start");

    // off, as yggdrasil-idle does after the idle time: black, and listed as off
    p0.set_mode(Mode::Off);
    c.until("off", |s| s.power[&0] == 0);
    assert!(comp.must("outputs")[0].ends_with(" power=off"));
    assert!(!comp.must("outputs")[1].contains("power=off"), "only that one");
    let pic = comp.screenshot("HEADLESS-1");
    assert_eq!(pic.count((0, 0, 640, 480), (0, 0, 0)), 640 * 480);
    // on again at the first input
    p0.set_mode(Mode::On);
    c.until("on", |s| s.power[&0] == 1);
    assert!(comp.screenshot("HEADLESS-1").count((0, 0, 640, 480), RGB_BLUE) > 0);
    // the control socket switches too, and the program holding the control hears it
    comp.must("power HEADLESS-1 off");
    c.until("told off", |s| s.power[&0] == 0);
    comp.must("power HEADLESS-1 on");
    c.until("told on", |s| s.power[&0] == 1);
}

#[test]
fn idle_is_told_after_the_timeout_and_input_ends_it() {
    let _l = limit(30, "idle");
    let comp = Compositor::start(&["--size", "640x480"]);
    let mut c = comp.connect();
    let notifier: ExtIdleNotifierV1 = c.globals.bind(&c.qh, 1..=1, ()).expect("ext-idle-notify");
    let _n = notifier.get_idle_notification(300, &c.seat, &c.qh, 0);
    c.until("idle after 300 ms", |s| s.idled.get(&0) == Some(&1));
    assert_eq!(c.state.resumed.get(&0), None);
    comp.must("pointer 100 100");
    c.until("resumed at the first input", |s| s.resumed.get(&0) == Some(&1));
    comp.must("key shift");
    c.until("idle again", |s| s.idled.get(&0) == Some(&2));
}
