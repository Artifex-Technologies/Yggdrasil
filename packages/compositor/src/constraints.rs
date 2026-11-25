//! The pointer for games: relative motion (zwp-relative-pointer-v1) and pointer locks and confinement (zwp-pointer-constraints-v1).
//!
//! - **Relative motion**: every movement of a mouse or touchpad is also told, as it came (before and after libinput's
//!   acceleration), to the program under the pointer: a game turns its camera by it, also while the pointer is locked.
//! - **A lock** (a first-person game): while it is active the pointer stays where it is and only the relative motion goes on.
//!   **A confinement** (a strategy game's edge scrolling): the pointer stays inside the window (or the region it asked for).
//! - A constraint becomes active when the pointer is over its window, inside the region it gave, as other compositors do;
//!   Smithay ends it when the pointer leaves the window (its focus changes: another window raised, Alt+Tab, the lock screen).
//!   A lock's position hint (where the game drew its own pointer) is where the pointer is put when the lock ends.
//!
//! The hardware backend moves the pointer through `pointer_moved_by`; the headless backend's `pointer-move DX DY` (its virtual
//! mouse) does the same, for the tests.
use crate::state::{App, PtrFocus};
use smithay::input::pointer::{PointerHandle, RelativeMotionEvent};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point};
use smithay::wayland::pointer_constraints::{with_pointer_constraint, PointerConstraint, PointerConstraintsHandler};

/// What a constraint does to a movement.
enum Hold {
    /// none active: the pointer moves
    Free,
    /// a lock: it stays
    Locked,
    /// a confinement: it stays inside this (in the space of all screens)
    Confined(Box<dyn Fn(Point<f64, Logical>) -> bool>),
}

impl App {
    /// A mouse or touchpad moved by `delta` (libinput's, after its acceleration; `raw` before it), at `time`: the
    /// program under the pointer is told the relative motion, and the pointer moves unless a constraint holds it.
    pub fn pointer_moved_by(&mut self, delta: Point<f64, Logical>, raw: Point<f64, Logical>, time: smithay::backend::input::InputTime) {
        let pointer = self.pointer.clone();
        let under = self.target_under(self.pointer_location);
        pointer.relative_motion(self, under.clone(), &RelativeMotionEvent { delta, delta_unaccel: raw, time });
        let to = self.pointer_location + delta;
        let to = self.nearest_on_screens(to).unwrap_or(to);
        match self.hold(&pointer, under.as_ref()) {
            Hold::Locked => {
                self.cursor.last_motion = std::time::Instant::now();
                pointer.frame(self);
                return;
            }
            Hold::Confined(inside) if !inside(to) => {
                pointer.frame(self);
                return;
            }
            _ => {}
        }
        self.pointer_to(to, time);
        self.constraint_check();
    }

    /// The constraint active on the surface under the pointer, if the pointer is inside its region.
    fn hold(&self, pointer: &PointerHandle<App>, under: Option<&(PtrFocus, Point<f64, Logical>)>) -> Hold {
        let Some((focus, origin)) = under else { return Hold::Free };
        let (surface, origin) = (focus.surface(), *origin);
        let at = self.pointer_location - origin;
        // read before the constraint is (with_pointer_constraint holds the surface's state, which this takes too)
        let size =
            smithay::backend::renderer::utils::with_renderer_surface_state(surface, |st| st.surface_size()).flatten().unwrap_or_default();
        with_pointer_constraint(surface, pointer, |c| {
            let Some(c) = c.filter(|c| c.is_active()) else { return Hold::Free };
            if !c.region().is_none_or(|r| r.contains(at.to_i32_round())) {
                return Hold::Free;
            }
            match &*c {
                PointerConstraint::Locked(_) => Hold::Locked,
                PointerConstraint::Confined(conf) => {
                    let region = conf.region().cloned();
                    Hold::Confined(Box::new(move |p: Point<f64, Logical>| {
                        let local = p - origin;
                        let in_window = local.x >= 0.0 && local.y >= 0.0 && local.x < size.w as f64 && local.y < size.h as f64;
                        in_window && region.as_ref().is_none_or(|r| r.contains(local.to_i32_round()))
                    }))
                }
            }
        })
    }

    /// The constraint of the surface under the pointer becomes active when the pointer is inside its region.
    pub fn constraint_check(&mut self) {
        let pointer = self.pointer.clone();
        let Some((surface, origin)) = self.surface_under(self.pointer_location) else { return };
        let at = (self.pointer_location - origin).to_i32_round();
        with_pointer_constraint(&surface, &pointer, |c| {
            if let Some(c) = c.filter(|c| !c.is_active()) {
                if c.region().is_none_or(|r| r.contains(at)) {
                    c.activate();
                }
            }
        });
    }

    /// Whether a pointer lock is active (`ctl pointer-look`'s word for it, and the tests).
    pub fn pointer_held(&self) -> Option<&'static str> {
        let pointer = self.pointer.clone();
        let (surface, _) = self.surface_under(self.pointer_location)?;
        with_pointer_constraint(&surface, &pointer, |c| match c.filter(|c| c.is_active()).as_deref() {
            Some(PointerConstraint::Locked(_)) => Some("locked"),
            Some(PointerConstraint::Confined(_)) => Some("confined"),
            None => None,
        })
    }
}

impl PointerConstraintsHandler for App {
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
        // active at once when the pointer is already over it
        self.constraint_check();
    }

    fn cursor_position_hint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>, location: Point<f64, Logical>) {
        // where the game draws its own pointer: the pointer is put there, so it is there when the lock ends
        if !with_pointer_constraint(surface, pointer, |c| c.is_some_and(|c| c.is_active())) {
            return;
        }
        if let Some((s, origin)) = self.surface_under(self.pointer_location) {
            if &s == surface {
                self.pointer_location = origin + location;
            }
        }
    }
}
