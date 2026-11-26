//! Small controls more than one app draws the same way.

use eframe::egui::{self, vec2, Color32, CornerRadius, Sense};

use crate::theme::Palette;

/// An on/off switch (the track in the accent colour when on); its response is marked changed when it was flipped.
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
