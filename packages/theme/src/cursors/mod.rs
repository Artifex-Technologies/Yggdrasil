//! ygg-cursors in Rust: Yggdrasil's mouse cursors, drawn as the Python script drew them with Pillow (draw.rs: Pillow's
//! drawing; resample.rs: its Lanczos shrinking; text.rs: its text) and written as an X cursor theme. The pixels are
//! Pillow's, so the theme is the one the script made.

pub mod draw;
pub mod resample;
pub mod text;

use draw::{d_arc, d_ellipse_fill, d_ellipse_outline, d_line, d_polygon_fill, d_rounded_rectangle_fill, DrawError, Img, Ink};
use std::path::Path;

pub const DOC: &str = "ygg-cursors: draws Yggdrasil's mouse cursors and writes them as an X cursor theme (also used by Wayland programs).

  ygg-cursors OUTDIR            writes OUTDIR/cursors/* and OUTDIR/index.theme  (Name=Yggdrasil)
  ygg-cursors OUTDIR --preview  also writes OUTDIR/preview.png, a sheet of every shape

White shapes with a dark purple outline, so they read on any background; the busy ring uses the accent purple. Nothing is downloaded and
no other theme is used: the shapes are drawn here as Pillow draws them, at 8x and shrunk, in five sizes (24, 32, 48, 64, 96 px).
";

pub const SIZES: [i32; 5] = [24, 32, 48, 64, 96];
/// drawing is done at 8x and shrunk
const SS: i32 = 8;
const DARK: Ink = [24, 16, 40, 255];
const WHITE: Ink = [255, 255, 255, 255];
const PURPLE: Ink = [124, 84, 214, 255];
const RED: Ink = [225, 70, 82, 255];
/// outline width, in the 32-unit design grid
const OUT: f64 = 2.2;

type R<T> = Result<T, DrawError>;

/// ImagingFill2 with an "L" mask on an RGBA image (Image.paste(colour, mask=...) and ImageDraw's text)
pub fn fill_mask(im: &mut Img, ink: &Ink, mask: &Img, dx0: i32, dy0: i32) {
    let (mut xsize, mut ysize) = (mask.w as i64, mask.h as i64);
    let (mut dx0, mut dy0) = (dx0 as i64, dy0 as i64);
    let (mut sx0, mut sy0) = (0i64, 0i64);
    if dx0 < 0 {
        xsize += dx0;
        if xsize <= 0 {
            return;
        }
        sx0 = -dx0;
        dx0 = 0;
    }
    if dx0 + xsize > im.w as i64 {
        xsize = im.w as i64 - dx0;
    }
    if dy0 < 0 {
        ysize += dy0;
        if ysize <= 0 {
            return;
        }
        sy0 = -dy0;
        dy0 = 0;
    }
    if dy0 + ysize > im.h as i64 {
        ysize = im.h as i64 - dy0;
    }
    if xsize <= 0 || ysize <= 0 {
        return;
    }
    let blend = |m: u32, a: u32, b: u32| {
        let t = a * (255 - m) + b * m + 128;
        (((t >> 8) + t) >> 8) as u8
    };
    for y in 0..ysize {
        for x in 0..xsize {
            let m = mask.px[((y + sy0) * mask.w as i64 + x + sx0) as usize] as u32;
            let o = (((y + dy0) * im.w as i64 + x + dx0) * 4) as usize;
            for (i, &v) in ink.iter().enumerate() {
                let mut cm = m;
                if i != 3 && cm != 0 {
                    cm = 255 - (255 - cm) * (1 - (255 - im.px[o + 3] as u32) / 255);
                }
                im.px[o + i] = blend(cm, im.px[o + i] as u32, v as u32);
            }
        }
    }
}

struct Canvas {
    size: i32,
    k: f64,
    img: Img,
}

type Pt = (f64, f64);

impl Canvas {
    fn new(size: i32) -> Canvas {
        Canvas { size, k: size as f64 / 32.0 * SS as f64, img: Img::new(size * SS, size * SS, 4, [0, 0, 0, 0]) }
    }

    fn p(&self, pts: &[Pt]) -> Vec<Pt> {
        pts.iter().map(|&(x, y)| (x * self.k, y * self.k)).collect()
    }

    fn dot(&mut self, (x, y): Pt, r: f64, ink: &Ink) -> R<()> {
        d_ellipse_fill(&mut self.img, [x - r, y - r, x + r, y + r], ink)
    }

    fn poly(&mut self, pts: &[Pt], fill: Ink, outline: Option<Ink>, w: f64) -> R<()> {
        let pts = self.p(pts);
        if let Some(o) = outline {
            let mut closed = pts.clone();
            closed.push(pts[0]);
            closed.push(pts[1]);
            d_line(&mut self.img, &closed, &o, (w * self.k) as i32, true)?;
            for &pt in &pts {
                let r = w * self.k / 2.0;
                self.dot(pt, r, &o)?;
            }
        }
        d_polygon_fill(&mut self.img, &pts, &fill);
        Ok(())
    }

    fn line(&mut self, pts: &[Pt], w: f64, fill: Ink, outline: Option<Ink>, ow: f64) -> R<()> {
        let pp = self.p(pts);
        let ends = [pp[0], pp[pp.len() - 1]];
        if let Some(o) = outline {
            d_line(&mut self.img, &pp, &o, ((w + ow) * self.k) as i32, true)?;
            for pt in ends {
                let r = (w + ow) * self.k / 2.0;
                self.dot(pt, r, &o)?;
            }
        }
        d_line(&mut self.img, &pp, &fill, (w * self.k) as i32, true)?;
        for pt in ends {
            let r = w * self.k / 2.0;
            self.dot(pt, r, &fill)?;
        }
        Ok(())
    }

    fn circle(&mut self, c: Pt, r: f64, fill: Option<Ink>, outline: Option<Ink>, w: f64) -> R<()> {
        let (x, y) = (c.0 * self.k, c.1 * self.k);
        if let Some(o) = outline {
            let big = (r + w / 2.0) * self.k;
            self.dot((x, y), big, &o)?;
        }
        let small = (r - if outline.is_some() { w / 2.0 } else { 0.0 }) * self.k;
        if let Some(f) = fill {
            self.dot((x, y), small, &f)?;
        }
        Ok(())
    }

    fn rrect(&mut self, b: [f64; 4], r: f64, fill: Ink, outline: Option<Ink>, w: f64) -> R<()> {
        let [x0, y0, x1, y1] = b.map(|v| v * self.k);
        if let Some(o) = outline {
            let g = w / 2.0 * self.k;
            d_rounded_rectangle_fill(&mut self.img, [x0 - g, y0 - g, x1 + g, y1 + g], r * self.k + g, &o)?;
        }
        d_rounded_rectangle_fill(&mut self.img, [x0, y0, x1, y1], r * self.k, &fill)
    }

    fn text(&mut self, xy: Pt, s: &str, px: f64, fill: Ink) {
        text::draw_text_mm(&mut self.img, (xy.0 * self.k, xy.1 * self.k), s, px * self.k, &fill);
    }

    fn done(self) -> Img {
        resample::resize_lanczos(&self.img, self.size, self.size)
    }
}

const ARROW: [Pt; 7] = [(5.0, 3.0), (5.0, 25.5), (10.8, 20.0), (14.6, 28.5), (18.4, 26.8), (14.6, 18.5), (22.5, 18.5)];
const HAND: [Pt; 23] = [
    (10.0, 5.0),
    (10.8, 3.0),
    (13.2, 3.0),
    (14.0, 5.0),
    (14.0, 13.5),
    (15.5, 12.5),
    (17.5, 12.5),
    (18.5, 13.8),
    (19.5, 13.2),
    (21.5, 13.2),
    (22.8, 14.8),
    (23.6, 14.5),
    (25.4, 15.0),
    (26.4, 17.2),
    (26.4, 23.0),
    (24.0, 28.5),
    (15.0, 28.5),
    (11.8, 25.0),
    (8.5, 19.5),
    (7.3, 16.5),
    (8.0, 15.5),
    (9.8, 15.8),
    (10.2, 16.5),
];

fn arrow(c: &mut Canvas) -> R<()> {
    c.poly(&ARROW, WHITE, Some(DARK), OUT)
}

/// A filled triangle with its point at `tip`, pointing along `a` (radians).
fn arrow_head(c: &mut Canvas, tip: Pt, a: f64, n: f64) -> R<()> {
    let left = (tip.0 - n * a.cos() + n * 0.85 * a.sin(), tip.1 - n * a.sin() - n * 0.85 * a.cos());
    let right = (tip.0 - n * a.cos() - n * 0.85 * a.sin(), tip.1 - n * a.sin() + n * 0.85 * a.cos());
    c.poly(&[tip, left, right], WHITE, Some(DARK), 1.6)
}

/// A line with a head at both ends, through the middle, at angle `ang`.
fn double_arrow(c: &mut Canvas, ang: f64) -> R<Pt> {
    let (cx, cy) = (16.0, 16.0);
    let l = 11.5;
    let (ux, uy) = (ang.cos(), ang.sin());
    let a = (cx - ux * l, cy - uy * l);
    let b = (cx + ux * l, cy + uy * l);
    c.line(&[a, b], 2.4, WHITE, Some(DARK), OUT)?;
    arrow_head(c, (b.0 + ux * 1.2, b.1 + uy * 1.2), ang, 5.2)?;
    arrow_head(c, (a.0 - ux * 1.2, a.1 - uy * 1.2), ang + std::f64::consts::PI, 5.2)?;
    Ok((16.0, 16.0))
}

fn shape_text(c: &mut Canvas, vertical: bool) -> R<Pt> {
    let l = |c: &mut Canvas, a: Pt, b: Pt| c.line(&[a, b], 2.0, WHITE, Some(DARK), OUT);
    if vertical {
        l(c, (5.0, 16.0), (27.0, 16.0))?;
        l(c, (5.0, 12.5), (5.0, 19.5))?;
        l(c, (27.0, 12.5), (27.0, 19.5))?;
    } else {
        l(c, (16.0, 5.0), (16.0, 27.0))?;
        l(c, (12.3, 5.0), (19.7, 5.0))?;
        l(c, (12.3, 27.0), (19.7, 27.0))?;
    }
    Ok((16.0, 16.0))
}

fn palm(c: &mut Canvas, closed: bool) -> R<()> {
    if closed {
        c.rrect([8.5, 14.5, 25.5, 27.5], 5.0, WHITE, Some(DARK), OUT)?;
        for i in 0..4 {
            c.circle((11.2 + i as f64 * 4.4, 14.8), 2.6, Some(WHITE), Some(DARK), OUT)?;
        }
        c.rrect([8.8, 15.0, 25.2, 27.0], 4.6, WHITE, None, OUT)
    } else {
        for (i, top) in [5.5, 3.5, 4.5, 7.5].into_iter().enumerate() {
            c.rrect([9.4 + i as f64 * 3.9, top, 12.6 + i as f64 * 3.9, 20.0], 1.6, WHITE, Some(DARK), OUT)?;
        }
        c.rrect([6.0, 14.0, 12.5, 21.0], 2.6, WHITE, Some(DARK), OUT)?;
        c.rrect([8.5, 14.5, 25.5, 27.5], 5.0, WHITE, Some(DARK), OUT)?;
        c.rrect([9.0, 15.0, 25.0, 27.0], 4.6, WHITE, None, OUT)
    }
}

/// A busy ring: a white band with a dark edge on both sides, empty inside, and a purple arc that turns with the frame.
fn ring(c: &mut Canvas, cx: f64, cy: f64, r: f64, f: i32, w: f64) -> R<()> {
    let frames = 12;
    let k = c.k;
    let disc = |c: &mut Canvas, rad: f64, fill: Ink| {
        d_ellipse_fill(&mut c.img, [(cx - rad) * k, (cy - rad) * k, (cx + rad) * k, (cy + rad) * k], &fill)
    };
    disc(c, r + w / 2.0 + OUT / 2.0, DARK)?;
    disc(c, r + w / 2.0, WHITE)?;
    disc(c, r - w / 2.0, DARK)?;
    let mut hole = Img::new(c.img.w, c.img.h, 1, [0; 4]);
    let rad = r - w / 2.0 - OUT / 2.0;
    d_ellipse_fill(&mut hole, [(cx - rad) * k, (cy - rad) * k, (cx + rad) * k, (cy + rad) * k], &[255, 0, 0, 0])?;
    fill_mask(&mut c.img, &[0, 0, 0, 0], &hole, 0, 0);
    let a0 = (f * 360) as f64 / frames as f64 - 90.0;
    d_arc(&mut c.img, [(cx - r) * k, (cy - r) * k, (cx + r) * k, (cy + r) * k], a0, a0 + 100.0, &PURPLE, (w * k) as i32)
}

fn shape_zoom(c: &mut Canvas, plus: bool) -> R<Pt> {
    c.line(&[(18.5, 18.5), (27.0, 27.0)], 3.6, WHITE, Some(DARK), OUT)?;
    c.circle((13.0, 13.0), 8.2, Some([255, 255, 255, 120]), Some(DARK), 2.2)?;
    c.line(&[(9.5, 13.0), (16.5, 13.0)], 1.8, DARK, None, OUT)?;
    if plus {
        c.line(&[(13.0, 9.5), (13.0, 16.5)], 1.8, DARK, None, OUT)?;
    }
    Ok((13.0, 13.0))
}

/// Draw one shape's frame; its hotspot in the 32-unit grid.
fn draw_shape(c: &mut Canvas, shape: &str, f: i32) -> R<Pt> {
    use std::f64::consts::PI;
    match shape {
        "arrow" => {
            arrow(c)?;
            Ok((5.0, 3.0))
        }
        "text" => shape_text(c, false),
        "vtext" => shape_text(c, true),
        "pointer" => {
            c.poly(&HAND, WHITE, Some(DARK), OUT)?;
            Ok((12.0, 3.0))
        }
        "grab" => {
            palm(c, false)?;
            Ok((16.0, 16.0))
        }
        "grabbing" => {
            palm(c, true)?;
            Ok((16.0, 17.0))
        }
        "cross" => {
            c.line(&[(3.0, 16.0), (12.5, 16.0)], 1.8, WHITE, Some(DARK), OUT)?;
            c.line(&[(19.5, 16.0), (29.0, 16.0)], 1.8, WHITE, Some(DARK), OUT)?;
            c.line(&[(16.0, 3.0), (16.0, 12.5)], 1.8, WHITE, Some(DARK), OUT)?;
            c.line(&[(16.0, 19.5), (16.0, 29.0)], 1.8, WHITE, Some(DARK), OUT)?;
            c.circle((16.0, 16.0), 1.6, Some(DARK), Some(WHITE), 1.2)?;
            Ok((16.0, 16.0))
        }
        "no" => {
            c.circle((16.0, 16.0), 10.0, Some([255, 255, 255, 235]), Some(DARK), 2.2)?;
            c.circle((16.0, 16.0), 8.4, None, None, OUT)?;
            let k = c.k;
            d_ellipse_outline(
                &mut c.img,
                [16.0 * k - 8.2 * k, 16.0 * k - 8.2 * k, 16.0 * k + 8.2 * k, 16.0 * k + 8.2 * k],
                &RED,
                (2.8 * k) as i32,
            )?;
            d_line(
                &mut c.img,
                &[(16.0 * k - 5.8 * k, 16.0 * k - 5.8 * k), (16.0 * k + 5.8 * k, 16.0 * k + 5.8 * k)],
                &RED,
                (2.8 * k) as i32,
                false,
            )?;
            Ok((16.0, 16.0))
        }
        "wait" => {
            ring(c, 16.0, 16.0, 9.5, f, 3.0)?;
            Ok((16.0, 16.0))
        }
        "progress" => {
            arrow(c)?;
            ring(c, 23.0, 23.0, 5.2, f, 2.4)?;
            Ok((5.0, 3.0))
        }
        "move" => {
            c.line(&[(16.0, 5.0), (16.0, 27.0)], 2.2, WHITE, Some(DARK), OUT)?;
            c.line(&[(5.0, 16.0), (27.0, 16.0)], 2.2, WHITE, Some(DARK), OUT)?;
            for a in [0.0f64, 90.0, 180.0, 270.0] {
                let r = a.to_radians();
                arrow_head(c, (16.0 + 13.2 * r.cos(), 16.0 + 13.2 * r.sin()), r, 4.6)?;
            }
            Ok((16.0, 16.0))
        }
        "copy" => {
            arrow(c)?;
            c.circle((23.5, 23.5), 5.6, Some(WHITE), Some(DARK), OUT)?;
            c.line(&[(23.5, 20.7), (23.5, 26.3)], 1.5, DARK, None, OUT)?;
            c.line(&[(20.7, 23.5), (26.3, 23.5)], 1.5, DARK, None, OUT)?;
            Ok((5.0, 3.0))
        }
        "alias" => {
            arrow(c)?;
            c.circle((23.5, 23.5), 5.6, Some(WHITE), Some(DARK), OUT)?;
            c.line(&[(20.8, 25.5), (25.8, 21.5)], 1.5, DARK, None, OUT)?;
            c.poly(&[(26.6, 20.8), (26.2, 25.0), (22.2, 21.2)], DARK, None, OUT)?;
            Ok((5.0, 3.0))
        }
        "menu" => {
            arrow(c)?;
            c.rrect([17.0, 19.0, 28.5, 28.5], 1.8, WHITE, Some(DARK), OUT)?;
            for y in [21.6, 24.0, 26.4] {
                c.line(&[(19.0, y), (26.5, y)], 1.0, DARK, None, OUT)?;
            }
            Ok((5.0, 3.0))
        }
        "help" => {
            arrow(c)?;
            c.circle((23.0, 22.5), 6.0, Some(WHITE), Some(DARK), OUT)?;
            c.text((23.0, 22.8), "?", 9.0, DARK);
            Ok((5.0, 3.0))
        }
        "cell" => {
            c.line(&[(16.0, 6.0), (16.0, 26.0)], 4.2, WHITE, Some(DARK), OUT)?;
            c.line(&[(6.0, 16.0), (26.0, 16.0)], 4.2, WHITE, Some(DARK), OUT)?;
            Ok((16.0, 16.0))
        }
        "zoomin" => shape_zoom(c, true),
        "zoomout" => shape_zoom(c, false),
        "ew" => double_arrow(c, 0.0),
        "ns" => double_arrow(c, PI / 2.0),
        "nwse" => double_arrow(c, PI / 4.0),
        _ => double_arrow(c, -PI / 4.0),
    }
}

/// The shapes in the script's order: (shape, frames, cursor names; the first is the file, the others link to it)
pub const SHAPES: [(&str, i32, &[&str]); 22] = [
    (
        "arrow",
        1,
        &[
            "left_ptr",
            "default",
            "arrow",
            "top_left_arrow",
            "right_ptr",
            "center_ptr",
            "dnd-move",
            "dnd-none",
            "draft",
            "context-menu-less",
        ],
    ),
    ("text", 1, &["text", "xterm", "ibeam"]),
    ("vtext", 1, &["vertical-text"]),
    ("pointer", 1, &["pointer", "hand1", "hand2", "pointing_hand", "link"]),
    ("grab", 1, &["grab", "openhand", "hand"]),
    ("grabbing", 1, &["grabbing", "closedhand", "dnd-grab"]),
    ("cross", 1, &["crosshair", "cross", "tcross", "color-picker", "pencil", "target"]),
    ("no", 1, &["not-allowed", "forbidden", "crossed_circle", "no-drop", "dnd-no-drop", "circle", "pirate"]),
    ("wait", 12, &["wait", "watch"]),
    ("progress", 12, &["progress", "left_ptr_watch", "half-busy"]),
    ("move", 1, &["move", "fleur", "all-scroll", "size_all"]),
    ("copy", 1, &["copy", "dnd-copy"]),
    ("alias", 1, &["alias", "dnd-link"]),
    ("menu", 1, &["context-menu"]),
    ("help", 1, &["help", "question_arrow", "whats_this", "left_ptr_help"]),
    ("cell", 1, &["cell", "plus"]),
    ("zoomin", 1, &["zoom-in"]),
    ("zoomout", 1, &["zoom-out"]),
    (
        "ew",
        1,
        &[
            "ew-resize",
            "sb_h_double_arrow",
            "h_double_arrow",
            "col-resize",
            "split_h",
            "e-resize",
            "w-resize",
            "left_side",
            "right_side",
            "size_hor",
            "base_arrow_up",
        ],
    ),
    (
        "ns",
        1,
        &[
            "ns-resize",
            "sb_v_double_arrow",
            "v_double_arrow",
            "row-resize",
            "split_v",
            "n-resize",
            "s-resize",
            "top_side",
            "bottom_side",
            "size_ver",
            "base_arrow_down",
        ],
    ),
    ("nwse", 1, &["nwse-resize", "size_fdiag", "bd_double_arrow", "nw-resize", "se-resize", "top_left_corner", "bottom_right_corner"]),
    ("nesw", 1, &["nesw-resize", "size_bdiag", "fd_double_arrow", "ne-resize", "sw-resize", "top_right_corner", "bottom_left_corner"]),
];

/// One frame of a shape at a size: the picture and the hotspot in pixels.
pub fn render(shape: &str, size: i32, frame: i32) -> R<Frame> {
    let mut c = Canvas::new(size);
    let hot = draw_shape(&mut c, shape, frame)?;
    let round = |v: f64| (v * size as f64 / 32.0).round_ties_even() as u32;
    Ok((c.done(), (round(hot.0), round(hot.1))))
}

/// One frame: the picture and its hotspot
pub type Frame = (Img, (u32, u32));

/// The X cursor file: header, table of contents, one image chunk per picture (premultiplied ARGB, little endian).
pub fn xcursor(frames_by_size: &[(i32, Vec<Frame>)], delay: u32) -> Vec<u8> {
    let mut chunks: Vec<(u32, Vec<u8>)> = vec![];
    for (size, frames) in frames_by_size {
        for (img, hot) in frames {
            let mut data = Vec::with_capacity(36 + img.px.len());
            for v in [36, 0xFFFD0002, *size as u32, 1, img.w as u32, img.h as u32, hot.0, hot.1, delay] {
                data.extend(v.to_le_bytes());
            }
            for p in img.px.chunks(4) {
                let (r, g, b, a) = (p[0] as u32, p[1] as u32, p[2] as u32, p[3] as u32);
                data.extend(((a << 24) | ((r * a / 255) << 16) | ((g * a / 255) << 8) | (b * a / 255)).to_le_bytes());
            }
            chunks.push((*size as u32, data));
        }
    }
    let mut pos = 16 + 12 * chunks.len() as u32;
    let mut out = b"Xcur".to_vec();
    for v in [16u32, 0x10000, chunks.len() as u32] {
        out.extend(v.to_le_bytes());
    }
    for (size, data) in &chunks {
        for v in [0xFFFD0002u32, *size, pos] {
            out.extend(v.to_le_bytes());
        }
        pos += data.len() as u32;
    }
    for (_, data) in chunks {
        out.extend(data);
    }
    out
}

fn fail(msg: impl std::fmt::Display) -> ! {
    use std::io::Write;
    let _ = std::io::stdout().flush();
    eprintln!("ygg-cursors: {msg}");
    std::process::exit(1)
}

/// ygg-cursors OUTDIR [--preview]; the exit code.
pub fn main(argv0: &str, args: &[String]) -> i32 {
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let Some(out) = positional.first() else {
        println!("{DOC}");
        return 2;
    };
    let preview = argv0 == "--preview" || args.iter().any(|a| a == "--preview");
    let out = Path::new(out.as_str());
    let cur = out.join("cursors");
    if let Err(e) = std::fs::create_dir_all(&cur) {
        fail(format!("{}: {e}", cur.display()));
    }
    let mut sheet: Vec<Img> = vec![];
    for (shape, frames, names) in SHAPES {
        let mut by_size = vec![];
        for size in SIZES {
            let mut fr = vec![];
            for f in 0..frames {
                fr.push(render(shape, size, f).unwrap_or_else(|e| fail(e.0)));
            }
            by_size.push((size, fr));
        }
        let data = xcursor(&by_size, if frames > 1 { 80 } else { 0 });
        let main_name = names[0];
        let path = cur.join(main_name);
        if let Err(e) = std::fs::write(&path, data) {
            fail(format!("{}: {e}", path.display()));
        }
        for n in &names[1..] {
            let p = cur.join(n);
            if std::fs::symlink_metadata(&p).is_ok() {
                if let Err(e) = std::fs::remove_file(&p) {
                    fail(format!("{}: {e}", p.display()));
                }
            }
            if let Err(e) = std::os::unix::fs::symlink(main_name, &p) {
                fail(format!("{}: {e}", p.display()));
            }
        }
        sheet.push(by_size.swap_remove(2).1.swap_remove(0).0);
    }
    let index = out.join("index.theme");
    if let Err(e) =
        std::fs::write(&index, "[Icon Theme]\nName=Yggdrasil\nComment=Yggdrasil's mouse cursors: white shapes with a dark purple outline\n")
    {
        fail(format!("{}: {e}", index.display()));
    }
    if preview {
        let cols = 8;
        let rows = (sheet.len() as i32 + cols - 1) / cols;
        let mut s = Img::new(cols * 64, rows * 64, 4, [60, 60, 70, 255]);
        for (i, im) in sheet.iter().enumerate() {
            let i = i as i32;
            resample::alpha_composite(&mut s, im, (i % cols) * 64 + 8, (i / cols) * 64 + 8);
        }
        let big = resample::resize_lanczos(&s, cols * 96, rows * 96);
        let p = out.join("preview.png");
        if let Err(e) = std::fs::write(&p, resample::png(&big)) {
            fail(format!("{}: {e}", p.display()));
        }
    }
    let names: usize = SHAPES.iter().map(|s| s.2.len()).sum();
    println!("{names} cursor names, {} shapes, in {}", SHAPES.len(), cur.display());
    0
}
