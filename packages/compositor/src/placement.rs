//! Where a floating window goes: the same rules as the Hyprland session (`packages/session/hyprland/yggdrasil.lua` and
//! `keep-clear.lua`), so both compositors place windows alike:
//! - a new window opens centred in the room the bar and the dock leave free (Hyprland: `float, center` plus keep-clear), and
//!   is shortened to that room when it asks for more;
//! - a window keeps clear of the bar and the dock: one that reaches under them is moved out (and shortened if it must be);
//! - a window back from maximize or full screen that still covers nearly all the free room gets a normal size: three quarters
//!   of the room each way, centred.
//!
//! Pure functions on rectangles in the layout's coordinates; `packages/session/tests/keep-clear.lua` checks the Lua original with
//! the same cases as the tests below.

use smithay::utils::{Logical, Rectangle};

pub type Rect = Rectangle<i32, Logical>;

fn rect(x: i32, y: i32, w: i32, h: i32) -> Rect {
    Rectangle::new((x, y).into(), (w, h).into())
}

/// One axis: a window from p, len long, kept inside lo..hi (shortened when it does not fit).
fn fit_axis(p: i32, len: i32, lo: i32, hi: i32) -> (i32, i32) {
    let len = len.min(hi - lo);
    let p = if p + len > hi { hi - len } else { p };
    (p.max(lo), len)
}

/// The window `win` on a screen `screen` whose free room is `zone`. None when it may stay where it is. Only edges with something
/// reserved on them count (a window may still hang off a bare screen edge), and a window that covers its whole screen exactly (a
/// borderless game or a video) is left alone.
pub fn fit(win: Rect, screen: Rect, zone: Rect) -> Option<Rect> {
    let close = |a: i32, b: i32| (a - b).abs() <= 1;
    if close(win.loc.x, screen.loc.x)
        && close(win.loc.y, screen.loc.y)
        && close(win.size.w, screen.size.w)
        && close(win.size.h, screen.size.h)
    {
        return None;
    }
    let (top, left) = (zone.loc.y - screen.loc.y, zone.loc.x - screen.loc.x);
    let bottom = screen.loc.y + screen.size.h - (zone.loc.y + zone.size.h);
    let right = screen.loc.x + screen.size.w - (zone.loc.x + zone.size.w);
    let (x0, x1, y0, y1) = (zone.loc.x, zone.loc.x + zone.size.w, zone.loc.y, zone.loc.y + zone.size.h);
    if x1 - x0 < 100 || y1 - y0 < 100 {
        return None;
    }
    let (mut x, mut y, mut w, mut h) = (win.loc.x, win.loc.y, win.size.w, win.size.h);
    if (top > 0 && y < y0) || (bottom > 0 && y + h > y1) {
        (y, h) = fit_axis(y, h, y0, y1);
    }
    if (left > 0 && x < x0) || (right > 0 && x + w > x1) {
        (x, w) = fit_axis(x, w, x0, x1);
    }
    let out = rect(x, y, w, h);
    (out != win).then_some(out)
}

/// A window back from maximize or full screen: when it still covers nearly all the free room (90 % each way), a normal size,
/// three quarters of the room, centred in it. None when its own size is fine.
pub fn normal(win: Rect, screen: Rect, zone: Rect) -> Option<Rect> {
    let close = |a: i32, b: i32| (a - b).abs() <= 1;
    if close(win.loc.x, screen.loc.x)
        && close(win.loc.y, screen.loc.y)
        && close(win.size.w, screen.size.w)
        && close(win.size.h, screen.size.h)
    {
        return None;
    }
    let (fw, fh) = (zone.size.w as f64, zone.size.h as f64);
    if fw < 100.0 || fh < 100.0 || (win.size.w as f64) < 0.9 * fw || (win.size.h as f64) < 0.9 * fh {
        return None;
    }
    let (nw, nh) = ((fw * 0.75).round() as i32, (fh * 0.75).round() as i32);
    Some(centred((nw, nh), zone))
}

/// A window of `size` centred in the free room, shortened to it when it is bigger.
pub fn centred(size: (i32, i32), zone: Rect) -> Rect {
    let (w, h) = (size.0.min(zone.size.w).max(1), size.1.min(zone.size.h).max(1));
    let round = |v: f64| v.round() as i32;
    rect(zone.loc.x + round((zone.size.w - w) as f64 / 2.0), zone.loc.y + round((zone.size.h - h) as f64 / 2.0), w, h)
}

#[cfg(test)]
mod tests {
    use super::*;

    // the same screens as packages/session/tests/keep-clear.lua, in layout pixels
    // the iPad screen: 1210x834 to lay out, the bar 28 high, the dock 70
    fn ipad() -> (Rect, Rect) {
        (rect(0, 0, 1210, 834), rect(0, 28, 1210, 834 - 28 - 70))
    }
    // the big screen to its right, bar only
    fn wide() -> (Rect, Rect) {
        (rect(1210, 0, 3440, 1440), rect(1210, 28, 3440, 1440 - 28))
    }

    #[test]
    fn windows_keep_clear_of_the_bar_and_dock() {
        let (s, z) = ipad();
        assert_eq!(fit(rect(0, 28, 1210, 806), s, z), Some(rect(0, 28, 1210, 736)), "VS Code back from maximize");
        assert_eq!(fit(rect(300, 500, 600, 300), s, z), Some(rect(300, 464, 600, 300)), "small window over the dock");
        assert_eq!(fit(rect(100, 10, 500, 400), s, z), Some(rect(100, 28, 500, 400)), "under the bar");
        assert_eq!(fit(rect(100, 100, 500, 400), s, z), None, "clear already");
        assert_eq!(fit(rect(0, 28, 1210, 736), s, z), None, "touching the dock's edge");
        assert_eq!(fit(rect(0, 0, 1210, 834), s, z), None, "borderless full screen");
        assert_eq!(fit(rect(-200, 100, 500, 400), s, z), None, "off a bare edge");
        let (s, z) = wide();
        assert_eq!(fit(rect(1300, 28, 2000, 1412), s, z), None, "tall window, no dock");
        assert_eq!(fit(rect(1300, 0, 2000, 1412), s, z), Some(rect(1300, 28, 2000, 1412)), "under the bar of the big screen");
        assert_eq!(fit(rect(1300, 0, 2000, 1440), s, z), Some(rect(1300, 28, 2000, 1412)), "taller than the room");
        // the dock on the left side
        let (s, z) = (rect(0, 0, 1920, 1080), rect(70, 28, 1850, 1052));
        assert_eq!(fit(rect(10, 100, 800, 600), s, z), Some(rect(70, 100, 800, 600)), "left dock");
        assert_eq!(fit(rect(0, 100, 1920, 600), s, z), Some(rect(70, 100, 1850, 600)), "wider than the room");
    }

    #[test]
    fn back_from_maximize_a_window_gets_a_normal_size() {
        let (s, z) = ipad();
        assert_eq!(normal(rect(0, 28, 1210, 806), s, z), Some(rect(151, 120, 908, 552)), "VS Code's old size under the dock");
        assert_eq!(normal(rect(0, 29, 1208, 734), s, z), Some(rect(151, 120, 908, 552)), "Vivaldi at the maximized size");
        assert_eq!(normal(rect(200, 100, 700, 500), s, z), None, "a normal window keeps its size");
        assert_eq!(normal(rect(0, 28, 1210, 400), s, z), None, "wide but short keeps its size");
        assert_eq!(normal(rect(0, 0, 1210, 834), s, z), None, "the whole screen (a game) is left alone");
    }

    #[test]
    fn new_windows_open_centred_in_the_free_room() {
        let (_, z) = ipad();
        assert_eq!(centred((600, 400), z), rect(305, 196, 600, 400));
        assert_eq!(centred((2000, 2000), z), z, "too big: shortened to the room");
    }
}
