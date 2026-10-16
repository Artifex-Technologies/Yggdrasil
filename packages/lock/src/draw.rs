//! Just enough drawing for a lock screen: text and rounded shapes on a software pixmap. (Copied from the panel's paint.rs; to be shared
//! through a common crate later.)

use std::path::PathBuf;

use ab_glyph::{point, Font, FontVec, PxScale, ScaleFont};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PremultipliedColorU8, Transform};

pub type Rgba = (u8, u8, u8, u8);

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
    pub fn load() -> Option<Fonts> {
        let load = |pat: &str| -> Option<FontVec> { FontVec::try_from_vec(std::fs::read(fc_match(pat)?).ok()?).ok() };
        let regular = load("sans-serif")?;
        let bold = load("sans-serif:bold").or_else(|| load("sans-serif"))?;
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

pub fn draw_text(pix: &mut Pixmap, font: &FontVec, size: f32, x: f32, cy: f32, c: Rgba, s: &str) {
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
                let r = blend(c.0, d.red()).min(na);
                let g2 = blend(c.1, d.green()).min(na);
                let bl = blend(c.2, d.blue()).min(na);
                pixels[idx] = PremultipliedColorU8::from_rgba(r, g2, bl, na).unwrap_or(d);
            });
        }
        cursor += adv;
    }
}

pub fn fill_rounded(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    if let Some(path) = pb.finish() {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(c.0, c.1, c.2, c.3));
        paint.anti_alias = true;
        pix.fill_path(&path, &paint, FillRule::Winding, Transform::identity(), None);
    }
}

pub fn fill_circle(pix: &mut Pixmap, cx: f32, cy: f32, r: f32, c: Rgba) {
    fill_rounded(pix, cx - r, cy - r, r * 2.0, r * 2.0, r, c);
}
