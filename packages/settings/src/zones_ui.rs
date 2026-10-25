//! Settings > Displays > Window zones: turn the zones sheet on or off, the gap around windows, and which layouts it shows
//! (~/.config/yggdrasil/zones.conf, yggdrasil_conf::zones; the panel reads it each time the sheet opens).

use std::sync::{Mutex, OnceLock};

use eframe::egui;
use yggdrasil_conf::zones::{builtin, ZonesConf};
use yggdrasil_ui::theme::Palette;

use crate::ui;

fn conf() -> &'static Mutex<ZonesConf> {
    static C: OnceLock<Mutex<ZonesConf>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(ZonesConf::load()))
}

pub fn group(ui: &mut egui::Ui, pal: &Palette) {
    let Ok(mut c) = conf().lock() else { return };
    let before = c.clone();
    ui.add_space(8.0);
    ui::group(ui, pal, Some("Window zones"), |ui| {
        ui::row(ui, pal, true, "Window zones", Some("Hold Super and drag a window to the notch at the top of the screen, or press Super+Shift+Z, and pick where it goes"), |ui| {
            ui::switch(ui, pal, &mut c.enabled);
        });
        if c.enabled {
            let mut gap = c.gap as f32;
            ui::row(ui, pal, false, "Space around windows", None, |ui| {
                if ui::slider_range(ui, pal, &mut gap, 0.0..=32.0, " px").changed() {
                    c.gap = gap.round() as u32;
                }
            });
            for l in builtin() {
                let mut on = c.layouts.iter().any(|id| id == l.id);
                let detail = format!("{} zone{}", l.zones.len(), if l.zones.len() == 1 { "" } else { "s" });
                ui::row(ui, pal, false, l.name, Some(&detail), |ui| {
                    if ui::switch(ui, pal, &mut on).changed() {
                        if on {
                            // keep the built-in order
                            let order: Vec<&str> = builtin().iter().map(|b| b.id).collect();
                            c.layouts.push(l.id.to_string());
                            c.layouts.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
                        } else {
                            c.layouts.retain(|id| id != l.id);
                        }
                    }
                });
            }
        }
    });
    if *c != before {
        let _ = c.save();
    }
}
