//! Moving and resizing a window with the pointer (the xdg `move` and `resize` requests, sent when an app's own title bar or edge is dragged).

use smithay::desktop::Window;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent, GesturePinchEndEvent, GesturePinchUpdateEvent, GrabStartData, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent, MotionEvent, PointerGrab, PointerInnerHandle, RelativeMotionEvent,
};
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel::ResizeEdge;
use smithay::utils::{Logical, Point, Size};

use crate::state::App;

/// The window may not be made smaller than this.
const MIN_W: i32 = 240;
const MIN_H: i32 = 140;

macro_rules! pass_through_gestures {
    () => {
        fn relative_motion(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, focus: Option<(<App as smithay::input::SeatHandler>::PointerFocus, Point<f64, Logical>)>, event: &RelativeMotionEvent) {
            handle.relative_motion(data, focus, event);
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
        fn unset(&mut self, _data: &mut App) {}
    };
}

pub struct MoveGrab {
    pub start: GrabStartData<App>,
    pub window: Window,
    pub origin: Point<i32, Logical>,
}

impl PointerGrab<App> for MoveGrab {
    fn motion(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, _focus: Option<(<App as smithay::input::SeatHandler>::PointerFocus, Point<f64, Logical>)>, event: &MotionEvent) {
        // the pointer moves, but no window under it is told: the window being dragged is not "hovered" during the drag
        handle.motion(data, None, event);
        let delta = event.location - self.start.location;
        let to = (self.origin.to_f64() + delta).to_i32_round();
        data.space.map_element(self.window.clone(), to, true);
    }

    fn button(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn start_data(&self) -> &GrabStartData<App> {
        &self.start
    }

    pass_through_gestures!();
}

pub struct ResizeGrab {
    pub start: GrabStartData<App>,
    pub window: Window,
    pub edges: ResizeEdge,
    pub origin: Point<i32, Logical>,
    pub size: Size<i32, Logical>,
}

impl ResizeGrab {
    fn has(&self, names: &[ResizeEdge]) -> bool {
        names.contains(&self.edges)
    }
}

impl PointerGrab<App> for ResizeGrab {
    fn motion(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, _focus: Option<(<App as smithay::input::SeatHandler>::PointerFocus, Point<f64, Logical>)>, event: &MotionEvent) {
        handle.motion(data, None, event);
        let d = event.location - self.start.location;
        let (dx, dy) = (d.x as i32, d.y as i32);
        let left = self.has(&[ResizeEdge::Left, ResizeEdge::TopLeft, ResizeEdge::BottomLeft]);
        let right = self.has(&[ResizeEdge::Right, ResizeEdge::TopRight, ResizeEdge::BottomRight]);
        let top = self.has(&[ResizeEdge::Top, ResizeEdge::TopLeft, ResizeEdge::TopRight]);
        let bottom = self.has(&[ResizeEdge::Bottom, ResizeEdge::BottomLeft, ResizeEdge::BottomRight]);
        let mut w = self.size.w + if right { dx } else if left { -dx } else { 0 };
        let mut h = self.size.h + if bottom { dy } else if top { -dy } else { 0 };
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
    }

    fn button(&mut self, data: &mut App, handle: &mut PointerInnerHandle<'_, App>, event: &ButtonEvent) {
        handle.button(data, event);
        if !handle.current_pressed().contains(&self.start.button) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn start_data(&self) -> &GrabStartData<App> {
        &self.start
    }

    pass_through_gestures!();
}
