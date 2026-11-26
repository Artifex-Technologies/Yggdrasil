//! The touchscreen and the screen's pen, on the laptop's own screen (eDP, LVDS or DSI; the main screen when there is none or it is
//! off), as the Hyprland session maps them (`hl.device({ output = panel })`): a touch on the glass lands on that screen even when
//! a monitor is plugged in beside it.
//! - **Touch** goes to programs as wl_touch (down, motion, up, frame, one slot a finger): the seat gets its touch the first time
//!   a touchscreen is used. A finger put down on a window gives it the keyboard and raises it, as a click does. With a finger
//!   still down on its own window a program may move or resize that window, which then follows the finger (grabs.rs), or start
//!   a drag, whose icon is drawn under that finger (`App::fingers`; the pointer keeps its own look). The touchscreen's cancel
//!   ends every finger and a move, resize or drag made with one, and the programs with a finger down are told (wl_touch.cancel,
//!   also when it comes in a frame of its own, as libinput's does: our Smithay fork); XWayland then ends an X11 program's
//!   finger, which it hears as the release, during its finger drag too.
//! - **The pen** moves the pointer, and its tip is the left button, its first side button the right one and its second the
//!   middle one, so every program takes it (tablet-v2, pressure and tilt for drawing programs, is not offered yet).
//!
//! A turned screen (`transform`, the tablet watcher's in a 2-in-1's tablet mode) turns where a touch or the pen lands the same way:
//! libinput tells the point across the glass as it is built, and `on_glass` finds it in the turned picture.
//!
//! **A screen that is off** (idle turned it off; the laptop is awake): a single touch, a brush in a bag, wakes nothing and
//! reaches no program; two taps within half a second (`DOUBLE_TAP`) turn the screens on, and the lock screen shows if it is
//! locked. Both taps are the gesture and reach no program. A suspend is different: the touchscreen wakes the laptop by itself.
//!
//! The hardware backend gives these from libinput (udev.rs); the headless backend from its control socket (`touch-down`,
//! `touch-move`, `touch-up`, `touch-cancel`), for the tests.

use smithay::backend::input::InputTime;
use smithay::backend::input::{
    AbsolutePositionEvent, ButtonState, Event, InputBackend, InputEvent, ProximityState, TabletToolButtonEvent, TabletToolProximityEvent,
    TabletToolTipEvent, TabletToolTipState, TouchEvent, TouchSlot,
};
use smithay::input::touch::{DownEvent, MotionEvent, UpEvent};
use smithay::output::Output;
use smithay::utils::{Logical, Point, Rectangle, Serial, Size, Transform, SERIAL_COUNTER};

use crate::state::{output_rect, App};

/// The pen's buttons as pointer buttons: BTN_STYLUS (0x14b) the right one, BTN_STYLUS2 (0x14c) the middle one.
pub fn pen_button(code: u32) -> u32 {
    match code {
        0x14b => 0x111,
        0x14c => 0x112,
        c => c,
    }
}

/// Where a point on the glass lands in the picture of a screen of `size` (logical, as the picture is turned): `u` and `v` are
/// libinput's, from 0 to 1 across and down the glass as it is built. A turn is wl_output's: the picture turned counter-clockwise
/// onto the glass (after a mirror for the flipped ones), so a touch is turned back.
pub fn on_glass(t: Transform, size: Size<i32, Logical>, u: f64, v: f64) -> Point<f64, Logical> {
    let (w, h) = (size.w as f64, size.h as f64);
    let (x, y) = match t {
        Transform::Normal => (u, v),
        Transform::_90 => (1.0 - v, u),
        Transform::_180 => (1.0 - u, 1.0 - v),
        Transform::_270 => (v, 1.0 - u),
        Transform::Flipped => (1.0 - u, v),
        Transform::Flipped90 => (v, u),
        Transform::Flipped180 => (u, 1.0 - v),
        Transform::Flipped270 => (1.0 - v, 1.0 - u),
    };
    (x * w, y * h).into()
}

/// A finger down on the touchscreen that goes to the programs (`App::fingers`; one on a screen that is off does not).
pub struct Finger {
    pub slot: TouchSlot,
    /// the serial it went down with: its program is told it, and asks with it to move, resize or drag (`App::finger_down`)
    pub serial: Serial,
    /// where it is now: a drag made with it draws its icon there
    pub at: Point<f64, Logical>,
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
    pub fn touch_down(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time: InputTime) {
        self.wake();
        if !self.power.is_on(&self.touch_output()) {
            self.tap_on_dark_screen(slot, std::time::Instant::now());
            return;
        }
        self.idle_state.notify_activity(&self.seat);
        let touch = match self.seat.get_touch() {
            Some(t) => t,
            None => self.seat.add_touch(),
        };
        let serial = SERIAL_COUNTER.next_serial();
        self.focus_at(pos, serial);
        let under = self.target_under(pos);
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!("touch down {slot:?} at {:.0},{:.0}", pos.x, pos.y);
        }
        self.fingers.retain(|f| f.slot != slot);
        self.fingers.push(Finger { slot, serial, at: pos });
        touch.down(self, under, &DownEvent { slot, location: pos, serial, time });
    }

    pub fn touch_motion(&mut self, slot: TouchSlot, pos: Point<f64, Logical>, time: InputTime) {
        self.wake();
        if self.touch_swallowed.contains(&slot) {
            return;
        }
        let Some(touch) = self.seat.get_touch() else { return };
        if let Some(f) = self.fingers.iter_mut().find(|f| f.slot == slot) {
            f.at = pos;
        }
        let under = self.target_under(pos);
        touch.motion(self, under, &MotionEvent { slot, location: pos, time });
    }

    pub fn touch_up(&mut self, slot: TouchSlot, time: InputTime) {
        self.wake();
        if let Some(i) = self.touch_swallowed.iter().position(|s| *s == slot) {
            self.touch_swallowed.remove(i);
            return;
        }
        self.fingers.retain(|f| f.slot != slot);
        let Some(touch) = self.seat.get_touch() else { return };
        touch.up(self, &UpEvent { slot, serial: SERIAL_COUNTER.next_serial(), time });
    }

    /// The finger that went down with `serial` is still down. A program asks to move, resize or drag with a finger's serial, and
    /// Smithay keeps the first finger's (the touchscreen's grab, with that finger's slot) until every finger is up: without this a
    /// program could ask with a finger that has lifted while another is still down (a palm beside it, or a program late with its
    /// request), and what it asked for would wait for the next finger in that slot, wherever that went down.
    pub fn finger_down(&self, serial: Serial) -> bool {
        self.fingers.iter().any(|f| f.serial == serial)
    }

    /// A finger down while the touchscreen's screen is off: the first tap is remembered, a second within `DOUBLE_TAP` turns
    /// every screen that is off on again and counts as activity (idle starts over, the lock screen shows). Neither reaches a
    /// program, nor does what the finger does until it lifts.
    fn tap_on_dark_screen(&mut self, slot: TouchSlot, now: std::time::Instant) {
        self.touch_swallowed.push(slot);
        if !second_tap(self.touch_wake_at, now) {
            self.touch_wake_at = Some(now);
            return;
        }
        self.touch_wake_at = None;
        let off: Vec<Output> = self.space.outputs().filter(|o| !self.power.is_on(o)).cloned().collect();
        for o in &off {
            self.set_output_power(o, true);
        }
        self.idle_state.notify_activity(&self.seat);
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!("two taps on a dark screen: {} screen(s) on", off.len());
        }
    }

    /// The end of a group of touch changes (libinput says when; the headless commands after each one).
    pub fn touch_frame(&mut self) {
        if let Some(touch) = self.seat.get_touch() {
            touch.frame(self);
        }
    }

    pub fn touch_cancel(&mut self) {
        self.touch_swallowed.clear();
        self.fingers.clear();
        if let Some(touch) = self.seat.get_touch() {
            touch.cancel(self);
        }
    }

    /// Touch and pen input from a device (libinput): true when the event was one of these.
    pub fn on_touch<B: InputBackend>(&mut self, event: &InputEvent<B>) -> bool {
        let area = self.touch_area();
        let turn = self.touch_output().current_transform();
        // libinput's point across the glass (0 to 1), in the turned picture, in the space of all screens
        let at = |p: Point<f64, Logical>| on_glass(turn, area.size, p.x, p.y) + area.loc.to_f64();
        // libinput gives a point scaled to a size: 1 by 1 gives it from 0 to 1, as the glass is built
        let glass: Size<i32, Logical> = (1, 1).into();
        match event {
            InputEvent::TouchDown { event } => self.touch_down(event.slot(), at(event.position_transformed(glass)), event.time()),
            InputEvent::TouchMotion { event } => self.touch_motion(event.slot(), at(event.position_transformed(glass)), event.time()),
            InputEvent::TouchUp { event } => self.touch_up(event.slot(), event.time()),
            InputEvent::TouchFrame { .. } => self.touch_frame(),
            InputEvent::TouchCancel { .. } => self.touch_cancel(),
            InputEvent::TabletToolAxis { event } => self.pointer_to(at(event.position_transformed(glass)), event.time()),
            InputEvent::TabletToolProximity { event } => {
                if event.state() == ProximityState::In {
                    self.pointer_to(at(event.position_transformed(glass)), event.time());
                }
            }
            InputEvent::TabletToolTip { event } => {
                self.pointer_to(at(event.position_transformed(glass)), event.time());
                let state = match event.tip_state() {
                    TabletToolTipState::Down => ButtonState::Pressed,
                    TabletToolTipState::Up => ButtonState::Released,
                };
                self.button(0x110, state, event.time());
            }
            InputEvent::TabletToolButton { event } => self.button(pen_button(event.button()), event.button_state(), event.time()),
            _ => return false,
        }
        true
    }
}

/// Two taps on the touchscreen within this while its screen is off turn the screens on (YGG-102).
pub const DOUBLE_TAP: std::time::Duration = std::time::Duration::from_millis(500);

/// Is a tap at `now` the second of a double tap, after a first at `first`?
pub fn second_tap(first: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    first.is_some_and(|t| now.saturating_duration_since(t) <= DOUBLE_TAP)
}

#[cfg(test)]
mod tests {
    #[test]
    fn two_taps_within_half_a_second_wake_a_dark_screen() {
        use std::time::{Duration, Instant};
        let t = Instant::now();
        assert!(!super::second_tap(None, t), "a first tap alone wakes nothing");
        assert!(super::second_tap(Some(t), t + Duration::from_millis(300)));
        assert!(super::second_tap(Some(t), t + Duration::from_millis(500)));
        assert!(!super::second_tap(Some(t), t + Duration::from_millis(501)), "too far apart: a new first tap");
    }

    #[test]
    fn the_pens_buttons_are_the_right_and_middle_ones() {
        assert_eq!(super::pen_button(0x14b), 0x111);
        assert_eq!(super::pen_button(0x14c), 0x112);
        assert_eq!(super::pen_button(0x110), 0x110);
    }

    #[test]
    fn a_touch_on_a_turned_screen_lands_under_the_finger() {
        use smithay::utils::Transform;
        let near =
            |p: smithay::utils::Point<f64, smithay::utils::Logical>, x: f64, y: f64| (p.x - x).abs() < 1e-9 && (p.y - y).abs() < 1e-9;
        // the X1 Yoga's glass is 1920x1200 at 1.25: 1536x960 upright, 960x1536 turned a quarter
        let wide: smithay::utils::Size<i32, smithay::utils::Logical> = (1536, 960).into();
        let tall: smithay::utils::Size<i32, smithay::utils::Logical> = (960, 1536).into();
        // upright: as it is
        assert!(near(super::on_glass(Transform::Normal, wide, 0.25, 0.5), 384.0, 480.0));
        // held with its left edge up (the reading "left-up", turn 1): the glass's bottom left corner is now the top left one the
        // eye sees, and its top left the top right
        assert!(near(super::on_glass(Transform::_90, tall, 0.0, 1.0), 0.0, 0.0));
        assert!(near(super::on_glass(Transform::_90, tall, 0.0, 0.0), 960.0, 0.0));
        assert!(near(super::on_glass(Transform::_90, tall, 1.0, 1.0), 0.0, 1536.0));
        // upside down (turn 2): every corner the opposite one
        assert!(near(super::on_glass(Transform::_180, wide, 0.0, 0.0), 1536.0, 960.0));
        // held with its right edge up (turn 3): the glass's top right corner is the top left one seen
        assert!(near(super::on_glass(Transform::_270, tall, 1.0, 0.0), 0.0, 0.0));
        assert!(near(super::on_glass(Transform::_270, tall, 0.0, 0.0), 0.0, 1536.0));
        // the middle stays the middle, whatever the turn
        for t in [Transform::Normal, Transform::_90, Transform::_180, Transform::_270, Transform::Flipped, Transform::Flipped90] {
            let size = if matches!(t, Transform::_90 | Transform::_270 | Transform::Flipped90) { tall } else { wide };
            assert!(near(super::on_glass(t, size, 0.5, 0.5), size.w as f64 / 2.0, size.h as f64 / 2.0), "{t:?}");
        }
    }
}
