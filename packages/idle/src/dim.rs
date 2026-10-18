//! Dimming the screen by drawing over it: one see-through black layer-shell surface per screen, faded in and out. It takes no input
//! (clicks and keys go through to what is underneath, and count as activity), reserves no room, and disappears if this program ends,
//! so it can never leave the screen stuck dim (a monitor's own brightness set over DDC could). Works on every monitor.

use std::time::{Duration, Instant};

use wayland_client::globals::GlobalList;
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_output, wl_region, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};

use crate::App;

/// Seconds it takes the screen to dim, and to come back.
const FADE_IN: Duration = Duration::from_millis(900);
const FADE_OUT: Duration = Duration::from_millis(150);

struct Overlay {
    id: u32,
    wl: wl_surface::WlSurface,
    layer: ZwlrLayerSurfaceV1,
    viewport: WpViewport,
    configured: bool,
}

pub struct Dimmer {
    compositor: wl_compositor::WlCompositor,
    layer_shell: ZwlrLayerShellV1,
    viewporter: WpViewporter,
    pool: wl_shm_pool::WlShmPool,
    /// One 1x1 black pixel per opacity (0..=255), made when first needed.
    bufs: Vec<Option<wl_buffer::WlBuffer>>,
    overlays: Vec<Overlay>,
    next_id: u32,
    /// Opacity now, where it is heading, where it started, and when.
    pub alpha: f32,
    target: f32,
    from: f32,
    started: Instant,
    dur: Duration,
    shown: bool,
}

pub fn alpha_for(level_percent: u32) -> f32 {
    1.0 - (level_percent.clamp(10, 90) as f32 / 100.0)
}

impl Dimmer {
    pub fn new(globals: &GlobalList, qh: &QueueHandle<App>) -> Option<Dimmer> {
        let compositor = globals.bind::<wl_compositor::WlCompositor, _, _>(qh, 1..=6, ()).ok()?;
        let shm = globals.bind::<wl_shm::WlShm, _, _>(qh, 1..=1, ()).ok()?;
        let layer_shell = globals.bind::<ZwlrLayerShellV1, _, _>(qh, 1..=4, ()).ok()?;
        let viewporter = globals.bind::<WpViewporter, _, _>(qh, 1..=1, ()).ok()?;
        // a pool with one pixel per opacity
        let fd = unsafe { libc::memfd_create(c"labyrinth-idle-dim".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return None;
        }
        let file = unsafe { <std::fs::File as std::os::fd::FromRawFd>::from_raw_fd(fd) };
        file.set_len(256 * 4).ok()?;
        let mut px = Vec::with_capacity(1024);
        for a in 0..=255u32 {
            px.extend_from_slice(&(a << 24).to_le_bytes());
        }
        std::io::Write::write_all(&mut &file, &px).ok()?;
        let pool = shm.create_pool(std::os::fd::AsFd::as_fd(&file), 256 * 4, qh, ());
        Some(Dimmer { compositor, layer_shell, viewporter, pool, bufs: vec![None; 256], overlays: vec![], next_id: 1, alpha: 0.0, target: 0.0, from: 0.0, started: Instant::now(), dur: FADE_IN, shown: false })
    }

    pub fn is_shown(&self) -> bool {
        self.shown
    }

    /// Whether the fade needs another frame.
    pub fn animating(&self) -> bool {
        (self.alpha - self.target).abs() > 0.001 || self.overlays.iter().any(|o| !o.configured)
    }

    /// Fade the screens down to `alpha` opacity.
    pub fn show(&mut self, globals: &GlobalList, qh: &QueueHandle<App>, alpha: f32) {
        if self.overlays.is_empty() {
            let outputs: Vec<u32> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| g.name).collect());
            for name in outputs {
                let out = globals.registry().bind::<wl_output::WlOutput, _, _>(name, 1, qh, ());
                let wl = self.compositor.create_surface(qh, ());
                let id = self.next_id;
                self.next_id += 1;
                let layer = self.layer_shell.get_layer_surface(&wl, Some(&out), Layer::Overlay, "labyrinth-dim".into(), qh, id);
                layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
                layer.set_size(0, 0);
                layer.set_exclusive_zone(-1);
                layer.set_keyboard_interactivity(KeyboardInteractivity::None);
                // no input region: pointer and touch pass straight through
                let region = self.compositor.create_region(qh, ());
                wl.set_input_region(Some(&region));
                region.destroy();
                let viewport = self.viewporter.get_viewport(&wl, qh, ());
                wl.commit();
                self.overlays.push(Overlay { id, wl, layer, viewport, configured: false });
            }
        }
        self.fade(alpha, FADE_IN);
        self.shown = true;
    }

    /// Fade back to normal; the overlays go once they are clear.
    pub fn hide(&mut self) {
        if self.shown {
            self.fade(0.0, FADE_OUT);
            self.shown = false;
        }
    }

    fn fade(&mut self, to: f32, dur: Duration) {
        self.from = self.alpha;
        self.target = to;
        self.started = Instant::now();
        self.dur = dur;
    }

    pub fn configured(&mut self, id: u32, w: u32, h: u32, qh: &QueueHandle<App>) {
        if let Some(o) = self.overlays.iter_mut().find(|o| o.id == id) {
            o.configured = true;
            o.viewport.set_destination(w.max(1) as i32, h.max(1) as i32);
        }
        self.paint(qh);
    }

    pub fn closed(&mut self, id: u32) {
        if let Some(i) = self.overlays.iter().position(|o| o.id == id) {
            let o = self.overlays.remove(i);
            o.viewport.destroy();
            o.layer.destroy();
            o.wl.destroy();
        }
    }

    /// One animation step: move the opacity along and redraw.
    pub fn tick(&mut self, qh: &QueueHandle<App>) {
        let t = (self.started.elapsed().as_secs_f32() / self.dur.as_secs_f32().max(0.001)).clamp(0.0, 1.0);
        // ease out, so it settles gently
        let e = 1.0 - (1.0 - t) * (1.0 - t);
        self.alpha = self.from + (self.target - self.from) * e;
        if t >= 1.0 {
            self.alpha = self.target;
        }
        self.paint(qh);
        if !self.shown && self.alpha <= 0.001 {
            for o in self.overlays.drain(..) {
                o.viewport.destroy();
                o.layer.destroy();
                o.wl.destroy();
            }
        }
    }

    fn paint(&mut self, qh: &QueueHandle<App>) {
        let a = (self.alpha.clamp(0.0, 1.0) * 255.0).round() as usize;
        if self.bufs[a].is_none() {
            self.bufs[a] = Some(self.pool.create_buffer((a * 4) as i32, 1, 1, 4, wl_shm::Format::Argb8888, qh, ()));
        }
        let Some(buf) = self.bufs[a].as_ref() else { return };
        for o in self.overlays.iter().filter(|o| o.configured) {
            o.wl.attach(Some(buf), 0, 0);
            o.wl.damage_buffer(0, 0, 1, 1);
            o.wl.commit();
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, u32> for App {
    fn event(app: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, id: &u32, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                if let Some(d) = app.dimmer.as_mut() {
                    d.configured(*id, width, height, qh);
                }
            }
            zwlr_layer_surface_v1::Event::Closed => {
                if let Some(d) = app.dimmer.as_mut() {
                    d.closed(*id);
                }
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(App: ignore wl_compositor::WlCompositor);
wayland_client::delegate_noop!(App: ignore wl_shm::WlShm);
wayland_client::delegate_noop!(App: ignore wl_shm_pool::WlShmPool);
wayland_client::delegate_noop!(App: ignore wl_buffer::WlBuffer);
wayland_client::delegate_noop!(App: ignore wl_surface::WlSurface);
wayland_client::delegate_noop!(App: ignore wl_region::WlRegion);
wayland_client::delegate_noop!(App: ignore wl_output::WlOutput);
wayland_client::delegate_noop!(App: ignore ZwlrLayerShellV1);
wayland_client::delegate_noop!(App: ignore WpViewporter);
wayland_client::delegate_noop!(App: ignore WpViewport);

#[cfg(test)]
mod tests {
    use super::alpha_for;

    #[test]
    fn level_is_brightness_not_darkness() {
        assert!((alpha_for(30) - 0.7).abs() < 1e-6);
        assert!((alpha_for(100) - 0.1).abs() < 1e-6, "never fully bright: capped at 90");
        assert!((alpha_for(0) - 0.9).abs() < 1e-6, "never fully black");
    }
}
