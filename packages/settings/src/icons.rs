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
    User,
    Clock,
    Apps,
    Keyboard,
    Info,
    Restart,
    Close,
    Trash,
    Back,
    Printer,
    /// Open in a window of its own: a square open at its top right corner, an arrow leaving through it.
    PopOut,
    /// Games: a gamepad (its body, a cross on the left, two buttons on the right).
    Gamepad,
    /// Sarina: the constellation S of her app icon (four stars joined by three lines), without its hexagon.
    Constellation,
    /// A padlock: a secured Wi-Fi network.
    Lock,
    /// A check mark: the network this computer is on.
    Check,
}

fn arc(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    (0..=16)
        .map(|i| {
            let a = (from + (to - from) * i as f32 / 16.0).to_radians();
            pos2(c.x + r * a.cos(), c.y - r * a.sin())
        })
        .collect()
}

pub fn paint(p: &Painter, rect: Rect, icon: Icon, color: Color32) {
    let s = Stroke::new(1.6, color);
    let c = rect.center();
    let w = rect.width();
    let at = |x: f32, y: f32| pos2(rect.left() + x * w, rect.top() + y * w);
    match icon {
        Icon::Printer => {
            // paper sticking out of the top, the body, and the tray the page comes out on
            p.rect_stroke(Rect::from_min_max(at(0.3, 0.12), at(0.7, 0.36)), 1.0, s, eframe::egui::StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_max(at(0.12, 0.36), at(0.88, 0.7)), 2.0, s, eframe::egui::StrokeKind::Inside);
            p.rect_stroke(Rect::from_min_max(at(0.28, 0.58), at(0.72, 0.88)), 1.0, s, eframe::egui::StrokeKind::Inside);
        }
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
            p.add(Shape::convex_polygon(
                vec![at(0.14, 0.4), at(0.3, 0.4), at(0.5, 0.22), at(0.5, 0.78), at(0.3, 0.6), at(0.14, 0.6)],
                Color32::TRANSPARENT,
                s,
            ));
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
        Icon::Constellation => {
            // the stars of the S, top right to bottom left, as they sit in her icon (brand-icons/native/sarina.svg)
            let stars = [at(0.75, 0.22), at(0.34, 0.33), at(0.66, 0.52), at(0.25, 0.78)];
            let line = Stroke::new(2.0, color);
            for pair in stars.windows(2) {
                p.line_segment([pair[0], pair[1]], line);
            }
            for (i, star) in stars.into_iter().enumerate() {
                // the outer two a little bigger, as in the icon
                p.circle_filled(star, if i == 0 || i == 3 { 0.115 * w } else { 0.105 * w }, color);
            }
        }
        Icon::Shield => {
            p.add(Shape::closed_line(vec![at(0.5, 0.1), at(0.84, 0.24), at(0.8, 0.56), at(0.5, 0.9), at(0.2, 0.56), at(0.16, 0.24)], s));
        }
        Icon::User => {
            p.circle_stroke(at(0.5, 0.34), 0.16 * w, s);
            p.add(Shape::line(arc(at(0.5, 0.92), 0.34 * w, 20.0, 160.0), s));
        }
        Icon::Clock => {
            p.circle_stroke(c, 0.38 * w, s);
            p.line_segment([c, at(0.5, 0.26)], s);
            p.line_segment([c, at(0.68, 0.6)], s);
        }
        Icon::Apps => {
            for (x, y) in [(0.3, 0.3), (0.7, 0.3), (0.3, 0.7), (0.7, 0.7)] {
                p.rect_stroke(
                    Rect::from_center_size(at(x, y), eframe::egui::vec2(0.26 * w, 0.26 * w)),
                    2.0,
                    s,
                    eframe::egui::StrokeKind::Inside,
                );
            }
        }
        Icon::Keyboard => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.26), at(0.92, 0.74)), 2.0, s, eframe::egui::StrokeKind::Inside);
            for x in [0.26, 0.42, 0.58, 0.74] {
                p.circle_filled(at(x, 0.42), 0.035 * w, color);
            }
            p.line_segment([at(0.3, 0.6), at(0.7, 0.6)], s);
        }
        Icon::Gamepad => {
            p.rect_stroke(Rect::from_min_max(at(0.08, 0.3), at(0.92, 0.72)), 0.2 * w, s, eframe::egui::StrokeKind::Inside);
            p.line_segment([at(0.22, 0.51), at(0.4, 0.51)], s);
            p.line_segment([at(0.31, 0.42), at(0.31, 0.6)], s);
            p.circle_filled(at(0.64, 0.46), 0.045 * w, color);
            p.circle_filled(at(0.76, 0.56), 0.045 * w, color);
        }
        Icon::Restart => {
            p.add(Shape::line(arc(c, 0.3 * w, 60.0, 340.0), s));
            p.add(Shape::line(vec![at(0.62, 0.12), at(0.8, 0.28), at(0.58, 0.34)], s));
        }
        Icon::Close => {
            p.line_segment([at(0.26, 0.26), at(0.74, 0.74)], s);
            p.line_segment([at(0.74, 0.26), at(0.26, 0.74)], s);
        }
        Icon::Trash => {
            p.add(Shape::closed_line(vec![at(0.26, 0.3), at(0.74, 0.3), at(0.68, 0.86), at(0.32, 0.86)], s));
            p.line_segment([at(0.18, 0.3), at(0.82, 0.3)], s);
            p.add(Shape::line(vec![at(0.4, 0.3), at(0.4, 0.18), at(0.6, 0.18), at(0.6, 0.3)], s));
        }
        Icon::PopOut => {
            p.add(Shape::line(vec![at(0.5, 0.2), at(0.2, 0.2), at(0.2, 0.8), at(0.8, 0.8), at(0.8, 0.5)], s));
            p.line_segment([at(0.44, 0.56), at(0.82, 0.18)], s);
            p.add(Shape::line(vec![at(0.58, 0.18), at(0.82, 0.18), at(0.82, 0.42)], s));
        }
        Icon::Back => {
            p.add(Shape::line(vec![at(0.6, 0.22), at(0.34, 0.5), at(0.6, 0.78)], s));
        }
        Icon::Lock => {
            p.add(Shape::line(arc(at(0.5, 0.46), 0.18 * w, 0.0, 180.0), s));
            p.line_segment([at(0.32, 0.46), at(0.32, 0.5)], s);
            p.line_segment([at(0.68, 0.46), at(0.68, 0.5)], s);
            p.rect_filled(Rect::from_min_max(at(0.22, 0.48), at(0.78, 0.88)), 0.06 * w, color);
        }
        Icon::Check => {
            p.add(Shape::line(vec![at(0.16, 0.54), at(0.4, 0.78), at(0.86, 0.24)], Stroke::new(2.2, color)));
        }
        Icon::Info => {
            p.circle_stroke(c, 0.38 * w, s);
            p.circle_filled(at(0.5, 0.32), 0.05 * w, color);
            p.line_segment([at(0.5, 0.44), at(0.5, 0.72)], s);
        }
    }
}
