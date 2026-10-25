//! Window zones, in the spirit of PowerToys' FancyZones: drag a window to the notch (Fylgja) at the top of the screen and a sheet of
//! layouts opens; drop it on a zone and it fills that part of the screen. `~/.config/yggdrasil/zones.conf`, written by
//! Settings > Displays > Window zones, read by the panel:
//!
//!   enabled = yes
//!   layouts = halves, thirds, two-thirds-left, quarters      # which layouts the sheet shows, in this order
//!   gap = 8                                                  # space around and between zones, in logical pixels
//!
//! Zones are fractions of the screen's free area (what the bar and the dock leave), so a layout fits any screen.

use std::path::PathBuf;

/// One zone: left, top, width and height as fractions (0 to 1) of the free area.
pub type Zone = (f32, f32, f32, f32);

#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub id: &'static str,
    pub name: &'static str,
    pub zones: Vec<Zone>,
}

/// The built-in layouts, in the order Settings lists them.
pub fn builtin() -> Vec<Layout> {
    const T: f32 = 1.0 / 3.0;
    vec![
        Layout { id: "halves", name: "Halves", zones: vec![(0.0, 0.0, 0.5, 1.0), (0.5, 0.0, 0.5, 1.0)] },
        Layout { id: "thirds", name: "Thirds", zones: vec![(0.0, 0.0, T, 1.0), (T, 0.0, T, 1.0), (2.0 * T, 0.0, T, 1.0)] },
        Layout { id: "two-thirds-left", name: "Wide left", zones: vec![(0.0, 0.0, 2.0 * T, 1.0), (2.0 * T, 0.0, T, 1.0)] },
        Layout { id: "two-thirds-right", name: "Wide right", zones: vec![(0.0, 0.0, T, 1.0), (T, 0.0, 2.0 * T, 1.0)] },
        Layout { id: "priority", name: "Priority", zones: vec![(0.0, 0.0, 0.25, 1.0), (0.25, 0.0, 0.5, 1.0), (0.75, 0.0, 0.25, 1.0)] },
        Layout { id: "quarters", name: "Quarters", zones: vec![(0.0, 0.0, 0.5, 0.5), (0.5, 0.0, 0.5, 0.5), (0.0, 0.5, 0.5, 0.5), (0.5, 0.5, 0.5, 0.5)] },
        Layout {
            id: "grid",
            name: "Grid",
            zones: vec![(0.0, 0.0, T, 0.5), (T, 0.0, T, 0.5), (2.0 * T, 0.0, T, 0.5), (0.0, 0.5, T, 0.5), (T, 0.5, T, 0.5), (2.0 * T, 0.5, T, 0.5)],
        },
        Layout { id: "rows", name: "Rows", zones: vec![(0.0, 0.0, 1.0, 0.5), (0.0, 0.5, 1.0, 0.5)] },
        Layout { id: "focus", name: "Focus", zones: vec![(0.15, 0.06, 0.7, 0.88)] },
    ]
}

#[derive(Clone, Debug, PartialEq)]
pub struct ZonesConf {
    pub enabled: bool,
    /// Layout ids, in the order the sheet shows them (unknown ids are dropped).
    pub layouts: Vec<String>,
    pub gap: u32,
}

impl Default for ZonesConf {
    fn default() -> Self {
        ZonesConf { enabled: true, layouts: ["halves", "thirds", "two-thirds-left", "quarters", "focus"].iter().map(|s| s.to_string()).collect(), gap: 8 }
    }
}

impl ZonesConf {
    pub fn path() -> PathBuf {
        crate::dirs::config().join("zones.conf")
    }

    pub fn parse(text: &str) -> ZonesConf {
        let mut c = ZonesConf::default();
        let known: Vec<&str> = builtin().iter().map(|l| l.id).collect();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "enabled" => c.enabled = matches!(v, "yes" | "true" | "on" | "1"),
                "gap" => c.gap = v.parse::<u32>().map(|g| g.min(64)).unwrap_or(c.gap),
                "layouts" => {
                    let mut ids: Vec<String> = vec![];
                    for id in v.split(',').map(str::trim).filter(|id| known.contains(id)) {
                        if !ids.iter().any(|x| x == id) {
                            ids.push(id.to_string());
                        }
                    }
                    c.layouts = ids;
                }
                _ => {}
            }
        }
        c
    }

    pub fn load() -> ZonesConf {
        std::fs::read_to_string(Self::path()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Window zones: drag a window to the notch at the top of the screen (or press Super+Shift+Z) and pick where it goes.\n\
             # Written by Settings > Displays > Window zones.\nenabled = {}\nlayouts = {}\ngap = {}\n",
            if self.enabled { "yes" } else { "no" },
            self.layouts.join(", "),
            self.gap
        )
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = Self::path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = p.with_extension("conf.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(tmp, p)
    }

    /// The layouts the sheet shows, in order.
    pub fn shown(&self) -> Vec<Layout> {
        let all = builtin();
        self.layouts.iter().filter_map(|id| all.iter().find(|l| l.id == id).cloned()).collect()
    }
}

/// A rectangle in logical pixels: x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// A screen as Hyprland reports it (`hyprctl -j monitors`): position in the global logical space, mode size in physical pixels,
/// scale, transform, and the strips other surfaces reserve (left, top, right, bottom, logical pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct Monitor {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub scale: f32,
    pub transform: u32,
    pub reserved: [i32; 4],
}

impl Monitor {
    /// The screen in logical pixels (rotated screens swap width and height).
    pub fn logical(&self) -> Rect {
        let s = if self.scale > 0.0 { self.scale } else { 1.0 };
        let (w, h) = ((self.width as f32 / s).round() as i32, (self.height as f32 / s).round() as i32);
        let (w, h) = if self.transform % 2 == 1 { (h, w) } else { (w, h) };
        (self.x, self.y, w, h)
    }

    /// What windows may use: the screen less the bar's, the dock's and any other reserved strips.
    pub fn free_area(&self) -> Rect {
        let (x, y, w, h) = self.logical();
        let [l, t, r, b] = self.reserved;
        (x + l, y + t, (w - l - r).max(1), (h - t - b).max(1))
    }

    pub fn contains(&self, px: f64, py: f64) -> bool {
        let (x, y, w, h) = self.logical();
        px >= x as f64 && py >= y as f64 && px < (x + w) as f64 && py < (y + h) as f64
    }
}

/// Where a window put in `zone` goes on a screen whose free area is `area`: the same `gap` at the screen's edges and between
/// neighbouring zones (each inner edge gives half).
pub fn place(zone: Zone, area: Rect, gap: u32) -> Rect {
    let (ax, ay, aw, ah) = (area.0 as f32, area.1 as f32, area.2 as f32, area.3 as f32);
    let g = gap as f32;
    let edge = |f: f32| f.abs() < 0.001 || (f - 1.0).abs() < 0.001;
    let (zx, zy, zw, zh) = zone;
    let inset = |f: f32| if edge(f) { g } else { g / 2.0 };
    let left = ax + zx * aw + inset(zx);
    let top = ay + zy * ah + inset(zy);
    let right = ax + (zx + zw) * aw - inset(zx + zw);
    let bottom = ay + (zy + zh) * ah - inset(zy + zh);
    let (l, t) = (left.round() as i32, top.round() as i32);
    (l, t, ((right.round() as i32) - l).max(40), ((bottom.round() as i32) - t).max(40))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(x: i32, y: i32, w: u32, h: u32, scale: f32, reserved: [i32; 4]) -> Monitor {
        Monitor { name: "DP-1".into(), x, y, width: w, height: h, scale, transform: 0, reserved }
    }

    #[test]
    fn halves_leave_the_same_gap_at_the_edges_and_between() {
        let m = mon(0, 0, 3440, 1440, 1.0, [0, 28, 0, 0]);
        let a = m.free_area();
        assert_eq!(a, (0, 28, 3440, 1412));
        let l = place((0.0, 0.0, 0.5, 1.0), a, 8);
        let r = place((0.5, 0.0, 0.5, 1.0), a, 8);
        assert_eq!(l, (8, 36, 1708, 1396));
        assert_eq!(r, (1724, 36, 1708, 1396));
        // 8 at the left edge, 8 between (4 + 4), 8 at the right edge
        assert_eq!(r.0 - (l.0 + l.2), 8);
        assert_eq!(3440 - (r.0 + r.2), 8);
    }

    #[test]
    fn the_dock_strip_and_a_second_scaled_screen_are_respected() {
        // a 4K screen at 1.5 to the right of the first, with a 76-pixel dock reserved at the bottom
        let m = mon(3440, 0, 3840, 2160, 1.5, [0, 28, 0, 76]);
        assert_eq!(m.logical(), (3440, 0, 2560, 1440));
        let a = m.free_area();
        assert_eq!(a, (3440, 28, 2560, 1336));
        let q = place((0.5, 0.5, 0.5, 0.5), a, 0);
        assert_eq!(q, (4720, 696, 1280, 668));
        assert!(m.contains(4000.0, 10.0) && !m.contains(3439.0, 10.0));
    }

    #[test]
    fn a_rotated_screen_swaps_its_sides() {
        let m = Monitor { transform: 1, ..mon(0, 0, 1920, 1080, 1.0, [0; 4]) };
        assert_eq!(m.logical(), (0, 0, 1080, 1920));
    }

    #[test]
    fn thirds_cover_the_screen_without_overlap() {
        let a = (0, 0, 1920, 1080);
        let t: Vec<Rect> = builtin()[1].zones.iter().map(|z| place(*z, a, 10)).collect();
        assert_eq!(t[0].0, 10);
        for w in t.windows(2) {
            assert_eq!(w[1].0 - (w[0].0 + w[0].2), 10, "{t:?}");
        }
        assert_eq!(1920 - (t[2].0 + t[2].2), 10);
    }

    #[test]
    fn the_conf_keeps_known_layouts_in_order() {
        let c = ZonesConf::parse("enabled = no\nlayouts = quarters, nonsense, halves, quarters\ngap = 500\n");
        assert!(!c.enabled);
        assert_eq!(c.layouts, vec!["quarters", "halves"]);
        assert_eq!(c.gap, 64);
        assert_eq!(c.shown()[0].id, "quarters");
        assert_eq!(ZonesConf::parse(&c.to_text()), c);
        assert_eq!(ZonesConf::parse(""), ZonesConf::default());
    }
}
