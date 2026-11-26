//! A full screen window and the windows over it: what is drawn in front, what a click lands on, which window has the keyboard
//! and which one the programs are told is active must always be the same window. On Hyprland a smaller window opened over a full
//! screen game sometimes stays drawn while clicks pass through it into the game, or switching back is refused; these tests hold
//! this compositor to one stacking order (src/state/stacking.rs) through opening, clicking, Alt+Tab, the control socket's
//! `activate`, minimize and restore, closing, leaving full screen, menus and dialogs, a second screen, the lock, the bar and
//! dock changing, the screen changing size, a decorated window's resize grips, and a finger on the touchscreen.
//!
//! Every check looks at all four: the screen's pixels at a probe point, the client that gets a click there, the client that
//! gets a key, and the xdg_toplevel `activated` state the windows were sent.

mod common;

use std::time::{Duration, Instant};

use common::*;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols::ext::session_lock::v1::client::ext_session_lock_manager_v1::ExtSessionLockManagerV1;
use wayland_protocols::xdg::shell::client::xdg_popup::{self, XdgPopup};
use wayland_protocols::xdg::shell::client::xdg_positioner::{Anchor as PAnchor, Gravity, XdgPositioner};

const RED: u32 = 0xffff0000;
const GREEN: u32 = 0xff00ff00;
const BLUE: u32 = 0xff0000ff;
const YELLOW: u32 = 0xffffff00;
const MAGENTA: u32 = 0xffff00ff;
const CYAN: u32 = 0xff00ffff;
const RGB_RED: (u8, u8, u8) = (255, 0, 0);
const RGB_BLUE: (u8, u8, u8) = (0, 0, 255);
const RGB_YELLOW: (u8, u8, u8) = (255, 255, 0);
const RGB_MAGENTA: (u8, u8, u8) = (255, 0, 255);
const RGB_CYAN: (u8, u8, u8) = (0, 255, 255);

// xdg_toplevel states
const FULLSCREEN: u32 = 2;
const ACTIVATED: u32 = 4;

/// The screen, and where the windows go on it: A full screen, B (640 x 480) centred in the room the bar and dock leave free.
const SCREEN: (i32, i32) = (1600, 900);
const B_RECT: (i32, i32, i32, i32) = (480, 194, 640, 480);
/// B's middle (over A too), a point of A only, and the bar's middle.
const IN_B: (i32, i32) = (800, 434);
const A_ONLY: (i32, i32) = (200, 450);
const ON_BAR: (i32, i32) = (800, 10);

impl Dispatch<XdgPopup, usize> for State {
    fn event(st: &mut Self, _: &XdgPopup, event: xdg_popup::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_popup::Event::Configure { width, height, .. } = event {
            st.wins[*i].size = (width, height);
        }
    }
}

/// A bar (red, Top layer, across the top) and a dock (green, Top layer, at the bottom), both reserving their room, as the panel.
fn bar_and_dock(c: &mut Client, output: usize) -> (LayerSurf, LayerSurf) {
    let bar =
        c.layer_with(output, Layer::Top, "test-bar", Anchor::Top | Anchor::Left | Anchor::Right, (0, 32), 32, KeyboardInteractivity::None);
    c.draw_layer(&bar, RED);
    let dock = c.layer_with(output, Layer::Top, "test-dock", Anchor::Bottom, (600, 64), 64, KeyboardInteractivity::None);
    c.draw_layer(&dock, GREEN);
    (bar, dock)
}

/// Draw a window again at the size the compositor last gave it, until the compositor lists it at that size.
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

/// A full screen window A (blue) and a 640 x 480 window B (yellow) opened over it, on a screen with a bar and a dock.
struct Setup {
    comp: Compositor,
    c: Client,
    a: Window,
    b: Window,
    _layers: (LayerSurf, LayerSurf),
}

fn setup_with(env: &[(&str, &str)]) -> Setup {
    let comp = Compositor::start_env(&["--size", "1600x900"], env, |_| {});
    let mut c = comp.connect();
    let _input = c.input();
    let layers = bar_and_dock(&mut c, 0);
    let a = c.window("test.a", "Game");
    c.draw(&a, (640, 480), BLUE);
    a.toplevel.set_fullscreen(None);
    c.roundtrip();
    assert!(c.state.wins[a.index].states.contains(&FULLSCREEN));
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert_eq!(info.rect(), (0, 0, SCREEN.0, SCREEN.1));
    let b = c.window("test.b", "Small");
    c.draw(&b, (640, 480), YELLOW);
    comp.wait(10, "B placed", |comp| comp.window("test.b").is_some_and(|i| i.rect() == B_RECT));
    Setup { comp, c, a, b, _layers: layers }
}

fn setup() -> Setup {
    setup_with(&[])
}

/// The window drawn in front of all others on the workspace showing (the control socket lists them back to front).
fn front_app(comp: &Compositor) -> String {
    comp.windows().into_iter().rfind(|w| !w.has("minimized") && w.workspace == 1).map(|w| w.app_id).unwrap_or_default()
}

fn focused_app(comp: &Compositor) -> String {
    comp.focus().split('\t').nth(1).unwrap_or("").to_string()
}

fn id_of(comp: &Compositor, app: &str) -> String {
    comp.must("clients").iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap().split('\t').next().unwrap().to_string()
}

/// The colour drawn at a point of the first screen.
fn drawn(comp: &Compositor, (x, y): (i32, i32)) -> (u8, u8, u8) {
    comp.screenshot("HEADLESS-1").at(x as usize, y as usize)
}

/// Click at a point: the surface that got the button (None when no program got it).
fn clicked(comp: &Compositor, c: &mut Client, (x, y): (i32, i32)) -> Option<ObjectId> {
    let before = c.state.buttons.len();
    comp.must(&format!("pointer {x} {y}"));
    comp.must("click");
    let end = Instant::now() + Duration::from_millis(1500);
    while c.state.buttons.len() == before && Instant::now() < end {
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(10));
    }
    if c.state.buttons.len() > before {
        c.state.pointer_on.clone()
    } else {
        None
    }
}

/// Press X: the surface that got the key (None when no surface of this client got it).
fn typed_to(comp: &Compositor, c: &mut Client) -> Option<ObjectId> {
    let before = c.state.keys.len();
    comp.must("key x");
    let end = Instant::now() + Duration::from_millis(1500);
    while c.state.keys.len() == before && Instant::now() < end {
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(10));
    }
    c.state.keys[before..].last().map(|(s, _)| s.clone())
}

fn active(c: &Client, w: &Window) -> bool {
    c.state.wins[w.index].states.contains(&ACTIVATED)
}

/// The heart of these tests: `w` is in front everywhere. It is listed last, drawn at `probe` (in `colour`), has the keyboard
/// (a key reaches it), is the one the programs are told is active, and a click at `probe` reaches it and changes nothing.
#[track_caller]
fn in_front(s: &mut Setup, w: &Window, app: &str, colour: (u8, u8, u8), probe: (i32, i32), others: &[&Window], when: &str) {
    let (comp, c) = (&s.comp, &mut s.c);
    c.roundtrip();
    assert_eq!(front_app(comp), app, "{when}: {app} listed in front");
    assert_eq!(drawn(comp, probe), colour, "{when}: {app} drawn in front at {probe:?}");
    assert_eq!(focused_app(comp), app, "{when}: {app} has the keyboard");
    assert_eq!(typed_to(comp, c), Some(w.surface.id()), "{when}: a key reaches {app}");
    assert!(active(c, w), "{when}: {app} is told it is active");
    for o in others {
        assert!(!active(c, o), "{when}: only {app} is told it is active");
    }
    assert_eq!(clicked(comp, c, probe), Some(w.surface.id()), "{when}: a click at {probe:?} reaches {app}");
    assert_eq!(front_app(comp), app, "{when}: the click left {app} in front");
    assert_eq!(focused_app(comp), app, "{when}: the click left the keyboard with {app}");
}

#[test]
fn a_window_opened_over_a_full_screen_one_is_in_front_until_the_full_screen_one_is_clicked() {
    let _l = limit(90, "a window over a full screen one");
    let mut s = setup();
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "B opened over full screen A");
    // with B (not full screen) in front, the bar shows again and takes its clicks
    assert_eq!(drawn(&s.comp, ON_BAR), RGB_RED, "the bar shows over A while B is in front");
    assert_eq!(clicked(&s.comp, &mut s.c, ON_BAR), Some(s._layers.0.surface.id()), "a click on the bar reaches the bar");
    assert_eq!(focused_app(&s.comp), "test.b", "the bar takes no keyboard");

    // a click on A outside B brings A forward: B is no longer drawn over it, and a click where B was reaches A
    assert_eq!(clicked(&s.comp, &mut s.c, A_ONLY), Some(a.surface.id()));
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked");
    // and the bar is behind A again, for the eye and for the pointer
    assert_eq!(drawn(&s.comp, ON_BAR), RGB_BLUE, "the bar hides behind A in front");
    assert_eq!(clicked(&s.comp, &mut s.c, ON_BAR), Some(a.surface.id()), "a click where the bar is reaches A");
}

#[test]
fn switching_by_alt_tab_clicks_and_the_control_socket_moves_drawing_clicks_and_keys_together() {
    let _l = limit(120, "switching back and forth");
    let mut s = setup();
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "opened");
    for round in 0..2 {
        s.comp.must("key alt+Tab");
        in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], &format!("Alt+Tab to A ({round})"));
        s.comp.must("key alt+Tab");
        in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], &format!("Alt+Tab to B ({round})"));
    }
    clicked(&s.comp, &mut s.c, A_ONLY);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked");
    let (ida, idb) = (id_of(&s.comp, "test.a"), id_of(&s.comp, "test.b"));
    for round in 0..2 {
        s.comp.must(&format!("activate {idb}"));
        in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], &format!("activate B ({round})"));
        s.comp.must(&format!("activate {ida}"));
        in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], &format!("activate A ({round})"));
    }
    // Alt+Tab from A, a click back on A, Alt+Tab again
    s.comp.must("key alt+Tab");
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "Alt+Tab to B");
    clicked(&s.comp, &mut s.c, A_ONLY);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked again");
    s.comp.must("key alt+Tab");
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "Alt+Tab back to B");
}

#[test]
fn minimize_restore_close_and_leaving_full_screen_keep_the_order() {
    let _l = limit(120, "minimize, restore, close, leave full screen");
    let mut s = setup();
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());

    // B minimized: A in front with the keyboard, a click where B was reaches A; B restored: in front again
    s.comp.must("key super+Down");
    s.comp.wait(10, "B minimized", |c| c.window("test.b").is_some_and(|i| i.has("minimized")));
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "B minimized");
    let idb = id_of(&s.comp, "test.b");
    s.comp.must(&format!("activate {idb}"));
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "B restored");

    // A leaves full screen while B is in front: B stays in front with the keyboard, A goes back to its place behind it
    s.a.toplevel.unset_fullscreen();
    s.c.roundtrip();
    let info = settle(&s.comp, &mut s.c, &a, "test.a", BLUE);
    assert!(!info.has("fullscreen"));
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "A left full screen behind B");
    // and full screen again from behind (a game asking again): it stays behind B
    s.a.toplevel.set_fullscreen(None);
    s.c.roundtrip();
    settle(&s.comp, &mut s.c, &a, "test.a", BLUE);
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "A full screen again behind B");
    assert_eq!(drawn(&s.comp, A_ONLY), RGB_BLUE, "A covers the rest of the screen");

    // B closed while it has the keyboard: A gets it back, is told it is active, clicks reach it
    s.b.toplevel.destroy();
    s.b.xdg.destroy();
    s.b.surface.destroy();
    s.c.roundtrip();
    s.comp.wait(10, "B gone", |c| c.window("test.b").is_none());
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[], "B closed");

    // A leaves full screen while in front, with another window open behind it: A stays in front, the bar is back
    let b2 = s.c.window("test.b2", "Small again");
    s.c.draw(&b2, (640, 480), YELLOW);
    s.comp.wait(10, "B2 placed", |c| c.window("test.b2").is_some_and(|i| i.rect() == B_RECT));
    clicked(&s.comp, &mut s.c, A_ONLY);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b2], "A clicked over B2");
    s.a.toplevel.unset_fullscreen();
    s.c.roundtrip();
    let info = settle(&s.comp, &mut s.c, &a, "test.a", BLUE);
    let (x, y, w, h) = info.rect();
    in_front(&mut s, &a, "test.a", RGB_BLUE, (x + w / 2, y + h / 2), &[&b2], "A left full screen in front");
    assert_eq!(drawn(&s.comp, ON_BAR), RGB_RED, "the bar is back");
}

#[test]
fn moving_and_resizing_windows_never_lifts_one_over_the_window_in_front() {
    let _l = limit(120, "relocating keeps the stack");
    let mut s = setup();
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());

    // A in front; the dock grows (the free room changes and floating windows are kept clear of it): B stays behind
    clicked(&s.comp, &mut s.c, A_ONLY);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked");
    let dock = &s._layers.1;
    dock.layer.set_size(600, 80);
    dock.layer.set_exclusive_zone(80);
    dock.surface.commit();
    let idx = dock.index;
    s.c.until("the dock's new size", |st| st.layer_configures[&idx].last() == Some(&(600, 80)));
    s.c.draw_layer(&s._layers.1, GREEN);
    s.comp.wait(10, "the free room changed", |c| c.must("free") == ["HEADLESS-1\t0,32\t1600x788"]);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "the dock grew");

    // B in front; the screen changes size (every window follows it, A made full screen on it again): B stays in front
    s.comp.must(&format!("activate {}", id_of(&s.comp, "test.b")));
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "B activated");
    s.comp.must("resize HEADLESS-1 1280x720");
    settle(&s.comp, &mut s.c, &a, "test.a", BLUE);
    let info = settle(&s.comp, &mut s.c, &b, "test.b", YELLOW);
    let (x, y, w, h) = info.rect();
    in_front(&mut s, &b, "test.b", RGB_YELLOW, (x + w / 2, y + h / 2), &[&a], "the screen got smaller");
}

#[test]
fn a_menu_and_a_dialog_of_the_small_window_are_in_front_of_the_full_screen_one() {
    let _l = limit(90, "menu and dialog over full screen");
    let mut s = setup();
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());

    // B's menu (an xdg_popup), 200 x 150 from B's point 600,400: it reaches 160 x 70 past B's corner, over A only
    let index = s.c.state.wins.len();
    s.c.state.wins.push(Win::default());
    let surface = s.c.compositor.create_surface(&s.c.qh, ());
    let xdg = s.c.wm_base.get_xdg_surface(&surface, &s.c.qh, index);
    let pos: XdgPositioner = s.c.wm_base.create_positioner(&s.c.qh, ());
    pos.set_size(200, 150);
    pos.set_anchor_rect(600, 400, 1, 1);
    pos.set_anchor(PAnchor::TopLeft);
    pos.set_gravity(Gravity::BottomRight);
    let popup = xdg.get_popup(Some(&s.b.xdg), &pos, &s.c.qh, index);
    surface.commit();
    s.c.until("the menu's configure", |st| st.wins[index].configured);
    let buf = s.c.buffer(200, 150, MAGENTA);
    surface.attach(Some(&buf), 0, 0);
    surface.damage_buffer(0, 0, 200, 150);
    surface.commit();
    s.c.roundtrip();
    let past_b = (B_RECT.0 + 600 + 180, B_RECT.1 + 400 + 120);
    assert_eq!(drawn(&s.comp, past_b), RGB_MAGENTA, "B's menu drawn over A past B's corner");
    assert_eq!(clicked(&s.comp, &mut s.c, past_b), Some(surface.id()), "a click on the menu reaches it, not A");
    assert_eq!(focused_app(&s.comp), "test.b", "the menu's window keeps the keyboard");
    assert_eq!(front_app(&s.comp), "test.b");
    popup.destroy();
    xdg.destroy();
    surface.destroy();
    s.c.roundtrip();
    assert_eq!(drawn(&s.comp, past_b), RGB_BLUE, "the menu gone");
    assert_eq!(clicked(&s.comp, &mut s.c, past_b), Some(a.surface.id()), "where the menu was is A's");
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked past the menu");

    // a dialog of B (a toplevel with B as its parent) opens in front of full screen A, with the keyboard
    s.comp.must(&format!("activate {}", id_of(&s.comp, "test.b")));
    let index = s.c.state.wins.len();
    s.c.state.wins.push(Win::default());
    let surface = s.c.compositor.create_surface(&s.c.qh, ());
    let xdg = s.c.wm_base.get_xdg_surface(&surface, &s.c.qh, index);
    let toplevel = xdg.get_toplevel(&s.c.qh, index);
    toplevel.set_parent(Some(&s.b.toplevel));
    toplevel.set_app_id("test.dialog".into());
    toplevel.set_title("Save?".into());
    surface.commit();
    s.c.until("the dialog's configure", |st| st.wins[index].configured);
    let d = Window { index, surface, xdg, toplevel };
    s.c.draw(&d, (300, 200), CYAN);
    s.comp.wait(10, "the dialog placed", |c| c.window("test.dialog").is_some_and(|i| i.size == (300, 200)));
    in_front(&mut s, &d, "test.dialog", RGB_CYAN, IN_B, &[&a, &b], "B's dialog opened");
    clicked(&s.comp, &mut s.c, A_ONLY);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b, &d], "A clicked past the dialog");
}

#[test]
fn a_full_screen_window_hides_only_its_own_screens_bar() {
    let _l = limit(90, "full screen on one of two screens");
    let comp = Compositor::start(&["--output", "1280x720", "--output", "1280x720"]);
    let mut c = comp.connect();
    let _input = c.input();
    let one = bar_and_dock(&mut c, 0);
    let two = bar_and_dock(&mut c, 1);
    comp.must("pointer 640 360");
    let a = c.window("test.a", "Game");
    c.draw(&a, (640, 480), BLUE);
    a.toplevel.set_fullscreen(None);
    c.roundtrip();
    let info = settle(&comp, &mut c, &a, "test.a", BLUE);
    assert_eq!(info.rect(), (0, 0, 1280, 720));
    // B opens on the second screen (where the pointer is) and is the frontmost window of all
    comp.must("pointer 1920 360");
    let b = c.window("test.b", "Small");
    c.draw(&b, (400, 300), YELLOW);
    comp.wait(10, "B on the second screen", |comp| comp.window("test.b").is_some_and(|i| i.at.0 >= 1280));
    let shot = |n: u32, x: i32, y: i32| comp.screenshot(&format!("HEADLESS-{n}")).at(x as usize, y as usize);
    // the first screen's bar stays behind A, which is in front there, and the click goes to A
    assert_eq!(shot(1, 640, 10), RGB_BLUE, "the first screen's bar stays hidden behind its full screen window");
    assert_eq!(clicked(&comp, &mut c, (640, 10)), Some(a.surface.id()), "a click there reaches A");
    // the second screen's bar is drawn and clickable, whatever the first screen shows
    assert_eq!(shot(2, 640, 10), RGB_RED, "the second screen's bar shows");
    assert_eq!(clicked(&comp, &mut c, (1920, 10)), Some(two.0.surface.id()), "a click on the second screen's bar reaches it");
    drop(one);
}

/// Under variable refresh on the hardware backend only a full screen window's own frames start a refresh of its screen; every
/// other change waits for its next frame (udev.rs, `vrr_flip`, which has the rule's own tests). Which commits are its frames
/// is decided for every backend: its own, counted for its own screen. Never the pointer moving or changing its picture, the
/// bar, a notification, a window drawn over it or a window on another screen; and none once it has left full screen.
#[test]
fn only_a_full_screen_windows_own_frames_count_for_its_screen() {
    let _l = limit(90, "a full screen window's frames");
    let comp = Compositor::start(&["--output", "1280x720", "--output", "1280x720"]);
    let mut c = comp.connect();
    let (_k, pointer) = c.input();
    let (bar, _dock) = bar_and_dock(&mut c, 0);
    let counts = |comp: &Compositor| (comp.game_commits("HEADLESS-1"), comp.game_commits("HEADLESS-2"));
    comp.must("pointer 640 360");
    let a = c.window("test.a", "Game");
    c.draw(&a, (640, 480), BLUE);
    c.draw(&a, (640, 480), BLUE);
    assert_eq!(counts(&comp), (0, 0), "a window's frames before it is full screen");
    a.toplevel.set_fullscreen(None);
    c.roundtrip();
    assert_eq!(settle(&comp, &mut c, &a, "test.a", BLUE).rect(), (0, 0, 1280, 720));
    let (n, _) = counts(&comp);
    assert!(n > 0, "its frames at full screen are counted");
    assert_eq!(counts(&comp), (n, 0), "for its own screen only");

    // a window over it, the bar behind it drawing again, a notification on the Overlay layer
    let b = c.window("test.b", "Small");
    c.draw(&b, (400, 300), YELLOW);
    comp.wait(10, "B over A", |comp| comp.window("test.b").is_some_and(|i| i.at.0 < 1280));
    c.draw(&b, (400, 300), CYAN);
    c.draw_layer(&bar, MAGENTA);
    let toast = c.layer_with(0, Layer::Overlay, "test-toast", Anchor::Top | Anchor::Right, (300, 80), 0, KeyboardInteractivity::None);
    c.draw_layer(&toast, YELLOW);
    assert_eq!(comp.screenshot("HEADLESS-1").at(1280 - 150, 40), RGB_YELLOW, "the notification is drawn over A");
    // the pointer moving over A and the program giving it a picture of its own
    comp.must("pointer 100 600");
    comp.must("pointer-move 20 -10");
    c.until("the pointer on A", |s| s.pointer_on == Some(a.surface.id()));
    let cs = c.compositor.create_surface(&c.qh, ());
    let pic = c.buffer(16, 16, MAGENTA);
    cs.attach(Some(&pic), 0, 0);
    cs.damage_buffer(0, 0, 16, 16);
    pointer.set_cursor(c.state.pointer_serial, Some(&cs), 0, 0);
    cs.commit();
    c.roundtrip();
    assert_eq!(comp.must("pointer-look"), ["surface\t16x16\t0,0"]);
    // a window on the second screen
    comp.must("pointer 1920 360");
    let d = c.window("test.d", "Other");
    c.draw(&d, (400, 300), GREEN);
    comp.wait(10, "D on the second screen", |comp| comp.window("test.d").is_some_and(|i| i.at.0 >= 1280));
    c.draw(&d, (400, 300), RED);
    assert_eq!(counts(&comp), (n, 0), "none of these is A's frame");

    // A's own frame counts, once, for its screen
    c.draw(&a, (1280, 720), BLUE);
    assert_eq!(counts(&comp), (n + 1, 0));
    // D full screen on the second screen: its frames count there, A's still here
    d.toplevel.set_fullscreen(None);
    c.roundtrip();
    assert_eq!(settle(&comp, &mut c, &d, "test.d", RED).rect(), (1280, 0, 1280, 720));
    let (_, m) = counts(&comp);
    assert!(m > 0, "D's frames at full screen are counted for the second screen");
    c.draw(&d, (1280, 720), GREEN);
    c.draw(&a, (1280, 720), BLUE);
    assert_eq!(counts(&comp), (n + 2, m + 1));
    // A out of full screen: its frames are a window's like any other again
    a.toplevel.unset_fullscreen();
    c.roundtrip();
    settle(&comp, &mut c, &a, "test.a", BLUE);
    assert!(!comp.window("test.a").unwrap().has("fullscreen"));
    let (left, _) = counts(&comp);
    c.draw(&a, (640, 480), CYAN);
    assert_eq!(counts(&comp), (left, m + 1), "a window that left full screen paces nothing");
}

#[test]
fn a_hidden_windows_resize_grips_take_no_clicks_from_the_full_screen_window() {
    let _l = limit(90, "grips behind a full screen window");
    // every window decorated by the compositor (server-side), so B has grips just outside its edges
    let mut s = setup_with(&[("YGGDRASIL_COMPOSITOR_DECORATE", "all")]);
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());
    clicked(&s.comp, &mut s.c, A_ONLY);
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked");
    // just outside B's right edge and its top left corner: hidden B's grips, but A in front there
    for p in [(B_RECT.0 + B_RECT.2 + 3, IN_B.1), (B_RECT.0 - 3, B_RECT.1 - 3)] {
        assert_eq!(drawn(&s.comp, p), RGB_BLUE);
        assert_eq!(clicked(&s.comp, &mut s.c, p), Some(a.surface.id()), "a click at {p:?} reaches A, not B's grip");
        in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A clicked by B's hidden grip");
        assert_eq!(s.comp.window("test.b").unwrap().rect(), B_RECT, "B was neither moved nor resized");
    }
}

#[test]
fn the_lock_keeps_the_keyboard_when_windows_open_and_close_behind_it() {
    let _l = limit(90, "the lock and windows behind it");
    let mut s = setup();
    let mut l = s.comp.connect();
    let _input = l.input();
    let mgr: ExtSessionLockManagerV1 = l.globals.bind(&l.qh, 1..=1, ()).expect("ext-session-lock");
    let lock = mgr.lock(&l.qh, ());
    let ls: WlSurface = l.compositor.create_surface(&l.qh, ());
    let o = l.wl_outputs[0].clone();
    let _lsurf = lock.get_lock_surface(&ls, &o, &l.qh, 0);
    l.until("locked", |st| st.locked && st.lock_sizes.contains_key(&0));
    let (w, h) = l.state.lock_sizes[&0];
    let buf = l.buffer(w as i32, h as i32, RED);
    ls.attach(Some(&buf), 0, 0);
    ls.damage_buffer(0, 0, w as i32, h as i32);
    ls.commit();
    l.roundtrip();
    assert_eq!(s.comp.focus(), "lock\tHEADLESS-1");

    // a window opens behind the lock, and B closes: the keyboard stays with the lock, every key reaches only the lock
    let c2 = s.c.window("test.c", "Opened while locked");
    s.c.draw(&c2, (400, 300), MAGENTA);
    s.b.toplevel.destroy();
    s.b.xdg.destroy();
    s.b.surface.destroy();
    s.c.roundtrip();
    s.comp.wait(10, "B gone", |c| c.window("test.b").is_none());
    assert_eq!(s.comp.focus(), "lock\tHEADLESS-1", "the lock keeps the keyboard");
    let before = s.c.state.keys.len();
    s.comp.must("type secret");
    s.c.roundtrip();
    l.roundtrip();
    assert_eq!(s.c.state.keys.len(), before, "no key reached a window behind the lock");
    assert!(l.state.keys.len() >= 6, "the lock got the keys");
    assert_eq!(drawn(&s.comp, IN_B), RGB_RED, "only the lock is drawn");
}

/// A finger tapped at a point of the touchscreen (the first screen): where it landed on the surface it went to, in that
/// surface's own coordinates (None when no surface of this client got it).
fn tapped(comp: &Compositor, c: &mut Client, (x, y): (i32, i32)) -> Option<(f64, f64)> {
    let before = c.state.touches.len();
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    let end = Instant::now() + Duration::from_millis(1500);
    while !c.state.touches[before..].iter().any(|t| t.0 == "up") && Instant::now() < end {
        c.roundtrip();
        std::thread::sleep(Duration::from_millis(10));
    }
    c.state.touches[before..].iter().find(|t| t.0 == "down").map(|t| (t.2, t.3))
}

#[test]
fn a_finger_follows_the_same_order_as_the_pointer() {
    let _l = limit(90, "touch over a full screen window");
    let mut s = setup();
    let (a, b) = (s.a.clone_handles(), s.b.clone_handles());
    // the first touch gives the seat its touch (on the bar, which takes no keyboard: nothing changes)
    s.comp.must("touch-down 0 1 1");
    s.comp.must("touch-up 0");
    s.c.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _t = s.c.touch();
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "B opened");
    // a tap in B lands on B (at its middle), and B stays in front
    assert_eq!(tapped(&s.comp, &mut s.c, IN_B), Some((320.0, 240.0)), "a tap in B reaches B");
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "B tapped");
    // a tap on A outside B lands on A and brings it forward; then a tap where B was, or where the bar is, lands on A
    assert_eq!(tapped(&s.comp, &mut s.c, A_ONLY), Some((200.0, 450.0)), "a tap outside B reaches A");
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A tapped");
    assert_eq!(tapped(&s.comp, &mut s.c, IN_B), Some((800.0, 434.0)), "a tap where B was reaches A");
    assert_eq!(tapped(&s.comp, &mut s.c, ON_BAR), Some((800.0, 10.0)), "a tap where the hidden bar is reaches A");
    in_front(&mut s, &a, "test.a", RGB_BLUE, IN_B, &[&b], "A tapped again");
    // Alt+Tab back to B: a tap in B reaches B again
    s.comp.must("key alt+Tab");
    in_front(&mut s, &b, "test.b", RGB_YELLOW, IN_B, &[&a], "Alt+Tab to B");
    assert_eq!(tapped(&s.comp, &mut s.c, IN_B), Some((320.0, 240.0)), "a tap in B reaches B");
}

/// A window that asks for full screen and commits its first picture before it has read the configure that makes it so (at
/// its own size, the state not acked yet) is still placed full screen: it is not centred as a floating window at that size
/// and left there when its full screen picture comes.
#[test]
fn a_window_whose_first_picture_comes_before_its_full_screen_configure_is_placed_full_screen() {
    let _l = limit(60, "a full screen window's early first picture");
    let comp = Compositor::start(&["--size", "1600x900"]);
    let mut c = comp.connect();
    let w = c.window("test.early", "Early");
    // asked for full screen, and the first picture sent with the same flush: the compositor gets the commit before the client
    // has read (and acked) the full screen configure
    w.toplevel.set_fullscreen(None);
    let b = c.buffer(400, 300, BLUE);
    w.surface.attach(Some(&b), 0, 0);
    w.surface.damage_buffer(0, 0, 400, 300);
    w.surface.commit();
    c.roundtrip();
    assert!(c.state.wins[w.index].states.contains(&FULLSCREEN), "the full screen configure came after the commit");
    let at = comp.window("test.early").expect("listed").at;
    assert_eq!(at, (0, 0), "not centred at its first picture's size");
    // its full screen picture: the whole screen
    let info = settle(&comp, &mut c, &w, "test.early", BLUE);
    assert!(info.has("fullscreen"), "{info:?}");
    assert_eq!(info.rect(), (0, 0, SCREEN.0, SCREEN.1));
    assert_eq!(drawn(&comp, (1500, 850)), RGB_BLUE, "drawn to the bottom right corner");
}

/// Handles of a window to keep beside the test's set-up (the protocol objects are cheap clones of the same objects).
trait CloneHandles {
    fn clone_handles(&self) -> Window;
}

impl CloneHandles for Window {
    fn clone_handles(&self) -> Window {
        Window { index: self.index, surface: self.surface.clone(), xdg: self.xdg.clone(), toplevel: self.toplevel.clone() }
    }
}
