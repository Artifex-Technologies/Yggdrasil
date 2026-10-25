//! yggdrasil-shot: a screenshot of anything on the screen, open menus included (as Snagit does it).
//!
//! The screens are captured first, the moment it starts (the Print key goes to the compositor, so a program's open menu is still
//! there), and the choosing happens on that frozen picture: nothing on the screen can close or change while you choose.
//!   drag            an area
//!   click           the window under the pointer (it is lit up as the pointer moves), or the whole screen where there is none
//!   Enter           the whole screen the pointer is on
//!   Esc, right-click  nothing
//! The picture goes to Pictures/Screenshots and to the clipboard (wl-copy), with a notification.
#![allow(dead_code)]

mod design;
mod draw;

use std::os::fd::AsFd;
use std::process::{Command, Stdio};

use memmap2::MmapMut;
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

/// How much of the screen's brightness is left outside the choice.
const DIM: u32 = 110; // of 256
/// A press that moves less than this (in the picture's pixels) is a click.
const DRAG: f64 = 5.0;
const KEY_ESC: u32 = 1;
const KEY_ENTER: u32 = 28;
const KEY_KP_ENTER: u32 = 96;
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;

/// A captured screen: BGRA, as the compositor shows it, and a dimmed copy to draw from.
struct Img {
    w: u32,
    h: u32,
    bright: Vec<u8>,
    dim: Vec<u8>,
}

/// A rectangle in a screen's picture (its pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
struct R {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
}

impl R {
    fn from_corners(a: (f64, f64), b: (f64, f64)) -> R {
        let (x0, x1) = (a.0.min(b.0).floor() as i32, a.0.max(b.0).ceil() as i32);
        let (y0, y1) = (a.1.min(b.1).floor() as i32, a.1.max(b.1).ceil() as i32);
        R { x: x0, y: y0, w: x1 - x0, h: y1 - y0 }
    }
    fn clamp(self, w: u32, h: u32) -> Option<R> {
        let x0 = self.x.clamp(0, w as i32);
        let y0 = self.y.clamp(0, h as i32);
        let x1 = (self.x + self.w).clamp(0, w as i32);
        let y1 = (self.y + self.h).clamp(0, h as i32);
        (x1 > x0 && y1 > y0).then_some(R { x: x0, y: y0, w: x1 - x0, h: y1 - y0 })
    }
    fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x as f64 && p.1 >= self.y as f64 && p.0 < (self.x + self.w) as f64 && p.1 < (self.y + self.h) as f64
    }
}

struct Screen {
    name: String,
    output: WlOutput,
    img: Option<Img>,
    surface: Option<WlSurface>,
    layer: Option<ZwlrLayerSurfaceV1>,
    viewport: Option<WpViewport>,
    /// The surface's size in the compositor's (logical) units.
    size: (u32, u32),
    bufs: Vec<(WlBuffer, bool)>,
    map: Option<MmapMut>,
    /// Windows on it, in its picture's pixels.
    windows: Vec<R>,
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
    /// The screen the pointer is on, and where (in its picture's pixels).
    at: Option<(usize, (f64, f64))>,
    press: Option<(usize, (f64, f64))>,
    accent: (u8, u8, u8),
    result: Option<Option<(usize, R)>>,
}

/// What would be taken now: the area being dragged, else the window under the pointer.
fn choice(app: &App) -> Option<(usize, R)> {
    let (i, p) = app.at?;
    if let Some((pi, start)) = app.press {
        if pi == i && ((p.0 - start.0).abs() > DRAG || (p.1 - start.1).abs() > DRAG) {
            return Some((i, R::from_corners(start, p)));
        }
    }
    window_at(&app.screens[i].windows, p).map(|r| (i, r))
}

/// The smallest window under the point (a dialog over its main window, not the main window).
fn window_at(windows: &[R], p: (f64, f64)) -> Option<R> {
    windows.iter().filter(|r| r.contains(p)).min_by_key(|r| r.w as i64 * r.h as i64).copied()
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
        let before = choice(app);
        match e {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                if let Some(d) = &app.cursor_dev {
                    d.set_shape(serial, Shape::Crosshair);
                }
                if let Some(i) = app.screens.iter().position(|s| s.surface.as_ref() == Some(&surface)) {
                    app.at = Some((i, app.to_px(i, surface_x, surface_y)));
                }
            }
            wl_pointer::Event::Leave { .. } => {}
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some((i, _)) = app.at {
                    app.at = Some((i, app.to_px(i, surface_x, surface_y)));
                }
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(st), .. } => {
                if button == BTN_RIGHT && st == wl_pointer::ButtonState::Pressed {
                    app.result = Some(None);
                } else if button == BTN_LEFT {
                    match st {
                        wl_pointer::ButtonState::Pressed => app.press = app.at,
                        wl_pointer::ButtonState::Released => {
                            if let Some((i, _)) = app.press {
                                let whole = app.screens[i].img.as_ref().map(|m| R { x: 0, y: 0, w: m.w as i32, h: m.h as i32 });
                                app.result = Some(choice(app).or(whole.map(|r| (i, r))));
                            }
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        let after = choice(app);
        if after != before {
            for i in 0..app.screens.len() {
                if before.map(|c| c.0) == Some(i) || after.map(|c| c.0) == Some(i) {
                    app.screens[i].dirty = true;
                    app.draw(i, qh);
                }
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(app: &mut Self, _: &WlKeyboard, e: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } = e {
            match key {
                KEY_ESC => app.result = Some(None),
                KEY_ENTER | KEY_KP_ENTER => {
                    let i = app.at.map(|a| a.0).unwrap_or(0);
                    app.result = Some(app.screens.get(i).and_then(|s| s.img.as_ref()).map(|m| (i, R { x: 0, y: 0, w: m.w as i32, h: m.h as i32 })));
                }
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
                b.1 = false;
            }
        }
    }
}

impl App {
    /// A position on a surface (logical units) in its picture's pixels.
    fn to_px(&self, i: usize, x: f64, y: f64) -> (f64, f64) {
        let s = &self.screens[i];
        match (&s.img, s.size) {
            (Some(m), (w, h)) if w > 0 && h > 0 => (x * m.w as f64 / w as f64, y * m.h as f64 / h as f64),
            _ => (x, y),
        }
    }

    fn draw(&mut self, i: usize, qh: &QueueHandle<App>) {
        let chosen = choice(self).filter(|c| c.0 == i).map(|c| c.1);
        let accent = self.accent;
        let shm = self.shm.clone();
        let s = &mut self.screens[i];
        let (Some(img), Some(surface)) = (&s.img, &s.surface) else { return };
        if !s.dirty || s.frame_pending || s.size.0 == 0 {
            return;
        }
        let (w, h) = (img.w, img.h);
        let one = (w * h * 4) as usize;
        if s.bufs.is_empty() {
            let Ok(fd) = rustix::fs::memfd_create(c"yggdrasil-shot", rustix::fs::MemfdFlags::CLOEXEC) else { return };
            if rustix::fs::ftruncate(&fd, (one * 2) as u64).is_err() {
                return;
            }
            let file = std::fs::File::from(fd);
            let Ok(map) = (unsafe { MmapMut::map_mut(&file) }) else { return };
            let pool = shm.create_pool(file.as_fd(), (one * 2) as i32, qh, ());
            s.bufs = (0..2).map(|k| (pool.create_buffer((k * one) as i32, w as i32, h as i32, (w * 4) as i32, wl_shm::Format::Argb8888, qh, (i, k)), false)).collect();
            pool.destroy();
            s.map = Some(map);
        }
        let Some(k) = s.bufs.iter().position(|b| !b.1) else { return };
        let Some(map) = s.map.as_mut() else { return };
        let dst = &mut map[k * one..(k + 1) * one];
        dst.copy_from_slice(&img.dim);
        if let Some(r) = chosen.and_then(|r| r.clamp(w, h)) {
            let stride = (w * 4) as usize;
            for y in r.y..r.y + r.h {
                let a = y as usize * stride + r.x as usize * 4;
                let b = a + r.w as usize * 4;
                dst[a..b].copy_from_slice(&img.bright[a..b]);
            }
            // a border in the accent colour, inside the choice
            let px = [accent.2, accent.1, accent.0, 255];
            let t = 2.min(r.w).min(r.h);
            let mut fill = |x0: i32, y0: i32, x1: i32, y1: i32| {
                for y in y0..y1 {
                    for x in x0..x1 {
                        let o = y as usize * stride + x as usize * 4;
                        dst[o..o + 4].copy_from_slice(&px);
                    }
                }
            };
            fill(r.x, r.y, r.x + r.w, r.y + t);
            fill(r.x, r.y + r.h - t, r.x + r.w, r.y + r.h);
            fill(r.x, r.y, r.x + t, r.y + r.h);
            fill(r.x + r.w - t, r.y, r.x + r.w, r.y + r.h);
        }
        s.bufs[k].1 = true;
        surface.attach(Some(&s.bufs[k].0), 0, 0);
        surface.damage_buffer(0, 0, w as i32, h as i32);
        surface.frame(qh, i);
        surface.commit();
        s.frame_pending = true;
        s.dirty = false;
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

/// Binary PPM (P6, 8 bits) to BGRA, with a dimmed copy.
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
    let mut bright = Vec::with_capacity((w * h * 4) as usize);
    let mut dim = Vec::with_capacity((w * h * 4) as usize);
    for p in rgb.chunks_exact(3) {
        bright.extend_from_slice(&[p[2], p[1], p[0], 255]);
        dim.extend_from_slice(&[(p[2] as u32 * DIM >> 8) as u8, (p[1] as u32 * DIM >> 8) as u8, (p[0] as u32 * DIM >> 8) as u8, 255]);
    }
    Some(Img { w, h, bright, dim })
}

/// The windows on each screen, from Hyprland (in each screen's picture's pixels). Empty elsewhere: then a click takes the screen.
fn hypr_windows() -> std::collections::HashMap<String, Vec<R>> {
    let mut out = std::collections::HashMap::new();
    let json = |a: &str| -> Option<serde_json::Value> {
        let o = Command::new("hyprctl").args(["-j", a]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        serde_json::from_slice(&o.stdout).ok()
    };
    let (Some(mons), Some(clients)) = (json("monitors"), json("clients")) else { return out };
    for m in mons.as_array().into_iter().flatten() {
        let name = m["name"].as_str().unwrap_or("").to_string();
        let (mx, my, scale) = (m["x"].as_f64().unwrap_or(0.0), m["y"].as_f64().unwrap_or(0.0), m["scale"].as_f64().unwrap_or(1.0));
        let shown = [m["activeWorkspace"]["id"].as_i64(), m["specialWorkspace"]["id"].as_i64().filter(|id| *id != 0)];
        let mut v = vec![];
        for c in clients.as_array().into_iter().flatten() {
            if c["hidden"].as_bool() == Some(true) || c["mapped"].as_bool() == Some(false) || !shown.contains(&c["workspace"]["id"].as_i64()) {
                continue;
            }
            let at = (c["at"][0].as_f64().unwrap_or(0.0), c["at"][1].as_f64().unwrap_or(0.0));
            let size = (c["size"][0].as_f64().unwrap_or(0.0), c["size"][1].as_f64().unwrap_or(0.0));
            v.push(R::from_corners(((at.0 - mx) * scale, (at.1 - my) * scale), ((at.0 - mx + size.0) * scale, (at.1 - my + size.1) * scale)));
        }
        out.insert(name, v);
    }
    out
}

fn save(img: &Img, r: R) -> Result<std::path::PathBuf, String> {
    let r = r.clamp(img.w, img.h).ok_or("nothing chosen")?;
    let mut rgba = Vec::with_capacity((r.w * r.h * 4) as usize);
    for y in r.y..r.y + r.h {
        let row = &img.bright[(y as u32 * img.w * 4 + r.x as u32 * 4) as usize..][..(r.w * 4) as usize];
        for p in row.chunks_exact(4) {
            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
        }
    }
    let pictures = Command::new("xdg-user-dir").arg("PICTURES").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).filter(|s| !s.is_empty());
    let dir = std::path::PathBuf::from(pictures.unwrap_or_else(|| format!("{}/Pictures", std::env::var("HOME").unwrap_or_default()))).join("Screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let stamp = Command::new("date").arg("+%F %H-%M-%S").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mut path = dir.join(format!("Screenshot {stamp}.png"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("Screenshot {stamp} ({n}).png"));
        n += 1;
    }
    image::save_buffer(&path, &rgba, r.w as u32, r.h as u32, image::ExtendedColorType::Rgba8).map_err(|e| e.to_string())?;
    Ok(path)
}

fn main() {
    let Ok(conn) = Connection::connect_to_env() else {
        eprintln!("yggdrasil-shot: no Wayland display");
        std::process::exit(1);
    };
    let Ok((globals, mut queue)) = registry_queue_init::<App>(&conn) else { std::process::exit(1) };
    let qh = queue.handle();
    let (Ok(compositor), Ok(shm), Ok(layer_shell)) = (globals.bind::<WlCompositor, _, _>(&qh, 1..=6, ()), globals.bind::<WlShm, _, _>(&qh, 1..=1, ()), globals.bind::<ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())) else {
        eprintln!("yggdrasil-shot: the compositor has no layer shell");
        std::process::exit(1);
    };
    let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
    let accent = design::accent();
    let mut app = App { compositor, shm, screens: vec![], cursor: globals.bind(&qh, 1..=1, ()).ok(), cursor_dev: None, pointer: None, keyboard: None, at: None, press: None, accent: (accent.0, accent.1, accent.2), result: None };
    // the screens' names first (nothing is shown yet)
    let outputs: Vec<(u32, u32)> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect());
    for (n, (name, version)) in outputs.into_iter().enumerate() {
        let output = globals.registry().bind::<WlOutput, _, _>(name, version.min(4), &qh, n);
        app.screens.push(Screen { name: String::new(), output, img: None, surface: None, layer: None, viewport: None, size: (0, 0), bufs: vec![], map: None, windows: vec![], frame_pending: false, dirty: true });
    }
    let _ = queue.roundtrip(&mut app);
    // capture every screen at once, before anything of ours is on it
    let names: Vec<String> = app.screens.iter().map(|s| s.name.clone()).collect();
    let shots: Vec<Option<Img>> = std::thread::scope(|sc| names.iter().map(|n| sc.spawn(move || capture(n))).collect::<Vec<_>>().into_iter().map(|h| h.join().ok().flatten()).collect());
    let mut windows = hypr_windows();
    for (s, img) in app.screens.iter_mut().zip(shots) {
        s.img = img;
        s.windows = windows.remove(&s.name).unwrap_or_default();
    }
    if app.screens.iter().all(|s| s.img.is_none()) {
        eprintln!("yggdrasil-shot: could not capture the screen (is grim installed?)");
        let _ = Command::new("notify-send").args(["Screenshot", "The screen could not be captured"]).status();
        std::process::exit(1);
    }
    // the frozen pictures, over everything, taking the keyboard and the pointer
    for i in 0..app.screens.len() {
        if app.screens[i].img.is_none() {
            continue;
        }
        let surface = app.compositor.create_surface(&qh, ());
        let layer = layer_shell.get_layer_surface(&surface, Some(&app.screens[i].output), zwlr_layer_shell_v1::Layer::Overlay, "yggdrasil-shot".into(), &qh, i);
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
    let seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    if seat.is_none() {
        eprintln!("yggdrasil-shot: no seat");
    }
    while app.result.is_none() {
        if queue.blocking_dispatch(&mut app).is_err() {
            break;
        }
    }
    // away at once, then save (the picture is already taken)
    for s in &app.screens {
        if let Some(l) = &s.layer {
            l.destroy();
        }
        if let Some(sf) = &s.surface {
            sf.destroy();
        }
    }
    let _ = conn.flush();
    let Some(Some((i, r))) = app.result else { return };
    let Some(img) = app.screens[i].img.as_ref() else { return };
    match save(img, r) {
        Ok(path) => {
            if let Ok(f) = std::fs::File::open(&path) {
                let _ = Command::new("wl-copy").args(["--type", "image/png"]).stdin(f).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
            }
            let _ = Command::new("notify-send").arg("-i").arg(&path).args(["Screenshot", "Saved to Pictures/Screenshots and copied"]).spawn();
            eprintln!("yggdrasil-shot: {}", path.display());
        }
        Err(e) => {
            eprintln!("yggdrasil-shot: {e}");
            let _ = Command::new("notify-send").args(["Screenshot", &format!("Could not save it: {e}")]).spawn();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppm_becomes_bgra() {
        let mut ppm = b"P6\n# grim\n2 1\n255\n".to_vec();
        ppm.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let m = from_ppm(&ppm).unwrap();
        assert_eq!((m.w, m.h), (2, 1));
        assert_eq!(m.bright, vec![0, 0, 255, 255, 255, 0, 0, 255]);
        assert!(m.dim[2] < 255 && m.dim[2] > 0);
        assert!(from_ppm(b"P5\n1 1\n255\n\0").is_none());
    }

    #[test]
    fn a_drag_in_any_direction_is_one_rectangle() {
        assert_eq!(R::from_corners((10.0, 20.0), (4.0, 2.0)), R { x: 4, y: 2, w: 6, h: 18 });
        assert_eq!(R { x: -5, y: 5, w: 20, h: 100 }.clamp(10, 50), Some(R { x: 0, y: 5, w: 10, h: 45 }));
        assert_eq!(R { x: 20, y: 0, w: 5, h: 5 }.clamp(10, 10), None);
    }

    #[test]
    fn the_smallest_window_under_the_pointer_wins() {
        let main = R { x: 0, y: 0, w: 100, h: 100 };
        let dialog = R { x: 20, y: 20, w: 30, h: 30 };
        assert_eq!(window_at(&[main, dialog], (25.0, 25.0)), Some(dialog));
        assert_eq!(window_at(&[main, dialog], (80.0, 80.0)), Some(main));
        assert_eq!(window_at(&[main], (150.0, 1.0)), None);
    }
}
