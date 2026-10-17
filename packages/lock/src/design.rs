//! The lock screen's looks. Several designs share one scene (who, what was typed, the time, the wallpaper) and draw it their own way; the chosen
//! one is in ~/.config/labyrinth/lock.conf (`design = split`) and Settings > Appearance > Lock screen changes it. `labyrinth-lock --preview
//! DESIGN --out FILE` draws one into a picture without locking anything (that is what Settings shows).
//!
//! "Split" follows the design of the same name in Omarchy's Lock Screen Explorer (https://github.com/SirJul1337/omarchy-lock-explorer, MIT,
//! (c) 2026 SirJul1337): wallpaper on the left with the clock under it, a frosted sign-in panel on the right. Redrawn here for Labyrinth.

use std::path::PathBuf;

use tiny_skia::{Color, FilterQuality, GradientStop, LinearGradient, Paint, Pixmap, PixmapPaint, Point, Rect, SpreadMode, Transform};

use crate::draw::{self, fill_circle, fill_rounded, Fonts, Rgba};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Design {
    Classic,
    Split,
    Zen,
    Poster,
}

pub const ALL: [(Design, &str, &str, &str); 4] = [
    (Design::Split, "split", "Split", "Wallpaper on the left, a frosted sign-in panel on the right"),
    (Design::Classic, "classic", "Classic", "Clock and password field, centred on a dark background"),
    (Design::Zen, "zen", "Zen", "The wallpaper, a small clock and a quiet password field"),
    (Design::Poster, "poster", "Poster", "A huge stacked clock, the field tucked in the corner"),
];

impl Design {
    pub fn id(self) -> &'static str {
        ALL.iter().find(|d| d.0 == self).map(|d| d.1).unwrap_or("split")
    }
    pub fn parse(s: &str) -> Option<Design> {
        ALL.iter().find(|d| d.1 == s.trim().to_lowercase()).map(|d| d.0)
    }
}

/// The chosen design (Split unless lock.conf says otherwise).
pub fn chosen() -> Design {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let text = dir.and_then(|d| std::fs::read_to_string(d.join("labyrinth/lock.conf")).ok()).unwrap_or_default();
    text.lines().filter_map(|l| l.split_once('=')).find(|(k, _)| k.trim() == "design").and_then(|(_, v)| Design::parse(v)).unwrap_or(Design::Split)
}

/// Everything a design needs to draw one frame.
pub struct Scene<'a> {
    pub fonts: &'a Fonts,
    pub user: &'a str,
    pub dots: usize,
    pub waiting: bool,
    pub message: &'a str,
    pub time: String,
    pub date: String,
    pub greeting: String,
    /// The wallpaper scaled to cover the screen, and a blurred copy of it.
    pub wall: Option<&'a Pixmap>,
    pub blurred: Option<&'a Pixmap>,
    pub avatar: Option<&'a Pixmap>,
    pub accent: Rgba,
}

const TEXT: Rgba = (240, 236, 250, 255);
const DIM: Rgba = (190, 184, 208, 255);

fn gradient(pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, from: (Point, Color), to: (Point, Color)) {
    let Some(shader) = LinearGradient::new(from.0, to.0, vec![GradientStop::new(0.0, from.1), GradientStop::new(1.0, to.1)], SpreadMode::Pad, Transform::identity()) else { return };
    let mut paint = Paint::default();
    paint.shader = shader;
    if let Some(r) = Rect::from_xywh(x, y, w, h) {
        pix.fill_rect(r, &paint, Transform::identity(), None);
    }
}

fn shade_top_bottom(pix: &mut Pixmap, top: u8, bottom: u8) {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    gradient(pix, 0.0, 0.0, w, h * 0.35, (Point::from_xy(0.0, 0.0), Color::from_rgba8(0, 0, 0, top)), (Point::from_xy(0.0, h * 0.35), Color::from_rgba8(0, 0, 0, 0)));
    gradient(pix, 0.0, h * 0.55, w, h * 0.45, (Point::from_xy(0.0, h * 0.55), Color::from_rgba8(0, 0, 0, 0)), (Point::from_xy(0.0, h), Color::from_rgba8(0, 0, 0, bottom)));
}

fn background(pix: &mut Pixmap, s: &Scene, dim: u8) {
    pix.fill(Color::from_rgba8(12, 9, 22, 255));
    if let Some(w) = s.wall {
        pix.draw_pixmap(0, 0, w.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
    if dim > 0 {
        pix.fill_rect(Rect::from_xywh(0.0, 0.0, pix.width() as f32, pix.height() as f32).unwrap(), &flat(0, 0, 0, dim), Transform::identity(), None);
    }
}

fn flat(r: u8, g: u8, b: u8, a: u8) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba8(r, g, b, a));
    p
}

fn avatar(pix: &mut Pixmap, s: &Scene, cx: f32, cy: f32, r: f32) {
    fill_circle(pix, cx, cy, r + 3.0, (255, 255, 255, 60));
    match s.avatar {
        Some(a) => {
            // the picture, cut to a circle
            let size = (r * 2.0).round() as u32;
            if let Some(mut round) = Pixmap::new(size, size) {
                let scaled_ok = {
                    let sc = size as f32 / a.width().min(a.height()) as f32;
                    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
                    round.draw_pixmap(0, 0, a.as_ref(), &paint, Transform::from_scale(sc, sc), None);
                    true
                };
                if scaled_ok {
                    let mut mask = tiny_skia::Mask::new(size, size).unwrap();
                    let mut pb = tiny_skia::PathBuilder::new();
                    pb.push_circle(r, r, r);
                    if let Some(path) = pb.finish() {
                        mask.fill_path(&path, tiny_skia::FillRule::Winding, true, Transform::identity());
                        pix.draw_pixmap((cx - r) as i32, (cy - r) as i32, round.as_ref(), &PixmapPaint::default(), Transform::identity(), Some(&mask));
                    }
                }
            }
        }
        None => {
            fill_circle(pix, cx, cy, r, s.accent);
            let initial = s.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            let size = r * 1.1;
            let tw = draw::text_width(&s.fonts.bold, size, &initial);
            draw::draw_text(pix, &s.fonts.bold, size, cx - tw / 2.0, cy, (255, 255, 255, 255), &initial);
        }
    }
}

/// The password field: dots, or a hint. `light` draws it for a dark panel.
fn field(pix: &mut Pixmap, s: &Scene, x: f32, y: f32, w: f32, h: f32) {
    fill_rounded(pix, x, y, w, h, h / 2.6, (255, 255, 255, 30));
    fill_rounded(pix, x + 0.5, y + 0.5, w - 1.0, h - 1.0, h / 2.6, (0, 0, 0, 0));
    if s.dots == 0 {
        let hint = if s.waiting { "Please wait..." } else { "Password" };
        draw::draw_text(pix, &s.fonts.regular, h * 0.36, x + h * 0.5, y + h / 2.0, DIM, hint);
    } else {
        let shown = s.dots.min(20);
        let step = (h * 0.42).min((w - h) / shown.max(1) as f32);
        let total = shown as f32 * step;
        for i in 0..shown {
            fill_circle(pix, x + (w - total) / 2.0 + i as f32 * step + step / 2.0, y + h / 2.0, h * 0.1, TEXT);
        }
    }
    // the edge in the accent
    let a = s.accent;
    fill_rounded(pix, x, y + h - 2.0, w, 2.0, 1.0, (a.0, a.1, a.2, 200));
}

fn message(pix: &mut Pixmap, s: &Scene, x: f32, y: f32) {
    if !s.message.is_empty() {
        draw::draw_text(pix, &s.fonts.regular, 16.0, x, y, (240, 110, 110, 255), s.message);
    }
}

pub fn paint(d: Design, pix: &mut Pixmap, s: &Scene) {
    match d {
        Design::Split => split(pix, s),
        Design::Classic => classic(pix, s),
        Design::Zen => zen(pix, s),
        Design::Poster => poster(pix, s),
    }
}

fn split(pix: &mut Pixmap, s: &Scene) {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    background(pix, s, 0);
    shade_top_bottom(pix, 64, 115);
    // the clock, bottom left
    let big = (h * 0.13).max(48.0);
    draw::draw_text(pix, &s.fonts.bold, big, 72.0 * h / 1080.0 + 8.0, h - 64.0 * h / 1080.0 - big * 0.9, TEXT, &s.time);
    draw::draw_text(pix, &s.fonts.regular, (h * 0.028).max(16.0), 76.0 * h / 1080.0 + 8.0, h - 64.0 * h / 1080.0 - big * 0.25 + 4.0, (230, 226, 240, 220), &s.date);
    // the panel: the wallpaper blurred and darkened, a dark wash over it, a thin edge on its left
    let pw = (w * 0.34).clamp(420.0, 560.0).min(w);
    let px = w - pw;
    if let Some(b) = s.blurred {
        pix.draw_pixmap(0, 0, b.as_ref(), &PixmapPaint::default(), Transform::identity(), Some(&panel_mask(pix.width(), pix.height(), px)));
    }
    pix.fill_rect(Rect::from_xywh(px, 0.0, pw, h).unwrap(), &flat(14, 10, 26, 150), Transform::identity(), None);
    pix.fill_rect(Rect::from_xywh(px, 0.0, 1.0, h).unwrap(), &flat(255, 255, 255, 36), Transform::identity(), None);
    // who, and the field
    let cx = px + pw / 2.0;
    let margin = 56.0;
    avatar(pix, s, cx - pw / 2.0 + margin + 42.0, h / 2.0 - 130.0, 42.0);
    let left = px + margin;
    draw::draw_text(pix, &s.fonts.regular, 17.0, left, h / 2.0 - 52.0, DIM, &s.greeting);
    draw::draw_text(pix, &s.fonts.bold, 30.0, left, h / 2.0 - 18.0, TEXT, s.user);
    field(pix, s, left, h / 2.0 + 26.0, pw - margin * 2.0, 54.0);
    message(pix, s, left, h / 2.0 + 110.0);
}

fn panel_mask(w: u32, h: u32, from_x: f32) -> tiny_skia::Mask {
    let mut m = tiny_skia::Mask::new(w, h).unwrap();
    let mut pb = tiny_skia::PathBuilder::new();
    pb.push_rect(Rect::from_xywh(from_x, 0.0, w as f32 - from_x, h as f32).unwrap());
    if let Some(path) = pb.finish() {
        m.fill_path(&path, tiny_skia::FillRule::Winding, false, Transform::identity());
    }
    m
}

fn classic(pix: &mut Pixmap, s: &Scene) {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    pix.fill(Color::from_rgba8(10, 10, 16, 255));
    let f = s.fonts;
    let k = h / 720.0;
    let tw = draw::text_width(&f.bold, 104.0 * k, &s.time);
    draw::draw_text(pix, &f.bold, 104.0 * k, (w - tw) / 2.0, h * 0.26, (240, 240, 246, 255), &s.time);
    let dw = draw::text_width(&f.regular, 24.0 * k, &s.date);
    draw::draw_text(pix, &f.regular, 24.0 * k, (w - dw) / 2.0, h * 0.26 + 76.0 * k, (160, 160, 176, 255), &s.date);
    let (fw, fh) = (380.0 * k, 52.0 * k);
    let (fx, fy) = ((w - fw) / 2.0, h * 0.58);
    let uw = draw::text_width(&f.bold, 18.0 * k, s.user);
    draw::draw_text(pix, &f.bold, 18.0 * k, (w - uw) / 2.0, fy - 26.0 * k, (200, 200, 214, 255), s.user);
    field(pix, s, fx, fy, fw, fh);
    let mw = draw::text_width(&f.regular, 16.0, s.message);
    message(pix, s, (w - mw) / 2.0, fy + fh + 34.0 * k);
}

fn zen(pix: &mut Pixmap, s: &Scene) {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    background(pix, s, 90);
    shade_top_bottom(pix, 40, 90);
    let tw = draw::text_width(&s.fonts.regular, h * 0.1, &s.time);
    draw::draw_text(pix, &s.fonts.regular, h * 0.1, (w - tw) / 2.0, h * 0.38, TEXT, &s.time);
    let dw = draw::text_width(&s.fonts.regular, (h * 0.024).max(15.0), &s.date);
    draw::draw_text(pix, &s.fonts.regular, (h * 0.024).max(15.0), (w - dw) / 2.0, h * 0.38 + h * 0.07, DIM, &s.date);
    let fw = 300.0;
    field(pix, s, (w - fw) / 2.0, h * 0.58, fw, 44.0);
    let mw = draw::text_width(&s.fonts.regular, 16.0, s.message);
    message(pix, s, (w - mw) / 2.0, h * 0.58 + 76.0);
}

fn poster(pix: &mut Pixmap, s: &Scene) {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    background(pix, s, 70);
    shade_top_bottom(pix, 50, 110);
    let (hh, mm) = s.time.split_once(':').unwrap_or(("--", "--"));
    let size = h * 0.34;
    let hw = draw::text_width(&s.fonts.bold, size, hh);
    let mwid = draw::text_width(&s.fonts.bold, size, mm);
    draw::draw_text(pix, &s.fonts.bold, size, w - 90.0 - hw.max(mwid) + (hw.max(mwid) - hw), h * 0.30, TEXT, hh);
    draw::draw_text(pix, &s.fonts.bold, size, w - 90.0 - hw.max(mwid) + (hw.max(mwid) - mwid), h * 0.30 + size * 0.95, (s.accent.0, s.accent.1, s.accent.2, 255), mm);
    draw::draw_text(pix, &s.fonts.regular, (h * 0.022).max(14.0), 72.0, h - 150.0, DIM, &s.date);
    field(pix, s, 72.0, h - 110.0, 340.0, 46.0);
    message(pix, s, 72.0, h - 40.0);
}

/// A wallpaper picture, scaled to cover `w` x `h` (cropped from the middle).
pub fn cover(img: &Pixmap, w: u32, h: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(w, h)?;
    let sc = (w as f32 / img.width() as f32).max(h as f32 / img.height() as f32);
    let (tx, ty) = ((w as f32 - img.width() as f32 * sc) / 2.0, (h as f32 - img.height() as f32 * sc) / 2.0);
    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
    out.draw_pixmap(0, 0, img.as_ref(), &paint, Transform::from_scale(sc, sc).post_translate(tx, ty), None);
    Some(out)
}

/// A soft copy: shrunk, box-blurred a few times, grown back.
pub fn blur(src: &Pixmap) -> Option<Pixmap> {
    let (w, h) = (src.width(), src.height());
    let (sw, sh) = ((w / 8).max(2), (h / 8).max(2));
    let mut small = Pixmap::new(sw, sh)?;
    let paint = PixmapPaint { quality: FilterQuality::Bilinear, ..Default::default() };
    small.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(sw as f32 / w as f32, sh as f32 / h as f32), None);
    for _ in 0..3 {
        box_blur(&mut small, 3);
    }
    let mut out = Pixmap::new(w, h)?;
    out.draw_pixmap(0, 0, small.as_ref(), &paint, Transform::from_scale(w as f32 / sw as f32, h as f32 / sh as f32), None);
    Some(out)
}

fn blur_line(src: &[u8], dst: &mut [u8], start: usize, stride: usize, len: usize, c: usize, r: i32) {
    let at = |i: i32| src[(start + (i.clamp(0, len as i32 - 1) as usize) * stride) * 4 + c] as i32;
    let n = 2 * r + 1;
    let mut acc: i32 = (-r..=r).map(at).sum();
    for i in 0..len as i32 {
        dst[(start + i as usize * stride) * 4 + c] = (acc / n) as u8;
        acc += at(i + r + 1) - at(i - r);
    }
}

fn box_blur(p: &mut Pixmap, r: i32) {
    let (w, h) = (p.width() as usize, p.height() as usize);
    let a = p.data().to_vec();
    let mut b = vec![0u8; a.len()];
    for y in 0..h {
        for c in 0..4 {
            blur_line(&a, &mut b, y * w, 1, w, c, r);
        }
    }
    let mut out = vec![0u8; a.len()];
    for x in 0..w {
        for c in 0..4 {
            blur_line(&b, &mut out, x, w, h, c, r);
        }
    }
    p.data_mut().copy_from_slice(&out);
}

/// A picture file (PNG or JPEG ...) as a pixmap.
pub fn load_picture(path: &std::path::Path) -> Option<Pixmap> {
    let img = image::open(path).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let mut pm = Pixmap::new(w, h)?;
    for (dst, src) in pm.pixels_mut().iter_mut().zip(img.pixels()) {
        let a = src[3] as u32;
        let pm_ = |c: u8| ((c as u32 * a + 127) / 255) as u8;
        *dst = tiny_skia::PremultipliedColorU8::from_rgba(pm_(src[0]), pm_(src[1]), pm_(src[2]), src[3]).unwrap_or(*dst);
    }
    Some(pm)
}

/// The wallpaper named in ~/.config/labyrinth/wallpaper.
pub fn wallpaper_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    let p = PathBuf::from(std::fs::read_to_string(dir.join("labyrinth/wallpaper")).ok()?.trim());
    p.is_file().then_some(p)
}

pub fn avatar_path() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [home.join(".face"), home.join(".face.icon")].into_iter().find(|p| p.is_file())
}

pub fn greeting(hour: u32) -> String {
    match hour {
        5..=11 => "Good morning,",
        12..=17 => "Good afternoon,",
        _ => "Good evening,",
    }
    .to_string()
}
