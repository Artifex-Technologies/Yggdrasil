//! The compositor's grabs of the pointer and the touchscreen: a window moved or resized (the xdg `move` and `resize` requests,
//! sent when an app's own title bar or edge is dragged; X11's `_NET_WM_MOVERESIZE`; Super+drag, the grips and the title area),
//! and a program's drag and drop (`DragGrab`).
//!
//! A move or a resize (`Move`, `Resize`: what the window does) is held by a pointer button (`ByButton`) until that button is let
//! go, or by a finger (`ByFinger`) until that finger lifts or the touchscreen cancels it.

use smithay::desktop::Window;
use smithay::input::dnd::GrabType;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent,
    GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent, GestureSwipeUpdateEvent, GrabStartData as PointerStart,
    MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
};
use smithay::input::touch::{self, GrabStartData as TouchStart, TouchGrab, TouchInnerHandle};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::utils::{Logical, Point, Size};

use crate::state::{App, PtrFocus};

/// The window may not be made smaller than this.
const MIN_W: i32 = 240;
const MIN_H: i32 = 140;

/// What is under the pointer or a finger: the surface and where its origin is.
type Under = Option<(PtrFocus, Point<f64, Logical>)>;

/// What the window does while the button or finger holding it is `moved` from where it went down.
pub trait Follow: Send + 'static {
    fn follow(&self, data: &mut App, moved: Point<f64, Logical>);
}

/// The window moves as far as the pointer or finger has.
pub struct Move {
    pub window: Window,
    pub origin: Point<i32, Logical>,
}

impl Follow for Move {
    fn follow(&self, data: &mut App, moved: Point<f64, Logical>) {
        let to = (self.origin.to_f64() + moved).to_i32_round();
        data.space.map_element(self.window.clone(), to, true);
        crate::state::x11_place(&self.window, to, crate::state::window_size(&self.window));
    }
}

/// The window's edge (or the two at a corner) moves as far as the pointer or finger has.
pub struct Resize {
    pub window: Window,
    pub edges: ResizeEdge,
    pub origin: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
}

impl Resize {
    fn has(&self, names: &[ResizeEdge]) -> bool {
        names.contains(&self.edges)
    }
}

impl Follow for Resize {
    fn follow(&self, data: &mut App, moved: Point<f64, Logical>) {
        let (dx, dy) = (moved.x as i32, moved.y as i32);
        let left = self.has(&[ResizeEdge::Left, ResizeEdge::TopLeft, ResizeEdge::BottomLeft]);
        let right = self.has(&[ResizeEdge::Right, ResizeEdge::TopRight, ResizeEdge::BottomRight]);
        let top = self.has(&[ResizeEdge::Top, ResizeEdge::TopLeft, ResizeEdge::TopRight]);
        let bottom = self.has(&[ResizeEdge::Bottom, ResizeEdge::BottomLeft, ResizeEdge::BottomRight]);
        let mut w = self.size.w
            + if right {
                dx
            } else if left {
                -dx
            } else {
                0
            };
        let mut h = self.size.h
            + if bottom {
                dy
            } else if top {
                -dy
            } else {
                0
            };
        w = w.max(MIN_W);
        h = h.max(MIN_H);
        if let Some(t) = self.window.toplevel() {
            t.with_pending_state(|s| s.size = Some((w, h).into()));
            t.send_pending_configure();
        }
        // dragging the left or top edge also moves the window's corner, so the opposite edge stays where it was
        let x = if left { self.origin.x + self.size.w - w } else { self.origin.x };
        let y = if top { self.origin.y + self.size.h - h } else { self.origin.y };
        data.space.map_element(self.window.clone(), (x, y), false);
        // an X11 window is given its new size and place at once (it has no configure round trip like a Wayland one)
        crate::state::x11_place(&self.window, (x, y).into(), (w, h).into());
    }
}

/// A move or resize held by a pointer button, until it is let go. The pointer's motion reaches no program meanwhile: the window
/// being dragged is not "hovered" during the drag.
pub struct ByButton<F> {
    pub start: PointerStart<App>,
    pub what: F,
}

impl<F: Follow> PointerGrab<App> for ByButton<F> {
    fn motion(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, _focus: Under, event: &MotionEvent) {
        handle.motion(data, None, event);
        self.what.follow(data, event.location - self.start.location);
    }

    fn relative_motion(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, focus: Under, event: &RelativeMotionEvent) {
        handle.relative_motion(data, focus, event);
    }

    fn button(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, details: AxisFrame) {
        handle.axis(data, details);
    }

    fn frame(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GestureSwipeBeginEvent) {
        handle.gesture_swipe_begin(data, event);
    }

    fn gesture_swipe_update(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GestureSwipeUpdateEvent) {
        handle.gesture_swipe_update(data, event);
    }

    fn gesture_swipe_end(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GestureSwipeEndEvent) {
        handle.gesture_swipe_end(data, event);
    }

    fn gesture_pinch_begin(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GesturePinchBeginEvent) {
        handle.gesture_pinch_begin(data, event);
    }

    fn gesture_pinch_update(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GesturePinchUpdateEvent) {
        handle.gesture_pinch_update(data, event);
    }

    fn gesture_pinch_end(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GesturePinchEndEvent) {
        handle.gesture_pinch_end(data, event);
    }

    fn gesture_hold_begin(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GestureHoldBeginEvent) {
        handle.gesture_hold_begin(data, event);
    }

    fn gesture_hold_end(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &GestureHoldEndEvent) {
        handle.gesture_hold_end(data, event);
    }

    fn start_data(&self) -> &PointerStart<App> {
        &self.start
    }

    fn unset(&mut self, _data: &mut App) {}
}

/// A move or resize held by a finger, until it lifts or the touchscreen cancels it. The program the finger went down on hears
/// nothing of that finger meanwhile but its lift at the end (which ends its touch point there); fingers already down go on to
/// their programs, and one put down meanwhile reaches none (as during a finger's drag, Smithay's).
pub struct ByFinger<F> {
    pub start: TouchStart<App>,
    pub what: F,
    /// another finger's change was passed on since the last frame: only then is the frame (Smithay takes a frame with nothing
    /// before it for a mistake)
    told: bool,
}

impl<F> ByFinger<F> {
    pub fn new(start: TouchStart<App>, what: F) -> ByFinger<F> {
        ByFinger { start, what, told: false }
    }
}

impl<F: Follow> TouchGrab<App> for ByFinger<F> {
    fn down(&mut self, _data: &mut App, _handle: &mut TouchInnerHandle<'_, App>, _focus: Under, _event: &touch::DownEvent) {}

    fn up(&mut self, data: &mut App, handle: &mut TouchInnerHandle<'_, App>, event: &touch::UpEvent) {
        handle.up(data, event);
        self.told = true;
        if event.slot == self.start.slot {
            handle.unset_grab(self, data);
        }
    }

    fn motion(&mut self, data: &mut App, handle: &mut TouchInnerHandle<'_, App>, focus: Under, event: &touch::MotionEvent) {
        if event.slot == self.start.slot {
            self.what.follow(data, event.location - self.start.location);
        } else {
            handle.motion(data, focus, event);
            self.told = true;
        }
    }

    fn frame(&mut self, data: &mut App, handle: &mut TouchInnerHandle<'_, App>) {
        if std::mem::take(&mut self.told) {
            handle.frame(data);
        }
    }

    fn cancel(&mut self, data: &mut App, handle: &mut TouchInnerHandle<'_, App>) {
        handle.cancel(data);
        handle.unset_grab(self, data);
    }

    fn shape(&mut self, data: &mut App, handle: &mut TouchInnerHandle<'_, App>, event: &touch::ShapeEvent) {
        if event.slot != self.start.slot {
            handle.shape(data, event);
            self.told = true;
        }
    }

    fn orientation(&mut self, data: &mut App, handle: &mut TouchInnerHandle<'_, App>, event: &touch::OrientationEvent) {
        if event.slot != self.start.slot {
            handle.orientation(data, event);
            self.told = true;
        }
    }

    fn start_data(&self) -> &TouchStart<App> {
        &self.start
    }

    fn unset(&mut self, _data: &mut App) {}
}

/// A program's drag (Smithay's drag and drop grab, `grab`) held by a pointer button or by a finger (`by`). Smithay tells a
/// drag's end (`DndGrabHandler::dropped`, `cancelled`) without saying whose it was, and a drag ends only within a call to its
/// grab, which comes through here: `App::drag_by` says whose for that call, so only that device's drag ends (state.rs).
pub struct DragGrab<G> {
    pub grab: G,
    pub by: GrabType,
}

/// Each call passed on to the drag's own grab, with `App::drag_by` saying whose drag it is meanwhile.
macro_rules! as_the_drag {
    ($($call:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $call(&mut self, data: &mut App, $($arg: $ty),*) {
            let outer = data.drag_by.replace(self.by);
            self.grab.$call(data, $($arg),*);
            data.drag_by = outer;
        }
    )*};
}

impl<G: PointerGrab<App>> PointerGrab<App> for DragGrab<G> {
    as_the_drag! {
        motion(handle: &mut PointerInnerHandle<'_, App>, focus: Under, event: &MotionEvent);
        relative_motion(handle: &mut PointerInnerHandle<'_, App>, focus: Under, event: &RelativeMotionEvent);
        button(handle: &mut PointerInnerHandle<'_, App>, event: &ButtonEvent);
        axis(handle: &mut PointerInnerHandle<'_, App>, details: AxisFrame);
        frame(handle: &mut PointerInnerHandle<'_, App>);
        gesture_swipe_begin(handle: &mut PointerInnerHandle<'_, App>, event: &GestureSwipeBeginEvent);
        gesture_swipe_update(handle: &mut PointerInnerHandle<'_, App>, event: &GestureSwipeUpdateEvent);
        gesture_swipe_end(handle: &mut PointerInnerHandle<'_, App>, event: &GestureSwipeEndEvent);
        gesture_pinch_begin(handle: &mut PointerInnerHandle<'_, App>, event: &GesturePinchBeginEvent);
        gesture_pinch_update(handle: &mut PointerInnerHandle<'_, App>, event: &GesturePinchUpdateEvent);
        gesture_pinch_end(handle: &mut PointerInnerHandle<'_, App>, event: &GesturePinchEndEvent);
        gesture_hold_begin(handle: &mut PointerInnerHandle<'_, App>, event: &GestureHoldBeginEvent);
        gesture_hold_end(handle: &mut PointerInnerHandle<'_, App>, event: &GestureHoldEndEvent);
        unset();
    }

    fn start_data(&self) -> &PointerStart<App> {
        self.grab.start_data()
    }
}

impl<G: TouchGrab<App>> TouchGrab<App> for DragGrab<G> {
    as_the_drag! {
        down(handle: &mut TouchInnerHandle<'_, App>, focus: Under, event: &touch::DownEvent);
        up(handle: &mut TouchInnerHandle<'_, App>, event: &touch::UpEvent);
        motion(handle: &mut TouchInnerHandle<'_, App>, focus: Under, event: &touch::MotionEvent);
        frame(handle: &mut TouchInnerHandle<'_, App>);
        cancel(handle: &mut TouchInnerHandle<'_, App>);
        shape(handle: &mut TouchInnerHandle<'_, App>, event: &touch::ShapeEvent);
        orientation(handle: &mut TouchInnerHandle<'_, App>, event: &touch::OrientationEvent);
        unset();
    }

    fn start_data(&self) -> &TouchStart<App> {
        self.grab.start_data()
    }
}
