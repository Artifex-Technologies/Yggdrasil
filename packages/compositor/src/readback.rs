//! Screen and window copies on the hardware backend (udev.rs): wlr-screencopy (screencopy.rs: grim, yggdrasil-shot, the colour
//! picker, Sunshine) and ext-image-copy-capture (capture.rs: the panel's window previews, and grim 1.5, which takes it first).
//! The headless backend copies from its pictures in memory; here a screen is on the GPU, in buffers the DrmCompositor owns
//! (and partly on planes), so a copy is drawn again: the same elements as the screen's frame (the lock screen's while locked,
//! black while the screen is off or dark), with the GPU's renderer (OpenGL ES, or pixman on a machine without one),
//! - into a picture of its own on the GPU, read back into the program's shared memory (XRGB8888 or ARGB8888; GL reads RGBA,
//!   whose red and blue are swapped here, and from the bottom row up when the mapping says so), the pointer painted in when
//!   asked, as headless; or
//! - straight into the program's GPU buffer (dmabuf: wlr-screencopy's linux_dmabuf, ext-image-copy-capture's dmabuf formats,
//!   offered with OpenGL ES only, whole screens and windows; the pointer is not painted into a GPU buffer).
//!
//! Nothing is drawn for copies while none is asked for. A copy that waits for a change (copy_with_damage, or a session's next
//! frame) waits until the screen's own frame had damage. Copies are made while another terminal shows too (the GPU draws without
//! the screens), so `grim` from another session does not hang.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use smithay::backend::allocator::dmabuf::Dmabuf;
use smithay::backend::allocator::{Buffer, Fourcc};
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{AsRenderElements, RenderElement};
use smithay::backend::renderer::{Bind, ExportMem, Offscreen, Renderer, Texture, TextureMapping};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::FailureReason;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Physical, Rectangle, Size, Transform};

use crate::capture::Source;
use crate::state::{App, Draw};

/// A picture read back: XRGB8888 (4-byte pixels, in memory blue, green, red, alpha), rows `stride` bytes apart.
pub struct Picture {
    pub data: Vec<u8>,
    pub stride: usize,
}

/// Pixels read from the GPU as they are to be handed on: `rgba` (GL's own order: red first in memory) has its red and blue
/// swapped into XRGB8888's order, and a `flipped` picture (its bottom row first) is turned the right way up.
pub fn to_xrgb(data: &[u8], w: usize, h: usize, rgba: bool, flipped: bool) -> Vec<u8> {
    let row = w * 4;
    let mut out = Vec::with_capacity(row * h);
    for y in 0..h {
        let src_y = if flipped { h - 1 - y } else { y };
        let Some(src) = data.get(src_y * row..(src_y + 1) * row) else { break };
        if rgba {
            for px in src.as_chunks::<4>().0 {
                out.extend_from_slice(&[px[2], px[1], px[0], px[3]]);
            }
        } else {
            out.extend_from_slice(src);
        }
    }
    out
}

/// Draw `elements` into a picture of the renderer's own (`T`, in `format`) of `size` pixels at `scale`, and read it back.
fn draw_and_read<R, T, E>(
    renderer: &mut R,
    format: Fourcc,
    size: Size<i32, Physical>,
    scale: f64,
    transform: Transform,
    elements: &[E],
    clear: [f32; 4],
) -> Result<Picture, String>
where
    R: Renderer + Offscreen<T> + Bind<T> + ExportMem,
    R::TextureId: Texture,
    E: RenderElement<R>,
{
    let mut target: T = renderer.create_buffer(format, (size.w, size.h).into()).map_err(|e| format!("a picture to copy into: {e}"))?;
    let mut fb = renderer.bind(&mut target).map_err(|e| e.to_string())?;
    let mut damage = OutputDamageTracker::new(size, scale, transform);
    damage.render_output(renderer, &mut fb, 0, elements, clear).map_err(|e| format!("{e:?}"))?;
    let region = Rectangle::from_size((size.w, size.h).into());
    // XRGB8888's own order where the GPU reads it (GL_EXT_read_format_bgra), else GL's RGBA, swapped here
    let (mapping, rgba) = match renderer.copy_framebuffer(&fb, region, Fourcc::Argb8888) {
        Ok(m) => (m, false),
        Err(_) => (renderer.copy_framebuffer(&fb, region, Fourcc::Abgr8888).map_err(|e| format!("read back: {e}"))?, true),
    };
    let flipped = mapping.flipped();
    let bytes = renderer.map_texture(&mapping).map_err(|e| format!("read back: {e}"))?;
    let (w, h) = (size.w as usize, size.h as usize);
    Ok(Picture { data: to_xrgb(bytes, w, h, rgba, flipped), stride: w * 4 })
}

/// Draw `elements` straight into a program's GPU buffer, and wait until the GPU is done with it.
fn draw_into<R, E>(
    renderer: &mut R,
    dmabuf: &Dmabuf,
    size: Size<i32, Physical>,
    scale: f64,
    transform: Transform,
    elements: &[E],
    clear: [f32; 4],
) -> Result<(), String>
where
    R: Renderer + Bind<Dmabuf>,
    R::TextureId: Texture,
    E: RenderElement<R>,
{
    if (dmabuf.width() as i32, dmabuf.height() as i32) != (size.w, size.h) {
        return Err(format!("a GPU buffer of {}x{} for a copy of {}x{}", dmabuf.width(), dmabuf.height(), size.w, size.h));
    }
    let mut d = dmabuf.clone();
    let mut fb = renderer.bind(&mut d).map_err(|e| e.to_string())?;
    let mut damage = OutputDamageTracker::new(size, scale, transform);
    let r = damage.render_output(renderer, &mut fb, 0, elements, clear).map_err(|e| format!("{e:?}"))?;
    let _ = r.sync.wait();
    Ok(())
}

/// What a screen's copy shows: its frame's elements and background; nothing on black while it is `dark` (off, switched off
/// in Settings, or the lid closed on it).
fn screen_elements<R: Draw>(app: &mut App, renderer: &mut R, o: &Output, dark: bool) -> (Vec<R::Elem>, [f32; 4])
where
    R::TextureId: Send + Clone + 'static,
{
    if dark {
        (vec![], crate::udev::BLACK)
    } else if app.locked {
        (app.lock_elements(renderer, o).into_iter().map(Into::into).collect(), crate::udev::BLACK)
    } else {
        (app.scene(renderer, o), crate::udev::BACKGROUND)
    }
}

/// A screen's size in pixels, scale and turn, for drawing a copy of it.
fn geometry(o: &Output) -> Option<(Size<i32, Physical>, f64, Transform)> {
    let m = o.current_mode()?;
    Some((m.size, o.current_scale().fractional_scale(), o.current_transform()))
}

/// The copies asked for, made now. `changed`: the screens whose frame had damage since their last copy that waited for one;
/// `dark`: whether a screen is not lit; `format`: the renderer's own pictures' format.
pub fn serve<R, T>(renderer: &mut R, format: Fourcc, app: &mut App, changed: &mut HashSet<String>, dark: &dyn Fn(&App, &Output) -> bool)
where
    R: Draw + Offscreen<T> + Bind<T> + Bind<Dmabuf> + ExportMem,
    R::TextureId: Texture + Send + Clone + 'static,
{
    if app.screencopy.pending.is_empty() && app.capture.pending.is_empty() {
        return;
    }
    let when: Duration = smithay::utils::Clock::<smithay::utils::Monotonic>::new().now().into();
    let mut pictures: HashMap<String, Picture> = HashMap::new();
    // a screen's picture, drawn once for every copy of it in this pass
    let mut picture = |app: &mut App, renderer: &mut R, o: &Output| -> Option<(Vec<u8>, usize)> {
        if let Some(p) = pictures.get(&o.name()) {
            return Some((p.data.clone(), p.stride));
        }
        let (size, scale, transform) = geometry(o)?;
        let d = dark(app, o);
        let (els, clear) = screen_elements(app, renderer, o, d);
        match draw_and_read::<R, T, R::Elem>(renderer, format, size, scale, transform, &els, clear) {
            Ok(p) => {
                let out = (p.data.clone(), p.stride);
                pictures.insert(o.name(), p);
                Some(out)
            }
            Err(e) => {
                eprintln!("yggdrasil-compositor: a copy of {}: {e}", o.name());
                None
            }
        }
    };

    // wlr-screencopy
    let mut served: Vec<String> = vec![];
    for c in std::mem::take(&mut app.screencopy.pending) {
        if !app.space.outputs().any(|o| *o == c.output) {
            c.frame.failed();
            continue;
        }
        let name = c.output.name();
        let key = c.frame.client().map(|cl| (cl.id(), name.clone()));
        let first = key.as_ref().is_some_and(|k| !app.screencopy.seen.contains(k));
        if c.with_damage && !first && !changed.contains(&name) {
            app.screencopy.pending.push(c);
            continue;
        }
        if let Some(k) = key {
            app.screencopy.seen.insert(k);
        }
        if c.with_damage {
            served.push(name.clone());
        }
        if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(&c.buffer).cloned() {
            let Some((size, scale, transform)) = geometry(&c.output) else {
                c.frame.failed();
                continue;
            };
            let d = dark(app, &c.output);
            let (els, clear) = screen_elements(app, renderer, &c.output, d);
            match draw_into(renderer, &dmabuf, size, scale, transform, &els, clear) {
                Ok(()) => crate::screencopy::ready(&c, when),
                Err(e) => {
                    eprintln!("yggdrasil-compositor: a copy of {name}: {e}");
                    c.frame.failed();
                }
            }
            continue;
        }
        let Some((data, stride)) = picture(app, renderer, &c.output) else {
            c.frame.failed();
            continue;
        };
        let pointer = if c.cursor { app.pointer_picture(&c.output) } else { None };
        crate::screencopy::fill(&c, &data, stride, pointer.as_ref().map(|(p, at)| (&**p, *at)), when);
    }
    for n in served {
        changed.remove(&n);
    }

    // ext-image-copy-capture: screens and windows
    for p in std::mem::take(&mut app.capture.pending) {
        let Some(i) = app.capture.sessions.iter().position(|s| s.resource == p.session) else {
            p.frame.failed(FailureReason::Stopped);
            continue;
        };
        let source = app.capture.sessions[i].source.clone();
        let size = app.capture_size(&source);
        let s = &mut app.capture.sessions[i];
        let Some(size) = size else {
            if !s.stopped {
                s.stopped = true;
                s.resource.stopped();
            }
            p.frame.failed(FailureReason::Stopped);
            continue;
        };
        if size != s.told {
            s.told = size;
            app.capture_constraints(i);
            p.frame.failed(FailureReason::BufferConstraints);
            continue;
        }
        if !s.fresh && !s.dirty {
            app.capture.pending.push(p);
            continue;
        }
        s.fresh = false;
        s.dirty = false;
        let cursors = s.cursors;
        let dmabuf = smithay::wayland::dmabuf::get_dmabuf(&p.buffer).cloned().ok();
        match source {
            Source::Output(o) => {
                if let Some(d) = dmabuf {
                    let Some((psize, scale, transform)) = geometry(&o) else {
                        p.frame.failed(FailureReason::Unknown);
                        continue;
                    };
                    let dk = dark(app, &o);
                    let (els, clear) = screen_elements(app, renderer, &o, dk);
                    match draw_into(renderer, &d, psize, scale, transform, &els, clear) {
                        Ok(()) => crate::capture::ready(&p, size, when),
                        Err(e) => {
                            eprintln!("yggdrasil-compositor: a copy of {}: {e}", o.name());
                            p.frame.failed(FailureReason::Unknown);
                        }
                    }
                    continue;
                }
                let Some((data, stride)) = picture(app, renderer, &o) else {
                    p.frame.failed(FailureReason::Unknown);
                    continue;
                };
                let pointer = if cursors { app.pointer_picture(&o) } else { None };
                crate::capture::fill(&p, &data, stride, size, pointer.as_ref().map(|(p, at)| (&**p, *at)), when);
            }
            Source::Window(surface) => {
                // nothing of the session is copied while it is locked
                let Some(w) = app.window_for(&surface).filter(|_| !app.locked) else {
                    p.frame.failed(FailureReason::Unknown);
                    continue;
                };
                let scale = app.window_output(&w).current_scale().fractional_scale();
                let geo = w.geometry();
                let at = smithay::utils::Point::<i32, Physical>::from((
                    -(geo.loc.x as f64 * scale).round() as i32,
                    -(geo.loc.y as f64 * scale).round() as i32,
                ));
                let els: Vec<WaylandSurfaceRenderElement<R>> = w.render_elements(renderer, at, smithay::utils::Scale::from(scale), 1.0);
                let psize = Size::<i32, Physical>::from((size.0 as i32, size.1 as i32));
                let clear = [0.0f32, 0.0, 0.0, 0.0];
                let done = match dmabuf {
                    Some(d) => draw_into(renderer, &d, psize, scale, Transform::Normal, &els, clear).map(|_| None),
                    None => draw_and_read::<R, T, _>(renderer, format, psize, scale, Transform::Normal, &els, clear).map(Some),
                };
                match done {
                    Ok(Some(pic)) => crate::capture::fill(&p, &pic.data, pic.stride, size, None, when),
                    Ok(None) => crate::capture::ready(&p, size, when),
                    Err(e) => {
                        eprintln!("yggdrasil-compositor: a window's copy: {e}");
                        p.frame.failed(FailureReason::Unknown);
                    }
                }
            }
            Source::Gone => p.frame.failed(FailureReason::Stopped),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::to_xrgb;

    #[test]
    fn pixels_read_from_the_gpu_are_handed_on_in_xrgb_order_and_upright() {
        // two rows of two pixels, RGBA as GL reads them: red, green / blue, white
        let rgba = [255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255];
        let x = to_xrgb(&rgba, 2, 2, true, false);
        assert_eq!(&x[..4], &[0, 0, 255, 255], "red is the third byte in memory");
        assert_eq!(&x[8..12], &[255, 0, 0, 255], "blue the first");
        let f = to_xrgb(&rgba, 2, 2, true, true);
        assert_eq!(&f[..4], &[255, 0, 0, 255], "the bottom row first: turned upright");
        assert_eq!(to_xrgb(&x, 2, 2, false, false), x, "already in order");
        assert_eq!(to_xrgb(&rgba[..12], 2, 2, true, false).len(), 8, "a short picture: the rows there are");
    }
}
