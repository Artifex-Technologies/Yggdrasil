//! Icons drawn as vector shapes, so they look the same everywhere and need no icon font.
//! (egui's built-in font draws arrows and many symbols as empty boxes; EdenExplorer solves
//! that with the Phosphor icon font, this solves it without a font.)
//!
//! Each icon is laid out in a unit square and scaled to the rectangle it is given.

use eframe::egui::{vec2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Back,
    Forward,
    Up,
    Refresh,
    NewFolder,
    NewFile,
    Star,
    StarFilled,
    Menu,
    Folder,
    File,
    Drive,
    Close,
    Plus,
    List,
    Grid,
    Search,
    Chevron,
    Home,
    Computer,
    Cloud,
    Pin,
    SortUp,
    SortDown,
    Archive,
    Preview,
}

struct Frame {
    origin: Pos2,
    size: f32,
}

impl Frame {
    fn new(rect: Rect) -> Self {
        let size = rect.width().min(rect.height());
        Frame { origin: rect.center() - vec2(size / 2.0, size / 2.0), size }
    }
    fn p(&self, x: f32, y: f32) -> Pos2 {
        self.origin + vec2(x * self.size, y * self.size)
    }
    fn rect(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.p(x0, y0), self.p(x1, y1))
    }
    fn stroke(&self, color: Color32) -> Stroke {
        Stroke::new((self.size * 0.085).max(1.3), color)
    }
}

fn line(p: &Painter, f: &Frame, pts: &[(f32, f32)], color: Color32) {
    p.add(Shape::line(pts.iter().map(|&(x, y)| f.p(x, y)).collect(), f.stroke(color)));
}

fn arc(cx: f32, cy: f32, r: f32, from_deg: f32, to_deg: f32, steps: usize) -> Vec<(f32, f32)> {
    (0..=steps)
        .map(|i| {
            let a = (from_deg + (to_deg - from_deg) * i as f32 / steps as f32).to_radians();
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect()
}

fn darker(c: Color32, k: f32) -> Color32 {
    Color32::from_rgba_unmultiplied((c.r() as f32 * k) as u8, (c.g() as f32 * k) as u8, (c.b() as f32 * k) as u8, c.a())
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let f = Frame::new(rect);
    let s = f.stroke(color);
    match icon {
        Icon::Back => {
            line(p, &f, &[(0.2, 0.5), (0.8, 0.5)], color);
            line(p, &f, &[(0.45, 0.25), (0.2, 0.5), (0.45, 0.75)], color);
        }
        Icon::Forward => {
            line(p, &f, &[(0.2, 0.5), (0.8, 0.5)], color);
            line(p, &f, &[(0.55, 0.25), (0.8, 0.5), (0.55, 0.75)], color);
        }
        Icon::Up => {
            line(p, &f, &[(0.5, 0.8), (0.5, 0.2)], color);
            line(p, &f, &[(0.25, 0.45), (0.5, 0.2), (0.75, 0.45)], color);
        }
        Icon::Refresh => {
            // a clockwise circle (↻) open at the top right, the arrow head at its end pointing on along the circle
            let end = -50f32;
            line(p, &f, &arc(0.5, 0.5, 0.3, end - 290.0, end, 24), color);
            let a = end.to_radians();
            let tip = (0.5 + 0.3 * a.cos(), 0.5 + 0.3 * a.sin());
            let (dx, dy) = (-a.sin(), a.cos()); // the way the circle runs at its end
            let (len, wide) = (0.16, 0.12);
            line(
                p,
                &f,
                &[
                    (tip.0 - dx * len - dy * wide, tip.1 - dy * len + dx * wide),
                    tip,
                    (tip.0 - dx * len + dy * wide, tip.1 - dy * len - dx * wide),
                ],
                color,
            );
        }
        Icon::NewFolder => {
            p.rect_stroke(f.rect(0.1, 0.3, 0.9, 0.82), CornerRadius::same(2), s, StrokeKind::Inside);
            line(p, &f, &[(0.1, 0.3), (0.1, 0.2), (0.4, 0.2), (0.48, 0.3)], color);
            line(p, &f, &[(0.5, 0.45), (0.5, 0.68)], color);
            line(p, &f, &[(0.385, 0.565), (0.615, 0.565)], color);
        }
        Icon::NewFile => {
            line(p, &f, &[(0.22, 0.1), (0.6, 0.1), (0.8, 0.3), (0.8, 0.9), (0.22, 0.9), (0.22, 0.1)], color);
            line(p, &f, &[(0.6, 0.1), (0.6, 0.3), (0.8, 0.3)], color);
            line(p, &f, &[(0.51, 0.48), (0.51, 0.76)], color);
            line(p, &f, &[(0.37, 0.62), (0.65, 0.62)], color);
        }
        Icon::Star | Icon::StarFilled => {
            let mut pts = Vec::new();
            for i in 0..10 {
                let a = (-90.0 + i as f32 * 36.0).to_radians();
                let r = if i % 2 == 0 { 0.42 } else { 0.18 };
                pts.push(f.p(0.5 + r * a.cos(), 0.54 + r * a.sin()));
            }
            if icon == Icon::StarFilled {
                // A star is not convex; fill it as a fan of triangles from the centre.
                let c = f.p(0.5, 0.54);
                for i in 0..10 {
                    p.add(Shape::convex_polygon(vec![c, pts[i], pts[(i + 1) % 10]], color, Stroke::NONE));
                }
            } else {
                pts.push(pts[0]);
                p.add(Shape::line(pts, s));
            }
        }
        Icon::Menu => {
            for y in [0.28, 0.5, 0.72] {
                line(p, &f, &[(0.18, y), (0.82, y)], color);
            }
        }
        Icon::Folder => {
            p.rect_filled(f.rect(0.06, 0.2, 0.46, 0.4), CornerRadius::same(2), darker(color, 0.78));
            p.rect_filled(f.rect(0.06, 0.3, 0.94, 0.84), CornerRadius::same(2), color);
        }
        Icon::File => {
            let page = vec![f.p(0.2, 0.08), f.p(0.6, 0.08), f.p(0.82, 0.3), f.p(0.82, 0.92), f.p(0.2, 0.92)];
            p.add(Shape::convex_polygon(page, color, Stroke::NONE));
            p.add(Shape::convex_polygon(vec![f.p(0.6, 0.08), f.p(0.82, 0.3), f.p(0.6, 0.3)], darker(color, 0.7), Stroke::NONE));
            for y in [0.5, 0.64, 0.78] {
                p.line_segment([f.p(0.32, y), f.p(0.7, y)], Stroke::new((f.size * 0.05).max(1.0), darker(color, 0.55)));
            }
        }
        Icon::Drive => {
            p.rect_filled(f.rect(0.06, 0.4, 0.94, 0.8), CornerRadius::same(3), color);
            p.rect_filled(f.rect(0.14, 0.26, 0.86, 0.42), CornerRadius::same(2), darker(color, 0.7));
            p.circle_filled(f.p(0.8, 0.62), f.size * 0.045, Color32::from_rgb(90, 200, 120));
            p.line_segment([f.p(0.16, 0.62), f.p(0.5, 0.62)], Stroke::new((f.size * 0.05).max(1.0), darker(color, 0.55)));
        }
        Icon::Close => {
            line(p, &f, &[(0.27, 0.27), (0.73, 0.73)], color);
            line(p, &f, &[(0.73, 0.27), (0.27, 0.73)], color);
        }
        Icon::Plus => {
            line(p, &f, &[(0.5, 0.2), (0.5, 0.8)], color);
            line(p, &f, &[(0.2, 0.5), (0.8, 0.5)], color);
        }
        Icon::List => {
            for y in [0.28, 0.5, 0.72] {
                p.circle_filled(f.p(0.2, y), f.size * 0.045, color);
                line(p, &f, &[(0.34, y), (0.84, y)], color);
            }
        }
        Icon::Grid => {
            for (x, y) in [(0.16, 0.16), (0.55, 0.16), (0.16, 0.55), (0.55, 0.55)] {
                p.rect_stroke(f.rect(x, y, x + 0.29, y + 0.29), CornerRadius::same(2), s, StrokeKind::Inside);
            }
        }
        Icon::Search => {
            p.circle_stroke(f.p(0.43, 0.43), f.size * 0.27, s);
            line(p, &f, &[(0.63, 0.63), (0.86, 0.86)], color);
        }
        Icon::Chevron => {
            line(p, &f, &[(0.38, 0.22), (0.66, 0.5), (0.38, 0.78)], color);
        }
        Icon::Home => {
            line(p, &f, &[(0.1, 0.5), (0.5, 0.14), (0.9, 0.5)], color);
            line(p, &f, &[(0.2, 0.46), (0.2, 0.86), (0.8, 0.86), (0.8, 0.46)], color);
            line(p, &f, &[(0.42, 0.86), (0.42, 0.6), (0.58, 0.6), (0.58, 0.86)], color);
        }
        Icon::Computer => {
            p.rect_stroke(f.rect(0.1, 0.2, 0.9, 0.66), CornerRadius::same(3), s, StrokeKind::Inside);
            line(p, &f, &[(0.5, 0.66), (0.5, 0.82)], color);
            line(p, &f, &[(0.3, 0.84), (0.7, 0.84)], color);
        }
        Icon::Cloud => {
            p.circle_filled(f.p(0.34, 0.58), f.size * 0.2, color);
            p.circle_filled(f.p(0.52, 0.42), f.size * 0.24, color);
            p.circle_filled(f.p(0.7, 0.58), f.size * 0.2, color);
            p.rect_filled(f.rect(0.34, 0.55, 0.7, 0.78), CornerRadius::same(2), color);
        }
        Icon::SortUp => {
            p.add(Shape::convex_polygon(vec![f.p(0.2, 0.68), f.p(0.8, 0.68), f.p(0.5, 0.28)], color, Stroke::NONE));
        }
        Icon::SortDown => {
            p.add(Shape::convex_polygon(vec![f.p(0.2, 0.32), f.p(0.8, 0.32), f.p(0.5, 0.72)], color, Stroke::NONE));
        }
        Icon::Archive => {
            p.rect_filled(f.rect(0.16, 0.1, 0.84, 0.9), CornerRadius::same(3), color);
            let dark = darker(color, 0.55);
            for k in 0..5 {
                let y = 0.16 + k as f32 * 0.1;
                let x = if k % 2 == 0 { 0.46 } else { 0.5 };
                p.rect_filled(f.rect(x, y, x + 0.08, y + 0.07), CornerRadius::ZERO, dark);
            }
            p.rect_stroke(
                f.rect(0.43, 0.66, 0.57, 0.84),
                CornerRadius::same(2),
                Stroke::new((f.size * 0.06).max(1.0), dark),
                StrokeKind::Inside,
            );
        }
        Icon::Preview => {
            p.rect_stroke(f.rect(0.1, 0.18, 0.9, 0.82), CornerRadius::same(3), s, StrokeKind::Inside);
            p.rect_filled(f.rect(0.62, 0.18, 0.9, 0.82), CornerRadius { nw: 0, ne: 3, sw: 0, se: 3 }, color.gamma_multiply(0.6));
        }
        Icon::Pin => {
            p.circle_filled(f.p(0.5, 0.4), f.size * 0.2, color);
            line(p, &f, &[(0.5, 0.55), (0.5, 0.9)], color);
        }
    }
}

/// The well-known folders' pictures, in full colour (our own drawings, in the manner of Windows Explorer's): a monitor for
/// Desktop, a page for Documents, an arrow into a tray for Downloads, a note for Music, a landscape for Pictures, a film strip
/// for Videos, a house for Home, a cube for 3D Objects.
pub fn paint_special(p: &Painter, rect: Rect, which: minos_core::places::Special) {
    use minos_core::places::Special;
    let f = Frame::new(rect);
    let poly = |pts: &[(f32, f32)], c: Color32| {
        p.add(Shape::convex_polygon(pts.iter().map(|&(x, y)| f.p(x, y)).collect(), c, Stroke::NONE));
    };
    let blue = Color32::from_rgb(52, 132, 240);
    let deep = Color32::from_rgb(28, 92, 196);
    let rim = Stroke::new((f.size * 0.045).max(1.0), Color32::from_rgb(70, 78, 92));
    match which {
        Special::Desktop => {
            p.rect_filled(f.rect(0.06, 0.14, 0.94, 0.7), CornerRadius::same(2), Color32::from_rgb(52, 60, 74));
            p.rect_filled(f.rect(0.11, 0.19, 0.89, 0.65), CornerRadius::same(1), blue);
            p.rect_filled(f.rect(0.11, 0.42, 0.89, 0.65), CornerRadius::same(1), deep);
            p.rect_filled(f.rect(0.43, 0.7, 0.57, 0.8), CornerRadius::ZERO, Color32::from_rgb(150, 156, 168));
            p.rect_filled(f.rect(0.28, 0.8, 0.72, 0.87), CornerRadius::same(1), Color32::from_rgb(150, 156, 168));
        }
        Special::Documents => {
            poly(&[(0.2, 0.06), (0.62, 0.06), (0.82, 0.26), (0.82, 0.94), (0.2, 0.94)], Color32::from_rgb(246, 248, 251));
            p.add(Shape::closed_line(
                [(0.2, 0.06), (0.62, 0.06), (0.82, 0.26), (0.82, 0.94), (0.2, 0.94)].iter().map(|&(x, y)| f.p(x, y)).collect(),
                rim,
            ));
            poly(&[(0.62, 0.06), (0.82, 0.26), (0.62, 0.26)], Color32::from_rgb(200, 206, 216));
            for (y, w) in [(0.42, 0.7), (0.54, 0.7), (0.66, 0.7), (0.78, 0.56)] {
                p.line_segment([f.p(0.3, y), f.p(w, y)], Stroke::new((f.size * 0.05).max(1.0), blue));
            }
        }
        Special::Downloads => {
            p.rect_filled(f.rect(0.4, 0.08, 0.6, 0.5), CornerRadius::ZERO, blue);
            poly(&[(0.16, 0.44), (0.84, 0.44), (0.5, 0.8)], blue);
            p.rect_filled(f.rect(0.12, 0.85, 0.88, 0.93), CornerRadius::same(1), deep);
        }
        Special::Music => {
            p.circle_filled(f.p(0.3, 0.76), f.size * 0.14, blue);
            p.circle_filled(f.p(0.7, 0.66), f.size * 0.14, blue);
            p.rect_filled(f.rect(0.37, 0.22, 0.44, 0.76), CornerRadius::ZERO, blue);
            p.rect_filled(f.rect(0.77, 0.12, 0.84, 0.66), CornerRadius::ZERO, blue);
            poly(&[(0.37, 0.22), (0.84, 0.1), (0.84, 0.24), (0.37, 0.36)], deep);
        }
        Special::Pictures => {
            p.rect_filled(f.rect(0.06, 0.14, 0.94, 0.86), CornerRadius::same(2), Color32::from_rgb(140, 190, 246));
            p.circle_filled(f.p(0.72, 0.34), f.size * 0.08, Color32::from_rgb(250, 204, 64));
            poly(&[(0.06, 0.86), (0.38, 0.44), (0.7, 0.86)], Color32::from_rgb(64, 168, 84));
            poly(&[(0.44, 0.86), (0.68, 0.56), (0.94, 0.86)], Color32::from_rgb(38, 128, 62));
            p.rect_stroke(f.rect(0.06, 0.14, 0.94, 0.86), CornerRadius::same(2), rim, StrokeKind::Inside);
        }
        Special::Videos => {
            p.rect_filled(f.rect(0.12, 0.08, 0.88, 0.92), CornerRadius::same(2), Color32::from_rgb(52, 56, 66));
            for k in 0..5 {
                let y = 0.13 + k as f32 * 0.16;
                for x in [0.16, 0.77] {
                    p.rect_filled(f.rect(x, y, x + 0.07, y + 0.08), CornerRadius::same(1), Color32::from_rgb(230, 232, 238));
                }
            }
            p.rect_filled(f.rect(0.29, 0.14, 0.71, 0.47), CornerRadius::same(1), blue);
            p.rect_filled(f.rect(0.29, 0.53, 0.71, 0.86), CornerRadius::same(1), deep);
        }
        Special::Home => {
            p.rect_filled(f.rect(0.2, 0.46, 0.8, 0.9), CornerRadius::ZERO, Color32::from_rgb(240, 242, 246));
            p.rect_stroke(f.rect(0.2, 0.46, 0.8, 0.9), CornerRadius::ZERO, rim, StrokeKind::Inside);
            poly(&[(0.06, 0.52), (0.5, 0.1), (0.94, 0.52)], blue);
            p.rect_filled(f.rect(0.43, 0.64, 0.57, 0.9), CornerRadius::ZERO, Color32::from_rgb(150, 100, 60));
        }
        Special::Objects3D => {
            poly(&[(0.5, 0.1), (0.88, 0.3), (0.5, 0.5), (0.12, 0.3)], Color32::from_rgb(150, 200, 250));
            poly(&[(0.12, 0.3), (0.5, 0.5), (0.5, 0.92), (0.12, 0.72)], blue);
            poly(&[(0.5, 0.5), (0.88, 0.3), (0.88, 0.72), (0.5, 0.92)], deep);
        }
    }
}
