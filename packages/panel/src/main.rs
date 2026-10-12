//! labyrinth-panel: Labyrinth's top bar and dock, written for Wayland (wlr-layer-shell) and drawn
//! on the CPU. Windows come from KWin through the plasma-window-management protocol, which KWin
//! only gives to programs whose desktop file asks for it (labyrinth-panel.desktop).
//!
//! One full bar on every screen except the tiling screen (which gets a slim one), and the dock on
//! the main screen. Which screen is which comes from KDE (kscreen priority, the tiling setting);
//! it is re-read every few seconds, so changing it in Settings takes effect live.

mod apps;
mod paint;
mod shell;

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use memmap2::MmapMut;
use resvg::tiny_skia::Pixmap;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_registry::{self, WlRegistry};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::{Shape, WpCursorShapeDeviceV1};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::{self, OrgKdePlasmaWindow};
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window_management::{self, OrgKdePlasmaWindowManagement};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::{Layer, ZwlrLayerShellV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{self, Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1};

use labyrinth_conf::PanelConf;
use shell::{Action, Hit, Model, Win};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    Bar { slim: bool },
    Dock,
}

struct Buffer {
    buffer: WlBuffer,
    map: MmapMut,
    w: u32,
    h: u32,
    busy: Arc<AtomicBool>,
}

struct Surf {
    id: usize,
    kind: Kind,
    /// Global name of the output this sits on.
    output: u32,
    wl: WlSurface,
    layer: ZwlrLayerSurfaceV1,
    /// Logical size the compositor gave us, and the size we asked for.
    w: u32,
    h: u32,
    want: (u32, u32),
    configured: bool,
    bufs: Vec<Buffer>,
    hits: Vec<Hit>,
    hover: Option<usize>,
}

struct Out {
    wl: WlOutput,
    name: String,
    scale: i32,
    ready: bool,
}

struct Assets {
    fonts: paint::Fonts,
    icons: paint::Icons,
}

/// Which screen does what, as KDE has it configured.
#[derive(Clone, Debug, Default, PartialEq)]
struct Layout {
    primary: String,
    tiling: Vec<String>,
}

struct State {
    qh: QueueHandle<State>,
    conn: Connection,
    compositor: WlCompositor,
    layer_shell: ZwlrLayerShellV1,
    shm: WlShm,
    model: Model,
    assets: Assets,
    surfs: Vec<Surf>,
    next_surf: usize,
    outputs: HashMap<u32, Out>,
    layout: Layout,
    built_for: Option<Layout>,
    pointer_surface: Option<usize>,
    cursor: Option<WpCursorShapeDeviceV1>,
    cursor_mgr: Option<WpCursorShapeManagerV1>,
    seat: Option<WlSeat>,
    next_key: u32,
    dirty: bool,
}

// ------------------------------------------------------------------ small helpers

fn clock_text() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        let mut buf = [0u8; 64];
        let fmt = c"%a %b %-d  %-I:%M %p";
        let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), fmt.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).to_string()
    }
}

fn command_output(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

/// The main screen (kscreen priority 1) and the tiling screens, from KDE.
fn read_layout() -> Layout {
    let json: serde_json::Value = serde_json::from_str(&command_output("kscreen-doctor", &["-j"])).unwrap_or(serde_json::Value::Null);
    let primary = json["outputs"]
        .as_array()
        .and_then(|a| a.iter().find(|o| o["priority"].as_i64() == Some(1) && o["enabled"].as_bool().unwrap_or(false)))
        .and_then(|o| o["name"].as_str())
        .unwrap_or("")
        .to_string();
    let tiling = command_output("kreadconfig6", &["--file", "kwinrc", "--group", "Script-labyrinth-screen-keys", "--key", "tilingOutputs"])
        .trim()
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    Layout { primary, tiling }
}

/// Whether the system style is dark (the colour scheme's name says so; Settings > Appearance).
fn system_dark() -> bool {
    let scheme = command_output("kreadconfig6", &["--file", "kdeglobals", "--group", "General", "--key", "ColorScheme"]);
    let scheme = scheme.trim().to_lowercase();
    scheme.is_empty() || scheme.contains("dark")
}

fn new_buffer(shm: &WlShm, qh: &QueueHandle<State>, w: u32, h: u32) -> Option<Buffer> {
    let stride = w * 4;
    let size = (stride * h) as usize;
    let fd = rustix::fs::memfd_create(c"labyrinth-panel", rustix::fs::MemfdFlags::CLOEXEC).ok()?;
    rustix::fs::ftruncate(&fd, size as u64).ok()?;
    let file = std::fs::File::from(fd);
    let map = unsafe { MmapMut::map_mut(&file).ok()? };
    let pool = shm.create_pool(file.as_fd(), size as i32, qh, ());
    let busy = Arc::new(AtomicBool::new(false));
    let buffer = pool.create_buffer(0, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, qh, busy.clone());
    pool.destroy();
    Some(Buffer { buffer, map, w, h, busy })
}

fn run(cmd: &[&str]) {
    let Some((p, a)) = cmd.split_first() else { return };
    let _ = std::process::Command::new(p).args(a).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

impl State {
    fn scale_of(&self, s: &Surf) -> i32 {
        self.outputs.get(&s.output).map(|o| o.scale).unwrap_or(1).max(1)
    }

    /// Which output the dock goes on: the main screen unless it is the tiling one.
    fn dock_output(&self) -> Option<u32> {
        let ready = || self.outputs.iter().filter(|(_, o)| o.ready && !o.name.is_empty());
        let is_tiling = |o: &Out| self.layout.tiling.contains(&o.name);
        ready()
            .find(|(_, o)| o.name == self.layout.primary && !is_tiling(o))
            .or_else(|| ready().find(|(_, o)| !is_tiling(o)))
            .or_else(|| ready().next())
            .map(|(k, _)| *k)
    }

    /// Make the surfaces match the screens and the layout: a bar on each, the dock on one.
    fn build_surfaces(&mut self) {
        let ready: Vec<(u32, String)> = self.outputs.iter().filter(|(_, o)| o.ready && !o.name.is_empty()).map(|(k, o)| (*k, o.name.clone())).collect();
        if ready.is_empty() || self.outputs.values().any(|o| !o.ready) && self.surfs.is_empty() && ready.len() < self.outputs.len() {
            return;
        }
        for s in self.surfs.drain(..) {
            s.layer.destroy();
            s.wl.destroy();
        }
        self.pointer_surface = None;
        let dock_out = self.dock_output();
        let mut sorted = ready;
        sorted.sort_by(|a, b| a.1.cmp(&b.1));
        for (key, name) in sorted {
            let slim = self.layout.tiling.contains(&name);
            self.add_surface(key, Kind::Bar { slim });
            if Some(key) == dock_out {
                self.add_surface(key, Kind::Dock);
            }
        }
        self.built_for = Some(self.layout.clone());
    }

    fn add_surface(&mut self, output: u32, kind: Kind) {
        let id = self.next_surf;
        self.next_surf += 1;
        let Some(out) = self.outputs.get(&output) else { return };
        let wl = self.compositor.create_surface(&self.qh, ());
        let layer = self.layer_shell.get_layer_surface(&wl, Some(&out.wl), Layer::Top, "labyrinth-panel".into(), &self.qh, id);
        let (anchor, size, zone, margin) = match kind {
            Kind::Bar { .. } => (Anchor::Top | Anchor::Left | Anchor::Right, (0, shell::BAR_H), shell::BAR_H as i32, (0, 0, 0, 0)),
            Kind::Dock => {
                let sz = shell::dock_size(&shell::dock_items(&self.model), self.model.conf.dock_icon);
                (Anchor::Bottom, sz, sz.1 as i32 + 8, (0, 0, 8, 0))
            }
        };
        layer.set_anchor(anchor);
        layer.set_size(size.0, size.1);
        layer.set_exclusive_zone(zone);
        layer.set_margin(margin.0, margin.1, margin.2, margin.3);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        wl.commit();
        self.surfs.push(Surf { id, kind, output, wl, layer, w: size.0, h: size.1, want: size, configured: false, bufs: vec![], hits: vec![], hover: None });
    }

    /// Ask for a new dock size if the items changed it.
    fn resize_dock(&mut self) {
        let want = shell::dock_size(&shell::dock_items(&self.model), self.model.conf.dock_icon);
        for s in self.surfs.iter_mut().filter(|s| s.kind == Kind::Dock) {
            if s.want != want {
                s.want = want;
                s.layer.set_size(want.0, want.1);
                s.layer.set_exclusive_zone(want.1 as i32 + 8); // windows keep clear of the dock at any size
                s.wl.commit();
            }
        }
    }

    fn redraw(&mut self) {
        self.resize_dock();
        for i in 0..self.surfs.len() {
            self.draw(i);
        }
        let _ = self.conn.flush();
        self.dirty = false;
    }

    fn draw(&mut self, i: usize) {
        let (kind, w, h, hover, configured) = {
            let s = &self.surfs[i];
            (s.kind, s.w, s.h, s.hover, s.configured)
        };
        let scale = self.scale_of(&self.surfs[i]);
        if !configured || w == 0 || h == 0 {
            return;
        }
        let (pw, ph) = (w * scale as u32, h * scale as u32);
        let Some(mut pix) = Pixmap::new(pw, ph) else { return };
        let s = scale as f32;
        let hits = match kind {
            Kind::Bar { slim } => shell::paint_bar(&mut pix, s, slim, &self.model, &self.assets.fonts, &mut self.assets.icons, hover),
            Kind::Dock => shell::paint_dock(&mut pix, s, &self.model, &self.assets.fonts, &mut self.assets.icons, hover),
        };
        let surf = &mut self.surfs[i];
        surf.hits = hits;
        let idx = match surf.bufs.iter().position(|b| !b.busy.load(Ordering::Acquire) && b.w == pw && b.h == ph) {
            Some(i) => i,
            None => {
                surf.bufs.retain(|b| b.busy.load(Ordering::Acquire) || (b.w == pw && b.h == ph));
                match new_buffer(&self.shm, &self.qh, pw, ph) {
                    Some(b) => {
                        surf.bufs.push(b);
                        surf.bufs.len() - 1
                    }
                    None => return,
                }
            }
        };
        let b = &mut surf.bufs[idx];
        // tiny-skia is premultiplied RGBA; wl_shm's ARGB8888 (little-endian) is BGRA in memory.
        for (dst, src) in b.map.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        b.busy.store(true, Ordering::Release);
        surf.wl.set_buffer_scale(scale);
        surf.wl.attach(Some(&b.buffer), 0, 0);
        surf.wl.damage_buffer(0, 0, pw as i32, ph as i32);
        surf.wl.commit();
    }

    // -------------------------------------------------------------- actions

    fn click(&mut self, action: Action) {
        match action {
            Action::Logo => run(&["qdbus6", "org.kde.krunner", "/App", "org.kde.krunner.App.display"]),
            Action::Clock => {}
            Action::Dock(app_id) => self.click_dock(&app_id),
        }
    }

    fn click_dock(&mut self, app_id: &str) {
        let items = shell::dock_items(&self.model);
        let Some(item) = items.iter().find(|i| i.app_id == app_id) else { return };
        if item.wins.is_empty() {
            if let Some(app) = self.model.apps.iter().find(|a| a.id == app_id) {
                apps::launch(app);
            }
            return;
        }
        // One window: focus it, or minimize it if it already has focus. Several: go to the next.
        let wins = &self.model.wins;
        let active_pos = item.wins.iter().position(|&i| wins[i].active());
        let target = match active_pos {
            Some(p) if item.wins.len() == 1 => {
                wins[item.wins[p]].proxy.set_state(shell::ST_MINIMIZED, shell::ST_MINIMIZED);
                return;
            }
            Some(p) => item.wins[(p + 1) % item.wins.len()],
            None => item.wins[0],
        };
        wins[target].proxy.set_state(shell::ST_ACTIVE | shell::ST_MINIMIZED, shell::ST_ACTIVE);
    }

    fn surf_index(&self, wl: &WlSurface) -> Option<usize> {
        self.surfs.iter().position(|s| &s.wl == wl)
    }

    fn set_cursor(&self, serial: u32) {
        if let Some(c) = &self.cursor {
            c.set_shape(serial, Shape::Default);
        }
    }

    fn hover_at(&mut self, x: f32, y: f32) {
        let Some(i) = self.pointer_surface else { return };
        let s = self.scale_of(&self.surfs[i]) as f32;
        let new = self.surfs[i].hits.iter().position(|h| h.contains(x * s, y * s));
        if new != self.surfs[i].hover {
            self.surfs[i].hover = new;
            self.dirty = true;
        }
    }
}

// ------------------------------------------------------------------ Wayland dispatch

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(state: &mut Self, registry: &WlRegistry, event: wl_registry::Event, _: &GlobalListContents, _: &Connection, qh: &QueueHandle<Self>) {
        match event {
            wl_registry::Event::Global { name, interface, version } if interface == "wl_output" => {
                let wl = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, name);
                state.outputs.insert(name, Out { wl, name: String::new(), scale: 1, ready: false });
            }
            wl_registry::Event::GlobalRemove { name } => {
                if state.outputs.remove(&name).is_some() {
                    state.built_for = None; // a screen went away: rebuild
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, u32> for State {
    fn event(state: &mut Self, _: &WlOutput, event: wl_output::Event, key: &u32, _: &Connection, _: &QueueHandle<Self>) {
        let Some(o) = state.outputs.get_mut(key) else { return };
        match event {
            wl_output::Event::Name { name } => o.name = name,
            wl_output::Event::Scale { factor } => {
                o.scale = factor.max(1);
                state.dirty = true;
            }
            wl_output::Event::Done => {
                if !o.ready {
                    o.ready = true;
                    state.built_for = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSurface, ()> for State {
    fn event(_: &mut Self, _: &WlSurface, _: wl_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlBuffer, Arc<AtomicBool>> for State {
    fn event(_: &mut Self, _: &WlBuffer, event: wl_buffer::Event, busy: &Arc<AtomicBool>, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            busy.store(false, Ordering::Release);
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for State {
    fn event(state: &mut Self, layer: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, id: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                layer.ack_configure(serial);
                let Some(i) = state.surfs.iter().position(|s| s.id == *id) else { return };
                let s = &mut state.surfs[i];
                if width > 0 {
                    s.w = width;
                }
                if height > 0 {
                    s.h = height;
                }
                s.configured = true;
                state.draw(i);
                let _ = state.conn.flush();
            }
            zwlr_layer_surface_v1::Event::Closed => {
                // The screen went away or the compositor revoked the surface; rebuild from scratch.
                if let Some(i) = state.surfs.iter().position(|s| s.id == *id) {
                    let s = state.surfs.remove(i);
                    s.layer.destroy();
                    s.wl.destroy();
                    state.pointer_surface = None;
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = event {
            if c.contains(wl_seat::Capability::Pointer) && state.seat.is_none() {
                state.seat = Some(seat.clone());
                let p = seat.get_pointer(qh, ());
                if let Some(m) = &state.cursor_mgr {
                    state.cursor = Some(m.get_pointer(&p, qh, ()));
                }
            }
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(state: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { serial, surface, surface_x, surface_y } => {
                state.pointer_surface = state.surf_index(&surface);
                state.set_cursor(serial);
                state.hover_at(surface_x as f32, surface_y as f32);
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(i) = state.pointer_surface.take() {
                    if state.surfs.get_mut(i).map(|s| s.hover.take().is_some()).unwrap_or(false) {
                        state.dirty = true;
                    }
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => state.hover_at(surface_x as f32, surface_y as f32),
            wl_pointer::Event::Button { state: WEnum::Value(wl_pointer::ButtonState::Pressed), button, .. } => {
                // 272 = BTN_LEFT
                if button == 272 {
                    if let Some(i) = state.pointer_surface {
                        if let Some(h) = state.surfs.get(i).and_then(|s| s.hover) {
                            if let Some(hit) = state.surfs[i].hits.get(h).cloned() {
                                state.click(hit.action);
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<OrgKdePlasmaWindowManagement, ()> for State {
    fn event(state: &mut Self, wm: &OrgKdePlasmaWindowManagement, event: org_kde_plasma_window_management::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let org_kde_plasma_window_management::Event::WindowWithUuid { uuid, .. } = event {
            state.next_key += 1;
            let key = state.next_key;
            let proxy = wm.get_window_by_uuid(uuid.clone(), qh, key);
            state.model.wins.push(Win { key, uuid, title: String::new(), app_id: String::new(), flags: 0, ready: false, menu: None, proxy });
        }
    }
}

impl Dispatch<OrgKdePlasmaWindow, u32> for State {
    fn event(state: &mut Self, _: &OrgKdePlasmaWindow, event: org_kde_plasma_window::Event, key: &u32, _: &Connection, _: &QueueHandle<Self>) {
        use org_kde_plasma_window::Event as E;
        let Some(pos) = state.model.wins.iter().position(|w| w.key == *key) else { return };
        match event {
            E::TitleChanged { title } => state.model.wins[pos].title = title,
            E::AppIdChanged { app_id } => state.model.wins[pos].app_id = app_id,
            E::StateChanged { flags } => state.model.wins[pos].flags = flags,
            E::ApplicationMenu { service_name, object_path } => state.model.wins[pos].menu = Some((service_name, object_path)),
            E::InitialState => state.model.wins[pos].ready = true,
            E::Unmapped => {
                let w = state.model.wins.remove(pos);
                w.proxy.destroy();
            }
            _ => return,
        }
        state.dirty = true;
    }
}

impl Dispatch<WpCursorShapeManagerV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeManagerV1, _: <WpCursorShapeManagerV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
impl Dispatch<WpCursorShapeDeviceV1, ()> for State {
    fn event(_: &mut Self, _: &WpCursorShapeDeviceV1, _: <WpCursorShapeDeviceV1 as Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(State: WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ZwlrLayerShellV1);

// ------------------------------------------------------------------ main

fn bind_outputs(globals: &GlobalList, state: &mut State, qh: &QueueHandle<State>) {
    let existing: Vec<(u32, u32)> = globals.contents().clone_list().iter().filter(|g| g.interface == "wl_output").map(|g| (g.name, g.version)).collect();
    for (name, version) in existing {
        let wl = globals.registry().bind::<WlOutput, _, _>(name, version.min(4), qh, name);
        state.outputs.insert(name, Out { wl, name: String::new(), scale: 1, ready: false });
    }
}

fn main() {
    let Some(fonts) = paint::Fonts::load() else {
        eprintln!("labyrinth-panel: no sans-serif font found (fc-match)");
        std::process::exit(1);
    };
    let conn = Connection::connect_to_env().unwrap_or_else(|e| {
        eprintln!("labyrinth-panel: cannot connect to Wayland: {e}");
        std::process::exit(1);
    });
    let (globals, mut queue) = registry_queue_init::<State>(&conn).expect("registry");
    let qh = queue.handle();

    let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
    let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).unwrap_or_else(|_| {
        eprintln!("labyrinth-panel: the compositor has no wlr-layer-shell");
        std::process::exit(1);
    });
    let _seat: Option<WlSeat> = globals.bind(&qh, 1..=7, ()).ok();
    let cursor_mgr: Option<WpCursorShapeManagerV1> = globals.bind(&qh, 1..=1, ()).ok();
    let wm: Option<OrgKdePlasmaWindowManagement> = globals.bind(&qh, 1..=18, ()).ok();
    if wm.is_none() {
        eprintln!("labyrinth-panel: KWin did not offer window management (is labyrinth-panel.desktop installed with X-KDE-Wayland-Interfaces?); the dock will only launch apps");
    }

    let mut state = State {
        qh: qh.clone(),
        conn: conn.clone(),
        compositor,
        layer_shell,
        shm,
        model: Model { apps: apps::load_all(), pinned: apps::pinned(), wins: vec![], clock: clock_text(), conf: PanelConf::load(), dark: system_dark() },
        assets: Assets { fonts, icons: paint::Icons::new() },
        surfs: vec![],
        next_surf: 0,
        outputs: HashMap::new(),
        layout: read_layout(),
        built_for: None,
        pointer_surface: None,
        cursor: None,
        cursor_mgr,
        seat: None,
        next_key: 0,
        dirty: false,
    };
    bind_outputs(&globals, &mut state, &qh);

    let mut event_loop: EventLoop<State> = EventLoop::try_new().expect("event loop");
    // Learn the outputs' names before drawing anything.
    queue.roundtrip(&mut state).expect("roundtrip");
    queue.roundtrip(&mut state).expect("roundtrip");
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).expect("wayland source");

    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(1)), |_, _, st| {
            let now = clock_text();
            if now != st.model.clock {
                st.model.clock = now;
                st.dirty = true;
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .expect("clock timer");
    // KDE's screen settings can change under us (Settings, kscreen-doctor, a plugged-in monitor).
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(4)), |_, _, st| {
            let l = read_layout();
            if l != st.layout {
                st.layout = l;
                st.built_for = None;
            }
            let dark = system_dark();
            if dark != st.model.dark {
                st.model.dark = dark;
                st.dirty = true;
            }
            TimeoutAction::ToDuration(Duration::from_secs(4))
        })
        .expect("layout timer");

    // Settings writes ~/.config/labyrinth/panel.conf; pick changes up within a second.
    let mut seen = PanelConf::modified();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(1)), move |_, _, st| {
            let now = PanelConf::modified();
            if now != seen {
                seen = now;
                st.model.conf = PanelConf::load();
                st.dirty = true;
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .expect("config timer");

    event_loop
        .run(None, &mut state, |st| {
            if st.built_for.is_none() {
                st.build_surfaces();
            }
            if st.dirty {
                st.redraw();
            }
        })
        .expect("event loop run");
}
