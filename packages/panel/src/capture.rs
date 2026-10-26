//! Pictures of windows for the dock's window list, through the standard Wayland capture protocols (ext-foreign-toplevel-list,
//! ext-image-capture-source, ext-image-copy-capture): Hyprland has them, and other compositors are adding them.
//!
//! Each round opens a connection of its own on the calling thread (the panel's event loop is never held up), finds the windows by
//! app id and title, copies each into shared memory once, and shrinks it to a thumbnail. A window the compositor does not draw
//! (minimized, on the hidden workspace) may never deliver a frame: every wait has a deadline, and then it simply has no picture.

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use resvg::tiny_skia::Pixmap;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_registry::WlRegistry, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool};
use wayland_client::{delegate_noop, event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1::{self, ExtForeignToplevelHandleV1},
    ext_foreign_toplevel_list_v1::{self, ExtForeignToplevelListV1},
};
use wayland_protocols::ext::image_capture_source::v1::client::{
    ext_foreign_toplevel_image_capture_source_manager_v1::ExtForeignToplevelImageCaptureSourceManagerV1, ext_image_capture_source_v1::ExtImageCaptureSourceV1,
};
use wayland_protocols::ext::image_copy_capture::v1::client::{
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1},
    ext_image_copy_capture_manager_v1::{ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};

/// How long one window may take to deliver its picture.
const PER_WINDOW: Duration = Duration::from_millis(700);

#[derive(Default)]
struct Top {
    handle: Option<ExtForeignToplevelHandleV1>,
    title: String,
    app_id: String,
    closed: bool,
}

#[derive(Default)]
struct Cap {
    tops: Vec<Top>,
    listed: bool,
    // the session being set up
    size: Option<(u32, u32)>,
    formats: Vec<wl_shm::Format>,
    constraints_done: bool,
    stopped: bool,
    // the frame being captured: Some(true) ready, Some(false) failed
    frame: Option<bool>,
}

/// One window to picture: a key the caller knows it by, and how the compositor names it.
#[derive(Clone, Debug)]
pub struct Want {
    pub key: String,
    pub app_id: String,
    pub title: String,
}

/// Which listed window each wanted one is: same app id and title, the n-th of several with the same title for the n-th wanted;
/// with no title match, the app's only window. Pure, for tests.
pub fn match_windows(wants: &[Want], listed: &[(String, String)]) -> Vec<Option<usize>> {
    let mut used = vec![false; listed.len()];
    wants
        .iter()
        .map(|w| {
            let exact = (0..listed.len()).find(|&i| !used[i] && listed[i].0 == w.app_id && listed[i].1 == w.title);
            let pick = exact.or_else(|| {
                let same: Vec<usize> = (0..listed.len()).filter(|&i| listed[i].0 == w.app_id).collect();
                (same.len() == 1 && !used[same[0]]).then(|| same[0])
            });
            if let Some(i) = pick {
                used[i] = true;
            }
            pick
        })
        .collect()
}

/// Whether this compositor offers what `capture` needs (the live test asks first).
#[cfg(test)]
pub fn available() -> bool {
    let Ok(conn) = Connection::connect_to_env() else { return false };
    let Ok((globals, _q)) = registry_queue_init::<Cap>(&conn) else { return false };
    let names: Vec<String> = globals.contents().clone_list().into_iter().map(|g| g.interface).collect();
    ["ext_foreign_toplevel_list_v1", "ext_foreign_toplevel_image_capture_source_manager_v1", "ext_image_copy_capture_manager_v1"].iter().all(|n| names.iter().any(|x| x == n))
}

/// Pictures of the wanted windows, each shrunk to fit `box_w` x `box_h` pixels (aspect kept); `got` is called for each one as it
/// arrives. Returns an error only when the compositor offers no way to capture windows.
pub fn capture(wants: &[Want], box_w: u32, box_h: u32, mut got: impl FnMut(String, Pixmap)) -> Result<(), String> {
    let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Cap>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).map_err(|e| e.to_string())?;
    let list: ExtForeignToplevelListV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "no ext_foreign_toplevel_list_v1".to_string())?;
    let sources: ExtForeignToplevelImageCaptureSourceManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "no toplevel capture source manager".to_string())?;
    let copier: ExtImageCopyCaptureManagerV1 = globals.bind(&qh, 1..=1, ()).map_err(|_| "no ext_image_copy_capture_manager_v1".to_string())?;
    let mut st = Cap::default();
    // the compositor lists every window, each followed by its title and app id
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;
    queue.roundtrip(&mut st).map_err(|e| e.to_string())?;
    st.listed = true;
    let listed: Vec<(String, String)> = st.tops.iter().map(|t| (t.app_id.clone(), t.title.clone())).collect();
    for (want, idx) in wants.iter().zip(match_windows(wants, &listed)) {
        let Some(i) = idx else { continue };
        let Some(handle) = st.tops[i].handle.clone() else { continue };
        if st.tops[i].closed {
            continue;
        }
        if let Some(p) = one(&mut st, &mut queue, &qh, &shm, &sources, &copier, &handle, box_w, box_h) {
            got(want.key.clone(), p);
        }
    }
    for t in &st.tops {
        if let Some(h) = &t.handle {
            h.destroy();
        }
    }
    list.stop();
    sources.destroy();
    copier.destroy();
    let _ = conn.flush();
    Ok(())
}

/// Waits for events until `done` says so or the deadline passes. False on the deadline (or a broken connection).
fn wait(st: &mut Cap, queue: &mut EventQueue<Cap>, deadline: Instant, done: impl Fn(&Cap) -> bool) -> bool {
    loop {
        if queue.dispatch_pending(st).is_err() {
            return false;
        }
        if done(st) {
            return true;
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        if queue.flush().is_err() {
            return false;
        }
        let Some(guard) = queue.prepare_read() else { continue };
        let fd = guard.connection_fd();
        let mut pfd = [rustix::event::PollFd::new(&fd, rustix::event::PollFlags::IN)];
        let ms = left.as_millis().min(i32::MAX as u128) as i32;
        let ts = rustix::event::Timespec { tv_sec: (ms / 1000) as i64, tv_nsec: ((ms % 1000) as i64) * 1_000_000 };
        match rustix::event::poll(&mut pfd, Some(&ts)) {
            Ok(n) if n > 0 => {
                if guard.read().is_err() {
                    return false;
                }
            }
            Ok(_) => drop(guard),
            Err(_) => return false,
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn one(
    st: &mut Cap,
    queue: &mut EventQueue<Cap>,
    qh: &QueueHandle<Cap>,
    shm: &WlShm,
    sources: &ExtForeignToplevelImageCaptureSourceManagerV1,
    copier: &ExtImageCopyCaptureManagerV1,
    handle: &ExtForeignToplevelHandleV1,
    box_w: u32,
    box_h: u32,
) -> Option<Pixmap> {
    st.size = None;
    st.formats.clear();
    st.constraints_done = false;
    st.stopped = false;
    st.frame = None;
    let deadline = Instant::now() + PER_WINDOW;
    let source = sources.create_source(handle, qh, ());
    let session = copier.create_session(&source, Options::empty(), qh, ());
    let result = (|| {
        if !wait(st, queue, deadline, |s| s.constraints_done || s.stopped) || st.stopped {
            return None;
        }
        let (w, h) = st.size?;
        let format = [wl_shm::Format::Argb8888, wl_shm::Format::Xrgb8888].into_iter().find(|f| st.formats.contains(f))?;
        if w == 0 || h == 0 || w > 16384 || h > 16384 {
            return None;
        }
        let stride = w * 4;
        let size = (stride * h) as usize;
        let fd = rustix::fs::memfd_create(c"yggdrasil-preview", rustix::fs::MemfdFlags::CLOEXEC).ok()?;
        let file = std::fs::File::from(fd);
        file.set_len(size as u64).ok()?;
        let pool: WlShmPool = shm.create_pool(file.as_fd(), size as i32, qh, ());
        let buffer: WlBuffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, format, qh, ());
        let frame = session.create_frame(qh, ());
        frame.attach_buffer(&buffer);
        frame.damage_buffer(0, 0, w as i32, h as i32);
        frame.capture();
        let ok = wait(st, queue, deadline, |s| s.frame.is_some() || s.stopped) && st.frame == Some(true);
        frame.destroy();
        buffer.destroy();
        pool.destroy();
        if !ok {
            return None;
        }
        let map = unsafe { memmap2::Mmap::map(&file).ok()? };
        to_thumb(&map, w, h, stride, format == wl_shm::Format::Xrgb8888, box_w, box_h)
    })();
    session.destroy();
    source.destroy();
    let _ = queue.flush();
    result
}

/// Little-endian ARGB/XRGB rows (B G R A in memory, premultiplied) to a shrunk tiny-skia picture.
fn to_thumb(data: &[u8], w: u32, h: u32, stride: u32, opaque: bool, box_w: u32, box_h: u32) -> Option<Pixmap> {
    if data.len() < (stride * h) as usize {
        return None;
    }
    let mut src = Pixmap::new(w, h)?;
    let out = src.data_mut();
    for y in 0..h as usize {
        let row = &data[y * stride as usize..];
        for x in 0..w as usize {
            let (b, g, r, a) = (row[x * 4], row[x * 4 + 1], row[x * 4 + 2], if opaque { 255 } else { row[x * 4 + 3] });
            let o = (y * w as usize + x) * 4;
            // premultiplied already; keep each channel within alpha so tiny-skia accepts it
            out[o..o + 4].copy_from_slice(&[r.min(a), g.min(a), b.min(a), a]);
        }
    }
    let scale = (box_w as f32 / w as f32).min(box_h as f32 / h as f32).min(1.0);
    let (dw, dh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
    crate::paint::area_resize(&src, dw, dh)
}

impl Dispatch<WlRegistry, GlobalListContents> for Cap {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<ExtForeignToplevelListV1, ()> for Cap {
    fn event(st: &mut Self, _: &ExtForeignToplevelListV1, event: ext_foreign_toplevel_list_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let ext_foreign_toplevel_list_v1::Event::Toplevel { toplevel } = event {
            if st.listed {
                // a window that opened during the round: not asked for
                toplevel.destroy();
            } else {
                st.tops.push(Top { handle: Some(toplevel), ..Default::default() });
            }
        }
    }

    event_created_child!(Cap, ExtForeignToplevelListV1, [
        ext_foreign_toplevel_list_v1::EVT_TOPLEVEL_OPCODE => (ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ExtForeignToplevelHandleV1, ()> for Cap {
    fn event(st: &mut Self, h: &ExtForeignToplevelHandleV1, event: ext_foreign_toplevel_handle_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let Some(t) = st.tops.iter_mut().find(|t| t.handle.as_ref().map(|x| x.id()) == Some(h.id())) else { return };
        match event {
            ext_foreign_toplevel_handle_v1::Event::Title { title } => t.title = title,
            ext_foreign_toplevel_handle_v1::Event::AppId { app_id } => t.app_id = app_id,
            ext_foreign_toplevel_handle_v1::Event::Closed => t.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for Cap {
    fn event(st: &mut Self, _: &ExtImageCopyCaptureSessionV1, event: ext_image_copy_capture_session_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
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
    fn event(st: &mut Self, _: &ExtImageCopyCaptureFrameV1, event: ext_image_copy_capture_frame_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_image_copy_capture_frame_v1::Event::Ready => st.frame = Some(true),
            ext_image_copy_capture_frame_v1::Event::Failed { .. } => st.frame = Some(false),
            _ => {}
        }
    }
}

delegate_noop!(Cap: ignore WlShm);
delegate_noop!(Cap: ignore WlShmPool);
delegate_noop!(Cap: ignore WlBuffer);
delegate_noop!(Cap: ExtForeignToplevelImageCaptureSourceManagerV1);
delegate_noop!(Cap: ExtImageCaptureSourceV1);
delegate_noop!(Cap: ExtImageCopyCaptureManagerV1);

#[cfg(test)]
mod tests {
    use super::*;

    fn want(key: &str, app: &str, title: &str) -> Want {
        Want { key: key.into(), app_id: app.into(), title: title.into() }
    }

    #[test]
    fn windows_are_matched_by_app_and_title_then_by_app_alone() {
        let listed = vec![
            ("code".to_string(), "main.rs — Labyrinth_OS — Visual Studio Code".to_string()),
            ("vivaldi-stable".to_string(), "GitHub - Vivaldi".to_string()),
            ("vivaldi-stable".to_string(), "GitHub - Vivaldi".to_string()),
            ("com.mitchellh.ghostty".to_string(), "~".to_string()),
        ];
        let wants = vec![
            want("a", "vivaldi-stable", "GitHub - Vivaldi"),
            want("b", "vivaldi-stable", "GitHub - Vivaldi"),
            want("c", "code", "a title that changed since"),
            want("d", "vivaldi-stable", "Another tab - Vivaldi"),
            want("e", "nothing", "x"),
        ];
        assert_eq!(match_windows(&wants, &listed), vec![Some(1), Some(2), Some(0), None, None]);
    }

    #[test]
    fn a_frame_becomes_a_thumbnail_that_fits() {
        // 4 x 2 opaque blue (B G R X), shrunk into a 2 x 2 box: 2 x 1, blue
        let data: Vec<u8> = std::iter::repeat([255u8, 0, 0, 0]).take(8).flatten().collect();
        let t = to_thumb(&data, 4, 2, 16, true, 2, 2).unwrap();
        assert_eq!((t.width(), t.height()), (2, 1));
        assert_eq!(&t.data()[0..4], &[0, 0, 255, 255]);
        assert!(to_thumb(&data[..10], 4, 2, 16, true, 2, 2).is_none(), "a short buffer is refused");
    }

    /// Run by hand (`cargo test -- --ignored live_capture --nocapture`) in a Hyprland session: pictures every window and writes
    /// the first to $PREVIEW_OUT (a PNG).
    #[test]
    #[ignore]
    fn live_capture() {
        assert!(available(), "the compositor has no window capture");
        let out = std::process::Command::new("hyprctl").args(["-j", "clients"]).output().unwrap();
        let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
        let wants: Vec<Want> = v.as_array().unwrap().iter().map(|c| want(c["address"].as_str().unwrap(), c["class"].as_str().unwrap(), c["title"].as_str().unwrap())).collect();
        let mut got = vec![];
        let t = Instant::now();
        capture(&wants, 440, 280, |k, p| got.push((k, p))).unwrap();
        eprintln!("{} of {} windows pictured in {:?}", got.len(), wants.len(), t.elapsed());
        if let (Some((k, p)), Ok(path)) = (got.first(), std::env::var("PREVIEW_OUT")) {
            eprintln!("{k}: {}x{} -> {path}", p.width(), p.height());
            p.save_png(path).unwrap();
        }
        assert!(!got.is_empty());
    }
}
