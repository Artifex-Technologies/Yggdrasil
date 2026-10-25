//! Find my mouse (after PowerToys' Mouse Utilities): Ctrl pressed twice puts a spotlight on the pointer. The compositor tells the
//! panel each time Ctrl is tapped on its own (`yggdrasil-panelctl cursor-tap`, a release bind on Ctrl); two taps close together dim
//! the screen the pointer is on, a circle closes in on the pointer, and the whole thing fades away by itself after about a second
//! and a half. It takes no clicks and no keys, and nothing runs once it has gone.

use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use resvg::tiny_skia::{BlendMode, FillRule, Paint, PathBuilder, Pixmap, Transform};

use crate::paint::{self, Rgba};

/// Two taps within this long are a double tap.
pub const DOUBLE_TAP: Duration = Duration::from_millis(400);
/// The circle closes in over this long, stays a moment, and everything fades out over the last part.
const SHRINK: Duration = Duration::from_millis(450);
const FADE: Duration = Duration::from_millis(400);
pub const LENGTH: Duration = Duration::from_millis(450 + 550 + 400);
/// The spotlight's final radius around the pointer (logical pixels).
const RADIUS: f32 = 70.0;

/// Times Ctrl taps: the second within DOUBLE_TAP of the first is a double tap (and starts counting afresh).
#[derive(Default)]
pub struct Taps {
    last: Option<Instant>,
}

impl Taps {
    pub fn tap(&mut self, now: Instant) -> bool {
        match self.last {
            Some(t) if now.saturating_duration_since(t) <= DOUBLE_TAP => {
                self.last = None;
                true
            }
            _ => {
                self.last = Some(now);
                false
            }
        }
    }
}

/// A spotlight being shown: on which screen (its name), where the pointer was (logical pixels on that screen), since when.
#[derive(Clone, Debug)]
pub struct Spot {
    pub output: String,
    pub x: f32,
    pub y: f32,
    pub start: Instant,
}

impl Spot {
    pub fn done(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= LENGTH
    }
}

fn hyprctl_json(what: &str) -> Option<serde_json::Value> {
    let out = Command::new("hyprctl").args(["-j", what]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    serde_json::from_slice(&out.stdout).ok()
}

/// The screen under the pointer from Hyprland's monitor list (`monitors -j`), and the pointer's place on it in logical pixels.
pub fn on_monitor(cursor: (f64, f64), monitors: &serde_json::Value) -> Option<(String, f32, f32)> {
    for m in monitors.as_array()? {
        let scale = m["scale"].as_f64().filter(|s| *s > 0.0).unwrap_or(1.0);
        let (mx, my) = (m["x"].as_f64()?, m["y"].as_f64()?);
        // a screen turned sideways (transform 1, 3, 5, 7) is as wide as it is tall
        let sideways = m["transform"].as_u64().unwrap_or(0) % 2 == 1;
        let (pw, ph) = (m["width"].as_f64()?, m["height"].as_f64()?);
        let (w, h) = if sideways { (ph / scale, pw / scale) } else { (pw / scale, ph / scale) };
        if cursor.0 >= mx && cursor.0 < mx + w && cursor.1 >= my && cursor.1 < my + h {
            return Some((m["name"].as_str()?.to_string(), (cursor.0 - mx) as f32, (cursor.1 - my) as f32));
        }
    }
    None
}

/// Where the pointer is now (Hyprland only).
pub fn locate() -> Option<(String, f32, f32)> {
    let c = hyprctl_json("cursorpos")?;
    let cursor = (c["x"].as_f64()?, c["y"].as_f64()?);
    on_monitor(cursor, &hyprctl_json("monitors")?)
}

fn ease_out(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

/// The spotlight at `now` on a surface the size of the screen (`s` is its scale): the screen dimmed, a clear circle around the
/// pointer closing in on it, a thin ring in the accent colour.
pub fn paint(pix: &mut Pixmap, s: f32, spot: &Spot, now: Instant) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let t = now.saturating_duration_since(spot.start);
    let fade = if t + FADE > LENGTH { 1.0 - (t + FADE - LENGTH).as_secs_f32() / FADE.as_secs_f32() } else { 1.0 };
    let fade = fade.clamp(0.0, 1.0) * ease_out(t.as_secs_f32() / 0.15);
    if fade <= 0.0 {
        return;
    }
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    let start_r = (w * w + h * h).sqrt() / s;
    let r = start_r + (RADIUS - start_r) * ease_out(t.as_secs_f32() / SHRINK.as_secs_f32());
    let dim: Rgba = (0, 0, 0, (150.0 * fade) as u8);
    pix.fill(paint::color(dim));
    let (cx, cy) = (spot.x * s, spot.y * s);
    if let Some(circle) = PathBuilder::from_circle(cx, cy, r * s) {
        let mut clear = Paint::default();
        clear.blend_mode = BlendMode::Clear;
        clear.anti_alias = true;
        pix.fill_path(&circle, &clear, FillRule::Winding, Transform::identity(), None);
    }
    paint::stroke_arc(pix, cx, cy, r * s, 3.0 * s, 1.0, paint::accent((230.0 * fade) as u8));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_taps_close_together_are_a_double_tap() {
        let mut t = Taps::default();
        let t0 = Instant::now();
        assert!(!t.tap(t0));
        assert!(t.tap(t0 + Duration::from_millis(300)));
        // a third tap starts again
        assert!(!t.tap(t0 + Duration::from_millis(500)));
        // too slow
        assert!(!t.tap(t0 + Duration::from_millis(1200)));
        assert!(t.tap(t0 + Duration::from_millis(1500)));
    }

    #[test]
    fn the_pointer_is_found_on_its_screen() {
        let mons = serde_json::json!([
            {"name": "DP-1", "x": 0, "y": 0, "width": 3440, "height": 1440, "scale": 1.0, "transform": 0},
            {"name": "HDMI-A-1", "x": 3440, "y": 0, "width": 3840, "height": 2160, "scale": 2.0, "transform": 1},
        ]);
        assert_eq!(on_monitor((100.0, 50.0), &mons), Some(("DP-1".into(), 100.0, 50.0)));
        // the second screen: 1080 x 1920 logical once turned
        assert_eq!(on_monitor((3440.0 + 500.0, 1500.0), &mons), Some(("HDMI-A-1".into(), 500.0, 1500.0)));
        assert_eq!(on_monitor((3440.0 + 1200.0, 10.0), &mons), None);
    }

    #[test]
    fn the_spotlight_ends_and_leaves_the_pointer_clear() {
        let start = Instant::now();
        let spot = Spot { output: "DP-1".into(), x: 100.0, y: 100.0, start };
        let mut pix = Pixmap::new(400, 300).unwrap();
        paint(&mut pix, 1.0, &spot, start + Duration::from_millis(700));
        assert_eq!(pix.pixel(100, 100).unwrap().alpha(), 0, "the pointer itself stays clear");
        assert!(pix.pixel(390, 290).unwrap().alpha() > 100, "the rest is dimmed");
        assert!(spot.done(start + LENGTH));
        assert!(!spot.done(start + Duration::from_millis(500)));
    }
}
