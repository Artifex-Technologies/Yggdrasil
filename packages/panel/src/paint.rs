//! Drawing: rounded shapes, text and icons into a tiny-skia pixmap. Everything is done on the CPU:
//! the bar and dock are mostly still, so there is no GPU context to keep alive and no wgpu memory.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use resvg::tiny_skia::{self, Color, FillRule, Paint, Path as SkPath, PathBuilder, Pixmap, Transform};

pub type Rgba = (u8, u8, u8, u8);

static ACCENT_RGB: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0x007c_54d6);

/// The accent color (the system's, from Settings > Appearance) at opacity `a`.
pub fn accent(a: u8) -> Rgba {
    let v = ACCENT_RGB.load(std::sync::atomic::Ordering::Relaxed);
    (((v >> 16) & 255) as u8, ((v >> 8) & 255) as u8, (v & 255) as u8, a)
}

/// Read the accent color from kdeglobals (`AccentColor=r,g,b` under [General]); true when it changed.
pub fn refresh_accent() -> bool {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let Some(text) = dir.and_then(|d| std::fs::read_to_string(d.join("kdeglobals")).ok()) else { return false };
    let mut in_general = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_general = l == "[General]";
        } else if in_general {
            if let Some(v) = l.strip_prefix("AccentColor=") {
                let p: Vec<u32> = v.trim().trim_matches('"').split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if p.len() == 3 && p.iter().all(|c| *c < 256) {
                    let packed = (p[0] << 16) | (p[1] << 8) | p[2];
                    return ACCENT_RGB.swap(packed, std::sync::atomic::Ordering::Relaxed) != packed;
                }
            }
        }
    }
    false
}

/// The icon theme chosen in KDE (`[Icons] Theme` in kdeglobals).
pub fn selected_icon_theme() -> Option<String> {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let text = std::fs::read_to_string(dir.join("kdeglobals")).ok()?;
    let mut in_icons = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_icons = line == "[Icons]";
        } else if in_icons {
            if let Some(v) = line.strip_prefix("Theme=") {
                let v = v.trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

pub fn color(c: Rgba) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, c.3)
}

/// A rounded rectangle path (x, y, w, h, radius).
pub fn rounded_path(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<SkPath> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.552_284_75 * r; // circle approximated by cubic curves
    let mut p = PathBuilder::new();
    p.move_to(x + r, y);
    p.line_to(x + w - r, y);
    p.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    p.line_to(x + w, y + h - r);
    p.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    p.line_to(x + r, y + h);
    p.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    p.line_to(x, y + r);
    p.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    p.close();
    p.finish()
}

pub fn fill_rounded(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
    if let Some(path) = rounded_path(x, y, w, h, r) {
        let mut paint = Paint::default();
        paint.set_color(color(c));
        paint.anti_alias = true;
        pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

pub fn stroke_rounded(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, width: f32, c: Rgba) {
    if let Some(path) = rounded_path(x, y, w, h, r) {
        let mut paint = Paint::default();
        paint.set_color(color(c));
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke { width, ..Default::default() };
        pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

/// A ring segment: from the top, clockwise, `frac` of the way round (1.0 is the whole circle).
pub fn stroke_arc(pix: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, frac: f32, c: Rgba) {
    let frac = frac.clamp(0.0, 1.0);
    if frac <= 0.0 {
        return;
    }
    let n = ((frac * 96.0).ceil() as usize).max(2);
    let mut pb = PathBuilder::new();
    for i in 0..=n {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * (i as f32 / n as f32);
        let (x, y) = (cx + r * a.cos(), cy + r * a.sin());
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(color(c));
        paint.anti_alias = true;
        let stroke = tiny_skia::Stroke { width, line_cap: tiny_skia::LineCap::Round, ..Default::default() };
        pix.stroke_path(&path, &paint, &stroke, Transform::identity(), None);
    }
}

pub fn fill_circle(pix: &mut Pixmap, cx: f32, cy: f32, r: f32, c: Rgba) {
    fill_rounded(pix, cx - r, cy - r, r * 2.0, r * 2.0, r, c);
}

// ---------------------------------------------------------------- fonts

pub struct Fonts {
    pub regular: FontVec,
    pub bold: FontVec,
}

fn fc_match(pattern: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim().to_string());
    p.is_file().then_some(p)
}

impl Fonts {
    /// The system's default sans (Noto Sans in the image), regular and bold.
    pub fn load() -> Option<Fonts> {
        let load = |pat: &str| -> Option<FontVec> { FontVec::try_from_vec(std::fs::read(fc_match(pat)?).ok()?).ok() };
        let regular = load("sans-serif")?;
        let bold = load("sans-serif:bold").unwrap_or_else(|| FontVec::try_from_vec(std::fs::read(fc_match("sans-serif").unwrap()).unwrap()).unwrap());
        Some(Fonts { regular, bold })
    }
}

pub fn text_width(font: &FontVec, size: f32, s: &str) -> f32 {
    let f = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for ch in s.chars() {
        let g = f.scaled_glyph(ch);
        if let Some(p) = prev {
            w += f.kern(p, g.id);
        }
        w += f.h_advance(g.id);
        prev = Some(g.id);
    }
    w
}

/// Draw `s` with its left edge at `x` and the text vertically centred on `cy`. Returns the width.
pub fn draw_text(pix: &mut Pixmap, font: &FontVec, size: f32, x: f32, cy: f32, c: Rgba, s: &str) -> f32 {
    let f = font.as_scaled(PxScale::from(size));
    let baseline = cy + (f.ascent() + f.descent()) / 2.0;
    let mut cursor = x;
    let mut prev = None;
    let (pw, ph) = (pix.width() as i32, pix.height() as i32);
    for ch in s.chars() {
        let mut g = f.scaled_glyph(ch);
        if let Some(p) = prev {
            cursor += f.kern(p, g.id);
        }
        g.position = point(cursor, baseline);
        let adv = f.h_advance(g.id);
        prev = Some(g.id);
        if let Some(o) = font.outline_glyph(g) {
            let b = o.px_bounds();
            let pixels = pix.pixels_mut();
            o.draw(|gx, gy, cov| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                if px < 0 || py < 0 || px >= pw || py >= ph || cov <= 0.0 {
                    return;
                }
                let a = cov * c.3 as f32 / 255.0;
                let idx = (py * pw + px) as usize;
                let d = pixels[idx];
                let blend = |src: u8, dst: u8| ((src as f32 * a) + dst as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
                let na = ((a * 255.0) + d.alpha() as f32 * (1.0 - a)).round().clamp(0.0, 255.0) as u8;
                // The pixmap stores premultiplied colour: premultiply the source by `a` (done in blend).
                let r = blend(c.0, d.red()).min(na);
                let g2 = blend(c.1, d.green()).min(na);
                let bl = blend(c.2, d.blue()).min(na);
                pixels[idx] = tiny_skia::PremultipliedColorU8::from_rgba(r, g2, bl, na).unwrap_or(d);
            });
        }
        cursor += adv;
    }
    cursor - x
}

// ---------------------------------------------------------------- icons

pub struct Icons {
    cache: HashMap<(String, u32), Option<Pixmap>>,
    themes: Vec<String>,
}

impl Icons {
    pub fn new() -> Icons {
        Icons { cache: HashMap::new(), themes: Self::chain(selected_icon_theme().as_deref()) }
    }

    /// The lookup order: the icon pack chosen in KDE first (if it is not one of ours already), then
    /// Labyrinth's own, which fills in whatever the pack has no icon for.
    fn chain(chosen: Option<&str>) -> Vec<String> {
        const BASE: [&str; 7] = ["Labyrinth", "MacTahoe-purple-dark", "MacTahoe-purple", "hicolor", "breeze-dark", "breeze", "Adwaita"];
        let mut v: Vec<String> = vec![];
        if let Some(c) = chosen {
            if c != "Labyrinth" {
                // another pack: then hicolor, where Minos's icon for that pack is put (the pack's own
                // file-manager icon if it has no Minos one), then Labyrinth's to fill the gaps
                v.push(c.to_string());
                v.push("hicolor".to_string());
            }
        }
        v.extend(BASE.iter().map(|s| s.to_string()));
        v
    }

    /// The icon pack was changed in KDE: use it from now on.
    pub fn set_pack(&mut self, chosen: Option<&str>) {
        self.themes = Self::chain(chosen);
        self.cache.clear();
    }

    fn roots() -> Vec<PathBuf> {
        let mut v = vec![];
        if let Some(h) = std::env::var_os("HOME") {
            v.push(PathBuf::from(h).join(".local/share/icons"));
        }
        v.push(PathBuf::from("/usr/share/icons"));
        v
    }

    /// The file for an icon name (or an absolute path), best match for `size`.
    pub fn find(&self, name: &str, size: u32) -> Option<PathBuf> {
        if name.starts_with('/') {
            return Path::new(name).is_file().then(|| PathBuf::from(name));
        }
        let exts = ["svg", "svgz", "png"];
        for root in Self::roots() {
            for theme in &self.themes {
                let t = root.join(theme);
                let mut dirs: Vec<PathBuf> = vec![];
                for ctx in ["apps", "places", "status", "actions"] {
                    dirs.push(t.join(format!("{ctx}/scalable")));
                    dirs.push(t.join(format!("scalable/{ctx}")));
                    dirs.push(t.join(format!("{ctx}/symbolic")));
                    dirs.push(t.join(format!("symbolic/{ctx}")));
                    for s in [size, 512, 384, 256, 192, 144, 128, 96, 72, 64, 48, 36, 32, 24, 22, 16] {
                        dirs.push(t.join(format!("{ctx}/{s}")));
                        dirs.push(t.join(format!("{s}x{s}/{ctx}")));
                    }
                }
                for d in dirs {
                    for e in exts {
                        let p = d.join(format!("{name}.{e}"));
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
        for e in exts {
            let p = PathBuf::from(format!("/usr/share/pixmaps/{name}.{e}"));
            if p.is_file() {
                return Some(p);
            }
        }
        None
    }

    fn render(&self, name: &str, size: u32) -> Option<Pixmap> {
        let path = self.find(name, size)?;
        let data = std::fs::read(&path).ok()?;
        let mut pm = Pixmap::new(size, size)?;
        if path.extension().map(|e| e == "png").unwrap_or(false) {
            let src = Pixmap::decode_png(&data).ok()?;
            let scale = (size as f32 / src.width() as f32).min(size as f32 / src.height() as f32);
            if scale < 1.0 {
                // Shrinking a big picture (the pack's app icons are 1024 px) straight to a few dozen pixels
                // samples it too sparsely and looks jagged: average it down instead, then place it as it is.
                let (tw, th) = (((src.width() as f32 * scale).round() as u32).max(1), ((src.height() as f32 * scale).round() as u32).max(1));
                let small = area_resize(&src, tw, th)?;
                let (tx, ty) = ((size - tw) / 2, (size - th) / 2);
                pm.draw_pixmap(tx as i32, ty as i32, small.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
                return Some(pm);
            }
            let tx = (size as f32 - src.width() as f32 * scale) / 2.0;
            let ty = (size as f32 - src.height() as f32 * scale) / 2.0;
            let paint = tiny_skia::PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() };
            pm.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(scale, scale).post_translate(tx, ty), None);
            return Some(pm);
        }
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let sz = tree.size();
        let scale = (size as f32 / sz.width()).min(size as f32 / sz.height());
        let tx = (size as f32 - sz.width() * scale) / 2.0;
        let ty = (size as f32 - sz.height() * scale) / 2.0;
        resvg::render(&tree, Transform::from_scale(scale, scale).post_translate(tx, ty), &mut pm.as_mut());
        Some(pm)
    }

    /// A symbolic (single-colour) icon painted in `color`: these have no colour of their own and
    /// would be black on a dark bar.
    pub fn get_tinted(&mut self, name: &str, size: u32, color: Rgba) -> Option<&Pixmap> {
        let key = (format!("{name}#{:02x}{:02x}{:02x}", color.0, color.1, color.2), size);
        if !self.cache.contains_key(&key) {
            let r = self.find(name, size).and_then(|path| {
                let svg = std::fs::read_to_string(&path).ok()?;
                let tinted = svg.replacen("<svg ", &format!("<svg fill='#{:02x}{:02x}{:02x}' ", color.0, color.1, color.2), 1);
                let tree = resvg::usvg::Tree::from_data(tinted.as_bytes(), &resvg::usvg::Options::default()).ok()?;
                let sz = tree.size();
                let mut pm = Pixmap::new(size, size)?;
                let scale = (size as f32 / sz.width()).min(size as f32 / sz.height());
                let tx = (size as f32 - sz.width() * scale) / 2.0;
                let ty = (size as f32 - sz.height() * scale) / 2.0;
                resvg::render(&tree, Transform::from_scale(scale, scale).post_translate(tx, ty), &mut pm.as_mut());
                Some(pm)
            });
            self.cache.insert(key.clone(), r);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }

    /// An SVG that ships inside the program, drawn in one colour (for the solid Labyrinth mark).
    pub fn get_svg(&mut self, name: &str, svg: &str, size: u32, color: Rgba) -> Option<&Pixmap> {
        let key = (format!("{name}#{:02x}{:02x}{:02x}", color.0, color.1, color.2), size);
        if !self.cache.contains_key(&key) {
            let tinted = svg.replacen("<path", &format!("<path fill='#{:02x}{:02x}{:02x}'", color.0, color.1, color.2), 1);
            let r = resvg::usvg::Tree::from_data(tinted.as_bytes(), &resvg::usvg::Options::default()).ok().and_then(|tree| {
                let sz = tree.size();
                let mut pm = Pixmap::new(size, size)?;
                let scale = (size as f32 / sz.width()).min(size as f32 / sz.height());
                resvg::render(&tree, Transform::from_scale(scale, scale), &mut pm.as_mut());
                Some(pm)
            });
            self.cache.insert(key.clone(), r);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }

    /// Forget every icon drawn so far (an icon file changed on disk, like Minos's when the accent colour changes).
    pub fn clear(&mut self) {
        self.cache.clear();
    }

    pub fn get(&mut self, name: &str, size: u32) -> Option<&Pixmap> {
        let key = (name.to_string(), size);
        if !self.cache.contains_key(&key) {
            let r = self.render(name, size);
            self.cache.insert(key.clone(), r);
        }
        self.cache.get(&key).and_then(|o| o.as_ref())
    }
}

pub fn draw_pixmap(dst: &mut Pixmap, src: &Pixmap, x: i32, y: i32) {
    dst.draw_pixmap(x, y, src.as_ref(), &tiny_skia::PixmapPaint::default(), Transform::identity(), None);
}

/// `src` shrunk to `dw` x `dh` by averaging every source pixel under each new one (a box filter with
/// fractional edges, on premultiplied colour, so thin lines fade instead of vanishing and edges stay clean).
pub fn area_resize(src: &Pixmap, dw: u32, dh: u32) -> Option<Pixmap> {
    let (sw, sh, dw_, dh_) = (src.width() as usize, src.height() as usize, dw as usize, dh as usize);
    let (kx, ky) = (sw as f32 / dw_ as f32, sh as f32 / dh_ as f32);
    let s = src.data();
    // horizontal pass: sh rows of dw pixels
    let mut mid = vec![0f32; sh * dw_ * 4];
    for y in 0..sh {
        for x in 0..dw_ {
            let (a, b) = (x as f32 * kx, (x + 1) as f32 * kx);
            let mut acc = [0f32; 4];
            let mut sx = a.floor() as usize;
            while (sx as f32) < b && sx < sw {
                let w = (b.min(sx as f32 + 1.0) - a.max(sx as f32)).max(0.0);
                for c in 0..4 {
                    acc[c] += s[(y * sw + sx) * 4 + c] as f32 * w;
                }
                sx += 1;
            }
            for c in 0..4 {
                mid[(y * dw_ + x) * 4 + c] = acc[c] / kx;
            }
        }
    }
    let mut out = Pixmap::new(dw, dh)?;
    let d = out.data_mut();
    for y in 0..dh_ {
        let (a, b) = (y as f32 * ky, (y + 1) as f32 * ky);
        for x in 0..dw_ {
            let mut acc = [0f32; 4];
            let mut sy = a.floor() as usize;
            while (sy as f32) < b && sy < sh {
                let w = (b.min(sy as f32 + 1.0) - a.max(sy as f32)).max(0.0);
                for c in 0..4 {
                    acc[c] += mid[(sy * dw_ + x) * 4 + c] * w;
                }
                sy += 1;
            }
            for c in 0..4 {
                d[(y * dw_ + x) * 4 + c] = (acc[c] / ky).round().clamp(0.0, 255.0) as u8;
            }
        }
    }
    Some(out)
}

#[cfg(test)]
mod resize_tests {
    use super::*;

    #[test]
    fn a_thin_line_fades_instead_of_vanishing_and_flat_colour_stays() {
        // 64x64 transparent with one opaque white 1px vertical line, shrunk 8x
        let mut src = Pixmap::new(64, 64).unwrap();
        for y in 0..64 {
            let i = (y * 64 + 20) * 4;
            src.data_mut()[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
        }
        let small = area_resize(&src, 8, 8).unwrap();
        let a = small.data()[(4 * 8 + 2) * 4 + 3];
        assert!((25..=35).contains(&a), "one pixel in eight should leave about 1/8 alpha, got {a}");
        let mut flat = Pixmap::new(40, 40).unwrap();
        flat.fill(tiny_skia::Color::from_rgba8(200, 100, 50, 255));
        let f = area_resize(&flat, 7, 7).unwrap();
        assert_eq!(&f.data()[..4], &[200, 100, 50, 255]);
    }
}
