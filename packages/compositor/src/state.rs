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
        }
    }

    pub fn window_of(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.space.elements().find(|w| w.toplevel().map(|t| t == surface).unwrap_or(false)).cloned()
    }

    /// Put a window on `area` (maximized or full screen), remembering where it was.
    fn fit_window(&mut self, surface: &ToplevelSurface, area: Rectangle<i32, Logical>, state: xdg_toplevel::State) {
        let Some(window) = self.window_of(surface) else { return };
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

    fn restore_window(&mut self, surface: &ToplevelSurface, state: xdg_toplevel::State) {
        let Some(window) = self.window_of(surface) else { return };
        let back = self.saved.iter().position(|(s, _)| s == surface.wl_surface()).map(|i| self.saved.remove(i).1);
        surface.with_pending_state(|s| {
            s.states.unset(state);
            s.size = back.map(|r| r.size);
        });
        surface.send_configure();
        if let Some(r) = back {
            self.space.map_element(window, r.loc, false);
        }
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
                keyboard.input::<(), _>(self, event.key_code(), event.state(), serial, event.time_msec(), |_, _, _| FilterResult::Forward);
            }
            InputEvent::PointerMotionAbsolute { event } => {
                let size = self.output.current_mode().map(|m| m.size).unwrap_or_default();
                let pos = event.position_transformed(size.to_logical(1));
                self.pointer_location = pos;
                if std::env::var_os("LABYRINTH_COMPOSITOR_DEBUG").is_some() {
                    eprintln!("pointer {:.0},{:.0}", pos.x, pos.y);
                }
                let serial = SERIAL_COUNTER.next_serial();
                let under = self.surface_under(pos);
                pointer.motion(self, under, &MotionEvent { location: pos, serial, time: event.time_msec() });
                pointer.frame(self);
            }
            InputEvent::PointerButton { event } => {
                let serial = SERIAL_COUNTER.next_serial();
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
        self.space.map_element(window, at, true);
        let serial = SERIAL_COUNTER.next_serial();
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(surface.wl_surface().clone()), serial);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
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

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        self.fit_window(&surface, zone, xdg_toplevel::State::Maximized);
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        self.restore_window(&surface, xdg_toplevel::State::Maximized);
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        let size = self.output.current_mode().map(|m| m.size.to_logical(1)).unwrap_or_default();
        self.fit_window(&surface, Rectangle::new((0, 0).into(), size), xdg_toplevel::State::Fullscreen);
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        self.restore_window(&surface, xdg_toplevel::State::Fullscreen);
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
