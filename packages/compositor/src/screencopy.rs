//! wlr-screencopy (version 3): a program asks for a copy of an output, or of a part of it, in a shared-memory buffer of its own.
//! grim (and with it yggdrasil-shot and the colour picker) and Sunshine take screenshots and stream the screen this way on
//! Hyprland. Smithay does not ship this protocol, so it is written here, like wlr-foreign-toplevel in toplevels.rs.
//!
//! Only the headless backend offers it today (headless.rs fills the copies from its pictures in memory); the nested window and the
//! future hardware backend need a read-back from the GPU first. While the session is locked the picture is the lock screen's, so a
//! copy never shows what the lock hides. Copies go in XRGB8888 (ARGB8888 is accepted too), shared memory only (no dmabuf yet).
//! The pointer is painted in when the program asks (overlay_cursor: grim -c), as on Hyprland.

use std::collections::HashSet;
use std::sync::Mutex;

use smithay::output::Output;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::protocol::{wl_buffer::WlBuffer, wl_shm};
use smithay::reexports::wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};
use smithay::utils::{Physical, Rectangle};
use wayland_protocols_wlr::screencopy::v1::server::{
    zwlr_screencopy_frame_v1::{self, ZwlrScreencopyFrameV1},
    zwlr_screencopy_manager_v1::{self, ZwlrScreencopyManagerV1},
};

use crate::state::App;

/// One copy asked for and not yet made: made after the next frame is drawn (headless.rs).
pub struct Copy {
    pub frame: ZwlrScreencopyFrameV1,
    pub buffer: WlBuffer,
    pub output: Output,
    /// the part of the output's picture, in its pixels
    pub region: Rectangle<i32, Physical>,
    /// copy_with_damage: wait until something on the output changed (except for the first copy of a program)
    pub with_damage: bool,
    /// the pointer painted in
    pub cursor: bool,
}

#[derive(Default)]
pub struct Screencopy {
    pub pending: Vec<Copy>,
    /// (program, output) pairs that have had a copy: their next copy_with_damage waits for a change
    pub seen: HashSet<(ClientId, String)>,
}

/// What a frame object knows: the output and part asked for, and whether a copy was already asked of it.
pub struct FrameData {
    output: Option<Output>,
    region: Rectangle<i32, Physical>,
    cursor: bool,
    used: Mutex<bool>,
}

pub fn create_global(dh: &DisplayHandle) {
    dh.create_global::<App, ZwlrScreencopyManagerV1, ()>(3, ());
}

/// The part of an output's picture that a logical rectangle (relative to the output's corner) covers, cut to the picture.
pub fn physical_region(output: &Output, logical: Option<(i32, i32, i32, i32)>) -> Option<Rectangle<i32, Physical>> {
    let mode = output.current_mode()?.size;
    let whole = Rectangle::<i32, Physical>::from_size((mode.w, mode.h).into());
    let Some((x, y, w, h)) = logical else { return Some(whole) };
    if w <= 0 || h <= 0 {
        return None;
    }
    let s = output.current_scale().fractional_scale();
    let (x0, y0) = ((x as f64 * s).round() as i32, (y as f64 * s).round() as i32);
    let (x1, y1) = (((x + w) as f64 * s).round() as i32, ((y + h) as f64 * s).round() as i32);
    Rectangle::<i32, Physical>::new((x0, y0).into(), (x1 - x0, y1 - y0).into()).intersection(whole).filter(|r| !r.is_empty())
}

impl App {
    fn new_frame(
        &mut self,
        init: &mut DataInit<'_, Self>,
        frame: New<ZwlrScreencopyFrameV1>,
        output: Option<Output>,
        logical: Option<(i32, i32, i32, i32)>,
        cursor: bool,
    ) {
        let region = output.as_ref().and_then(|o| physical_region(o, logical));
        let data = FrameData { output: output.clone(), region: region.unwrap_or_default(), cursor, used: Mutex::new(false) };
        let frame = init.init(frame, data);
        let Some(region) = region.filter(|_| output.is_some()) else {
            frame.failed();
            return;
        };
        let (w, h) = (region.size.w as u32, region.size.h as u32);
        frame.buffer(wl_shm::Format::Xrgb8888, w, h, w * 4);
        if frame.version() >= 3 {
            frame.buffer_done();
        }
    }
}

impl GlobalDispatch<ZwlrScreencopyManagerV1, ()> for App {
    fn bind(_: &mut Self, _: &DisplayHandle, _: &Client, resource: New<ZwlrScreencopyManagerV1>, _: &(), init: &mut DataInit<'_, Self>) {
        init.init(resource, ());
    }
}

impl Dispatch<ZwlrScreencopyManagerV1, ()> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &ZwlrScreencopyManagerV1,
        request: zwlr_screencopy_manager_v1::Request,
        _: &(),
        _: &DisplayHandle,
        init: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_manager_v1::Request as R;
        match request {
            R::CaptureOutput { frame, output, overlay_cursor } => {
                let o = Output::from_resource(&output);
                state.new_frame(init, frame, o, None, overlay_cursor != 0);
            }
            R::CaptureOutputRegion { frame, output, x, y, width, height, overlay_cursor } => {
                let o = Output::from_resource(&output);
                state.new_frame(init, frame, o, Some((x, y, width, height)), overlay_cursor != 0);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, FrameData> for App {
    fn request(
        state: &mut Self,
        _: &Client,
        frame: &ZwlrScreencopyFrameV1,
        request: zwlr_screencopy_frame_v1::Request,
        data: &FrameData,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        use zwlr_screencopy_frame_v1::Request as R;
        let (buffer, with_damage) = match request {
            R::Copy { buffer } => (buffer, false),
            R::CopyWithDamage { buffer } => (buffer, true),
            _ => return,
        };
        {
            let mut used = data.used.lock().unwrap();
            if *used {
                frame.post_error(zwlr_screencopy_frame_v1::Error::AlreadyUsed, "a frame copies once");
                return;
            }
            *used = true;
        }
        let Some(output) = data.output.clone() else {
            frame.failed();
            return;
        };
        let r = data.region;
        let fits = smithay::wayland::shm::with_buffer_contents(&buffer, |_, len, b| {
            matches!(b.format, wl_shm::Format::Xrgb8888 | wl_shm::Format::Argb8888)
                && b.width == r.size.w
                && b.height == r.size.h
                && b.stride >= r.size.w * 4
                && b.offset >= 0
                && (b.offset as usize + b.stride as usize * b.height as usize) <= len
        });
        if !matches!(fits, Ok(true)) {
            frame.post_error(
                zwlr_screencopy_frame_v1::Error::InvalidBuffer,
                "the buffer must be shared memory, XRGB8888, of the size given",
            );
            return;
        }
        state.screencopy.pending.push(Copy { frame: frame.clone(), buffer, output, region: r, with_damage, cursor: data.cursor });
        state.wake();
    }

    fn destroyed(state: &mut Self, _: ClientId, frame: &ZwlrScreencopyFrameV1, _: &FrameData) {
        state.screencopy.pending.retain(|c| &c.frame != frame);
    }
}

/// Fill a copy from an output's picture (XRGB8888 rows `stride` bytes apart), the pointer painted in when given (its picture and
/// its top-left corner in the output's pixels), and tell the program it is ready.
pub fn fill(
    copy: &Copy,
    picture: &[u8],
    stride: usize,
    pointer: Option<(&crate::pointer::Picture, (i32, i32))>,
    when: std::time::Duration,
) {
    let r = copy.region;
    let done = smithay::wayland::shm::with_buffer_contents_mut(&copy.buffer, |ptr, len, b| {
        let (w, h, dst_stride, offset) = (r.size.w as usize, r.size.h as usize, b.stride as usize, b.offset as usize);
        if offset + dst_stride * h > len {
            return false;
        }
        for row in 0..h {
            let from = (r.loc.y as usize + row) * stride + r.loc.x as usize * 4;
            let Some(src) = picture.get(from..from + w * 4) else { return false };
            // SAFETY: the destination row lies inside the pool (checked above); the program may write to its own memory meanwhile,
            // which only spoils its own picture
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), ptr.add(offset + row * dst_stride), w * 4) };
        }
        if let Some((pic, at)) = pointer {
            // SAFETY: as above, the rows lie inside the pool
            let dst = unsafe { std::slice::from_raw_parts_mut(ptr.add(offset), dst_stride * h) };
            crate::pointer::blend(dst, dst_stride, (w as u32, h as u32), pic, (at.0 - r.loc.x, at.1 - r.loc.y));
        }
        true
    });
    if !matches!(done, Ok(true)) {
        copy.frame.failed();
        return;
    }
    copy.frame.flags(zwlr_screencopy_frame_v1::Flags::empty());
    if copy.with_damage && copy.frame.version() >= 2 {
        copy.frame.damage(0, 0, r.size.w as u32, r.size.h as u32);
    }
    let secs = when.as_secs();
    copy.frame.ready((secs >> 32) as u32, secs as u32, when.subsec_nanos());
}
