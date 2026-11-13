//! Appearance > Wallpaper's preview, as on a tablet: the lock screen and the desktop side by side, in the current wallpaper,
//! style (light or dark), accent colour, icon pack and lock screen design.
//!
//! The lock screen is the lock's own drawing (`yggdrasil-lock --preview`, the design in use, nothing locked); until it is
//! ready a likeness drawn here stands in. The desktop is drawn here: the wallpaper, the top bar, a window with the accent's
//! border, and the dock with the first pinned apps in the icon pack. Both are made again only when one of those changes.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke};
use yggdrasil_ui::theme::Palette;

use crate::{appmgr, sys, ui, App};

/// What the preview is drawn from: when any of it changes, it is made again.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    pub wallpaper: Option<String>,
    pub dark: bool,
    pub accent: Option<[u8; 3]>,
    pub icon_theme: String,
    pub design: String,
}

/// The lock screen design in use (lock.conf, Split when unset).
pub fn lock_design() -> String {
    design_in(&std::fs::read_to_string(yggdrasil_conf::dirs::config().join("lock.conf")).unwrap_or_default())
}

/// lock.conf's `design = NAME`.
pub fn design_in(text: &str) -> String {
    text.lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "design")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "split".into())
}

/// The dock's apps (desktop id, icon file), found in the background.
type DockIcons = Arc<Mutex<Vec<(String, Option<PathBuf>)>>>;

/// The preview's state: the key it was made for, the lock's picture once drawn, the dock's icons.
pub struct Preview {
    pub key: Key,
    lock_png: PathBuf,
    lock_ready: Arc<AtomicBool>,
    icons: DockIcons,
    ctx: egui::Context,
}

impl Drop for Preview {
    /// The lock's picture leaves memory with the page.
    fn drop(&mut self) {
        self.ctx.forget_image(&format!("file://{}", self.lock_png.display()));
    }
}

impl Preview {
    /// Starts making the pictures for `key` in the background.
    pub fn start(key: Key, ctx: &egui::Context) -> Preview {
        let dir = yggdrasil_conf::dirs::cache().join("wallpaper-preview");
        let lock_png = dir.join("lock.png");
        let lock_ready = Arc::new(AtomicBool::new(false));
        let icons: DockIcons = Default::default();
        let (ready, ic, out, c, k) = (lock_ready.clone(), icons.clone(), lock_png.clone(), ctx.clone(), key.clone());
        std::thread::spawn(move || {
            // the dock: the first pinned apps, each in the icon pack (an app's id is its icon's name for most)
            let found: Vec<(String, Option<PathBuf>)> =
                yggdrasil_conf::dock::pinned().into_iter().take(6).map(|id| (id.clone(), appmgr::icon_path(&id, &k.icon_theme))).collect();
            *ic.lock().unwrap_or_else(|e| e.into_inner()) = found;
            c.request_repaint();
            let _ = std::fs::create_dir_all(&dir);
            let ok = std::process::Command::new("yggdrasil-lock")
                .args(["--preview", &k.design, "--size", "640x360", "--out"])
                .arg(&out)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|s| s.success());
            c.forget_image(&format!("file://{}", out.display()));
            ready.store(ok, Ordering::Release);
            c.request_repaint();
        });
        Preview { key, lock_png, lock_ready, icons, ctx: ctx.clone() }
    }
}

/// The wallpaper filling `r` (cropped to cover it), or the style's background without one.
fn wallpaper(ui: &egui::Ui, r: Rect, wall: Option<&str>, pal: &Palette) {
    let p = ui.painter_at(r);
    p.rect_filled(r, CornerRadius::same(10), pal.strip);
    if let Some(w) = wall.filter(|w| std::path::Path::new(w).is_file()) {
        egui::Image::new(format!("file://{w}")).maintain_aspect_ratio(false).fit_to_exact_size(r.size()).corner_radius(10).paint_at(ui, r);
    }
}

fn clock() -> (String, String) {
    let tz = jiff::tz::TimeZone::system();
    let now = jiff::Timestamp::now().to_zoned(tz);
    (now.strftime("%H:%M").to_string(), now.strftime("%A %-d %B").to_string())
}

impl App {
    /// The preview card at the top of Appearance > Wallpaper.
    pub(crate) fn wallpaper_preview(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, a: &sys::Appearance) {
        let key = Key {
            wallpaper: yggdrasil_conf::wallpaper(),
            dark: a.dark,
            accent: a.accent,
            icon_theme: a.icon_theme.clone(),
            design: lock_design(),
        };
        if self.wall_preview.as_ref().is_none_or(|p| p.key != key) {
            self.wall_preview = Some(Preview::start(key.clone(), ctx));
        }
        let Some(pv) = &self.wall_preview else { return };
        let accent = pal.accent;
        let (time, date) = clock();
        ui::group(ui, pal, None, |ui| {
            ui.add_space(10.0);
            let gap = 16.0;
            let w = ((ui.available_width() - gap) / 2.0).min(330.0);
            let size = vec2(w, w * 9.0 / 16.0);
            ui.horizontal(|ui| {
                let total = 2.0 * w + gap;
                ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
                // the lock screen: the lock's own drawing once made, a likeness until then
                ui.vertical(|ui| {
                    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                    if pv.lock_ready.load(Ordering::Acquire) && pv.lock_png.is_file() {
                        egui::Image::new(format!("file://{}", pv.lock_png.display()))
                            .fit_to_exact_size(size)
                            .corner_radius(10)
                            .paint_at(ui, r);
                    } else {
                        wallpaper(ui, r, key.wallpaper.as_deref(), pal);
                        let p = ui.painter_at(r);
                        p.rect_filled(r, CornerRadius::same(10), Color32::from_black_alpha(90));
                        p.text(
                            r.center_top() + vec2(0.0, r.height() * 0.32),
                            Align2::CENTER_CENTER,
                            &time,
                            FontId::proportional(w * 0.14),
                            Color32::WHITE,
                        );
                        p.text(
                            r.center_top() + vec2(0.0, r.height() * 0.46),
                            Align2::CENTER_CENTER,
                            &date,
                            FontId::proportional(w * 0.04),
                            Color32::from_white_alpha(220),
                        );
                        let field = Rect::from_center_size(r.center_top() + vec2(0.0, r.height() * 0.68), vec2(w * 0.42, w * 0.06));
                        p.rect_filled(field, CornerRadius::same(20), Color32::from_black_alpha(110));
                        p.rect_stroke(field, CornerRadius::same(20), Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                    }
                    ui.add_space(4.0);
                    ui.allocate_ui(vec2(w, 18.0), |ui| {
                        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("Lock Screen").size(12.5).color(pal.dim)));
                    });
                });
                ui.add_space(gap);
                // the desktop: wallpaper, top bar, a window with the accent's border, the dock
                ui.vertical(|ui| {
                    let (r, _) = ui.allocate_exact_size(size, Sense::hover());
                    wallpaper(ui, r, key.wallpaper.as_deref(), pal);
                    let p = ui.painter_at(r);
                    let bar = Rect::from_min_size(r.min, vec2(r.width(), r.height() * 0.06));
                    p.rect_filled(bar, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, pal.bg.gamma_multiply(0.85));
                    p.circle_filled(pos2(bar.left() + bar.height() * 0.9, bar.center().y), bar.height() * 0.28, accent);
                    p.text(
                        pos2(bar.right() - 8.0, bar.center().y),
                        Align2::RIGHT_CENTER,
                        &time,
                        FontId::proportional(bar.height() * 0.6),
                        pal.text,
                    );
                    let win =
                        Rect::from_min_size(r.min + vec2(r.width() * 0.2, r.height() * 0.17), vec2(r.width() * 0.6, r.height() * 0.52));
                    p.rect_filled(win, CornerRadius::same(6), pal.bg);
                    p.rect_stroke(win, CornerRadius::same(6), Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                    let side = Rect::from_min_size(win.min, vec2(win.width() * 0.28, win.height()));
                    p.rect_filled(side, CornerRadius { nw: 6, ne: 0, sw: 6, se: 0 }, pal.sidebar);
                    let pill = Rect::from_min_size(side.min + vec2(4.0, 10.0), vec2(side.width() - 8.0, win.height() * 0.1));
                    p.rect_filled(pill, CornerRadius::same(4), accent);
                    for k in 0..3 {
                        let row = Rect::from_min_size(
                            win.min + vec2(side.width() + 8.0, 10.0 + k as f32 * win.height() * 0.22),
                            vec2(win.width() * 0.62, win.height() * 0.16),
                        );
                        p.rect_filled(row, CornerRadius::same(4), pal.field);
                    }
                    let icons = pv.icons.lock().unwrap_or_else(|e| e.into_inner()).clone();
                    let n = icons.len().max(4) as f32;
                    let cell = r.height() * 0.1;
                    let dock =
                        Rect::from_center_size(pos2(r.center().x, r.bottom() - cell * 0.9), vec2(n * cell * 1.2 + cell * 0.4, cell * 1.3));
                    p.rect_filled(dock, CornerRadius::same(8), pal.bg.gamma_multiply(0.85));
                    for (i, (id, icon)) in icons.iter().enumerate() {
                        let c = pos2(dock.left() + cell * 0.2 + cell * 1.2 * (i as f32 + 0.5), dock.center().y);
                        let ir = Rect::from_center_size(c, vec2(cell, cell));
                        match icon {
                            Some(path) => {
                                egui::Image::new(format!("file://{}", path.display())).fit_to_exact_size(ir.size()).paint_at(ui, ir)
                            }
                            None => {
                                p.rect_filled(ir, CornerRadius::same(4), accent.gamma_multiply(0.6));
                                let l: String = id.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                                p.text(c, Align2::CENTER_CENTER, l, FontId::proportional(cell * 0.55), Color32::WHITE);
                            }
                        }
                    }
                    ui.add_space(4.0);
                    ui.allocate_ui(vec2(w, 18.0), |ui| {
                        ui.centered_and_justified(|ui| ui.label(egui::RichText::new("Desktop").size(12.5).color(pal.dim)));
                    });
                });
            });
            ui.add_space(6.0);
        });
        ui::footnote(
            ui,
            pal,
            "Drawn in your wallpaper, style, accent colour, icons and lock screen design; made again when one of them changes.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_design_is_read_from_lock_conf() {
        assert_eq!(design_in(""), "split", "unset: Split");
        assert_eq!(design_in("# chosen in Settings\ndesign = zen\n"), "zen");
        assert_eq!(design_in("design =\n"), "split");
        // a change to any part makes it again
        let a = Key { design: "zen".into(), ..Default::default() };
        assert_ne!(a, Key { accent: Some([1, 2, 3]), ..a.clone() });
    }
}
