//! The compositor's state and the protocol handlers: windows (xdg-shell), panels and backgrounds (layer-shell), the screen locker
//! (ext-session-lock), idle detection (ext-idle-notify), and the usual seat, output and clipboard parts.

use std::time::Duration;

use smithay::backend::input::{AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputEvent, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent};
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::backend::winit::WinitInput;
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupKind, PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::keyboard::{FilterResult, KeyboardHandle};
use smithay::input::pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, PointerHandle};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState};
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::selection::data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker};
use smithay::wayland::shell::wlr_layer::{Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState};
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::input::pointer::Focus;
use smithay::utils::Rectangle;
use crate::grabs::{MoveGrab, ResizeGrab};
use crate::shortcuts::{self, Action, Binding, Mods};
use crate::toplevels::Toplevels;
use crate::deco::{self, Bar, Hit, Key as BarKey};
use crate::draw::Fonts;
use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::{ImportAll, ImportMem};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecoMode;
use smithay::utils::Scale;
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::delegate_xdg_decoration;
use std::collections::HashMap;

smithay::render_elements! {
    /// What is drawn each frame: the programs' own pictures and the title bars the compositor adds.
    pub Elem<R> where R: ImportAll + ImportMem;
    Surface = WaylandSurfaceRenderElement<R>,
    Bar = MemoryRenderBufferRenderElement<R>,
}
use smithay::backend::input::KeyState;
use smithay::input::keyboard::ModifiersState;
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::viewporter::ViewporterState;
use smithay::{delegate_compositor, delegate_data_device, delegate_idle_notify, delegate_layer_shell, delegate_output, delegate_seat, delegate_session_lock, delegate_shm, delegate_viewporter, delegate_xdg_shell};

pub struct App {
    pub display_handle: DisplayHandle,
    pub loop_signal: LoopSignal,
    pub space: Space<Window>,
    pub popups: PopupManager,
    pub output: Output,
    pub compositor_state: CompositorState,
    pub xdg_shell_state: XdgShellState,
    pub layer_shell_state: WlrLayerShellState,
    pub shm_state: ShmState,
    pub seat_state: SeatState<Self>,
    pub data_device_state: DataDeviceState,
    pub output_manager_state: OutputManagerState,
    pub viewporter_state: ViewporterState,
    pub lock_state: SessionLockManagerState,
    pub idle_state: IdleNotifierState<Self>,
    pub seat: Seat<Self>,
    pub keyboard: KeyboardHandle<Self>,
    pub pointer: PointerHandle<Self>,
    pub pointer_location: Point<f64, Logical>,
    /// The session is locked: only `lock_surface` is shown and it alone gets input.
    pub locked: bool,
    lock_surface: Option<LockSurface>,
    opened: usize,
    /// Where a window was (and how big) before it was maximized or made full screen.
    saved: Vec<(WlSurface, Rectangle<i32, Logical>)>,
    /// Windows hidden with minimize (still alive, listed to shell programs, brought back with activate).
    pub minimized: Vec<Window>,
    pub toplevels: Toplevels,
    pub bindings: Vec<Binding>,
    /// The Super key went down and nothing else was pressed since: letting go of it opens the start menu.
    super_tap: bool,
    /// Keys whose press was taken by a shortcut: their release is not passed on either.
    taken: Vec<u32>,
    /// What the shortcuts asked for while the keyboard handler was busy.
    queued: Vec<Action>,
    /// The Wayland socket programs started by shortcuts connect to.
    pub socket: String,
    fonts: Option<Fonts>,
    bars: HashMap<WlSurface, Bar>,
    /// Windows that have a title bar drawn by the compositor (they asked for server-side decoration).
    decorated: Vec<WlSurface>,
    decorate_all: bool,
    bar_hover: Option<(WlSurface, Hit)>,
    last_click: Option<(std::time::Instant, WlSurface)>,
    pub decoration_state: XdgDecorationState,
}

#[derive(Default)]
pub struct ClientState {
    pub compositor_state: CompositorClientState,
}
impl ClientData for ClientState {
    fn initialized(&self, _: ClientId) {}
    fn disconnected(&self, _: ClientId, _: DisconnectReason) {}
}

impl App {
    pub fn new(dh: &DisplayHandle, output: Output, handle: LoopHandle<'static, App>, loop_signal: LoopSignal) -> App {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");
        let keyboard = seat.add_keyboard(Default::default(), 300, 40).expect("keyboard");
        let pointer = seat.add_pointer();
        App {
            display_handle: dh.clone(),
            loop_signal,
            space: Space::default(),
            popups: PopupManager::default(),
            output,
            compositor_state: CompositorState::new::<Self>(dh),
            xdg_shell_state: XdgShellState::new::<Self>(dh),
            layer_shell_state: WlrLayerShellState::new::<Self>(dh),
            shm_state: ShmState::new::<Self>(dh, vec![]),
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(dh),
            output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(dh),
            viewporter_state: ViewporterState::new::<Self>(dh),
            lock_state: SessionLockManagerState::new::<Self, _>(dh, |_| true),
            idle_state: IdleNotifierState::new(dh, handle),
            seat,
            keyboard,
            pointer,
            pointer_location: (0.0, 0.0).into(),
            locked: false,
            lock_surface: None,
            opened: 0,
            saved: vec![],
            minimized: vec![],
            toplevels: Toplevels::default(),
            bindings: {
                let text = std::fs::read_to_string(shortcuts::config_path()).unwrap_or_default();
                let (b, bad) = shortcuts::load(&text);
                for l in bad {
                    eprintln!("labyrinth-compositor: shortcuts.conf: cannot use the line: {l}");
                }
                b
            },
            super_tap: false,
            taken: vec![],
            queued: vec![],
            socket: String::new(),
            fonts: Fonts::load(),
            bars: HashMap::new(),
            decorated: vec![],
            decorate_all: std::env::var("LABYRINTH_COMPOSITOR_DECORATE").map(|v| v == "all").unwrap_or(false),
            bar_hover: None,
            last_click: None,
            decoration_state: XdgDecorationState::new::<Self>(dh),
        }
    }

    pub fn window_of(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.space.elements().find(|w| w.toplevel().map(|t| t == surface).unwrap_or(false)).cloned()
    }

    /// Put a window on `area` (maximized or full screen), remembering where it was.
    pub fn fit_window(&mut self, surface: &ToplevelSurface, area: Rectangle<i32, Logical>, state: xdg_toplevel::State) {
        let Some(window) = self.window_of(surface) else { return };
        let mut area = area;
        if self.decorated.contains(surface.wl_surface()) && state != xdg_toplevel::State::Fullscreen {
            // the bar takes the top of the area
            area.loc.y += deco::BAR_H;
            area.size.h -= deco::BAR_H;
        }
        if !self.saved.iter().any(|(s, _)| s == surface.wl_surface()) {
            let loc = self.space.element_location(&window).unwrap_or_default();
            self.saved.push((surface.wl_surface().clone(), Rectangle::new(loc, window.geometry().size)));
        }
        surface.with_pending_state(|s| {
            s.states.set(state);
            s.size = Some(area.size);
        });
        surface.send_configure();
        self.space.map_element(window, area.loc, true);
    }

    pub fn restore_window(&mut self, surface: &ToplevelSurface) {
        let Some(window) = self.window_of(surface) else { return };
        let back = self.saved.iter().position(|(s, _)| s == surface.wl_surface()).map(|i| self.saved.remove(i).1);
        surface.with_pending_state(|s| {
            for st in [xdg_toplevel::State::Maximized, xdg_toplevel::State::Fullscreen, xdg_toplevel::State::TiledLeft, xdg_toplevel::State::TiledRight] {
                s.states.unset(st);
            }
            s.size = back.map(|r| r.size);
        });
        surface.send_configure();
        if let Some(r) = back {
            self.space.map_element(window, r.loc, false);
        }
    }

    pub fn toplevel_of(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        self.space.elements().chain(self.minimized.iter()).find_map(|w| w.toplevel().filter(|t| t.wl_surface() == surface).cloned())
    }

    /// Bring a window forward (from minimized too) and give it the keyboard.
    pub fn activate_surface(&mut self, surface: &WlSurface) {
        if let Some(i) = self.minimized.iter().position(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)) {
            let w = self.minimized.remove(i);
            let at = self.saved.iter().find(|(s, _)| s == surface).map(|(_, r)| r.loc).unwrap_or((80, 80).into());
            self.space.map_element(w, at, true);
        }
        let Some(window) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)).cloned() else { return };
        self.space.raise_element(&window, true);
        for w in self.space.elements() {
            w.set_activated(*w == window);
            if let Some(t) = w.toplevel() {
                t.send_pending_configure();
            }
        }
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(surface.clone()), SERIAL_COUNTER.next_serial());
    }

    pub fn close_surface(&mut self, surface: &WlSurface) {
        if let Some(t) = self.toplevel_of(surface) {
            t.send_close();
        }
    }

    pub fn minimize_surface(&mut self, surface: &WlSurface) {
        let Some(window) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)).cloned() else { return };
        let loc = self.space.element_location(&window).unwrap_or_default();
        self.saved.retain(|(s, _)| s != surface);
        self.saved.push((surface.clone(), Rectangle::new(loc, window.geometry().size)));
        self.space.unmap_elem(&window);
        self.minimized.push(window);
        // the keyboard goes to the window now on top
        let next = self.space.elements().last().and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
        let kb = self.keyboard.clone();
        kb.set_focus(self, next, SERIAL_COUNTER.next_serial());
    }

    pub fn set_maximized(&mut self, surface: &WlSurface, on: bool) {
        let Some(t) = self.toplevel_of(surface) else { return };
        if on {
            self.maximize_request(t);
        } else {
            self.unmaximize_request(t);
        }
    }

    pub fn set_fullscreen(&mut self, surface: &WlSurface, on: bool) {
        let Some(t) = self.toplevel_of(surface) else { return };
        if on {
            self.fullscreen_request(t, None);
        } else {
            self.unfullscreen_request(t);
        }
    }

    /// Decides what a key does: a shortcut (taken, not passed to the focused program) or nothing special.
    fn key_filter(&mut self, code: u32, state: KeyState, mods: &ModifiersState, sym: u32) -> FilterResult<Action> {
        if self.locked {
            return FilterResult::Forward;
        }
        if state == KeyState::Released {
            if shortcuts::is_super(sym) && std::mem::take(&mut self.super_tap) {
                self.queued.push(Action::ToggleStart);
            }
            if let Some(i) = self.taken.iter().position(|c| *c == code) {
                self.taken.remove(i);
                return FilterResult::Intercept(Action::None);
            }
            return FilterResult::Forward;
        }
        if shortcuts::is_super(sym) {
            self.super_tap = true;
            return FilterResult::Forward;
        }
        self.super_tap = false;
        match shortcuts::find(&self.bindings, Mods::of(mods), sym) {
            Some(b) => {
                self.taken.push(code);
                FilterResult::Intercept(b.action.clone())
            }
            None => FilterResult::Forward,
        }
    }

    fn spawn(&self, cmd: &str) {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(cmd).env_remove("DISPLAY");
        if !self.socket.is_empty() {
            c.env("WAYLAND_DISPLAY", &self.socket);
        }
        match c.spawn() {
            // reaped in the background so no defunct processes pile up
            Ok(mut child) => {
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
            }
            Err(e) => eprintln!("labyrinth-compositor: cannot start {cmd}: {e}"),
        }
    }

    fn panel(&self, method: &str, arg: Option<u32>) {
        let a = arg.map(|n| format!(" u {n}")).unwrap_or_default();
        self.spawn(&format!("busctl --user call org.labyrinth.Panel /Panel org.labyrinth.Panel {method}{a}"));
    }

    fn focused(&self) -> Option<WlSurface> {
        let f = self.keyboard.current_focus()?;
        self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface() == &f).unwrap_or(false)).and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()))
    }

    pub fn run_action(&mut self, action: Action) {
        use xdg_toplevel::State as S;
        let current = self.focused();
        let has = |app: &App, s: &WlSurface, st: S| app.toplevel_of(s).map(|t| t.current_state().states.contains(st)).unwrap_or(false);
        match action {
            Action::None => {}
            Action::ToggleStart => self.panel("ToggleStart", None),
            Action::Clipboard => self.panel("ShowClipboard", None),
            Action::Sarina => self.panel("AskSarina", None),
            Action::Calendar => self.panel("ToggleCalendar", None),
            Action::Fylgja(n) => self.panel("ShowFylgja", Some(n)),
            Action::Exec(cmd) => self.spawn(&cmd),
            Action::Lock => self.spawn("labyrinth-lock"),
            Action::Quit => self.quit(),
            Action::Close => {
                if let Some(s) = current {
                    self.close_surface(&s);
                }
            }
            Action::Maximize => {
                if let Some(s) = current {
                    self.set_maximized(&s, !has(self, &s, S::Maximized));
                }
            }
            Action::Restore => {
                if let Some(s) = current {
                    if has(self, &s, S::Maximized) || has(self, &s, S::Fullscreen) || has(self, &s, S::TiledLeft) || has(self, &s, S::TiledRight) {
                        if let Some(t) = self.toplevel_of(&s) {
                            self.restore_window(&t);
                        }
                    } else {
                        self.minimize_surface(&s);
                    }
                }
            }
            Action::Fullscreen => {
                if let Some(s) = current {
                    self.set_fullscreen(&s, !has(self, &s, S::Fullscreen));
                }
            }
            Action::Minimize => {
                if let Some(s) = current {
                    self.minimize_surface(&s);
                }
            }
            Action::SnapLeft | Action::SnapRight => {
                let Some(t) = current.and_then(|s| self.toplevel_of(&s)) else { return };
                let zone = layer_map_for_output(&self.output).non_exclusive_zone();
                let half = zone.size.w / 2;
                let (area, st) = if action == Action::SnapLeft { (Rectangle::new(zone.loc, (half, zone.size.h).into()), S::TiledLeft) } else { (Rectangle::new((zone.loc.x + half, zone.loc.y).into(), (zone.size.w - half, zone.size.h).into()), S::TiledRight) };
                self.fit_window(&t, area, st);
            }
            Action::NextWindow => {
                // the window at the bottom comes to the top: repeating goes through all of them
                let next = self.space.elements().next().and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
                if let Some(s) = next {
                    self.activate_surface(&s);
                }
            }
            Action::PrevWindow => {
                let n = self.space.elements().count();
                let prev = self.space.elements().nth(n.saturating_sub(2)).and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
                if let Some(s) = prev {
                    self.activate_surface(&s);
                }
            }
        }
    }

    fn bar_visible(&self, w: &Window) -> bool {
        let Some(t) = w.toplevel() else { return false };
        self.decorated.contains(t.wl_surface()) && !t.current_state().states.contains(xdg_toplevel::State::Fullscreen)
    }

    /// The title bar of a window, in screen coordinates.
    fn bar_rect(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.bar_visible(w) {
            return None;
        }
        let loc = self.space.element_location(w)?;
        Some(Rectangle::new((loc.x, loc.y - deco::BAR_H).into(), (w.geometry().size.w, deco::BAR_H).into()))
    }

    /// A title bar under the point (looking from the top window down; a window in front hides the bar of one behind it).
    fn bar_at(&self, pos: Point<f64, Logical>) -> Option<(Window, Hit)> {
        for w in self.space.elements().rev() {
            if let Some(r) = self.bar_rect(w) {
                if r.to_f64().contains(pos) {
                    return Some((w.clone(), deco::hit(pos.x - r.loc.x as f64, r.size.w)));
                }
            }
            if let Some(loc) = self.space.element_location(w) {
                if Rectangle::new(loc, w.geometry().size).to_f64().contains(pos) {
                    return None;
                }
            }
        }
        None
    }

    /// The resize grips just outside a decorated window (left, right and bottom).
    fn edge_at(&self, pos: Point<f64, Logical>) -> Option<(Window, ResizeEdge)> {
        for w in self.space.elements().rev() {
            if !self.bar_visible(w) {
                continue;
            }
            let loc = self.space.element_location(w)?;
            let size = w.geometry().size;
            let inner = Rectangle::new((loc.x, loc.y - deco::BAR_H).into(), (size.w, size.h + deco::BAR_H).into()).to_f64();
            let g = deco::GRIP as f64;
            let outer = Rectangle::new((inner.loc.x - g, inner.loc.y).into(), (inner.size.w + 2.0 * g, inner.size.h + g).into());
            if inner.contains(pos) {
                return None;
            }
            if outer.contains(pos) {
                let (left, right, bottom) = (pos.x < inner.loc.x, pos.x >= inner.loc.x + inner.size.w, pos.y >= inner.loc.y + inner.size.h);
                let edge = match (left, right, bottom) {
                    (true, _, true) => ResizeEdge::BottomLeft,
                    (_, true, true) => ResizeEdge::BottomRight,
                    (true, _, _) => ResizeEdge::Left,
                    (_, true, _) => ResizeEdge::Right,
                    (_, _, true) => ResizeEdge::Bottom,
                    _ => return None,
                };
                return Some((w.clone(), edge));
            }
        }
        None
    }

    /// A click on a title bar or a resize grip. Returns true when the compositor took it (the program under it never sees it).
    fn press_on_decoration(&mut self, serial: Serial, button: u32) -> bool {
        let pos = self.pointer_location;
        let pointer = self.pointer.clone();
        let start = smithay::input::pointer::GrabStartData { focus: None, button, location: pos };
        if let Some((window, edges)) = self.edge_at(pos) {
            let Some(t) = window.toplevel() else { return false };
            self.activate_surface(&t.wl_surface().clone());
            let origin = self.space.element_location(&window).unwrap_or_default();
            let size = window.geometry().size;
            pointer.set_grab(self, ResizeGrab { start, window, edges, origin, size }, serial, Focus::Clear);
            return true;
        }
        let Some((window, hit)) = self.bar_at(pos) else { return false };
        let Some(surface) = window.toplevel().map(|t| t.wl_surface().clone()) else { return false };
        self.activate_surface(&surface);
        match hit {
            Hit::Close => self.close_surface(&surface),
            Hit::Minimize => self.minimize_surface(&surface),
            Hit::Maximize => {
                let on = !self.toplevel_of(&surface).map(|t| t.current_state().states.contains(xdg_toplevel::State::Maximized)).unwrap_or(false);
                self.set_maximized(&surface, on);
            }
            Hit::Drag => {
                let double = matches!(&self.last_click, Some((t, s)) if s == &surface && t.elapsed() < std::time::Duration::from_millis(400));
                self.last_click = Some((std::time::Instant::now(), surface.clone()));
                if double {
                    let on = !self.toplevel_of(&surface).map(|t| t.current_state().states.contains(xdg_toplevel::State::Maximized)).unwrap_or(false);
                    self.set_maximized(&surface, on);
                } else {
                    let origin = self.space.element_location(&window).unwrap_or_default();
                    pointer.set_grab(self, MoveGrab { start, window, origin }, serial, Focus::Clear);
                }
            }
        }
        true
    }

    /// What is drawn this frame, front to back: panels on the top layers, the windows with their title bars, then the layers behind.
    pub fn scene(&mut self, renderer: &mut GlesRenderer) -> Vec<Elem<GlesRenderer>> {
        use smithay::backend::renderer::element::AsRenderElements;
        let mut out: Vec<Elem<GlesRenderer>> = vec![];
        let scale = Scale::from(1.0);
        let layer_els = |app: &App, renderer: &mut GlesRenderer, layers: &[Layer], out: &mut Vec<Elem<GlesRenderer>>| {
            let map = layer_map_for_output(&app.output);
            for layer in layers {
                for l in map.layers_on(*layer).rev() {
                    let Some(geo) = map.layer_geometry(l) else { continue };
                    out.extend(l.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(renderer, geo.loc.to_physical(1), scale, 1.0).into_iter().map(Elem::from));
                }
            }
        };
        layer_els(self, renderer, &[Layer::Overlay, Layer::Top], &mut out);
        let windows: Vec<Window> = self.space.elements().rev().cloned().collect();
        for w in windows {
            let Some(loc) = self.space.element_location(&w) else { continue };
            let geo = w.geometry();
            out.extend(w.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(renderer, (loc - geo.loc).to_physical(1), scale, 1.0).into_iter().map(Elem::from));
            if let (Some(r), Some(t)) = (self.bar_rect(&w), w.toplevel()) {
                let surface = t.wl_surface().clone();
                let title = with_states(&surface, |st| st.data_map.get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>().and_then(|d| d.lock().unwrap().title.clone())).unwrap_or_default();
                let key = BarKey {
                    title,
                    width: r.size.w,
                    active: self.keyboard.current_focus().map(|f| f == surface).unwrap_or(false),
                    hover: self.bar_hover.as_ref().filter(|(s, _)| s == &surface).map(|(_, h)| *h),
                    maximized: t.current_state().states.contains(xdg_toplevel::State::Maximized),
                };
                if self.bars.get(&surface).map(|b| b.key != key).unwrap_or(true) {
                    if let Some(f) = &self.fonts {
                        self.bars.insert(surface.clone(), deco::make(f, key));
                    }
                }
                if let Some(bar) = self.bars.get(&surface) {
                    if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, (r.loc.x as f64, r.loc.y as f64), &bar.buffer, None, None, None, Kind::Unspecified) {
                        out.push(Elem::from(el));
                    }
                }
            }
        }
        layer_els(self, renderer, &[Layer::Bottom, Layer::Background], &mut out);
        out
    }

    pub fn quit(&mut self) {
        self.loop_signal.stop();
    }

    /// The window was resized: lay the panels out again and resize the lock surface.
    pub fn output_resized(&mut self) {
        layer_map_for_output(&self.output).arrange();
        if let (Some(ls), Some(mode)) = (&self.lock_surface, self.output.current_mode()) {
            ls.with_pending_state(|s| s.size = Some((mode.size.w as u32, mode.size.h as u32).into()));
            ls.send_configure();
        }
    }

    pub fn send_frames(&self, t: Duration) {
        let out = self.output.clone();
        for w in self.space.elements() {
            w.send_frame(&out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
        }
        let map = layer_map_for_output(&self.output);
        for l in map.layers() {
            l.send_frame(&out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
        }
        if let Some(ls) = &self.lock_surface {
            smithay::desktop::utils::send_frames_surface_tree(ls.wl_surface(), &out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
        }
    }

    pub fn lock_elements(&self, renderer: &mut GlesRenderer) -> Vec<WaylandSurfaceRenderElement<GlesRenderer>> {
        match &self.lock_surface {
            Some(ls) => render_elements_from_surface_tree(renderer, ls.wl_surface(), (0, 0), 1.0, 1.0, Kind::Unspecified),
            None => vec![],
        }
    }

    /// The surface under a point, and where that surface's origin is on screen. Panels on the top layers first, then windows, then
    /// the layers behind them; while the session is locked, only the lock surface.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        if self.locked {
            return self.lock_surface.as_ref().map(|l| (l.wl_surface().clone(), (0.0, 0.0).into()));
        }
        let map = layer_map_for_output(&self.output);
        let on_layer = |layer: Layer| {
            let l = map.layer_under(layer, pos)?;
            let geo = map.layer_geometry(l)?;
            let (s, p) = l.surface_under(pos - geo.loc.to_f64(), WindowSurfaceType::ALL)?;
            Some((s, (p + geo.loc).to_f64()))
        };
        if let Some(x) = on_layer(Layer::Overlay).or_else(|| on_layer(Layer::Top)) {
            return Some(x);
        }
        if self.bar_at(pos).is_some() || self.edge_at(pos).is_some() {
            return None;
        }
        if let Some((window, loc)) = self.space.element_under(pos) {
            if let Some((s, p)) = window.surface_under(pos - loc.to_f64(), WindowSurfaceType::ALL) {
                return Some((s, (p + loc).to_f64()));
            }
        }
        on_layer(Layer::Bottom).or_else(|| on_layer(Layer::Background))
    }

    /// A click gives the keyboard to what is under the pointer (a window, raised, or a panel that asks for the keyboard).
    pub fn focus_under_pointer(&mut self, serial: Serial) {
        if self.locked {
            return;
        }
        let pos = self.pointer_location;
        let layer_hit = {
            let map = layer_map_for_output(&self.output);
            [Layer::Overlay, Layer::Top].into_iter().find_map(|layer| map.layer_under(layer, pos).cloned())
        };
        if let Some(l) = layer_hit {
            use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, LayerSurfaceCachedState};
            let wants = with_states(l.wl_surface(), |st| st.cached_state.get::<LayerSurfaceCachedState>().current().keyboard_interactivity != KeyboardInteractivity::None);
            if wants {
                let kb = self.keyboard.clone();
                kb.set_focus(self, Some(l.wl_surface().clone()), serial);
            }
            return;
        }
        let Some((window, _)) = self.space.element_under(pos).map(|(w, l)| (w.clone(), l)) else { return };
        self.space.raise_element(&window, true);
        let surface = window.toplevel().map(|t| t.wl_surface().clone());
        for w in self.space.elements() {
            w.set_activated(*w == window);
            if let Some(t) = w.toplevel() {
                t.send_pending_configure();
            }
        }
        let kb = self.keyboard.clone();
        kb.set_focus(self, surface, serial);
    }

    pub fn on_input(&mut self, event: InputEvent<WinitInput>) {
        if std::env::var_os("LABYRINTH_COMPOSITOR_DEBUG").is_some() {
            if let InputEvent::PointerButton { .. } = &event {
                eprintln!("button at {:?}, over {:?}", self.pointer_location, self.surface_under(self.pointer_location).map(|x| x.0.id()));
            }
        }
        self.idle_state.notify_activity(&self.seat);
        let (keyboard, pointer) = (self.keyboard.clone(), self.pointer.clone());
        match event {
            InputEvent::Keyboard { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                let state = event.state();
                let code = event.key_code();
                let taken = keyboard.input::<Action, _>(self, code, state, serial, event.time_msec(), |app, mods, handle| {
                    let sym = handle.raw_latin_sym_or_raw_current_sym().map(|s| shortcuts::lower(s.raw())).unwrap_or(0);
                    app.key_filter(u32::from(code), state, mods, sym)
                });
                if let Some(a) = taken {
                    self.queued.push(a);
                }
                for a in std::mem::take(&mut self.queued) {
                    self.run_action(a);
                }
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let size = self.output.current_mode().map(|m| m.size).unwrap_or_default();
                let pos = event.position_transformed(size.to_logical(1));
                self.pointer_location = pos;
                if std::env::var_os("LABYRINTH_COMPOSITOR_DEBUG").is_some() {
                    eprintln!("pointer {:.0},{:.0}", pos.x, pos.y);
                }
                let serial = SERIAL_COUNTER.next_serial();
                let hover = self.bar_at(pos).and_then(|(w, h)| w.toplevel().map(|t| (t.wl_surface().clone(), h)));
                if hover != self.bar_hover {
                    self.bar_hover = hover;
                }
                let under = self.surface_under(pos);
                pointer.motion(self, under, &MotionEvent { location: pos, serial, time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerButton { event } => {
                let serial = SERIAL_COUNTER.next_serial();
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() && !self.locked && self.press_on_decoration(serial, event.button_code()) {
                    return;
                }
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() {
                    self.focus_under_pointer(serial);
                }
                pointer.button(self, &ButtonEvent { button: event.button_code(), state: event.state(), serial, time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerAxis { event } => {
                let amount = |a: Axis| event.amount(a).unwrap_or_else(|| event.amount_v120(a).unwrap_or(0.0) * 15.0 / 120.0);
                let mut frame = AxisFrame::new(event.time_msec()).source(event.source());
                for a in [Axis::Horizontal, Axis::Vertical] {
                    let v = amount(a);
                    if v != 0.0 {
                        frame = frame.value(a, v);
                        if let Some(d) = event.amount_v120(a) {
                            frame = frame.v120(a, d as i32);
                        }
                    }
                    if event.source() == AxisSource::Finger && v == 0.0 {
                        frame = frame.stop(a);
                    }
                }
                pointer.axis(self, frame);
                pointer.frame(self);
            }
            _ => {}
        }
    }
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        &client.get_data::<ClientState>().unwrap().compositor_state
    }
    fn commit(&mut self, surface: &WlSurface) {
        on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(p) = get_parent(&root) {
                root = p;
            }
            if let Some(w) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface() == &root).unwrap_or(false)).cloned() {
                w.on_commit();
            }
        }
        // Panels and backgrounds: their size and place are settled on every commit, and the first commit is answered with the
        // first configure (arrange() never sends that one by itself).
        use smithay::wayland::shell::wlr_layer::LayerSurfaceData;
        let mut map = layer_map_for_output(&self.output);
        if let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned() {
            map.arrange();
            let sent = with_states(layer.wl_surface(), |st| st.data_map.get::<LayerSurfaceData>().map(|d| d.lock().unwrap().initial_configure_sent).unwrap_or(false));
            if !sent {
                layer.layer_surface().send_configure();
            }
        }
    }
}
delegate_compositor!(App);

impl BufferHandler for App {
    fn buffer_destroyed(&mut self, _: &wl_buffer::WlBuffer) {}
}
impl ShmHandler for App {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_shm!(App);

impl SeatHandler for App {
    type KeyboardFocus = WlSurface;
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    fn focus_changed(&mut self, _: &Seat<Self>, _: Option<&WlSurface>) {}
    fn cursor_image(&mut self, _: &Seat<Self>, _: CursorImageStatus) {}
}
delegate_seat!(App);

impl SelectionHandler for App {
    type SelectionUserData = ();
}
impl DataDeviceHandler for App {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
}
impl ClientDndGrabHandler for App {}
impl ServerDndGrabHandler for App {}
delegate_data_device!(App);

impl OutputHandler for App {}
delegate_output!(App);
delegate_viewporter!(App);

impl XdgShellHandler for App {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.xdg_shell_state
    }

    fn new_toplevel(&mut self, surface: ToplevelSurface) {
        // New windows open one after another, a little offset, and take the keyboard.
        let n = self.opened;
        self.opened += 1;
        let window = Window::new_wayland_window(surface.clone());
        surface.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();
        let at = (60 + 36 * (n as i32 % 8), 60 + 36 * (n as i32 % 8));
        self.space.map_element(window.clone(), at, true);
        if self.decorate_all {
            self.decorated.push(surface.wl_surface().clone());
        }
        self.foreign_added(&window);
        let serial = SERIAL_COUNTER.next_serial();
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(surface.wl_surface().clone()), serial);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.foreign_removed(surface.wl_surface());
        self.decorated.retain(|s| s != surface.wl_surface());
        self.bars.remove(surface.wl_surface());
        self.minimized.retain(|w| w.toplevel().map(|t| t != &surface).unwrap_or(true));
        let gone = self.space.elements().find(|w| w.toplevel().map(|t| *t == surface).unwrap_or(false)).cloned();
        if let Some(w) = gone {
            self.space.unmap_elem(&w);
        }
        // the keyboard goes to the window now on top
        let next = self.space.elements().last().and_then(|w| w.toplevel().map(|t| t.wl_surface().clone()));
        let serial = SERIAL_COUNTER.next_serial();
        let kb = self.keyboard.clone();
        kb.set_focus(self, next, serial);
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        if std::env::var_os("LABYRINTH_COMPOSITOR_DEBUG").is_some() {
            eprintln!("move request, serial {:?}, grab ok: {}", serial, self.pointer.has_grab(serial));
        }
        let pointer = self.pointer.clone();
        let (Some(start), Some(window)) = (pointer.grab_start_data().filter(|_| pointer.has_grab(serial)), self.window_of(&surface)) else { return };
        let origin = self.space.element_location(&window).unwrap_or_default();
        pointer.set_grab(self, MoveGrab { start, window, origin }, serial, Focus::Clear);
    }

    fn resize_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial, edges: ResizeEdge) {
        let pointer = self.pointer.clone();
        let (Some(start), Some(window)) = (pointer.grab_start_data().filter(|_| pointer.has_grab(serial)), self.window_of(&surface)) else { return };
        let origin = self.space.element_location(&window).unwrap_or_default();
        let size = window.geometry().size;
        pointer.set_grab(self, ResizeGrab { start, window, edges, origin, size }, serial, Focus::Clear);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.minimize_surface(surface.wl_surface());
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        self.fit_window(&surface, zone, xdg_toplevel::State::Maximized);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.restore_window(&surface);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        let size = self.output.current_mode().map(|m| m.size.to_logical(1)).unwrap_or_default();
        self.fit_window(&surface, Rectangle::new((0, 0).into(), size), xdg_toplevel::State::Fullscreen);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.restore_window(&surface);
    }

    fn new_popup(&mut self, surface: PopupSurface, positioner: PositionerState) {
        surface.with_pending_state(|s| s.geometry = positioner.get_geometry());
        let _ = self.popups.track_popup(PopupKind::Xdg(surface));
    }
    fn grab(&mut self, _: PopupSurface, _: wl_seat::WlSeat, _: Serial) {}
    fn reposition_request(&mut self, surface: PopupSurface, positioner: PositionerState, token: u32) {
        surface.with_pending_state(|s| {
            s.geometry = positioner.get_geometry();
            s.positioner = positioner;
        });
        surface.send_repositioned(token);
    }
}
delegate_xdg_shell!(App);

impl WlrLayerShellHandler for App {
    fn shell_state(&mut self) -> &mut WlrLayerShellState {
        &mut self.layer_shell_state
    }

    fn new_layer_surface(&mut self, surface: WlrLayerSurface, output: Option<WlOutput>, _layer: Layer, namespace: String) {
        let out = output.as_ref().and_then(Output::from_resource).unwrap_or_else(|| self.output.clone());
        let mut map = layer_map_for_output(&out);
        let _ = map.map_layer(&LayerSurface::new(surface, namespace));
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        let mut map = layer_map_for_output(&self.output);
        let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
        if let Some(l) = found {
            map.unmap_layer(&l);
        }
    }
}
delegate_layer_shell!(App);

impl SessionLockHandler for App {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        // From now on nothing of the session is drawn or receives input. Confirmed at once: the screen is already only the lock's.
        self.locked = true;
        confirmation.lock();
        let kb = self.keyboard.clone();
        kb.set_focus(self, None, SERIAL_COUNTER.next_serial());
        let (pointer, location) = (self.pointer.clone(), self.pointer_location);
        pointer.motion(self, None, &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time: 0 });
    }

    fn unlock(&mut self) {
        self.locked = false;
        self.lock_surface = None;
    }

    fn new_surface(&mut self, surface: LockSurface, _output: WlOutput) {
        if let Some(mode) = self.output.current_mode() {
            surface.with_pending_state(|s| s.size = Some((mode.size.w as u32, mode.size.h as u32).into()));
        }
        surface.send_configure();
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(surface.wl_surface().clone()), SERIAL_COUNTER.next_serial());
        self.lock_surface = Some(surface);
    }
}
delegate_session_lock!(App);

impl IdleNotifierHandler for App {
    fn idle_notifier_state(&mut self) -> &mut IdleNotifierState<Self> {
        &mut self.idle_state
    }
}
delegate_idle_notify!(App);

impl XdgDecorationHandler for App {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        self.use_server_side(&toplevel);
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, _mode: DecoMode) {
        // Programs that ask for a bar of their own are still given ours: the look stays the same everywhere.
        self.use_server_side(&toplevel);
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.use_server_side(&toplevel);
    }
}
delegate_xdg_decoration!(App);

impl App {
    fn use_server_side(&mut self, t: &ToplevelSurface) {
        t.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ServerSide));
        if !self.decorated.contains(t.wl_surface()) {
            self.decorated.push(t.wl_surface().clone());
        }
        t.send_pending_configure();
    }
}
