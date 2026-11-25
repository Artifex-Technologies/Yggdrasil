//! One stacking order for everything: what is drawn in front (`App::scene`), what a click, a touch or the pointer lands on
//! (`under`: the surface under the pointer, the window controls, the resize grips) and which window a click gives the keyboard
//! to all walk Smithay's `Space` from its last element (the front) to its first. Nothing else orders windows.
//!
//! The order changes only when a window is raised on purpose (it gets the keyboard at the same time: a click, Alt+Tab, the dock,
//! `activate`) or a window is shown (opened, brought back from minimized, a workspace shown). Moving or resizing a window keeps
//! its place in the stack: Smithay's `map_element` always puts the element on top, so everything that only moves a window goes
//! through `relocate`. Before 2026-11-19 maximize, full screen, snapping, tiling, keeping windows clear of the bar and screens
//! changing size all went through `map_element` and lifted windows over the one with the keyboard: a window behind a full
//! screen game could end up drawn over it and take its clicks while the keyboard stayed with the game.
//!
//! A full screen window in front on its own screen hides that screen's top bar and dock (the Top layer goes behind the windows,
//! for drawing and for the pointer alike); screens are looked at one by one, so a game on one screen leaves the bar of the other.

use smithay::desktop::{layer_map_for_output, LayerSurface, Window, WindowSurfaceType};
use smithay::output::Output;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::{self, ResizeEdge};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{FrameExtents, Logical, Point, Rectangle};
use smithay::wayland::shell::wlr_layer::Layer;
use smithay::xwayland::X11Surface;

use super::{win_has, window_geometry, App, PtrFocus};
use crate::deco::{self, Hit};

/// How far out from its window an X11 program's surface takes clicks. A program that draws its own shadow (GTK, Chromium) keeps
/// its resize border at the inner edge of the shadow and lets clicks on the rest through to what is behind, by an X input
/// shape that XWayland does not hand on: the surface would take them all over the shadow, which is not drawn. GTK 3 takes
/// them 10 pixels out, GTK 4 12 (a click past GTK 3's would be lost: the X server finds no window of it there). In the
/// program's own pixels: X11 programs draw at XWayland's scale, so at 125 % this is 8 logical pixels.
const X11_BORDER: f64 = 10.0;

/// What is under a point of the space, the frontmost thing there.
// an X11 window is large (see KbFocus); this lives only while a point is looked up, so boxing it would only add an allocation
#[allow(clippy::large_enum_variant)]
pub enum Under {
    /// the lock's surface on that screen (while locked there is nothing else)
    Lock(WlSurface, Point<f64, Logical>),
    /// an X11 menu or tooltip: its window, its surface and where that surface's origin is
    Overlay(X11Surface, WlSurface, Point<f64, Logical>),
    /// a panel or background on a layer: the (sub)surface under the point and its origin
    Layer(LayerSurface, Layer, WlSurface, Point<f64, Logical>),
    /// a window: its surface, a subsurface or one of its menus (xdg popups) under the point, and that surface's origin
    Window(Window, WlSurface, Point<f64, Logical>),
    /// a window's own controls (server-side decoration)
    Controls(Window, Hit),
    /// a resize grip just outside a decorated window
    Edge(Window, ResizeEdge),
}

impl Under {
    /// The surface that gets pointer and touch events (none for the compositor's own controls and grips).
    pub fn surface(self) -> Option<(WlSurface, Point<f64, Logical>)> {
        match self {
            Under::Lock(s, p) | Under::Overlay(_, s, p) | Under::Layer(_, _, s, p) | Under::Window(_, s, p) => Some((s, p)),
            Under::Controls(..) | Under::Edge(..) => None,
        }
    }

    /// What pointer and touch events go to, as the seat's focus: as `surface`, with an X11 window or menu as itself, so a drag
    /// can speak XDND to it (its surface gets the events all the same).
    pub fn target(self) -> Option<(PtrFocus, Point<f64, Logical>)> {
        match self {
            Under::Overlay(x, s, p) => Some((PtrFocus::X11(x, s), p)),
            Under::Window(w, s, p) => Some((
                match w.x11_surface() {
                    Some(x) => PtrFocus::X11(x.clone(), s),
                    None => PtrFocus::Wl(s),
                },
                p,
            )),
            u => u.surface().map(|(s, p)| (PtrFocus::Wl(s), p)),
        }
    }

    /// The window this belongs to (its surface, its controls or its grips).
    pub fn window(&self) -> Option<&Window> {
        match self {
            Under::Window(w, ..) | Under::Controls(w, _) | Under::Edge(w, _) => Some(w),
            _ => None,
        }
    }
}

impl App {
    /// The frontmost thing at `pos`, in the order `scene` draws: X11 menus, the Overlay layer, the Top layer (behind the windows
    /// when a full screen window is in front on that screen), the windows from the front (each: its controls, its picture and
    /// menus, its grips), then the Bottom and Background layers. While locked, only the lock.
    pub fn under(&self, pos: Point<f64, Logical>) -> Option<Under> {
        let output = self.output_at(pos);
        let origin = output.current_location();
        if self.locked {
            return self
                .lock_surfaces
                .iter()
                .find(|(o, _)| o == &output)
                .map(|(_, l)| Under::Lock(l.wl_surface().clone(), origin.to_f64()));
        }
        if let Some((x, s, p)) = self.overlay_under(pos) {
            return Some(Under::Overlay(x, s, p));
        }
        let full = self.fullscreen_in_front(&output);
        let on_layer = |layer: Layer| {
            let map = layer_map_for_output(&output);
            let local = pos - origin.to_f64();
            let l = map.layer_under(layer, local)?;
            let geo = map.layer_geometry(l)?;
            let (s, p) = l.surface_under(local - geo.loc.to_f64(), WindowSurfaceType::ALL)?;
            Some(Under::Layer(l.clone(), layer, s, (p + geo.loc + origin).to_f64()))
        };
        if let Some(u) = on_layer(Layer::Overlay).or_else(|| if full { None } else { on_layer(Layer::Top) }) {
            return Some(u);
        }
        if let Some(u) = self.space.elements().rev().find_map(|w| self.window_part(w, pos)) {
            return Some(u);
        }
        if full { on_layer(Layer::Top) } else { None }.or_else(|| on_layer(Layer::Bottom)).or_else(|| on_layer(Layer::Background))
    }

    /// The part of one window at `pos`: its controls (drawn over its picture), its picture or menus, its grips (outside it).
    fn window_part(&self, w: &Window, pos: Point<f64, Logical>) -> Option<Under> {
        let loc = self.space.element_location(w)?;
        let geo = window_geometry(w);
        if geo.size.w <= 0 || geo.size.h <= 0 {
            // nothing drawn yet: `scene` skips it, so the pointer passes through it too
            return None;
        }
        if let Some(r) = self.bar_rect(w) {
            if r.to_f64().contains(pos) {
                return Some(Under::Controls(w.clone(), deco::hit(pos.x - r.loc.x as f64)));
            }
        }
        let inner = Rectangle::new(loc, geo.size).to_f64();
        let around = |by: f64| inner + FrameExtents::new(by, by, by, by);
        let decorated = self.bar_visible(w);
        // where the surface's own origin is (the window's geometry may start inside it, past a client-side shadow); an X11
        // window's surface only as far out as X11_BORDER, and not past a decorated window, where the compositor's grips are
        let render = loc - geo.loc;
        if w.x11_surface().is_none() || around(if decorated { 0.0 } else { X11_BORDER / self.x11_client_scale() }).contains(pos) {
            if let Some((s, p)) = w.surface_under(pos - render.to_f64(), WindowSurfaceType::ALL) {
                return Some(Under::Window(w.clone(), s, (p + render).to_f64()));
            }
        }
        // grips around a decorated window, and around any floating one where its own surface does not reach (a program that
        // draws its own frame without a shadow to resize by, as Chromium and Electron programs do, would have no edge to drag
        // but a few pixels inside it)
        if !decorated && !self.free_floating(w) {
            return None;
        }
        let outer = around(deco::GRIP as f64);
        if inner.contains(pos) || !outer.contains(pos) {
            return None;
        }
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
        Some(Under::Edge(w.clone(), edge))
    }

    /// The window controls under the point, when they are the frontmost thing there.
    pub(super) fn bar_at(&self, pos: Point<f64, Logical>) -> Option<(Window, Hit)> {
        match self.under(pos)? {
            Under::Controls(w, h) => Some((w, h)),
            _ => None,
        }
    }

    /// The resize grip under the point, when it is the frontmost thing there.
    pub(super) fn edge_at(&self, pos: Point<f64, Logical>) -> Option<(Window, ResizeEdge)> {
        match self.under(pos)? {
            Under::Edge(w, e) => Some((w, e)),
            _ => None,
        }
    }

    /// Is the window in front on this screen (the frontmost drawn window whose middle is on it) full screen? Then its top bar
    /// and dock go behind the windows, for drawing and for the pointer.
    pub fn fullscreen_in_front(&self, o: &Output) -> bool {
        self.space
            .elements()
            .rev()
            .filter(|w| {
                let size = window_geometry(w).size;
                size.w > 0 && size.h > 0
            })
            .find(|w| self.window_output(w) == *o)
            .is_some_and(|w| win_has(w, xdg_toplevel::State::Fullscreen))
    }

    /// Move a shown window to `loc` and keep its place in the stack (Smithay's `map_element` puts it on top, so the windows that
    /// were in front of it are put back in front, in their order). Neither raises it nor changes which window is active.
    pub fn relocate(&mut self, w: &Window, loc: Point<i32, Logical>) {
        let in_front: Vec<Window> = self.space.elements().skip_while(|e| *e != w).skip(1).cloned().collect();
        self.space.map_element(w.clone(), loc, false);
        for e in in_front {
            self.space.raise_element(&e, false);
        }
    }
}
