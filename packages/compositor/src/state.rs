//! The compositor's state and the protocol handlers: windows (xdg-shell), panels and backgrounds (layer-shell), the screen locker
//! (ext-session-lock), idle detection (ext-idle-notify), and the usual seat, output and clipboard parts.

use std::time::Duration;

use crate::deco::{self, Bar, Hit, Key as BarKey};
use crate::draw::Fonts;
use crate::grabs::{MoveGrab, ResizeGrab};
use crate::shortcuts::{self, Action, Binding, Mods};
use crate::switcher;
use crate::tiling::{self, Profile};
use crate::toplevels::Toplevels;
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Event, InputEvent, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
};
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::utils::on_commit_buffer_handler;
use smithay::backend::winit::WinitInput;
use smithay::delegate_xdg_decoration;
use smithay::desktop::{layer_map_for_output, LayerSurface, PopupKind, PopupManager, Space, Window, WindowSurfaceType};
use smithay::input::keyboard::{FilterResult, KeyboardHandle};
use smithay::input::pointer::Focus;
use smithay::input::pointer::{AxisFrame, ButtonEvent, CursorImageStatus, MotionEvent, PointerHandle};
use smithay::input::{Seat, SeatHandler, SeatState};
use smithay::output::Output;
use smithay::reexports::calloop::{LoopHandle, LoopSignal};
use smithay::reexports::wayland_protocols::xdg::decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode as DecoMode;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::reexports::wayland_server::backend::{ClientData, ClientId, DisconnectReason};
use smithay::reexports::wayland_server::protocol::{wl_buffer, wl_output::WlOutput, wl_seat, wl_surface::WlSurface};
use smithay::reexports::wayland_server::{Client, DisplayHandle, Resource};
use smithay::utils::Rectangle;
use smithay::utils::{IsAlive, Scale};
use smithay::utils::{Logical, Point, Serial, SERIAL_COUNTER};
use smithay::wayland::buffer::BufferHandler;
use smithay::wayland::compositor::{
    get_parent, is_sync_subsurface, with_states, CompositorClientState, CompositorHandler, CompositorState,
};
use smithay::wayland::idle_notify::{IdleNotifierHandler, IdleNotifierState};
use smithay::wayland::output::{OutputHandler, OutputManagerState};
use smithay::wayland::seat::WaylandFocus;
use smithay::wayland::selection::data_device::{ClientDndGrabHandler, DataDeviceHandler, DataDeviceState, ServerDndGrabHandler};
use smithay::wayland::selection::SelectionHandler;
use smithay::wayland::session_lock::{LockSurface, SessionLockHandler, SessionLockManagerState, SessionLocker};
use smithay::wayland::shell::wlr_layer::{Layer, LayerSurface as WlrLayerSurface, WlrLayerShellHandler, WlrLayerShellState};
use smithay::wayland::shell::xdg::decoration::{XdgDecorationHandler, XdgDecorationState};
use smithay::wayland::shell::xdg::{PopupSurface, PositionerState, ToplevelSurface, XdgShellHandler, XdgShellState};
use smithay::xwayland::X11Surface;
use std::collections::HashMap;

smithay::render_elements! {
    /// What is drawn each frame: the programs' own pictures and the title bars the compositor adds.
    pub Elem<=GlesRenderer>;
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    Bar = MemoryRenderBufferRenderElement<GlesRenderer>,
    /// a full screen X11 window, drawn with its alpha channel ignored (see xwayland.rs)
    Opaque = crate::xwayland::OpaqueElement,
    /// a window's picture with its corners rounded, its border's and shadow's pieces (look.rs)
    Clipped = crate::look::Clipped<GlesRenderer>,
    Piece = crate::look::Piece<GlesRenderer>,
    Strip = crate::look::Strip,
}

smithay::render_elements! {
    /// The same for the software renderer of the headless backend (headless.rs): no shaders, so no opaque variant.
    pub PixElem<=PixmanRenderer>;
    Surface = WaylandSurfaceRenderElement<PixmanRenderer>,
    Bar = MemoryRenderBufferRenderElement<PixmanRenderer>,
    Clipped = crate::look::Clipped<PixmanRenderer>,
    Piece = crate::look::Piece<PixmanRenderer>,
    Strip = crate::look::Strip,
}

/// How a window's picture is drawn on an output: clipped to its rectangle (physical pixels) with the corners' radius (None for
/// its menus), and stretched about its corner (the point and the scale; 1 but for an animation).
pub type Shape = (Option<(Rectangle<i32, smithay::utils::Physical>, i32)>, Point<i32, smithay::utils::Physical>, Scale<f64>);

/// A renderer the scene can be drawn with: the GPU one of the nested window (GLES) and the software one of the headless
/// backend (pixman). They differ only in how a window that must be drawn opaque is drawn.
pub trait Draw:
    smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportAll + smithay::backend::renderer::ImportMem + Sized
{
    type Elem: smithay::backend::renderer::element::RenderElement<Self>
        + From<WaylandSurfaceRenderElement<Self>>
        + From<MemoryRenderBufferRenderElement<Self>>
        + From<crate::look::Piece<Self>>
        + From<crate::look::Strip>
        + From<crate::look::Clipped<Self>>;
    /// A window's elements, clipped to its rounded rectangle; `opaque` when its alpha channel must be ignored (a full screen
    /// X11 window, a game).
    fn window_elements(
        app: &mut App,
        renderer: &mut Self,
        els: Vec<WaylandSurfaceRenderElement<Self>>,
        opaque: bool,
        shape: Shape,
    ) -> Vec<Self::Elem>;
}

impl Draw for GlesRenderer {
    type Elem = Elem;
    fn window_elements(
        app: &mut App,
        renderer: &mut Self,
        els: Vec<WaylandSurfaceRenderElement<Self>>,
        opaque: bool,
        shape: Shape,
    ) -> Vec<Elem> {
        match opaque.then(|| app.opaque_shader(renderer)).flatten() {
            // (a game: drawn as it is, neither rounded nor stretched)
            Some(program) => {
                els.into_iter().map(|inner| Elem::Opaque(crate::xwayland::OpaqueElement { inner, program: program.clone() })).collect()
            }
            None => els.into_iter().map(|e| Elem::Clipped(crate::look::Clipped::new(e, shape.0, shape.1, shape.2))).collect(),
        }
    }
}

impl Draw for PixmanRenderer {
    type Elem = PixElem;
    // no shader to ignore the alpha channel with; games do not run headless
    fn window_elements(_: &mut App, _: &mut Self, els: Vec<WaylandSurfaceRenderElement<Self>>, _: bool, shape: Shape) -> Vec<PixElem> {
        els.into_iter().map(|e| PixElem::Clipped(crate::look::Clipped::new(e, shape.0, shape.1, shape.2))).collect()
    }
}

/// An output's size in logical coordinates (its mode divided by its scale).
pub fn logical_size(o: &Output) -> smithay::utils::Size<i32, Logical> {
    let scale = o.current_scale().fractional_scale();
    o.current_mode().map(|m| o.current_transform().transform_size(m.size.to_f64().to_logical(scale).to_i32_round())).unwrap_or_default()
}

/// An output's place and size in the space, in logical coordinates.
pub fn output_rect(o: &Output) -> Rectangle<i32, Logical> {
    Rectangle::new(o.current_location(), logical_size(o))
}
use smithay::backend::input::KeyState;
use smithay::input::keyboard::ModifiersState;
use smithay::wayland::shm::{ShmHandler, ShmState};
use smithay::wayland::viewporter::ViewporterState;
use smithay::{
    delegate_compositor, delegate_data_device, delegate_idle_notify, delegate_layer_shell, delegate_output, delegate_seat,
    delegate_session_lock, delegate_shm, delegate_viewporter, delegate_xdg_shell,
};

/// How the renderer takes in a program's GPU picture (true when it could).
pub type DmabufImport = Box<dyn FnMut(&smithay::backend::allocator::dmabuf::Dmabuf) -> bool>;
/// Shows the pointer the way it should look.
pub type CursorHook = Box<dyn FnMut(&CursorImageStatus)>;
/// The switcher's picture, and the windows and the choice it was drawn for.
type SwitcherArt = ((Vec<(String, String)>, usize), MemoryRenderBuffer);

pub struct App {
    /// GPU pictures from programs (linux-dmabuf): the protocol's state, and how the renderer takes one in (set by the backend;
    /// None until then, when every import fails and programs fall back to shared memory).
    pub dmabuf_state: smithay::wayland::dmabuf::DmabufState,
    pub dmabuf_import: Option<DmabufImport>,
    /// Surfaces that keep the screen awake (idle-inhibit: a video, a game): while any is alive, idle does not start.
    pub inhibitors: Vec<WlSurface>,
    // The protocol states named with a leading _ (here and further down) are only held: each owns its global (what programs
    // see advertised) and no handler reads it back.
    pub _idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState,
    pub primary_selection_state: smithay::wayland::selection::primary_selection::PrimarySelectionState,
    pub _fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState,
    pub activation_state: smithay::wayland::xdg_activation::XdgActivationState,
    pub _cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState,
    pub _tablet_state: smithay::wayland::tablet_manager::TabletManagerState,
    /// What the pointer should look like, for the backend to show (nested: the host window's cursor).
    pub cursor_hook: Option<CursorHook>,
    /// Wakes the drawing: the screen is drawn only when something changed (a program committed a picture, input, a setting).
    ping: smithay::reexports::calloop::ping::Ping,
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
    pub _output_manager_state: OutputManagerState,
    pub _viewporter_state: ViewporterState,
    pub lock_state: SessionLockManagerState,
    pub idle_state: IdleNotifierState<Self>,
    pub seat: Seat<Self>,
    pub keyboard: KeyboardHandle<Self>,
    pub pointer: PointerHandle<Self>,
    pub pointer_location: Point<f64, Logical>,
    /// The session is locked: only the lock surfaces are shown (one per output) and they alone get input.
    pub locked: bool,
    lock_surfaces: Vec<(Output, LockSurface)>,
    /// The lock object of the locker that holds the lock (a second locker is refused while it is alive).
    lock_owner: Option<smithay::reexports::wayland_protocols::ext::session_lock::v1::server::ext_session_lock_v1::ExtSessionLockV1>,
    opened: usize,
    /// Where a window was (and how big) before it was maximized or made full screen.
    saved: Vec<(WlSurface, Rectangle<i32, Logical>)>,
    /// Windows hidden with minimize (still alive, listed to shell programs, brought back with activate).
    pub minimized: Vec<Window>,
    pub toplevels: Toplevels,
    /// Copies of the screen asked for (wlr-screencopy, screencopy.rs) and outputs switched off (outputpower.rs): headless only.
    pub screencopy: crate::screencopy::Screencopy,
    pub power: crate::outputpower::OutputPower,
    pub bindings: Vec<Binding>,
    /// A modifier key with a tap binding went down and nothing else was pressed since (its key code and what letting go does):
    /// Super on its own opens the start menu, Ctrl on its own is a tap of Find my mouse.
    tap: Option<(u32, Action)>,
    /// The key of a repeating binding held down now (its code and a number of its own: the repeat timer of an older press stops).
    repeating: Option<(u32, u64)>,
    repeat_serial: u64,
    /// Minimized windows shown over the desktop (Super+Shift+M; Hyprland's special workspace): still minimized, on show.
    peek: Vec<WlSurface>,
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
    pub _decoration_state: XdgDecorationState,
    /// Windows, most recently used first (for the switcher).
    focus_order: Vec<WlSurface>,
    /// Windows in the order they were opened (tiling follows it).
    opened_at: Vec<WlSurface>,
    /// The Alt+Tab switcher while it is open: the windows in order and which one is chosen.
    switcher: Option<(Vec<WlSurface>, usize)>,
    /// The modifiers held down to keep the switcher open (those of the Tab binding): letting go of one picks the window.
    switcher_hold: Mods,
    switcher_art: Option<SwitcherArt>,
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
    conf_times: (Option<std::time::SystemTime>, Option<std::time::SystemTime>, Option<std::time::SystemTime>),
    /// Programs always drawn opaque (opaque.conf, one app id per line: games that fill their alpha channel with junk), besides
    /// the ones recognised as games (xwayland::is_game).
    pub opaque_apps: Vec<String>,
    /// Windows on other workspaces: where they were, and which workspace they are on.
    parked: Vec<(Window, usize, Point<i32, Logical>)>,
    /// X11 windows snapped to a half of the screen (a Wayland window keeps that in its xdg state; X11 has no such state).
    x11_snapped: Vec<WlSurface>,
    /// New windows not yet placed: they are centred in the free room once their first picture says how big they are.
    unplaced: Vec<WlSurface>,
    /// The free room of each output (what the bar and the dock leave) when windows were last kept clear of them.
    clear_zones: Vec<Rectangle<i32, Logical>>,
    /// X11 programs (xwayland.rs): the window manager once XWayland is ready, its display number (DISPLAY=:n), and the X11
    /// menus and tooltips now shown (override-redirect windows: drawn where they ask, on top, never given the keyboard).
    pub xwm: Option<smithay::xwayland::X11Wm>,
    pub x11_display: Option<u32>,
    /// Reads what an X11 window asked for before it was shown (x11asked.rs).
    pub x11_reader: Option<crate::x11asked::Reader>,
    pub xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState,
    pub overlays: Vec<X11Surface>,
    pub loop_handle: LoopHandle<'static, App>,
    /// The shader that draws full screen X11 windows opaque, made the first time one is drawn (None if it failed to build).
    opaque_shader: Option<Option<smithay::backend::renderer::gles::GlesTexProgram>>,
    /// Seconds without pointer motion after which the pointer is hidden (0: never), as Yggdrasil's tools set it (yggdrasil-idle
    /// while the screen is dimmed; Hyprland's `cursor:inactive_timeout`). Kept and reported; the pointer is not drawn by this
    /// compositor yet (the nested window's host draws it), so nothing is hidden until it is.
    pub pointer_timeout: f64,
    /// The pointer's theme and size as Yggdrasil's tools set it (ygg-theme), for when the compositor draws the pointer itself.
    pub cursor_theme: Option<(String, u32)>,
    /// The keyboard layout and variant in use: the system's (keymap.rs), or one set from outside (the installer's try-out);
    /// None: xkb's default.
    pub kb_layout: Option<(String, String)>,
    /// When the system's keyboard files (keymap.rs) were changed as last seen: a change brings their layout in.
    kb_times: [Option<std::time::SystemTime>; 2],
    /// Where programs' windows were when their last one closed (places.rs: the session's window-places).
    places: std::collections::BTreeMap<String, crate::places::Rect>,
    /// Windows that opened on a screen set to tile (they tile there, as the session's `tile-on-SCREEN` rules), and those
    /// screens (~/.config/yggdrasil/tiling, Settings > Displays) with when the file was changed as last seen.
    screen_tiled: Vec<WlSurface>,
    tiling_screens: Vec<String>,
    tiling_time: Option<std::time::SystemTime>,
    /// The screens, on or off, their settings and wlr-output-management (outputs.rs). Headless only: nested has its window.
    pub heads: crate::outputs::Heads,
    /// The session's window look (look.rs): border, rounding, shadow, gaps, animations; its pictures per scale; the clock the
    /// animations run on; closing windows still shrinking away.
    pub look: crate::look::Look,
    art: Vec<(u64, crate::look::Look, std::sync::Arc<crate::look::Art>)>,
    look_times: [Option<std::time::SystemTime>; 2],
    pub clock: crate::look::Clock,
    pub closing: Vec<crate::look::Closing>,
    /// The backend takes a closing window's last picture from the screen (headless); without that there is no closing animation.
    pub pictures_of_closing: bool,
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

/// Who has the keyboard: a Wayland surface (a window, a panel, the locker), or an X11 window. An X11 window needs more than its
/// surface: the X server must also be told which of its windows has the focus, or XWayland passes the keys to none of them
/// (Smithay's X11Surface does that, then hands the keys to its surface).
#[derive(Debug, Clone, PartialEq)]
pub enum KbFocus {
    Wl(WlSurface),
    X11(X11Surface),
}

impl KbFocus {
    /// The surface that receives the keys (an X11 window's own surface).
    pub fn surface(&self) -> Option<WlSurface> {
        match self {
            KbFocus::Wl(s) => Some(s.clone()),
            KbFocus::X11(x) => x.wl_surface(),
        }
    }
}

impl IsAlive for KbFocus {
    fn alive(&self) -> bool {
        match self {
            KbFocus::Wl(s) => s.alive(),
            KbFocus::X11(x) => x.alive(),
        }
    }
}

impl WaylandFocus for KbFocus {
    fn wl_surface(&self) -> Option<std::borrow::Cow<'_, WlSurface>> {
        match self {
            KbFocus::Wl(s) => Some(std::borrow::Cow::Borrowed(s)),
            KbFocus::X11(x) => WaylandFocus::wl_surface(x),
        }
    }
}

impl smithay::input::keyboard::KeyboardTarget<App> for KbFocus {
    fn enter(&self, seat: &Seat<App>, data: &mut App, keys: Vec<smithay::input::keyboard::KeysymHandle<'_>>, serial: Serial) {
        match self {
            KbFocus::Wl(s) => smithay::input::keyboard::KeyboardTarget::enter(s, seat, data, keys, serial),
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::enter(x, seat, data, keys, serial),
        }
    }
    fn leave(&self, seat: &Seat<App>, data: &mut App, serial: Serial) {
        match self {
            KbFocus::Wl(s) => smithay::input::keyboard::KeyboardTarget::leave(s, seat, data, serial),
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::leave(x, seat, data, serial),
        }
    }
    fn key(
        &self,
        seat: &Seat<App>,
        data: &mut App,
        key: smithay::input::keyboard::KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        match self {
            KbFocus::Wl(s) => smithay::input::keyboard::KeyboardTarget::key(s, seat, data, key, state, serial, time),
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::key(x, seat, data, key, state, serial, time),
        }
    }
    fn modifiers(&self, seat: &Seat<App>, data: &mut App, modifiers: ModifiersState, serial: Serial) {
        match self {
            KbFocus::Wl(s) => smithay::input::keyboard::KeyboardTarget::modifiers(s, seat, data, modifiers, serial),
            KbFocus::X11(x) => smithay::input::keyboard::KeyboardTarget::modifiers(x, seat, data, modifiers, serial),
        }
    }
}

// Windows are either Wayland (xdg toplevel) or X11 ones (through XWayland). These few helpers answer the questions the rest of
// the compositor asks about a window whichever kind it is, so both get the same treatment.

/// The program's surface of a window. An X11 window has none until XWayland has made it (shortly after it is mapped).
pub fn surface_of(w: &Window) -> Option<WlSurface> {
    w.wl_surface().map(|s| s.into_owned())
}

/// Is this the window of that surface?
pub fn is_window_of(w: &Window, s: &WlSurface) -> bool {
    w.wl_surface().is_some_and(|x| *x == *s)
}

/// Whether a window is maximized or full screen: a Wayland window's xdg state, an X11 window's _NET_WM_STATE.
pub fn win_has(w: &Window, st: xdg_toplevel::State) -> bool {
    if let Some(t) = w.toplevel() {
        return t.current_state().states.contains(st);
    }
    match (w.x11_surface(), st) {
        (Some(x), xdg_toplevel::State::Maximized) => x.is_maximized(),
        (Some(x), xdg_toplevel::State::Fullscreen) => x.is_fullscreen(),
        _ => false,
    }
}

/// A window's title and app id (for an X11 window: its WM_CLASS class, which is what desktop files match).
pub fn title_app(w: &Window) -> (String, String) {
    match (w.x11_surface(), w.toplevel()) {
        (Some(x), _) => (x.title(), x.class()),
        (_, Some(t)) => xdg_title_app(t.wl_surface()),
        _ => Default::default(),
    }
}

fn xdg_title_app(s: &WlSurface) -> (String, String) {
    with_states(s, |st| {
        st.data_map
            .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
            .map(|d| {
                let d = d.lock().unwrap();
                (d.title.clone().unwrap_or_default(), d.app_id.clone().unwrap_or_default())
            })
            .unwrap_or_default()
    })
}

/// Steam asks at start to have its window un-maximized, and Hyprland maximized it instead: the session ignores Steam's own
/// maximize requests (`steam-no-self-maximize`; Super+Up still maximizes it). The same here.
pub fn ignores_maximize(w: &Window) -> bool {
    title_app(w).1 == "steam"
}

/// Tell an X11 window where it is and how big (X11 programs place their menus from it). Wayland windows are left alone.
pub fn x11_place(w: &Window, loc: Point<i32, Logical>, size: smithay::utils::Size<i32, Logical>) {
    if let Some(x) = w.x11_surface() {
        let _ = x.configure(Rectangle::new(loc, size));
    }
}

impl App {
    pub fn new(
        dh: &DisplayHandle,
        output: Output,
        handle: LoopHandle<'static, App>,
        loop_signal: LoopSignal,
        ping: smithay::reexports::calloop::ping::Ping,
    ) -> App {
        let mut seat_state = SeatState::new();
        let mut seat = seat_state.new_wl_seat(dh, "seat0");
        // the system's keyboard layout, as the session reads it for Hyprland (keymap.rs); xkb's default when it cannot be loaded
        let system_kb = crate::keymap::system();
        let (keyboard, kb_layout) = match seat.add_keyboard(system_kb.xkb(), shortcuts::REPEAT_DELAY, shortcuts::REPEAT_RATE) {
            Ok(k) => (k, Some((system_kb.layout.clone(), system_kb.variant.clone()))),
            Err(e) => {
                eprintln!(
                    "yggdrasil-compositor: the system's keyboard layout {system_kb:?} could not be loaded ({e:?}); using xkb's default"
                );
                (seat.add_keyboard(Default::default(), shortcuts::REPEAT_DELAY, shortcuts::REPEAT_RATE).expect("keyboard"), None)
            }
        };
        let pointer = seat.add_pointer();
        let loop_handle = handle.clone();
        App {
            ping,
            display_handle: dh.clone(),
            loop_signal,
            space: Space::default(),
            popups: PopupManager::default(),
            output,
            compositor_state: CompositorState::new::<Self>(dh),
            xdg_shell_state: XdgShellState::new::<Self>(dh),
            layer_shell_state: WlrLayerShellState::new::<Self>(dh),
            shm_state: ShmState::new::<Self>(dh, vec![]),
            dmabuf_state: smithay::wayland::dmabuf::DmabufState::new(),
            dmabuf_import: None,
            inhibitors: vec![],
            _idle_inhibit_state: smithay::wayland::idle_inhibit::IdleInhibitManagerState::new::<Self>(dh),
            primary_selection_state: smithay::wayland::selection::primary_selection::PrimarySelectionState::new::<Self>(dh),
            _fractional_scale_state: smithay::wayland::fractional_scale::FractionalScaleManagerState::new::<Self>(dh),
            activation_state: smithay::wayland::xdg_activation::XdgActivationState::new::<Self>(dh),
            _cursor_shape_state: smithay::wayland::cursor_shape::CursorShapeManagerState::new::<Self>(dh),
            _tablet_state: smithay::wayland::tablet_manager::TabletManagerState::new::<Self>(dh),
            cursor_hook: None,
            seat_state,
            data_device_state: DataDeviceState::new::<Self>(dh),
            _output_manager_state: OutputManagerState::new_with_xdg_output::<Self>(dh),
            _viewporter_state: ViewporterState::new::<Self>(dh),
            lock_state: SessionLockManagerState::new::<Self, _>(dh, |_| true),
            idle_state: IdleNotifierState::new(dh, handle),
            seat,
            keyboard,
            pointer,
            pointer_location: (0.0, 0.0).into(),
            locked: false,
            lock_surfaces: vec![],
            lock_owner: None,
            opened: 0,
            saved: vec![],
            minimized: vec![],
            toplevels: Toplevels::default(),
            screencopy: Default::default(),
            power: Default::default(),
            bindings: {
                let text = std::fs::read_to_string(shortcuts::config_path()).unwrap_or_default();
                let (b, bad) = shortcuts::load(&text);
                for l in bad {
                    eprintln!("yggdrasil-compositor: shortcuts.conf: cannot use the line: {l}");
                }
                b
            },
            tap: None,
            repeating: None,
            repeat_serial: 0,
            peek: vec![],
            taken: vec![],
            queued: vec![],
            socket: String::new(),
            fonts: Fonts::load(),
            bars: HashMap::new(),
            decorated: vec![],
            decorate_all: std::env::var("YGGDRASIL_COMPOSITOR_DECORATE").map(|v| v == "all").unwrap_or(false),
            bar_hover: None,
            last_click: None,
            _decoration_state: XdgDecorationState::new::<Self>(dh),
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
            conf_times: (None, None, None),
            opaque_apps: crate::xwayland::parse_opaque(&std::fs::read_to_string(crate::xwayland::opaque_path()).unwrap_or_default()),
            parked: vec![],
            x11_snapped: vec![],
            unplaced: vec![],
            clear_zones: vec![],
            xwm: None,
            x11_display: None,
            x11_reader: None,
            xwayland_shell_state: smithay::wayland::xwayland_shell::XWaylandShellState::new::<Self>(dh),
            overlays: vec![],
            loop_handle,
            opaque_shader: None,
            pointer_timeout: 0.0,
            cursor_theme: None,
            kb_layout,
            kb_times: crate::keymap::files().map(|f| std::fs::metadata(f).and_then(|m| m.modified()).ok()),
            places: crate::places::load(),
            screen_tiled: vec![],
            tiling_screens: yggdrasil_conf::tiling::outputs(),
            tiling_time: std::fs::metadata(yggdrasil_conf::tiling::path()).and_then(|m| m.modified()).ok(),
            heads: Default::default(),
            look: crate::look::Look::load(),
            art: vec![],
            look_times: [None, None],
            clock: Default::default(),
            closing: vec![],
            pictures_of_closing: false,
        }
    }

    /// Where the next window opens: one after another, a little offset.
    pub fn cascade(&mut self) -> Point<i32, Logical> {
        let n = self.opened;
        self.opened += 1;
        (60 + 36 * (n as i32 % 8), 60 + 36 * (n as i32 % 8)).into()
    }

    /// Every window gets the window controls (YGGDRASIL_COMPOSITOR_DECORATE=all, for testing).
    pub fn decorates_all(&self) -> bool {
        self.decorate_all
    }

    /// Give a window the window controls, or take them away.
    pub fn set_decorated(&mut self, surface: &WlSurface, on: bool) {
        if on && !self.decorated.contains(surface) {
            self.decorated.push(surface.clone());
        } else if !on {
            self.decorated.retain(|s| s != surface);
            self.bars.remove(surface);
        }
        self.wake();
    }

    pub fn x11_snapped_has(&self, surface: &WlSurface) -> bool {
        self.x11_snapped.contains(surface)
    }

    /// Is this window laid out by tiling (the workspace showing tiles, or it opened on a screen set to tile; and it is not
    /// floating, maximized or full screen)?
    pub fn is_tiled(&self, w: &Window, s: &WlSurface) -> bool {
        (self.ws_profile(self.current_ws) == Some(Profile::Tiling) || self.screen_tiled.contains(s))
            && !self.floating.contains(s)
            && !win_has(w, xdg_toplevel::State::Maximized)
            && !win_has(w, xdg_toplevel::State::Fullscreen)
    }

    pub fn window_of(&self, surface: &ToplevelSurface) -> Option<Window> {
        self.space.elements().find(|w| w.toplevel().map(|t| t == surface).unwrap_or(false)).cloned()
    }

    /// Put a window on `area` (maximized, full screen or snapped to a half), remembering where it was.
    pub fn fit_window(&mut self, window: &Window, area: Rectangle<i32, Logical>, state: xdg_toplevel::State) {
        if !self.space.elements().any(|w| w == window) {
            return;
        }
        let surface = surface_of(window);
        // (an X11 window that asks before XWayland has made its surface has no place worth remembering yet)
        if let Some(s) = &surface {
            if !self.saved.iter().any(|(x, _)| x == s) {
                let loc = self.space.element_location(window).unwrap_or_default();
                self.saved.push((s.clone(), Rectangle::new(loc, window.geometry().size)));
            }
        }
        if let Some(t) = window.toplevel() {
            t.with_pending_state(|s| {
                s.states.set(state);
                s.size = Some(area.size);
            });
            t.send_configure();
        } else if let Some(x) = window.x11_surface() {
            match state {
                xdg_toplevel::State::Maximized => {
                    let _ = x.set_maximized(true);
                }
                xdg_toplevel::State::Fullscreen => {
                    let _ = x.set_fullscreen(true);
                }
                _ => {
                    if let Some(s) = &surface {
                        if !self.x11_snapped.contains(s) {
                            self.x11_snapped.push(s.clone());
                        }
                    }
                }
            }
            let _ = x.configure(area);
        }
        self.space.map_element(window.clone(), area.loc, true);
    }

    pub fn restore_window(&mut self, window: &Window) {
        if !self.space.elements().any(|w| w == window) {
            return;
        }
        let surface = surface_of(window);
        let back = surface.as_ref().and_then(|surface| self.saved.iter().position(|(s, _)| s == surface)).map(|i| self.saved.remove(i).1);
        // as the session's keep-clear does: a window that comes back nearly as big as the free room gets a normal size, and none
        // comes back under the bar or the dock
        let back = back.map(|r| {
            let (screen, zone) = self.screen_of(r);
            let r = crate::placement::normal(r, screen, zone).unwrap_or(r);
            crate::placement::fit(r, screen, zone).unwrap_or(r)
        });
        if let Some(t) = window.toplevel() {
            t.with_pending_state(|s| {
                for st in [
                    xdg_toplevel::State::Maximized,
                    xdg_toplevel::State::Fullscreen,
                    xdg_toplevel::State::TiledLeft,
                    xdg_toplevel::State::TiledRight,
                ] {
                    s.states.unset(st);
                }
                s.size = back.map(|r| r.size);
            });
            t.send_configure();
        } else if let Some(x) = window.x11_surface() {
            let _ = x.set_maximized(false);
            let _ = x.set_fullscreen(false);
            self.x11_snapped.retain(|s| Some(s) != surface.as_ref());
            if let Some(r) = back {
                let _ = x.configure(r);
            }
        }
        if let Some(r) = back {
            self.space.map_element(window.clone(), r.loc, false);
        }
        self.retile();
    }

    /// Maximize a window to the space the panels leave free, less the session's outer gap and the border (Hyprland's maximize
    /// keeps the gaps and the bar).
    pub fn maximize(&mut self, window: &Window) {
        let zone = self.free_area(&self.window_output(window));
        let inset = self.look.gaps_out + self.look.border;
        let zone = Rectangle::new(
            (zone.loc.x + inset, zone.loc.y + inset).into(),
            ((zone.size.w - 2 * inset).max(1), (zone.size.h - 2 * inset).max(1)).into(),
        );
        self.fit_window(window, zone, xdg_toplevel::State::Maximized);
    }

    /// Put a window at a place and size of the caller's choosing (Yggdrasil's tools: window zones, yggdrasil-snap), as a plain
    /// floating window: no longer maximized, full screen or snapped, and floating on a tiling workspace. Only a window on the
    /// workspace showing can be placed.
    pub fn place_window(&mut self, window: &Window, rect: Rectangle<i32, Logical>) -> bool {
        self.wake();
        if !self.space.elements().any(|w| w == window) {
            return false;
        }
        let Some(surface) = surface_of(window) else { return false };
        if self.is_tiled(window, &surface) {
            self.floating.push(surface.clone());
        }
        self.saved.retain(|(s, _)| s != &surface);
        self.unplaced.retain(|s| s != &surface);
        if let Some(t) = window.toplevel() {
            t.with_pending_state(|s| {
                for st in [
                    xdg_toplevel::State::Maximized,
                    xdg_toplevel::State::Fullscreen,
                    xdg_toplevel::State::TiledLeft,
                    xdg_toplevel::State::TiledRight,
                ] {
                    s.states.unset(st);
                }
                s.size = Some(rect.size);
            });
            t.send_configure();
        } else if let Some(x) = window.x11_surface() {
            let _ = x.set_maximized(false);
            let _ = x.set_fullscreen(false);
            self.x11_snapped.retain(|s| s != &surface);
            let _ = x.configure(rect);
        }
        self.space.map_element(window.clone(), rect.loc, false);
        self.retile();
        true
    }

    /// The room the panels leave free on an output (what its bar and dock do not reserve), in the space's coordinates.
    pub fn free_area(&self, o: &Output) -> Rectangle<i32, Logical> {
        let mut zone = layer_map_for_output(o).non_exclusive_zone();
        zone.loc += o.current_location();
        zone
    }

    /// The screen a window is on (the one under its middle); the first one for a window not shown.
    pub fn window_output(&self, w: &Window) -> Output {
        match self.space.element_location(w) {
            Some(loc) => {
                let size = w.geometry().size;
                self.output_at(Point::from(((loc.x + size.w / 2) as f64, (loc.y + size.h / 2) as f64)))
            }
            None => self.output.clone(),
        }
    }

    /// The left or right half of a screen's free room, as yggdrasil-snap works them out: each w/2 wide, the right one ending at
    /// the room's right edge.
    fn half(&self, o: &Output, left: bool) -> Rectangle<i32, Logical> {
        let zone = self.free_area(o);
        let half = zone.size.w / 2;
        if left {
            Rectangle::new(zone.loc, (half, zone.size.h).into())
        } else {
            Rectangle::new((zone.loc.x + zone.size.w - half, zone.loc.y).into(), (half, zone.size.h).into())
        }
    }

    /// The output a window is shown on (the one under its middle), with the whole output's rectangle and its free room.
    fn screen_of(&self, r: Rectangle<i32, Logical>) -> (Rectangle<i32, Logical>, Rectangle<i32, Logical>) {
        let mid = Point::<f64, Logical>::from(((r.loc.x + r.size.w / 2) as f64, (r.loc.y + r.size.h / 2) as f64));
        let o = self.output_at(mid);
        (output_rect(&o), self.free_area(&o))
    }

    /// Floating windows on the workspace now showing (not tiled, maximized, full screen or snapped), bottom first.
    fn floating_shown(&self) -> Vec<Window> {
        use xdg_toplevel::State as S;
        self.space
            .elements()
            .filter(|w| {
                let Some(s) = surface_of(w) else { return false };
                !self.is_tiled(w, &s)
                    && !self.x11_snapped.contains(&s)
                    && ![S::Maximized, S::Fullscreen, S::TiledLeft, S::TiledRight].into_iter().any(|st| win_has(w, st))
            })
            .cloned()
            .collect()
    }

    /// Put a window at `r`, asking it for the new size when that differs.
    fn place_at(&mut self, w: &Window, r: Rectangle<i32, Logical>) {
        if let Some(t) = w.toplevel() {
            if r.size != w.geometry().size {
                t.with_pending_state(|s| s.size = Some(r.size));
                t.send_pending_configure();
            }
        } else if let Some(x) = w.x11_surface() {
            let _ = x.configure(r);
        }
        self.space.map_element(w.clone(), r.loc, false);
    }

    /// A new window's first picture (now its size is known): centred in the free room of the output it opens on, shortened to
    /// it when it asked for more (Hyprland's `float, center` and keep-clear in the session's config).
    /// A program whose window opens with none of its others open comes back where its last one was (the session's
    /// window-places: places.rs), when the middle of that spot is on a screen; kept clear of the bar and the dock as any other.
    fn place_new(&mut self, w: &Window) {
        let Some(s) = surface_of(w) else { return };
        self.unplaced.retain(|x| x != &s);
        if !self.floating_shown().contains(w) {
            return;
        }
        let zone = self.free_area(&self.window_output(w));
        let size = w.geometry().size;
        let r = self.remembered_place(w).unwrap_or_else(|| crate::placement::centred((size.w, size.h), zone));
        self.place_at(w, r);
        self.space.raise_element(w, true);
    }

    /// Where a new window goes back to (places.rs), kept clear of the bar and the dock; None: it opens as any other.
    pub fn remembered_place(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        let class = title_app(w).1;
        let p = *self.places.get(&class)?;
        if self.all_windows().iter().any(|(x, _)| x != w && title_app(x).1 == class) {
            return None;
        }
        let mid = Point::<f64, Logical>::from(((p.loc.x + p.size.w / 2) as f64, (p.loc.y + p.size.h / 2) as f64));
        if !self.space.outputs().any(|o| output_rect(o).to_f64().contains(mid)) {
            return None;
        }
        let (screen, zone) = self.screen_of(p);
        Some(crate::placement::fit(p, screen, zone).unwrap_or(p))
    }

    /// A program's window closes: when it was its last one and a plain floating window (not tiled, maximized, full screen or
    /// minimized) of at least 120 x 80, its place and size are kept for its next window, in the session's file (places.rs).
    fn remember_place(&mut self, w: &Window) {
        use xdg_toplevel::State as S;
        let Some(s) = surface_of(w) else { return };
        if self.minimized.contains(w)
            || self.peek.contains(&s)
            || self.is_tiled(w, &s)
            || win_has(w, S::Maximized)
            || win_has(w, S::Fullscreen)
        {
            return;
        }
        let class = title_app(w).1;
        if !crate::places::usable_class(&class) || self.all_windows().iter().any(|(x, _)| x != w && title_app(x).1 == class) {
            return;
        }
        let Some(loc) = self.space.element_location(w).or_else(|| self.parked.iter().find(|(x, _, _)| x == w).map(|(_, _, at)| *at)) else {
            return;
        };
        let r = Rectangle::new(loc, w.geometry().size);
        if !crate::places::worth_keeping(r) || self.places.get(&class) == Some(&r) {
            return;
        }
        self.places.insert(class, r);
        if let Err(e) = crate::places::save(&self.places) {
            eprintln!("yggdrasil-compositor: keeping a window's place: {e}");
        }
    }

    /// The bar or the dock appeared, went or changed size: floating windows that now reach under them are moved out (and
    /// shortened if they must be), as keep-clear does in the Hyprland session. Only when the free room changed.
    pub fn keep_clear(&mut self) {
        let zones: Vec<Rectangle<i32, Logical>> = self.space.outputs().map(|o| self.free_area(o)).collect();
        if self.clear_zones == zones {
            return;
        }
        self.clear_zones = zones;
        // every floating window is mapped again in order, so the stacking stays as it was
        for w in self.floating_shown() {
            let Some(loc) = self.space.element_location(&w) else { continue };
            let r = Rectangle::new(loc, w.geometry().size);
            if r.size.w <= 0 || r.size.h <= 0 {
                continue;
            }
            let (screen, zone) = self.screen_of(r);
            let n = crate::placement::fit(r, screen, zone).unwrap_or(r);
            self.place_at(&w, n);
        }
    }

    /// The keyboard goes back to the window used last on the workspace now showing (a panel's pop-up closed, or let go of it).
    pub fn focus_back(&mut self) {
        let next = self
            .focus_order
            .iter()
            .find(|s| self.space.elements().any(|w| is_window_of(w, s)))
            .cloned()
            .or_else(|| self.space.elements().rev().find_map(surface_of));
        self.give_keyboard(next, SERIAL_COUNTER.next_serial());
    }

    /// A panel's surface was committed: one that asks for the keyboard exclusively (the start menu) gets it as soon as it shows,
    /// and gives it back to the window when it hides or stops asking (Hyprland does the first; the session's config the second).
    fn layer_keyboard(&mut self, surface: &WlSurface) {
        use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, LayerSurfaceCachedState};
        if self.locked {
            return;
        }
        let (wants, layer) = with_states(surface, |st| {
            let c = *st.cached_state.get::<LayerSurfaceCachedState>().current();
            (c.keyboard_interactivity, c.layer)
        });
        let shown = smithay::backend::renderer::utils::with_renderer_surface_state(surface, |s| s.buffer().is_some()).unwrap_or(false);
        let has = self.focused_surface().as_ref() == Some(surface);
        if wants == KeyboardInteractivity::Exclusive && shown && matches!(layer, Layer::Top | Layer::Overlay) {
            if !has {
                let kb = self.keyboard.clone();
                kb.set_focus(self, Some(KbFocus::Wl(surface.clone())), SERIAL_COUNTER.next_serial());
            }
        } else if has && (!shown || wants == KeyboardInteractivity::None) {
            self.focus_back();
        }
    }

    /// Make a window full screen (the whole output).
    pub fn fullscreen(&mut self, window: &Window) {
        let r = output_rect(&self.window_output(window));
        self.fit_window(window, r, xdg_toplevel::State::Fullscreen);
    }

    /// A window has opened (an X11 one: XWayland has made its surface): list it for the panel, lay the workspace out again and
    /// give it the keyboard.
    pub fn adopt(&mut self, window: &Window, surface: &WlSurface, decorate: bool) {
        if decorate {
            self.decorated.push(surface.clone());
        }
        // opened on a screen set to tile: it tiles (the session's tile-on-SCREEN rule, decided when a window opens)
        if self.tiling_screens.contains(&self.window_output(window).name()) && !self.screen_tiled.contains(surface) {
            self.screen_tiled.push(surface.clone());
        }
        self.opened_at.push(surface.clone());
        self.foreign_added(window);
        self.retile();
        self.give_keyboard(Some(surface.clone()), SERIAL_COUNTER.next_serial());
        self.touch_focus(surface);
    }

    /// A window has closed (an X11 one: unmapped or destroyed): forget it everywhere, and the keyboard goes to the window now on top.
    /// `is` picks the window out; `surface` is None for an X11 window that went before XWayland made its surface.
    pub fn window_gone(&mut self, surface: Option<&WlSurface>, is: impl Fn(&Window) -> bool) {
        self.wake();
        if let Some((w, _)) = self.all_windows().into_iter().find(|(w, _)| is(w)) {
            self.remember_place(&w);
        }
        if let Some(s) = surface {
            self.screen_tiled.retain(|x| x != s);
            self.foreign_removed(s);
            self.decorated.retain(|x| x != s);
            self.focus_order.retain(|x| x != s);
            self.opened_at.retain(|x| x != s);
            if let Some((list, i)) = &mut self.switcher {
                list.retain(|x| x != s);
                *i = (*i).min(list.len().saturating_sub(1));
            }
            self.bars.remove(s);
            self.saved.retain(|(x, _)| x != s);
            self.x11_snapped.retain(|x| x != s);
            self.floating.retain(|x| x != s);
            self.peek.retain(|x| x != s);
        }
        self.parked.retain(|(w, _, _)| !is(w));
        self.minimized.retain(|w| !is(w));
        let gone = self.space.elements().find(|w| is(w)).cloned();
        if let Some(w) = gone {
            // shrinks and fades away from where it was last drawn (the backend takes its picture before the next frame)
            let shown = w.user_data().get::<std::cell::RefCell<crate::look::WinLook>>().and_then(|l| crate::look::shown(&l.borrow()));
            if let (true, true, Some(rect)) = (self.look.animate, self.pictures_of_closing, shown) {
                self.closing.push(crate::look::Closing {
                    id: smithay::backend::renderer::element::Id::new(),
                    rect,
                    start: self.clock.now(),
                    picture: None,
                });
            }
            self.space.unmap_elem(&w);
        }
        let next = self.space.elements().rev().find_map(surface_of);
        self.give_keyboard(next, SERIAL_COUNTER.next_serial());
        self.retile();
    }

    /// The window of a surface, wherever it is (shown, minimized or on another workspace).
    pub fn window_for(&self, surface: &WlSurface) -> Option<Window> {
        self.space
            .elements()
            .chain(self.minimized.iter())
            .chain(self.parked.iter().map(|(w, _, _)| w))
            .find(|w| is_window_of(w, surface))
            .cloned()
    }

    /// The window of a surface on the workspace now showing.
    fn shown_window(&self, surface: &WlSurface) -> Option<Window> {
        self.space.elements().find(|w| is_window_of(w, surface)).cloned()
    }

    /// Whether the window of a surface is in a state (maximized, full screen, snapped left or right).
    fn has_state(&self, surface: &WlSurface, st: xdg_toplevel::State) -> bool {
        let Some(w) = self.window_for(surface) else { return false };
        if w.x11_surface().is_some() && matches!(st, xdg_toplevel::State::TiledLeft | xdg_toplevel::State::TiledRight) {
            return self.x11_snapped.contains(surface);
        }
        win_has(&w, st)
    }

    /// Give the keyboard to a surface (None: to nobody). The surface of an X11 window gets it as that X11 window (see KbFocus).
    pub fn give_keyboard(&mut self, surface: Option<WlSurface>, serial: Serial) {
        let focus = surface.map(|s| {
            let x11 = self.window_for(&s).and_then(|w| w.x11_surface().cloned());
            match x11 {
                Some(x) => KbFocus::X11(x),
                None => KbFocus::Wl(s),
            }
        });
        let kb = self.keyboard.clone();
        kb.set_focus(self, focus, serial);
    }

    /// The surface that has the keyboard (a window's, a panel's or the locker's).
    pub fn focused_surface(&self) -> Option<WlSurface> {
        self.keyboard.current_focus().and_then(|f| f.surface())
    }

    /// Show `window` as the active one and all others as not (Wayland windows hear it with their next configure).
    pub fn mark_active(&mut self, window: &Window) {
        for w in self.space.elements() {
            w.set_activated(w == window);
            if let Some(t) = w.toplevel() {
                t.send_pending_configure();
            }
        }
        self.x11_raise(window);
    }

    /// Bring a window forward (from minimized too) and give it the keyboard.
    pub fn activate_surface(&mut self, surface: &WlSurface) {
        self.wake();
        // a window on another workspace: go there first
        if let Some(ws) = self.parked.iter().find(|(w, _, _)| is_window_of(w, surface)).map(|(_, ws, _)| *ws) {
            self.switch_workspace(ws);
        }
        if let Some(i) = self.minimized.iter().position(|w| is_window_of(w, surface)) {
            self.unminimize(i);
        }
        // brought back for good (it was only on show with the minimized ones)
        self.peek.retain(|s| s != surface);
        let Some(window) = self.shown_window(surface) else { return };
        self.space.raise_element(&window, true);
        self.mark_active(&window);
        self.give_keyboard(Some(surface.clone()), SERIAL_COUNTER.next_serial());
        self.touch_focus(surface);
    }

    /// A minimized window back on the desktop where it was (not yet raised or given the keyboard).
    fn unminimize(&mut self, i: usize) -> Window {
        let w = self.minimized.remove(i);
        let surface = surface_of(&w);
        let at = self.saved.iter().find(|(s, _)| Some(s) == surface.as_ref()).map(|(_, r)| r.loc).unwrap_or((80, 80).into());
        if let Some(x) = w.x11_surface() {
            let _ = x.set_suspended(false);
            if let Some(r) = &self.x11_reader {
                r.iconic(x.window_id(), false);
            }
        }
        x11_place(&w, at, w.geometry().size);
        self.space.map_element(w.clone(), at, true);
        self.retile();
        w
    }

    /// Super+Shift+M: the minimized windows shown over the desktop (still minimized: the dock and the window list say so), and
    /// again hidden; as Hyprland's special workspace "minimized" that the session's minimize uses. One brought forward from
    /// there (the dock, activate) stays on the desktop.
    pub fn toggle_peek(&mut self) {
        self.wake();
        if !self.peek.is_empty() {
            self.end_peek();
            self.focus_back();
            return;
        }
        let mut top = None;
        while !self.minimized.is_empty() {
            let w = self.unminimize(0);
            if let Some(s) = surface_of(&w) {
                self.peek.push(s);
            }
            top = Some(w);
        }
        if let Some(w) = top {
            self.mark_active(&w);
            let s = surface_of(&w);
            if let Some(s) = &s {
                self.touch_focus(s);
            }
            self.give_keyboard(s, SERIAL_COUNTER.next_serial());
        }
    }

    /// The minimized windows on show go back out of sight.
    fn end_peek(&mut self) {
        for s in std::mem::take(&mut self.peek) {
            self.minimize_surface(&s);
        }
    }

    pub fn close_surface(&mut self, surface: &WlSurface) {
        let Some(w) = self.window_for(surface) else { return };
        if let Some(t) = w.toplevel() {
            t.send_close();
        } else if let Some(x) = w.x11_surface() {
            // through x11asked's connection, which ends a program that cannot be asked (Smithay would leave it windowless)
            match &self.x11_reader {
                Some(r) => r.close(x.window_id()),
                None => {
                    let _ = x.close();
                }
            }
        }
    }

    pub fn minimize_surface(&mut self, surface: &WlSurface) {
        self.wake();
        self.peek.retain(|s| s != surface);
        let Some(window) = self.shown_window(surface) else { return };
        let loc = self.space.element_location(&window).unwrap_or_default();
        self.saved.retain(|(s, _)| s != surface);
        self.saved.push((surface.clone(), Rectangle::new(loc, window.geometry().size)));
        self.space.unmap_elem(&window);
        // an X11 program is told it is hidden (_NET_WM_STATE_HIDDEN), so a game may stop drawing
        if let Some(x) = window.x11_surface() {
            let _ = x.set_suspended(true);
            // and minimized in ICCCM's WM_STATE, which Wine watches (see x11asked.rs)
            if let Some(r) = &self.x11_reader {
                r.iconic(x.window_id(), true);
            }
        }
        // no longer the active window (an X11 one loses _NET_WM_STATE_FOCUSED)
        window.set_activated(false);
        if let Some(t) = window.toplevel() {
            t.send_pending_configure();
        }
        self.minimized.push(window);
        self.retile();
        // the keyboard, and the active look, go to the window now on top
        let next = self.space.elements().rev().find(|w| surface_of(w).is_some()).cloned();
        if let Some(w) = &next {
            self.mark_active(w);
        }
        self.give_keyboard(next.as_ref().and_then(surface_of), SERIAL_COUNTER.next_serial());
    }

    pub fn set_maximized(&mut self, surface: &WlSurface, on: bool) {
        let Some(w) = self.window_for(surface) else { return };
        if on {
            self.maximize(&w);
        } else {
            self.restore_window(&w);
        }
    }

    pub fn set_fullscreen(&mut self, surface: &WlSurface, on: bool) {
        let Some(w) = self.window_for(surface) else { return };
        if on {
            self.fullscreen(&w);
        } else {
            self.restore_window(&w);
        }
    }

    /// Decides what a key does: a shortcut (taken, not passed to the focused program) or nothing special.
    fn key_filter(&mut self, code: u32, state: KeyState, mods: &ModifiersState, sym: u32) -> FilterResult<Action> {
        if self.locked {
            // only the media keys' shortcuts run on the lock screen (volume and brightness repeating); every other key goes to the lock
            self.tap = None;
            if state == KeyState::Pressed {
                self.repeating = None;
            }
            if state == KeyState::Pressed && shortcuts::works_locked(sym) {
                if let Some(b) = shortcuts::find(&self.bindings, Mods::of(mods), sym) {
                    self.taken.push(code);
                    if b.repeat {
                        self.repeat_serial += 1;
                        self.repeating = Some((code, self.repeat_serial));
                    }
                    return FilterResult::Intercept(b.action.clone());
                }
            }
            if state == KeyState::Released {
                if self.repeating.is_some_and(|(c, _)| c == code) {
                    self.repeating = None;
                }
                if let Some(i) = self.taken.iter().position(|c| *c == code) {
                    self.taken.remove(i);
                    return FilterResult::Intercept(Action::None);
                }
            }
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
            if self.repeating.is_some_and(|(c, _)| c == code) {
                self.repeating = None;
            }
            if self.tap.as_ref().is_some_and(|(c, _)| *c == code) {
                if let Some((_, action)) = self.tap.take() {
                    self.queued.push(action);
                }
            }
            if let Some(i) = self.taken.iter().position(|c| *c == code) {
                self.taken.remove(i);
                return FilterResult::Intercept(Action::None);
            }
            return FilterResult::Forward;
        }
        // any other key pressed: a held key no longer repeats, a modifier held is no longer a tap
        self.repeating = None;
        self.tap = None;
        if shortcuts::is_modifier(sym) {
            // a modifier on its own does its binding when let go (Super: the start menu), and always reaches the program
            self.tap = shortcuts::find(&self.bindings, Mods::of(mods), sym).map(|b| (code, b.action.clone()));
            return FilterResult::Forward;
        }
        match shortcuts::find(&self.bindings, Mods::of(mods), sym) {
            Some(b) => {
                self.taken.push(code);
                if matches!(b.action, Action::NextWindow | Action::PrevWindow) {
                    // the switcher stays open while these modifiers (Alt for Alt+Tab) are held
                    self.switcher_hold = Mods { shift: false, ..Mods::of(mods) };
                }
                if b.repeat {
                    self.repeat_serial += 1;
                    self.repeating = Some((code, self.repeat_serial));
                }
                FilterResult::Intercept(b.action.clone())
            }
            None => FilterResult::Forward,
        }
    }

    /// A repeating binding's key was pressed: while it stays down, its action runs again after the keyboard's delay and then at
    /// its rate (the volume and brightness keys, the magnifier).
    fn start_repeat(&mut self, action: Action) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        let Some((_, serial)) = self.repeating else { return };
        let every = Duration::from_millis(1000 / shortcuts::REPEAT_RATE.max(1) as u64);
        let delay = Duration::from_millis(shortcuts::REPEAT_DELAY.max(0) as u64);
        let _ = self.loop_handle.insert_source(Timer::from_duration(delay), move |_, _, app| {
            // let go, or another key pressed meanwhile (on the lock screen too: its key handling ends a repeat the same way)
            if app.repeating.map(|(_, s)| s) != Some(serial) {
                return TimeoutAction::Drop;
            }
            app.run_action(action.clone());
            TimeoutAction::ToDuration(every)
        });
    }

    fn spawn(&self, cmd: &str) {
        // /bin/sh by its path: the commands are looked up on the compositor's PATH by the shell (the tests give one of stand-ins)
        let mut c = std::process::Command::new("/bin/sh");
        // X11 programs go to this compositor's XWayland, never to the host's X server
        c.arg("-c").arg(cmd).env_remove("DISPLAY").env_remove("HYPRLAND_INSTANCE_SIGNATURE");
        if let Some(n) = self.x11_display {
            c.env("DISPLAY", format!(":{n}"));
        }
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
            Err(e) => eprintln!("yggdrasil-compositor: cannot start {cmd}: {e}"),
        }
    }

    fn focused(&self) -> Option<WlSurface> {
        let f = self.focused_surface()?;
        self.space.elements().any(|w| is_window_of(w, &f)).then_some(f)
    }

    pub fn run_action(&mut self, action: Action) {
        self.wake();
        use xdg_toplevel::State as S;
        let current = self.focused();
        let has = |app: &App, s: &WlSurface, st: S| app.has_state(s, st);
        match action {
            Action::None => {}
            // the panel's own actions through its command (yggdrasil-panelctl), as the session binds them
            Action::ToggleStart => self.spawn("yggdrasil-panelctl start"),
            Action::Clipboard => self.spawn("yggdrasil-clipboard"),
            Action::Assistant => self.spawn("yggdrasil-panelctl ask-press"),
            Action::Calendar => self.spawn("yggdrasil-panelctl calendar"),
            Action::Fylgja(n) => self.spawn(&format!("yggdrasil-panelctl fylgja {n}")),
            Action::Exec(cmd) => self.spawn(&cmd),
            // the session's lock: logind asks the idle manager, which starts the lock screen (headless: no system bus, see headless.rs)
            Action::Lock => self.spawn("loginctl lock-session"),
            Action::Quit => self.quit(),
            Action::Close => {
                if let Some(s) = current {
                    // as the session's yggdrasil-close: a program that keeps running (keep-running) is minimized instead
                    let class = self.window_for(&s).map(|w| title_app(&w).1).unwrap_or_default();
                    if yggdrasil_conf::keep_running::keeps(&yggdrasil_conf::keep_running::load(), &class) {
                        self.minimize_surface(&s);
                    } else {
                        self.close_surface(&s);
                    }
                }
            }
            Action::Maximize => {
                if let Some(s) = current {
                    self.set_maximized(&s, !has(self, &s, S::Maximized));
                }
            }
            Action::Restore => {
                if let Some(s) = current {
                    if has(self, &s, S::Maximized)
                        || has(self, &s, S::Fullscreen)
                        || has(self, &s, S::TiledLeft)
                        || has(self, &s, S::TiledRight)
                    {
                        if let Some(w) = self.window_for(&s) {
                            self.restore_window(&w);
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
            Action::ShowMinimized => self.toggle_peek(),
            Action::SnapLeft | Action::SnapRight => {
                let Some(t) = current.and_then(|s| self.window_for(&s)) else { return };
                let left = action == Action::SnapLeft;
                let area = self.half(&self.window_output(&t), left);
                self.fit_window(&t, area, if left { S::TiledLeft } else { S::TiledRight });
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
        self.wake();
        // windows in the order they were opened, each on the screen it is on; maximized, full screen and floating ones keep out
        let mut tiled: Vec<(Window, Output)> = self
            .space
            .elements()
            .filter(|w| {
                // (an X11 window waits until XWayland has made its surface)
                let Some(s) = w.wl_surface() else { return false };
                self.is_tiled(w, &s)
            })
            .map(|w| (w.clone(), self.window_output(w)))
            .collect();
        if tiled.is_empty() {
            return;
        }
        tiled.sort_by_key(|(w, _)| w.wl_surface().and_then(|s| self.opened_at.iter().position(|x| *x == *s)).unwrap_or(usize::MAX));
        for o in self.space.outputs().cloned().collect::<Vec<_>>() {
            let here: Vec<Window> = tiled.iter().filter(|(_, x)| *x == o).map(|(w, _)| w.clone()).collect();
            if !here.is_empty() {
                self.tile_on(&o, &here);
            }
        }
    }

    /// Lay these windows out on one screen (dwindle in its free room).
    fn tile_on(&mut self, o: &Output, tiled: &[Window]) {
        let zone = self.free_area(o);
        // the session's gaps; each box holds the window and its border
        let b = self.look.border;
        let boxes = tiling::dwindle(zone, tiled.len(), self.look.gaps_in, self.look.gaps_out);
        let boxes = boxes
            .into_iter()
            .map(|r| Rectangle::new((r.loc.x + b, r.loc.y + b).into(), ((r.size.w - 2 * b).max(1), (r.size.h - 2 * b).max(1)).into()));
        for (w, r) in tiled.iter().zip(boxes) {
            if let Some(t) = w.toplevel() {
                t.with_pending_state(|s| s.size = Some(r.size));
                t.send_pending_configure();
            } else if let Some(x) = w.x11_surface() {
                let _ = x.configure(r);
            } else {
                continue;
            }
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

    /// Read workspaces.conf, shortcuts.conf and opaque.conf again when they changed (Settings writes the first).
    pub fn reload_config(&mut self) {
        self.wake();
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
        // the accent, the style and "reduce motion" (appearance.conf, panel.conf)
        let look_t = crate::look::Look::files().map(|f| f.and_then(mtime));
        if look_t != self.look_times {
            self.look_times = look_t;
            let look = crate::look::Look::load();
            if look != self.look {
                let gaps = (look.gaps_in, look.gaps_out, look.border) != (self.look.gaps_in, self.look.gaps_out, self.look.border);
                self.look = look;
                if gaps {
                    self.retile();
                }
            }
        }
        let op_t = mtime(crate::xwayland::opaque_path());
        if op_t != self.conf_times.2 {
            self.conf_times.2 = op_t;
            self.opaque_apps = crate::xwayland::parse_opaque(&std::fs::read_to_string(crate::xwayland::opaque_path()).unwrap_or_default());
        }
        // the system's keyboard layout (keymap.rs): changed with localectl or by the installer
        let kb_t = crate::keymap::files().map(mtime);
        if kb_t != self.kb_times {
            self.kb_times = kb_t;
            let l = crate::keymap::system();
            let kb = self.keyboard.clone();
            match kb.set_xkb_config(self, l.xkb()) {
                Ok(()) => self.kb_layout = Some((l.layout, l.variant)),
                Err(e) => eprintln!("yggdrasil-compositor: the system's keyboard layout {l:?} could not be loaded ({e:?})"),
            }
        }
        // the screens that tile (Settings > Displays); windows opening there from now on tile, as on Hyprland after a reload
        let tl_t = mtime(yggdrasil_conf::tiling::path());
        if tl_t != self.tiling_time {
            self.tiling_time = tl_t;
            self.tiling_screens = yggdrasil_conf::tiling::outputs();
        }
        // Settings > Displays' modes, scales and places (outputs.rs)
        self.reload_monitors();
    }

    pub fn touch_focus(&mut self, s: &WlSurface) {
        self.focus_order.retain(|x| x != s);
        self.focus_order.insert(0, s.clone());
    }

    /// Every window that exists, wherever it is (shown, minimized or on another workspace).
    pub fn all_windows(&self) -> Vec<(Window, bool)> {
        self.space
            .elements()
            .map(|w| (w.clone(), false))
            .chain(self.minimized.iter().map(|w| (w.clone(), true)))
            .chain(self.parked.iter().map(|(w, _, _)| (w.clone(), false)))
            .collect()
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
                let existing: Vec<WlSurface> = self.all_windows().iter().filter_map(|(w, _)| surface_of(w)).collect();
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
        self.wake();
        let n = n.min(WORKSPACES - 1);
        if n == self.current_ws {
            return;
        }
        // the minimized windows on show go out of sight first (they belong to no desktop)
        self.end_peek();
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
        let next = self.focus_order.iter().find(|s| self.space.elements().any(|w| is_window_of(w, s))).cloned();
        self.give_keyboard(next, SERIAL_COUNTER.next_serial());
    }

    /// Send a window to another workspace (it disappears from this one).
    pub fn move_to_workspace(&mut self, surface: &WlSurface, n: usize) {
        let n = n.min(WORKSPACES - 1);
        if n == self.current_ws {
            return;
        }
        let Some(window) = self.shown_window(surface) else { return };
        let loc = self.space.element_location(&window).unwrap_or_default();
        self.space.unmap_elem(&window);
        self.parked.push((window, n, loc));
        self.retile();
        let next = self.focus_order.iter().find(|s| *s != surface && self.space.elements().any(|w| is_window_of(w, s))).cloned();
        self.give_keyboard(next, SERIAL_COUNTER.next_serial());
    }

    fn bar_visible(&self, w: &Window) -> bool {
        let Some(s) = w.wl_surface() else { return false };
        self.decorated.contains(&s) && !win_has(w, xdg_toplevel::State::Fullscreen)
    }

    /// The window controls (the pill at the top right inside the window), in screen coordinates.
    fn bar_rect(&self, w: &Window) -> Option<Rectangle<i32, Logical>> {
        if !self.bar_visible(w) {
            return None;
        }
        let loc = self.space.element_location(w)?;
        let size = w.geometry().size;
        Some(Rectangle::new(
            (loc.x + size.w - deco::PILL_W - deco::MARGIN_X, loc.y + deco::MARGIN_Y).into(),
            (deco::PILL_W, deco::PILL_H).into(),
        ))
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
        let Some(surface) = surface_of(&window) else { return false };
        let pointer = self.pointer.clone();
        let start = smithay::input::pointer::GrabStartData { focus: None, button, location: pos };
        self.activate_surface(&surface);
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
        // an X11 menu over the controls takes the click
        if self.over_overlay(pos) {
            return false;
        }
        let pointer = self.pointer.clone();
        let start = smithay::input::pointer::GrabStartData { focus: None, button, location: pos };
        if let Some((window, edges)) = self.edge_at(pos) {
            let Some(surface) = surface_of(&window) else { return false };
            self.activate_surface(&surface);
            let origin = self.space.element_location(&window).unwrap_or_default();
            let size = window.geometry().size;
            pointer.set_grab(self, ResizeGrab { start, window, edges, origin, size }, serial, Focus::Clear);
            return true;
        }
        let Some((window, hit)) = self.bar_at(pos) else { return false };
        let Some(surface) = surface_of(&window) else { return false };
        self.activate_surface(&surface);
        match hit {
            Hit::Close => self.close_surface(&surface),
            Hit::Minimize => self.minimize_surface(&surface),
            Hit::Maximize => {
                let on = !self.has_state(&surface, xdg_toplevel::State::Maximized);
                self.set_maximized(&surface, on);
            }
            Hit::Drag => {
                let double =
                    matches!(&self.last_click, Some((t, s)) if s == &surface && t.elapsed() < std::time::Duration::from_millis(400));
                self.last_click = Some((std::time::Instant::now(), surface.clone()));
                if double {
                    let on = !self.has_state(&surface, xdg_toplevel::State::Maximized);
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
    /// One output's part: everything is placed relative to the output's own corner and drawn at its scale (the nested window has
    /// one output at 0,0 with scale 1; the headless backend may have several, side by side, at any scale). The switcher and the
    /// profile question are shown on the first output only.
    pub fn scene<R: Draw>(&mut self, renderer: &mut R, output: &Output) -> Vec<R::Elem>
    where
        R::TextureId: Send + Clone + 'static,
    {
        use smithay::backend::renderer::element::AsRenderElements;
        let mut out: Vec<R::Elem> = vec![];
        let scale = Scale::from(output.current_scale().fractional_scale());
        let origin = output.current_location();
        let primary = output == &self.output;
        let layer_els = |renderer: &mut R, layers: &[Layer], out: &mut Vec<R::Elem>| {
            let map = layer_map_for_output(output);
            for layer in layers {
                for l in map.layers_on(*layer).rev() {
                    let Some(geo) = map.layer_geometry(l) else { continue };
                    out.extend(
                        l.render_elements::<WaylandSurfaceRenderElement<R>>(renderer, geo.loc.to_physical_precise_round(scale), scale, 1.0)
                            .into_iter()
                            .map(R::Elem::from),
                    );
                }
            }
        };
        if !self.asked_at_start {
            self.asked_at_start = true;
            self.maybe_ask();
        }
        // where a picture made by the compositor (in logical coordinates of the space) goes on this output
        let at = |p: Point<i32, Logical>| -> Point<f64, smithay::utils::Physical> { (p - origin).to_f64().to_physical(scale) };
        let screen = logical_size(output);
        if primary {
            if let Some((ws, sel)) = self.chooser {
                if self.chooser_art.as_ref().map(|(k, _)| *k != (ws, sel)).unwrap_or(true) {
                    if let Some(f) = &self.fonts {
                        self.chooser_art = Some(((ws, sel), switcher::make_chooser(f, ws, sel, accent())));
                    }
                }
                if let Some((_, buf)) = &self.chooser_art {
                    let (w, h) = switcher::chooser_size();
                    let p = at(origin + Point::from(((screen.w - w) / 2, (screen.h - h) / 2)));
                    if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, p, buf, None, None, None, Kind::Unspecified) {
                        out.push(R::Elem::from(el));
                    }
                }
            }
            if let Some((list, index)) = self.switcher.clone() {
                let entries: Vec<(String, String)> =
                    list.iter().map(|s| self.window_for(s).map(|w| title_app(&w)).unwrap_or_else(|| xdg_title_app(s))).collect();
                if self.switcher_art.as_ref().map(|(k, _)| k != &(entries.clone(), index)).unwrap_or(true) {
                    if let Some(f) = &self.fonts {
                        self.switcher_art = Some(((entries.clone(), index), switcher::make(f, &entries, index, accent())));
                    }
                }
                if let Some((_, buf)) = &self.switcher_art {
                    let (w, h) = switcher::size(switcher::visible(entries.len(), index).len());
                    let p = at(origin + Point::from(((screen.w - w) / 2, (screen.h - h) / 2)));
                    if let Ok(el) = MemoryRenderBufferRenderElement::from_buffer(renderer, p, buf, None, None, None, Kind::Unspecified) {
                        out.push(R::Elem::from(el));
                    }
                }
            }
        }
        // X11 menus and tooltips, last shown in front, over everything but the switcher and the profile question
        for x in self.overlays.iter().rev() {
            let loc = x.geometry().loc - origin;
            out.extend(
                x.render_elements::<WaylandSurfaceRenderElement<R>>(renderer, loc.to_physical_precise_round(scale), scale, 1.0)
                    .into_iter()
                    .map(R::Elem::from),
            );
        }
        // A full screen window in front (a game, a video) covers the top bar and the dock: the Top layer then goes behind the
        // windows. The Overlay layer (the lock's own picture, alerts that must be seen) stays in front.
        let full_front = self.space.elements().last().is_some_and(|w| win_has(w, xdg_toplevel::State::Fullscreen));
        layer_els(renderer, if full_front { &[Layer::Overlay][..] } else { &[Layer::Overlay, Layer::Top][..] }, &mut out);
        let now = self.clock.now();
        let s = output.current_scale().fractional_scale();
        let art = crate::look::art(&mut self.art, &self.look, s);
        // closed windows shrinking away, over the windows
        self.closing.retain(|c| crate::look::closing_place(c, now).is_some());
        for c in &self.closing {
            let Some((buffer, on)) = &c.picture else { continue };
            if *on != output.name() {
                continue;
            }
            if let Some((rect, alpha)) = crate::look::closing_place(c, now) {
                let geo = rect.physical(origin, s);
                out.extend(crate::look::closing_piece(renderer, c, buffer, geo, alpha).map(R::Elem::from));
            }
        }
        let grabbed = self.pointer.is_grabbed();
        let front = self.front_window();
        let windows: Vec<Window> = self.space.elements().rev().cloned().collect();
        for w in windows {
            let Some(loc) = self.space.element_location(&w) else { continue };
            let geo = w.geometry();
            if geo.size.w <= 0 || geo.size.h <= 0 {
                // nothing drawn yet (a window waiting for its first picture)
                continue;
            }
            let full = win_has(&w, xdg_toplevel::State::Fullscreen);
            // where it should be: its place, and the size the compositor asked for while the program has not drawn it yet
            let asked = w.toplevel().and_then(|t| {
                let pending = t.with_pending_state(|p| p.size);
                pending.filter(|p| p.w > 0 && p.h > 0 && Some(*p) != t.current_state().size)
            });
            let goal = Rectangle::new(loc, asked.unwrap_or(geo.size));
            let wl = w.user_data().get_or_insert(|| std::cell::RefCell::new(crate::look::WinLook::default()));
            let (rect, alpha) = crate::look::place(&mut wl.borrow_mut(), goal, asked.is_some(), self.look.animate, grabbed, now);
            // the controls sit on top of the program's own picture, so they come first in a front-to-back list
            if let (Some(r), Some(surface)) = (self.bar_rect(&w), surface_of(&w)) {
                let key = BarKey {
                    active: self.focused_surface().as_ref() == Some(&surface),
                    hover: self.bar_hover.as_ref().filter(|(s, _)| s == &surface).map(|(_, h)| *h),
                };
                if self.bars.get(&surface).map(|b| b.key != key).unwrap_or(true) {
                    self.bars.insert(surface.clone(), deco::make(key));
                }
                if let Some(bar) = self.bars.get(&surface) {
                    if let Ok(el) =
                        MemoryRenderBufferRenderElement::from_buffer(renderer, at(r.loc), &bar.buffer, None, None, None, Kind::Unspecified)
                    {
                        out.push(R::Elem::from(el));
                    }
                }
            }
            // drawn at `rect`: its picture stretched there while an animation runs (as Hyprland does), else as it is
            let (sx, sy) = crate::look::stretch(rect, geo.size);
            let stretch = Scale::from((sx, sy));
            let corner = crate::look::RectF { x: rect.x, y: rect.y, w: 0.0, h: 0.0 }.physical(origin, s).loc;
            let draw_at: Point<i32, smithay::utils::Physical> =
                corner - Point::from(((geo.loc.x as f64 * s).round() as i32, (geo.loc.y as f64 * s).round() as i32));
            let main: Vec<WaylandSurfaceRenderElement<R>> = match w.toplevel() {
                Some(t) => {
                    // its menus (xdg popups) in front, not clipped
                    let mut menus = vec![];
                    for (popup, offset) in PopupManager::popups_for_surface(t.wl_surface()) {
                        let o = (geo.loc + offset - popup.geometry().loc).to_physical_precise_round(s);
                        menus.extend(render_elements_from_surface_tree::<R, WaylandSurfaceRenderElement<R>>(
                            renderer,
                            popup.wl_surface(),
                            draw_at + o,
                            s,
                            alpha,
                            Kind::Unspecified,
                        ));
                    }
                    out.extend(R::window_elements(self, renderer, menus, false, (None, corner, stretch)));
                    render_elements_from_surface_tree(renderer, t.wl_surface(), draw_at, s, alpha, Kind::Unspecified)
                }
                None => w.render_elements::<WaylandSurfaceRenderElement<R>>(renderer, draw_at, Scale::from(s), alpha),
            };
            let clip = crate::look::clip_of(rect, origin, s, &self.look, full);
            // the border in front of the picture, the shadow behind it; neither in full screen
            let around = (!full).then(|| {
                let active = front.as_ref().is_some_and(|f| is_window_of(&w, f));
                crate::look::deco(renderer, &art, &wl.borrow(), clip.0, active, alpha, &self.look)
            });
            let mut back = vec![];
            if let Some(d) = around {
                out.extend(d.front.into_iter().map(R::Elem::from));
                out.extend(d.strips.into_iter().map(R::Elem::from));
                back = d.back;
            }
            // a full screen X11 window, a game, or a program in opaque.conf is drawn opaque: see OPAQUE_SHADER in xwayland.rs
            let opaque = w
                .x11_surface()
                .is_some_and(|x| x.is_fullscreen() || crate::xwayland::is_game(x) || crate::xwayland::listed(x, &self.opaque_apps));
            out.extend(R::window_elements(self, renderer, main, opaque, (Some(clip), corner, stretch)));
            out.extend(back.into_iter().map(R::Elem::from));
        }
        layer_els(
            renderer,
            if full_front { &[Layer::Top, Layer::Bottom, Layer::Background][..] } else { &[Layer::Bottom, Layer::Background][..] },
            &mut out,
        );
        out
    }

    /// The window whose border shows the accent: the one with the keyboard, or, while a panel has it (the start menu), the one
    /// that had it last.
    fn front_window(&self) -> Option<WlSurface> {
        let f = self.focused_surface()?;
        if self.space.elements().any(|w| is_window_of(w, &f)) {
            return Some(f);
        }
        let on_layer = self.space.outputs().any(|o| layer_map_for_output(o).layer_for_surface(&f, WindowSurfaceType::ALL).is_some());
        on_layer.then(|| self.focus_order.iter().find(|s| self.space.elements().any(|w| is_window_of(w, s))).cloned()).flatten()
    }

    /// Something is moving: frames go on being drawn until it has settled (never while the clock is stopped).
    pub fn animating(&self) -> bool {
        self.clock.running()
            && (!self.closing.is_empty()
                || self.space.elements().any(|w| {
                    w.user_data().get::<std::cell::RefCell<crate::look::WinLook>>().is_some_and(|l| crate::look::animating(&l.borrow()))
                }))
    }

    /// The shader for opaque X11 windows, built once (the first time a full screen X11 window is drawn).
    fn opaque_shader(&mut self, renderer: &mut GlesRenderer) -> Option<smithay::backend::renderer::gles::GlesTexProgram> {
        if self.opaque_shader.is_none() {
            let built = renderer.compile_custom_texture_shader(crate::xwayland::OPAQUE_SHADER, &[]);
            if let Err(e) = &built {
                eprintln!("yggdrasil-compositor: the shader for opaque X11 windows did not build, they are drawn as they are: {e}");
            }
            self.opaque_shader = Some(built.ok());
        }
        self.opaque_shader.clone().flatten()
    }

    /// Something changed: draw the screen again soon.
    pub fn wake(&self) {
        self.ping.ping();
    }

    pub fn quit(&mut self) {
        self.loop_signal.stop();
    }

    /// An output changed size or scale (the nested window was resized, the headless backend was told to): lay the panels out
    /// again, resize the lock surfaces and tell the programs the new scale.
    pub fn output_resized(&mut self) {
        self.wake();
        for o in self.space.outputs().cloned().collect::<Vec<_>>() {
            layer_map_for_output(&o).arrange();
        }
        self.retile();
        for (o, ls) in &self.lock_surfaces {
            let size = logical_size(o);
            ls.with_pending_state(|s| s.size = Some((size.w as u32, size.h as u32).into()));
            ls.send_configure();
        }
        self.apply_scale();
    }

    /// A screen went (switched off or unplugged): out of the space, its panels told they are closed, its lock surface dropped.
    /// A panel or lock surface of it that had the keyboard gives it back (to the window used last, or to another lock surface).
    pub fn output_gone(&mut self, o: &Output) {
        let focus = self.focused_surface();
        let layers: Vec<LayerSurface> = layer_map_for_output(o).layers().cloned().collect();
        let mut had = false;
        for l in layers {
            had |= focus.as_ref() == Some(l.wl_surface());
            l.layer_surface().send_close();
            layer_map_for_output(o).unmap_layer(&l);
        }
        self.space.unmap_output(o);
        had |= self.lock_surfaces.iter().any(|(x, l)| x == o && focus.as_ref() == Some(l.wl_surface()));
        self.lock_surfaces.retain(|(x, _)| x != o);
        self.power.off.retain(|n| *n != o.name());
        if had {
            if self.locked {
                let next = self.lock_surfaces.first().map(|(_, l)| KbFocus::Wl(l.wl_surface().clone()));
                let kb = self.keyboard.clone();
                kb.set_focus(self, next, SERIAL_COUNTER.next_serial());
            } else {
                self.focus_back();
            }
        }
    }

    /// The screens changed (`before`: the name and rectangle of each that was on): windows go along with their screen, keeping
    /// their place on it, and those on a screen that went move to the first one; then each is fitted to its screen again
    /// (maximized, full screen and snapped ones to its room, floating ones kept clear of the bar and the dock). Windows on other
    /// desktops and minimized ones are moved alike.
    pub fn windows_follow_outputs(&mut self, before: &[(String, Rectangle<i32, Logical>)]) {
        use xdg_toplevel::State as S;
        for o in self.space.outputs().cloned().collect::<Vec<_>>() {
            layer_map_for_output(&o).arrange();
        }
        let now: Vec<(String, Rectangle<i32, Logical>)> = self.space.outputs().map(|o| (o.name(), output_rect(o))).collect();
        let main = self.output.name();
        let follow = |r: Rectangle<i32, Logical>| -> Option<Rectangle<i32, Logical>> {
            let mid = Point::<i32, Logical>::from((r.loc.x + r.size.w / 2, r.loc.y + r.size.h / 2));
            let (name, old) = before.iter().find(|(_, o)| o.contains(mid))?;
            let (_, new) = now.iter().find(|(n, _)| n == name).or_else(|| now.iter().find(|(n, _)| *n == main))?;
            (new != old).then(|| Rectangle::new(new.loc + (r.loc - old.loc), r.size))
        };
        for (_, r) in &mut self.saved {
            if let Some(n) = follow(*r) {
                *r = n;
            }
        }
        for (w, _, at) in &mut self.parked {
            if let Some(n) = follow(Rectangle::new(*at, w.geometry().size)) {
                *at = n.loc;
            }
        }
        for w in self.space.elements().cloned().collect::<Vec<_>>() {
            let Some(loc) = self.space.element_location(&w) else { continue };
            let Some(n) = follow(Rectangle::new(loc, w.geometry().size)) else { continue };
            self.space.map_element(w.clone(), n.loc, false);
            x11_place(&w, n.loc, n.size);
            let Some(s) = surface_of(&w) else { continue };
            let o = self.window_output(&w);
            if win_has(&w, S::Fullscreen) {
                self.fullscreen(&w);
            } else if win_has(&w, S::Maximized) {
                self.maximize(&w);
            } else if self.has_state(&s, S::TiledLeft) || self.has_state(&s, S::TiledRight) {
                let left = self.has_state(&s, S::TiledLeft);
                let area = self.half(&o, left);
                self.fit_window(&w, area, if left { S::TiledLeft } else { S::TiledRight });
            } else if !self.is_tiled(&w, &s) {
                let (screen, zone) = (output_rect(&o), self.free_area(&o));
                if let Some(f) = crate::placement::fit(n, screen, zone) {
                    self.place_at(&w, f);
                }
            }
        }
    }

    /// The output a surface is shown on (a panel's own, a window's first), else the first one.
    pub fn output_of(&self, surface: &WlSurface) -> Output {
        for o in self.space.outputs() {
            if layer_map_for_output(o).layer_for_surface(surface, WindowSurfaceType::ALL).is_some() {
                return o.clone();
            }
        }
        if let Some(w) = self.space.elements().find(|w| is_window_of(w, surface)) {
            if let Some(o) = self.space.outputs_for_element(w).into_iter().next() {
                return o;
            }
        }
        self.output.clone()
    }

    /// Every panel and window is told the scale of its output: the fractional scale (wp-fractional-scale) and the whole-number
    /// buffer scale (wl_surface version 6). Only changes are sent.
    pub fn apply_scale(&self) {
        let tell = |o: &Output, surface: &WlSurface, states: &smithay::wayland::compositor::SurfaceData| {
            let s = o.current_scale();
            smithay::wayland::fractional_scale::with_fractional_scale(states, |fs| fs.set_preferred_scale(s.fractional_scale()));
            // never a transform: the nested window's output is "flipped" only because of how OpenGL counts rows
            smithay::wayland::compositor::send_surface_state(surface, states, s.integer_scale(), smithay::utils::Transform::Normal);
        };
        for o in self.space.outputs() {
            for l in layer_map_for_output(o).layers() {
                l.with_surfaces(|surface, states| tell(o, surface, states));
            }
        }
        for w in self.space.elements() {
            let o = self.space.outputs_for_element(w).into_iter().next().unwrap_or_else(|| self.output.clone());
            w.with_surfaces(|surface, states| tell(&o, surface, states));
        }
    }

    pub fn send_frames(&self, t: Duration) {
        let primary = self.output.clone();
        for w in self.space.elements() {
            let out = self.space.outputs_for_element(w).into_iter().next().unwrap_or_else(|| primary.clone());
            w.send_frame(&out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
        }
        for out in self.space.outputs() {
            let map = layer_map_for_output(out);
            for l in map.layers() {
                l.send_frame(out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
            }
        }
        for (out, ls) in &self.lock_surfaces {
            smithay::desktop::utils::send_frames_surface_tree(ls.wl_surface(), out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
        }
        let out = primary;
        for x in &self.overlays {
            if let Some(s) = x.wl_surface() {
                smithay::desktop::utils::send_frames_surface_tree(&s, &out, t, Some(Duration::ZERO), |_, _| Some(out.clone()));
            }
        }
    }

    /// While locked: the locker's own picture for this output, and nothing else.
    pub fn lock_elements<R>(&self, renderer: &mut R, output: &Output) -> Vec<WaylandSurfaceRenderElement<R>>
    where
        R: smithay::backend::renderer::Renderer + smithay::backend::renderer::ImportAll,
        R::TextureId: Clone + 'static,
    {
        let scale = output.current_scale().fractional_scale();
        match self.lock_surfaces.iter().find(|(o, _)| o == output) {
            Some((_, ls)) => render_elements_from_surface_tree(renderer, ls.wl_surface(), (0, 0), scale, 1.0, Kind::Unspecified),
            None => vec![],
        }
    }

    /// The output a point of the space is on (the first one when it is on none).
    pub fn output_at(&self, pos: Point<f64, Logical>) -> Output {
        self.space.outputs().find(|o| output_rect(o).to_f64().contains(pos)).cloned().unwrap_or_else(|| self.output.clone())
    }

    /// The surface under a point, and where that surface's origin is on screen. Panels on the top layers first, then windows, then
    /// the layers behind them; while the session is locked, only the lock surface.
    pub fn surface_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        let output = self.output_at(pos);
        let origin = output.current_location();
        if self.locked {
            return self.lock_surfaces.iter().find(|(o, _)| o == &output).map(|(_, l)| (l.wl_surface().clone(), origin.to_f64()));
        }
        // X11 menus and tooltips are in front of everything
        if let Some(x) = self.overlay_under(pos) {
            return Some(x);
        }
        let map = layer_map_for_output(&output);
        let local = pos - origin.to_f64();
        let on_layer = |layer: Layer| {
            let l = map.layer_under(layer, local)?;
            let geo = map.layer_geometry(l)?;
            let (s, p) = l.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL)?;
            Some((s, (p + geo.loc + origin).to_f64()))
        };
        // behind a full screen window the top bar and dock are hidden, so they get no pointer either (see scene())
        let full_front = self.space.elements().last().is_some_and(|w| win_has(w, xdg_toplevel::State::Fullscreen));
        if let Some(x) = on_layer(Layer::Overlay).or_else(|| if full_front { None } else { on_layer(Layer::Top) }) {
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
        // a click in an X11 menu leaves the keyboard where it is (the menu's own window keeps it)
        if self.over_overlay(pos) {
            return;
        }
        let layer_hit = {
            let output = self.output_at(pos);
            let map = layer_map_for_output(&output);
            let local = pos - output.current_location().to_f64();
            [Layer::Overlay, Layer::Top].into_iter().find_map(|layer| map.layer_under(layer, local).cloned())
        };
        if let Some(l) = layer_hit {
            use smithay::wayland::shell::wlr_layer::{KeyboardInteractivity, LayerSurfaceCachedState};
            let wants = with_states(l.wl_surface(), |st| {
                st.cached_state.get::<LayerSurfaceCachedState>().current().keyboard_interactivity != KeyboardInteractivity::None
            });
            if wants {
                let kb = self.keyboard.clone();
                kb.set_focus(self, Some(KbFocus::Wl(l.wl_surface().clone())), serial);
            }
            return;
        }
        let Some((window, _)) = self.space.element_under(pos).map(|(w, l)| (w.clone(), l)) else { return };
        self.space.raise_element(&window, true);
        let surface = surface_of(&window);
        self.mark_active(&window);
        if let Some(s) = &surface {
            self.touch_focus(s);
        }
        self.give_keyboard(surface, serial);
    }

    /// Input from the nested window (winit). The headless backend calls the same steps (key, pointer_to, button) with input of
    /// its own, made from the control socket: never from a real device.
    pub fn on_input(&mut self, event: InputEvent<WinitInput>) {
        match event {
            InputEvent::Keyboard { event } => self.key(event.key_code(), event.state(), event.time_msec()),
            InputEvent::PointerMotionAbsolute { event } => {
                let size = self.output.current_mode().map(|m| m.size).unwrap_or_default();
                let pos = event.position_transformed(size.to_logical(1));
                self.pointer_to(pos, event.time_msec());
            }
            InputEvent::PointerButton { event } => self.button(event.button_code(), event.state(), event.time_msec()),
            InputEvent::PointerAxis { event } => {
                self.input_seen();
                let pointer = self.pointer.clone();
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

    /// Any input: draw again soon, and the user is not idle.
    fn input_seen(&mut self) {
        self.wake();
        self.idle_state.notify_activity(&self.seat);
    }

    /// A key went down or up (an xkb key code: the kernel's code + 8). Shortcuts are taken here; the rest goes to the focus.
    pub fn key(&mut self, code: smithay::input::keyboard::Keycode, state: KeyState, time: u32) {
        self.input_seen();
        let keyboard = self.keyboard.clone();
        let serial = SERIAL_COUNTER.next_serial();
        let repeating_before = self.repeating;
        let taken = keyboard.input::<Action, _>(self, code, state, serial, time, |app, mods, handle| {
            let sym = handle.raw_latin_sym_or_raw_current_sym().map(|s| shortcuts::lower(s.raw())).unwrap_or(0);
            app.key_filter(u32::from(code), state, mods, sym)
        });
        if let Some(a) = taken {
            if self.repeating.is_some() && self.repeating != repeating_before {
                self.start_repeat(a.clone());
            }
            self.queued.push(a);
        }
        for a in std::mem::take(&mut self.queued) {
            self.run_action(a);
        }
    }

    /// The pointer moved to `pos` (in the space's coordinates).
    pub fn pointer_to(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.input_seen();
        let pointer = self.pointer.clone();
        self.pointer_location = pos;
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!("pointer {:.0},{:.0}", pos.x, pos.y);
        }
        let serial = SERIAL_COUNTER.next_serial();
        let hover = self.bar_at(pos).and_then(|(w, h)| surface_of(&w).map(|s| (s, h)));
        if hover != self.bar_hover {
            self.bar_hover = hover;
        }
        let under = self.surface_under(pos);
        pointer.motion(self, under, &MotionEvent { location: pos, serial, time });
        pointer.frame(self);
    }

    /// A pointer button went down or up (a kernel button code: 0x110 is the left one).
    pub fn button(&mut self, button: u32, state: ButtonState, time: u32) {
        self.input_seen();
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!("button at {:?}, over {:?}", self.pointer_location, self.surface_under(self.pointer_location).map(|x| x.0.id()));
        }
        let pointer = self.pointer.clone();
        let serial = SERIAL_COUNTER.next_serial();
        if state == ButtonState::Pressed {
            // Super held for a Super+drag is not a tap of Super
            self.tap = None;
        }
        if state == ButtonState::Pressed
            && !pointer.is_grabbed()
            && !self.locked
            && (self.press_on_decoration(serial, button) || self.super_drag(serial, button))
        {
            return;
        }
        if state == ButtonState::Pressed && !pointer.is_grabbed() {
            self.focus_under_pointer(serial);
        }
        pointer.button(self, &ButtonEvent { button, state, serial, time });
        pointer.frame(self);
    }
}

/// What the headless backend's control socket reports (headless.rs): the windows wherever they are, the keyboard's focus, the
/// workspace and the lock.
pub struct WindowInfo {
    pub window: Window,
    /// 0-based
    pub workspace: usize,
    pub at: Point<i32, Logical>,
    pub states: Vec<&'static str>,
}

impl App {
    pub fn workspace(&self) -> usize {
        self.current_ws
    }

    /// Every window: those showing (bottom first), then minimized ones, then those on other workspaces.
    pub fn window_infos(&self) -> Vec<WindowInfo> {
        use xdg_toplevel::State as S;
        let focus = self.focused_surface();
        let states = |w: &Window, minimized: bool| {
            let s = surface_of(w);
            let mut v = vec![];
            if s.is_some() && s == focus {
                v.push("active");
            }
            v.push(match &s {
                Some(s) if self.is_tiled(w, s) => "tiled",
                _ => "floating",
            });
            for (st, name) in [(S::Maximized, "maximized"), (S::Fullscreen, "fullscreen"), (S::TiledLeft, "left"), (S::TiledRight, "right")]
            {
                if win_has(w, st) {
                    v.push(name);
                }
            }
            if minimized || s.as_ref().is_some_and(|s| self.peek.contains(s)) {
                v.push("minimized");
            }
            v
        };
        let mut out: Vec<WindowInfo> = self
            .space
            .elements()
            .map(|w| WindowInfo {
                window: w.clone(),
                workspace: self.current_ws,
                at: self.space.element_location(w).unwrap_or_default(),
                states: states(w, false),
            })
            .collect();
        for w in &self.minimized {
            let at = surface_of(w).and_then(|s| self.saved.iter().find(|(x, _)| *x == s).map(|(_, r)| r.loc)).unwrap_or_default();
            out.push(WindowInfo { window: w.clone(), workspace: self.current_ws, at, states: states(w, true) });
        }
        for (w, ws, at) in &self.parked {
            out.push(WindowInfo { window: w.clone(), workspace: *ws, at: *at, states: states(w, false) });
        }
        out
    }

    /// What has the keyboard: "window APP TITLE", "layer NAMESPACE OUTPUT", "lock OUTPUT", "surface" or "none" (tab separated).
    pub fn focus_info(&self) -> String {
        let Some(f) = self.focused_surface() else { return "none".into() };
        if let Some((o, _)) = self.lock_surfaces.iter().find(|(_, l)| l.wl_surface() == &f) {
            return format!("lock\t{}", o.name());
        }
        if let Some(w) = self.window_for(&f) {
            let (title, app_id) = title_app(&w);
            return format!("window\t{app_id}\t{title}");
        }
        for o in self.space.outputs() {
            let map = layer_map_for_output(o);
            if let Some(l) = map.layer_for_surface(&f, WindowSurfaceType::TOPLEVEL) {
                return format!("layer\t{}\t{}", l.namespace(), o.name());
            }
        }
        "surface".into()
    }

    /// The lock surfaces: their output and the size the locker drew at (None until it has).
    pub fn lock_infos(&self) -> Vec<(Output, Option<smithay::utils::Size<u32, Logical>>)> {
        self.lock_surfaces.iter().map(|(o, l)| (o.clone(), l.current_state().size)).collect()
    }
}

impl CompositorHandler for App {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.compositor_state
    }
    fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
        // XWayland is a client of its own kind (Smithay starts it with its own client data)
        if let Some(x) = client.get_data::<smithay::xwayland::XWaylandClientData>() {
            return &x.compositor_state;
        }
        &client.get_data::<ClientState>().unwrap().compositor_state
    }
    fn commit(&mut self, surface: &WlSurface) {
        self.wake();
        on_commit_buffer_handler::<Self>(surface);
        self.popups.commit(surface);
        if !is_sync_subsurface(surface) {
            let mut root = surface.clone();
            while let Some(p) = get_parent(&root) {
                root = p;
            }
            let found = self.space.elements().find(|w| is_window_of(w, &root)).cloned();
            if let Some(w) = found {
                w.on_commit();
                if self.unplaced.contains(&root) && w.geometry().size.w > 0 && w.geometry().size.h > 0 {
                    self.place_new(&w);
                }
            }
        }
        // Panels and backgrounds: their size and place are settled on every commit, and the first commit is answered with the
        // first configure (arrange() never sends that one by itself).
        use smithay::wayland::shell::wlr_layer::LayerSurfaceData;
        for o in self.space.outputs().cloned().collect::<Vec<_>>() {
            let mut map = layer_map_for_output(&o);
            if let Some(layer) = map.layer_for_surface(surface, WindowSurfaceType::TOPLEVEL).cloned() {
                map.arrange();
                let sent = with_states(layer.wl_surface(), |st| {
                    st.data_map.get::<LayerSurfaceData>().map(|d| d.lock().unwrap().initial_configure_sent).unwrap_or(false)
                });
                if !sent {
                    // the first configure, with the output's scale told before it (a program then draws its first picture sharp)
                    let s = o.current_scale();
                    layer.with_surfaces(|surface, states| {
                        smithay::wayland::fractional_scale::with_fractional_scale(states, |fs| {
                            fs.set_preferred_scale(s.fractional_scale())
                        });
                        smithay::wayland::compositor::send_surface_state(
                            surface,
                            states,
                            s.integer_scale(),
                            smithay::utils::Transform::Normal,
                        );
                    });
                    layer.layer_surface().send_configure();
                }
                drop(map);
                self.keep_clear();
                self.layer_keyboard(surface);
                break;
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

// GPU pictures (linux-dmabuf): a program renders with the GPU (Vulkan, OpenGL) into a buffer and hands it over without a copy.
// The import is checked against the renderer at once, so a buffer the GPU cannot read is refused when it is made, not when drawn.
impl smithay::wayland::dmabuf::DmabufHandler for App {
    fn dmabuf_state(&mut self) -> &mut smithay::wayland::dmabuf::DmabufState {
        &mut self.dmabuf_state
    }
    fn dmabuf_imported(
        &mut self,
        _global: &smithay::wayland::dmabuf::DmabufGlobal,
        dmabuf: smithay::backend::allocator::dmabuf::Dmabuf,
        notifier: smithay::wayland::dmabuf::ImportNotifier,
    ) {
        let ok = self.dmabuf_import.as_mut().is_some_and(|f| f(&dmabuf));
        if ok {
            let _ = notifier.successful::<App>();
        } else {
            notifier.failed();
        }
    }
}
smithay::delegate_dmabuf!(App);

// Keeping the screen awake (idle-inhibit): a video player or game asks while its surface shows; the idle timer stops while any asks.
impl smithay::wayland::idle_inhibit::IdleInhibitHandler for App {
    fn inhibit(&mut self, surface: WlSurface) {
        if !self.inhibitors.contains(&surface) {
            self.inhibitors.push(surface);
        }
        self.idle_state.set_is_inhibited(true);
    }
    fn uninhibit(&mut self, surface: WlSurface) {
        self.inhibitors.retain(|s| s != &surface);
        self.inhibitors.retain(|s| s.is_alive());
        self.idle_state.set_is_inhibited(!self.inhibitors.is_empty());
    }
}
smithay::delegate_idle_inhibit!(App);

// Middle-click paste (primary selection), beside the ordinary clipboard.
impl smithay::wayland::selection::primary_selection::PrimarySelectionHandler for App {
    fn primary_selection_state(&self) -> &smithay::wayland::selection::primary_selection::PrimarySelectionState {
        &self.primary_selection_state
    }
}
smithay::delegate_primary_selection!(App);

// Fractional scaling: programs are told the output's scale so they draw sharp at 125 % or 150 % (the nested window is always 1.0; headless outputs have the scale
// they were given). Changes later are sent by apply_scale().
impl smithay::wayland::fractional_scale::FractionalScaleHandler for App {
    fn new_fractional_scale(&mut self, surface: WlSurface) {
        let scale = self.output_of(&surface).current_scale().fractional_scale();
        smithay::wayland::compositor::with_states(&surface, |states| {
            smithay::wayland::fractional_scale::with_fractional_scale(states, |fs| fs.set_preferred_scale(scale));
        });
    }
}
smithay::delegate_fractional_scale!(App);

// Focus requests between programs (xdg-activation): a program may bring a window forward only with a token made in answer to
// the person's own input in the last 10 seconds (a click on a link, a launcher), so nothing can steal the keyboard on its own.
impl smithay::wayland::xdg_activation::XdgActivationHandler for App {
    fn activation_state(&mut self) -> &mut smithay::wayland::xdg_activation::XdgActivationState {
        &mut self.activation_state
    }
    fn request_activation(
        &mut self,
        token: smithay::wayland::xdg_activation::XdgActivationToken,
        data: smithay::wayland::xdg_activation::XdgActivationTokenData,
        surface: WlSurface,
    ) {
        let fresh = data.timestamp.elapsed() < std::time::Duration::from_secs(10);
        if fresh && data.serial.is_some() {
            self.activate_surface(&surface);
        }
        self.activation_state.remove_token(&token);
    }
}
smithay::delegate_xdg_activation!(App);

// Cursor shapes by name (cursor-shape-v1; it needs the tablet protocol's seat part, which costs nothing without a tablet).
impl smithay::wayland::tablet_manager::TabletSeatHandler for App {}
smithay::delegate_cursor_shape!(App);
smithay::delegate_tablet_manager!(App);

impl SeatHandler for App {
    type KeyboardFocus = KbFocus;
    // the pointer needs no X11 variant: XWayland reads pointer events on the window's own surface
    type PointerFocus = WlSurface;
    type TouchFocus = WlSurface;
    fn seat_state(&mut self) -> &mut SeatState<Self> {
        &mut self.seat_state
    }
    /// The program with the keyboard is the one offered the clipboard and the middle-click selection (Smithay offers them only
    /// to the focused program; without this, what one program copied reached no other, X11 or Wayland).
    fn focus_changed(&mut self, seat: &Seat<Self>, focus: Option<&KbFocus>) {
        let client = focus.and_then(|f| f.surface()).and_then(|s| self.display_handle.get_client(s.id()).ok());
        smithay::wayland::selection::data_device::set_data_device_focus(&self.display_handle, seat, client.clone());
        smithay::wayland::selection::primary_selection::set_primary_focus(&self.display_handle, seat, client);
    }
    fn cursor_image(&mut self, _: &Seat<Self>, image: CursorImageStatus) {
        if let Some(f) = self.cursor_hook.as_mut() {
            f(&image);
        }
    }
}
delegate_seat!(App);

impl SelectionHandler for App {
    type SelectionUserData = ();

    /// A Wayland program copied something (or the clipboard emptied): X11 programs are offered it too.
    fn new_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        source: Option<smithay::wayland::selection::SelectionSource>,
        _seat: Seat<Self>,
    ) {
        if let Some(wm) = self.xwm.as_mut() {
            if let Err(e) = wm.new_selection(ty, source.map(|s| s.mime_types())) {
                eprintln!("yggdrasil-compositor: X11 programs could not be offered the clipboard: {e}");
            }
        }
    }

    /// A Wayland program pastes what an X11 program copied (the X11 one is asked through the window manager).
    fn send_selection(
        &mut self,
        ty: smithay::wayland::selection::SelectionTarget,
        mime_type: String,
        fd: std::os::fd::OwnedFd,
        _seat: Seat<Self>,
        _: &(),
    ) {
        let handle = self.loop_handle.clone();
        if let Some(wm) = self.xwm.as_mut() {
            if let Err(e) = wm.send_selection(ty, mime_type, fd, handle) {
                eprintln!("yggdrasil-compositor: could not paste from an X11 program: {e}");
            }
        }
    }
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
        self.wake();
        // New windows open on the screen the pointer is on (Hyprland's focused monitor), one after another, a little offset (until
        // their first picture says how big they are: place_new), and take the keyboard.
        let at = self.output_at(self.pointer_location).current_location() + self.cascade();
        let window = Window::new_wayland_window(surface.clone());
        surface.with_pending_state(|s| {
            s.states.set(xdg_toplevel::State::Activated);
        });
        surface.send_configure();
        self.space.map_element(window.clone(), at, true);
        // centred in the free room once its first picture says how big it is (place_new)
        self.unplaced.push(surface.wl_surface().clone());
        let decorate = self.decorate_all;
        self.adopt(&window, surface.wl_surface(), decorate);
    }

    fn toplevel_destroyed(&mut self, surface: ToplevelSurface) {
        self.window_gone(Some(surface.wl_surface()), |w| w.toplevel() == Some(&surface));
    }

    fn move_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial) {
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!("move request, serial {:?}, grab ok: {}", serial, self.pointer.has_grab(serial));
        }
        let pointer = self.pointer.clone();
        let (Some(start), Some(window)) = (pointer.grab_start_data().filter(|_| pointer.has_grab(serial)), self.window_of(&surface)) else {
            return;
        };
        let origin = self.space.element_location(&window).unwrap_or_default();
        pointer.set_grab(self, MoveGrab { start, window, origin }, serial, Focus::Clear);
    }

    fn resize_request(&mut self, surface: ToplevelSurface, _seat: wl_seat::WlSeat, serial: Serial, edges: ResizeEdge) {
        let pointer = self.pointer.clone();
        let (Some(start), Some(window)) = (pointer.grab_start_data().filter(|_| pointer.has_grab(serial)), self.window_of(&surface)) else {
            return;
        };
        let origin = self.space.element_location(&window).unwrap_or_default();
        let size = window.geometry().size;
        pointer.set_grab(self, ResizeGrab { start, window, edges, origin, size }, serial, Focus::Clear);
    }

    fn minimize_request(&mut self, surface: ToplevelSurface) {
        self.minimize_surface(surface.wl_surface());
    }

    fn maximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_of(&surface) {
            if ignores_maximize(&w) {
                return;
            }
            self.maximize(&w);
        }
    }

    fn unmaximize_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_of(&surface) {
            self.restore_window(&w);
        }
    }

    fn fullscreen_request(&mut self, surface: ToplevelSurface, _output: Option<WlOutput>) {
        if let Some(w) = self.window_of(&surface) {
            self.fullscreen(&w);
        }
    }

    fn unfullscreen_request(&mut self, surface: ToplevelSurface) {
        if let Some(w) = self.window_of(&surface) {
            self.restore_window(&w);
        }
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
        self.wake();
        let out = output.as_ref().and_then(Output::from_resource).unwrap_or_else(|| self.output.clone());
        let mut map = layer_map_for_output(&out);
        let _ = map.map_layer(&LayerSurface::new(surface, namespace));
    }

    fn layer_destroyed(&mut self, surface: WlrLayerSurface) {
        self.wake();
        let had_keyboard = self.focused_surface().as_ref() == Some(surface.wl_surface());
        for o in self.space.outputs() {
            let mut map = layer_map_for_output(o);
            let found = map.layers().find(|l| l.layer_surface() == &surface).cloned();
            if let Some(l) = found {
                map.unmap_layer(&l);
            }
        }
        if had_keyboard && !self.locked {
            self.focus_back();
        }
        self.keep_clear();
    }
}
delegate_layer_shell!(App);

impl SessionLockHandler for App {
    fn lock_state(&mut self) -> &mut SessionLockManagerState {
        &mut self.lock_state
    }

    fn lock(&mut self, confirmation: SessionLocker) {
        self.wake();
        // A second locker while the first is running is refused (dropping the confirmation tells it "finished"). One whose
        // locker died may be taken over by a new one, which can then unlock: the session's allow_session_lock_restore on Hyprland.
        if self.locked && self.lock_owner.as_ref().is_some_and(|l| l.is_alive()) {
            return;
        }
        self.lock_owner = Some(confirmation.ext_session_lock().clone());
        // From now on nothing of the session is drawn or receives input. Confirmed at once: the screen is already only the lock's.
        self.locked = true;
        confirmation.lock();
        let kb = self.keyboard.clone();
        kb.set_focus(self, None, SERIAL_COUNTER.next_serial());
        let (pointer, location) = (self.pointer.clone(), self.pointer_location);
        pointer.motion(self, None, &MotionEvent { location, serial: SERIAL_COUNTER.next_serial(), time: 0 });
    }

    fn unlock(&mut self) {
        self.wake();
        self.locked = false;
        self.lock_surfaces.clear();
        self.lock_owner = None;
        // the keyboard goes back to the window used last (it was the lock's while locked)
        self.focus_back();
    }

    /// One lock surface per output, as big as it; the keyboard goes to the one on the first output.
    fn new_surface(&mut self, surface: LockSurface, output: WlOutput) {
        let output = Output::from_resource(&output).unwrap_or_else(|| self.output.clone());
        let size = logical_size(&output);
        surface.with_pending_state(|s| s.size = Some((size.w as u32, size.h as u32).into()));
        surface.send_configure();
        if output == self.output || self.lock_surfaces.is_empty() {
            let kb = self.keyboard.clone();
            kb.set_focus(self, Some(KbFocus::Wl(surface.wl_surface().clone())), SERIAL_COUNTER.next_serial());
        }
        self.lock_surfaces.retain(|(o, _)| o != &output);
        self.lock_surfaces.push((output, surface));
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

/// The accent for the switcher and chooser: the operating system's default (its branding), else Yggdrasil's.
fn accent() -> (u8, u8, u8) {
    let a = yggdrasil_conf::integrations::default_accent();
    (a[0], a[1], a[2])
}
