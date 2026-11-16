//! Where a floating window goes: the same rules as the Hyprland session (`packages/session/hyprland/yggdrasil.lua` and
//! `keep-clear.lua`), so both compositors place windows alike:
//! - a new window opens centred in the room the bar and the dock leave free (Hyprland: `float, center` plus keep-clear); one
//!   bigger than that room is made as big as the room less the outer gap and centred in it (keep-clear's `open`), and so is a
//!   remembered place that no longer fits its screen (the scale changed since);
//! - a window keeps clear of the bar and the dock: one that reaches under them is moved out (and shortened if it must be);
//! - a window whose screen gets smaller (another mode or scale), or goes, is pulled back inside the free room of the screen it
//!   is on, bare screen edges too (made to fit along a side where it has become longer than the room);
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

/// A window that covers its whole screen exactly (a borderless game or a video), give or take a pixel.
fn covers(win: Rect, screen: Rect) -> bool {
    let close = |a: i32, b: i32| (a - b).abs() <= 1;
    close(win.loc.x, screen.loc.x) && close(win.loc.y, screen.loc.y) && close(win.size.w, screen.size.w) && close(win.size.h, screen.size.h)
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
    if covers(win, screen) {
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

/// A window as it opens (keep-clear.lua's `open`): one bigger than the free room is made as big as that room less `gap` (the
/// session's gaps_out; none when the room would be under 100 less it) and centred in it; one that fits is only kept clear of
/// the bar and the dock (`fit`). None when it may stay as it is; a window covering its whole screen is left alone.
pub fn open(win: Rect, screen: Rect, zone: Rect, gap: i32) -> Option<Rect> {
    if covers(win, screen) {
        return None;
    }
    let (fw, fh) = (zone.size.w, zone.size.h);
    if fw < 100 || fh < 100 || (win.size.w <= fw && win.size.h <= fh) {
        return fit(win, screen, zone);
    }
    let mut gap = gap.clamp(0, 64);
    if fw - 2 * gap < 100 || fh - 2 * gap < 100 {
        gap = 0;
    }
    let (x0, y0, fw, fh) = (zone.loc.x + gap, zone.loc.y + gap, fw - 2 * gap, fh - 2 * gap);
    let (nw, nh) = (win.size.w.min(fw), win.size.h.min(fh));
    let round = |v: f64| v.round() as i32;
    let out = rect(x0 + round((fw - nw) as f64 / 2.0), y0 + round((fh - nh) as f64 / 2.0), nw, nh);
    (out != win).then_some(out)
}

/// A window whose screen changed under it (a smaller mode, another scale, or its screen gone and the window moved to another):
/// pulled back inside the free room on every side, bare screen edges too, so none of it is left off the screen, and kept clear
/// of the bar and the dock. It keeps its place where it can; along a side where it is longer than the room it is made as long as
/// the room less `gap` (as `open`; none when the room would be under 100 less it) and centred. None when it is inside already;
/// a window covering its whole screen is left alone.
pub fn inside(win: Rect, screen: Rect, zone: Rect, gap: i32) -> Option<Rect> {
    if covers(win, screen) || zone.size.w < 1 || zone.size.h < 1 {
        return None;
    }
    let gap = gap.clamp(0, 64);
    let axis = |p: i32, len: i32, lo: i32, room: i32| {
        if len <= room {
            return fit_axis(p, len, lo, lo + room);
        }
        let gap = if room - 2 * gap < 100 { 0 } else { gap };
        (lo + gap, room - 2 * gap)
    };
    let (x, w) = axis(win.loc.x, win.size.w, zone.loc.x, zone.size.w);
    let (y, h) = axis(win.loc.y, win.size.h, zone.loc.y, zone.size.h);
    let out = rect(x, y, w, h);
    (out != win).then_some(out)
}

/// A window back from maximize or full screen: when it still covers nearly all the free room (90 % each way), a normal size,
/// three quarters of the room, centred in it. None when its own size is fine.
pub fn normal(win: Rect, screen: Rect, zone: Rect) -> Option<Rect> {
    if covers(win, screen) {
        return None;
    }
    let (fw, fh) = (zone.size.w as f64, zone.size.h as f64);
    if fw < 100.0 || fh < 100.0 || (win.size.w as f64) < 0.9 * fw || (win.size.h as f64) < 0.9 * fh {
        return None;
    }
    let (nw, nh) = ((fw * 0.75).round() as i32, (fh * 0.75).round() as i32);
    Some(centred((nw, nh), zone))
}

/// A window of `size` centred in the free room (one bigger than it hangs over: `open` then fits it).
pub fn centred(size: (i32, i32), zone: Rect) -> Rect {
    let (w, h) = (size.0.max(1), size.1.max(1));
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
    fn a_window_opens_no_bigger_than_its_screen() {
        // the same cases as keep-clear.lua's open(): the laptop's 1920x1200 at 1.5 (1280x800 to lay out), bar 28, dock 70
        let (s, z) = (rect(0, 0, 1280, 800), rect(0, 28, 1280, 702));
        assert_eq!(open(rect(100, 50, 1208, 806), s, z, 8), Some(rect(36, 36, 1208, 686)), "kept size from before the scale changed");
        assert_eq!(open(rect(-110, 200, 1500, 400), s, z, 8), Some(rect(8, 179, 1264, 400)), "wider than the screen");
        assert_eq!(open(rect(-200, -100, 1680, 1000), s, z, 8), Some(rect(8, 36, 1264, 686)), "bigger both ways");
        assert_eq!(open(rect(374, 311, 532, 177), s, z, 8), None, "a normal dialog is left alone");
        assert_eq!(open(rect(0, 28, 1280, 702), s, z, 8), None, "as big as the free room exactly is left alone");
        assert_eq!(open(rect(100, 500, 600, 300), s, z, 8), Some(rect(100, 430, 600, 300)), "fits, but under the dock");
        assert_eq!(open(rect(0, 0, 1280, 800), s, z, 8), None, "borderless full screen as it opens");
        assert_eq!(open(rect(-110, 0, 1500, 900), s, z, 0), Some(rect(0, 28, 1280, 702)), "no gap");
        // scale 2 (960x600 to lay out)
        let (s, z) = (rect(0, 0, 960, 600), rect(0, 28, 960, 502));
        assert_eq!(open(rect(180, 20, 600, 560), s, z, 8), Some(rect(180, 36, 600, 486)), "tall dialog at scale 2");
        // a screen with nothing reserved
        let s = rect(1280, 0, 1280, 800);
        assert_eq!(open(rect(1220, -50, 1400, 900), s, s, 8), Some(rect(1288, 8, 1264, 784)), "no bar or dock");
        // turned into a tablet (portrait, 800x1280 to lay out)
        let (s, z) = (rect(0, 0, 800, 1280), rect(0, 28, 800, 1182));
        assert_eq!(open(rect(-204, 216, 1208, 806), s, z, 8), Some(rect(8, 216, 784, 806)), "portrait screen");
    }

    #[test]
    fn a_window_is_pulled_back_inside_a_screen_that_changed() {
        // 1920x1080 became 1280x1024 (bar 32, dock 64): Settings at 430,164 1060x720 hung 210 off the right edge
        let (s, z) = (rect(0, 0, 1280, 1024), rect(0, 32, 1280, 928));
        assert_eq!(inside(rect(430, 164, 1060, 720), s, z, 8), Some(rect(220, 164, 1060, 720)), "moved in, its size and height kept");
        assert_eq!(fit(rect(430, 164, 1060, 720), s, z), None, "a bare edge alone would have let it hang off");
        assert_eq!(inside(rect(100, 500, 600, 480), s, z, 8), Some(rect(100, 480, 600, 480)), "out of the dock's room");
        assert_eq!(inside(rect(-300, 0, 600, 400), s, z, 8), Some(rect(0, 32, 600, 400)), "off the left edge and under the bar");
        assert_eq!(inside(rect(100, 100, 600, 400), s, z, 8), None, "inside already");
        assert_eq!(inside(rect(0, 0, 1280, 1024), s, z, 8), None, "borderless full screen");
        // scale 2 on top (640x512 to lay out): longer than the room both ways, the room less the gap
        let (s, z) = (rect(0, 0, 640, 512), rect(0, 32, 640, 416));
        assert_eq!(inside(rect(220, 164, 1060, 720), s, z, 8), Some(rect(8, 40, 624, 400)));
        // longer one way only: that way the room less the gap, the other way its place kept
        assert_eq!(inside(rect(20, 100, 900, 200), s, z, 8), Some(rect(8, 100, 624, 200)));
        // a screen to the right of another, nothing reserved
        let s = rect(1280, 0, 800, 600);
        assert_eq!(inside(rect(1900, -40, 400, 300), s, s, 8), Some(rect(1680, 0, 400, 300)));
        // a room too small for the gap: all of it
        let (s, z) = (rect(0, 0, 110, 150), rect(0, 20, 110, 110));
        assert_eq!(inside(rect(0, 0, 300, 300), s, z, 8), Some(rect(0, 20, 110, 110)));
    }

    #[test]
    fn new_windows_open_centred_in_the_free_room() {
        let (_, z) = ipad();
        assert_eq!(centred((600, 400), z), rect(305, 196, 600, 400));
        assert_eq!(centred((2000, 2000), z), rect(-395, -604, 2000, 2000), "too big: centred over it");
        let (s, _) = ipad();
        assert_eq!(open(centred((2000, 2000), z), s, z, 8), Some(rect(8, 36, 1194, 720)), "then made to fit, less the gap");
    }
}
