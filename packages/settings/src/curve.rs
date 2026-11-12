//! A fan curve to look at and drag: temperature across, speed up. Shared by Settings > Power > Graphics card (gpufan) and
//! Settings > Power > Cooling (casefan). The lowest speed and the "full speed" range are shaded and named, the part's temperature
//! and the fan's speed now are marked, and a point can be dragged within its neighbours.

use eframe::egui::{self, pos2, vec2, Rect, Sense, Shape, Stroke};
use yggdrasil_ui::theme::Palette;

/// The temperature axis, in degrees C.
pub const T_LO: f32 = 20.0;
pub const T_HI: f32 = 100.0;

/// What the editor draws besides the curve.
pub struct Marks<'a> {
    /// The lowest speed (shaded below) and what to call it.
    pub min: i64,
    pub min_label: &'a str,
    /// From this temperature on the fan runs at full speed whatever the curve says.
    pub hot: i64,
    /// The temperature and the speed now.
    pub now: Option<(f32, f32)>,
}

/// Moves point `i` to (t, p), kept 3 C from its neighbours, no slower than the one before and no faster than the one after,
/// and never below `min`.
pub fn move_point(pts: &mut [[i64; 2]], i: usize, t: i64, p: i64, min: i64) {
    let (t_lo, p_lo) = if i == 0 { (T_LO as i64, min) } else { (pts[i - 1][0] + 3, pts[i - 1][1].max(min)) };
    let (t_hi, p_hi) = if i + 1 == pts.len() { (T_HI as i64, 100) } else { (pts[i + 1][0] - 3, pts[i + 1][1]) };
    pts[i] = [t.clamp(t_lo, t_hi.max(t_lo)), p.clamp(p_lo, p_hi.max(p_lo))];
}

/// Draws the curve and handles dragging; `drag` is the point being dragged (kept by the caller between frames). Returns
/// whether a point moved.
pub fn editor(ui: &mut egui::Ui, pal: &Palette, curve: &mut [[i64; 2]], drag: &mut Option<usize>, m: &Marks) -> bool {
    if curve.len() < 2 {
        return false;
    }
    let mut edited = false;
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 190.0), Sense::click_and_drag());
    let plot = Rect::from_min_max(pos2(rect.left() + 40.0, rect.top() + 8.0), pos2(rect.right() - 10.0, rect.bottom() - 22.0));
    let x = |t: f32| plot.left() + (t - T_LO) / (T_HI - T_LO) * plot.width();
    let y = |p: f32| plot.bottom() - p / 100.0 * plot.height();
    let painter = ui.painter_at(rect);
    let small = egui::FontId::proportional(11.0);
    // the grid: quiet lines every 25 % and every 20 C, labelled outside the plot
    for p in [0.0, 25.0, 50.0, 75.0, 100.0] {
        painter.hline(plot.x_range(), y(p), Stroke::new(1.0, pal.border));
        painter.text(pos2(plot.left() - 6.0, y(p)), egui::Align2::RIGHT_CENTER, format!("{p:.0} %"), small.clone(), pal.dim);
    }
    for t in [20.0, 40.0, 60.0, 80.0, 100.0] {
        painter.vline(x(t), plot.y_range(), Stroke::new(1.0, pal.border));
        // the last label ends at the plot's edge instead of running past it
        let align = if t >= T_HI { egui::Align2::RIGHT_TOP } else { egui::Align2::CENTER_TOP };
        painter.text(pos2(x(t), plot.bottom() + 4.0), align, format!("{t:.0} °C"), small.clone(), pal.dim);
    }
    // below the lowest speed, and from the hot point on, the curve has no say
    let min_rect = Rect::from_min_max(pos2(plot.left(), y(m.min as f32)), plot.right_bottom());
    painter.rect_filled(min_rect, 0.0, pal.hover.gamma_multiply(0.6));
    painter.text(pos2(plot.right() - 4.0, y(m.min as f32) + 2.0), egui::Align2::RIGHT_TOP, m.min_label, small.clone(), pal.dim);
    let hot = Rect::from_min_max(pos2(x(m.hot as f32), plot.top()), plot.right_bottom());
    painter.rect_filled(hot, 0.0, pal.red.gamma_multiply(0.12));
    painter.text(
        pos2(plot.right() - 4.0, plot.top() + 14.0),
        egui::Align2::RIGHT_TOP,
        format!("full speed from {} °C", m.hot),
        small.clone(),
        pal.dim,
    );
    // the curve, carried flat to both edges as the fan services read it
    let pts: Vec<egui::Pos2> = curve.iter().map(|p| pos2(x(p[0] as f32), y(p[1].max(m.min) as f32))).collect();
    let mut line = vec![pos2(plot.left(), pts[0].y)];
    line.extend(pts.iter().copied());
    line.push(pos2(plot.right(), pts[pts.len() - 1].y));
    painter.add(Shape::line(line, Stroke::new(2.0, pal.accent)));
    if let Some((temp, fan)) = m.now {
        let at = pos2(x(temp.clamp(T_LO, T_HI)), y(fan.clamp(0.0, 100.0)));
        painter.vline(at.x, plot.y_range(), Stroke::new(1.0, pal.dim.gamma_multiply(0.6)));
        painter.circle(at, 4.5, pal.text, Stroke::new(2.0, pal.field));
        painter.text(
            pos2(at.x + 6.0, plot.top() + 14.0),
            egui::Align2::LEFT_TOP,
            format!("now {temp:.0} °C, {fan:.0} %"),
            small.clone(),
            pal.text,
        );
    }
    let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
    if resp.drag_started() {
        *drag = pointer.and_then(|p| {
            pts.iter()
                .enumerate()
                .map(|(i, q)| (i, q.distance(p)))
                .filter(|(_, d)| *d < 16.0)
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i)
        });
    }
    if let (Some(i), Some(p), true) = (*drag, pointer, resp.dragged()) {
        let t = (T_LO + (p.x - plot.left()) / plot.width() * (T_HI - T_LO)).round() as i64;
        let pct = ((plot.bottom() - p.y) / plot.height() * 100.0).round() as i64;
        let before = curve[i];
        move_point(curve, i, t, pct, m.min);
        edited = curve[i] != before;
    }
    if resp.drag_stopped() {
        *drag = None;
    }
    let near = pointer.and_then(|p| pts.iter().position(|q| q.distance(p) < 16.0));
    for (i, q) in pts.iter().enumerate() {
        let big = *drag == Some(i) || near == Some(i);
        painter.circle(*q, if big { 7.0 } else { 5.5 }, pal.accent, Stroke::new(2.0, pal.field));
    }
    if let Some(i) = drag.or(near) {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        let c = curve[i];
        painter.text(
            pts[i] + vec2(10.0, -10.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} °C → {} %", c[0], c[1].max(m.min)),
            egui::FontId::proportional(12.0),
            pal.text,
        );
    }
    edited
}
