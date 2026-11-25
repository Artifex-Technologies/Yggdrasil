//! Drag and drop between two programs (wl_data_device): what is dropped arrives. One program drags text out of its window;
//! the other, under the pointer when the button is let go, is offered it, accepts it, and reads it through the pipe the
//! compositor hands over; the dragging program is told the drop finished. The compositor's part is routing: the offer to the
//! window under the pointer, the drop on release, the transfer between the two.

mod common;

use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use common::*;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};

const MIME: &str = "text/plain;charset=utf-8";
const WORDS: &[u8] = b"dropped from one program into another";

/// What the receiving and the dragging program were told (one test: a log of its own).
#[derive(Default)]
struct Log {
    /// the receiver: the offer it was given when the drag came in, with its serial, and the types it offers
    entered: Option<(u32, WlDataOffer)>,
    mimes: Vec<String>,
    dropped: bool,
    /// the dragging program: data asked for, the drop finished
    sent: usize,
    finished: bool,
    cancelled: bool,
}

static LOG: Mutex<Option<Log>> = Mutex::new(None);

fn log<T>(f: impl FnOnce(&mut Log) -> T) -> T {
    f(LOG.lock().unwrap().get_or_insert_with(Log::default))
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
                if mime_type == MIME {
                    let mut f = std::fs::File::from(fd);
                    let _ = f.write_all(WORDS);
                }
                log(|l| l.sent += 1);
            }
            wl_data_source::Event::DndFinished => log(|l| l.finished = true),
            wl_data_source::Event::Cancelled => log(|l| l.cancelled = true),
            _ => {}
        }
    }
}

/// Dispatches both programs until `done`, at most 5 s.
fn until(a: &mut Client, b: &mut Client, what: &str, done: impl Fn(&Log) -> bool) {
    let start = Instant::now();
    while !log(|l| done(l)) {
        a.roundtrip();
        b.roundtrip();
        assert!(start.elapsed() < Duration::from_secs(5), "{what}");
        std::thread::sleep(Duration::from_millis(10));
    }
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
    let id = |app: &str| {
        let lines = comp.must("clients");
        lines.iter().find(|l| l.split('\t').nth(1) == Some(app)).unwrap().split('\t').next().unwrap().to_string()
    };
    comp.must(&format!("place {} 50 100 300 200", id("test.source")));
    comp.must(&format!("place {} 600 100 300 200", id("test.target")));
    s.roundtrip();
    t.roundtrip();
    let (si, ti) = (comp.window("test.source").unwrap(), comp.window("test.target").unwrap());
    assert!(si.at.0 + 300 <= ti.at.0, "side by side: {si:?} {ti:?}");

    // the receiver's data device, before the drag comes
    let tm: WlDataDeviceManager = t.globals.bind(&t.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let _tdev: WlDataDevice = tm.get_data_device(&t.seat, &t.qh, Receiver);
    t.roundtrip();

    // the button held on the source's window, and the drag started there
    comp.must(&format!("pointer {} {}", si.at.0 + 100, si.at.1 + 100));
    s.until("the pointer on the source", |st| st.pointer_on == Some(sw.surface.id()));
    comp.must("press");
    s.until("the press", |st| st.button_serial != 0);
    let sm: WlDataDeviceManager = s.globals.bind(&s.qh, 3..=3, ()).expect("wl_data_device_manager 3");
    let sdev: WlDataDevice = sm.get_data_device(&s.seat, &s.qh, ());
    let source: WlDataSource = sm.create_data_source(&s.qh, Dragged);
    source.offer(MIME.into());
    source.set_actions(DndAction::Copy);
    sdev.start_drag(Some(&source), &sw.surface, None, s.state.button_serial);
    s.roundtrip();

    // over the receiver's window: it is offered the text, and accepts it
    comp.must(&format!("pointer {} {}", ti.at.0 + 150, ti.at.1 + 100));
    until(&mut s, &mut t, "the receiver offered the drag", |l| l.entered.is_some() && l.mimes.iter().any(|m| m == MIME));
    let (serial, offer) = log(|l| l.entered.clone()).unwrap();
    offer.accept(serial, Some(MIME.into()));
    offer.set_actions(DndAction::Copy, DndAction::Copy);
    t.roundtrip();
    s.roundtrip();

    // let go over it: the drop comes, the receiver reads what was dragged
    comp.must("release");
    until(&mut s, &mut t, "the drop", |l| l.dropped);
    let (mut read, write) = std::io::pipe().unwrap();
    offer.receive(MIME.into(), std::os::fd::AsFd::as_fd(&write));
    drop(write);
    until(&mut s, &mut t, "the dragging program asked for the text", |l| l.sent > 0);
    let mut got = vec![];
    read.read_to_end(&mut got).unwrap();
    assert_eq!(got, WORDS, "what was dropped arrived");
    offer.finish();
    until(&mut s, &mut t, "the dragging program told the drop finished", |l| l.finished);
    assert!(!log(|l| l.cancelled), "not cancelled");
}
