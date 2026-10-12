//! Sidebar icons, drawn as simple line shapes (no icon font or image files to ship), in the
//! spirit of GNOME Settings' symbolic icons.

use eframe::egui::{pos2, Color32, Painter, Pos2, Rect, Shape, Stroke};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Icon {
    Wifi,
    Bluetooth,
    Display,
    Sound,
    Appearance,
    Power,
    Shield,
    Clock,
    Apps,
    Keyboard,
    Info,
}

fn arc(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    (0..=16).map(|i| {
        let a = (from + (to - from) * i as f32 / 16.0).to_radians();
        pos2(c.x + r * a.cos(), c.y - r * a.sin())
    }).collect()
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.6, color);
    let c = rect.center();
    let w = rect.width();
    let at = |x: f32, y: f32| pos2(rect.left() + x * w, rect.top() + y * w);
    match icon {
        Icon::Wifi => {
            for (i, r) in [0.42, 0.29, 0.16].iter().enumerate() {
                let _ = i;
                p.add(Shape::line(arc(at(0.5, 0.8), r * w, 45.0, 135.0), s));
            }
            p.circle_filled(at(0.5, 0.78), 0.06 * w, color);
        }
        Icon::Bluetooth => {
            p.add(Shape::line(vec![at(0.3, 0.3), at(0.7, 0.68), at(0.5, 0.86), at(0.5, 0.14), at(0.7, 0.32), at(0.3, 0.7)], s));
        }
        Icon::Display => {
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.2), at(0.88, 0.7)), 2.0, s, eframe::egui::StrokeKind::Inside);
            p.line_segment([at(0.5, 0.7), at(0.5, 0.84)], s);
            p.line_segment([at(0.32, 0.86), at(0.68, 0.86)], s);
        }
        Icon::Sound => {
            p.add(Shape::convex_polygon(vec![at(0.14, 0.4), at(0.3, 0.4), at(0.5, 0.22), at(0.5, 0.78), at(0.3, 0.6), at(0.14, 0.6)], Color32::TRANSPARENT, s));
            p.add(Shape::line(arc(at(0.52, 0.5), 0.18 * w, -50.0, 50.0), s));
            p.add(Shape::line(arc(at(0.52, 0.5), 0.32 * w, -50.0, 50.0), s));
        }
        Icon::Appearance => {
            p.circle_stroke(c, 0.36 * w, s);
            let half = arc(c, 0.36 * w, 90.0, 270.0);
            p.add(Shape::convex_polygon(half, color, Stroke::NONE));
        }
        Icon::Power => {
            p.add(Shape::line(arc(c, 0.34 * w, 120.0, 420.0), s));
            p.line_segment([at(0.5, 0.1), at(0.5, 0.48)], s);
        }
        Icon::Shield => {
            p.add(Shape::closed_line(vec![at(0.5, 0.1), at(0.84, 0.24), at(0.8, 0.56), at(0.5, 0.9), at(0.2, 0.56), at(0.16, 0.24)], s));
        }
        Icon::Clock => {
            p.circle_stroke(c, 0.38 * w, s);
            p.line_segment([c, at(0.5, 0.26)], s);
            p.line_segment([c, at(0.68, 0.6)], s);
        }
        Icon::Apps => {
            for (x, y) in [(0.3, 0.3), (0.7, 0.3), (0.3, 0.7), (0.7, 0.7)] {
                p.rect_stroke(Rect::from_center_size(at(x, y), eframe::egui::vec2(0.26 * w, 0.26 * w)), 2.0, s, eframe::egui::StrokeKind::Inside);
            }
        }
        Icon::Keyboard => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.26), at(0.92, 0.74)), 2.0, s, eframe::egui::StrokeKind::Inside);
            for x in [0.26, 0.42, 0.58, 0.74] {
                p.circle_filled(at(x, 0.42), 0.035 * w, color);
            }
            p.line_segment([at(0.3, 0.6), at(0.7, 0.6)], s);
        }
        Icon::Info => {
            p.circle_stroke(c, 0.38 * w, s);
            p.circle_filled(at(0.5, 0.32), 0.05 * w, color);
            p.line_segment([at(0.5, 0.44), at(0.5, 0.72)], s);
        }
    }
}
