//! Minos's own settings (Menu > Settings...): where new windows and tabs open, searching, hidden items, Proton Drive's file
//! marks and the see-through window. They live in the desktop's settings file (yggdrasil_conf::PanelConf) as before; each change
//! is written at once into a freshly read copy of that file, so nothing another program changed meanwhile is undone, and every
//! open Minos window follows within a second (they watch the settings folder). Cloud drives are managed in Settings > Cloud
//! Storage; the window has a button to it.

use eframe::egui::{self, Align, Layout, RichText};
use yggdrasil_conf::{PanelConf, StartIn, MINOS_OPACITY_MIN};
use yggdrasil_ui::theme::Palette;
use yggdrasil_ui::widgets::switch;

pub(crate) struct SettingsWin {
    conf: PanelConf,
    error: Option<String>,
}

impl SettingsWin {
    pub(crate) fn open() -> SettingsWin {
        SettingsWin { conf: PanelConf::load(), error: None }
    }

    /// Draws the window; false once it is closed. `proton`: Proton Drive was added (its row shows only then). `manage` is set
    /// when Manage cloud drives was clicked.
    pub(crate) fn show(&mut self, ctx: &egui::Context, pal: &Palette, proton: bool, manage: &mut bool) -> bool {
        let mut open = true;
        let mut changed = false;
        let c = &mut self.conf;
        let error = &self.error;
        egui::Window::new("Minos settings")
            .id(egui::Id::new("minos-settings"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(440.0);
                heading(ui, pal, "Where it opens", true);
                changed |= choice(
                    ui,
                    pal,
                    "First new window",
                    "Your home folder, or the tabs you had when you last closed Minos",
                    &mut c.minos_first_window,
                    &[(StartIn::Home, "Home"), (StartIn::Last, "Where you left off")],
                );
                changed |= choice(
                    ui,
                    pal,
                    "New window while one is open",
                    "Home, or a copy of the folder the open window is in",
                    &mut c.minos_more_windows,
                    &[(StartIn::Home, "Home"), (StartIn::Here, "Copy of where I am")],
                );
                changed |= choice(
                    ui,
                    pal,
                    "New tab",
                    "Home, or a copy of the folder the current tab is in",
                    &mut c.minos_new_tab,
                    &[(StartIn::Home, "Home"), (StartIn::Here, "Copy of where I am")],
                );

                heading(ui, pal, "Browsing", false);
                changed |= toggle(
                    ui,
                    pal,
                    "Search inside subfolders",
                    "Searching looks through every folder below the one you are in; off searches only that folder",
                    &mut c.search_subfolders,
                );
                changed |= toggle(ui, pal, "Show hidden items", "Files and folders whose names start with a dot", &mut c.show_hidden);
                if proton {
                    changed |= toggle(
                        ui,
                        pal,
                        "Show Proton Drive file status",
                        "A mark on each Proton Drive file: kept on this computer, or only in the cloud",
                        &mut c.drive_status,
                    );
                }

                heading(ui, pal, "Window", false);
                changed |= toggle(ui, pal, "See-through window", "Lets the desktop show through Minos", &mut c.minos_transparent);
                ui.add_enabled_ui(c.minos_transparent, |ui| {
                    ui.horizontal(|ui| {
                        ui.label("How solid");
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let mut v = c.minos_opacity.clamp(MINOS_OPACITY_MIN, 100);
                            if ui.add(egui::Slider::new(&mut v, MINOS_OPACITY_MIN..=100).suffix(" %")).changed() {
                                c.minos_opacity = v;
                                changed = true;
                            }
                        });
                    });
                });

                heading(ui, pal, "Cloud Storage", false);
                help(ui, pal, "Cloud drives are added, mounted and removed in Settings > Cloud Storage");
                if ui.button("Manage cloud drives...").clicked() {
                    *manage = true;
                }
                if let Some(e) = error {
                    ui.add_space(6.0);
                    ui.label(RichText::new(e).color(pal.red));
                }
            });
        if changed {
            self.error = save(&self.conf).err().map(|e| format!("Not saved: {e}"));
            ctx.request_repaint();
        }
        open
    }
}

/// Writes Minos's settings into the settings file as it is now (read again, so other programs' changes stay).
fn save(c: &PanelConf) -> Result<(), String> {
    let mut f = PanelConf::load();
    merge(&mut f, c);
    f.save()
}

/// Minos's own keys from `c` into `f`; every other key of `f` stays as it is.
fn merge(f: &mut PanelConf, c: &PanelConf) {
    f.minos_first_window = c.minos_first_window;
    f.minos_more_windows = c.minos_more_windows;
    f.minos_new_tab = c.minos_new_tab;
    f.search_subfolders = c.search_subfolders;
    f.show_hidden = c.show_hidden;
    f.drive_status = c.drive_status;
    f.minos_transparent = c.minos_transparent;
    f.minos_opacity = c.minos_opacity;
}

fn heading(ui: &mut egui::Ui, pal: &Palette, text: &str, first: bool) {
    if !first {
        ui.add_space(10.0);
        ui.separator();
    }
    ui.add_space(4.0);
    ui.label(RichText::new(text).strong().color(pal.dim));
    ui.add_space(2.0);
}

fn help(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(pal.dim));
}

/// A setting that is on or off: its name with the switch on the right, and what it does under it; true when it was flipped.
fn toggle(ui: &mut egui::Ui, pal: &Palette, label: &str, explain: &str, v: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| changed = switch(ui, pal, v).changed());
    });
    help(ui, pal, explain);
    ui.add_space(4.0);
    changed
}

/// A setting with a few choices, side by side on its right; true when another was picked.
fn choice<T: PartialEq + Copy>(ui: &mut egui::Ui, pal: &Palette, label: &str, explain: &str, v: &mut T, opts: &[(T, &str)]) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            for (o, name) in opts.iter().rev() {
                if ui.selectable_label(*v == *o, *name).clicked() && *v != *o {
                    *v = *o;
                    changed = true;
                }
            }
        });
    });
    help(ui, pal, explain);
    ui.add_space(4.0);
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_minos_keys_are_written() {
        // what another program wrote meanwhile (a key Minos does not own) stays; every Minos key is taken from the window
        let mut file = PanelConf::default();
        file.clock_24h = !file.clock_24h;
        let mut mine = PanelConf::default();
        mine.minos_first_window = StartIn::Home;
        mine.minos_more_windows = StartIn::Home;
        mine.minos_new_tab = StartIn::Home;
        mine.search_subfolders = !mine.search_subfolders;
        mine.show_hidden = !mine.show_hidden;
        mine.drive_status = !mine.drive_status;
        mine.minos_transparent = !mine.minos_transparent;
        mine.minos_opacity = 55;
        let before = file.clock_24h;
        merge(&mut file, &mine);
        assert_eq!(file.clock_24h, before);
        assert_eq!(file.minos_first_window, StartIn::Home);
        assert_eq!(file.minos_more_windows, StartIn::Home);
        assert_eq!(file.minos_new_tab, StartIn::Home);
        assert_eq!(file.search_subfolders, mine.search_subfolders);
        assert_eq!(file.show_hidden, mine.show_hidden);
        assert_eq!(file.drive_status, mine.drive_status);
        assert_eq!(file.minos_transparent, mine.minos_transparent);
        assert_eq!(file.minos_opacity, 55);
    }
}
