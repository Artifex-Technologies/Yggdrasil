//! The map as a picture: drawn on the processor (tiny-skia) only when something it shows changes (the view, its size, the
//! colours or the chosen band), then handed to egui as one texture. Nothing is drawn while it sits still.

use eframe::egui::{Color32, ColorImage};
use tiny_skia::{FillRule, Mask, Paint, PathBuilder, Pixmap, Transform};

use crate::world::{Ring, World};

/// The latitudes the map shows at its widest: the north of Greenland to Tierra del Fuego (no Antarctica).
pub const LAT_TOP: f32 = 84.0;
pub const LAT_BOTTOM: f32 = -57.0;
pub const MAX_ZOOM: f32 = 12.0;

/// What part of the map is shown: the point in the middle (degrees east, north) and how far in (1: the whole width).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub lon: f32,
    pub lat: f32,
    pub zoom: f32,
}

impl Default for View {
    fn default() -> Self {
        View { lon: 0.0, lat: (LAT_TOP + LAT_BOTTOM) / 2.0, zoom: 1.0 }
    }
}

impl View {
    /// Pixels per degree, on a map `w` pixels wide.
    pub fn scale(&self, w: f32) -> f32 {
        w / 360.0 * self.zoom
    }

    /// Where a place is on a map `w` x `h` (from its top left corner).
    pub fn to_px(&self, w: f32, h: f32, lon: f32, lat: f32) -> (f32, f32) {
        let s = self.scale(w);
        ((lon - self.lon) * s + w / 2.0, (self.lat - lat) * s + h / 2.0)
    }

    /// The place at a point of the map.
    pub fn to_deg(&self, w: f32, h: f32, x: f32, y: f32) -> (f32, f32) {
        let s = self.scale(w);
        ((x - w / 2.0) / s + self.lon, self.lat - (y - h / 2.0) / s)
    }

    /// Zoomed in or out by `factor`, the place under (x, y) staying where it is.
    pub fn zoom_at(self, w: f32, h: f32, factor: f32, x: f32, y: f32) -> View {
        let (lon, lat) = self.to_deg(w, h, x, y);
        let zoom = (self.zoom * factor).clamp(1.0, MAX_ZOOM);
        let s = w / 360.0 * zoom;
        View { lon: lon - (x - w / 2.0) / s, lat: lat + (y - h / 2.0) / s, zoom }.clamp(w, h)
    }

    /// Kept inside the world: no empty space beside it, nor above or below it unless the whole height fits.
    pub fn clamp(self, w: f32, h: f32) -> View {
        let zoom = self.zoom.clamp(1.0, MAX_ZOOM);
        let s = w / 360.0 * zoom;
        let half_lon = w / 2.0 / s;
        let half_lat = h / 2.0 / s;
        let lon = if half_lon >= 180.0 { 0.0 } else { self.lon.clamp(-180.0 + half_lon, 180.0 - half_lon) };
        let lat = if half_lat * 2.0 >= LAT_TOP - LAT_BOTTOM {
            (LAT_TOP + LAT_BOTTOM) / 2.0
        } else {
            self.lat.clamp(LAT_BOTTOM + half_lat, LAT_TOP - half_lat)
        };
        View { lon, lat, zoom }
    }

    /// Whether a box of degrees (min east, min north, max east, max north) is at least partly on a map `w` x `h`.
    fn sees(&self, w: f32, h: f32, b: [f32; 4]) -> bool {
        let (x0, y0) = self.to_px(w, h, b[0], b[3]);
        let (x1, y1) = self.to_px(w, h, b[2], b[1]);
        x1 >= -2.0 && x0 <= w + 2.0 && y1 >= -2.0 && y0 <= h + 2.0
    }
}

/// The map's colours (premultiplied, as egui keeps them).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Colors {
    pub dots: Color32,
    pub land: Color32,
    pub lines: Color32,
    pub highlight: Color32,
    /// The light style: the band is a little stronger, its glow a little softer.
    pub light: bool,
}

fn paint(c: Color32) -> Paint<'static> {
    let [r, g, b, a] = c.to_srgba_unmultiplied();
    let mut p = Paint::default();
    p.set_color_rgba8(r, g, b, a);
    p.anti_alias = true;
    p
}

fn path(w: f32, h: f32, view: &View, rings: &[&Ring]) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    for r in rings {
        let mut first = true;
        for p in &r.pts {
            let (x, y) = view.to_px(w, h, p[0], p[1]);
            if first {
                pb.move_to(x, y);
                first = false;
            } else {
                pb.line_to(x, y);
            }
        }
        pb.close();
    }
    pb.finish()
}

/// Blends `c` (premultiplied, 0..255) at `a` (0..1) over a premultiplied pixel.
fn over(px: &mut [u8], c: [f32; 4], a: f32) {
    if a <= 0.0 {
        return;
    }
    let keep = 1.0 - c[3] / 255.0 * a;
    for k in 0..4 {
        px[k] = (c[k] * a + px[k] as f32 * keep).round().clamp(0.0, 255.0) as u8;
    }
}

fn rgba(c: Color32) -> [f32; 4] {
    c.to_array().map(f32::from)
}

/// A morphological closing by one pixel: the largest of each 3x3 neighbourhood, then the smallest.
fn close(m: &mut [u8], w: usize, h: usize) {
    let mut tmp = vec![0u8; m.len()];
    for grow in [true, false] {
        let pick = |a: u8, b: u8| if grow { a.max(b) } else { a.min(b) };
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let l = if x > 0 { m[i - 1] } else { m[i] };
                let r = if x + 1 < w { m[i + 1] } else { m[i] };
                tmp[i] = pick(pick(l, m[i]), r);
            }
        }
        for y in 0..h {
            for x in 0..w {
                let i = y * w + x;
                let u = if y > 0 { tmp[i - w] } else { tmp[i] };
                let d = if y + 1 < h { tmp[i + w] } else { tmp[i] };
                m[i] = pick(pick(u, tmp[i]), d);
            }
        }
    }
}

/// Two box blurs across and down: a soft glow from the band's outline.
fn blur(src: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut a: Vec<u16> = src.iter().map(|&v| v as u16).collect();
    let mut b = vec![0u16; a.len()];
    let n = (2 * r + 1) as u32;
    for _ in 0..2 {
        for y in 0..h {
            let row = &a[y * w..(y + 1) * w];
            let mut sum: u32 = (0..=r).map(|i| row[i.min(w - 1)] as u32).sum::<u32>() + row[0] as u32 * r as u32;
            for x in 0..w {
                b[y * w + x] = (sum / n) as u16;
                sum += row[(x + r + 1).min(w - 1)] as u32;
                sum -= row[x.saturating_sub(r)] as u32;
            }
        }
        for x in 0..w {
            let at = |y: usize| b[y.min(h - 1) * w + x] as u32;
            let mut sum: u32 = (0..=r).map(at).sum::<u32>() + at(0) * r as u32;
            for y in 0..h {
                a[y * w + x] = (sum / n) as u16;
                sum += at(y + r + 1);
                sum -= at(y.saturating_sub(r));
            }
        }
    }
    a.into_iter().map(|v| v.min(255) as u8).collect()
}

/// The map `w` x `h` pixels: a faint dot grid, the land, the lines between the zone bands, and the band of `band` (a standard
/// offset in seconds) filled in the highlight colour with a glow around it. `ppp`: pixels per point (dots and glow keep their
/// size on a sharper screen).
pub fn render(world: &World, view: &View, w: usize, h: usize, ppp: f32, colors: &Colors, band: Option<i32>) -> ColorImage {
    let Some(mut pm) = Pixmap::new(w.max(1) as u32, h.max(1) as u32) else { return ColorImage::filled([1, 1], Color32::TRANSPARENT) };
    let (wf, hf) = (w as f32, h as f32);
    // the dots: screen-fixed, every 8 points
    let step = (8.0 * ppp).round().max(4.0) as usize;
    let size = ppp.round().max(1.0) as usize;
    let dot = colors.dots.to_array();
    {
        let data = pm.data_mut();
        for y in (step / 2..h).step_by(step) {
            for x in (step / 2..w).step_by(step) {
                for dy in 0..size.min(h - y) {
                    for dx in 0..size.min(w - x) {
                        data[((y + dy) * w + x + dx) * 4..][..4].copy_from_slice(&dot);
                    }
                }
            }
        }
    }
    // the land: one path into a mask, closed (grown by a pixel and shrunk back) because the countries were simplified one by one
    // and their borders no longer meet exactly; the closing fills those hairline gaps and leaves the coasts where they were
    let mut mask = Mask::new(w as u32, h as u32);
    let land: Vec<&Ring> = world.land.iter().filter(|r| view.sees(wf, hf, r.bbox)).collect();
    if let (Some(p), Some(mask)) = (path(wf, hf, view, &land), mask.as_mut()) {
        mask.fill_path(&p, FillRule::Winding, true, Transform::identity());
        close(mask.data_mut(), w, h);
        if let Some(r) = tiny_skia::Rect::from_xywh(0.0, 0.0, wf, hf) {
            pm.fill_rect(r, &paint(colors.land), Transform::identity(), Some(mask));
        }
    }
    // the lines between bands: where the band a pixel is in changes (two bands of the same offset that touch get no line)
    let mut ids = vec![u8::MAX; w * h];
    if let Some(mask) = mask.as_mut() {
        for (i, b) in world.bands.iter().enumerate().filter(|(_, b)| view.sees(wf, hf, b.bbox)) {
            let rings: Vec<&Ring> = b.rings.iter().filter(|r| view.sees(wf, hf, r.bbox)).collect();
            let Some(p) = path(wf, hf, view, &rings) else { continue };
            mask.data_mut().fill(0);
            mask.fill_path(&p, FillRule::EvenOdd, false, Transform::identity());
            for (id, m) in ids.iter_mut().zip(mask.data()) {
                if *m >= 128 {
                    *id = i as u8;
                }
            }
        }
    }
    let line = rgba(colors.lines);
    {
        let data = pm.data_mut();
        for y in 0..h {
            for x in 0..w {
                let id = ids[y * w + x];
                let right = if x + 1 < w { ids[y * w + x + 1] } else { id };
                let down = if y + 1 < h { ids[(y + 1) * w + x] } else { id };
                if id != u8::MAX && (right != id && right != u8::MAX || down != id && down != u8::MAX) {
                    over(&mut data[(y * w + x) * 4..][..4], line, 1.0);
                }
            }
        }
    }
    // the chosen band: filled, outlined and glowing
    if let (Some(off), Some(mask)) = (band, mask.as_mut()) {
        let rings: Vec<&Ring> =
            world.bands.iter().filter(|b| b.offset == off).flat_map(|b| &b.rings).filter(|r| view.sees(wf, hf, r.bbox)).collect();
        if let Some(p) = path(wf, hf, view, &rings) {
            mask.data_mut().fill(0);
            mask.fill_path(&p, FillRule::EvenOdd, true, Transform::identity());
            let m = mask.data();
            let glow = blur(m, w, h, (7.0 * ppp).round().max(2.0) as usize);
            let hi = rgba(colors.highlight);
            let (fill_k, glow_k) = if colors.light { (0.42, 0.40) } else { (0.34, 0.55) };
            let data = pm.data_mut();
            let at = |x: usize, y: usize| m[y * w + x] as f32 / 255.0;
            for y in 0..h {
                for x in 0..w {
                    let i = y * w + x;
                    let mv = m[i] as f32 / 255.0;
                    let px = &mut data[i * 4..][..4];
                    over(px, hi, glow[i] as f32 / 255.0 * glow_k * (1.0 - mv));
                    over(px, hi, mv * fill_k);
                    let gx = (at((x + 1).min(w - 1), y) - at(x.saturating_sub(1), y)).abs();
                    let gy = (at(x, (y + 1).min(h - 1)) - at(x, y.saturating_sub(1))).abs();
                    over(px, hi, ((gx + gy) * 0.9).min(0.95));
                }
            }
        }
    }
    ColorImage::from_rgba_premultiplied([w, h], pm.data())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_maps_places_to_points_and_back() {
        let v = View::default();
        let (w, h) = (720.0, 282.0);
        let (x, y) = v.to_px(w, h, 0.0, v.lat);
        assert_eq!((x, y), (360.0, 141.0));
        let (lon, lat) = v.to_deg(w, h, 0.0, 0.0);
        assert!((lon + 180.0).abs() < 1e-3 && (lat - LAT_TOP).abs() < 1e-3, "the top left corner is the world's");
        // zooming keeps the place under the pointer, and the world fills the map
        let z = v.zoom_at(w, h, 4.0, 200.0, 100.0);
        let before = v.to_deg(w, h, 200.0, 100.0);
        let after = z.to_deg(w, h, 200.0, 100.0);
        assert!((before.0 - after.0).abs() < 1e-3 && (before.1 - after.1).abs() < 1e-3);
        assert_eq!(v.zoom_at(w, h, 0.25, 100.0, 100.0), v.clamp(w, h), "never smaller than the world");
        assert_eq!(v.zoom_at(w, h, 1000.0, 0.0, 0.0).zoom, MAX_ZOOM);
        // panned too far: back at the edge
        let far = View { lon: 170.0, lat: 80.0, zoom: 2.0 }.clamp(w, h);
        assert_eq!(far.to_px(w, h, 180.0, 0.0).0, w);
        assert!((far.to_px(w, h, 0.0, LAT_TOP).1).abs() < 1e-3);
    }

    #[test]
    fn the_chosen_band_is_drawn_and_nothing_else_is() {
        let world = World::get();
        let colors = Colors {
            dots: Color32::from_rgb(1, 1, 1),
            land: Color32::from_rgb(60, 50, 110),
            lines: Color32::from_rgba_unmultiplied(255, 255, 255, 40),
            highlight: Color32::from_rgb(255, 140, 50),
            light: false,
        };
        let v = View::default();
        let (w, h) = (360, 141);
        let plain = render(world, &v, w, h, 1.0, &colors, None);
        let chosen = render(world, &v, w, h, 1.0, &colors, Some(-7 * 3600));
        let px = |img: &ColorImage, lon: f32, lat: f32| {
            let (x, y) = v.to_px(w as f32, h as f32, lon, lat);
            img.pixels[y as usize * w + x as usize]
        };
        // Colorado: land, orange in the -07:00 band
        assert_eq!(px(&plain, -105.5, 39.0), colors.land);
        assert!(px(&chosen, -105.5, 39.0).r() > colors.land.r() + 40);
        // Berlin: the same either way
        assert_eq!(px(&plain, 13.4, 52.5), px(&chosen, 13.4, 52.5));
    }
}
