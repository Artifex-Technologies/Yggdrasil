//! The capture: one screen copied into shared memory through ext-image-copy-capture, frame after frame, on a thread of its own.
//! The compositor answers a capture once the screen has changed since the last one, so the frames come at the screen's own pace
//! and none while it stands still. Two buffers take turns; each finished frame is copied to the shared `Frame` and the
//! stream's side is told.

use std::os::fd::AsFd;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_output, wl_output::WlOutput, wl_registry::WlRegistry};
use wayland_client::protocol::{wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, QueueHandle, WEnum};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_image_capture_source_v1::ExtImageCaptureSourceV1, ext_output_image_capture_source_manager_v1::ExtOutputImageCaptureSourceManagerV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

use crate::{Frame, Msg};

/// What the stream needs to know of the frames: their size, the bytes of a row, and whether they carry alpha.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub alpha: bool,
}

#[derive(Default)]
struct Cap {
    /// the screens, with the names they gave (wl_output version 4)
    outputs: Vec<(WlOutput, String)>,
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    stopped: bool,
    /// the frame being captured: Some(Ok) ready, Some(Err(reason)) failed
    frame: Option<Result<(), String>>,
}

/// Starts the capture of the screen named `output` (the pointer painted in with `cursor`) and waits until its shape is known.
pub fn start(output: &str, cursor: bool, frame: Arc<Mutex<Frame>>, tx: pipewire::channel::Sender<Msg>) -> Result<Shape, String> {
    let (shape_tx, shape_rx) = mpsc::channel::<Result<Shape, String>>();
    let output = output.to_string();
    std::thread::Builder::new()
        .name("capture".into())
        .spawn(move || {
            if let Err(e) = run(&output, cursor, &frame, &tx, &shape_tx) {
                // before the shape: the caller hears it; after: the stream's side
                if shape_tx.send(Err(e.clone())).is_err() {
                    let _ = tx.send(Msg::Ended(e));
                }
            }
        })
        .map_err(|e| e.to_string())?;
    shape_rx.recv().map_err(|_| "the capture ended before it started".to_string())?
}

fn wait(st: &mut Cap, queue: &mut EventQueue<Cap>, done: impl Fn(&Cap) -> bool) -> Result<(), String> {
    while !done(st) {
        queue.blocking_dispatch(st).map_err(|e| format!("the compositor: {e}"))?;
    }
    Ok(())
}

fn run(
    want: &str,
    cursor: bool,
    frame: &Arc<Mutex<Frame>>,
    tx: &pipewire::channel::Sender<Msg>,
    shape_tx: &mpsc::Sender<Result<Shape, String>>,
) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| format!("no compositor: {e}"))?;
    let (globals, mut queue) = registry_queue_init::<Cap>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
    let sources: ExtOutputImageCaptureSourceManagerV1 =
        globals.bind(&qh, 1..=1, ()).map_err(|_| "the compositor offers no screen capture (ext-output-image-capture-source)")?;
    let copier: ExtImageCopyCaptureManagerV1 =
        globals.bind(&qh, 1..=1, ()).map_err(|_| "the compositor offers no screen capture (ext-image-copy-capture)")?;
    let mut st = Cap::default();
    for g in globals.contents().clone_list() {
        if g.interface == "wl_output" && g.version >= 4 {
            let o: WlOutput = globals.registry().bind(g.name, 4, &qh, ());
            st.outputs.push((o, String::new()));
        }
    }
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;
    let Some(output) = st.outputs.iter().find(|(_, n)| n == want).map(|(o, _)| o.clone()) else {
        let names: Vec<&str> = st.outputs.iter().map(|(_, n)| n.as_str()).collect();
        return Err(format!("no screen named {want} (there are: {})", names.join(", ")));
    };
    let source = sources.create_source(&output, &qh, ());
    let options = if cursor { Options::PaintCursors } else { Options::empty() };
    let session = copier.create_session(&source, options, &qh, ());
    wait(&mut st, &mut queue, |s| s.constraints_done || s.stopped)?;
    if st.stopped {
        return Err(format!("the screen {want} cannot be captured"));
    }
    let (w, h) = st.size.ok_or("the compositor gave no size")?;
    let (format, alpha) = if st.formats.contains(&wl_shm::Format::Xrgb8888) {
        (wl_shm::Format::Xrgb8888, false)
    } else if st.formats.contains(&wl_shm::Format::Argb8888) {
        (wl_shm::Format::Argb8888, true)
    } else {
        return Err("the compositor offers no 32-bit shared-memory format".into());
    };
    if w == 0 || h == 0 || w > 16384 || h > 16384 {
        return Err(format!("a screen of {w}x{h}"));
    }
    let stride = w * 4;
    let size = (stride * h) as usize;
    // two buffers taking turns
    let mut bufs = vec![];
    for _ in 0..2 {
        let fd = rustix::fs::memfd_create(c"yggdrasil-screencast", rustix::fs::MemfdFlags::CLOEXEC).map_err(|e| e.to_string())?;
        let file = std::fs::File::from(fd);
        file.set_len(size as u64).map_err(|e| e.to_string())?;
        let pool: WlShmPool = shm.create_pool(file.as_fd(), size as i32, &qh, ());
        let buffer: WlBuffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, &qh, ());
        // SAFETY: a memfd of our own, of the size set above; the compositor writes it only between capture and ready
        let map = unsafe { memmap2::Mmap::map(&file) }.map_err(|e| e.to_string())?;
        bufs.push((buffer, pool, map, file));
    }
    let _ = shape_tx.send(Ok(Shape { width: w, height: h, stride, alpha }));

    let mut i = 0;
    loop {
        st.frame = None;
        let f = session.create_frame(&qh, ());
        f.attach_buffer(&bufs[i].0);
        f.damage_buffer(0, 0, w as i32, h as i32);
        f.capture();
        wait(&mut st, &mut queue, |s| s.frame.is_some() || s.stopped)?;
        f.destroy();
        if st.stopped {
            let _ = tx.send(Msg::Ended(format!("the screen {want} is no longer captured")));
            return Ok(());
        }
        match st.frame.take() {
            Some(Ok(())) => {
                let mut fr = frame.lock().unwrap_or_else(|e| e.into_inner());
                fr.pixels.clear();
                fr.pixels.extend_from_slice(&bufs[i].2[..size]);
                fr.fresh = true;
                drop(fr);
                if tx.send(Msg::Frame).is_err() {
                    return Ok(());
                }
                i = (i + 1) % bufs.len();
            }
            Some(Err(reason)) => {
                let _ = tx.send(Msg::Ended(format!("a frame failed ({reason}); the screen may have changed size")));
                return Ok(());
            }
            None => {}
        }
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for Cap {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, ()> for Cap {
    fn event(st: &mut Self, o: &WlOutput, event: wl_output::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = event {
            if let Some(e) = st.outputs.iter_mut().find(|(x, _)| x == o) {
                e.1 = name;
            }
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for Cap {
    fn event(
        st: &mut Self,
        _: &ExtImageCopyCaptureSessionV1,
        event: ext_image_copy_capture_session_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_session_v1::Event::BufferSize { width, height } => st.size = Some((width, height)),
            ext_image_copy_capture_session_v1::Event::ShmFormat { format: WEnum::Value(f) } => st.formats.push(f),
            ext_image_copy_capture_session_v1::Event::Done => st.constraints_done = true,
            ext_image_copy_capture_session_v1::Event::Stopped => st.stopped = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, ()> for Cap {
    fn event(
        st: &mut Self,
        _: &ExtImageCopyCaptureFrameV1,
        event: ext_image_copy_capture_frame_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => st.frame = Some(Ok(())),
            ext_image_copy_capture_frame_v1::Event::Failed { reason } => st.frame = Some(Err(format!("{reason:?}"))),
            _ => {}
        }
    }
}

delegate_noop!(Cap: ignore WlShm);
delegate_noop!(Cap: ignore WlShmPool);
delegate_noop!(Cap: ignore WlBuffer);
delegate_noop!(Cap: ExtOutputImageCaptureSourceManagerV1);
delegate_noop!(Cap: ExtImageCaptureSourceV1);
delegate_noop!(Cap: ExtImageCopyCaptureManagerV1);
