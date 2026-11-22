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
use smithay::xwayland::{X11Surface, X11Wm, XWayland, XWaylandClientData, XWaylandEvent, XwmHandler};
use x11rb::connection::Connection as _;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::wrapper::ConnectionExt as _;

use crate::grabs::{MoveGrab, ResizeGrab};
use crate::state::{self, App, KbFocus};

/// Start XWayland. It answers a moment later; then its window manager starts, X11 programs can be started (DISPLAY is known)
/// and `then` runs, also when XWayland failed (it starts the --run programs, so an X11 one finds its window manager waiting).
pub fn start(app: &mut App, handle: LoopHandle<'static, App>, then: Box<dyn FnOnce(&mut App)>) -> Result<(), Box<dyn std::error::Error>> {
    // XWayland's own messages are rarely useful; YGGDRASIL_COMPOSITOR_DEBUG shows them
    let out = || if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() { Stdio::inherit() } else { Stdio::null() };
    let (xwayland, client) =
        XWayland::spawn(&app.display_handle, None, std::iter::empty::<(String, String)>(), true, out(), out(), |_| ())?;
    // X11 programs at the screens' own resolution from the start (follow_scale), before XWayland reads the screens
    app.xwayland_client = Some(client.clone());
    app.follow_scale();
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
                        set_xft_dpi(display_number, app.x11_client_scale());
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

/// `~/.config/yggdrasil/xwayland.conf`: `stretch = true` draws X11 programs as before 2026-11-22, stretched to the desktop's
/// scale like a Wayland program that ignores it (the right size, but soft, and a game offered only the scaled-down resolution).
pub fn xwayland_conf_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("xwayland.conf")
}

pub fn parse_stretch(text: &str) -> bool {
    text.lines().filter_map(|l| l.split('#').next().unwrap_or("").split_once('=')).any(|(k, v)| k.trim() == "stretch" && v.trim() == "true")
}

/// The scale X11 programs draw at (XWayland's client scale): the largest of the screens' that are on, so XWayland is told each
/// screen's size in pixels and a game offers, and draws, the screen's own resolution (1920x1200 on a 1920x1200 screen at 125%,
/// not 1536x960 stretched). As KDE's "apply scaling themselves" and Hyprland's force_zero_scaling. Never below 1 (Smithay 0.7
/// divides by its whole part), and 1 with `stretch`.
pub fn x11_scale(scales: impl Iterator<Item = f64>, stretch: bool) -> f64 {
    if stretch {
        return 1.0;
    }
    scales.fold(1.0, f64::max)
}

/// A new X11 window's geometry as it should be: Smithay 0.7 converts the first one (CreateNotify) with the scale rounded down to
/// a whole number (1.25 taken as 1), so a menu shown where it was made, without moving, would be drawn a quarter too far out.
pub fn unrounded(g: Rectangle<i32, Logical>, scale: f64) -> Rectangle<i32, Logical> {
    let k = (scale as i32).max(1) as f64 / scale;
    if (k - 1.0).abs() < 1e-9 {
        return g;
    }
    let f = |v: i32| (v as f64 * k).round() as i32;
    Rectangle::new((f(g.loc.x), f(g.loc.y)).into(), (f(g.size.w), f(g.size.h)).into())
}

/// X resources with Xft.dpi set to `dpi`, any earlier Xft.dpi line replaced.
pub fn with_xft_dpi(old: &str, dpi: u32) -> String {
    let mut s: String = old.lines().filter(|l| !l.trim_start().starts_with("Xft.dpi:")).map(|l| format!("{l}\n")).collect();
    s.push_str(&format!("Xft.dpi:\t{dpi}\n"));
    s
}

/// Tell X11 programs the DPI for the scale (Xft.dpi, 96 for each 1.0), as KDE does for programs that scale themselves: the fonts
/// of GTK and Qt programs, and Chromium-based ones (Steam's window) whole, grow back to the desktop's size. On a thread of its
/// own: the X server is a client of this compositor and may be waiting on it.
fn set_xft_dpi(display: u32, scale: f64) {
    let dpi = (96.0 * scale).round() as u32;
    let _ = std::thread::Builder::new().name("xft-dpi".into()).spawn(move || {
        let Ok((conn, screen)) = x11rb::rust_connection::RustConnection::connect(Some(&format!(":{display}"))) else { return };
        let root = conn.setup().roots[screen].root;
        let old = conn
            .get_property(false, root, AtomEnum::RESOURCE_MANAGER, AtomEnum::STRING, 0, 1 << 20)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| String::from_utf8_lossy(&r.value).into_owned())
            .unwrap_or_default();
        let _ = conn.change_property8(
            PropMode::REPLACE,
            root,
            AtomEnum::RESOURCE_MANAGER,
            AtomEnum::STRING,
            with_xft_dpi(&old, dpi).as_bytes(),
        );
        // an answered request: everything before it has reached the server
        let _ = conn.get_input_focus().map(|c| c.reply());
    });
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
        } else if a.maximized && !a.fullscreen && !x.is_maximized() && !self.placed_by_layout(&w) && !crate::state::ignores_maximize(&w) {
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

    /// The scale X11 programs draw at now (XWayland's client scale; 1 before XWayland runs).
    pub fn x11_client_scale(&self) -> f64 {
        self.xwayland_client
            .as_ref()
            .and_then(|c| c.get_data::<XWaylandClientData>())
            .map(|d| d.compositor_state.client_scale())
            .unwrap_or(1.0)
    }

    /// Set the scale X11 programs draw at to the screens' (`x11_scale`): at XWayland's start, when the screens change and when
    /// xwayland.conf does. XWayland is told the screens again (their size in its pixels), every X11 window its place and size
    /// in those pixels, and the programs their DPI.
    pub fn follow_scale(&mut self) {
        let Some(client) = self.xwayland_client.clone() else { return };
        let Some(data) = client.get_data::<XWaylandClientData>() else { return };
        let stretch = parse_stretch(&std::fs::read_to_string(xwayland_conf_path()).unwrap_or_default());
        let want = x11_scale(self.heads.list.iter().filter(|h| h.enabled).map(|h| h.output.current_scale().fractional_scale()), stretch);
        if data.compositor_state.client_scale() == want {
            return;
        }
        data.compositor_state.set_client_scale(want);
        for h in self.heads.list.iter().filter(|h| h.enabled) {
            h.output.change_current_state(None, None, None, None);
        }
        for w in self.space.elements() {
            if let Some(x) = w.x11_surface() {
                let _ = x.configure(None);
            }
        }
        if let Some(d) = self.x11_display {
            set_xft_dpi(d, want);
        }
    }

    /// Where an X11 menu or tooltip is: its geometry, set right while Smithay still has its rounded first one (`unrounded`).
    pub fn overlay_geo(&self, x: &X11Surface) -> Rectangle<i32, Logical> {
        if self.x11_rounded.contains(&x.window_id()) {
            unrounded(x.geometry(), self.x11_client_scale())
        } else {
            x.geometry()
        }
    }

    /// Is the point over an X11 menu or tooltip? A click there is the menu's, not the window's behind it.
    pub fn over_overlay(&self, pos: Point<f64, Logical>) -> bool {
        self.overlays.iter().any(|x| self.overlay_geo(x).to_f64().contains(pos))
    }

    /// The surface of an X11 menu or tooltip under the point, and where that surface's origin is on screen.
    pub fn overlay_under(&self, pos: Point<f64, Logical>) -> Option<(WlSurface, Point<f64, Logical>)> {
        for x in self.overlays.iter().rev() {
            let Some(s) = x.wl_surface() else { continue };
            let loc = self.overlay_geo(x).loc;
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

    /// A new X11 window: its first geometry set right (`unrounded`), which tells the X server the same place in its pixels.
    fn new_window(&mut self, _xwm: XwmId, window: X11Surface) {
        let s = self.x11_client_scale();
        if s.fract() != 0.0 {
            let _ = window.configure(unrounded(window.geometry(), s));
        }
    }

    /// A new menu or tooltip: it cannot be configured, so its first geometry is set right where it is used, until it moves.
    fn new_override_redirect_window(&mut self, _xwm: XwmId, window: X11Surface) {
        if self.x11_client_scale().fract() != 0.0 {
            self.x11_rounded.push(window.window_id());
        }
    }

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
        // on the screen the pointer is on, as a Wayland window
        let o = self.output_at(self.pointer_location);
        let zone = self.free_area(&o);
        let screen = crate::state::logical_size(&o);
        let game_full = is_game(&window) && screen.w > 0 && geo.size.w >= screen.w && geo.size.h >= screen.h;
        let start = o.current_location() + self.cascade();
        let w = Window::new_x11_window(window.clone());
        // back where the program's last window closed (places.rs), as a Wayland window
        let place = if game_full {
            Rectangle::new(start, geo.size)
        } else {
            self.remembered_place(&w).unwrap_or_else(|| fit_in(zone, Rectangle::new(start, geo.size)))
        };
        let _ = window.configure(place);
        self.space.map_element(w.clone(), place.loc, true);
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
        self.x11_rounded.retain(|id| *id != window.window_id());
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
    /// and tooltips are drawn from their own geometry (Smithay's, exact from now on), so only a new frame is needed.
    fn configure_notify(&mut self, _xwm: XwmId, window: X11Surface, _geometry: Rectangle<i32, Logical>, _above: Option<u32>) {
        self.wake();
        self.x11_rounded.retain(|id| *id != window.window_id());
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
            if crate::state::ignores_maximize(&w) {
                return;
            }
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
    #[test]
    fn x11_programs_draw_at_the_screens_resolution() {
        use super::{parse_stretch, unrounded, with_xft_dpi, x11_scale};
        use smithay::utils::Rectangle;
        // the largest scale of the screens that are on; never below 1; 1 when xwayland.conf says stretch
        assert_eq!(x11_scale([1.25].into_iter(), false), 1.25);
        assert_eq!(x11_scale([1.0, 2.0].into_iter(), false), 2.0);
        assert_eq!(x11_scale([0.75].into_iter(), false), 1.0);
        assert_eq!(x11_scale(std::iter::empty(), false), 1.0);
        assert_eq!(x11_scale([1.25].into_iter(), true), 1.0);
        assert!(parse_stretch("# old way\nstretch = true\n"));
        assert!(!parse_stretch("stretch = false\n# stretch = true\n"));
        assert!(!parse_stretch(""));
        // Smithay took a new window's 1000,500 1920x1200 (X11 pixels) at 1.25 as if the scale were 1
        let g = Rectangle::new((1000, 500).into(), (1920, 1200).into());
        assert_eq!(unrounded(g, 1.25), Rectangle::new((800, 400).into(), (1536, 960).into()));
        assert_eq!(unrounded(g, 2.0), g, "a whole scale was converted right");
        assert_eq!(unrounded(g, 1.0), g);
        // at 2.5 it divided by 2: the rest of the way
        assert_eq!(
            unrounded(Rectangle::new((400, 200).into(), (800, 600).into()), 2.5),
            Rectangle::new((320, 160).into(), (640, 480).into())
        );
        // the programs' DPI, the other resources kept
        assert_eq!(with_xft_dpi("Xcursor.size:\t24\nXft.dpi:\t96\n", 120), "Xcursor.size:\t24\nXft.dpi:\t120\n");
        assert_eq!(with_xft_dpi("", 96), "Xft.dpi:\t96\n");
    }

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
