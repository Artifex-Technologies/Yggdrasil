//! Drag and drop between two programs: what is dropped arrives. One program drags out of its window; the other, under the
//! pointer when the button is let go, is offered it, accepts it, and reads it through the pipe the compositor hands over; the
//! dragging program is told the drop finished. The compositor's part is routing: the offer to the window under the pointer,
//! the drop on release, the transfer between the two.
//!
//! Wayland programs speak wl_data_device, X11 programs XDND, and Smithay's X11 window manager speaks XDND for the Wayland side,
//! both ways. A Wayland program's drag over an X11 window is told to that window (XdndEnter, XdndPosition, XdndDrop), which
//! asks for the data through XdndSelection. While an X11 program drags, the window manager stands in for the Wayland windows
//! with a window of its own over the whole screen that takes XDND. XWayland can go away in the middle of a drag (it crashed):
//! its windows go, and the compositor and a Wayland program's drag go on. It can also go away in the middle of a transfer, a
//! drop's or the clipboard's (a paste goes through a pipe the window manager hands over, as a drop does): the transfer ends
//! with it, and the compositor goes on. The X11 tests are skipped where XWayland is not installed.

mod common;

use std::cell::RefCell;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use common::*;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ClientMessageEvent, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, SelectionNotifyEvent,
    SelectionRequestEvent, Window as XWindow, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

const MIME: &str = "text/plain;charset=utf-8";
const WORDS: &[u8] = b"dropped from one program into another";
/// A file, as a file manager drags one: its URI (text/uri-list: one per line, each line ending in CR LF).
const URI_MIME: &str = "text/uri-list";
const URI: &[u8] = b"file:///tmp/dropped%20file.txt\r\n";

/// What the receiving and the dragging program were told (each test runs on a thread of its own, with a log of its own).
#[derive(Default)]
struct Log {
    /// the receiver: the offer it was given when the drag came in, with its serial, and the types it offers
    entered: Option<(u32, WlDataOffer)>,
    mimes: Vec<String>,
    dropped: bool,
    /// the receiver: the clipboard it was offered
    clipboard: Option<WlDataOffer>,
    /// the dragging program: the action the receiver chose, data asked for, the drop finished
    action: Option<DndAction>,
    sent: usize,
    finished: bool,
    cancelled: bool,
    /// the dragging (or copying) program is a slow one (`hold`): asked for its data, it keeps the pipe for later (`held`)
    hold: bool,
    held: Option<OwnedFd>,
    /// a program's looks at the serial counter (`look`), the serial of the last finger down it was told of, and whether it was
    /// told a finger lifted
    looks: Vec<u32>,
    touched: Option<u32>,
    lifted: bool,
}

thread_local! {
    static LOG: RefCell<Log> = RefCell::default();
}

fn log<T>(f: impl FnOnce(&mut Log) -> T) -> T {
    LOG.with(|l| f(&mut l.borrow_mut()))
}

/// Our own user data types, so these handlers sit beside the harness's ignoring ones.
struct Receiver;
struct Offered;
struct Dragged;

impl Dispatch<WlDataDevice, Receiver> for State {
    fn event(_: &mut Self, _: &WlDataDevice, event: wl_data_device::Event, _: &Receiver, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_device::Event::Enter { serial, id: Some(offer), .. } => log(|l| l.entered = Some((serial, offer))),
            wl_data_device::Event::Drop => log(|l| l.dropped = true),
            wl_data_device::Event::Selection { id } => log(|l| l.clipboard = id),
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, Offered),
    ]);
}

impl Dispatch<WlDataOffer, Offered> for State {
    fn event(_: &mut Self, _: &WlDataOffer, event: wl_data_offer::Event, _: &Offered, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            log(|l| l.mimes.push(mime_type));
        }
    }
}

impl Dispatch<WlDataSource, Dragged> for State {
    fn event(_: &mut Self, _: &WlDataSource, event: wl_data_source::Event, _: &Dragged, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                if log(|l| l.hold) {
                    log(|l| l.held = Some(fd));
                } else {
                    let what = match mime_type.as_str() {
                        MIME => WORDS,
                        URI_MIME => URI,
                        _ => &[],
                    };
                    let _ = std::fs::File::from(fd).write_all(what);
                }
                log(|l| l.sent += 1);
            }
            wl_data_source::Event::Action { dnd_action: WEnum::Value(a) } => log(|l| l.action = Some(a)),
            wl_data_source::Event::DndFinished => log(|l| l.finished = true),
            wl_data_source::Event::Cancelled => log(|l| l.cancelled = true),
            _ => {}
        }
    }
}

/// A window of a program's own that is never drawn (so never shown): its first configure's serial tells where the serial
/// counter stands, as any program can see it (`look`).
struct Look;

impl Dispatch<XdgSurface, Look> for State {
    fn event(_: &mut Self, xs: &XdgSurface, event: xdg_surface::Event, _: &Look, _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            xs.ack_configure(serial);
            log(|l| l.looks.push(serial));
        }
    }
}

impl Dispatch<XdgToplevel, Look> for State {
    fn event(_: &mut Self, _: &XdgToplevel, _: xdg_toplevel::Event, _: &Look, _: &Connection, _: &QueueHandle<Self>) {}
}

/// A program's touch: the serial of a finger down, which a drag made with that finger names, and the finger's lift (a finger
/// that became a drag lifts without the program being told).
struct Finger;

impl Dispatch<WlTouch, Finger> for State {
    fn event(_: &mut Self, _: &WlTouch, event: wl_touch::Event, _: &Finger, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_touch::Event::Down { serial, .. } => log(|l| l.touched = Some(serial)),
            wl_touch::Event::Up { .. } => log(|l| l.lifted = true),
            _ => {}
        }
    }
}

/// Dispatches the programs until `done`, at most 5 s.
fn until(programs: &mut [&mut Client], what: &str, done: impl Fn(&Log) -> bool) {
    until_in(5, programs, what, done);
}

/// Dispatches the programs until `done`, at most `secs` seconds.
fn until_in(secs: u64, programs: &mut [&mut Client], what: &str, done: impl Fn(&Log) -> bool) {
    let start = Instant::now();
    while !log(|l| done(l)) {
        for p in programs.iter_mut() {
            p.roundtrip();
        }
        assert!(start.elapsed() < Duration::from_secs(secs), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The control socket's number for a window, by its app id (an X11 window's: its WM_CLASS class).
fn id_of(comp: &Compositor, app: &str) -> String {
    let lines = comp.must("clients");
    let line = lines.iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap_or_else(|| panic!("{app} in {lines:?}"));
    line.split('\t').next().unwrap().to_string()
}

/// The receiving program's data device, made before the drag comes.
fn data_device(t: &mut Client) -> WlDataDevice {
    let m: WlDataDeviceManager = t.globals.bind(&t.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device = m.get_data_device(&t.seat, &t.qh, Receiver);
    t.roundtrip();
    device
}

/// The button held on the program's window `w` (placed at `at`), and a drag of `mime` started there, offered as a copy.
fn drag_from(comp: &Compositor, s: &mut Client, w: &Window, at: (i32, i32), mime: &str) {
    comp.must(&format!("pointer {} {}", at.0 + 100, at.1 + 100));
    s.until("the pointer on the source", |st| st.pointer_on == Some(w.surface.id()));
    comp.must("press");
    s.until("the press", |st| st.button_serial != 0);
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    let source: WlDataSource = m.create_data_source(&s.qh, Dragged);
    source.offer(mime.into());
    source.set_actions(DndAction::Copy);
    device.start_drag(Some(&source), &w.surface, None, s.state.button_serial);
    s.roundtrip();
}

/// A new source of the program `c`'s text, offered as a copy.
fn text(c: &Client, m: &WlDataDeviceManager) -> WlDataSource {
    let source = m.create_data_source(&c.qh, Dragged);
    source.offer(MIME.into());
    source.set_actions(DndAction::Copy);
    source
}

/// Drag icons' colours (ARGB, and as a screenshot shows them).
const RED: u32 = 0xffff0000;
const MAGENTA: u32 = 0xffff00ff;
const RGB_RED: (u8, u8, u8) = (255, 0, 0);
const RGB_MAGENTA: (u8, u8, u8) = (255, 0, 255);

/// A drag's icon of the program `c`: a 20 x 20 picture of one colour, drawn with its corner at the pointer or the finger.
fn icon(c: &Client, argb: u32) -> WlSurface {
    let icon = c.compositor.create_surface(&c.qh, ());
    icon.attach(Some(&c.buffer(20, 20, argb)), 0, 0);
    icon.damage_buffer(0, 0, 20, 20);
    icon.commit();
    icon
}

/// Where the screen shows a drag's icon of that colour: its corner, the one 20 x 20 square of the colour there is; None when
/// none is drawn.
fn icon_at(comp: &Compositor, rgb: (u8, u8, u8)) -> Option<(i32, i32)> {
    let pic = comp.screenshot("HEADLESS-1");
    let (x, y) = (0..pic.h).flat_map(|y| (0..pic.w).map(move |x| (x, y))).find(|&(x, y)| pic.at(x, y) == rgb)?;
    let (x, y) = (x as i32, y as i32);
    let all = pic.count((0, 0, pic.w as i32, pic.h as i32), rgb);
    assert_eq!((pic.count((x, y, 20, 20), rgb), all), (400, 400), "one icon of {rgb:?}, whole, at {x},{y}");
    Some((x, y))
}

/// Where the serial counter stands, as the program `c` can see it at any time: a window of its own (`Look`) and the serial of
/// its first configure.
fn look(c: &mut Client) -> u32 {
    let n = log(|l| l.looks.len());
    let surface = c.compositor.create_surface(&c.qh, ());
    let xdg = c.wm_base.get_xdg_surface(&surface, &c.qh, Look);
    xdg.get_toplevel(&c.qh, Look);
    surface.commit();
    until(&mut [c], "the first configure of a window of its own", |l| l.looks.len() > n);
    log(|l| l.looks[n])
}

#[test]
fn what_is_dropped_on_another_program_arrives_there() {
    let _l = limit(60, "drag and drop");
    let comp = Compositor::start(&["--size", "1000x600"]);
    // the program dragging, on the left; the one receiving, on the right
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.target")));
    s.roundtrip();
    t.roundtrip();
    let (si, ti) = (comp.window("test.source").unwrap(), comp.window("test.target").unwrap());
    assert!(si.at.0 + 300 <= ti.at.0, "side by side: {si:?} {ti:?}");
    let _tdev = data_device(&mut t);
    drag_from(&comp, &mut s, &sw, si.at, MIME);

    // over the receiver's window: it is offered the text, and accepts it
    comp.must(&format!("pointer {} {}", ti.at.0 + 150, ti.at.1 + 100));
    until(&mut [&mut s, &mut t], "the receiver offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == MIME));
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    s.roundtrip();

    // let go over it: the drop comes, the receiver reads what was dragged
    comp.must("release");
    until(&mut [&mut s, &mut t], "the drop", |l| l.dropped);
    let (mut read, write) = std::io::pipe().unwrap();
    offer.receive(MIME.into(), write.as_fd());
    drop(write);
    until(&mut [&mut s, &mut t], "the dragging program asked for the text", |l| l.sent > 0);
    let mut got = vec![];
    read.read_to_end(&mut got).unwrap();
    assert_eq!(got, WORDS, "what was dropped arrived");
    offer.finish();
    until(&mut [&mut s, &mut t], "the dragging program told the drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "not cancelled");
}

/// A program can make a drag only of a press on its own window. Smithay checks only a drag's serial, and serials are one counter
/// every program sees (`look`): a program with no pointer and nothing shown asks to drag its text with the serial of a press on
/// another program's window, then with every serial around the compositor's own move (Super and the left button), whose serial
/// no program is told. Neither becomes its drag: it is told its drag is cancelled, the pointer is not the drag's closed hand,
/// the window the pointer goes over is offered nothing, and the press and the move go on as they were.
#[test]
fn a_program_cannot_drag_with_another_programs_press() {
    let _l = limit(60, "a drag with another program's press");
    let comp = Compositor::start(&["--size", "1000x600"]);
    // the program pressed on, on the left; the one the pointer goes over, on the right, which takes text
    let mut a = comp.connect();
    let (_ak, _ap) = a.input();
    let aw = a.window("test.pressed", "Pressed");
    a.draw(&aw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.pressed")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.target")));
    a.roundtrip();
    t.roundtrip();
    let (ai, ti) = (comp.window("test.pressed").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    // the other program: a data device and its text, no pointer, no keyboard, nothing shown
    let mut b = comp.connect();
    let m: WlDataDeviceManager = b.globals.bind(&b.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&b.seat, &b.qh, ());
    let origin = b.compositor.create_surface(&b.qh, ());
    b.roundtrip();

    // the button held on the first program's window, and the other program told that press's serial
    let (px, py) = (ai.at.0 + 100, ai.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    a.until("the pointer on the pressed window", |st| st.pointer_on == Some(aw.surface.id()));
    comp.must("press");
    a.until("the press", |st| st.button_serial != 0);
    let pointer = comp.must("pointer-look");
    device.start_drag(Some(&text(&b, &m)), &origin, None, a.state.button_serial);
    until(&mut [&mut b], "the other program told its drag is cancelled", |l| l.cancelled);
    assert_eq!(comp.must("pointer-look"), pointer, "the pointer as it was, not a drag's closed hand");
    // over the window on the right: it is offered nothing, and the press is still the first program's
    comp.must(&format!("pointer {} {}", ti.at.0 + 150, ti.at.1 + 100));
    for p in [&mut a, &mut b, &mut t] {
        p.roundtrip();
    }
    assert!(log(|l| l.entered.is_none()), "nothing offered to the window under the pointer");
    assert_eq!(a.state.pointer_on, Some(aw.surface.id()), "the press still the first program's");
    comp.must("release");

    // the compositor's own move, with every serial handed out between two looks at the counter (the move's among them)
    log(|l| *l = Log::default());
    let first = look(&mut b);
    comp.must(&format!("pointer {px} {py}"));
    comp.must("key-down super");
    comp.must("press");
    comp.must(&format!("pointer {} {py}", px + 20));
    assert_eq!(comp.window("test.pressed").unwrap().at, (ai.at.0 + 20, ai.at.1), "Super and the left button move the window");
    let pointer = comp.must("pointer-look");
    let last = look(&mut b);
    let source = text(&b, &m);
    for serial in first..=last {
        device.start_drag(Some(&source), &origin, None, serial);
    }
    until(&mut [&mut b], "the other program told its drag is cancelled", |l| l.cancelled);
    assert_eq!(comp.must("pointer-look"), pointer, "the pointer as it was, not a drag's closed hand");
    comp.must(&format!("pointer {} {py}", px + 40));
    assert_eq!(comp.window("test.pressed").unwrap().at, (ai.at.0 + 40, ai.at.1), "the move goes on");
    comp.must("release");
    comp.must("key-up super");
}

/// The same with a finger: a program told the serial of a finger put down on another program's window cannot make a drag of
/// it. It is told its drag is cancelled, the pointer is not a drag's closed hand, the window the finger goes over is offered
/// nothing, and the finger stays the first program's: it is told the finger lifted.
#[test]
fn a_program_cannot_drag_with_another_programs_finger() {
    let _l = limit(60, "a drag with another program's finger");
    let comp = Compositor::start(&["--size", "1000x600"]);
    // the program touched, on the left; the one the finger goes over, on the right, which takes text
    let mut a = comp.connect();
    let (_ak, _ap) = a.input();
    let aw = a.window("test.pressed", "Pressed");
    a.draw(&aw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.pressed")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.target")));
    a.roundtrip();
    t.roundtrip();
    let (ai, ti) = (comp.window("test.pressed").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    // the other program: a data device and its text, no touch, nothing shown
    let mut b = comp.connect();
    let m: WlDataDeviceManager = b.globals.bind(&b.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&b.seat, &b.qh, ());
    let origin = b.compositor.create_surface(&b.qh, ());
    b.roundtrip();
    // the first touch gives the seat its touch; the first program then takes it
    let (x, y) = (ai.at.0 + 100, ai.at.1 + 100);
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    a.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = a.seat.get_touch(&a.qh, Finger);
    a.roundtrip();

    // a finger down on the first program's window, and the other program told its serial
    comp.must(&format!("touch-down 1 {x} {y}"));
    until(&mut [&mut a], "the finger down", |l| l.touched.is_some());
    let pointer = comp.must("pointer-look");
    device.start_drag(Some(&text(&b, &m)), &origin, None, log(|l| l.touched.unwrap()));
    until(&mut [&mut b], "the other program told its drag is cancelled", |l| l.cancelled);
    assert_eq!(comp.must("pointer-look"), pointer, "the pointer as it was, not a drag's closed hand");
    // over the window on the right: it is offered nothing; the finger lifts, and the first program is told
    comp.must(&format!("touch-move 1 {} {}", ti.at.0 + 150, ti.at.1 + 100));
    for p in [&mut a, &mut b, &mut t] {
        p.roundtrip();
    }
    assert!(log(|l| l.entered.is_none()), "nothing offered to the window under the finger");
    comp.must("touch-up 1");
    until(&mut [&mut a], "the first program told the finger lifted", |l| l.lifted);
}

/// A program can make a drag only of a finger still down. Smithay keeps the first finger's serial (with its slot) until every
/// finger is up, so a program can ask with a finger that has already lifted while another is still down (a palm beside it, or
/// a program late with its request): it is told its drag is cancelled and no icon is drawn, and the next finger reaches the
/// window under it, which is offered nothing.
#[test]
fn a_program_cannot_drag_with_a_finger_that_has_lifted() {
    let _l = limit(60, "a drag with a finger that has lifted");
    let comp = Compositor::start(&["--size", "1000x600"]);
    // the program touched, on the left; the one the next finger goes down on, on the right, which takes text
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.target")));
    s.roundtrip();
    t.roundtrip();
    let (si, ti) = (comp.window("test.source").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    // the first touch gives the seat its touch; both programs then take it
    let (x, y) = (si.at.0 + 100, si.at.1 + 100);
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    s.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    t.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = s.seat.get_touch(&s.qh, Finger);
    let _ttouch = t.touch();
    s.roundtrip();

    // finger 0 down on the window, finger 1 down below it (a palm), finger 0 lifts; then the program drags with finger 0
    comp.must(&format!("touch-down 0 {x} {y}"));
    until(&mut [&mut s], "finger 0 down", |l| l.touched.is_some());
    let serial = log(|l| l.touched.unwrap());
    comp.must(&format!("touch-down 1 {x} 560"));
    comp.must("touch-up 0");
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    device.start_drag(Some(&text(&s, &m)), &sw.surface, Some(&icon(&s, RED)), serial);
    until(&mut [&mut s], "the program told its drag is cancelled", |l| l.cancelled);
    assert_eq!(icon_at(&comp, RGB_RED), None, "no drag's icon");

    // finger 1 lifts; the next finger, on the window on the right, reaches it, and it is offered nothing
    comp.must("touch-up 1");
    t.roundtrip();
    let heard = t.state.touches.len();
    let (tx, ty) = (ti.at.0 + 150, ti.at.1 + 100);
    comp.must(&format!("touch-down 0 {tx} {ty}"));
    comp.must(&format!("touch-move 0 {} {}", tx + 20, ty + 10));
    comp.must("touch-up 0");
    t.until("the window on the right told of the finger", |st| {
        st.touches[heard..].iter().map(|e| e.0).filter(|e| *e != "frame").eq(["down", "motion", "up"])
    });
    s.roundtrip();
    assert!(log(|l| l.entered.is_none() && !l.dropped), "nothing offered to the window under the finger");
}

/// A program that starts its drag again with the same press (the button still held) replaces its first drag: that one is told it
/// is cancelled, and the closed hand stays for the new one until the button is let go.
#[test]
fn a_drag_started_again_with_the_same_press_keeps_the_closed_hand() {
    let _l = limit(60, "a drag started again");
    let comp = Compositor::start(&["--size", "1000x600"]);
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    let si = comp.window("test.source").unwrap();
    drag_from(&comp, &mut s, &sw, si.at, MIME);
    let pointer = comp.must("pointer-look");
    assert!(pointer[0].ends_with("\tgrabbing"), "the drag's closed hand: {pointer:?}");
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    device.start_drag(Some(&text(&s, &m)), &sw.surface, None, s.state.button_serial);
    until(&mut [&mut s], "the first drag told it is cancelled", |l| l.cancelled);
    assert_eq!(comp.must("pointer-look"), pointer, "the closed hand stays for the new drag");
    comp.must("release");
    s.roundtrip();
    let pointer = comp.must("pointer-look");
    assert!(!pointer[0].ends_with("\tgrabbing"), "the closed hand gone with the drag: {pointer:?}");
}

/// A drag whose program goes away (it crashed halfway through) ends with it, though the button is still held: once the pointer
/// moves on, it is no longer the drag's closed hand.
#[test]
fn a_drag_whose_program_goes_away_ends_with_its_closed_hand() {
    let _l = limit(60, "a drag whose program goes away");
    let comp = Compositor::start(&["--size", "1000x600"]);
    let si = {
        let mut s = comp.connect();
        let (_sk, _sp) = s.input();
        let sw = s.window("test.source", "Source");
        s.draw(&sw, (300, 200), 0xff0000ff);
        let si = comp.window("test.source").unwrap();
        drag_from(&comp, &mut s, &sw, si.at, MIME);
        let pointer = comp.must("pointer-look");
        assert!(pointer[0].ends_with("\tgrabbing"), "the drag's closed hand: {pointer:?}");
        si
        // the connection ends here, the button still held (a crash)
    };
    comp.wait(10, "the program's window gone", |comp| comp.window("test.source").is_none());
    comp.must(&format!("pointer {} {}", si.at.0 + 150, si.at.1 + 100));
    let pointer = comp.must("pointer-look");
    assert!(!pointer[0].ends_with("\tgrabbing"), "the closed hand gone with the drag: {pointer:?}");
    comp.must("release");
}

/// A drag made with a finger, which the touchscreen then cancels (libinput's touch cancel: the touch was a palm, or the system
/// took it for a gesture): the drag ends there. Its icon, drawn under the finger, goes with it, the dragging program is told it
/// is cancelled, and the pointer is never the drag's closed hand (the mouse is not the drag's).
#[test]
fn a_finger_drag_the_touchscreen_cancels_ends_with_its_icon() {
    let _l = limit(60, "a finger's drag cancelled");
    let comp = Compositor::start(&["--size", "1000x600"]);
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let si = comp.window("test.source").unwrap();
    let (x, y) = (si.at.0 + 100, si.at.1 + 100);
    // the first touch gives the seat its touch; the program then takes it
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    s.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = s.seat.get_touch(&s.qh, Finger);
    s.roundtrip();

    // a finger down on the window, and the program drags its text with it: its icon under the finger, the pointer as it was
    let pointer = comp.must("pointer-look");
    comp.must(&format!("touch-down 1 {x} {y}"));
    until(&mut [&mut s], "the finger down", |l| l.touched.is_some());
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    device.start_drag(Some(&text(&s, &m)), &sw.surface, Some(&icon(&s, RED)), log(|l| l.touched.unwrap()));
    s.roundtrip();
    assert_eq!(icon_at(&comp, RGB_RED), Some((x, y)), "the drag's icon under the finger");
    assert_eq!(comp.must("pointer-look"), pointer, "the pointer as it was, not a drag's closed hand");

    // the touchscreen cancels the finger: the drag ends, and its icon with it
    comp.must("touch-cancel");
    until(&mut [&mut s], "the dragging program told its drag is cancelled", |l| l.cancelled);
    assert_eq!(icon_at(&comp, RGB_RED), None, "the icon gone with the drag");
    assert_eq!(comp.must("pointer-look"), pointer, "the pointer still as it was");
}

/// A drag made with a finger draws its icon under the finger and keeps it there while the mouse moves (a palm on a 2-in-1's
/// touchpad halfway through): the mouse is not the drag's, and the pointer keeps its own look. The drag goes on: its icon
/// follows the finger, the window the finger goes over is offered it, and the drop, when the finger lifts, ends the icon.
#[test]
fn a_finger_drag_draws_its_icon_under_the_finger_while_the_mouse_moves() {
    let _l = limit(60, "a finger's drag while the mouse moves");
    let comp = Compositor::start(&["--size", "1000x600"]);
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.target")));
    s.roundtrip();
    t.roundtrip();
    let (si, ti) = (comp.window("test.source").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    let (x, y) = (si.at.0 + 100, si.at.1 + 100);
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    s.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = s.seat.get_touch(&s.qh, Finger);
    s.roundtrip();

    // a finger down on the window, and the program drags its text with it: its icon under the finger, which the mouse moving
    // leaves there
    comp.must(&format!("touch-down 1 {x} {y}"));
    until(&mut [&mut s], "the finger down", |l| l.touched.is_some());
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    device.start_drag(Some(&text(&s, &m)), &sw.surface, Some(&icon(&s, RED)), log(|l| l.touched.unwrap()));
    s.roundtrip();
    assert_eq!(icon_at(&comp, RGB_RED), Some((x, y)), "the drag's icon under the finger");
    comp.must("pointer 500 500");
    assert_eq!(icon_at(&comp, RGB_RED), Some((x, y)), "the icon still under the finger while the mouse moves");
    let pointer = comp.must("pointer-look");
    assert!(!pointer[0].ends_with("\tgrabbing"), "the pointer keeps its own look, not the drag's closed hand: {pointer:?}");

    // the finger over the window on the right: the icon follows it, and the window is offered the text, and accepts it
    let (tx, ty) = (ti.at.0 + 150, ti.at.1 + 100);
    comp.must(&format!("touch-move 1 {tx} {ty}"));
    until(&mut [&mut s, &mut t], "the receiver offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == MIME));
    assert_eq!(icon_at(&comp, RGB_RED), Some((tx, ty)), "the icon follows the finger");
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    s.roundtrip();
    // the finger lifts: the drop, and the icon goes with the drag
    comp.must("touch-up 1");
    until(&mut [&mut s, &mut t], "the drop", |l| l.dropped);
    assert_eq!(icon_at(&comp, RGB_RED), None, "the icon gone with the drop");
    assert_eq!(comp.must("pointer-look"), pointer, "the pointer as it was");
}

/// A drag made with the mouse and one made with a finger at the same time (two programs, each dragging out of its own window):
/// each has its own look, the mouse's icon under the pointer with the closed hand, the finger's icon under the finger, and each
/// ends on its own. The finger's lift leaves the mouse's drag with its icon and closed hand; the button let go leaves a finger's
/// drag with its icon under the finger.
#[test]
fn a_mouse_drag_and_a_finger_drag_at_once_each_keep_their_own_look() {
    let _l = limit(60, "a mouse's drag and a finger's at once");
    let comp = Compositor::start(&["--size", "1000x600"]);
    // the program dragged with the mouse, on the left; the one dragged with a finger, on the right
    let mut b = comp.connect();
    let (_bk, _bp) = b.input();
    let bw = b.window("test.mouse", "Mouse");
    b.draw(&bw, (300, 200), 0xff0000ff);
    let mut f = comp.connect();
    let (_fk, _fp) = f.input();
    let fw = f.window("test.finger", "Finger");
    f.draw(&fw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.mouse")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.finger")));
    b.roundtrip();
    f.roundtrip();
    let (bi, fi) = (comp.window("test.mouse").unwrap(), comp.window("test.finger").unwrap());
    let (x, y) = (fi.at.0 + 100, fi.at.1 + 100);
    comp.must(&format!("touch-down 0 {x} {y}"));
    comp.must("touch-up 0");
    f.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = f.seat.get_touch(&f.qh, Finger);
    let bm: WlDataDeviceManager = b.globals.bind(&b.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let bdev: WlDataDevice = bm.get_data_device(&b.seat, &b.qh, ());
    let fm: WlDataDeviceManager = f.globals.bind(&f.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let fdev: WlDataDevice = fm.get_data_device(&f.seat, &f.qh, ());
    b.roundtrip();
    f.roundtrip();
    // a finger down on the right window, and its program drags its text with it, with a magenta icon
    let finger_drag = |f: &mut Client| {
        log(|l| l.touched = None);
        comp.must(&format!("touch-down 1 {x} {y}"));
        until(&mut [f], "the finger down", |l| l.touched.is_some());
        fdev.start_drag(Some(&text(f, &fm)), &fw.surface, Some(&icon(f, MAGENTA)), log(|l| l.touched.unwrap()));
        f.roundtrip();
    };

    // the button held on the left window, and its program drags its text with a red icon; then the finger's drag
    let (px, py) = (bi.at.0 + 100, bi.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    b.until("the pointer on the window", |st| st.pointer_on == Some(bw.surface.id()));
    comp.must("press");
    b.until("the press", |st| st.button_serial != 0);
    bdev.start_drag(Some(&text(&b, &bm)), &bw.surface, Some(&icon(&b, RED)), b.state.button_serial);
    b.roundtrip();
    finger_drag(&mut f);
    assert_eq!(icon_at(&comp, RGB_RED), Some((px, py)), "the mouse's icon under the pointer");
    assert_eq!(icon_at(&comp, RGB_MAGENTA), Some((x, y)), "the finger's icon under the finger");
    let pointer = comp.must("pointer-look");
    assert!(pointer[0].ends_with("\tgrabbing"), "the mouse's drag's closed hand: {pointer:?}");

    // the finger lifts where nothing takes its drag: that drag ends, its icon with it; the mouse's goes on as it was
    comp.must("touch-up 1");
    until(&mut [&mut f], "the finger's drag told it is cancelled", |l| l.cancelled);
    assert_eq!(icon_at(&comp, RGB_MAGENTA), None, "the finger's icon gone with its drag");
    assert_eq!(comp.must("pointer-look"), pointer, "the mouse's closed hand stays");
    comp.must(&format!("pointer {} {}", px + 30, py + 20));
    assert_eq!(icon_at(&comp, RGB_RED), Some((px + 30, py + 20)), "the mouse's icon goes on under the pointer");

    // a finger's drag again, then the button is let go: the mouse's drag ends with its icon and closed hand, the finger's goes on
    log(|l| l.cancelled = false);
    finger_drag(&mut f);
    comp.must("release");
    until(&mut [&mut b], "the mouse's drag told it is cancelled", |l| l.cancelled);
    assert_eq!(icon_at(&comp, RGB_RED), None, "the mouse's icon gone with its drag");
    let after = comp.must("pointer-look");
    assert!(!after[0].ends_with("\tgrabbing"), "the closed hand gone with the mouse's drag: {after:?}");
    assert_eq!(icon_at(&comp, RGB_MAGENTA), Some((x, y)), "the finger's icon still under the finger");
    log(|l| l.cancelled = false);
    comp.must("touch-up 1");
    until(&mut [&mut f], "the finger's drag told it is cancelled", |l| l.cancelled);
    assert_eq!(icon_at(&comp, RGB_MAGENTA), None, "the finger's icon gone with its drag");
}

// ------------------------------------------------------------------ X11 programs (XDND)

/// The XDND version the test's X11 programs speak (the newest).
const XDND: u32 = 5;

/// An X11 program on the compositor's XWayland, with XDND's atoms and the clipboard's.
struct X11 {
    conn: RustConnection,
    root: XWindow,
    aware: Atom,
    enter: Atom,
    position: Atom,
    status: Atom,
    leave: Atom,
    drop: Atom,
    finished: Atom,
    selection: Atom,
    copy: Atom,
    uri: Atom,
    clipboard: Atom,
    targets: Atom,
}

impl X11 {
    fn connect(display: &str) -> X11 {
        let (conn, screen) = x11rb::connect(Some(display)).expect("connect to XWayland");
        let root = conn.setup().roots[screen].root;
        let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).unwrap().reply().unwrap().atom;
        X11 {
            root,
            aware: atom("XdndAware"),
            enter: atom("XdndEnter"),
            position: atom("XdndPosition"),
            status: atom("XdndStatus"),
            leave: atom("XdndLeave"),
            drop: atom("XdndDrop"),
            finished: atom("XdndFinished"),
            selection: atom("XdndSelection"),
            copy: atom("XdndActionCopy"),
            uri: atom(URI_MIME),
            clipboard: atom("CLIPBOARD"),
            targets: atom("TARGETS"),
            conn,
        }
    }

    /// A 300x200 window with `class` as its WM_CLASS, hearing `events`, shown.
    fn window(&self, class: &[u8], events: EventMask) -> XWindow {
        let win = self.conn.generate_id().unwrap();
        let aux = CreateWindowAux::new().background_pixel(0x00ff00).event_mask(events);
        self.conn
            .create_window(x11rb::COPY_DEPTH_FROM_PARENT, win, self.root, 0, 0, 300, 200, 0, WindowClass::INPUT_OUTPUT, 0, &aux)
            .unwrap();
        self.conn.change_property8(PropMode::REPLACE, win, AtomEnum::WM_CLASS, AtomEnum::STRING, class).unwrap();
        self.conn.map_window(win).unwrap();
        self.conn.flush().unwrap();
        win
    }

    /// The XDND version a window takes (its XdndAware), if it takes XDND.
    fn aware_of(&self, w: XWindow) -> Option<u32> {
        self.conn.get_property(false, w, self.aware, AtomEnum::ATOM, 0, 1).ok()?.reply().ok()?.value32()?.next()
    }

    /// The window under the pointer that takes XDND, as an XDND source looks for its target: from the top window there down
    /// (the window manager frames X11 windows, so a program's own window is inside its frame).
    fn target_under_pointer(&self) -> Option<XWindow> {
        let mut w = self.root;
        loop {
            w = self.conn.query_pointer(w).ok()?.reply().ok()?.child;
            if w == x11rb::NONE || self.aware_of(w).is_some() {
                return Some(w).filter(|w| *w != x11rb::NONE);
            }
        }
    }

    /// An XDND message `kind` to the window `to`.
    fn send(&self, to: XWindow, kind: Atom, data: [u32; 5]) {
        self.conn.send_event(false, to, EventMask::NO_EVENT, ClientMessageEvent::new(32, to, kind, data)).unwrap();
        self.conn.flush().unwrap();
    }

    /// Waits (at most 5 s) for the event `want` picks, dispatching the Wayland programs meanwhile; the events before it are
    /// passed over.
    fn until<T>(&self, programs: &mut [&mut Client], what: &str, mut want: impl FnMut(&Event) -> Option<T>) -> T {
        let start = Instant::now();
        loop {
            while let Some(e) = self.conn.poll_for_event().expect("the X11 connection") {
                if let Some(t) = want(&e) {
                    return t;
                }
            }
            for p in programs.iter_mut() {
                p.roundtrip();
            }
            assert!(start.elapsed() < Duration::from_secs(5), "{what}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// The data of the next XDND message `kind` the program is sent.
    fn message(&self, programs: &mut [&mut Client], kind: Atom, what: &str) -> [u32; 5] {
        self.until(programs, what, |e| match e {
            Event::ClientMessage(m) if m.type_ == kind => Some(m.data.as_data32()),
            _ => None,
        })
    }

    /// The time the program hears the pointer move to `at` (on the X screen).
    fn motion(&self, programs: &mut [&mut Client], at: (i32, i32)) -> u32 {
        self.until(programs, &format!("the X11 program hears the pointer at {at:?}"), |e| match e {
            Event::MotionNotify(m) if (m.root_x as i32, m.root_y as i32) == at => Some(m.time),
            _ => None,
        })
    }

    /// The time the program hears a finger move to `at`, give or take a pixel: XWayland passes a finger to a program as the
    /// pointer (`x11_finger_drag_from`), through its touch device's range (0 to 65535 across the screen), which loses one.
    fn finger_motion(&self, programs: &mut [&mut Client], at: (i32, i32)) -> u32 {
        self.until(programs, &format!("the X11 program hears the finger at {at:?}"), |e| match e {
            Event::MotionNotify(m) if (m.root_x as i32 - at.0).abs() <= 1 && (m.root_y as i32 - at.1).abs() <= 1 => Some(m.time),
            _ => None,
        })
    }

    /// The next request for `selection` (XdndSelection: what it drags; the clipboard: what it copied), which this program owns.
    fn asked(&self, programs: &mut [&mut Client], selection: Atom) -> SelectionRequestEvent {
        self.until(programs, "the X11 program asked for what it owns", |e| match e {
            Event::SelectionRequest(r) if r.selection == selection => Some(*r),
            _ => None,
        })
    }

    /// Answers a request for the selection with `data`, as its owner.
    fn give(&self, asked: &SelectionRequestEvent, data: &[u8]) {
        self.conn.change_property8(PropMode::REPLACE, asked.requestor, asked.property, asked.target, data).unwrap();
        self.answered(asked);
    }

    /// Answers a request for the types the selection comes in (TARGETS) with `types`, as its owner.
    fn give_types(&self, asked: &SelectionRequestEvent, types: &[Atom]) {
        self.conn.change_property32(PropMode::REPLACE, asked.requestor, asked.property, AtomEnum::ATOM, types).unwrap();
        self.answered(asked);
    }

    /// Tells the program that asked that the answer is in its property (SelectionNotify).
    fn answered(&self, asked: &SelectionRequestEvent) {
        let given = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: asked.time,
            requestor: asked.requestor,
            selection: asked.selection,
            target: asked.target,
            property: asked.property,
        };
        self.conn.send_event(false, asked.requestor, EventMask::NO_EVENT, given).unwrap();
        self.conn.flush().unwrap();
    }
}

/// The events an X11 program that drags hears: the pointer and its buttons.
fn pointer_events() -> EventMask {
    EventMask::ENTER_WINDOW | EventMask::POINTER_MOTION | EventMask::BUTTON_PRESS | EventMask::BUTTON_RELEASE
}

/// Waits until the X11 window placed at `at` is drawn there (the green X11::window gives it): only then has XWayland made its
/// surface, so the pointer and a drag can be in it.
fn x11_drawn(comp: &Compositor, at: (i32, i32)) {
    comp.wait(10, "the X11 window drawn where it was placed", |c| {
        c.screenshot("HEADLESS-1").count((at.0 + 20, at.1 + 20, 50, 50), (0, 255, 0)) == 50 * 50
    });
}

/// The button held on the X11 program's window `win` (placed at `at`), and the program starts a drag with it
/// (`x11_takes_the_drag`). The stand-in.
fn x11_drag_from(comp: &Compositor, x: &X11, win: XWindow, at: (i32, i32), over: (i32, i32)) -> XWindow {
    x11_drawn(comp, at);
    comp.must(&format!("pointer {} {}", at.0 + 100, at.1 + 100));
    x.until(&mut [], "the pointer on the X11 window", |e| matches!(e, Event::EnterNotify(_) | Event::MotionNotify(_)).then_some(()));
    comp.must("press");
    let pressed = x.until(&mut [], "the press", |e| match e {
        Event::ButtonPress(b) => Some(b.time),
        _ => None,
    });
    x11_takes_the_drag(x, win, pressed, over)
}

/// A finger (slot 1) put down on the X11 program's window `win` (placed at `at`), and the program starts a drag with it
/// (`x11_takes_the_drag`). XWayland passes a finger to a program that does not take touch (XInput 2) as the pointer and its
/// left button, so the program drags as with a mouse. XWayland takes touch once the seat says it has it (from the first finger
/// on): a finger put down before that is lost to it, so a test gives the seat touch before XWayland starts. The stand-in.
fn x11_finger_drag_from(comp: &Compositor, x: &X11, win: XWindow, at: (i32, i32), over: (i32, i32)) -> XWindow {
    x11_drawn(comp, at);
    comp.must(&format!("touch-down 1 {} {}", at.0 + 100, at.1 + 100));
    let pressed = x.until(&mut [], "the finger heard as a press", |e| match e {
        Event::ButtonPress(b) => Some(b.time),
        _ => None,
    });
    x11_takes_the_drag(x, win, pressed, over)
}

/// The X11 program takes XdndSelection with its press (heard at `pressed`), as an X11 program starting a drag does. The window
/// manager's stand-in for the Wayland windows comes: a window of its own that takes XDND, over the whole screen while the drag
/// lasts (looked for at `over`). The stand-in.
fn x11_takes_the_drag(x: &X11, win: XWindow, pressed: u32, over: (i32, i32)) -> XWindow {
    x.conn.set_selection_owner(win, x.selection, pressed).unwrap();
    assert_eq!(x.conn.get_selection_owner(x.selection).unwrap().reply().unwrap().owner, win, "XdndSelection taken");
    let start = Instant::now();
    loop {
        let child = x.conn.translate_coordinates(x.root, x.root, over.0 as i16, over.1 as i16).unwrap().reply().unwrap().child;
        if child != x11rb::NONE && x.aware_of(child).is_some() {
            return child;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "no window that takes XDND at {over:?}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A point on the X screen as XDND gives it: x in the high 16 bits, y in the low ones.
fn xy(x: i32, y: i32) -> u32 {
    ((x as u32) << 16) | (y as u32 & 0xffff)
}

/// What comes through the pipe until it is closed (at most 5 s).
fn read_all(mut from: std::io::PipeReader) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut got = vec![];
        let _ = from.read_to_end(&mut got);
        let _ = tx.send(got);
    });
    rx.recv_timeout(Duration::from_secs(5)).expect("the pipe closed")
}

/// A Wayland program drags a file onto an X11 program, an XDND target: the X11 program is told the drag came and where the
/// pointer is, takes it, and on the drop reads the file's URI through XdndSelection; the Wayland program is asked for it and
/// told the drop finished.
#[test]
fn a_file_dragged_from_a_wayland_program_onto_an_x11_program_arrives_there() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "drag and drop onto an X11 program");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the drop, on the right: its window says it takes XDND (XdndAware)
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    comp.wait(10, "the X11 window managed", |c| c.window("X11target").is_some());
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());
    assert!(si.at.0 + 300 <= xi.at.0, "side by side: {si:?} {xi:?}");
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);

    // over the X11 window: it is told a drag came, offering a URI (XdndEnter), and where the pointer is (XdndPosition)
    x11_drawn(&comp, xi.at);
    let (px, py) = (xi.at.0 + 150, xi.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    let enter = x.message(&mut [&mut s], x.enter, "XdndEnter");
    let source = enter[0];
    assert!(enter[1] >> 24 >= 2, "an XDND version the X11 program takes: {enter:?}");
    assert!(enter[2..].contains(&x.uri), "a URI offered: {enter:?}");
    let position = x.message(&mut [&mut s], x.position, "XdndPosition");
    assert_eq!(position[0], source, "from the source that entered");
    assert_eq!(position[2], xy(px, py), "the pointer where it is on the X screen: {:?}", (position[2] >> 16, position[2] & 0xffff));
    // it takes it, as a copy (XdndStatus): the dragging program is told the action
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));

    // let go over it: the X11 program is told (XdndDrop) and asks for the URI, which the dragging program is asked for
    comp.must("release");
    let dropped = x.message(&mut [&mut s], x.drop, "XdndDrop");
    assert_eq!(dropped[0], source, "from the source that entered");
    let into = x.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.selection, x.uri, into, dropped[2]).unwrap();
    x.conn.flush().unwrap();
    let property = x.until(&mut [&mut s], "the URI given to the X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == win => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = x.conn.get_property(true, win, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    // it is done, the drop taken as a copy (XdndFinished): the dragging program is told the drop finished
    x.send(source, x.finished, [win, 1, x.copy, 0, 0]);
    until(&mut [&mut s], "the dragging program told the drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "not cancelled");
}

/// The same with a finger, as on a 2-in-1's screen: a Wayland program drags a file with a finger onto an X11 program, which is
/// told the drag came and where the finger is, takes it, and once the finger lifts over it is told of the drop and reads the
/// file's URI; the Wayland program is told the drop finished.
#[test]
fn a_file_dragged_with_a_finger_from_a_wayland_program_onto_an_x11_program_arrives_there() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a finger's drag onto an X11 program");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the drop, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left; the first touch gives the seat its touch, and the program then takes it
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());
    let (fx, fy) = (si.at.0 + 100, si.at.1 + 100);
    comp.must(&format!("touch-down 0 {fx} {fy}"));
    comp.must("touch-up 0");
    s.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = s.seat.get_touch(&s.qh, Finger);
    s.roundtrip();

    // a finger down on its window, and the program drags its file with it
    comp.must(&format!("touch-down 1 {fx} {fy}"));
    until(&mut [&mut s], "the finger down", |l| l.touched.is_some());
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    let file: WlDataSource = m.create_data_source(&s.qh, Dragged);
    file.offer(URI_MIME.into());
    file.set_actions(DndAction::Copy);
    device.start_drag(Some(&file), &sw.surface, None, log(|l| l.touched.unwrap()));
    s.roundtrip();

    // the finger over the X11 window: it is told a drag came, offering a URI (XdndEnter), and where the finger is (XdndPosition)
    x11_drawn(&comp, xi.at);
    let (px, py) = (xi.at.0 + 150, xi.at.1 + 100);
    comp.must(&format!("touch-move 1 {px} {py}"));
    let enter = x.message(&mut [&mut s], x.enter, "XdndEnter");
    let source = enter[0];
    assert!(enter[2..].contains(&x.uri), "a URI offered: {enter:?}");
    let position = x.message(&mut [&mut s], x.position, "XdndPosition");
    assert_eq!(position[2], xy(px, py), "the finger where it is on the X screen: {:?}", (position[2] >> 16, position[2] & 0xffff));
    // it takes it, as a copy (XdndStatus): the dragging program is told the action
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));

    // the finger lifts over it: the X11 program is told (XdndDrop) and asks for the URI, which the dragging program is asked for
    comp.must("touch-up 1");
    let dropped = x.message(&mut [&mut s], x.drop, "XdndDrop");
    assert_eq!(dropped[0], source, "from the source that entered");
    let into = x.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.selection, x.uri, into, dropped[2]).unwrap();
    x.conn.flush().unwrap();
    let property = x.until(&mut [&mut s], "the URI given to the X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == win => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = x.conn.get_property(true, win, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    x.send(source, x.finished, [win, 1, x.copy, 0, 0]);
    until(&mut [&mut s], "the dragging program told the drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "not cancelled");
}

/// An X11 program, an XDND source, drags a file onto a Wayland program: it sends its XDND messages to the window under the
/// pointer, the window manager's stand-in, and its drag icon under the pointer is passed over; the Wayland program is offered
/// the URI, takes it, and reads it through the pipe; the X11 program is told the drop finished.
#[test]
fn a_file_dragged_from_an_x11_program_onto_a_wayland_program_arrives_there() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "drag and drop from an X11 program");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program dragging, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11source\0X11source\0", pointer_events());
    comp.wait(10, "the X11 window managed", |c| c.window("X11source").is_some());
    place_x11(&comp, "X11source", (600, 100));
    // the Wayland program that takes the drop, in the top left corner
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 0 0 300 200", id_of(&comp, "test.target")));
    t.roundtrip();
    let (xi, ti) = (comp.window("X11source").unwrap(), comp.window("test.target").unwrap());
    assert!(ti.at.0 + 300 <= xi.at.0, "side by side: {ti:?} {xi:?}");
    let _tdev = data_device(&mut t);
    let (px, py) = (ti.at.0 + 32, ti.at.1 + 32);
    let stand_in = x11_drag_from(&comp, &x, win, xi.at, (px, py));

    // the pointer over the Wayland window: the X11 program follows it and finds the window under it, the stand-in
    comp.must(&format!("pointer {px} {py}"));
    let moved = x.motion(&mut [&mut t], (px, py));
    assert_eq!(x.target_under_pointer(), Some(stand_in), "the window under the pointer");
    let version = x.aware_of(stand_in).unwrap().min(XDND);
    // its drag icon, as GTK and Qt show one: a window of its own over the others, around the pointer; passed over, so the drag
    // goes to the Wayland window beneath
    let icon = x.conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0xff0000).override_redirect(1);
    let (ix, iy) = ((px - 32) as i16, (py - 32) as i16);
    x.conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, icon, x.root, ix, iy, 64, 64, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    x.conn.map_window(icon).unwrap();
    x.conn.flush().unwrap();
    comp.wait(10, "the drag icon drawn under the pointer", |c| {
        c.screenshot("HEADLESS-1").count((px - 2, py - 2, 24, 4), (255, 0, 0)) == 96
    });
    // it tells the stand-in the drag came, offering a URI (XdndEnter), and where (XdndPosition): not taken, nobody was offered it
    x.send(stand_in, x.enter, [win, version << 24, x.uri, 0, 0]);
    x.send(stand_in, x.position, [win, 0, xy(px, py), moved, x.copy]);
    let status = x.message(&mut [&mut t], x.status, "XdndStatus");
    assert_eq!((status[0], status[1] & 1), (stand_in, 0), "not taken before the Wayland program says so: {status:?}");

    // the pointer moves on: the Wayland program is offered the URI, and takes it as a copy
    comp.must(&format!("pointer {} {py}", px + 10));
    until(&mut [&mut t], "the Wayland program offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == URI_MIME));
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(URI_MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    // the X11 program's next position is answered: taken, as a copy
    comp.must(&format!("pointer {} {py}", px + 20));
    let moved = x.motion(&mut [&mut t], (px + 20, py));
    x.send(stand_in, x.position, [win, 0, xy(px + 20, py), moved, x.copy]);
    let status = x.message(&mut [&mut t], x.status, "XdndStatus once the Wayland program took it");
    assert_eq!((status[1] & 1, status[4]), (1, x.copy), "taken as a copy: {status:?}");

    // let go: the X11 program drops (XdndDrop), and the Wayland program is told
    comp.must("release");
    let released = x.until(&mut [&mut t], "the release", |e| match e {
        Event::ButtonRelease(b) => Some(b.time),
        _ => None,
    });
    x.send(stand_in, x.drop, [win, 0, released, 0, 0]);
    until(&mut [&mut t], "the drop", |l| l.dropped);
    // it reads the URI through the pipe: the X11 program is asked for it through XdndSelection, and gives it
    let (read, write) = std::io::pipe().unwrap();
    offer.receive(URI_MIME.into(), write.as_fd());
    drop(write);
    let asked = x.asked(&mut [&mut t], x.selection);
    assert_eq!(asked.target, x.uri, "asked for the URI's type");
    x.give(&asked, URI);
    assert_eq!(read_all(read), URI, "what was dropped arrived");
    // it is done: the X11 program is told the drop finished, taken as a copy (XdndFinished)
    offer.finish();
    t.roundtrip();
    let finished = x.message(&mut [&mut t], x.finished, "XdndFinished");
    assert_eq!((finished[0], finished[1] & 1, finished[2]), (stand_in, 1, x.copy), "the drop finished: {finished:?}");
}

/// The same with a finger: an X11 program drags a file with a finger onto a Wayland program. It hears the finger as the pointer
/// and its left button (`x11_finger_drag_from`), so it finds the stand-in under the finger and speaks XDND to it as with a
/// mouse; the Wayland program is offered the URI where the finger is and takes it; the finger's lift reaches the X11 program as
/// the release it drops on (XWayland keeps the finger down until it is told); the URI arrives through the pipe and the X11
/// program is told the drop finished.
#[test]
fn a_file_dragged_with_a_finger_from_an_x11_program_onto_a_wayland_program_arrives_there() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a finger's drag from an X11 program");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the seat gets touch from a first finger before XWayland starts (with the first X11 program), which takes it as it starts
    comp.must("touch-down 0 500 500");
    comp.must("touch-up 0");
    // the X11 program dragging, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11source\0X11source\0", pointer_events());
    place_x11(&comp, "X11source", (600, 100));
    // the Wayland program that takes the drop, in the top left corner
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 0 0 300 200", id_of(&comp, "test.target")));
    t.roundtrip();
    let (xi, ti) = (comp.window("X11source").unwrap(), comp.window("test.target").unwrap());
    assert!(ti.at.0 + 300 <= xi.at.0, "side by side: {ti:?} {xi:?}");
    let _tdev = data_device(&mut t);
    let (px, py) = (ti.at.0 + 32, ti.at.1 + 32);
    let stand_in = x11_finger_drag_from(&comp, &x, win, xi.at, (px, py));

    // the finger over the Wayland window: the X11 program follows it and finds the window under it, the stand-in
    comp.must(&format!("touch-move 1 {px} {py}"));
    let moved = x.finger_motion(&mut [&mut t], (px, py));
    assert_eq!(x.target_under_pointer(), Some(stand_in), "the window under the finger");
    let version = x.aware_of(stand_in).unwrap().min(XDND);
    // it tells the stand-in the drag came, offering a URI (XdndEnter), and where (XdndPosition): not taken, nobody was offered it
    x.send(stand_in, x.enter, [win, version << 24, x.uri, 0, 0]);
    x.send(stand_in, x.position, [win, 0, xy(px, py), moved, x.copy]);
    let status = x.message(&mut [&mut t], x.status, "XdndStatus");
    assert_eq!((status[0], status[1] & 1), (stand_in, 0), "not taken before the Wayland program says so: {status:?}");

    // the finger moves on: the Wayland program is offered the URI, and takes it as a copy
    comp.must(&format!("touch-move 1 {} {py}", px + 10));
    until(&mut [&mut t], "the Wayland program offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == URI_MIME));
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(URI_MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    // the X11 program's next position is answered: taken, as a copy
    comp.must(&format!("touch-move 1 {} {py}", px + 20));
    let moved = x.finger_motion(&mut [&mut t], (px + 20, py));
    x.send(stand_in, x.position, [win, 0, xy(px + 20, py), moved, x.copy]);
    let status = x.message(&mut [&mut t], x.status, "XdndStatus once the Wayland program took it");
    assert_eq!((status[1] & 1, status[4]), (1, x.copy), "taken as a copy: {status:?}");

    // the finger lifts: the X11 program hears it as the release and drops (XdndDrop), and the Wayland program is told
    comp.must("touch-up 1");
    let released = x.until(&mut [&mut t], "the lift heard as the release", |e| match e {
        Event::ButtonRelease(b) => Some(b.time),
        _ => None,
    });
    x.send(stand_in, x.drop, [win, 0, released, 0, 0]);
    until(&mut [&mut t], "the drop", |l| l.dropped);
    // it reads the URI through the pipe: the X11 program is asked for it through XdndSelection, and gives it
    let (read, write) = std::io::pipe().unwrap();
    offer.receive(URI_MIME.into(), write.as_fd());
    drop(write);
    let asked = x.asked(&mut [&mut t], x.selection);
    assert_eq!(asked.target, x.uri, "asked for the URI's type");
    x.give(&asked, URI);
    assert_eq!(read_all(read), URI, "what was dropped arrived");
    // it is done: the X11 program is told the drop finished, taken as a copy (XdndFinished)
    offer.finish();
    t.roundtrip();
    let finished = x.message(&mut [&mut t], x.finished, "XdndFinished");
    assert_eq!((finished[0], finished[1] & 1, finished[2]), (stand_in, 1, x.copy), "the drop finished: {finished:?}");
}

/// Two X11 programs drag a file between them as on any X server: over an X11 window the window manager's stand-in steps
/// aside, so the programs speak XDND to each other, and the URI arrives.
#[test]
fn a_file_dragged_between_two_x11_programs_arrives_there() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "drag and drop between X11 programs");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program dragging, on the left, and the one taking the drop, on the right (it takes XDND)
    let x = X11::connect(&display);
    let win = x.window(b"x11source\0X11source\0", pointer_events());
    let y = X11::connect(&display);
    let target = y.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    y.conn.change_property32(PropMode::REPLACE, target, y.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    y.conn.flush().unwrap();
    comp.wait(10, "the X11 windows managed", |c| c.window("X11source").is_some() && c.window("X11target").is_some());
    place_x11(&comp, "X11source", (50, 100));
    place_x11(&comp, "X11target", (600, 100));
    let (xi, yi) = (comp.window("X11source").unwrap(), comp.window("X11target").unwrap());
    assert!(xi.at.0 + 300 <= yi.at.0, "side by side: {xi:?} {yi:?}");
    x11_drawn(&comp, yi.at);
    let (px, py) = (yi.at.0 + 150, yi.at.1 + 100);
    let stand_in = x11_drag_from(&comp, &x, win, xi.at, (px, py));

    // over the X11 window the source finds the stand-in under the pointer at first, and tells it the drag came
    comp.must(&format!("pointer {px} {py}"));
    let moved = x.motion(&mut [], (px, py));
    assert_eq!(x.target_under_pointer(), Some(stand_in), "the window under the pointer");
    x.send(stand_in, x.enter, [win, XDND << 24, x.uri, 0, 0]);
    x.send(stand_in, x.position, [win, 0, xy(px, py), moved, x.copy]);
    x.message(&mut [], x.status, "XdndStatus from the stand-in");
    // the pointer moves on: the stand-in steps aside over an X11 window, and the source finds the target's own window
    comp.must(&format!("pointer {} {py}", px + 10));
    let moved = x.motion(&mut [], (px + 10, py));
    let start = Instant::now();
    while x.target_under_pointer() != Some(target) {
        assert!(start.elapsed() < Duration::from_secs(5), "under the pointer: {:?}, not the target", x.target_under_pointer());
        std::thread::sleep(Duration::from_millis(20));
    }
    // so the programs speak XDND to each other: the source leaves the stand-in and enters the target, which takes it
    x.send(stand_in, x.leave, [win, 0, 0, 0, 0]);
    x.send(target, x.enter, [win, XDND << 24, x.uri, 0, 0]);
    x.send(target, x.position, [win, 0, xy(px + 10, py), moved, x.copy]);
    let enter = y.message(&mut [], y.enter, "XdndEnter at the target");
    assert_eq!(enter[0], win, "from the source itself");
    y.message(&mut [], y.position, "XdndPosition at the target");
    y.send(win, y.status, [target, 1, 0, 0, y.copy]);
    let status = x.message(&mut [], x.status, "the target's XdndStatus");
    assert_eq!((status[0], status[1] & 1), (target, 1), "taken by the target: {status:?}");

    // let go: the target is told (XdndDrop) and asks the source itself for the URI
    comp.must("release");
    let released = x.until(&mut [], "the release", |e| match e {
        Event::ButtonRelease(b) => Some(b.time),
        _ => None,
    });
    x.send(target, x.drop, [win, 0, released, 0, 0]);
    let dropped = y.message(&mut [], y.drop, "XdndDrop at the target");
    let into = y.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    y.conn.convert_selection(target, y.selection, y.uri, into, dropped[2]).unwrap();
    y.conn.flush().unwrap();
    let asked = x.asked(&mut [], x.selection);
    assert_eq!(asked.requestor, target, "asked by the target itself");
    x.give(&asked, URI);
    let property = y.until(&mut [], "the URI given to the target", |e| match e {
        Event::SelectionNotify(n) if n.requestor == target => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = y.conn.get_property(true, target, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    y.send(win, y.finished, [target, 1, y.copy, 0, 0]);
    let finished = x.message(&mut [], x.finished, "XdndFinished");
    assert_eq!((finished[0], finished[1] & 1), (target, 1), "the drop finished: {finished:?}");
}

/// XWayland goes away (it crashed, or was killed) while a Wayland program drags over an X11 window: the window goes with it,
/// and the drag goes on to the Wayland window the pointer moves to, where the drop arrives.
#[test]
fn a_drag_over_an_x11_window_goes_on_when_xwayland_goes_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drag over an X11 window when XWayland goes away");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes XDND, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    comp.wait(10, "the X11 window managed", |c| c.window("X11target").is_some());
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left, and the one receiving, below it
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    comp.must(&format!("place {} 50 350 300 200", id_of(&comp, "test.target")));
    s.roundtrip();
    t.roundtrip();
    let (si, ti, xi) = (comp.window("test.source").unwrap(), comp.window("test.target").unwrap(), comp.window("X11target").unwrap());
    let _tdev = data_device(&mut t);
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    // over the X11 window: it is told the drag came
    x11_drawn(&comp, xi.at);
    comp.must(&format!("pointer {} {}", xi.at.0 + 150, xi.at.1 + 100));
    x.message(&mut [&mut s], x.enter, "XdndEnter");

    // XWayland goes away, and the X11 window with it
    let xwayland = xwayland_of(&comp).expect("an Xwayland");
    unsafe { libc::kill(xwayland as i32, libc::SIGKILL) };
    comp.wait(10, "the X11 window gone", |c| c.window("X11target").is_none());
    // the drag goes on over the Wayland window: it is offered the URI, takes it, and reads it on the drop
    comp.must(&format!("pointer {} {}", ti.at.0 + 150, ti.at.1 + 100));
    until(&mut [&mut s, &mut t], "the receiver offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == URI_MIME));
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(URI_MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    s.roundtrip();
    comp.must("release");
    until(&mut [&mut s, &mut t], "the drop", |l| l.dropped);
    let (read, write) = std::io::pipe().unwrap();
    offer.receive(URI_MIME.into(), write.as_fd());
    drop(write);
    until(&mut [&mut s, &mut t], "the dragging program asked for the URI", |l| l.sent > 0);
    assert_eq!(read_all(read), URI, "what was dropped arrived");
    offer.finish();
    until(&mut [&mut s, &mut t], "the dragging program told the drop finished", |l| l.finished);
}

/// XWayland goes away while an X11 program drags over another X11 window: the drag ends with it, and the pointer goes on to a
/// Wayland window as ever.
#[test]
fn an_x11_drag_ends_when_xwayland_goes_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 drag when XWayland goes away");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program dragging, on the left, the one it drags to, on the right, and a Wayland window below that
    let x = X11::connect(&display);
    let win = x.window(b"x11source\0X11source\0", pointer_events());
    let y = X11::connect(&display);
    let target = y.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    y.conn.change_property32(PropMode::REPLACE, target, y.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    y.conn.flush().unwrap();
    comp.wait(10, "the X11 windows managed", |c| c.window("X11source").is_some() && c.window("X11target").is_some());
    place_x11(&comp, "X11source", (50, 100));
    place_x11(&comp, "X11target", (600, 100));
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 600 350 300 200", id_of(&comp, "test.target")));
    t.roundtrip();
    let (xi, yi, ti) = (comp.window("X11source").unwrap(), comp.window("X11target").unwrap(), comp.window("test.target").unwrap());
    x11_drawn(&comp, yi.at);
    let (px, py) = (yi.at.0 + 150, yi.at.1 + 100);
    let stand_in = x11_drag_from(&comp, &x, win, xi.at, (px, py));
    // over the other X11 window the stand-in steps aside: the drag's target is that window itself
    comp.must(&format!("pointer {px} {py}"));
    let moved = x.motion(&mut [], (px, py));
    x.send(stand_in, x.enter, [win, XDND << 24, x.uri, 0, 0]);
    x.send(stand_in, x.position, [win, 0, xy(px, py), moved, x.copy]);
    x.message(&mut [], x.status, "XdndStatus from the stand-in");
    comp.must(&format!("pointer {} {py}", px + 10));
    x.motion(&mut [], (px + 10, py));
    let start = Instant::now();
    while x.target_under_pointer() != Some(target) {
        assert!(start.elapsed() < Duration::from_secs(5), "under the pointer: {:?}, not the target", x.target_under_pointer());
        std::thread::sleep(Duration::from_millis(20));
    }

    // XWayland goes away, and the X11 windows with it
    let xwayland = xwayland_of(&comp).expect("an Xwayland");
    unsafe { libc::kill(xwayland as i32, libc::SIGKILL) };
    comp.wait(10, "the X11 windows gone", |c| c.window("X11source").is_none() && c.window("X11target").is_none());
    // the pointer moves on to the Wayland window and the button is let go: the drag is over, and the window has the pointer
    comp.must(&format!("pointer {} {}", ti.at.0 + 150, ti.at.1 + 100));
    comp.must("release");
    comp.must(&format!("pointer {} {}", ti.at.0 + 160, ti.at.1 + 100));
    t.until("the pointer on the Wayland window", |st| st.pointer_on == Some(tw.surface.id()));
}

/// XWayland goes away while a Wayland program's drag is over an X11 window that said it takes it, and the button is let go
/// there without the pointer moving: nothing is there any more to take the drop, so the dragging program is told the drag is
/// cancelled (it does not wait for a finish that cannot come).
#[test]
fn a_drag_let_go_where_an_x11_window_was_is_cancelled() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop where an X11 window was");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes XDND, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    comp.wait(10, "the X11 window managed", |c| c.window("X11target").is_some());
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    // over the X11 window, which takes it
    x11_drawn(&comp, xi.at);
    comp.must(&format!("pointer {} {}", xi.at.0 + 150, xi.at.1 + 100));
    let source = x.message(&mut [&mut s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));

    // XWayland goes away, and the X11 window with it; the button is let go where it was
    let xwayland = xwayland_of(&comp).expect("an Xwayland");
    unsafe { libc::kill(xwayland as i32, libc::SIGKILL) };
    comp.wait(10, "the X11 window gone", |c| c.window("X11target").is_none());
    comp.must("release");
    until(&mut [&mut s], "the dragging program told the drag is cancelled", |l| l.cancelled);
    assert!(!log(|l| l.finished), "not finished: nothing took the drop");
}

/// An X11 program (`win`, its window at `from`) drags over the Wayland program `t`'s window (at `over`), which is offered the
/// drag and takes it; then the X11 program gives the drag up while the button is still held (on Escape it leaves the window it
/// spoke to: XdndLeave), and the button is let go over the Wayland window.
fn give_an_x11_drag_up(comp: &Compositor, x: &X11, win: XWindow, from: (i32, i32), t: &mut Client, over: (i32, i32)) {
    let (px, py) = (over.0 + 150, over.1 + 100);
    let stand_in = x11_drag_from(comp, x, win, from, (px, py));
    comp.must(&format!("pointer {px} {py}"));
    let moved = x.motion(&mut [&mut *t], (px, py));
    x.send(stand_in, x.enter, [win, XDND << 24, x.uri, 0, 0]);
    x.send(stand_in, x.position, [win, 0, xy(px, py), moved, x.copy]);
    x.message(&mut [&mut *t], x.status, "XdndStatus");
    comp.must(&format!("pointer {} {py}", px + 10));
    until(&mut [&mut *t], "the Wayland program offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == URI_MIME));
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(URI_MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    // Escape: the X11 program leaves the stand-in. The window manager reads its X events in order, so once it manages a window
    // the program shows after that, it has read the leave too; then the button is let go.
    x.send(stand_in, x.leave, [win, 0, 0, 0, 0]);
    x.window(b"after\0After\0", EventMask::NO_EVENT);
    comp.wait(10, "the window shown after the leave managed", |c| c.window("After").is_some());
    place_x11(comp, "After", (600, 350));
    comp.must("release");
}

/// An X11 program gives its drag up while the button is still held, and the button is let go over a Wayland window
/// (`give_an_x11_drag_up`). An X11 menu shown under the pointer afterwards gets the pointer: with no button held no drag goes
/// on, so nothing is passed over as a drag's icon.
#[test]
fn an_x11_menu_gets_the_pointer_after_an_x11_drag_was_given_up() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 menu after a drag given up");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program dragging, on the right; the Wayland program it drags over, in the top left corner
    let x = X11::connect(&display);
    let win = x.window(b"x11source\0X11source\0", pointer_events());
    comp.wait(10, "the X11 window managed", |c| c.window("X11source").is_some());
    place_x11(&comp, "X11source", (600, 100));
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 0 0 300 200", id_of(&comp, "test.target")));
    t.roundtrip();
    let (xi, ti) = (comp.window("X11source").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    give_an_x11_drag_up(&comp, &x, win, xi.at, &mut t, ti.at);

    // another program's X11 menu over the Wayland window, and the pointer onto it: the menu has it
    let m = X11::connect(&display);
    let menu = m.conn.generate_id().unwrap();
    let aux = CreateWindowAux::new().background_pixel(0xff0000).override_redirect(1).event_mask(pointer_events());
    m.conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, menu, m.root, 0, 0, 120, 120, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    m.conn.map_window(menu).unwrap();
    m.conn.flush().unwrap();
    comp.wait(10, "the menu drawn", |c| c.screenshot("HEADLESS-1").count((10, 10, 20, 20), (255, 0, 0)) == 400);
    comp.must("pointer 40 40");
    m.until(&mut [&mut t], "the menu hears the pointer", |e| matches!(e, Event::EnterNotify(_) | Event::MotionNotify(_)).then_some(()));
}

/// After an X11 program gave a drag up (`give_an_x11_drag_up`), its next drag is one again: the window manager's stand-in comes
/// over the Wayland window, which is offered the URI.
#[test]
fn an_x11_program_drags_again_after_it_gave_a_drag_up() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 drag after one given up");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    let x = X11::connect(&display);
    let win = x.window(b"x11source\0X11source\0", pointer_events());
    comp.wait(10, "the X11 window managed", |c| c.window("X11source").is_some());
    place_x11(&comp, "X11source", (600, 100));
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 0 0 300 200", id_of(&comp, "test.target")));
    t.roundtrip();
    let (xi, ti) = (comp.window("X11source").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    give_an_x11_drag_up(&comp, &x, win, xi.at, &mut t, ti.at);

    // its next drag, from its window again: the stand-in comes, and the Wayland window is offered the URI
    log(|l| *l = Log::default());
    let (px, py) = (ti.at.0 + 150, ti.at.1 + 100);
    let stand_in = x11_drag_from(&comp, &x, win, xi.at, (px, py));
    comp.must(&format!("pointer {px} {py}"));
    let moved = x.motion(&mut [&mut t], (px, py));
    x.send(stand_in, x.enter, [win, XDND << 24, x.uri, 0, 0]);
    x.send(stand_in, x.position, [win, 0, xy(px, py), moved, x.copy]);
    x.message(&mut [&mut t], x.status, "XdndStatus");
    comp.must(&format!("pointer {} {py}", px + 10));
    until(&mut [&mut t], "the Wayland program offered the next drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == URI_MIME));
    comp.must("release");
}

/// A Wayland program drags a file onto an X11 program, which says it takes it; the X11 program then ends (it crashed, or hung
/// and was killed) without saying it finished (XdndFinished): after the drop (`dropped`), or while the drag is still over it,
/// the button then let go there. Either way the dragging program is told the drag is over (cancelled), and the next X11
/// program's drag works: it keeps XdndSelection, and the stand-in comes.
fn an_x11_target_ends_unfinished(dropped: bool) {
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the drop, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    comp.wait(10, "the X11 window managed", |c| c.window("X11target").is_some());
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    x11_drawn(&comp, xi.at);
    comp.must(&format!("pointer {} {}", xi.at.0 + 150, xi.at.1 + 100));
    let source = x.message(&mut [&mut s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));
    // the next X11 program, which drags afterwards
    let a = X11::connect(&display);
    if dropped {
        comp.must("release");
        x.message(&mut [&mut s], x.drop, "XdndDrop");
        drop(x);
    } else {
        drop(x);
        // once the X server has destroyed its window, a window shown after that is managed only after the window manager read
        // of the destruction (it reads its X events in order); the button is let go only then
        let start = Instant::now();
        while a.conn.get_geometry(win).unwrap().reply().is_ok() {
            assert!(start.elapsed() < Duration::from_secs(5), "the X11 program's window not destroyed");
            std::thread::sleep(Duration::from_millis(20));
        }
        a.window(b"after\0After\0", EventMask::NO_EVENT);
        comp.wait(10, "the window shown after it managed", |c| c.window("After").is_some());
        place_x11(&comp, "After", (50, 350));
        comp.must("release");
    }
    until(&mut [&mut s], "the dragging program told the drag is over", |l| l.cancelled);

    // the next X11 program's drag: it keeps XdndSelection, and the stand-in comes over the Wayland window
    let awin = a.window(b"x11source\0X11source\0", pointer_events());
    comp.wait(10, "the next X11 program's window managed", |c| c.window("X11source").is_some());
    place_x11(&comp, "X11source", (600, 350));
    let ai = comp.window("X11source").unwrap();
    x11_drag_from(&comp, &a, awin, ai.at, (si.at.0 + 150, si.at.1 + 100));
    assert_eq!(a.conn.get_selection_owner(a.selection).unwrap().reply().unwrap().owner, awin, "XdndSelection kept");
    comp.must("release");
}

#[test]
fn a_drop_on_an_x11_program_that_ends_unfinished_does_not_stop_later_drags() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop left unfinished by an X11 program");
    an_x11_target_ends_unfinished(true);
}

#[test]
fn a_drag_over_an_x11_program_that_ends_is_cancelled_on_the_drop() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drag over an X11 program that ends");
    an_x11_target_ends_unfinished(false);
}

/// The Wayland program `s` (its window `sw` at `from`) drags a file onto the X11 program `x`'s window `win` (at `to`), which
/// takes it as a copy, and lets go there: the X11 program is told (XdndDrop). The source window the X11 program speaks XDND to
/// (the window manager's), and XdndDrop's data.
fn drop_on_x11(
    comp: &Compositor,
    s: &mut Client,
    sw: &Window,
    from: (i32, i32),
    x: &X11,
    win: XWindow,
    to: (i32, i32),
) -> (XWindow, [u32; 5]) {
    drag_from(comp, s, sw, from, URI_MIME);
    x11_drawn(comp, to);
    comp.must(&format!("pointer {} {}", to.0 + 150, to.1 + 100));
    let source = x.message(&mut [&mut *s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut *s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut *s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));
    comp.must("release");
    (source, x.message(&mut [&mut *s], x.drop, "XdndDrop"))
}

/// A Wayland program drops a file on an X11 program, which takes it, reads it, and then hangs without ending: it never says it
/// finished (XdndFinished). The window manager waits 10 s from the drop for that, then takes the X11 program for hung: the
/// dragging program is told the drop is cancelled (not finished: had it offered a move, it would keep its file), the X11
/// program is told nothing more (after XdndDrop only it speaks: no XdndLeave), a finish sent after that is not passed on, and
/// the next X11 program's drag works (while a drop waits for its finish, the window manager takes XdndSelection back from any
/// X11 program that takes it).
#[test]
fn a_drop_an_x11_program_never_finishes_is_cancelled_after_10_seconds() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop an X11 program never finishes");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the drop, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());

    // the file dropped on the X11 program, which asks for the URI and reads it, and hangs: no XdndFinished. The drop was made
    // before its XdndDrop came
    let (source, dropped) = drop_on_x11(&comp, &mut s, &sw, si.at, &x, win, xi.at);
    let let_go = Instant::now();
    let into = x.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.selection, x.uri, into, dropped[2]).unwrap();
    x.conn.flush().unwrap();
    let property = x.until(&mut [&mut s], "the URI given to the X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == win => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = x.conn.get_property(true, win, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    s.roundtrip();
    assert!(!log(|l| l.cancelled || l.finished), "the drop waits for the X11 program's finish");

    // 10 s after the drop, the dragging program is told the drop is cancelled
    until_in(20, &mut [&mut s], "the dragging program told the drop is cancelled", |l| l.cancelled);
    let waited = let_go.elapsed();
    assert!(waited >= Duration::from_secs(9), "cancelled only once the X11 program had its time: after {waited:?}");
    assert!(!log(|l| l.finished), "not finished");
    // the X11 program's finish, late: not passed on. Once the window manager manages a window the program shows after it, it
    // has read the finish too (it reads the X events in order)
    x.send(source, x.finished, [win, 1, x.copy, 0, 0]);
    x.window(b"after\0After\0", EventMask::NO_EVENT);
    place_x11(&comp, "After", (50, 350));
    s.roundtrip();
    assert!(!log(|l| l.finished), "a finish after the drop was cancelled is not passed on");
    // the X11 program was told nothing since the drop (a reply comes after every event sent to the program before it)
    x.conn.get_input_focus().unwrap().reply().unwrap();
    while let Some(e) = x.conn.poll_for_event().unwrap() {
        assert!(!matches!(&e, Event::ClientMessage(m) if m.type_ == x.leave), "the drag told left after its drop: {e:?}");
    }

    // the next X11 program's drag: it keeps XdndSelection, and the stand-in comes over the Wayland window
    let a = X11::connect(&display);
    let awin = a.window(b"x11source\0X11source\0", pointer_events());
    place_x11(&comp, "X11source", (600, 350));
    let ai = comp.window("X11source").unwrap();
    x11_drag_from(&comp, &a, awin, ai.at, (si.at.0 + 150, si.at.1 + 100));
    assert_eq!(a.conn.get_selection_owner(a.selection).unwrap().reply().unwrap().owner, awin, "XdndSelection kept");
    comp.must("release");
}

/// A Wayland program drops a file on an X11 program that took the drag, but hangs before it answers where the drag went last
/// (no XdndStatus for the last XdndPosition). The window manager holds the drop back until that answer, and waits 10 s from
/// the drop for it: then the dragging program is told the drop is cancelled, and the X11 program, never told of the drop, is
/// told the drag left (XdndLeave), so it does not keep a drag it believes is still over it.
#[test]
fn a_drop_an_x11_program_never_answers_is_called_off_after_10_seconds() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop an X11 program never answers");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the drag, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());

    // the file dragged over the X11 program, which takes it; the pointer moves on over it, and the X11 program hangs before it
    // answers that position; the button is let go
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    x11_drawn(&comp, xi.at);
    let (px, py) = (xi.at.0 + 150, xi.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    let source = x.message(&mut [&mut s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));
    comp.must(&format!("pointer {} {py}", px + 10));
    x.message(&mut [&mut s], x.position, "the next XdndPosition, never answered");
    comp.must("release");
    let let_go = Instant::now();
    s.roundtrip();
    assert!(!log(|l| l.cancelled || l.finished), "the drop waits for the X11 program's answer");

    // 10 s after the drop, the dragging program is told the drop is cancelled, and the X11 program that the drag left
    until_in(20, &mut [&mut s], "the dragging program told the drop is cancelled", |l| l.cancelled);
    let waited = let_go.elapsed();
    assert!(waited >= Duration::from_secs(9), "cancelled only once the X11 program had its time: after {waited:?}");
    assert!(!log(|l| l.finished), "not finished");
    let (told, data) = x.until(&mut [&mut s], "the X11 program told the drag left", |e| match e {
        Event::ClientMessage(m) if m.type_ == x.leave || m.type_ == x.drop => Some((m.type_, m.data.as_data32())),
        _ => None,
    });
    assert_eq!(told, x.leave, "told the drag left, never of a drop: {data:?}");
    assert_eq!(data[0], source, "by the source that entered");
}

/// A Wayland program drops a file on an X11 program that never finishes the drop, and drags another file onto another X11
/// program before the 10 s are up. XdndSelection serves the new drag from then on (the first X11 program could no longer read
/// its own drop), so the first drop is over at once and its program is told it is cancelled. The first drop's 10 s do not end
/// the new drag: held over the other X11 program past them, it is still taken there, and its drop arrives and finishes.
#[test]
fn a_drop_still_waiting_on_an_x11_program_ends_when_the_next_drag_comes() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop still waiting when the next drag comes");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the first drop, top right, and the one that takes the second, below it
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 50));
    let y = X11::connect(&display);
    let other = y.window(b"x11other\0X11other\0", EventMask::NO_EVENT);
    y.conn.change_property32(PropMode::REPLACE, other, y.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    y.conn.flush().unwrap();
    place_x11(&comp, "X11other", (600, 350));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi, yi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap(), comp.window("X11other").unwrap());

    // the first file dropped on the first X11 program, which never finishes it
    let first = Instant::now();
    drop_on_x11(&comp, &mut s, &sw, si.at, &x, win, xi.at);

    // the second file over the other X11 program: it is told the drag came, and the first drop is over at once
    log(|l| *l = Log::default());
    s.state.button_serial = 0;
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    x11_drawn(&comp, yi.at);
    let (px, py) = (yi.at.0 + 150, yi.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    let source = y.message(&mut [&mut s], y.enter, "XdndEnter at the other X11 program")[0];
    until(&mut [&mut s], "the first drop's program told it is cancelled", |l| l.cancelled);
    assert!(first.elapsed() < Duration::from_secs(5), "cancelled when the next drag came, not after 10 s: {:?}", first.elapsed());
    log(|l| l.cancelled = false);
    y.message(&mut [&mut s], y.position, "XdndPosition at the other X11 program");
    y.send(source, y.status, [other, 1, 0, 0, y.copy]);
    until(&mut [&mut s], "the dragging program told the other X11 program's choice", |l| l.action == Some(DndAction::Copy));

    // held there until the first drop's 10 s are over: the drag is still taken, and its drop arrives and finishes
    std::thread::sleep(Duration::from_secs(11).saturating_sub(first.elapsed()));
    comp.must(&format!("pointer {} {py}", px + 10));
    y.message(&mut [&mut s], y.position, "XdndPosition after the first drop's 10 s");
    y.send(source, y.status, [other, 1, 0, 0, y.copy]);
    comp.must("release");
    let dropped = y.message(&mut [&mut s], y.drop, "XdndDrop at the other X11 program");
    let into = y.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    y.conn.convert_selection(other, y.selection, y.uri, into, dropped[2]).unwrap();
    y.conn.flush().unwrap();
    let property = y.until(&mut [&mut s], "the URI given to the other X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == other => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = y.conn.get_property(true, other, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    y.send(source, y.finished, [other, 1, y.copy, 0, 0]);
    until(&mut [&mut s], "the dragging program told the second drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "neither drop cancelled since the second drag came");
}

/// A Wayland program drops a file on an X11 program that took the drag but hangs before it answers where the drag went last,
/// and drags another file over the same X11 program before the 10 s are up. The first drop, held back, is over at once: its
/// program is told it is cancelled, and the X11 program is told that drag left (XdndLeave) before it is told the next one
/// came (XdndEnter; the other way around, it would take the next drag for gone). The next drag is taken there, and its drop
/// arrives and finishes.
#[test]
fn a_drop_an_x11_program_never_answered_ends_when_the_next_drag_comes_over_it() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop never answered when the next drag comes");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes both drags, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());

    // the first file over the X11 program, which takes it; the pointer moves on over it, and the button is let go before the
    // X11 program answers that position
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    x11_drawn(&comp, xi.at);
    let (px, py) = (xi.at.0 + 150, xi.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    let source = x.message(&mut [&mut s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));
    comp.must(&format!("pointer {} {py}", px + 10));
    x.message(&mut [&mut s], x.position, "the next XdndPosition, not answered");
    comp.must("release");
    let first = Instant::now();

    // the second file over the same X11 program: it is told the first drag left, then that the second came, and the first
    // drop's program is told it is cancelled
    log(|l| *l = Log::default());
    s.state.button_serial = 0;
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    comp.must(&format!("pointer {px} {py}"));
    let told: Vec<Atom> = (0..2)
        .map(|_| {
            x.until(&mut [&mut s], "the X11 program told the first drag left and the second came", |e| match e {
                Event::ClientMessage(m) if [x.leave, x.enter, x.drop].contains(&m.type_) => Some(m.type_),
                _ => None,
            })
        })
        .collect();
    assert_eq!(told, [x.leave, x.enter], "the first drag left before the second came (leave {}, enter {})", x.leave, x.enter);
    until(&mut [&mut s], "the first drop's program told it is cancelled", |l| l.cancelled);
    assert!(first.elapsed() < Duration::from_secs(5), "cancelled when the next drag came, not after 10 s: {:?}", first.elapsed());
    log(|l| l.cancelled = false);

    // the second drag taken there: its drop arrives and finishes
    x.message(&mut [&mut s], x.position, "XdndPosition of the second drag");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice again", |l| l.action == Some(DndAction::Copy));
    comp.must("release");
    let dropped = x.message(&mut [&mut s], x.drop, "XdndDrop of the second drag");
    let into = x.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.selection, x.uri, into, dropped[2]).unwrap();
    x.conn.flush().unwrap();
    let property = x.until(&mut [&mut s], "the URI given to the X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == win => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = x.conn.get_property(true, win, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    x.send(source, x.finished, [win, 1, x.copy, 0, 0]);
    until(&mut [&mut s], "the dragging program told the second drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "the second drop not cancelled");
}

/// A Wayland program's drag over an X11 program that takes it, which then ends (it crashed, or hung and was killed) with the
/// drag still over it; the button is let go there, where its window was: the drag is cancelled at once, and that drop waits
/// for nothing. Nor does it end the next drag: held over another X11 program for more than 10 s after it (the time the fork's
/// fix gives an X11 program to finish a drop: a_drop_an_x11_program_never_finishes_is_cancelled_after_10_seconds), that drag
/// is still taken there, and its drop arrives and finishes.
#[test]
fn the_next_drag_is_not_cancelled_10_seconds_after_a_drop_where_an_x11_program_ended() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drag after a drop where an X11 program ended");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the first drag and ends, top right, and the one that takes the second, below it
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 50));
    let y = X11::connect(&display);
    let other = y.window(b"x11other\0X11other\0", EventMask::NO_EVENT);
    y.conn.change_property32(PropMode::REPLACE, other, y.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    y.conn.flush().unwrap();
    place_x11(&comp, "X11other", (600, 350));
    // the Wayland program dragging, on the left
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi, yi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap(), comp.window("X11other").unwrap());

    // the first drag over the first X11 program, which takes it and ends; the button let go there, where its window was
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    x11_drawn(&comp, xi.at);
    comp.must(&format!("pointer {} {}", xi.at.0 + 150, xi.at.1 + 100));
    let source = x.message(&mut [&mut s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the first X11 program's choice", |l| l.action == Some(DndAction::Copy));
    drop(x);
    // once the X server has destroyed its window, a window shown after that is managed only after the window manager read
    // of the destruction (it reads its X events in order); the button is let go only then
    let start = Instant::now();
    while y.conn.get_geometry(win).unwrap().reply().is_ok() {
        assert!(start.elapsed() < Duration::from_secs(5), "the X11 program's window not destroyed");
        std::thread::sleep(Duration::from_millis(20));
    }
    y.window(b"after\0After\0", EventMask::NO_EVENT);
    comp.wait(10, "the window shown after it managed", |c| c.window("After").is_some());
    place_x11(&comp, "After", (50, 350));
    comp.must("release");
    let first = Instant::now();
    until(&mut [&mut s], "the dragging program told the first drag is over", |l| l.cancelled);

    // the second drag over the other X11 program, which takes it
    log(|l| *l = Log::default());
    s.state.button_serial = 0;
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    x11_drawn(&comp, yi.at);
    let (px, py) = (yi.at.0 + 150, yi.at.1 + 100);
    comp.must(&format!("pointer {px} {py}"));
    let source = y.message(&mut [&mut s], y.enter, "XdndEnter at the other X11 program")[0];
    y.message(&mut [&mut s], y.position, "XdndPosition at the other X11 program");
    y.send(source, y.status, [other, 1, 0, 0, y.copy]);
    until(&mut [&mut s], "the dragging program told the other X11 program's choice", |l| l.action == Some(DndAction::Copy));

    // held there until the first drop is 11 s old: the drag is still taken, and its drop arrives and finishes
    std::thread::sleep(Duration::from_secs(11).saturating_sub(first.elapsed()));
    comp.must(&format!("pointer {} {py}", px + 10));
    y.message(&mut [&mut s], y.position, "XdndPosition 11 s after the first drop");
    y.send(source, y.status, [other, 1, 0, 0, y.copy]);
    comp.must("release");
    let dropped = y.message(&mut [&mut s], y.drop, "XdndDrop at the other X11 program");
    let into = y.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    y.conn.convert_selection(other, y.selection, y.uri, into, dropped[2]).unwrap();
    y.conn.flush().unwrap();
    let property = y.until(&mut [&mut s], "the URI given to the other X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == other => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = y.conn.get_property(true, other, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    y.send(source, y.finished, [other, 1, y.copy, 0, 0]);
    until(&mut [&mut s], "the dragging program told the second drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "the second drag not cancelled");
}

/// Two drags at once from two Wayland programs, a finger's and the mouse's (as on a 2-in-1's screen): the finger's over one
/// X11 program, which takes it, and the mouse's dropped on another, which takes it and never finishes the drop. The window
/// manager keeps one offer for the X11 programs, that of the drag that came over one last, and a drag that leaves an X11
/// program clears it, whichever drag's it is: the finger's drag leaving its X11 program takes the offer from the waiting drop.
/// Back over that X11 program, the finger's drag is offered there again and, held there for more than 10 s after the mouse's
/// drop (the time the fork's fix gives an X11 program to finish a drop), is still taken there: its drop arrives and finishes.
#[test]
fn a_finger_drag_outlasts_a_mouse_drop_another_x11_program_never_finishes() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a finger's drag and a mouse's drop at once");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program the finger's drag goes to, top right, and the one that takes the mouse's drop, below it
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 50));
    let y = X11::connect(&display);
    let other = y.window(b"x11other\0X11other\0", EventMask::NO_EVENT);
    y.conn.change_property32(PropMode::REPLACE, other, y.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    y.conn.flush().unwrap();
    place_x11(&comp, "X11other", (600, 350));
    // the Wayland program the finger drags from, top left, and the one the mouse drags from, below it. Each test's log is the
    // test thread's: the mouse's program is not dispatched once its drop is made, so what is logged after it is the finger's
    let mut f = comp.connect();
    let (_fk, _fp) = f.input();
    let fw = f.window("test.finger", "Finger");
    f.draw(&fw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 50 300 200", id_of(&comp, "test.finger")));
    f.roundtrip();
    let mut m = comp.connect();
    let (_mk, _mp) = m.input();
    let mw = m.window("test.mouse", "Mouse");
    m.draw(&mw, (300, 200), 0x0000ffff);
    comp.must(&format!("place {} 50 350 300 200", id_of(&comp, "test.mouse")));
    m.roundtrip();
    let (fi, mi) = (comp.window("test.finger").unwrap(), comp.window("test.mouse").unwrap());
    let (xi, yi) = (comp.window("X11target").unwrap(), comp.window("X11other").unwrap());
    // the first touch gives the seat its touch, and the finger's program then takes it
    let (fx, fy) = (fi.at.0 + 100, fi.at.1 + 100);
    comp.must(&format!("touch-down 0 {fx} {fy}"));
    comp.must("touch-up 0");
    f.until("the seat has touch", |st| st.seat_caps & 4 != 0);
    let _touch = f.seat.get_touch(&f.qh, Finger);
    f.roundtrip();

    // the finger's drag over the first X11 program, which takes it
    comp.must(&format!("touch-down 1 {fx} {fy}"));
    until(&mut [&mut f], "the finger down", |l| l.touched.is_some());
    let manager: WlDataDeviceManager = f.globals.bind(&f.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = manager.get_data_device(&f.seat, &f.qh, ());
    let file: WlDataSource = manager.create_data_source(&f.qh, Dragged);
    file.offer(URI_MIME.into());
    file.set_actions(DndAction::Copy);
    device.start_drag(Some(&file), &fw.surface, None, log(|l| l.touched.unwrap()));
    f.roundtrip();
    x11_drawn(&comp, xi.at);
    let (px, py) = (xi.at.0 + 150, xi.at.1 + 100);
    comp.must(&format!("touch-move 1 {px} {py}"));
    let source = x.message(&mut [&mut f], x.enter, "XdndEnter at the first X11 program")[0];
    x.message(&mut [&mut f], x.position, "XdndPosition at the first X11 program");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut f], "the finger's program told the first X11 program's choice", |l| l.action == Some(DndAction::Copy));

    // the mouse's drag over the other X11 program, which takes it; let go there, and never finished
    log(|l| l.action = None);
    drag_from(&comp, &mut m, &mw, mi.at, URI_MIME);
    x11_drawn(&comp, yi.at);
    comp.must(&format!("pointer {} {}", yi.at.0 + 150, yi.at.1 + 100));
    let msource = y.message(&mut [&mut m], y.enter, "XdndEnter at the other X11 program")[0];
    y.message(&mut [&mut m], y.position, "XdndPosition at the other X11 program");
    y.send(msource, y.status, [other, 1, 0, 0, y.copy]);
    until(&mut [&mut m], "the mouse's program told the other X11 program's choice", |l| l.action == Some(DndAction::Copy));
    comp.must("release");
    let mouse_drop = Instant::now();
    y.message(&mut [&mut m], y.drop, "XdndDrop at the other X11 program");

    // the finger's drag leaves its X11 program for its own program's window, and comes back: offered there again, and taken
    log(|l| l.action = None);
    comp.must(&format!("touch-move 1 {fx} {fy}"));
    x.message(&mut [&mut f], x.leave, "XdndLeave at the first X11 program");
    comp.must(&format!("touch-move 1 {px} {py}"));
    x.message(&mut [&mut f], x.enter, "XdndEnter at the first X11 program again");
    x.message(&mut [&mut f], x.position, "XdndPosition at the first X11 program again");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut f], "the finger's program told the first X11 program's choice again", |l| l.action == Some(DndAction::Copy));

    // held there until the mouse's drop is 11 s old: the finger's drag is still taken, and its drop arrives and finishes
    std::thread::sleep(Duration::from_secs(11).saturating_sub(mouse_drop.elapsed()));
    comp.must(&format!("touch-move 1 {} {py}", px + 10));
    x.message(&mut [&mut f], x.position, "XdndPosition 11 s after the mouse's drop");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    comp.must("touch-up 1");
    let dropped = x.message(&mut [&mut f], x.drop, "XdndDrop at the first X11 program");
    let into = x.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.selection, x.uri, into, dropped[2]).unwrap();
    x.conn.flush().unwrap();
    let property = x.until(&mut [&mut f], "the URI given to the first X11 program", |e| match e {
        Event::SelectionNotify(n) if n.requestor == win => Some(n.property),
        _ => None,
    });
    assert_eq!(property, into, "the URI given, not refused");
    let got = x.conn.get_property(true, win, into, AtomEnum::ANY, 0, 1 << 16).unwrap().reply().unwrap().value;
    assert_eq!(got, URI, "what was dropped arrived");
    x.send(source, x.finished, [win, 1, x.copy, 0, 0]);
    until(&mut [&mut f], "the finger's program told its drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "the finger's drag not cancelled");
}

/// An X11 program takes XdndSelection while a Wayland program drags between two Wayland windows (any X11 program can, at any
/// time): the drag stays the Wayland program's, and its text is what is dropped.
#[test]
fn an_x11_program_cannot_take_a_wayland_drag_over() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "an X11 program and a Wayland drag");
    let (comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the program dragging, on the left; the one receiving, on the right
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.target", "Target");
    t.draw(&tw, (300, 200), 0xff00ff00);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    comp.must(&format!("place {} 600 100 300 200", id_of(&comp, "test.target")));
    s.roundtrip();
    t.roundtrip();
    let (si, ti) = (comp.window("test.source").unwrap(), comp.window("test.target").unwrap());
    let _tdev = data_device(&mut t);
    drag_from(&comp, &mut s, &sw, si.at, MIME);
    comp.must(&format!("pointer {} {}", ti.at.0 + 150, ti.at.1 + 100));
    until(&mut [&mut s, &mut t], "the receiver offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == MIME));

    // an X11 program with no window shown takes XdndSelection; once the window manager has read that (it manages the window
    // the program shows after it), nothing has changed: no stand-in, and the Wayland program's drag goes on
    let h = X11::connect(&display);
    let hidden = h.conn.generate_id().unwrap();
    let aux = CreateWindowAux::new();
    h.conn.create_window(x11rb::COPY_DEPTH_FROM_PARENT, hidden, h.root, 0, 0, 1, 1, 0, WindowClass::INPUT_OUTPUT, 0, &aux).unwrap();
    h.conn.set_selection_owner(hidden, h.selection, x11rb::CURRENT_TIME).unwrap();
    h.window(b"later\0Later\0", EventMask::NO_EVENT);
    comp.wait(10, "the window shown after it managed", |c| c.window("Later").is_some());
    place_x11(&comp, "Later", (600, 350));
    let (px, py) = (ti.at.0 + 150, ti.at.1 + 100);
    let over = h.conn.translate_coordinates(h.root, h.root, px as i16, py as i16).unwrap().reply().unwrap().child;
    assert!(over == x11rb::NONE || h.aware_of(over).is_none(), "no stand-in for the X11 program's drag");
    s.roundtrip();
    t.roundtrip();
    assert!(!log(|l| l.cancelled), "the Wayland program's drag not cancelled");

    // taken and dropped: the Wayland program's text arrives
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    s.roundtrip();
    comp.must(&format!("pointer {} {py}", px + 10));
    comp.must("release");
    until(&mut [&mut s, &mut t], "the drop", |l| l.dropped);
    let (read, write) = std::io::pipe().unwrap();
    offer.receive(MIME.into(), write.as_fd());
    drop(write);
    until(&mut [&mut s, &mut t], "the dragging program asked for the text", |l| l.sent > 0);
    assert_eq!(read_all(read), WORDS, "what the Wayland program dragged arrived");
    offer.finish();
    until(&mut [&mut s, &mut t], "the dragging program told the drop finished", |l| l.finished);
}

// ------------------------------------------------------------------ XWayland going away in the middle of a transfer

/// Places the X11 program's window `app` at `at` once XWayland has made its surface. The window is listed a moment before
/// that, and it takes the keyboard when its surface comes: a window made after this keeps the keyboard.
fn place_x11(comp: &Compositor, app: &str, at: (i32, i32)) {
    comp.wait(10, &format!("{app} managed"), |c| c.window(app).is_some());
    comp.wait(10, &format!("{app} placed"), |c| c.ctl(&format!("place {} {} {} 300 200", id_of(c, app), at.0, at.1)).is_ok());
}

/// Kills this test's XWayland (as when it crashes) and waits until it has ended, its connections closed. Its process number.
fn end_xwayland(comp: &Compositor) -> u32 {
    let xwayland = xwayland_of(comp).expect("an Xwayland");
    unsafe { libc::kill(xwayland as i32, libc::SIGKILL) };
    let start = Instant::now();
    // xwayland_of passes over one that has ended and is not reaped yet
    while xwayland_of(comp) == Some(xwayland) {
        assert!(start.elapsed() < Duration::from_secs(5), "XWayland did not end");
        std::thread::sleep(Duration::from_millis(10));
    }
    xwayland
}

/// Waits until the compositor has seen XWayland go.
fn seen_gone(comp: &Compositor) {
    comp.wait(10, "XWayland gone", |c| c.log.lock().unwrap().iter().any(|l| l.contains("XWayland went away")));
}

/// Kills this test's XWayland (as when it crashes) and waits until the compositor has seen it go. Its process number.
fn kill_xwayland(comp: &Compositor) -> u32 {
    let xwayland = end_xwayland(comp);
    seen_gone(comp);
    xwayland
}

/// The compositor goes on after XWayland went away in the middle of a transfer (it once ended as soon as the transfer's pipe
/// was ready: the pipe's handler asked for the window manager that was gone), and the next X11 program starts a new XWayland,
/// which manages its window.
fn goes_on(comp: &mut Compositor, gone: u32) {
    let answer = comp.ctl("outputs");
    assert!(answer.is_ok() && matches!(comp.child.try_wait(), Ok(None)), "the compositor ended: {answer:?}\n{}", comp.log_tail(20));
    // the display is bound again before the next program connects (as tests/x11.rs's restart test waits): the same one, or,
    // when another test's compositor took it in between (tests run side by side), the one the compositor says
    let display = {
        let found = || {
            comp.log.lock().unwrap().iter().find_map(|l| {
                l.strip_suffix(")").and_then(|l| {
                    l.split_once("it starts again with the next X11 program (DISPLAY=")
                        .or_else(|| l.split_once("X11 programs now use DISPLAY="))
                        .map(|(_, d)| d.split_once(' ').map_or(d, |(d, _)| d).to_string())
                })
            })
        };
        comp.wait(10, "the display bound again", |_| found().is_some());
        found().unwrap()
    };
    let next = X11::connect(&display);
    next.window(b"next\0Next\0", EventMask::NO_EVENT);
    comp.wait(10, "the next X11 program's window managed", |c| c.window("Next").is_some());
    assert!(xwayland_of(comp).is_some_and(|x| x != gone), "a new XWayland");
}

/// XWayland (`gone`) went away while the Wayland program an X11 program asked for data held the pipe (`held`); now the program
/// writes `what` and closes the pipe. The transfer ended with XWayland, its pipe closed: the write fails, as nobody reads it
/// any more. The compositor goes on (`goes_on`).
fn written_after_xwayland_went_away(comp: &mut Compositor, gone: u32, what: &[u8]) {
    let held = log(|l| l.held.take()).expect("the pipe held");
    let wrote = std::fs::File::from(held).write_all(what);
    goes_on(comp, gone);
    assert!(matches!(&wrote, Err(e) if e.kind() == ErrorKind::BrokenPipe), "the transfer ended with XWayland: {wrote:?}");
}

/// XWayland goes away (it crashed, or was killed) while an X11 program pastes a file a Wayland program copied, before the
/// Wayland program has written it (a slow one): the paste ends, and the compositor and the next X11 program go on.
#[test]
fn a_paste_into_an_x11_program_ends_when_xwayland_goes_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a paste into an X11 program when XWayland goes away");
    let (mut comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that pastes, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11paste\0X11paste\0", EventMask::NO_EVENT);
    place_x11(&comp, "X11paste", (600, 100));
    // the Wayland program that copies a file: it has the keyboard, so it may set the clipboard (Smithay looks at the focus,
    // not at the serial); its source answers as a dragging program's does, and asked for the file it keeps the pipe
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.copy", "Copy");
    s.draw(&sw, (300, 200), 0xff0000ff);
    s.until("the keyboard on the copying program", |st| st.kb_focus == Some(sw.surface.id()));
    log(|l| l.hold = true);
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    let source: WlDataSource = m.create_data_source(&s.qh, Dragged);
    source.offer(URI_MIME.into());
    device.set_selection(Some(&source), 0);
    s.roundtrip();

    // the X11 program gets the keyboard (only then may it paste) and pastes: the copying program is asked for the file
    comp.must(&format!("activate {}", id_of(&comp, "X11paste")));
    comp.wait(10, "the keyboard on the X11 program", |c| c.focus().contains("X11paste"));
    let start = Instant::now();
    while x.conn.get_selection_owner(x.clipboard).unwrap().reply().unwrap().owner == x11rb::NONE {
        assert!(start.elapsed() < Duration::from_secs(5), "the copy not offered to X11 programs");
        std::thread::sleep(Duration::from_millis(20));
    }
    let into = x.conn.intern_atom(false, b"PASTED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.clipboard, x.uri, into, x11rb::CURRENT_TIME).unwrap();
    x.conn.flush().unwrap();
    until(&mut [&mut s], "the copying program asked for the file", |l| l.held.is_some());

    // XWayland goes away; then the copying program writes the file
    let gone = kill_xwayland(&comp);
    written_after_xwayland_went_away(&mut comp, gone, URI);
}

/// An X11 program pastes text a Wayland program copied and, before the Wayland program has written it (a slow one), asks again
/// from the same window. The window manager drops the first paste for the second (a program reads only the answer to its last
/// request), but the compositor is busy and gets to the second only after XWayland went away. The first paste ends all the
/// same, and the compositor and the next X11 program go on. (The window manager once told the program the first paste failed
/// before it dropped it: with XWayland gone that failed, the paste's pipe stayed watched, and the Wayland program's write into
/// it ended the compositor.)
#[test]
fn a_paste_asked_for_twice_ends_when_xwayland_goes_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a paste asked for twice when XWayland goes away");
    let (mut comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that pastes, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11paste\0X11paste\0", EventMask::NO_EVENT);
    place_x11(&comp, "X11paste", (600, 100));
    // the Wayland program that copies text, with the keyboard; asked for the text, it keeps the pipe
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.copy", "Copy");
    s.draw(&sw, (300, 200), 0xff0000ff);
    s.until("the keyboard on the copying program", |st| st.kb_focus == Some(sw.surface.id()));
    log(|l| l.hold = true);
    let m: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let device: WlDataDevice = m.get_data_device(&s.seat, &s.qh, ());
    let source: WlDataSource = m.create_data_source(&s.qh, Dragged);
    source.offer(MIME.into());
    device.set_selection(Some(&source), 0);
    s.roundtrip();

    // the X11 program gets the keyboard and pastes the text, as UTF8_STRING (a type the window manager knows without asking
    // XWayland, so it still gets to the second paste once XWayland is gone): the copying program is asked for it
    comp.must(&format!("activate {}", id_of(&comp, "X11paste")));
    comp.wait(10, "the keyboard on the X11 program", |c| c.focus().contains("X11paste"));
    let start = Instant::now();
    while x.conn.get_selection_owner(x.clipboard).unwrap().reply().unwrap().owner == x11rb::NONE {
        assert!(start.elapsed() < Duration::from_secs(5), "the copy not offered to X11 programs");
        std::thread::sleep(Duration::from_millis(20));
    }
    let text = x.conn.intern_atom(false, b"UTF8_STRING").unwrap().reply().unwrap().atom;
    let into = x.conn.intern_atom(false, b"PASTED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.clipboard, text, into, x11rb::CURRENT_TIME).unwrap();
    x.conn.flush().unwrap();
    until(&mut [&mut s], "the copying program asked for the text", |l| l.held.is_some());

    // The compositor is kept busy: it is sent half a control command and waits for the rest (2 s at most) on its main thread,
    // the one that handles X11 events too. It waits once it has read the half: nothing of it is left unread (SIOCOUTQ, which
    // libc calls TIOCOUTQ). Nothing on the Wayland side meanwhile, as it answers nobody.
    let mut busy = UnixStream::connect(&comp.ctl).expect("the control socket");
    busy.write_all(b"outputs").unwrap();
    let start = Instant::now();
    loop {
        let mut unread: libc::c_int = 0;
        unsafe { libc::ioctl(busy.as_raw_fd(), libc::TIOCOUTQ, &mut unread) };
        if unread == 0 {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(1), "the compositor did not read the command");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The X11 program asks for the text again. XWayland hands the request to the window manager (it has once the round trip
    // returns), whose thread for X11 events reads it at once (a moment is plenty), but the window manager gets to it only when
    // the compositor is no longer busy.
    x.conn.convert_selection(win, x.clipboard, text, into, x11rb::CURRENT_TIME).unwrap();
    x.conn.get_input_focus().unwrap().reply().unwrap();
    std::thread::sleep(Duration::from_millis(100));

    // XWayland goes away; then the compositor reads the rest of its command and gets to the second paste, after XWayland
    let gone = end_xwayland(&comp);
    busy.write_all(b"\n").unwrap();
    seen_gone(&comp);
    // the copying program writes the text; it was asked for it once, as the second paste came too late
    written_after_xwayland_went_away(&mut comp, gone, WORDS);
    s.roundtrip();
    assert_eq!(log(|l| l.sent), 1, "the window manager got to the second paste while XWayland was there");
}

/// XWayland goes away while a Wayland program pastes the files an X11 program copied, before it has read them all (more than
/// its pipe holds): the paste ends with what was in the pipe, and the compositor and the next X11 program go on.
#[test]
fn a_paste_from_an_x11_program_ends_when_xwayland_goes_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a paste from an X11 program when XWayland goes away");
    let (mut comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that copies the files, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11copy\0X11copy\0", EventMask::NO_EVENT);
    place_x11(&comp, "X11copy", (600, 100));
    // the Wayland program that pastes: it has the keyboard, so it is offered the clipboard
    let mut t = comp.connect();
    let (_tk, _tp) = t.input();
    let tw = t.window("test.paste", "Paste");
    t.draw(&tw, (300, 200), 0xff00ff00);
    t.until("the keyboard on the pasting program", |st| st.kb_focus == Some(tw.surface.id()));
    let _tdev = data_device(&mut t);
    // the X11 program copies: it takes the clipboard, and tells the window manager the types it comes in (TARGETS)
    x.conn.set_selection_owner(win, x.clipboard, x11rb::CURRENT_TIME).unwrap();
    x.conn.flush().unwrap();
    let asked = x.asked(&mut [&mut t], x.clipboard);
    assert_eq!(asked.target, x.targets, "asked for the types");
    x.give_types(&asked, &[x.targets, x.uri]);
    until(&mut [&mut t], "the Wayland program offered the copy", |l| l.clipboard.is_some() && l.mimes.iter().any(|m| m == URI_MIME));

    // it pastes: the X11 program is asked for the files and gives more than the pipe holds; the compositor fills the pipe
    let (read, write) = std::io::pipe().unwrap();
    log(|l| l.clipboard.clone()).unwrap().receive(URI_MIME.into(), write.as_fd());
    drop(write);
    let asked = x.asked(&mut [&mut t], x.clipboard);
    assert_eq!(asked.target, x.uri, "asked for the files");
    let room = usize::try_from(unsafe { libc::fcntl(read.as_raw_fd(), libc::F_GETPIPE_SZ) }).expect("the pipe's size");
    let files = URI.repeat(2 * room / URI.len());
    x.give(&asked, &files);
    let start = Instant::now();
    loop {
        let mut holds: libc::c_int = 0;
        unsafe { libc::ioctl(read.as_raw_fd(), libc::FIONREAD, &mut holds) };
        if holds as usize == room {
            break;
        }
        assert!(start.elapsed() < Duration::from_secs(5), "the pipe not filled: {holds} of {room} bytes");
        std::thread::sleep(Duration::from_millis(20));
    }

    // XWayland goes away; then the Wayland program reads: what was in the pipe, then the pipe closes
    let gone = kill_xwayland(&comp);
    let got = read_all(read);
    goes_on(&mut comp, gone);
    assert!(got == files[..room], "the paste ended with XWayland: {} bytes of {}", got.len(), files.len());
}

/// XWayland goes away while an X11 program asks for the file a Wayland program dropped on it, before the Wayland program has
/// written it (a slow one): the drop ends, its transfer too, and the Wayland program is told the drop was cancelled (the X11
/// program will never say it finished it). The compositor and the next X11 program go on.
#[test]
fn a_drop_on_an_x11_program_ends_when_xwayland_goes_away() {
    if !xwayland_installed() {
        eprintln!("skipped: XWayland is not installed");
        return;
    }
    let _l = limit(60, "a drop on an X11 program when XWayland goes away");
    let (mut comp, display) = with_xwayland_on(&["--size", "1000x600"], |_| {});
    // the X11 program that takes the drop, on the right
    let x = X11::connect(&display);
    let win = x.window(b"x11target\0X11target\0", EventMask::NO_EVENT);
    x.conn.change_property32(PropMode::REPLACE, win, x.aware, AtomEnum::ATOM, &[XDND]).unwrap();
    x.conn.flush().unwrap();
    place_x11(&comp, "X11target", (600, 100));
    // the Wayland program dragging a file, on the left: asked for the file, it keeps the pipe
    let mut s = comp.connect();
    let (_sk, _sp) = s.input();
    let sw = s.window("test.source", "Source");
    s.draw(&sw, (300, 200), 0xff0000ff);
    comp.must(&format!("place {} 50 100 300 200", id_of(&comp, "test.source")));
    s.roundtrip();
    let (si, xi) = (comp.window("test.source").unwrap(), comp.window("X11target").unwrap());
    log(|l| l.hold = true);
    drag_from(&comp, &mut s, &sw, si.at, URI_MIME);
    // over the X11 window, which takes it; let go there, and the X11 program asks for the file
    x11_drawn(&comp, xi.at);
    comp.must(&format!("pointer {} {}", xi.at.0 + 150, xi.at.1 + 100));
    let source = x.message(&mut [&mut s], x.enter, "XdndEnter")[0];
    x.message(&mut [&mut s], x.position, "XdndPosition");
    x.send(source, x.status, [win, 1, 0, 0, x.copy]);
    until(&mut [&mut s], "the dragging program told the X11 program's choice", |l| l.action == Some(DndAction::Copy));
    comp.must("release");
    let dropped = x.message(&mut [&mut s], x.drop, "XdndDrop");
    let into = x.conn.intern_atom(false, b"DROPPED").unwrap().reply().unwrap().atom;
    x.conn.convert_selection(win, x.selection, x.uri, into, dropped[2]).unwrap();
    x.conn.flush().unwrap();
    until(&mut [&mut s], "the dragging program asked for the file", |l| l.held.is_some());

    // XWayland goes away; then the dragging program writes the file, and hears the drop was cancelled
    let gone = kill_xwayland(&comp);
    written_after_xwayland_went_away(&mut comp, gone, URI);
    until(&mut [&mut s], "the dragging program told the drop was cancelled", |l| l.cancelled);
    assert!(!log(|l| l.finished), "the drop finished");
}
