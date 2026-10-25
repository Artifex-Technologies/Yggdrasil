//! A bar add-on's window as a dropdown under its pill, like the panel's own cards: the panel starts the app with
//! YGGDRASIL_DROPDOWN=1, the app leaves its class in the runtime folder, and the desktop session opens that window under the pointer,
//! just below the bar (Hyprland: the session's window.open hook). The window is drawn as a card and closes when it loses focus.

use eframe::egui::{self, Color32, CornerRadius, Stroke};
use std::path::PathBuf;

fn marker(app_id: &str) -> Option<PathBuf> {
    let dir = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("yggdrasil-dropdown");
    Some(dir.join(app_id))
}

/// Call before the window opens. True when it was opened from the bar: then it is a dropdown (place asked for, close on blur).
pub fn request(app_id: &str) -> bool {
    if std::env::var_os("YGGDRASIL_DROPDOWN").is_none() || !app_id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
        return false;
    }
    let Some(m) = marker(app_id) else { return false };
    let _ = std::fs::create_dir_all(m.parent().unwrap());
    std::fs::write(&m, b"").is_ok()
}

/// The card's look, matching the panel's add-on cards (rounded, near-black or near-white, a faint edge).
pub fn frame(dark: bool) -> egui::Frame {
    let (fill, edge) = if dark { (Color32::from_rgba_unmultiplied(24, 24, 30, 245), Color32::from_white_alpha(28)) } else { (Color32::from_rgba_unmultiplied(250, 250, 252, 245), Color32::from_black_alpha(26)) };
    egui::Frame::new().fill(fill).stroke(Stroke::new(1.0, edge)).corner_radius(CornerRadius::same(14)).inner_margin(16)
}

/// Closes the window once it has had focus and lost it (a click elsewhere), as the panel's cards close.
#[derive(Default)]
pub struct Blur {
    had_focus: bool,
}

impl Blur {
    pub fn check(&mut self, ctx: &egui::Context) {
        let focused = ctx.input(|i| i.viewport().focused).unwrap_or(true);
        if focused {
            self.had_focus = true;
        } else if self.had_focus {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
