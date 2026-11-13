//! Window and screen pictures through the standard capture protocols: ext-image-capture-source (what to capture: a window named
//! by its ext-foreign-toplevel-list handle, or a screen) and ext-image-copy-capture (the copy into the program's own buffer).
//! The panel's dock takes its window previews this way (packages/panel/src/capture.rs), on Hyprland and here alike. Smithay ships
//! the window list (ext-foreign-toplevel-list, toplevels.rs) but not these two, so they are written here.
//!
//! Only the headless backend offers them today (headless.rs makes the pictures: a window drawn on its own, at its screen's scale,
//! whether it is covered, on another desktop or minimized; a screen from its picture in memory); the nested window and the future
//! hardware backend need a read-back from the GPU first. Shared memory only (ARGB8888 and XRGB8888); GPU buffers (dmabuf) come
//! with the hardware backend. While the session is locked no window is copied (a screen's copy is the lock screen's picture, as
//! with wlr-screencopy). The pointer is painted into a screen's copy when the program asks (`paint_cursors`); a session for the
//! pointer's own picture (create_pointer_cursor_session) is answered with a session that has stopped.

use std::sync::Mutex;

use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::image_capture_source::v1::server::{
    ext_foreign_toplevel_image_capture_source_manager_v1::{self, ExtForeignToplevelImageCaptureSourceManagerV1},
    ext_image_capture_source_v1::{self, ExtImageCaptureSourceV1},
    ext_output_image_capture_source_manager_v1::{self, ExtOutputImageCaptureSourceManagerV1},
};
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::{
    ext_image_copy_capture_cursor_session_v1::{self, ExtImageCopyCaptureCursorSessionV1},
    ext_image_copy_capture_frame_v1::{self, ExtImageCopyCaptureFrameV1, FailureReason},
    ext_image_copy_capture_manager_v1::{self, ExtImageCopyCaptureManagerV1, Options},
    ext_image_copy_capture_session_v1::{self, ExtImageCopyCaptureSessionV1},
};
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_output::Transform, wl_shm, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::state::App;

/// What is captured: a window (by its surface), a screen, or nothing any more (the window closed, the screen went).
#[derive(Clone)]
pub enum Source {
    Window(WlSurface),
    Output(Output),
    Gone,
}

/// One capture session: its source, what the program was last told (the buffer size), and whether there is something new to copy.
pub struct Session {
    pub resource: ExtImageCopyCaptureSessionV1,
    pub source: Source,
    /// the pointer painted in (a screen's copy)
    pub cursors: bool,
    pub told: (u32, u32),
    /// nothing copied yet in this session: the first copy is made at once
    pub fresh: bool,
    /// the source changed since the last copy (a later copy waits for that)
    pub dirty: bool,
    pub stopped: bool,
    /// the session's frame object, while it lives (one at a time)
    frame: Option<ExtImageCopyCaptureFrameV1>,
}

/// A copy asked for and not yet made: made after the next frame is drawn (headless.rs).
pub struct Pending {
    pub frame: ExtImageCopyCaptureFrameV1,
    pub session: ExtImageCopyCaptureSessionV1,
    pub buffer: WlBuffer,
}

#[derive(Default)]
pub struct Capture {
    pub sessions: Vec<Session>,
    pub pending: Vec<Pending>,
}

/// What a frame object knows until its copy is asked for: the buffer attached and whether `capture` was sent.
pub struct FrameData {
    session: ExtImageCopyCaptureSessionV1,
    inner: Mutex<(Option<WlBuffer>, bool)>,
}

/// A cursor session's one capture session may be asked for once.
pub struct CursorData(Mutex<bool>);

pub fn create_globals(dh: &DisplayHandle) {
    dh.create_global::<App, ExtOutputImageCaptureSourceManagerV1, ()>(1, ());
    dh.create_global::<App, ExtForeignToplevelImageCaptureSourceManagerV1, ()>(1, ());
    dh.create_global::<App, ExtImageCopyCaptureManagerV1, ()>(1, ());
}

impl App {
    /// The size in pixels a source's copy has now: a window's (its own size at its screen's scale) or a screen's mode. None when
    /// the source is gone.
    pub fn capture_size(&self, source: &Source) -> Option<(u32, u32)> {
        match source {
            Source::Window(s) => {
                let w = self.window_for(s)?;
                let geo = w.geometry();
                let scale = self.window_output(&w).current_scale().fractional_scale();
                let (pw, ph) = ((geo.size.w as f64 * scale).round() as u32, (geo.size.h as f64 * scale).round() as u32);
                (pw > 0 && ph > 0).then_some((pw, ph))
            }
            Source::Output(o) => {
                let on = self.heads.list.iter().any(|h| h.enabled && h.output == *o);
                let m = o.current_mode()?.size;
                (on && m.w > 0 && m.h > 0).then_some((m.w as u32, m.h as u32))
            }
            Source::Gone => None,
        }
    }

    /// A window's picture changed (a commit somewhere in its surface tree): its sessions have something new.
    pub fn capture_window_changed(&mut self, root: &WlSurface) {
        for s in &mut self.capture.sessions {
            if matches!(&s.source, Source::Window(w) if w == root) {
                s.dirty = true;
            }
        }
    }

    /// A screen's picture changed (drawn with damage), or the pointer moved over it.
    pub fn capture_output_changed(&mut self, o: &Output, pointer_only: bool) {
        for s in &mut self.capture.sessions {
            if matches!(&s.source, Source::Output(x) if x == o) && (!pointer_only || s.cursors) {
                s.dirty = true;
            }
        }
    }

    /// The window or screen went: its sessions stop, and the copies waiting on them fail.
    pub fn capture_source_gone(&mut self, gone: impl Fn(&Source) -> bool) {
        for s in &mut self.capture.sessions {
            if !s.stopped && gone(&s.source) {
                s.stopped = true;
                s.source = Source::Gone;
                s.resource.stopped();
            }
        }
        let stopped: Vec<ExtImageCopyCaptureSessionV1> =
            self.capture.sessions.iter().filter(|s| s.stopped).map(|s| s.resource.clone()).collect();
        self.capture.pending.retain(|p| {
            if stopped.contains(&p.session) {
                p.frame.failed(FailureReason::Stopped);
                false
            } else {
                true
            }
        });
    }

    fn new_session(&mut self, init: &mut DataInit<'_, Self>, session: New<ExtImageCopyCaptureSessionV1>, source: Source, cursors: bool) {
        let resource = init.init(session, ());
        let size = self.capture_size(&source);
        let stopped = size.is_none();
        if let Some(size) = size {
            constraints(&resource, size);
        } else {
            resource.stopped();
        }
        self.capture.sessions.push(Session {
            resource,
            source,
            cursors,
            told: size.unwrap_or((0, 0)),
            fresh: true,
            dirty: true,
            stopped,
            frame: None,
        });
    }
}

/// The buffer a session's copies need: its size, shared memory in ARGB8888 or XRGB8888.
pub fn constraints(session: &ExtImageCopyCaptureSessionV1, (w, h): (u32, u32)) {
    session.buffer_size(w, h);
    session.shm_format(wl_shm::Format::Argb8888);
    session.shm_format(wl_shm::Format::Xrgb8888);
    session.done();
}

/// Whether a program's buffer takes a copy of this size: shared memory, one of the two formats, the size, room for every row.
pub fn buffer_fits(buffer: &WlBuffer, (w, h): (u32, u32)) -> bool {
    matches!(
        smithay::wayland::shm::with_buffer_contents(buffer, |_, len, b| {
            matches!(b.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
                && b.width == w as i32
                && b.height == h as i32
                && b.stride >= w as i32 * 4
                && b.offset >= 0
                && (b.offset as usize + b.stride as usize * b.height as usize) <= len
        }),
        Ok(true)
    )
}

/// Copy a picture (rows of 4-byte pixels `stride` bytes apart, as large as the buffer) into a program's buffer, the pointer
/// painted in when given (its picture and its top-left corner in the copy's pixels), and tell the program it is ready.
pub fn fill(
    p: &Pending,
    picture: &[u8],
    stride: usize,
    (w, h): (u32, u32),
    pointer: Option<(&crate::pointer::Picture, (i32, i32))>,
    when: std::time::Duration,
) {
    let done = smithay::wayland::shm::with_buffer_contents_mut(&p.buffer, |ptr, len, b| {
        let (rows, dst_stride, offset) = (h as usize, b.stride as usize, b.offset as usize);
        if offset + dst_stride * rows > len {
            return false;
        }
        // SAFETY: the rows lie inside the pool (checked above); the program may write to its own memory meanwhile, which only
        // spoils its own picture
        let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), dst_stride * rows) };
        for row in 0..rows {
            let Some(src) = picture.get(row * stride..row * stride + w as usize * 4) else { return false };
            dst[row * dst_stride..row * dst_stride + w as usize * 4].copy_from_slice(src);
        }
        if let Some((pic, at)) = pointer {
            crate::pointer::blend(dst, dst_stride, (w, h), pic, at);
        }
        true
    });
    if !matches!(done, Ok(true)) {
        p.frame.failed(FailureReason::Unknown);
        return;
    }
    p.frame.transform(Transform::Normal);
    p.frame.damage(0, 0, w as i32, h as i32);
    let secs = when.as_secs();
    p.frame.presentation_time((secs >> 32) as u32, secs as u32, when.subsec_nanos());
    p.frame.ready();
}

impl GlobalDispatch<ExtOutputImageCaptureSourceManagerV1, ()> for App {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtOutputImageCaptureSourceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ExtOutputImageCaptureSourceManagerV1, ()> for App {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtOutputImageCaptureSourceManagerV1,
        request: ext_output_image_capture_source_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_output_image_capture_source_manager_v1::Request::CreateSource { source, output } = request {
            let s = Output::from_resource(&output).map(Source::Output).unwrap_or(Source::Gone);
            init.init(source, s);
        }
    }
}

impl GlobalDispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for App {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtForeignToplevelImageCaptureSourceManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ExtForeignToplevelImageCaptureSourceManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ExtForeignToplevelImageCaptureSourceManagerV1,
        request: ext_foreign_toplevel_image_capture_source_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_foreign_toplevel_image_capture_source_manager_v1::Request::CreateSource { source, toplevel_handle } = request {
            let s = state.window_of_list_handle(&toplevel_handle).map(Source::Window).unwrap_or(Source::Gone);
            init.init(source, s);
        }
    }
}

impl Dispatch<ExtImageCaptureSourceV1, Source> for App {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &ExtImageCaptureSourceV1,
        _: ext_image_capture_source_v1::Request,
        _: &Source,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<ExtImageCopyCaptureManagerV1, ()> for App {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<ExtImageCopyCaptureManagerV1>,
        _: &(),
        init: &mut DataInit<'_, Self>,
    ) {
        init.init(resource, ());
    }
}

impl Dispatch<ExtImageCopyCaptureManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        manager: &ExtImageCopyCaptureManagerV1,
        request: ext_image_copy_capture_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use ext_image_copy_capture_manager_v1::Request as R;
        match request {
            R::CreateSession { session, source, options } => {
                let cursors = match options {
                    smithay::reexports::wayland_server::WEnum::Value(o) => o.contains(Options::PaintCursors),
                    smithay::reexports::wayland_server::WEnum::Unknown(_) => {
                        manager.post_error(ext_image_copy_capture_manager_v1::Error::InvalidOption, "unknown options");
                        return;
                    }
                };
                let src = source.data::<Source>().cloned().unwrap_or(Source::Gone);
                state.new_session(init, session, src, cursors);
            }
            R::CreatePointerCursorSession { session, .. } => {
                init.init(session, CursorData(Mutex::new(false)));
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtImageCopyCaptureCursorSessionV1, CursorData> for App {
    fn request(
        _: &mut Self,
        _: &Client,
        cursor: &ExtImageCopyCaptureCursorSessionV1,
        request: ext_image_copy_capture_cursor_session_v1::Request,
        data: &CursorData,
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_cursor_session_v1::Request::GetCaptureSession { session } = request {
            let mut asked = data.0.lock().unwrap();
            if *asked {
                cursor.post_error(
                    ext_image_copy_capture_cursor_session_v1::Error::DuplicateSession,
                    "one capture session per cursor session",
                );
                return;
            }
            *asked = true;
            // the pointer's own picture is not offered (yet): the session stops at once
            init.init(session, ()).stopped();
        }
    }
}

impl Dispatch<ExtImageCopyCaptureSessionV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        session: &ExtImageCopyCaptureSessionV1,
        request: ext_image_copy_capture_session_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        if let ext_image_copy_capture_session_v1::Request::CreateFrame { frame } = request {
            let entry = state.capture.sessions.iter_mut().find(|s| &s.resource == session);
            if entry.as_ref().is_some_and(|s| s.frame.as_ref().is_some_and(|f| f.is_alive())) {
                session.post_error(ext_image_copy_capture_session_v1::Error::DuplicateFrame, "one frame at a time");
                return;
            }
            let f = init.init(frame, FrameData { session: session.clone(), inner: Mutex::new((None, false)) });
            if let Some(s) = entry {
                s.frame = Some(f);
            }
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, session: &ExtImageCopyCaptureSessionV1, _: &()) {
        state.capture.sessions.retain(|s| &s.resource != session);
        state.capture.pending.retain(|p| &p.session != session);
    }
}

impl Dispatch<ExtImageCopyCaptureFrameV1, FrameData> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        frame: &ExtImageCopyCaptureFrameV1,
        request: ext_image_copy_capture_frame_v1::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use ext_image_copy_capture_frame_v1::{Error as E, Request as R};
        let mut inner = data.inner.lock().unwrap();
        match request {
            R::AttachBuffer { buffer } => {
                if inner.1 {
                    frame.post_error(E::AlreadyCaptured, "attach_buffer after capture");
                    return;
                }
                inner.0 = Some(buffer);
            }
            R::DamageBuffer { x, y, width, height } => {
                if inner.1 {
                    frame.post_error(E::AlreadyCaptured, "damage_buffer after capture");
                } else if x < 0 || y < 0 || width <= 0 || height <= 0 {
                    frame.post_error(E::InvalidBufferDamage, "damage must have a size and start at 0 or more");
                }
            }
            R::Capture => {
                if inner.1 {
                    frame.post_error(E::AlreadyCaptured, "a frame captures once");
                    return;
                }
                inner.1 = true;
                let Some(buffer) = inner.0.clone() else {
                    frame.post_error(E::NoBuffer, "capture without a buffer");
                    return;
                };
                let Some(s) = state.capture.sessions.iter().find(|s| s.resource == data.session) else {
                    frame.failed(FailureReason::Stopped);
                    return;
                };
                if s.stopped {
                    frame.failed(FailureReason::Stopped);
                    return;
                }
                if !buffer_fits(&buffer, s.told) {
                    frame.failed(FailureReason::BufferConstraints);
                    return;
                }
                state.capture.pending.push(Pending { frame: frame.clone(), session: data.session.clone(), buffer });
                state.wake();
            }
            _ => {}
        }
    }

    fn destroyed(state: &mut Self, _: ClientId, frame: &ExtImageCopyCaptureFrameV1, _: &FrameData) {
        state.capture.pending.retain(|p| &p.frame != frame);
    }
}
