//! Moving and resizing a window with the pointer (src/grabs.rs): a program asks while the button is held on its own window
//! (xdg_toplevel's move and resize for a Wayland program, _NET_WM_MOVERESIZE for an X11 one), and the window follows the pointer
//! until the button is let go. Only the program the button went down on may ask: Smithay checks a Wayland request's serial
//! alone, serials are one counter every program sees, and any X11 program may ask for any window at any time. The X11 test is
//! skipped where XWayland is not installed.

mod common;

use std::time::{Duration, Instant};

use common::*;
use wayland_client::Proxy;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, WindowClass};
use x11rb::protocol::Event;
use x11rb::wrapper::ConnectionExt as _;

const BLUE: u32 = 0xff0000ff;
const GREEN: u32 = 0xff00ff00;

/// The control socket's number for a window, by its app id (an X11 window's: its WM_CLASS class).
fn id_of(comp: &Compositor, app: &str) -> String {
    let lines = comp.must("clients");
    let line = lines.iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap_or_else(|| panic!("{app} in {lines:?}"));
    line.split('\t').next().unwrap().to_string()
}

/// The button held on a Wayland program's window (at `at`): the press's serial, as the program is told it.
fn press_on(comp: &Compositor, c: &mut Client, w: &Window, at: (i32, i32)) -> u32 {
    comp.must(&format!("pointer {} {}", at.0, at.1));
    c.until("the pointer on the window", |st| st.pointer_on == Some(w.surface.id()));
    c.state.button_serial = 0;
    comp.must("press");
    c.until("the press", |st| st.button_serial != 0);
    c.state.button_serial
}

/// A Wayland program moves and resizes its window with a press on it. Another program asking with that press, which it was told,
/// moves and resizes nothing of its own, and the press stays the pressed program's.
#[test]
fn a_window_moves_and_resizes_only_with_a_press_on_its_own_program() {
    let _l = limit(60, "moving and resizing with a press");
    let comp = Compositor::start(&["--size", "1000x600"]);
    let mut a = comp.connect();
    let (_ak, _ap) = a.input();
    let aw = a.window("test.a", "A");
    a.draw(&aw, (300, 200), BLUE);
    let mut b = comp.connect();
    let bw = b.window("test.b", "B");
    b.draw(&bw, (300, 200), GREEN);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.a")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.b")));
    a.roundtrip();
    b.roundtrip();
    let (ai, bi) = (comp.window("test.a").unwrap(), comp.window("test.b").unwrap());
    let b_size = b.state.wins[bw.index].size;

    // the button held on A's window: B asks to move its window with that press, then to resize it, and the pointer moves away
    let (px, py) = (ai.at.0 + 100, ai.at.1 + 100);
    let serial = press_on(&comp, &mut a, &aw, (px, py));
    bw.toplevel._move(&b.seat, serial);
    b.roundtrip();
    comp.must(&format!("pointer {} {}", px + 300, py + 150));
    bw.toplevel.resize(&b.seat, serial, ResizeEdge::BottomRight);
    b.roundtrip();
    comp.must(&format!("pointer {} {}", px + 320, py + 170));
    a.roundtrip();
    b.roundtrip();
    let now = comp.window("test.b").unwrap();
    assert_eq!((now.at, now.size), (bi.at, bi.size), "B's window neither moved nor resized");
    assert_eq!(b.state.wins[bw.index].size, b_size, "B not asked to take another size");
    assert_eq!(a.state.pointer_on, Some(aw.surface.id()), "the press still A's");
    comp.must("release");

    // A's own press: its window follows the pointer until the button is let go
    let serial = press_on(&comp, &mut a, &aw, (px, py));
    aw.toplevel._move(&a.seat, serial);
    a.roundtrip();
    comp.must(&format!("pointer {} {}", px + 40, py + 30));
    let moved = (ai.at.0 + 40, ai.at.1 + 30);
    assert_eq!(comp.window("test.a").unwrap().at, moved, "A's window follows the pointer");
    comp.must("release");
    comp.must(&format!("pointer {} {}", px + 60, py + 30));
    assert_eq!(comp.window("test.a").unwrap().at, moved, "and stays when the button is let go");

    // and its own press resizes it, from its bottom right corner
    let (px, py) = (moved.0 + 100, moved.1 + 100);
    let serial = press_on(&comp, &mut a, &aw, (px, py));
    aw.toplevel.resize(&a.seat, serial, ResizeEdge::BottomRight);
    a.roundtrip();
    comp.must(&format!("pointer {} {}", px + 60, py + 40));
    a.until("A asked to take the size the pointer gives it", |st| st.wins[aw.index].size == (360, 240));
    comp.must("release");
}

/// An X11 program asks to move its window, then to resize it (_NET_WM_MOVERESIZE, as on a press on its own title bar or edge),
/// while the button is held on a Wayland window: nothing moves or changes size, and the press stays the Wayland program's. With
/// the button held on its own window, the window follows the pointer.
#[test]
fn an_x11_window_moves_only_with_a_press_on_itself() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "moving an X11 window");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the Wayland window pressed on, on the right
    let mut a = comp.connect();
    let (_ak, _ap) = a.input();
    let aw = a.window("test.a", "A");
    a.draw(&aw, (300, 200), BLUE);
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.a")));
    a.roundtrip();
    // the X11 program's window, green, on the left; it hears its presses
    let (conn, screen) = x11rb::connect(Some(&display)).expect("connect to XWayland");
    let root = conn.setup().roots[screen].root;
    let moveresize = conn.intern_atom(false, b"_NET_WM_MOVERESIZE").unwrap().reply().unwrap().atom;
    let win = conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0x00ff00).event_mask(EventMask::BUTTON_PRESS);
    conn.create_window(0, win, root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, b"x11moved\0X11moved\0").unwrap();
    conn.map_window(win).unwrap();
    conn.flush().unwrap();
    comp.wait(10, "the X11 window managed", |c| c.window("X11moved").is_some());
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "X11moved")));
    // drawn: only then has XWayland made its surface, so the pointer can be in it
    comp.wait(10, "the X11 window drawn where it was placed", |c| {
        c.screenshot("HEADLESS-1").count((70, 120, 50, 50), (0, 255, 0)) == 50 * 50
    });
    let (ai, xi) = (comp.window("test.a").unwrap(), comp.window("X11moved").unwrap());
    // _NET_WM_MOVERESIZE for the window, sent to the root as programs send it (8: move, 4: resize from the bottom right corner)
    let ask = |direction: u32, at: (i32, i32)| {
        let event = ClientMessageEvent::new(32, win, moveresize, [at.0 as u32, at.1 as u32, direction, 1, 1]);
        conn.send_event(false, root, EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY, event).unwrap();
        conn.flush().unwrap();
    };

    // the button held on the Wayland window; the X11 program asks to move its window, then to resize it
    let (px, py) = (ai.at.0 + 100, ai.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    a.until("the pointer on the Wayland window", |st| st.pointer_on == Some(aw.surface.id()));
    comp.must("press");
    a.until("the press", |st| st.button_serial != 0);
    ask(8, (px, py));
    ask(4, (px, py));
    // the window manager reads its X events in order: once it manages a window the program shows after them, it has read them
    let after = conn.generate_id().unwrap();
    conn.create_window(0, after, root, 0, 0, 50, 50, 0, WindowClass::INPUT_OUTPUT, 0, &CreateWindowAux::new()).unwrap();
    conn.change_property8(PropMode::REPLACE, after, AtomEnum::WM_CLASS, AtomEnum::STRING, b"after\0After\0").unwrap();
    conn.map_window(after).unwrap();
    conn.flush().unwrap();
    comp.wait(10, "the window shown after the requests managed", |c| c.window("After").is_some());
    comp.must(&format!("place {} 900 500 50 50", id_of(&comp, "After")));
    comp.must(&format!("pointer {} {}", px - 200, py + 150));
    a.roundtrip();
    let now = comp.window("X11moved").unwrap();
    assert_eq!((now.at, now.size), (xi.at, xi.size), "the X11 window neither moved nor resized");
    assert_eq!(a.state.pointer_on, Some(aw.surface.id()), "the press still the Wayland program's");
    comp.must("release");

    // the button held on the X11 window itself: once the window manager has read the request, the window follows the pointer
    let (qx, qy) = (xi.at.0 + 100, xi.at.1 + 100);
    comp.must(&format!("pointer {qx} {qy}"));
    comp.must("press");
    let start = Instant::now();
    while !matches!(conn.poll_for_event().expect("the X11 connection"), Some(Event::ButtonPress(_))) {
        assert!(start.elapsed() < Duration::from_secs(5), "the X11 program hears its press");
        std::thread::sleep(Duration::from_millis(10));
    }
    ask(8, (qx, qy));
    let moved = (xi.at.0 + 40, xi.at.1 + 30);
    let start = Instant::now();
    loop {
        comp.must(&format!("pointer {} {}", qx + 40, qy + 30));
        let at = comp.window("X11moved").unwrap().at;
        if at == moved {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "the X11 window follows the pointer: at {at:?}, not {moved:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
    comp.must("release");
}
