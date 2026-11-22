//! The touchscreen and the screen's pen, on the laptop's own screen (eDP, LVDS or DSI; the main screen when there is none or it is
//! off), as the Hyprland session maps them (`hl.device({ output = panel })`): a touch on the glass lands on that screen even when
//! a monitor is plugged in beside it.
//! - **Touch** goes to programs as wl_touch (down, motion, up, frame, cancel, one slot a finger): the seat gets its touch the
//!   first time a touchscreen is used. A finger put down on a window gives it the keyboard and raises it, as a click does.
//! - **The pen** moves the pointer, and its tip is the left button, its first side button the right one and its second the
//!   middle one, so every program takes it (tablet-v2, pressure and tilt for drawing programs, is not offered yet).
//!
//! The hardware backend gives these from libinput (udev.rs); the headless backend from its control socket (`touch-down`,
//! `touch-move`, `touch-up`), for the tests.

use smithay::backend::input::{
    AbsolutePositionEvent, ButtonState, Event, InputBackend, InputEvent, ProximityState, TabletToolButtonEvent, TabletToolProximityEvent,
    TabletToolTipEvent, TabletToolTipState, TouchEvent,
};
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, SERIAL_COUNTER};

use crate::state::{output_rect, App};

/// The pen's buttons as pointer buttons: BTN_STYLUS (0x14b) the right one, BTN_STYLUS2 (0x14c) the middle one.
pub fn pen_button(code: u32) -> u32 {
    match code {
        0x14b => 0x111,
        0x14c => 0x112,
        c => c,
    }
}

impl App {
    /// The screen a touchscreen and the screen's pen belong to: the laptop's own while it is on, else the main screen.
    pub fn touch_output(&self) -> Output {
        self.space.outputs().find(|o| crate::laptop::is_internal(&o.name())).cloned().unwrap_or_else(|| self.output.clone())
    }

    /// Where on the touch screen (in the space of all screens) a point given in that screen's own size lands.
    fn touch_area(&self) -> Rectangle<i32, Logical> {
        output_rect(&self.touch_output())
    }

    /// A finger went down at `pos` (in the space of all screens).
    pub fn touch_down(&mut self, slot: smithay::backend::input::TouchSlot, pos: Point<f64, Logical>, time: u32) {
        self.wake();
        self.idle_state.notify_activity(&self.seat);
        let touch = match self.seat.get_touch() {
            Some(t) => t,
            None => self.seat.add_touch(),
        };
        let serial = SERIAL_COUNTER.next_serial();
        self.focus_at(pos, serial);
        let under = self.surface_under(pos);
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!("touch down {slot:?} at {:.0},{:.0}", pos.x, pos.y);
        }
        touch.down(self, under, &DownEvent { slot, location: pos, serial, time });
    }

    pub fn touch_motion(&mut self, slot: smithay::backend::input::TouchSlot, pos: Point<f64, Logical>, time: u32) {
        self.wake();
        let Some(touch) = self.seat.get_touch() else { return };
        let under = self.surface_under(pos);
        touch.motion(self, under, &MotionEvent { slot, location: pos, time });
    }

    pub fn touch_up(&mut self, slot: smithay::backend::input::TouchSlot, time: u32) {
        self.wake();
        let Some(touch) = self.seat.get_touch() else { return };
        touch.up(self, &UpEvent { slot, serial: SERIAL_COUNTER.next_serial(), time });
    }

    /// The end of a group of touch changes (libinput says when; the headless commands after each one).
    pub fn touch_frame(&mut self) {
        if let Some(touch) = self.seat.get_touch() {
            touch.frame(self);
        }
    }

    pub fn touch_cancel(&mut self) {
        if let Some(touch) = self.seat.get_touch() {
            touch.cancel(self);
        }
    }

    /// Touch and pen input from a device (libinput): true when the event was one of these.
    pub fn on_touch<B: InputBackend>(&mut self, event: &InputEvent<B>) -> bool {
        let area = self.touch_area();
        let at = |p: Point<f64, Logical>| p + area.loc.to_f64();
        match event {
            InputEvent::TouchDown { event } => self.touch_down(event.slot(), at(event.position_transformed(area.size)), event.time_msec()),
            InputEvent::TouchMotion { event } => {
                self.touch_motion(event.slot(), at(event.position_transformed(area.size)), event.time_msec())
            }
            InputEvent::TouchUp { event } => self.touch_up(event.slot(), event.time_msec()),
            InputEvent::TouchFrame { .. } => self.touch_frame(),
            InputEvent::TouchCancel { .. } => self.touch_cancel(),
            InputEvent::TabletToolAxis { event } => self.pointer_to(at(event.position_transformed(area.size)), event.time_msec()),
            InputEvent::TabletToolProximity { event } => {
                if event.state() == ProximityState::In {
                    self.pointer_to(at(event.position_transformed(area.size)), event.time_msec());
                }
            }
            InputEvent::TabletToolTip { event } => {
                self.pointer_to(at(event.position_transformed(area.size)), event.time_msec());
                let state = match event.tip_state() {
                    TabletToolTipState::Down => ButtonState::Pressed,
                    TabletToolTipState::Up => ButtonState::Released,
                };
                self.button(0x110, state, event.time_msec());
            }
            InputEvent::TabletToolButton { event } => self.button(pen_button(event.button()), event.button_state(), event.time_msec()),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_pens_buttons_are_the_right_and_middle_ones() {
        assert_eq!(super::pen_button(0x14b), 0x111);
        assert_eq!(super::pen_button(0x14c), 0x112);
        assert_eq!(super::pen_button(0x110), 0x110);
    }
}
