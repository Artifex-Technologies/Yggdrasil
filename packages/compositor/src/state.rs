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
use crate::switcher;
use crate::tiling::{self, Profile};
use crate::toplevels::Toplevels;
use crate::deco::{self, Bar, Hit, Key as BarKey};
use crate::draw::Fonts;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
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
    /// Windows, most recently used first (for the switcher).
    focus_order: Vec<WlSurface>,
    /// Windows in the order they were opened (tiling follows it).
    opened_at: Vec<WlSurface>,
    /// The Alt+Tab switcher while it is open: the windows in order and which one is chosen.
    switcher: Option<(Vec<WlSurface>, usize)>,
    /// The modifiers held down to keep the switcher open (those of the Tab binding): letting go of one picks the window.
    switcher_hold: Mods,
    switcher_art: Option<((Vec<(String, String)>, usize), MemoryRenderBuffer)>,
    current_ws: usize,
    /// The workspace profiles from workspaces.conf, and what was chosen for the workspaces that ask.
    ws_conf: tiling::Conf,
    asked: [Option<Profile>; WORKSPACES],
    /// Windows the user set floating on a tiling workspace.
    floating: Vec<WlSurface>,
    /// The profile question: (workspace, chosen card), and its picture.
    chooser: Option<(usize, usize)>,
    chooser_art: Option<((usize, usize), MemoryRenderBuffer)>,
    asked_at_start: bool,
    conf_times: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// Windows on other workspaces: where they were, and which workspace they are on.
    parked: Vec<(Window, usize, Point<i32, Logical>)>,
}

/// How many workspaces there are.
pub const WORKSPACES: usize = tiling::WORKSPACES;

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
            focus_order: vec![],
            opened_at: vec![],
            switcher: None,
            switcher_hold: Mods::default(),
            switcher_art: None,
            current_ws: 0,
            ws_conf: tiling::parse(&std::fs::read_to_string(tiling::config_path()).unwrap_or_default()),
            asked: [None; WORKSPACES],
            floating: vec![],
            chooser: None,
            chooser_art: None,
            asked_at_start: false,
            conf_times: (None, None),
            parked: vec![],
        }
    }

    pub fn window_of(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.space.elements().find(|w| w.toplevel().map(|t| t == surface).unwrap_or(false)).cloned()
    }

    /// Put a window on `area` (maximized or full screen), remembering where it was.
    pub fn fit_window(&mut self, surface: &ToplevelSurface, area: Rectangle<i32, Logical>, state: xdg_toplevel::State) {
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
        self.retile();
    }

    pub fn toplevel_of(&self, surface: &WlSurface) -> Option<ToplevelSurface> {
        self.space.elements().chain(self.minimized.iter()).chain(self.parked.iter().map(|(w, _, _)| w)).find_map(|w| w.toplevel().filter(|t| t.wl_surface() == surface).cloned())
    }

    /// Bring a window forward (from minimized too) and give it the keyboard.
    pub fn activate_surface(&mut self, surface: &WlSurface) {
        // a window on another workspace: go there first
        if let Some(ws) = self.parked.iter().find(|(w, _, _)| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)).map(|(_, ws, _)| *ws) {
            self.switch_workspace(ws);
        }
        if let Some(i) = self.minimized.iter().position(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)) {
            let w = self.minimized.remove(i);
            let at = self.saved.iter().find(|(s, _)| s == surface).map(|(_, r)| r.loc).unwrap_or((80, 80).into());
            self.space.map_element(w, at, true);
            self.retile();
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
        self.touch_focus(surface);
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
        self.retile();
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
        if self.chooser.is_some() {
            // the profile question takes every key while it is open
            if state == KeyState::Pressed {
                self.chooser_key(sym);
            }
            return FilterResult::Intercept(Action::None);
        }
        if self.switcher.is_some() {
            // while the switcher is open: Tab moves on (below), Escape cancels, letting go of Alt picks
            if state == KeyState::Pressed && shortcuts::is_escape(sym) {
                self.taken.push(code);
                return FilterResult::Intercept(Action::CancelSwitcher);
            }
            if state == KeyState::Released && shortcuts::is_held_modifier(sym, self.switcher_hold) {
                self.queued.push(Action::CommitSwitcher);
            }
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
                if matches!(b.action, Action::NextWindow | Action::PrevWindow) {
                    // the switcher stays open while these modifiers (Alt for Alt+Tab) are held
                    self.switcher_hold = Mods { shift: false, ..Mods::of(mods) };
                }
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
            Action::NextWindow => self.cycle(1),
            Action::PrevWindow => self.cycle(-1),
            Action::CancelSwitcher => {
                self.switcher = None;
            }
            Action::CommitSwitcher => {
                if let Some((list, i)) = self.switcher.take() {
                    if let Some(s) = list.get(i).cloned() {
                        self.activate_surface(&s);
                    }
                }
            }
            Action::ToggleFloat => {
                if let Some(s) = current {
                    self.toggle_floating(&s);
                }
            }
            Action::Workspace(n) => self.switch_workspace(n),
            Action::WorkspaceNext => self.switch_workspace((self.current_ws + 1) % WORKSPACES),
            Action::WorkspacePrev => self.switch_workspace((self.current_ws + WORKSPACES - 1) % WORKSPACES),
            Action::MoveToWorkspace(n) => {
                if let Some(s) = current {
                    self.move_to_workspace(&s, n);
                }
            }
        }
    }

    /// The profile of a workspace: set in workspaces.conf, or chosen when it was opened; None while it still has to be asked.
    fn ws_profile(&self, ws: usize) -> Option<Profile> {
        self.ws_conf.profile_of(ws).or(self.asked[ws])
    }

    /// Lay the windows of a tiling workspace out again (does nothing on a standard one).
    pub fn retile(&mut self) {
        if self.ws_profile(self.current_ws) != Some(Profile::Tiling) {
            return;
        }
        let zone = layer_map_for_output(&self.output).non_exclusive_zone();
        // windows in the order they were opened; maximized, full screen and floating ones keep out of it
        let mut tiled: Vec<Window> = self
            .space
            .elements()
            .filter(|w| {
                let Some(t) = w.toplevel() else { return false };
                let st = t.current_state().states;
                !self.floating.contains(t.wl_surface()) && !st.contains(xdg_toplevel::State::Maximized) && !st.contains(xdg_toplevel::State::Fullscreen)
            })
            .cloned()
            .collect();
        tiled.sort_by_key(|w| w.toplevel().and_then(|t| self.opened_at.iter().position(|s| s == t.wl_surface())).unwrap_or(usize::MAX));
        for (w, r) in tiled.iter().zip(tiling::dwindle(zone, tiled.len(), 8)) {
            let Some(t) = w.toplevel().cloned() else { continue };
            t.with_pending_state(|s| s.size = Some(r.size));
            t.send_pending_configure();
            self.space.map_element(w.clone(), r.loc, false);
        }
    }

    /// Ask for the profile of the workspace now showing, when it is empty and its profile is "ask".
    pub fn maybe_ask(&mut self) {
        if self.ws_profile(self.current_ws).is_none() && self.space.elements().next().is_none() && self.chooser.is_none() {
            self.chooser = Some((self.current_ws, 0));
        }
    }

    /// A key while the profile question is open.
    fn chooser_key(&mut self, sym: u32) {
        use smithay::input::keyboard::keysyms as k;
        let Some((ws, sel)) = self.chooser else { return };
        match sym {
            k::KEY_Left | k::KEY_Up | k::KEY_1 => self.chooser = Some((ws, 0)),
            k::KEY_Right | k::KEY_Down | k::KEY_Tab | k::KEY_2 => self.chooser = Some((ws, 1)),
            k::KEY_Return | k::KEY_KP_Enter => {
                self.asked[ws] = Some(if sel == 1 { Profile::Tiling } else { Profile::Standard });
                self.chooser = None;
                self.retile();
            }
            k::KEY_Escape => {
                self.asked[ws] = Some(Profile::Standard);
                self.chooser = None;
            }
            _ => {}
        }
    }

    /// Float or tile one window on a tiling workspace (Super+T).
    pub fn toggle_floating(&mut self, surface: &WlSurface) {
        if let Some(i) = self.floating.iter().position(|s| s == surface) {
            self.floating.remove(i);
        } else {
            self.floating.push(surface.clone());
        }
        self.retile();
    }

    /// Read workspaces.conf and shortcuts.conf again when they changed (Settings writes the first).
    pub fn reload_config(&mut self) {
        let mtime = |p: std::path::PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        let (ws_t, sc_t) = (mtime(tiling::config_path()), mtime(shortcuts::config_path()));
        if ws_t != self.conf_times.0 {
            self.conf_times.0 = ws_t;
            self.ws_conf = tiling::parse(&std::fs::read_to_string(tiling::config_path()).unwrap_or_default());
            self.retile();
            self.maybe_ask();
        }
        if sc_t != self.conf_times.1 {
            self.conf_times.1 = sc_t;
            let (b, _) = shortcuts::load(&std::fs::read_to_string(shortcuts::config_path()).unwrap_or_default());
            self.bindings = b;
        }
    }

    pub fn touch_focus(&mut self, s: &WlSurface) {
        self.focus_order.retain(|x| x != s);
        self.focus_order.insert(0, s.clone());
    }

    /// Every window that exists, wherever it is (shown, minimized or on another workspace).
    pub fn all_windows(&self) -> Vec<(Window, bool)> {
        self.space.elements().map(|w| (w.clone(), false)).chain(self.minimized.iter().map(|w| (w.clone(), true))).chain(self.parked.iter().map(|(w, _, _)| (w.clone(), false))).collect()
    }

    /// Alt+Tab: open the switcher (or move on in it). The window used last comes first.
    fn cycle(&mut self, dir: i32) {
        match &mut self.switcher {
            Some((list, i)) => {
                if !list.is_empty() {
                    *i = (*i as i32 + dir).rem_euclid(list.len() as i32) as usize;
                }
            }
            None => {
                let existing: Vec<WlSurface> = self.all_windows().iter().filter_map(|(w, _)| w.toplevel().map(|t| t.wl_surface().clone())).collect();
                let mut list: Vec<WlSurface> = self.focus_order.iter().filter(|s| existing.contains(s)).cloned().collect();
                for s in existing {
                    if !list.contains(&s) {
                        list.push(s);
                    }
                }
                if list.len() < 2 {
                    return;
                }
                let i = if dir > 0 { 1 } else { list.len() - 1 };
                self.switcher = Some((list, i));
            }
        }
    }

    /// Show another workspace: the windows of this one are put aside, those of the other come back.
    pub fn switch_workspace(&mut self, n: usize) {
        let n = n.min(WORKSPACES - 1);
        if n == self.current_ws {
            return;
        }
        let here = self.current_ws;
        let shown: Vec<Window> = self.space.elements().cloned().collect();
        for w in shown {
            let loc = self.space.element_location(&w).unwrap_or_default();
            self.space.unmap_elem(&w);
            self.parked.push((w, here, loc));
        }
        let mut still = vec![];
        for (w, ws, loc) in std::mem::take(&mut self.parked) {
            if ws == n {
                self.space.map_element(w, loc, false);
            } else {
                still.push((w, ws, loc));
            }
        }
        self.parked = still;
        self.current_ws = n;
        // an empty workspace asks again (when its profile is "ask")
        if self.space.elements().next().is_none() {
            self.asked[n] = None;
        }
        self.retile();
        self.maybe_ask();
        let next = self.focus_order.iter().find(|s| self.space.elements().any(|w| w.toplevel().map(|t| t.wl_surface() == *s).unwrap_or(false))).cloned();
        let kb = self.keyboard.clone();
        kb.set_focus(self, next, SERIAL_COUNTER.next_serial());
    }

    /// Send a window to another workspace (it disappears from this one).
    pub fn move_to_workspace(&mut self, surface: &WlSurface, n: usize) {
        let n = n.min(WORKSPACES - 1);
        if n == self.current_ws {
            return;
        }
        let Some(window) = self.space.elements().find(|w| w.toplevel().map(|t| t.wl_surface() == surface).unwrap_or(false)).cloned() else { return };
        let loc = self.space.element_location(&window).unwrap_or_default();
        self.space.unmap_elem(&window);
        self.parked.push((window, n, loc));
        self.retile();
        let next = self.focus_order.iter().find(|s| *s != surface && self.space.elements().any(|w| w.toplevel().map(|t| t.wl_surface() == *s).unwrap_or(false))).cloned();
        let kb = self.keyboard.clone();
        kb.set_focus(self, next, SERIAL_COUNTER.next_serial());
    }

    fn bar_visible(&self, w: &Window) -> bool {
        let Some(t) = w.toplevel() else { return false };
        self.decorated.contains(t.wl_surface()) && !t.current_state().states.contains(xdg_toplevel::State::Fullscreen)
    }

    /// The window controls (the pill at the top right inside the window), in screen coordinates.
    fn bar_rect(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.bar_visible(w) {
            return None;
        }
        let loc = self.space.element_location(w)?;
        let size = w.geometry().size;
        Some(Rectangle::new((loc.x + size.w - deco::PILL_W - deco::MARGIN_X, loc.y + deco::MARGIN_Y).into(), (deco::PILL_W, deco::PILL_H).into()))
    }

    /// The window controls under the point (looking from the top window down; a window in front hides the controls of one behind it).
    fn bar_at(&self, pos: Point<f64, Logical>) -> Option<(Window, Hit)> {
        for w in self.space.elements().rev() {
            if let Some(r) = self.bar_rect(w) {
                if r.to_f64().contains(pos) {
                    return Some((w.clone(), deco::hit(pos.x - r.loc.x as f64)));
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

    /// The resize grips just outside a decorated window (all four sides).
    fn edge_at(&self, pos: Point<f64, Logical>) -> Option<(Window, ResizeEdge)> {
        for w in self.space.elements().rev() {
            if !self.bar_visible(w) {
                continue;
            }
            let loc = self.space.element_location(w)?;
            let size = w.geometry().size;
            let inner = Rectangle::new(loc, size).to_f64();
            let g = deco::GRIP as f64;
            let outer = Rectangle::new((inner.loc.x - g, inner.loc.y - g).into(), (inner.size.w + 2.0 * g, inner.size.h + 2.0 * g).into());
            if inner.contains(pos) {
                return None;
            }
            if outer.contains(pos) {
                let (left, right) = (pos.x < inner.loc.x, pos.x >= inner.loc.x + inner.size.w);
                let (top, bottom) = (pos.y < inner.loc.y, pos.y >= inner.loc.y + inner.size.h);
                let edge = match (left, right, top, bottom) {
                    (true, _, true, _) => ResizeEdge::TopLeft,
                    (_, true, true, _) => ResizeEdge::TopRight,
                    (true, _, _, true) => ResizeEdge::BottomLeft,
                    (_, true, _, true) => ResizeEdge::BottomRight,
                    (true, ..) => ResizeEdge::Left,
                    (_, true, ..) => ResizeEdge::Right,
                    (_, _, true, _) => ResizeEdge::Top,
                    (_, _, _, true) => ResizeEdge::Bottom,
                    _ => return None,
                };
                return Some((w.clone(), edge));
            }
        }
        None
    }

    /// Super and the left button move the window under the pointer, Super and the right button resize it (from the nearest corner).
    fn super_drag(&mut self, serial: Serial, button: u32) -> bool {
        if !self.keyboard.modifier_state().logo || !(button == 0x110 || button == 0x111) {
            return false;
        }
        let pos = self.pointer_location;
        let Some((window, loc)) = self.space.element_under(pos).map(|(w, l)| (w.clone(), l)) else { return false };
        let Some(t) = window.toplevel() else { return false };
        let pointer = self.pointer.clone();
        let start = smithay::input::pointer::GrabStartData { focus: None, button, location: pos };
        self.activate_surface(&t.wl_surface().clone());
        let size = window.geometry().size;
        if button == 0x110 {
            pointer.set_grab(self, MoveGrab { start, window, origin: loc }, serial, Focus::Clear);
        } else {
            let (right, bottom) = (pos.x - loc.x as f64 > size.w as f64 / 2.0, pos.y - loc.y as f64 > size.h as f64 / 2.0);
            let edges = match (right, bottom) {
                (true, true) => ResizeEdge::BottomRight,
                (true, false) => ResizeEdge::TopRight,
                (false, true) => ResizeEdge::BottomLeft,
                (false, false) => ResizeEdge::TopLeft,
            };
            pointer.set_grab(self, ResizeGrab { start, window, edges, origin: loc, size }, serial, Focus::Clear);
        }
        true
    }

    /// A click on the window controls or a resize grip. Returns true when the compositor took it (the program under it never sees it).
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
        if !self.asked_at_start {
            self.asked_at_start = true;
            self.maybe_ask();
        }
        if let Some((ws, sel)) = self.chooser {
            if self.chooser_art.as_ref().map(|(k, _)| *k != (ws, sel)).unwrap_or(true) {
                if let Some(f) = &self.fonts {
                    self.chooser_art = Some(((ws, sel), switcher::make_chooser(f, ws, sel, (124, 84, 214))));
                }
            }
            if let (Some((_, buf)), Some(mode)) = (&self.chooser_art, self.output.current_mode()) {
                let (w, h) = switcher::chooser_size();
                let at = ((mode.size.w - w) as f64 / 2.0, (mode.size.h - h) as f64 / 2.0);
                if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, buf, None, None, None, Kind::Unspecified) {
                    out.push(Elem::from(el));
                }
            }
        }
        if let Some((list, index)) = self.switcher.clone() {
            let entries: Vec<(String, String)> = list.iter().map(|s| with_states(s, |st| st.data_map.get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>().map(|d| { let d = d.lock().unwrap(); (d.title.clone().unwrap_or_default(), d.app_id.clone().unwrap_or_default()) }).unwrap_or_default())).collect();
            if self.switcher_art.as_ref().map(|(k, _)| k != &(entries.clone(), index)).unwrap_or(true) {
                if let Some(f) = &self.fonts {
                    self.switcher_art = Some(((entries.clone(), index), switcher::make(f, &entries, index, (124, 84, 214))));
                }
            }
            if let (Some((_, buf)), Some(mode)) = (&self.switcher_art, self.output.current_mode()) {
                let (w, h) = switcher::size(switcher::visible(entries.len(), index).len());
                let at = ((mode.size.w - w) as f64 / 2.0, (mode.size.h - h) as f64 / 2.0);
                if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, at, buf, None, None, None, Kind::Unspecified) {
                    out.push(Elem::from(el));
                }
            }
        }
        layer_els(self, renderer, &[Layer::Overlay, Layer::Top], &mut out);
        let windows: Vec<Window> = self.space.elements().rev().cloned().collect();
        for w in windows {
            let Some(loc) = self.space.element_location(&w) else { continue };
            let geo = w.geometry();
            // the controls sit on top of the program's own picture, so they come first in a front-to-back list
            if let (Some(r), Some(t)) = (self.bar_rect(&w), w.toplevel()) {
                let surface = t.wl_surface().clone();
                let key = BarKey { active: self.keyboard.current_focus().map(|f| f == surface).unwrap_or(false), hover: self.bar_hover.as_ref().filter(|(s, _)| s == &surface).map(|(_, h)| *h) };
                if self.bars.get(&surface).map(|b| b.key != key).unwrap_or(true) {
                    self.bars.insert(surface.clone(), deco::make(key));
                }
                if let Some(bar) = self.bars.get(&surface) {
                    if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, (r.loc.x as f64, r.loc.y as f64), &bar.buffer, None, None, None, Kind::Unspecified) {
                        out.push(Elem::from(el));
                    }
                }
            }
            out.extend(w.render_elements::<WaylandSurfaceRenderElement<GlesRenderer>>(renderer, (loc - geo.loc).to_physical(1), scale, 1.0).into_iter().map(Elem::from));
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
        self.retile();
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
        if let Some(s) = &surface {
            self.touch_focus(s);
        }
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
                if event.state() == ButtonState::Pressed && !pointer.is_grabbed() && !self.locked && (self.press_on_decoration(serial, event.button_code()) || self.super_drag(serial, event.button_code())) {
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
        self.opened_at.push(surface.wl_surface().clone());
        self.foreign_added(&window);
        self.retile();
        let serial = SERIAL_COUNTER.next_serial();
        let kb = self.keyboard.clone();
        kb.set_focus(self, Some(surface.wl_surface().clone()), serial);
        self.touch_focus(surface.wl_surface());
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.foreign_removed(surface.wl_surface());
        self.decorated.retain(|s| s != surface.wl_surface());
        self.focus_order.retain(|s| s != surface.wl_surface());
        self.opened_at.retain(|s| s != surface.wl_surface());
        self.parked.retain(|(w, _, _)| w.toplevel().map(|t| t != &surface).unwrap_or(true));
        if let Some((list, i)) = &mut self.switcher {
            list.retain(|s| s != surface.wl_surface());
            *i = (*i).min(list.len().saturating_sub(1));
        }
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
        self.floating.retain(|s| s != surface.wl_surface());
        self.retile();
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
        // until the program says what it wants, a bar is the safe answer (a program that draws its own says so at once)
        self.use_server_side(&toplevel);
    }
    fn request_mode(&mut self, toplevel: ToplevelSurface, mode: DecoMode) {
        // What the program asks for is what it gets: Settings, Minos and the Marketplace ask for client-side (they draw their own
        // bar) and must not get a second one; Qt programs ask for server-side and get ours.
        match mode {
            DecoMode::ClientSide => self.use_client_side(&toplevel),
            _ => self.use_server_side(&toplevel),
        }
    }
    fn unset_mode(&mut self, toplevel: ToplevelSurface) {
        self.use_server_side(&toplevel);
    }
}
delegate_xdg_decoration!(App);

impl App {
    fn use_client_side(&mut self, t: &ToplevelSurface) {
        t.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ClientSide));
        self.decorated.retain(|s| s != t.wl_surface());
        self.bars.remove(t.wl_surface());
        t.send_pending_configure();
        self.retile();
    }

    fn use_server_side(&mut self, t: &ToplevelSurface) {
        t.with_pending_state(|s| s.decoration_mode = Some(DecoMode::ServerSide));
        if !self.decorated.contains(t.wl_surface()) {
            self.decorated.push(t.wl_surface().clone());
        }
        t.send_pending_configure();
    }
}
