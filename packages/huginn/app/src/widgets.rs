//! Drawing: video cards (a 16:9 picture, the length on it, the title in two lines, the channel and the views), the big card at the
//! top of Home (Agora's: the picture with a dark band from the left where the text is), and the small helpers they share.

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke};
use huginn_core::model::{count_text, duration_text, Video};
use yggdrasil_ui::theme::Palette;

/// A picture that is ready to draw (from the thumbnail cache), or None while it loads.
pub type Tex = egui::load::SizedTexture;

pub fn texture(ui: &egui::Ui, path: Option<&std::path::Path>) -> Option<Tex> {
    let path = path?;
    match ui.ctx().try_load_texture(&format!("file://{}", path.display()), egui::TextureOptions::LINEAR, egui::SizeHint::default()) {
        Ok(egui::load::TexturePoll::Ready { texture }) => Some(texture),
        _ => None,
    }
}

/// The part of a picture that fills `rect` without stretching it (cut at the sides or at the top and bottom).
pub fn cover_uv(tex_size: egui::Vec2, rect: Rect) -> Rect {
    let (ta, ca) = (tex_size.x / tex_size.y.max(1.0), rect.width() / rect.height().max(1.0));
    if ta > ca {
        let fw = ca / ta;
        Rect::from_min_max(pos2((1.0 - fw) / 2.0, 0.0), pos2((1.0 + fw) / 2.0, 1.0))
    } else {
        let fh = ta / ca;
        Rect::from_min_max(pos2(0.0, (1.0 - fh) / 2.0), pos2(1.0, (1.0 + fh) / 2.0))
    }
}

/// A picture in a rounded rectangle, or a soft placeholder until it arrives.
pub fn picture(ui: &egui::Ui, rect: Rect, radius: u8, tex: Option<&Tex>, pal: &Palette) {
    match tex {
        Some(t) => {
            let uv = cover_uv(t.size, rect);
            ui.painter().add(egui::epaint::RectShape::filled(rect, CornerRadius::same(radius), Color32::WHITE).with_texture(t.id, uv));
        }
        None => {
            ui.painter().rect_filled(rect, CornerRadius::same(radius), pal.field);
        }
    }
}

/// A gradient over a rounded rectangle (`color_at` says the colour at each point): Agora's dark band behind text.
pub fn gradient_rounded(painter: &egui::Painter, rect: Rect, radius: f32, color_at: impl Fn(egui::Pos2) -> Color32) {
    let mut pts: Vec<egui::Pos2> = vec![];
    let r = radius.min(rect.width() / 2.0).min(rect.height() / 2.0);
    let corners = [(rect.right() - r, rect.top() + r, -90.0f32), (rect.right() - r, rect.bottom() - r, 0.0), (rect.left() + r, rect.bottom() - r, 90.0), (rect.left() + r, rect.top() + r, 180.0)];
    for (cx, cy, start) in corners {
        for k in 0..=8 {
            let a = (start + 90.0 * k as f32 / 8.0).to_radians();
            pts.push(pos2(cx + r * a.cos(), cy + r * a.sin()));
        }
    }
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(rect.center(), color_at(rect.center()));
    for p in &pts {
        mesh.colored_vertex(*p, color_at(*p));
    }
    for i in 0..pts.len() {
        mesh.add_triangle(0, 1 + i as u32, 1 + ((i + 1) % pts.len()) as u32);
    }
    painter.add(egui::Shape::mesh(mesh));
}

/// Text in at most `rows` lines of `width`, cut with an ellipsis.
pub fn wrapped(ui: &egui::Ui, text: &str, size: f32, width: f32, rows: usize, color: Color32, strong: bool) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_string(), egui::TextFormat {
        font_id: if strong { FontId::new(size, egui::FontFamily::Proportional) } else { FontId::proportional(size) },
        color,
        ..Default::default()
    });
    job.wrap = egui::text::TextWrapping { max_width: width, max_rows: rows, break_anywhere: false, overflow_character: Some('…') };
    ui.fonts_mut(|f| f.layout_job(job))
}

/// "May 14, 2026" from YYYYMMDD.
pub fn date_text(ymd: &str) -> String {
    const M: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    if ymd.len() != 8 || !ymd.chars().all(|c| c.is_ascii_digit()) {
        return String::new();
    }
    let m: usize = ymd[4..6].parse().unwrap_or(0);
    let d: u32 = ymd[6..8].parse().unwrap_or(0);
    match M.get(m.wrapping_sub(1)) {
        Some(name) => format!("{name} {d}, {}", &ymd[..4]),
        None => String::new(),
    }
}

/// The small line under a video's title: "Techquickie · 756K views · May 14, 2026".
pub fn byline(v: &Video, with_channel: bool) -> String {
    let mut parts = vec![];
    if with_channel && !v.channel.is_empty() {
        parts.push(v.channel.clone());
    }
    if let Some(n) = v.views {
        parts.push(format!("{} views", count_text(n)));
    }
    if let Some(d) = v.published.as_deref().map(date_text).filter(|d| !d.is_empty()) {
        parts.push(d);
    }
    parts.join(" · ")
}

/// A video card `width` wide: the picture (16:9), the length on it, two lines of title, the byline. Returns the response (click =
/// open the video).
pub fn video_card(ui: &mut egui::Ui, pal: &Palette, v: &Video, tex: Option<&Tex>, width: f32, with_channel: bool) -> egui::Response {
    let pic_h = (width * 9.0 / 16.0).round();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, pic_h + 66.0), Sense::click());
    let pic = Rect::from_min_size(rect.min, vec2(width, pic_h));
    picture(ui, pic, 12, tex, pal);
    if resp.hovered() {
        ui.painter().rect_stroke(pic, CornerRadius::same(12), Stroke::new(2.0, pal.accent), egui::StrokeKind::Inside);
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let d = duration_text(v);
    if !d.is_empty() {
        let font = FontId::proportional(12.0);
        let w = ui.fonts_mut(|f| f.layout_no_wrap(d.clone(), font.clone(), Color32::WHITE).size().x) + 12.0;
        let badge = Rect::from_min_size(pic.right_bottom() - vec2(w + 8.0, 26.0), vec2(w, 20.0));
        ui.painter().rect_filled(badge, CornerRadius::same(5), if v.live { Color32::from_rgb(200, 30, 40) } else { Color32::from_black_alpha(190) });
        ui.painter().text(badge.center(), Align2::CENTER_CENTER, d, font, Color32::WHITE);
    }
    let title = wrapped(ui, &v.title, 14.5, width - 4.0, 2, pal.text, true);
    ui.painter().galley(pic.left_bottom() + vec2(2.0, 8.0), title.clone(), pal.text);
    let by = wrapped(ui, &byline(v, with_channel), 12.0, width - 4.0, 1, pal.dim, false);
    ui.painter().galley(pic.left_bottom() + vec2(2.0, 12.0 + title.size().y), by, pal.dim);
    resp
}

/// A rounded pill button: filled with the accent colour for the main action, quiet for the others.
pub fn pill(ui: &mut egui::Ui, pal: &Palette, label: &str, primary: bool) -> egui::Response {
    let font = FontId::proportional(14.0);
    let w = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), Color32::WHITE).size().x) + 32.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
    let (fill, text) = match (primary, resp.hovered()) {
        (true, false) => (pal.accent, pal.on_accent()),
        (true, true) => (pal.accent.gamma_multiply(1.15), pal.on_accent()),
        (false, false) => (pal.field, pal.text),
        (false, true) => (pal.hover, pal.text),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(18), fill);
    if !primary {
        ui.painter().rect_stroke(rect, CornerRadius::same(18), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
    }
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, label, font, text);
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp
}

/// A grid of cards that fills the width: as many columns as fit (cards at least `min_w` wide, at most five).
pub fn columns(width: f32, min_w: f32) -> (usize, f32) {
    let gap = 18.0;
    let n = (((width + gap) / (min_w + gap)).floor() as usize).clamp(1, 5);
    (n, ((width - gap * (n as f32 - 1.0)) / n as f32).floor())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_columns() {
        assert_eq!(date_text("20260514"), "May 14, 2026");
        assert_eq!(date_text("2026051"), "");
        assert_eq!(date_text("20261314"), "");
        assert_eq!(columns(1100.0, 250.0).0, 4);
        assert_eq!(columns(300.0, 250.0).0, 1);
        let (n, w) = columns(1000.0, 250.0);
        assert!(w * n as f32 + 18.0 * (n as f32 - 1.0) <= 1000.0);
    }

    #[test]
    fn the_byline() {
        let v = Video { channel: "Techquickie".into(), views: Some(756_582), published: Some("20260514".into()), ..Default::default() };
        assert_eq!(byline(&v, true), "Techquickie · 756K views · May 14, 2026");
        assert_eq!(byline(&v, false), "756K views · May 14, 2026");
    }

    #[test]
    fn cover_cuts_the_long_side() {
        let uv = cover_uv(vec2(1600.0, 900.0), Rect::from_min_size(pos2(0.0, 0.0), vec2(900.0, 900.0)));
        assert!((uv.width() - 0.5625).abs() < 1e-3 && (uv.height() - 1.0).abs() < 1e-6);
    }
}
