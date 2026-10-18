//! GNOME-style building blocks: rounded groups of rows ("boxed lists"), a row with a title,
//! an optional subtitle and a control on the right, a toggle switch, and a segmented choice.

use eframe::egui::{self, vec2, Color32, CornerRadius, Margin, RichText, Sense, Stroke};
use labyrinth_ui::theme::Palette;

/// A titled group of rows on a raised, rounded card.
pub fn group(ui: &mut egui::Ui, pal: &Palette, title: Option<&str>, add: impl FnOnce(&mut egui::Ui)) {
    if let Some(t) = title {
        ui.add_space(14.0);
        ui.label(RichText::new(t).strong().color(pal.dim));
        ui.add_space(4.0);
    } else {
        ui.add_space(10.0);
    }
    egui::Frame::new()
        .fill(pal.field)
        .stroke(Stroke::new(1.0, pal.border))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::symmetric(14, 6))
        .show(ui, |ui| {
            // Fill the column but never more: the frame's own margin and stroke come on top.
            ui.set_width((ui.available_width() - 2.0).max(0.0));
            add(ui);
        });
}

/// One row: title (and subtitle) on the left, `control` on the right. The control is laid out
/// first and the text wraps in the space left, so narrow windows never overlap the two. Rows
/// after the first in a group get a hairline above them.
pub fn row(ui: &mut egui::Ui, pal: &Palette, first: bool, title: &str, subtitle: Option<&str>, control: impl FnOnce(&mut egui::Ui)) {
    if !first {
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.left()..=r.right(), r.top(), Stroke::new(1.0, pal.border));
    }
    let min_h = if subtitle.is_some() { 50.0 } else { 40.0 };
    ui.allocate_ui_with_layout(vec2(ui.available_width(), min_h), egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.set_min_height(min_h);
        control(ui);
        ui.add_space(12.0);
        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.add_space(if subtitle.is_some() { 6.0 } else { 10.0 });
            ui.add(egui::Label::new(RichText::new(title).size(14.0)).wrap());
            if let Some(s) = subtitle {
                ui.add(egui::Label::new(RichText::new(s).size(12.0).color(pal.dim)).wrap());
            }
            ui.add_space(6.0);
        });
    });
}

/// An on/off switch, drawn like GNOME's (accent when on).
pub fn switch(ui: &mut egui::Ui, pal: &Palette, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(44.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let track = if *on { pal.accent } else { pal.hover };
    ui.painter().rect_filled(rect, CornerRadius::same(12), track);
    let x = egui::lerp(rect.left() + 12.0..=rect.right() - 12.0, t);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 9.0, if *on { pal.on_accent() } else { Color32::WHITE });
    resp
}

/// A row of mutually exclusive buttons; returns the index clicked.
pub fn segmented(ui: &mut egui::Ui, pal: &Palette, options: &[&str], selected: usize) -> Option<usize> {
    let mut clicked = None;
    let rtl = ui.layout().prefer_right_to_left();
    egui::Frame::new().fill(pal.hover).corner_radius(CornerRadius::same(9)).inner_margin(Margin::same(3)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            // Rows are laid out right to left, which shows the options mirrored; drawing them last to first reads in the
            // order given. Anywhere else (left to right) they are drawn in order.
            let order: Vec<usize> = if rtl { (0..options.len()).rev().collect() } else { (0..options.len()).collect() };
            for i in order {
                let o = &options[i];
                let sel = i == selected;
                let b = egui::Button::new(RichText::new(*o).color(if sel { pal.on_accent() } else { pal.text }))
                    .fill(if sel { pal.accent } else { Color32::TRANSPARENT })
                    .corner_radius(CornerRadius::same(7))
                    .min_size(vec2(70.0, 26.0));
                if ui.add(b).clicked() && !sel {
                    clicked = Some(i);
                }
            }
        });
    });
    clicked
}

/// A flat button in the accent colour, for the main action of a row.
pub fn action(ui: &mut egui::Ui, pal: &Palette, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label).color(pal.on_accent())).fill(pal.accent).corner_radius(CornerRadius::same(8)).min_size(vec2(80.0, 28.0)))
}

/// A quiet button for secondary actions.
pub fn quiet(ui: &mut egui::Ui, pal: &Palette, label: &str) -> egui::Response {
    ui.add(egui::Button::new(RichText::new(label)).fill(pal.hover).corner_radius(CornerRadius::same(8)).min_size(vec2(80.0, 28.0)))
}

/// A 0–100 % slider with a visible track and an accent fill up to the handle.
pub fn slider(ui: &mut egui::Ui, pal: &Palette, value: &mut f32) -> egui::Response {
    slider_range(ui, pal, value, 0.0..=100.0, " %")
}

/// The same for any range and unit.
pub fn slider_range(ui: &mut egui::Ui, pal: &Palette, value: &mut f32, range: std::ops::RangeInclusive<f32>, suffix: &str) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().slider_width = (ui.available_width() * 0.55).clamp(120.0, 260.0);
        let v = &mut ui.style_mut().visuals;
        v.widgets.inactive.bg_fill = pal.hover;
        v.widgets.inactive.fg_stroke = Stroke::new(1.0, pal.text);
        v.widgets.hovered.bg_fill = pal.hover;
        v.selection.bg_fill = pal.accent;
        ui.add(egui::Slider::new(value, range).trailing_fill(true).show_value(true).suffix(suffix.to_string()).fixed_decimals(0))
    })
    .inner
}

pub fn gib(bytes: u64) -> String {
    let g = bytes as f64 / 1e9;
    if g >= 1000.0 {
        format!("{:.1} TB", g / 1000.0)
    } else {
        format!("{g:.0} GB")
    }
}

/// A row that opens a page: the whole row is the button, with a `>` at the end. `extra` draws anything between the text and the `>`.
/// Returns true when it was clicked.
pub fn nav_row(ui: &mut egui::Ui, pal: &Palette, first: bool, title: &str, subtitle: Option<&str>, extra: impl FnOnce(&mut egui::Ui)) -> bool {
    let bg = ui.painter().add(egui::Shape::Noop);
    let id = ui.next_auto_id();
    let r = ui.scope(|ui| {
        row(ui, pal, first, title, subtitle, |ui| {
            ui.label(RichText::new("\u{203a}").size(20.0).color(pal.dim));
            extra(ui);
        });
    });
    let resp = ui.interact(r.response.rect, id.with("nav"), Sense::click());
    if resp.hovered() {
        // Inside the card with an even gap, clear of the hairline above and the one below.
        let mut hl = r.response.rect.expand2(vec2(6.0, 0.0));
        hl.min.y += if first { 3.0 } else { 4.0 };
        hl.max.y -= 3.0;
        ui.painter().set(bg, egui::Shape::rect_filled(hl, CornerRadius::same(8), pal.hover));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.clicked()
}
