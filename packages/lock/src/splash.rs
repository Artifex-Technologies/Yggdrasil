//! yggdrasil-splash: the screen between signing in and the desktop. Hyprland starts it first thing; it covers every screen with
//! falling columns of characters in the accent colour (Labyrinth: purple) behind the operating system's logo, and fades out once
//! the top bar is up (at the latest after twelve seconds), so the start-up is never a black screen or a console.
//!
//! What it shows is chosen in Settings > Appearance > Boot screen (~/.config/yggdrasil/splash.conf):
//!   mode = animation | messages | picture | off      picture = /path/to/picture
//! "messages" shows what the session is starting (its log), "picture" a picture of your own, "off" nothing at all.
//! It takes no clicks and no keys: what is under it gets them.
//!
//! `yggdrasil-splash --screensaver` is the screensaver (the idle watcher starts it after the time set in Settings > Power and
//! ends it when you are back): the same falling characters, the logo fading away after a few seconds so nothing stands still
//! on the screen, and the key or click that wakes it goes to it, not to the program underneath.
#![allow(dead_code)]

mod design;
mod draw;

use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use memmap2::MmapMut;
use tiny_skia::{Pixmap, PixmapMut, PixmapPaint, Transform};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::{self, WlBuffer}, wl_callback::{self, WlCallback}, wl_compositor::WlCompositor, wl_output::WlOutput, wl_region::WlRegion, wl_registry, wl_shm::{self, WlShm}, wl_shm_pool::WlShmPool, wl_surface::WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::alpha_modifier::v1::client::{wp_alpha_modifier_surface_v1::WpAlphaModifierSurfaceV1, wp_alpha_modifier_v1::WpAlphaModifierV1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1::{self, ZwlrLayerShellV1}, zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1}};

const CELL_W: f32 = 18.0;
const CELL_H: f32 = 24.0;
const GLYPHS: &str = "0123456789ABCDEFGHJKLMNPQRSTUVWXYZ<>=+*:.|ΞΣΛΠΦΨΩ";
/// Brightness steps a character can be drawn at (0 faint ... LEVELS-1 the bright head).
const LEVELS: usize = 10;
/// It shows at least this long, so it is seen (a fast start has the top bar up within a second).
const SHORTEST: Duration = Duration::from_millis(2500);
const LONGEST: Duration = Duration::from_secs(12);
const FADE: Duration = Duration::from_millis(450);
/// At most this many frames a second (the compositor's own pace below it): smooth, and light enough on a big screen.
const FRAME: Duration = Duration::from_micros(16_600);

#[derive(Clone, PartialEq, Debug)]
enum Mode {
    Animation,
    Messages,
    Picture(std::path::PathBuf),
    Off,
}

/// ~/.config/yggdrasil/splash.conf, as `key = value` lines.
fn mode_from(text: &str) -> Mode {
    let get = |k: &str| text.lines().filter_map(|l| l.split('#').next()?.split_once('=')).find(|(a, _)| a.trim() == k).map(|(_, v)| v.trim().to_string());
    match get("mode").as_deref() {
        Some("off") | Some("nothing") => Mode::Off,
        Some("messages") => Mode::Messages,
        Some("picture") => match get("picture").map(std::path::PathBuf::from).filter(|p| p.is_file()) {
            Some(p) => Mode::Picture(p),
            None => Mode::Animation,
        },
        _ => Mode::Animation,
    }
}

fn mode() -> Mode {
    mode_from(&std::fs::read_to_string(yggdrasil_conf::dirs::config().join("splash.conf")).unwrap_or_default())
}

/// The operating system's logo for this screen: its branding's splash_logo (a picture), if it has one.
fn logo() -> Option<Pixmap> {
    for f in std::fs::read_dir("/usr/share/yggdrasil/integrations").ok()?.flatten() {
        let text = std::fs::read_to_string(f.path()).unwrap_or_default();
        let get = |k: &str| text.lines().filter_map(|l| l.split('#').next()?.split_once('=')).find(|(a, _)| a.trim() == k).map(|(_, v)| v.trim().to_string());
        if get("kind").as_deref() == Some("branding") {
            if let Some(p) = get("splash_logo") {
                return design::load_picture(std::path::Path::new(&p));
            }
        }
    }
    None
}

/// One character drawn white on a transparent cell, at each brightness: tinted copies, so drawing is a plain copy.
struct Atlas {
    cells: Vec<[Pixmap; LEVELS]>,
}

fn font_file(pattern: &str) -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let p = std::path::PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    p.is_file().then_some(p)
}

/// The colour of brightness step `level`: the accent fading in, then lighter towards the head.
fn level_color(accent: (u8, u8, u8), level: usize) -> (u8, u8, u8, u8) {
    let k = level as f32 / (LEVELS - 1) as f32;
    if level == LEVELS - 1 {
        return (240, 234, 255, 255);
    }
    let light = |c: u8| (c as f32 + (255.0 - c as f32) * (k * k * 0.45)) as u8;
    (light(accent.0), light(accent.1), light(accent.2), (25.0 + 225.0 * k.powf(1.3)) as u8)
}

impl Atlas {
    fn new(accent: (u8, u8, u8)) -> Option<Atlas> {
        let font = FontVec::try_from_vec(std::fs::read(font_file("monospace")?).ok()?).ok()?;
        let scaled = font.as_scaled(PxScale::from(19.0));
        let mut cells = vec![];
        for (i, ch) in GLYPHS.chars().enumerate() {
            // the alpha of the character, mirrored for some like the film's
            let (w, h) = (CELL_W as usize, CELL_H as usize);
            let mut alpha = vec![0f32; w * h];
            let mut g = scaled.scaled_glyph(ch);
            let adv = scaled.h_advance(g.id);
            g.position = point((CELL_W - adv) / 2.0, 1.0 + scaled.ascent());
            if let Some(o) = font.outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|x, y, c| {
                    let (px, py) = (b.min.x as i32 + x as i32, b.min.y as i32 + y as i32);
                    if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                        let px = if i % 5 < 3 { w - 1 - px as usize } else { px as usize };
                        alpha[py as usize * w + px] = c.clamp(0.0, 1.0);
                    }
                });
            }
            let levels: [Pixmap; LEVELS] = std::array::from_fn(|l| {
                let (r, gg, bb, a) = level_color(accent, l);
                let mut p = Pixmap::new(w as u32, h as u32).expect("cell");
                for (px, &c) in p.pixels_mut().iter_mut().zip(alpha.iter()) {
                    let aa = c * a as f32 / 255.0;
                    let pm = |v: u8| (v as f32 * aa) as u8;
                    // blue and red swapped: the buffer is B, G, R, A in memory and this is drawn straight into it
                    *px = tiny_skia::PremultipliedColorU8::from_rgba(pm(bb), pm(gg), pm(r), (255.0 * aa) as u8).unwrap_or(*px);
                }
                p
            });
            cells.push(levels);
        }
        Some(Atlas { cells })
    }
}

/// A picture with red and blue swapped, for drawing straight into the compositor's B, G, R, A buffer.
fn bgra(mut p: Pixmap) -> Pixmap {
    for px in p.pixels_mut() {
        *px = tiny_skia::PremultipliedColorU8::from_rgba(px.blue(), px.green(), px.red(), px.alpha()).unwrap_or(*px);
    }
    p
}

/// Draws a premultiplied picture (already in the buffer's byte order) onto the buffer at (x, y). A plain loop: for thousands
/// of small characters a frame it costs a fraction of a general drawing call each.
fn blit(dst: &mut [u8], dw: u32, dh: u32, src: &Pixmap, x: i32, y: i32) {
    blit_faded(dst, dw, dh, src, x, y, 256);
}

/// The same with the picture made fainter: `k` of 256 (256 = as it is).
fn blit_faded(dst: &mut [u8], dw: u32, dh: u32, src: &Pixmap, x: i32, y: i32, k: u32) {
    if k == 0 {
        return;
    }
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    let (x0, x1, y0, y1) = (x.max(0), (x + sw).min(dw as i32), y.max(0), (y + sh).min(dh as i32));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let data = src.data();
    for py in y0..y1 {
        let sy = (py - y) as usize;
        let s0 = (sy * sw as usize + (x0 - x) as usize) * 4;
        let d0 = (py as usize * dw as usize + x0 as usize) * 4;
        let n = (x1 - x0) as usize * 4;
        for (d, sp) in dst[d0..d0 + n].chunks_exact_mut(4).zip(data[s0..s0 + n].chunks_exact(4)) {
            if k < 256 {
                let a = (sp[3] as u32 * k) >> 8;
                if a == 0 {
                    continue;
                }
                let inv = 255 - a;
                for c in 0..4 {
                    d[c] = (((sp[c] as u32 * k) >> 8) + (d[c] as u32 * inv + 127) / 255) as u8;
                }
                continue;
            }
            let a = sp[3] as u32;
            if a == 0 {
                continue;
            }
            if a == 255 {
                d.copy_from_slice(sp);
                continue;
            }
            let inv = 255 - a;
            for k in 0..4 {
                d[k] = (sp[k] as u32 + (d[k] as u32 * inv + 127) / 255) as u8;
            }
        }
    }
}

/// The whole buffer one colour (bytes in the buffer's order), as fast as a memory fill.
fn clear(dst: &mut [u8], px: [u8; 4]) {
    // SAFETY: any 4 bytes are a valid u32; align_to_mut only hands out the aligned middle as u32
    let (pre, mid, post) = unsafe { dst.align_to_mut::<u32>() };
    if pre.is_empty() && post.is_empty() {
        mid.fill(u32::from_ne_bytes(px));
    } else {
        for c in dst.chunks_exact_mut(4) {
            c.copy_from_slice(&px);
        }
    }
}

/// A small, fast random source (no crate needed for a screen of falling characters).
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn f(&mut self) -> f32 {
        (self.next() % 1_000_000) as f32 / 1_000_000.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

/// One falling column: its characters (head last), where its head is, and how fast it falls (whole pixels a frame).
struct Column {
    chars: Vec<usize>,
    head: f32,
    speed: f32,
}

fn new_column(rng: &mut Rng, h: f32, glyphs: usize, start: bool) -> Column {
    let len = 10 + rng.below(25);
    let head = if start { rng.f() * h * 1.4 - h * 0.4 } else { -rng.f() * h * 0.5 };
    // a whole number of pixels each frame (2 to 7): every step the same size, so the fall looks even, not 4, 4, 3, 4 ...
    Column { chars: (0..len).map(|_| rng.below(glyphs)).collect(), head: head.round(), speed: (2 + rng.below(6)) as f32 }
}

struct Screen {
    output: WlOutput,
    surface: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    size: (u32, u32),
    configured: bool,
    /// Two buffers in one shared-memory file: one shown, one drawn into.
    bufs: Vec<(WlBuffer, bool)>,
    map: Option<MmapMut>,
    pix: Option<Pixmap>,
    cols: Vec<Column>,
    /// The compositor's own see-through setting for this surface (the fade costs nothing with it); None without the protocol.
    alpha: Option<WpAlphaModifierSurfaceV1>,
    /// The compositor asked for the next frame (a frame callback came), and when the last one was drawn.
    wants_frame: bool,
    last: Option<Instant>,
}

struct App {
    qh: QueueHandle<App>,
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    screens: Vec<Screen>,
    mode: Mode,
    atlas: Option<Atlas>,
    logo: Option<Pixmap>,
    picture: Option<Pixmap>,
    font: Option<draw::Fonts>,
    rng: Rng,
    started: Instant,
    fading: Option<Instant>,
    /// The screensaver: runs until it is ended, takes the input that wakes it, the logo fades away.
    screensaver: bool,
    /// Set by a thread of its own once Hyprland lists the top bar (asking it never holds up a frame).
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    accent: (u8, u8, u8, u8),
    /// YGGDRASIL_SPLASH_STATS=1: the time between frames, printed at the end (for checking the pace).
    stats: Option<Vec<f32>>,
    draw_ms: Option<Vec<f32>>,
    done: bool,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(App: WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlOutput);
delegate_noop!(App: WlRegion);
delegate_noop!(App: ZwlrLayerShellV1);
delegate_noop!(App: WpAlphaModifierV1);
delegate_noop!(App: WpAlphaModifierSurfaceV1);

impl Dispatch<WlCallback, usize> for App {
    fn event(app: &mut Self, _: &WlCallback, e: wl_callback::Event, idx: &usize, _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = e {
            app.on_frame(*idx, qh);
        }
    }
}

impl Dispatch<WlBuffer, (usize, usize)> for App {
    fn event(app: &mut Self, _: &WlBuffer, e: wl_buffer::Event, idx: &(usize, usize), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = e {
            if let Some(b) = app.screens.get_mut(idx.0).and_then(|s| s.bufs.get_mut(idx.1)) {
                b.1 = false;
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for App {
    fn event(app: &mut Self, l: &ZwlrLayerSurfaceV1, e: zwlr_layer_surface_v1::Event, idx: &usize, _: &Connection, qh: &QueueHandle<Self>) {
        match e {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                l.ack_configure(serial);
                let i = *idx;
                if let Some(s) = app.screens.get_mut(i) {
                    if (width, height) != s.size && width > 0 && height > 0 {
                        s.size = (width, height);
                        s.bufs.clear();
                        s.map = None;
                        s.pix = None;
                        s.cols.clear();
                    }
                    s.configured = true;
                }
                app.prepare(i, qh);
                app.draw(i);
            }
            zwlr_layer_surface_v1::Event::Closed => app.done = true,
            _ => {}
        }
    }
}

impl App {
    /// The screen's buffers and columns, once its size is known.
    fn prepare(&mut self, i: usize, qh: &QueueHandle<App>) {
        let (w, h) = self.screens[i].size;
        if w == 0 || h == 0 || !self.screens[i].bufs.is_empty() {
            return;
        }
        let stride = w * 4;
        let one = (stride * h) as usize;
        let Ok(fd) = rustix::fs::memfd_create(c"yggdrasil-splash", rustix::fs::MemfdFlags::CLOEXEC) else { return };
        if rustix::fs::ftruncate(&fd, (one * 2) as u64).is_err() {
            return;
        }
        let file = std::fs::File::from(fd);
        let Ok(map) = (unsafe { MmapMut::map_mut(&file) }) else { return };
        let pool = self.shm.create_pool(file.as_fd(), (one * 2) as i32, qh, ());
        let bufs = (0..2).map(|k| (pool.create_buffer((k * one) as i32, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, qh, (i, k)), false)).collect();
        pool.destroy();
        let glyphs = GLYPHS.chars().count();
        let n = (w as f32 / CELL_W) as usize + 1;
        let cols = (0..n).map(|_| new_column(&mut self.rng, h as f32, glyphs, true)).collect();
        let s = &mut self.screens[i];
        s.bufs = bufs;
        s.map = Some(map);
        s.pix = if self.mode == Mode::Messages { Pixmap::new(w, h) } else { None };
        s.cols = cols;
    }

    /// The compositor is about to show its next frame on this screen (it says so once per refresh): draw now, in step with the
    /// screen, or, when the last one was drawn less than a frame's time ago (a 180 Hz screen asks 180 times a second), ask
    /// again for the refresh after this one. So the picture moves on at a steady pace on every refresh it changes on.
    fn on_frame(&mut self, i: usize, qh: &QueueHandle<App>) {
        let pace = if self.mode == Mode::Messages { Duration::from_millis(250) } else { FRAME };
        let Some(s) = self.screens.get_mut(i) else { return };
        s.wants_frame = true;
        // a refresh's time of slack (up to 3 ms early): a 60 Hz cap on a 180 Hz screen is then every third refresh, exactly
        if s.last.map(|t| t.elapsed() + Duration::from_millis(3) >= pace).unwrap_or(true) {
            self.draw(i);
        } else {
            // a refresh with nothing new on this surface may not be drawn by the compositor at all, and then its frame callback
            // comes a refresh late (a 22 ms frame now and then): one pixel marked as changed keeps it on the screen's rhythm
            s.surface.damage_buffer(0, 0, 1, 1);
            s.surface.frame(qh, i);
            s.surface.commit();
        }
    }

    /// How far the fade-out has gone (1 = fully shown).
    fn shown(&self) -> f32 {
        match self.fading {
            Some(t) => 1.0 - (t.elapsed().as_secs_f32() / FADE.as_secs_f32()).min(1.0),
            None => 1.0,
        }
    }

    fn draw(&mut self, i: usize) {
        let t0 = Instant::now();
        self.draw_inner(i);
        if let Some(v) = self.draw_ms.as_mut() {
            v.push(t0.elapsed().as_secs_f32() * 1000.0);
        }
    }

    fn draw_inner(&mut self, i: usize) {
        let shown = self.shown();
        let (glyphs, accent) = (GLYPHS.chars().count(), self.accent);
        let qh = self.qh.clone();
        let s = &mut self.screens[i];
        if !s.configured || s.bufs.is_empty() {
            return;
        }
        let Some(slot) = s.bufs.iter().position(|b| !b.1) else { return }; // both still on screen: wait for one
        let (w, h) = s.size;
        let now = Instant::now();
        let dt = s.last.map(|t| (now - t).as_secs_f32()).unwrap_or(0.0).min(0.1);
        if dt > 0.0 && self.stats.is_some() {
            if let Some(v) = self.stats.as_mut() {
                v.push(dt * 1000.0);
            }
        }
        if s.last.is_none() {
            eprintln!("yggdrasil-splash: first frame on screen {i} after {} ms", self.started.elapsed().as_millis());
        }
        s.last = Some(now);
        let one = (w * h * 4) as usize;
        let Some(map) = s.map.as_mut() else { return };
        let bytes = &mut map[slot * one..(slot + 1) * one];
        match &self.mode {
            Mode::Messages => {
                // text changes rarely: drawn on a picture of its own, then copied over in the buffer's byte order
                let Some(pix) = s.pix.as_mut() else { return };
                pix.fill(tiny_skia::Color::from_rgba8(5, 3, 10, 255));
                if let Some(f) = &self.font {
                    let text = std::fs::read_to_string(yggdrasil_conf::dirs::state().join("session-output.log")).unwrap_or_default();
                    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
                    let fit = ((h as f32 - 80.0) / 22.0) as usize;
                    let (r, g, b, _) = accent;
                    for (k, l) in lines.iter().rev().take(fit).rev().enumerate() {
                        let clean: String = l.chars().filter(|c| !c.is_control()).collect();
                        draw::draw_text(pix, &f.regular, 15.0, 40.0, 50.0 + k as f32 * 22.0, (r, g, b, 255), &clean);
                    }
                }
                for (d, src) in bytes.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
                    d[0] = src[2];
                    d[1] = src[1];
                    d[2] = src[0];
                    d[3] = src[3];
                }
            }
            Mode::Picture(_) => {
                let Some(mut pix) = PixmapMut::from_bytes(bytes, w, h) else { return };
                pix.fill(tiny_skia::Color::from_rgba8(10, 3, 5, 255)); // (5, 3, 10) with red and blue swapped
                {
                    if let Some(p) = &self.picture {
                        let k = (w as f32 * 0.9 / p.width() as f32).min(h as f32 * 0.9 / p.height() as f32).min(1.0);
                        let (pw, ph) = (p.width() as f32 * k, p.height() as f32 * k);
                        let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
                        pix.draw_pixmap(0, 0, p.as_ref(), &paint, Transform::from_row(k, 0.0, 0.0, k, (w as f32 - pw) / 2.0, (h as f32 - ph) / 2.0), None);
                    }
                }
            }
            _ => {
                clear(bytes, [10, 3, 5, 255]); // (5, 3, 10) in the buffer's B, G, R, A order
                if let Some(atlas) = &self.atlas {
                    for (c, col) in s.cols.iter_mut().enumerate() {
                        // frames come at a steady pace now; one that came late (dt of two frames) moves two steps
                        let steps = (dt / FRAME.as_secs_f32()).round().max(1.0);
                        col.head += col.speed * steps;
                        let n = col.chars.len();
                        if col.head - n as f32 * CELL_H > h as f32 {
                            *col = new_column(&mut self.rng, h as f32, glyphs, false);
                            continue;
                        }
                        // now and then a character changes, as in the film
                        if self.rng.below(24) == 0 {
                            let k = self.rng.below(n);
                            col.chars[k] = self.rng.below(glyphs);
                        }
                        for (k, &g) in col.chars.iter().enumerate() {
                            let y = col.head - (n - 1 - k) as f32 * CELL_H;
                            if y < -CELL_H || y > h as f32 {
                                continue;
                            }
                            let level = (k * (LEVELS - 1)) / (n - 1).max(1);
                            blit(bytes, w, h, &atlas.cells[g][level], (c as f32 * CELL_W) as i32, y.round() as i32);
                        }
                    }
                }
                if let Some(l) = &self.logo {
                    // as the screensaver the logo fades away between 4 and 6 seconds: nothing stands still to mark the screen
                    let k = if self.screensaver { (256.0 * (1.0 - ((self.started.elapsed().as_secs_f32() - 4.0) / 2.0).clamp(0.0, 1.0))) as u32 } else { 256 };
                    blit_faded(bytes, w, h, l, ((w - l.width().min(w)) / 2) as i32, ((h - l.height().min(h)) / 2) as i32, k);
                }
            }
        }
        // the fade: everything towards transparent, so the desktop shows through. The compositor does it when it can (one
        // number per frame); else every pixel here, two channels at a time
        if let Some(a) = &s.alpha {
            a.set_multiplier((shown.clamp(0.0, 1.0) as f64 * u32::MAX as f64) as u32);
        } else if shown < 1.0 {
            let k = (shown * 256.0) as u32;
            // SAFETY: any 4 bytes are a valid u32; only the aligned middle is used as u32
            let (pre, mid, post) = unsafe { bytes.align_to_mut::<u32>() };
            for b in pre.iter_mut().chain(post.iter_mut()) {
                *b = ((*b as u32 * k) >> 8) as u8;
            }
            for v in mid.iter_mut() {
                let rb = ((*v & 0x00ff_00ff) * k >> 8) & 0x00ff_00ff;
                let ga = (((*v >> 8) & 0x00ff_00ff) * k >> 8) & 0x00ff_00ff;
                *v = rb | (ga << 8);
            }
        }
        s.bufs[slot].1 = true;
        s.wants_frame = false;
        s.surface.attach(Some(&s.bufs[slot].0), 0, 0);
        s.surface.damage_buffer(0, 0, w as i32, h as i32);
        s.surface.frame(&qh, i);
        s.surface.commit();
    }

    /// The desktop is ready once Yggdrasil's top bar is on a screen (Hyprland lists its layer).
    fn desktop_ready() -> bool {
        hypr("j/layers").map(|t| t.contains("\"yggdrasil-panel\"")).unwrap_or(false)
    }
}

/// One request to Hyprland's control socket.
fn hypr(request: &str) -> Option<String> {
    let dir = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr").join(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?);
    let mut s = std::os::unix::net::UnixStream::connect(dir.join(".socket.sock")).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

fn main() {
    let launched = Instant::now();
    let screensaver = std::env::args().any(|a| a == "--screensaver");
    // the screensaver is always the falling characters (the choice in splash.conf is for after signing in)
    let mode = if screensaver { Mode::Animation } else { mode() };
    if mode == Mode::Off {
        return;
    }
    eprintln!("yggdrasil-splash: {mode:?}");
    let Ok(conn) = Connection::connect_to_env() else {
        eprintln!("yggdrasil-splash: no Wayland display");
        return;
    };
    let Ok((globals, queue)) = registry_queue_init::<App>(&conn) else {
        eprintln!("yggdrasil-splash: the compositor did not answer");
        return;
    };
    let qh = queue.handle();
    let (Ok(compositor), Ok(shm), Ok(layer_shell)) = (globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()), globals.bind::<WlShm, _, _>(&qh, 1..=1, ()), globals.bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())) else {
        eprintln!("yggdrasil-splash: the compositor has no layer shell");
        return;
    };
    let alpha_mgr: Option<WpAlphaModifierV1> = globals.bind(&qh, 1..=1, ()).ok();
    let accent = design::accent();
    let mut app = App {
        qh: qh.clone(),
        compositor,
        shm,
        layer_shell,
        screens: vec![],
        atlas: if matches!(mode, Mode::Animation) {
            let t = Instant::now();
            let a = Atlas::new((accent.0, accent.1, accent.2));
            eprintln!("yggdrasil-splash: characters ready in {} ms", t.elapsed().as_millis());
            a
        } else {
            None
        },
        logo: logo().map(bgra),
        picture: if let Mode::Picture(p) = &mode { design::load_picture(p).map(bgra) } else { None },
        font: if mode == Mode::Messages { draw::Fonts::load() } else { None },
        mode,
        rng: Rng(Instant::now().elapsed().as_nanos() as u64 ^ (std::process::id() as u64) << 20 | 1),
        started: launched,
        fading: None,
        screensaver,
        ready: Default::default(),
        accent,
        stats: std::env::var_os("YGGDRASIL_SPLASH_STATS").map(|_| vec![]),
        draw_ms: std::env::var_os("YGGDRASIL_SPLASH_STATS").map(|_| vec![]),
        done: false,
    };
    // one covering surface per screen, above everything, taking no clicks
    let outputs: Vec<u32> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| g.name).collect());
    for name in outputs {
        let output = globals.registry().bind::<WlOutput, _, _>(name, 1, &qh, ());
        let surface = app.compositor.create_surface(&qh, ());
        let idx = app.screens.len();
        let layer = app.layer_shell.get_layer_surface(&surface, Some(&output), zwlr_layer_shell_v1::Layer::Overlay, "yggdrasil-splash".into(), &qh, idx);
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_exclusive_zone(-1);
        let alpha = alpha_mgr.as_ref().map(|m| m.get_surface(&surface, &qh, ()));
        if screensaver {
            // it takes the keyboard and every click: what wakes it must not reach the program underneath
            layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        } else {
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            let region = app.compositor.create_region(&qh, ());
            surface.set_input_region(Some(&region));
            region.destroy();
        }
        surface.commit();
        app.screens.push(Screen { output, surface, layer, size: (0, 0), configured: false, bufs: vec![], map: None, pix: None, cols: vec![], alpha, wants_frame: true, last: None });
    }
    let Ok(mut event_loop) = EventLoop::<App>::try_new() else { return };
    if WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).is_err() {
        return;
    }
    // whether the top bar is up, asked in the background four times a second (not for the screensaver: it runs until ended)
    if !screensaver {
        let ready = app.ready.clone();
        std::thread::spawn(move || {
            std::thread::sleep(SHORTEST);
            while !App::desktop_ready() {
                std::thread::sleep(Duration::from_millis(250));
            }
            ready.store(true, std::sync::atomic::Ordering::Release);
            eprintln!("yggdrasil-splash: the top bar is up");
        });
    }
    // the fade starts once it is (or when it takes too long); frames themselves come from the compositor's frame callbacks
    let _ = event_loop.handle().insert_source(Timer::from_duration(Duration::from_millis(20)), |_, _, app: &mut App| {
        if !app.screensaver && app.fading.is_none() && (app.started.elapsed() > LONGEST || app.ready.load(std::sync::atomic::Ordering::Acquire)) {
            app.fading = Some(Instant::now());
        }
        if app.fading.map(|t| t.elapsed() > FADE).unwrap_or(false) {
            if !app.done {
                let frames = app.screens.iter().filter(|s| s.last.is_some()).count();
                eprintln!("yggdrasil-splash: shown on {frames} of {} screens for {} ms", app.screens.len(), app.started.elapsed().as_millis());
            }
            app.done = true;
        }
        TimeoutAction::ToDuration(Duration::from_millis(20))
    });
    while !app.done {
        if event_loop.dispatch(Some(Duration::from_millis(20)), &mut app).is_err() {
            break;
        }
        let _ = conn.flush();
    }
    if let Some(mut v) = app.draw_ms.take() {
        v.sort_by(|a, b| a.total_cmp(b));
        if !v.is_empty() {
            let pct = |p: f32| v[((v.len() - 1) as f32 * p) as usize];
            eprintln!("drawing a frame takes: median {:.1} ms  95% {:.1}  max {:.1}", pct(0.5), pct(0.95), v[v.len() - 1]);
        }
    }
    if let Some(mut v) = app.stats.take() {
        v.sort_by(|a, b| a.total_cmp(b));
        if !v.is_empty() {
            let pct = |p: f32| v[((v.len() - 1) as f32 * p) as usize];
            eprintln!("frames {}  ms between: median {:.1}  95% {:.1}  99% {:.1}  max {:.1}", v.len(), pct(0.5), pct(0.95), pct(0.99), v[v.len() - 1]);
        }
    }
    for s in &app.screens {
        s.layer.destroy();
        s.surface.destroy();
    }
    let _ = conn.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_choice_is_read_from_the_file() {
        assert_eq!(mode_from(""), Mode::Animation, "nothing chosen: the animation");
        assert_eq!(mode_from("mode = off\n"), Mode::Off);
        assert_eq!(mode_from("mode = messages # what starts\n"), Mode::Messages);
        assert_eq!(mode_from("mode = picture\npicture = /no/such/file.png\n"), Mode::Animation, "a missing picture: the animation");
        let f = std::env::temp_dir().join(format!("ygg-splash-{}.png", std::process::id()));
        std::fs::write(&f, b"x").unwrap();
        assert_eq!(mode_from(&format!("mode = picture\npicture = {}\n", f.display())), Mode::Picture(f.clone()));
        let _ = std::fs::remove_file(&f);
    }

    #[test]
    fn blit_copies_blends_and_clips() {
        let mut src = Pixmap::new(2, 2).unwrap();
        let px = |r, g, b, a| tiny_skia::PremultipliedColorU8::from_rgba(r, g, b, a).unwrap();
        src.pixels_mut().copy_from_slice(&[px(200, 100, 50, 255), px(0, 0, 0, 0), px(100, 50, 0, 128), px(10, 20, 30, 255)]);
        let mut dst = vec![0u8; 3 * 3 * 4];
        clear(&mut dst, [40, 40, 40, 255]);
        blit(&mut dst, 3, 3, &src, 1, 1);
        let at = |d: &Vec<u8>, x: usize, y: usize| d[(y * 3 + x) * 4..(y * 3 + x) * 4 + 4].to_vec();
        assert_eq!(at(&dst, 1, 1), vec![200, 100, 50, 255], "opaque: copied");
        assert_eq!(at(&dst, 2, 1), vec![40, 40, 40, 255], "transparent: untouched");
        assert_eq!(at(&dst, 1, 2), vec![120, 70, 20, 255], "half: blended over");
        assert_eq!(at(&dst, 0, 0), vec![40, 40, 40, 255], "outside: untouched");
        // partly off every edge: no panic, only the part on the buffer drawn
        blit(&mut dst, 3, 3, &src, -1, -1);
        blit(&mut dst, 3, 3, &src, 2, 2);
        assert_eq!(at(&dst, 0, 0), vec![10, 20, 30, 255]);
        assert_eq!(at(&dst, 2, 2), vec![200, 100, 50, 255]);
    }

    #[test]
    fn brightness_rises_to_a_light_head() {
        let a = (124, 84, 214);
        assert!(level_color(a, 0).3 < level_color(a, LEVELS / 2).3, "fainter at the top");
        assert_eq!(level_color(a, LEVELS - 1), (240, 234, 255, 255), "the head");
        let mid = level_color(a, LEVELS / 2);
        assert!(mid.2 >= a.2 && mid.0 >= a.0, "the accent, a little lighter");
    }
}
