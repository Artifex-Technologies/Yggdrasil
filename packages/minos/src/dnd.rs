//! Drag and drop between windows and programs on Wayland. winit (what egui runs on) has no
//! drag and drop on Wayland at all, so this speaks the protocol itself, on its own event queue on
//! the window's connection: files dragged out of Minos leave as a `text/uri-list` (any other window,
//! the desktop, another Minos can take them), and files dropped on Minos from anywhere come back as
//! `DndEvent::Drop`. Dragging inside one window is still egui's own (it needs no protocol).

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use raw_window_handle::{HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::{DndAction, WlDataDeviceManager};
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle, WEnum};

const URI_LIST: &str = "text/uri-list";

pub enum DndEvent {
    /// Something is being dragged over the window, at this point (logical pixels).
    Hover((f32, f32)),
    /// The drag left the window without dropping.
    Leave,
    /// Files were dropped at this point; `moved` is whether the drag asked for a move rather than a copy.
    Drop { paths: Vec<PathBuf>, at: (f32, f32), moved: bool },
    /// Our own drag out of the window ended (dropped somewhere, or cancelled).
    Finished,
}

struct St {
    ctx: eframe::egui::Context,
    tx: Sender<DndEvent>,
    conn: Connection,
    serial: Arc<AtomicU32>,
    down: Arc<AtomicBool>,
    offers: HashMap<ObjectId, Vec<String>>,
    entered: Option<(WlDataOffer, bool)>,
    action: DndAction,
    pos: (f32, f32),
    out_paths: Arc<Mutex<Vec<PathBuf>>>,
}

pub struct Dnd {
    qh: QueueHandle<St>,
    manager: WlDataDeviceManager,
    device: WlDataDevice,
    compositor: WlCompositor,
    shm: WlShm,
    surface: WlSurface,
    conn: Connection,
    serial: Arc<AtomicU32>,
    down: Arc<AtomicBool>,
    out_paths: Arc<Mutex<Vec<PathBuf>>>,
    pub events: Receiver<DndEvent>,
    dragging_out: bool,
}

/// Connect to the window's Wayland connection. `None` when the window is not a Wayland one.
pub fn init(frame: &eframe::Frame, ctx: &eframe::egui::Context) -> Option<Dnd> {
    let RawDisplayHandle::Wayland(d) = frame.display_handle().ok()?.as_raw() else { return None };
    let RawWindowHandle::Wayland(w) = frame.window_handle().ok()?.as_raw() else { return None };
    // SAFETY: the pointers come from the live window; the connection is only borrowed (never closed by us).
    let backend = unsafe { Backend::from_foreign_display(d.display.as_ptr().cast()) };
    let conn = Connection::from_backend(backend);
    let (globals, mut queue) = registry_queue_init::<St>(&conn).ok()?;
    let qh = queue.handle();
    let seat: WlSeat = globals.bind(&qh, 5..=8, ()).ok()?;
    let manager: WlDataDeviceManager = globals.bind(&qh, 3..=3, ()).ok()?;
    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).ok()?;
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).ok()?;
    let device = manager.get_data_device(&seat, &qh, ());
    let _pointer = seat.get_pointer(&qh, ());
    let surface_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), w.surface.as_ptr().cast()) }.ok()?;
    let surface = WlSurface::from_id(&conn, surface_id).ok()?;

    let (tx, events) = channel();
    let serial = Arc::new(AtomicU32::new(0));
    let down = Arc::new(AtomicBool::new(false));
    let out_paths = Arc::new(Mutex::new(vec![]));
    let mut st = St { ctx: ctx.clone(), tx, conn: conn.clone(), serial: serial.clone(), down: down.clone(), offers: HashMap::new(), entered: None, action: DndAction::empty(), pos: (0.0, 0.0), out_paths: out_paths.clone() };
    queue.roundtrip(&mut st).ok()?;
    std::thread::Builder::new()
        .name("minos-dnd".into())
        .spawn(move || {
            // Not blocking_dispatch: the window's own thread reads the same socket, and when it reads first
            // our events sit in this queue with nobody to wake us (so a drop target waiting on the data
            // timed out). Look again every few milliseconds instead.
            use std::os::fd::AsRawFd;
            loop {
                if queue.dispatch_pending(&mut st).is_err() {
                    break;
                }
                let _ = st.conn.flush();
                let Some(guard) = queue.prepare_read() else { continue };
                let mut p = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
                // SAFETY: one valid pollfd.
                if unsafe { libc::poll(&mut p, 1, 15) } > 0 {
                    let _ = guard.read();
                } else {
                    drop(guard);
                }
            }
        })
        .ok()?;
    Some(Dnd { qh, manager, device, compositor, shm, surface, conn, serial, down, out_paths, events, dragging_out: false })
}

impl Dnd {
    /// Whether the main button is held and the compositor gave us its serial (needed to start a drag).
    pub fn can_start(&self) -> bool {
        !self.dragging_out && self.down.load(Ordering::Relaxed) && self.serial.load(Ordering::Relaxed) != 0
    }

    pub fn dragging_out(&self) -> bool {
        self.dragging_out
    }

    pub fn finished(&mut self) {
        self.dragging_out = false;
    }

    /// Hand these files to the compositor as a drag, so they can be dropped on another window or the desktop.
    pub fn start_drag(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() || !self.can_start() {
            return;
        }
        *self.out_paths.lock().unwrap() = paths;
        let source = self.manager.create_data_source(&self.qh, ());
        source.offer(URI_LIST.into());
        source.offer("text/plain".into());
        source.offer("application/x-minos-drag".into());
        source.set_actions(DndAction::Copy | DndAction::Move);
        let icon = self.icon();
        self.device.start_drag(Some(&source), &self.surface, icon.as_ref(), self.serial.load(Ordering::Relaxed));
        let _ = self.conn.flush();
        self.dragging_out = true;
    }

    /// A small purple tile that travels with the pointer.
    fn icon(&self) -> Option<WlSurface> {
        const N: i32 = 40;
        let mut px = Vec::with_capacity((N * N * 4) as usize);
        for y in 0..N {
            for x in 0..N {
                let edge = x < 2 || y < 2 || x >= N - 2 || y >= N - 2;
                let c: [u8; 4] = if edge { [255, 255, 255, 230] } else { [124, 84, 214, 200] };
                px.extend_from_slice(&[c[2], c[1], c[0], c[3]]); // ARGB8888, little-endian
            }
        }
        let path = std::env::temp_dir().join(format!("minos-dnd-icon-{}", std::process::id()));
        let mut f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).ok()?;
        f.write_all(&px).ok()?;
        let _ = std::fs::remove_file(&path);
        let pool = self.shm.create_pool(f.as_fd(), px.len() as i32, &self.qh, ());
        let buf = pool.create_buffer(0, N, N, N * 4, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        let s = self.compositor.create_surface(&self.qh, ());
        s.attach(Some(&buf), 0, 0);
        s.commit();
        Some(s)
    }
}

fn encode(p: &std::path::Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut s = String::from("file://");
    for &b in p.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"/-_.~".contains(&b) {
            s.push(b as char);
        } else {
            s.push_str(&format!("%{b:02X}"));
        }
    }
    s
}

fn decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let (mut out, mut i) = (vec![], 0);
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    out
}

/// The files in a `text/uri-list`.
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    text.lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.strip_prefix("file://"))
        .map(|rest| rest.strip_prefix("localhost").unwrap_or(rest))
        .map(|p| PathBuf::from(std::ffi::OsString::from_vec(decode(p))))
        .collect()
}

impl Dispatch<wayland_client::protocol::wl_registry::WlRegistry, GlobalListContents> for St {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlSeat, ()> for St {
    fn event(_: &mut Self, _: &WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WlPointer, ()> for St {
    fn event(st: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_pointer::Event::Button { serial, button, state, .. } = event {
            if button == 0x110 {
                // BTN_LEFT
                st.down.store(matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed)), Ordering::Relaxed);
                if matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed)) {
                    st.serial.store(serial, Ordering::Relaxed);
                }
            }
        }
    }
}
impl Dispatch<WlDataDevice, ()> for St {
    fn event(st: &mut Self, _: &WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                st.offers.insert(id.id(), vec![]);
            }
            wl_data_device::Event::Enter { serial, x, y, id: Some(offer), .. } => {
                let mimes = st.offers.get(&offer.id()).cloned().unwrap_or_default();
                let files = mimes.iter().any(|m| m == URI_LIST);
                if files {
                    let ours = mimes.iter().any(|m| m == "application/x-minos-drag");
                    offer.accept(serial, Some(URI_LIST.into()));
                    offer.set_actions(DndAction::Copy | DndAction::Move, if ours { DndAction::Move } else { DndAction::Copy });
                } else {
                    offer.accept(serial, None);
                }
                st.pos = (x as f32, y as f32);
                st.entered = Some((offer, files));
                if files {
                    let _ = st.tx.send(DndEvent::Hover(st.pos));
                }
                st.ctx.request_repaint();
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                st.pos = (x as f32, y as f32);
                if matches!(st.entered, Some((_, true))) {
                    let _ = st.tx.send(DndEvent::Hover(st.pos));
                    st.ctx.request_repaint();
                }
            }
            wl_data_device::Event::Leave => {
                if let Some((o, _)) = st.entered.take() {
                    st.offers.remove(&o.id());
                    o.destroy();
                }
                let _ = st.tx.send(DndEvent::Leave);
                st.ctx.request_repaint();
            }
            wl_data_device::Event::Drop => {
                let Some((offer, files)) = st.entered.take() else { return };
                if !files {
                    offer.destroy();
                    return;
                }
                let (tx, ctx, conn, at, moved) = (st.tx.clone(), st.ctx.clone(), st.conn.clone(), st.pos, st.action.contains(DndAction::Move));
                let Ok((mut rd, wr)) = std::os::unix::net::UnixStream::pair() else { return };
                offer.receive(URI_LIST.into(), wr.as_fd());
                let _ = conn.flush();
                drop(wr);
                std::thread::spawn(move || {
                    let mut text = String::new();
                    let _ = rd.read_to_string(&mut text);
                    let paths = parse_uri_list(&text);
                    offer.finish();
                    offer.destroy();
                    let _ = conn.flush();
                    if !paths.is_empty() {
                        let _ = tx.send(DndEvent::Drop { paths, at, moved });
                    }
                    ctx.request_repaint();
                });
            }
            _ => {}
        }
    }
    event_created_child!(St, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, ()),
    ]);
}
impl Dispatch<WlDataOffer, ()> for St {
    fn event(st: &mut Self, offer: &WlDataOffer, event: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_offer::Event::Offer { mime_type } => st.offers.entry(offer.id()).or_default().push(mime_type),
            wl_data_offer::Event::Action { dnd_action: WEnum::Value(a) } => st.action = a,
            _ => {}
        }
    }
}
impl Dispatch<WlDataSource, ()> for St {
    fn event(st: &mut Self, source: &WlDataSource, event: wl_data_source::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let paths = st.out_paths.lock().unwrap().clone();
                let mut f = std::fs::File::from(fd);
                if mime_type == URI_LIST {
                    let _ = f.write_all(paths.iter().map(|p| encode(p) + "\r\n").collect::<String>().as_bytes());
                } else if mime_type == "text/plain" {
                    let _ = f.write_all(paths.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>().join("\n").as_bytes());
                }
            }
            wl_data_source::Event::Cancelled | wl_data_source::Event::DndFinished => {
                source.destroy();
                st.down.store(false, Ordering::Relaxed);
                let _ = st.tx.send(DndEvent::Finished);
                st.ctx.request_repaint();
            }
            _ => {}
        }
    }
}
delegate_noop!(St: WlDataDeviceManager);
delegate_noop!(St: WlCompositor);
delegate_noop!(St: WlSurface);
delegate_noop!(St: ignore WlShm);
delegate_noop!(St: WlShmPool);
delegate_noop!(St: ignore WlBuffer);
