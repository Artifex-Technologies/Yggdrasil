//! X11 programs (Steam's client, games through Proton or Wine, older programs) through XWayland: an X server that is itself a
//! client of this compositor. XWayland gives each X11 window a Wayland surface; the window manager here (Smithay's X11Wm) hears
//! about the windows and the compositor treats them like Wayland windows: the same placement, tiling, window controls, window
//! list and keyboard rules. Menus and tooltips (override-redirect windows) are drawn where they ask, on top, without focus.

use std::os::fd::OwnedFd;
use std::process::Stdio;

use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElement, UnderlyingStorage};
use smithay::backend::renderer::gles::{GlesError, GlesFrame, GlesRenderer, GlesTexProgram};
use smithay::backend::renderer::utils::{CommitCounter, DamageSet, OpaqueRegions};
use smithay::desktop::Window;
use smithay::input::pointer::Focus;
use smithay::reexports::calloop::LoopHandle;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Buffer as BufferCoords, Logical, Physical, Point, Rectangle, Scale, Transform, SERIAL_COUNTER};
use smithay::wayland::selection::data_device::{
    clear_data_device_selection, current_data_device_selection_userdata, request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::{
    clear_primary_selection, current_primary_selection_userdata, request_primary_client_selection, set_primary_selection,
};
use smithay::wayland::selection::SelectionTarget;
use smithay::wayland::xwayland_shell::{XWaylandShellHandler, XWaylandShellState};
use smithay::xwayland::xwm::{Reorder, ResizeEdge as X11Edge, WmWindowProperty, XwmId};
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandEvent, XwmHandler};

use crate::grabs::{MoveGrab, ResizeGrab};
use crate::state::{self, App, KbFocus};

/// Start XWayland. It answers a moment later; then its window manager starts, X11 programs can be started (DISPLAY is known)
/// and `then` runs, also when XWayland failed (it starts the --run programs, so an X11 one finds its window manager waiting).
pub fn start(app: &mut App, handle: LoopHandle<'static, App>, then: Box<dyn FnOnce(&mut App)>) -> Result<(), Box<dyn std::error::Error>> {
    // XWayland's own messages are rarely useful; YGGDRASIL_COMPOSITOR_DEBUG shows them
    let out = || if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() { Stdio::inherit() } else { Stdio::null() };
    let (xwayland, client) =
        XWayland::spawn(&app.display_handle, None, std::iter::empty::<(String, String)>(), true, out(), out(), |_| ())?;
    let mut then = Some(then);
    let wm_handle = handle.clone();
    handle
        .insert_source(xwayland, move |event, _, app| {
            match event {
                XWaylandEvent::Ready { x11_socket, display_number } => match X11Wm::start_wm(wm_handle.clone(), x11_socket, client.clone())
                {
                    Ok(mut wm) => {
                        // games ask for the primary monitor to know where to go full screen
                        let _ = wm.set_randr_primary_output(Some(&app.output));
                        // Smithay 0.7 marks that request's number to be ignored, and every X11 event carries the number of the
                        // window manager's last request: until it makes another, every event (a program's MapRequest too) was
                        // dropped and no X11 window ever showed. Any request with an answer moves the number on.
                        let _ = wm.get_randr_primary_output();
                        app.xwm = Some(wm);
                        app.x11_display = Some(display_number);
                        app.x11_reader = crate::x11asked::Reader::start(display_number, &wm_handle);
                        println!("DISPLAY=:{display_number}");
                    }
                    Err(e) => eprintln!("yggdrasil-compositor: the X11 window manager could not start, X11 programs will not show: {e}"),
                },
                XWaylandEvent::Error => eprintln!("yggdrasil-compositor: XWayland stopped while starting; X11 programs will not run"),
            }
            if let Some(f) = then.take() {
                f(app);
            }
        })
        .map_err(|e| e.error)?;
    Ok(())
}

/// A menu, tooltip, notification or splash screen that still goes through the window manager: shown like an override-redirect
/// one (where it asks, on top, without the keyboard, not in the window list).
fn is_helper(x: &X11Surface) -> bool {
    use smithay::xwayland::xwm::WmWindowType as T;
    matches!(x.window_type(), Some(T::Menu | T::DropdownMenu | T::PopupMenu | T::Tooltip | T::Notification | T::Splash))
}

/// The same X11 window? (compared by id: an X11Surface stops comparing equal to itself once its window is destroyed)
fn same(a: &X11Surface, b: &X11Surface) -> bool {
    a.xwm_id() == b.xwm_id() && a.window_id() == b.window_id()
}

/// `want` made to fit in `zone`: no bigger than it, and moved so that all of it is on it.
pub fn fit_in(zone: Rectangle<i32, Logical>, want: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
    let w = want.size.w.min(zone.size.w).max(1);
    let h = want.size.h.min(zone.size.h).max(1);
    let x = want.loc.x.min(zone.loc.x + zone.size.w - w).max(zone.loc.x);
    let y = want.loc.y.min(zone.loc.y + zone.size.h - h).max(zone.loc.y);
    Rectangle::new((x, y).into(), (w, h).into())
}

/// A game, recognised by its X11 class: a Steam game (`steam_app_<id>`) or a Windows program through Wine or Proton (`<name>.exe`).
/// Games are drawn opaque whatever their alpha channel holds.
pub fn is_game(x: &X11Surface) -> bool {
    is_game_class(&x.class()) || is_game_class(&x.instance())
}

pub fn is_game_class(c: &str) -> bool {
    let c = c.to_ascii_lowercase();
    c.starts_with("steam_app_") || c.ends_with(".exe")
}

/// `~/.config/yggdrasil/opaque.conf`: programs always drawn opaque, one app id (X11 class) per line; # starts a comment.
pub fn opaque_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("opaque.conf")
}

pub fn parse_opaque(text: &str) -> Vec<String> {
    text.lines().map(|l| l.split('#').next().unwrap_or("").trim().to_ascii_lowercase()).filter(|l| !l.is_empty()).collect()
}

/// Is this window's program in opaque.conf?
pub fn listed(x: &X11Surface, apps: &[String]) -> bool {
    let (c, i) = (x.class().to_ascii_lowercase(), x.instance().to_ascii_lowercase());
    apps.iter().any(|a| *a == c || *a == i)
}

impl App {
    /// What an X11 window asked for before it was shown (x11asked.rs), now that the answer is in: full screen or maximized,
    /// unless it already is, or the person has placed it since.
    pub fn x11_asked(&mut self, a: crate::x11asked::Asked) {
        let Some(w) = self.all_windows().into_iter().map(|(w, _)| w).find(|w| w.x11_surface().is_some_and(|x| x.window_id() == a.window))
        else {
            return;
        };
        let Some(x) = w.x11_surface() else { return };
        if a.fullscreen && !x.is_fullscreen() {
            self.fullscreen(&w);
        } else if a.maximized && !a.fullscreen && !x.is_maximized() && !self.placed_by_layout(&w) {
            self.maximize(&w);
        }
        self.wake();
    }

    /// The window (shown, minimized or on another workspace) of an X11 window, if the compositor manages it.
    fn x11_window(&self, x: &X11Surface) -> Option<Window> {
        self.all_windows().into_iter().map(|(w, _)| w).find(|w| w.x11_surface().is_some_and(|s| same(s, x)))
    }

    /// Put an X11 window on top in the X server's stacking order too (X11 programs read it, e.g. to place their menus).
    pub fn x11_raise(&mut self, window: &Window) {
        if let (Some(x), Some(wm)) = (window.x11_surface(), self.xwm.as_mut()) {
            let _ = wm.raise_window(x);
        }
    }

    /// Is the point over an X11 menu or tooltip? A click there is the menu's, not the window's behind it.
    pub fn over_overlay(&self, pos: Point<f64, Logical>) -> bool {
        self.overlays.iter().any(|x| x.geometry().to_f64().contains(pos))
    }

    /// The surface of an X11 menu or tooltip under the point, and where that surface's origin is on screen.
    pub fn overlay_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        for x in self.overlays.iter().rev() {
            let Some(s) = x.wl_surface() else { continue };
            let loc = x.geometry().loc;
            if let Some((s, p)) =
                smithay::desktop::utils::under_from_surface_tree(&s, pos - loc.to_f64(), (0, 0), smithay::desktop::WindowSurfaceType::ALL)
            {
                return Some((s, (p + loc).to_f64()));
            }
        }
        None
    }

    /// Did the compositor give this window its place and size (tiled, maximized, full screen, snapped)? Then it keeps them.
    fn placed_by_layout(&self, w: &Window) -> bool {
        use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::State as S;
        let Some(s) = state::surface_of(w) else { return false };
        state::win_has(w, S::Maximized) || state::win_has(w, S::Fullscreen) || self.x11_snapped_has(&s) || self.is_tiled(w, &s)
    }
}

impl XWaylandShellHandler for App {
    fn xwayland_shell_state(&mut self) -> &mut XWaylandShellState {
        &mut self.xwayland_shell_state
    }

    /// XWayland has made the surface of an X11 window: from now on it is a window like any other (listed for the panel, tiled,
    /// given the keyboard). Menus and tooltips are not windows here; they are drawn from `overlays`.
    fn surface_associated(&mut self, _xwm: XwmId, surface: WlSurface, window: X11Surface) {
        self.wake();
        let Some(w) = self.x11_window(&window) else { return };
        // window controls for an X11 window that does not draw its own (Steam and most games say they draw their own, or none)
        let decorate = self.decorates_all() || !window.is_decorated();
        self.adopt(&w, &surface, decorate);
        self.mark_active(&w);
    }
}
smithay::delegate_xwayland_shell!(App);

impl XwmHandler for App {
    fn xwm_state(&mut self, _xwm: XwmId) -> &mut X11Wm {
        self.xwm.as_mut().expect("an X11 window manager event without the window manager")
    }

    fn new_window(&mut self, _xwm: XwmId, _window: X11Surface) {}
    fn new_override_redirect_window(&mut self, _xwm: XwmId, _window: X11Surface) {}

    /// A program shows a window: it is placed like a Wayland window (one after another, a little offset; tiled on a tiling
    /// workspace once its surface exists).
    fn map_window_request(&mut self, _xwm: XwmId, window: X11Surface) {
        self.wake();
        if window.set_mapped(true).is_err() {
            return;
        }
        let geo = window.geometry();
        if is_helper(&window) {
            let _ = window.configure(geo);
            if !self.overlays.iter().any(|x| same(x, &window)) {
                self.overlays.push(window);
            }
            return;
        }
        // never bigger than the space the panels leave free, and wholly on it: Steam asks for 1373x1080 on a 1280x800 screen, and a
        // question it centres on that window landed off the edge, where nobody could answer it (found 2026-11-05)
        let zone = smithay::desktop::layer_map_for_output(&self.output).non_exclusive_zone();
        let screen = crate::state::logical_size(&self.output);
        let game_full = is_game(&window) && screen.w > 0 && geo.size.w >= screen.w && geo.size.h >= screen.h;
        let place =
            if game_full { Rectangle::new(self.cascade(), geo.size) } else { fit_in(zone, Rectangle::new(self.cascade(), geo.size)) };
        let at = place.loc;
        let _ = window.configure(place);
        let w = Window::new_x11_window(window.clone());
        self.space.map_element(w.clone(), at, true);
        // What it asked for before it was shown (full screen, maximized) is read on another thread: x11_asked acts on it.
        if let Some(r) = &self.x11_reader {
            r.ask(window.window_id());
        }
        // A game the size of the screen that did not say so is taken as going full screen (a guess, so only for games).
        if game_full {
            self.fullscreen(&w);
        }
    }

    fn mapped_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.wake();
        if !self.overlays.iter().any(|x| same(x, &window)) {
            self.overlays.push(window);
        }
    }

    /// The window was hidden: for the compositor it closed (the same clean-up as a closed Wayland window). Shown again, it
    /// opens anew with a new surface.
    fn unmapped_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.wake();
        self.overlays.retain(|x| !same(x, &window));
        if self.x11_window(&window).is_some() {
            let surface = window.wl_surface();
            self.window_gone(surface.as_ref(), |w| w.x11_surface().is_some_and(|x| same(x, &window)));
        }
        if !window.is_override_redirect() {
            let _ = window.set_mapped(false);
        }
    }

    fn destroyed_window(&mut self, _xwm: XwmId, window: X11Surface) {
        self.wake();
        // normally it was unmapped first and is already forgotten
        self.overlays.retain(|x| !same(x, &window));
        if self.x11_window(&window).is_some() {
            self.window_gone(None, |w| w.x11_surface().is_some_and(|x| same(x, &window)));
        }
    }

    /// A program asks for another size or place. A window the compositor placed (tiled, maximized, full screen, snapped) is told
    /// its place again; any other may change its size but not move itself (the compositor places windows). Before it is shown,
    /// anything goes (its place is chosen when it is shown).
    fn configure_request(
        &mut self,
        _xwm: XwmId,
        window: X11Surface,
        x: Option<i32>,
        y: Option<i32>,
        w: Option<u32>,
        h: Option<u32>,
        _reorder: Option<Reorder>,
    ) {
        self.wake();
        let mut geo = window.geometry();
        match self.x11_window(&window) {
            Some(win) if self.placed_by_layout(&win) => {
                let _ = window.configure(None);
                return;
            }
            Some(_) => {}
            None => {
                geo.loc.x = x.unwrap_or(geo.loc.x);
                geo.loc.y = y.unwrap_or(geo.loc.y);
            }
        }
        if let Some(w) = w {
            geo.size.w = w as i32;
        }
        if let Some(h) = h {
            geo.size.h = h as i32;
        }
        // a shown window may grow only as far as the free space (a full screen one was answered above)
        if self.x11_window(&window).is_some() {
            let zone = smithay::desktop::layer_map_for_output(&self.output).non_exclusive_zone();
            geo.size.w = geo.size.w.min(zone.size.w.max(1));
            geo.size.h = geo.size.h.min(zone.size.h.max(1));
        }
        let _ = window.configure(geo);
    }

    /// Something moved or changed size in the X server. Managed windows are placed by the compositor and already known; menus
    /// and tooltips are drawn from their own geometry, so only a new frame is needed.
    fn configure_notify(&mut self, _xwm: XwmId, _window: X11Surface, _geometry: Rectangle<i32, Logical>, _above: Option<u32>) {
        self.wake();
    }

    fn property_notify(&mut self, _xwm: XwmId, window: X11Surface, property: WmWindowProperty) {
        // a new title or class reaches the panel with the next frame
        self.wake();
        if property == WmWindowProperty::MotifHints && !self.decorates_all() {
            if let Some(s) = window.wl_surface() {
                self.set_decorated(&s, !window.is_decorated());
            }
        }
    }

    fn maximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(w) = self.x11_window(&window) {
            self.maximize(&w);
        }
    }

    fn unmaximize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(w) = self.x11_window(&window) {
            self.restore_window(&w);
        }
    }

    fn fullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(w) = self.x11_window(&window) {
            self.fullscreen(&w);
        }
    }

    fn unfullscreen_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(w) = self.x11_window(&window) {
            self.restore_window(&w);
        }
    }

    fn minimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(s) = window.wl_surface() {
            self.minimize_surface(&s);
        }
    }

    fn unminimize_request(&mut self, _xwm: XwmId, window: X11Surface) {
        if let Some(s) = window.wl_surface() {
            self.activate_surface(&s);
        }
    }

    /// The program's own title bar or edge is dragged (it holds the button down): the same grabs as for Wayland windows.
    fn move_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32) {
        let pointer = self.pointer.clone();
        let (Some(start), Some(w)) = (pointer.grab_start_data(), self.x11_window(&window)) else { return };
        let Some(origin) = self.space.element_location(&w) else { return };
        pointer.set_grab(self, MoveGrab { start, window: w, origin }, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    fn resize_request(&mut self, _xwm: XwmId, window: X11Surface, _button: u32, edge: X11Edge) {
        let pointer = self.pointer.clone();
        let (Some(start), Some(w)) = (pointer.grab_start_data(), self.x11_window(&window)) else { return };
        let Some(origin) = self.space.element_location(&w) else { return };
        let edges = match edge {
            X11Edge::Top => ResizeEdge::Top,
            X11Edge::Bottom => ResizeEdge::Bottom,
            X11Edge::Left => ResizeEdge::Left,
            X11Edge::Right => ResizeEdge::Right,
            X11Edge::TopLeft => ResizeEdge::TopLeft,
            X11Edge::TopRight => ResizeEdge::TopRight,
            X11Edge::BottomLeft => ResizeEdge::BottomLeft,
            X11Edge::BottomRight => ResizeEdge::BottomRight,
        };
        let size = w.geometry().size;
        pointer.set_grab(self, ResizeGrab { start, window: w, edges, origin, size }, SERIAL_COUNTER.next_serial(), Focus::Clear);
    }

    // The clipboard and the middle-click selection between X11 and Wayland programs. An X11 program may read them only while
    // one of its windows has the keyboard (as with Wayland programs), so a program in the background cannot watch the clipboard.

    fn allow_selection_access(&mut self, xwm: XwmId, _selection: SelectionTarget) -> bool {
        matches!(self.keyboard.current_focus(), Some(KbFocus::X11(x)) if x.xwm_id() == Some(xwm))
    }

    /// An X11 program pastes what a Wayland program copied.
    fn send_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_type: String, fd: OwnedFd) {
        let r = match selection {
            SelectionTarget::Clipboard => request_data_device_client_selection(&self.seat, mime_type, fd).map_err(|e| e.to_string()),
            SelectionTarget::Primary => request_primary_client_selection(&self.seat, mime_type, fd).map_err(|e| e.to_string()),
        };
        if let Err(e) = r {
            eprintln!("yggdrasil-compositor: an X11 program could not paste: {e}");
        }
    }

    /// An X11 program copied something: Wayland programs are offered it.
    fn new_selection(&mut self, _xwm: XwmId, selection: SelectionTarget, mime_types: Vec<String>) {
        match selection {
            SelectionTarget::Clipboard => set_data_device_selection(&self.display_handle, &self.seat, mime_types, ()),
            SelectionTarget::Primary => set_primary_selection(&self.display_handle, &self.seat, mime_types, ()),
        }
    }

    /// What an X11 program had copied is gone (only cleared when it was the X11 one: a Wayland program's stays).
    fn cleared_selection(&mut self, _xwm: XwmId, selection: SelectionTarget) {
        match selection {
            SelectionTarget::Clipboard => {
                if current_data_device_selection_userdata(&self.seat).is_some() {
                    clear_data_device_selection(&self.display_handle, &self.seat);
                }
            }
            SelectionTarget::Primary => {
                if current_primary_selection_userdata(&self.seat).is_some() {
                    clear_primary_selection(&self.display_handle, &self.seat);
                }
            }
        }
    }

    /// XWayland went away (it crashed or was killed): its windows are gone too.
    fn disconnected(&mut self, _xwm: XwmId) {
        eprintln!("yggdrasil-compositor: XWayland went away; X11 programs cannot start until the compositor is started again");
        self.xwm = None;
        self.x11_display = None;
        self.overlays.clear();
        let x11: Vec<Window> = self.all_windows().into_iter().map(|(w, _)| w).filter(|w| w.x11_surface().is_some()).collect();
        for w in x11 {
            let s = state::surface_of(&w);
            self.window_gone(s.as_ref(), |x| *x == w);
        }
    }
}

/// Draws the picture with its alpha channel ignored (a texture shader that sets alpha to 1).
///
/// Games often fill the alpha channel with leftovers: shown as it is, the desktop shows through in patches. An X11 window whose
/// visual has no alpha (most) already gets an XRGB buffer from XWayland, which the renderer draws opaque by itself; what is left
/// is a window with an ARGB visual, and a full screen one never means to be see-through, so full screen X11 windows are drawn
/// with this. It also tells the damage tracker the window covers its whole area, so nothing behind it is drawn.
pub const OPAQUE_SHADER: &str = r#"#version 100

//_DEFINES_

#if defined(EXTERNAL)
#extension GL_OES_EGL_image_external : require
#endif

precision mediump float;
#if defined(EXTERNAL)
uniform samplerExternalOES tex;
#else
uniform sampler2D tex;
#endif

uniform float alpha;
varying vec2 v_coords;

#if defined(DEBUG_FLAGS)
uniform float tint;
#endif

void main() {
    vec4 color = vec4(texture2D(tex, v_coords).rgb, 1.0) * alpha;
#if defined(DEBUG_FLAGS)
    if (tint == 1.0)
        color = vec4(0.0, 0.2, 0.0, 0.2) + color * 0.8;
#endif
    gl_FragColor = color;
}
"#;

/// A program's picture drawn opaque (see OPAQUE_SHADER).
pub struct OpaqueElement {
    pub inner: WaylandSurfaceRenderElement<GlesRenderer>,
    pub program: GlesTexProgram,
}

impl Element for OpaqueElement {
    fn id(&self) -> &Id {
        self.inner.id()
    }
    fn current_commit(&self) -> CommitCounter {
        self.inner.current_commit()
    }
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.inner.location(scale)
    }
    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.inner.src()
    }
    fn transform(&self) -> Transform {
        self.inner.transform()
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.inner.geometry(scale)
    }
    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        self.inner.damage_since(scale, commit)
    }
    fn opaque_regions(&self, scale: Scale<f64>) -> OpaqueRegions<i32, Physical> {
        OpaqueRegions::from_slice(&[Rectangle::from_size(self.geometry(scale).size)])
    }
    fn alpha(&self) -> f32 {
        self.inner.alpha()
    }
    fn kind(&self) -> Kind {
        self.inner.kind()
    }
}

impl RenderElement<GlesRenderer> for OpaqueElement {
    fn draw(
        &self,
        frame: &mut GlesFrame<'_, '_>,
        src: Rectangle<f64, BufferCoords>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), GlesError> {
        frame.override_default_tex_program(self.program.clone(), vec![]);
        let r = RenderElement::<GlesRenderer>::draw(&self.inner, frame, src, dst, damage, opaque_regions);
        frame.clear_tex_program_override();
        r
    }

    // No underlying buffer is offered (that would let a hardware backend show it directly, alpha and all); a hardware backend
    // that wants to scan full screen games out directly must put them on a plane that ignores alpha.
    fn underlying_storage(&self, _renderer: &mut GlesRenderer) -> Option<UnderlyingStorage<'_>> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_window_is_fitted_into_the_free_space() {
        let zone = Rectangle::new((0, 40).into(), (1280, 680).into());
        let r = |x, y, w, h| Rectangle::<i32, Logical>::new((x, y).into(), (w, h).into());
        assert_eq!(fit_in(zone, r(132, 132, 1373, 1080)), r(0, 40, 1280, 680), "Steam's window, shrunk and moved on");
        assert_eq!(fit_in(zone, r(100, 100, 400, 300)), r(100, 100, 400, 300), "a window that fits is left alone");
        assert_eq!(fit_in(zone, r(1100, 600, 400, 300)), r(880, 420, 400, 300), "moved back on at the edge");
    }

    #[test]
    fn games_are_known_by_their_class() {
        assert!(is_game_class("steam_app_242760"));
        assert!(is_game_class("TheForest.exe"));
        assert!(is_game_class("STEAM_APP_1"));
        assert!(!is_game_class("steam"), "Steam's own client is not a game");
        assert!(!is_game_class("xterm"));
        assert!(!is_game_class("exe"));
    }

    #[test]
    fn the_opaque_list_reads() {
        assert_eq!(
            parse_opaque("# games that leave junk in alpha\nMyGame\n  other  # comment\n\n"),
            vec!["mygame".to_string(), "other".to_string()]
        );
        assert!(parse_opaque("").is_empty());
    }
}
