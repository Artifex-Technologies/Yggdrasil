//! yggdrasil-colorpicker: pick a colour anywhere on the screen and copy it (Super+Shift+C; Settings > Keyboard Shortcuts names it).
//!
//! The screens are captured the moment it starts and shown frozen, full-screen, so nothing can change under the pointer; a loupe next
//! to the pointer shows the pixels around it, the one in the middle framed, with its colour.
//!   move, arrows   choose the pixel (an arrow moves one pixel)
//!   wheel          zoom the loupe in or out
//!   click, Enter   pick it: copied in the format chosen in Settings, kept in the history, shown in a notification
//!   Esc, right-click  nothing
//! Runs only while picking; nothing stays in the background.
//!
//!   --print           also print what was copied
//!   --format NAME     this once: hex, rgb, hsl, hsv, cmyk or float
//!   --timeout SECS    give up after SECS (for tests: the picker takes the keyboard)

mod draw;

use std::os::fd::AsFd;
use std::process::{Command, Stdio};

use colorpicker::{Format, Loupe, Rgb};
use memmap2::MmapMut;
use tiny_skia::{Paint, Pixmap, Rect, Transform};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_callback::{self, WlCallback},
    wl_compositor::WlCompositor,
    wl_keyboard::{self, WlKeyboard},
    wl_output::{self, WlOutput},
    wl_pointer::{self, WlPointer},
    wl_registry,
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::{
    wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1},
    wp_cursor_shape_manager_v1::WpCursorShapeManagerV1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::{self, ZwlrLayerShellV1},
    zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1},
};

const KEY_ESC: u32 = 1;
const KEY_ENTER: u32 = 28;
const KEY_KP_ENTER: u32 = 96;
const KEY_UP: u32 = 103;
const KEY_LEFT: u32 = 105;
const KEY_RIGHT: u32 = 106;
const KEY_DOWN: u32 = 108;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
/// The loupe's size and its distance from the pointer, in the compositor's (logical) units.
const LOUPE: f32 = 132.0;
const GAP: f32 = 24.0;
const LABEL: f32 = 30.0;
const PAD: f32 = 6.0;
/// Wheel movement (in its own units) that makes one zoom step.
const WHEEL_STEP: f64 = 10.0;

/// A captured screen, BGRA as the compositor shows it.
struct Img {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Img {
    fn at(&self, x: u32, y: u32) -> Rgb {
        let o = ((y * self.w + x) * 4) as usize;
        (self.px[o + 2], self.px[o + 1], self.px[o])
    }
}

/// One of the two buffers a screen draws into: whether it holds the picture yet, and where the loupe was last drawn on it.
struct Buf {
    buffer: WlBuffer,
    busy: bool,
    filled: bool,
    loupe: Option<(i32, i32, i32, i32)>,
}

struct Screen {
    name: String,
    output: WlOutput,
    img: Option<Img>,
    surface: Option<WlSurface>,
    layer: Option<ZwlrLayerSurfaceV1>,
    viewport: Option<WpViewport>,
    size: (u32, u32),
    bufs: Vec<Buf>,
    map: Option<MmapMut>,
    frame_pending: bool,
    dirty: bool,
}

struct App {
    compositor: WlCompositor,
    shm: WlShm,
    screens: Vec<Screen>,
    cursor: Option<WpCursorShapeManagerV1>,
    cursor_dev: Option<WpCursorShapeDeviceV1>,
    pointer: Option<WlPointer>,
    keyboard: Option<WlKeyboard>,
    /// The screen the pointer is on and where (in its picture's pixels), and the arrows' nudge on top.
    at: Option<(usize, (f64, f64))>,
    nudge: (i32, i32),
    zoom: usize,
    wheel: f64,
    fonts: Option<draw::Fonts>,
    result: Option<Option<Rgb>>,
}

impl App {
    /// The pixel being pointed at: its screen and coordinates.
    fn pixel(&self) -> Option<(usize, u32, u32)> {
        let (i, p) = self.at?;
        let img = self.screens.get(i)?.img.as_ref()?;
        let x = (p.0.floor() as i32 + self.nudge.0).clamp(0, img.w as i32 - 1);
        let y = (p.1.floor() as i32 + self.nudge.1).clamp(0, img.h as i32 - 1);
        Some((i, x as u32, y as u32))
    }

    fn colour(&self) -> Option<Rgb> {
        let (i, x, y) = self.pixel()?;
        Some(self.screens[i].img.as_ref()?.at(x, y))
    }

    fn to_px(&self, i: usize, x: f64, y: f64) -> (f64, f64) {
        let s = &self.screens[i];
        match (&s.img, s.size) {
            (Some(m), (w, h)) if w > 0 && h > 0 => (x * m.w as f64 / w as f64, y * m.h as f64 / h as f64),
            _ => (x, y),
        }
    }

    fn redraw_all(&mut self, qh: &QueueHandle<App>) {
        for i in 0..self.screens.len() {
            self.screens[i].dirty = true;
            self.draw(i, qh);
        }
    }

    /// The loupe for the pixel at (x, y) of screen `i`, drawn on its own (RGBA), and where it goes on the screen.
    fn loupe_image(&self, i: usize, x: u32, y: u32) -> Option<(Pixmap, i32, i32)> {
        let s = &self.screens[i];
        let img = s.img.as_ref()?;
        let f = if s.size.0 > 0 { img.w as f32 / s.size.0 as f32 } else { 1.0 };
        let l = Loupe::new(self.zoom, (LOUPE * f) as u32);
        let side = l.side() as f32;
        let (pad, label) = (PAD * f, LABEL * f);
        let (bw, bh) = (side + 2.0 * pad, side + 2.0 * pad + label);
        let mut pix = Pixmap::new(bw.ceil() as u32, bh.ceil() as u32)?;
        draw::fill_rounded(&mut pix, 0.0, 0.0, bw, bh, 12.0 * f, (24, 24, 30, 240));
        let mut paint = Paint::default();
        for (n, src) in l.source(x as i32, y as i32, img.w, img.h).into_iter().enumerate() {
            let (cx, cy) = ((n as u32 % l.cells) as f32, (n as u32 / l.cells) as f32);
            let (r, g, b) = src.map(|(sx, sy)| img.at(sx, sy)).unwrap_or((40, 40, 46));
            paint.set_color_rgba8(r, g, b, 255);
            if let Some(rect) = Rect::from_xywh(pad + cx * l.cell as f32, pad + cy * l.cell as f32, l.cell as f32, l.cell as f32) {
                pix.fill_rect(rect, &paint, Transform::identity(), None);
            }
        }
        // the middle pixel: framed, white inside black, so it shows on any colour
        let mid = (l.cells / 2) as f32 * l.cell as f32 + pad;
        let c = l.cell as f32;
        let frame = |pix: &mut Pixmap, inset: f32, t: f32, col: (u8, u8, u8)| {
            let mut p = Paint::default();
            p.set_color_rgba8(col.0, col.1, col.2, 255);
            let (x0, y0, w) = (mid - inset, mid - inset, c + 2.0 * inset);
            for r in [Rect::from_xywh(x0, y0, w, t), Rect::from_xywh(x0, y0 + w - t, w, t), Rect::from_xywh(x0, y0, t, w), Rect::from_xywh(x0 + w - t, y0, t, w)].into_iter().flatten() {
                pix.fill_rect(r, &p, Transform::identity(), None);
            }
        };
        frame(&mut pix, 2.0 * f, 1.0 * f.max(1.0), (0, 0, 0));
        frame(&mut pix, 1.0 * f, 1.0 * f.max(1.0), (255, 255, 255));
        // the colour, as a swatch and its hex
        let rgb = img.at(x, y);
        let ly = side + 2.0 * pad;
        let sw = label - 2.0 * pad;
        draw::fill_rounded(&mut pix, pad, ly, sw, sw, 4.0 * f, (rgb.0, rgb.1, rgb.2, 255));
        if let Some(fonts) = &self.fonts {
            draw::draw_text(&mut pix, &fonts.bold, 13.0 * f, pad + sw + 8.0 * f, ly + sw / 2.0, (240, 240, 245, 255), &colorpicker::hex(rgb));
        }
        let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
        let (lx, ly) = colorpicker::place(px as i32, py as i32, bw.ceil() as i32, bh.ceil() as i32, img.w as i32, img.h as i32, (GAP * f) as i32);
        Some((pix, lx, ly))
    }

    fn draw(&mut self, i: usize, qh: &QueueHandle<App>) {
        let loupe = match self.pixel() {
            Some((pi, x, y)) if pi == i => self.loupe_image(i, x, y),
            _ => None,
        };
        let shm = self.shm.clone();
        let s = &mut self.screens[i];
        let (Some(img), Some(surface)) = (&s.img, &s.surface) else { return };
        if !s.dirty || s.frame_pending || s.size.0 == 0 {
            return;
        }
        let (w, h) = (img.w, img.h);
        let stride = (w * 4) as usize;
        let one = stride * h as usize;
        if s.bufs.is_empty() {
            let Ok(fd) = rustix::fs::memfd_create(c"yggdrasil-colorpicker", rustix::fs::MemfdFlags::CLOEXEC) else { return };
            if rustix::fs::ftruncate(&fd, (one * 2) as u64).is_err() {
                return;
            }
            let file = std::fs::File::from(fd);
            let Ok(map) = (unsafe { MmapMut::map_mut(&file) }) else { return };
            let pool = shm.create_pool(file.as_fd(), (one * 2) as i32, qh, ());
            s.bufs = (0..2)
                .map(|k| Buf { buffer: pool.create_buffer((k * one) as i32, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, qh, (i, k)), busy: false, filled: false, loupe: None })
                .collect();
            pool.destroy();
            s.map = Some(map);
        }
        let Some(k) = s.bufs.iter().position(|b| !b.busy) else { return };
        let Some(map) = s.map.as_mut() else { return };
        let dst = &mut map[k * one..(k + 1) * one];
        let mut damage: Vec<(i32, i32, i32, i32)> = vec![];
        // the frozen picture once per buffer; after that only where a loupe was is put back
        if !s.bufs[k].filled {
            dst.copy_from_slice(&img.px);
            s.bufs[k].filled = true;
            damage.push((0, 0, w as i32, h as i32));
        } else if let Some((x0, y0, bw, bh)) = s.bufs[k].loupe.take() {
            for y in y0.max(0)..(y0 + bh).min(h as i32) {
                let a = y as usize * stride + x0.max(0) as usize * 4;
                let b = y as usize * stride + (x0 + bw).min(w as i32) as usize * 4;
                dst[a..b].copy_from_slice(&img.px[a..b]);
            }
            damage.push((x0, y0, bw, bh));
        }
        if let Some((pix, lx, ly)) = &loupe {
            let (bw, bh) = (pix.width() as i32, pix.height() as i32);
            let src = pix.data();
            for y in 0..bh {
                let dy = ly + y;
                if dy < 0 || dy >= h as i32 {
                    continue;
                }
                for x in 0..bw {
                    let dx = lx + x;
                    if dx < 0 || dx >= w as i32 {
                        continue;
                    }
                    // RGBA premultiplied over the picture (BGRA)
                    let so = ((y * bw + x) * 4) as usize;
                    let a = src[so + 3] as u32;
                    if a == 0 {
                        continue;
                    }
                    let o = dy as usize * stride + dx as usize * 4;
                    for (c, sc) in [(0, 2), (1, 1), (2, 0)] {
                        dst[o + c] = (src[so + sc] as u32 + dst[o + c] as u32 * (255 - a) / 255).min(255) as u8;
                    }
                    dst[o + 3] = 255;
                }
            }
            s.bufs[k].loupe = Some((*lx, *ly, bw, bh));
            damage.push((*lx, *ly, bw, bh));
        }
        s.bufs[k].busy = true;
        surface.attach(Some(&s.bufs[k].buffer), 0, 0);
        // this buffer last held an older frame: what the other buffer changed since must be redrawn here too
        let other = 1 - k;
        if let Some(r) = s.bufs.get(other).and_then(|b| b.loupe) {
            damage.push(r);
        }
        for (x, y, dw, dh) in damage {
            surface.damage_buffer(x, y, dw, dh);
        }
        surface.frame(qh, i);
        surface.commit();
        s.frame_pending = true;
        s.dirty = false;
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(App: WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ZwlrLayerShellV1);
delegate_noop!(App: WpViewporter);
delegate_noop!(App: WpViewport);
delegate_noop!(App: WpCursorShapeManagerV1);
delegate_noop!(App: WpCursorShapeDeviceV1);

impl Dispatch<WlOutput, usize> for App {
    fn event(app: &mut Self, _: &WlOutput, e: wl_output::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_output::Event::Name { name } = e {
            if let Some(s) = app.screens.get_mut(*i) {
                s.name = name;
            }
        }
    }
}

impl Dispatch<WlSeat, ()> for App {
    fn event(app: &mut Self, seat: &WlSeat, e: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = e {
            if c.contains(wl_seat::Capability::Pointer) && app.pointer.is_none() {
                let p = seat.get_pointer(qh, ());
                app.cursor_dev = app.cursor.as_ref().map(|m| m.get_pointer(&p, qh, ()));
                app.pointer = Some(p);
            }
            if c.contains(wl_seat::Capability::Keyboard) && app.keyboard.is_none() {
                app.keyboard = Some(seat.get_keyboard(qh, ()));
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for App {
    fn event(app: &mut Self, _: &WlPointer, e: wl_pointer::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        let before = app.pixel();
        let zoom = app.zoom;
        match e {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                if let Some(d) = &app.cursor_dev {
                    d.set_shape(serial, Shape::Crosshair);
                }
                if let Some(i) = app.screens.iter().position(|s| s.surface.as_ref() == Some(&surface)) {
                    app.at = Some((i, app.to_px(i, surface_x, surface_y)));
                    app.nudge = (0, 0);
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some((i, _)) = app.at {
                    app.at = Some((i, app.to_px(i, surface_x, surface_y)));
                    app.nudge = (0, 0);
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(wl_pointer::ButtonState::Pressed), .. } => {
                if button == BTN_RIGHT {
                    app.result = Some(None);
                } else if button == BTN_LEFT {
                    app.result = Some(app.colour());
                }
            }
            wl_pointer::Event::Axis { axis: WEnum::Value(wl_pointer::Axis::VerticalScroll), value, .. } => {
                // down shows more pixels (zooms out), up fewer
                app.wheel += value;
                while app.wheel >= WHEEL_STEP {
                    app.wheel -= WHEEL_STEP;
                    app.zoom = (app.zoom + 1).min(colorpicker::ZOOMS.len() - 1);
                }
                while app.wheel <= -WHEEL_STEP {
                    app.wheel += WHEEL_STEP;
                    app.zoom = app.zoom.saturating_sub(1);
                }
            }
            _ => {}
        }
        let after = app.pixel();
        if after != before || app.zoom != zoom {
            app.redraw_all(qh);
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(app: &mut Self, _: &WlKeyboard, e: wl_keyboard::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } = e {
            let step = |app: &mut App, dx: i32, dy: i32| {
                app.nudge = (app.nudge.0 + dx, app.nudge.1 + dy);
                app.redraw_all(qh);
            };
            match key {
                KEY_ESC => app.result = Some(None),
                KEY_ENTER | KEY_KP_ENTER => app.result = Some(app.colour()),
                KEY_LEFT => step(app, -1, 0),
                KEY_RIGHT => step(app, 1, 0),
                KEY_UP => step(app, 0, -1),
                KEY_DOWN => step(app, 0, 1),
                _ => {}
            }
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for App {
    fn event(app: &mut Self, l: &ZwlrLayerSurfaceV1, e: zwlr_layer_surface_v1::Event, i: &usize, _: &Connection, qh: &QueueHandle<Self>) {
        match e {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                l.ack_configure(serial);
                if let Some(s) = app.screens.get_mut(*i) {
                    s.size = (width, height);
                    if let Some(v) = &s.viewport {
                        v.set_destination(width as i32, height as i32);
                    }
                    s.dirty = true;
                }
                app.draw(*i, qh);
            }
            zwlr_layer_surface_v1::Event::Closed => app.result = Some(None),
            _ => {}
        }
    }
}

impl Dispatch<WlCallback, usize> for App {
    fn event(app: &mut Self, _: &WlCallback, e: wl_callback::Event, i: &usize, _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { .. } = e {
            if let Some(s) = app.screens.get_mut(*i) {
                s.frame_pending = false;
            }
            app.draw(*i, qh);
        }
    }
}

impl Dispatch<WlBuffer, (usize, usize)> for App {
    fn event(app: &mut Self, _: &WlBuffer, e: wl_buffer::Event, idx: &(usize, usize), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = e {
            if let Some(b) = app.screens.get_mut(idx.0).and_then(|s| s.bufs.get_mut(idx.1)) {
                b.busy = false;
            }
        }
    }
}

/// A screen as the compositor shows it, through grim (PPM, no compression: fast).
fn capture(name: &str) -> Option<Img> {
    let out = Command::new("grim").args(["-t", "ppm", "-o", name, "-"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    if !out.status.success() {
        return None;
    }
    from_ppm(&out.stdout)
}

/// Binary PPM (P6, 8 bits) to BGRA.
fn from_ppm(data: &[u8]) -> Option<Img> {
    let mut fields = vec![];
    let mut i = 0;
    while fields.len() < 4 && i < data.len() {
        while i < data.len() && data[i].is_ascii_whitespace() {
            i += 1;
        }
        if data.get(i) == Some(&b'#') {
            while i < data.len() && data[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < data.len() && !data[i].is_ascii_whitespace() {
            i += 1;
        }
        fields.push(std::str::from_utf8(&data[start..i]).ok()?.to_string());
    }
    if fields.first().map(String::as_str) != Some("P6") || fields.get(3).map(String::as_str) != Some("255") {
        return None;
    }
    let (w, h): (u32, u32) = (fields[1].parse().ok()?, fields[2].parse().ok()?);
    let rgb = data.get(i + 1..i + 1 + (w * h * 3) as usize)?;
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for p in rgb.chunks_exact(3) {
        px.extend_from_slice(&[p[2], p[1], p[0], 255]);
    }
    Some(Img { w, h, px })
}

/// A small square of the colour, for the notification.
fn swatch(c: Rgb) -> Option<std::path::PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from)?;
    let hex = colorpicker::hex(c);
    let p = dir.join(format!("yggdrasil-color-{}.svg", &hex[1..]));
    let svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect x="4" y="4" width="56" height="56" rx="12" fill="{hex}" stroke="#00000040" stroke-width="2"/></svg>"##);
    std::fs::write(&p, svg).ok().map(|_| p)
}

fn arg_after(args: &[String], flag: &str) -> Option<String> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let format = arg_after(&args, "--format").and_then(|f| Format::parse(&f)).unwrap_or_else(colorpicker::chosen_format);
    if let Some(secs) = arg_after(&args, "--timeout").and_then(|s| s.parse::<u64>().ok()) {
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            eprintln!("yggdrasil-colorpicker: timed out");
            std::process::exit(2);
        });
    }
    let Ok(conn) = Connection::connect_to_env() else {
        eprintln!("yggdrasil-colorpicker: no Wayland display");
        std::process::exit(1);
    };
    let Ok((globals, mut queue)) = registry_queue_init::<App>(&conn) else { std::process::exit(1) };
    let qh = queue.handle();
    let (Ok(compositor), Ok(shm), Ok(layer_shell)) = (globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()), globals.bind::<WlShm, _, _>(&qh, 1..=1, ()), globals.bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())) else {
        eprintln!("yggdrasil-colorpicker: the compositor has no layer shell");
        std::process::exit(1);
    };
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let mut app = App { compositor, shm, screens: vec![], cursor: globals.bind(&qh, 1..=1, ()).ok(), cursor_dev: None, pointer: None, keyboard: None, at: None, nudge: (0, 0), zoom: 3, wheel: 0.0, fonts: draw::Fonts::load(), result: None };
    let outputs: Vec<(u32, u32)> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect());
    for (n, (name, version)) in outputs.into_iter().enumerate() {
        let output = globals.registry().bind::<WlOutput, _, _>(name, version.min(4), &qh, n);
        app.screens.push(Screen { name: String::new(), output, img: None, surface: None, layer: None, viewport: None, size: (0, 0), bufs: vec![], map: None, frame_pending: false, dirty: true });
    }
    let _ = queue.roundtrip(&mut app);
    // every screen frozen at once, before anything of ours is on it
    let names: Vec<String> = app.screens.iter().map(|s| s.name.clone()).collect();
    let shots: Vec<Option<Img>> = std::thread::scope(|sc| names.iter().map(|n| sc.spawn(move || capture(n))).collect::<Vec<_>>().into_iter().map(|h| h.join().ok().flatten()).collect());
    for (s, img) in app.screens.iter_mut().zip(shots) {
        s.img = img;
    }
    if app.screens.iter().all(|s| s.img.is_none()) {
        eprintln!("yggdrasil-colorpicker: could not capture the screen (is grim installed?)");
        let _ = Command::new("notify-send").args(["-a", "Color Picker", "Color Picker", "The screen could not be captured"]).status();
        std::process::exit(1);
    }
    for i in 0..app.screens.len() {
        if app.screens[i].img.is_none() {
            continue;
        }
        let surface = app.compositor.create_surface(&qh, ());
        let layer = layer_shell.get_layer_surface(&surface, Some(&app.screens[i].output), zwlr_layer_shell_v1::Layer::Overlay, "yggdrasil-colorpicker".into(), &qh, i);
        layer.set_anchor(Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::Exclusive);
        let viewport = viewporter.as_ref().map(|v| v.get_viewport(&surface, &qh, ()));
        surface.commit();
        let s = &mut app.screens[i];
        s.surface = Some(surface);
        s.layer = Some(layer);
        s.viewport = viewport;
    }
    let _seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    while app.result.is_none() {
        if queue.blocking_dispatch(&mut app).is_err() {
            break;
        }
    }
    for s in &app.screens {
        if let Some(l) = &s.layer {
            l.destroy();
        }
        if let Some(sf) = &s.surface {
            sf.destroy();
        }
    }
    let _ = conn.flush();
    let Some(Some(c)) = app.result else { return };
    let text = format.text(c);
    let _ = Command::new("wl-copy").arg("--").arg(&text).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Err(e) = colorpicker::remember(c) {
        eprintln!("yggdrasil-colorpicker: history: {e}");
    }
    let mut n = Command::new("notify-send");
    n.args(["-a", "Color Picker", "-t", "4000"]);
    if let Some(p) = swatch(c) {
        n.arg("-i").arg(p);
    }
    let _ = n.args([text.as_str(), "Copied (Super+Shift+C picks another)"]).stdin(Stdio::null()).spawn();
    if args.iter().any(|a| a == "--print") {
        println!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppm_becomes_bgra_and_reads_back_as_rgb() {
        let mut ppm = b"P6\n# grim\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 128, 0, 1, 2, 3]);
        let m = from_ppm(&ppm).unwrap();
        assert_eq!((m.w, m.h), (2, 1));
        assert_eq!(m.px, vec![0, 128, 255, 255, 3, 2, 1, 255]);
        assert_eq!(m.at(0, 0), (255, 128, 0));
        assert_eq!(m.at(1, 0), (1, 2, 3));
        assert!(from_ppm(b"P5\n1 1\n255\n\0").is_none());
    }

    #[test]
    fn flags_are_read() {
        let a: Vec<String> = ["--format", "hsl", "--timeout", "3"].iter().map(|s| s.to_string()).collect();
        assert_eq!(arg_after(&a, "--format").as_deref(), Some("hsl"));
        assert_eq!(arg_after(&a, "--timeout").as_deref(), Some("3"));
        assert_eq!(arg_after(&a, "--print"), None);
    }
}
