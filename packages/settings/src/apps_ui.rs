//! Settings > Apps: the installed apps (restart, close, remove), one page of choices per app (network, startup,
//! storage, files, other programs), and Labyrinth's optional software and Minos on their own tabs.

use std::collections::HashMap;

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, RichText, Sense, Stroke, UiBuilder};
use labyrinth_ui::theme::Palette;

use crate::appmgr::{self, AppInfo, AppPolicy, Kind, NetMode};
use crate::icons::{self, Icon};
use crate::{ui, App};

#[derive(Clone, Debug, PartialEq)]
pub enum Confirm {
    /// Remove the app (shows the package it would remove).
    Remove(String),
    /// Clear the app's saved settings and data (asks for the password).
    ClearData(String),
}

pub const SORTS: [&str; 3] = ["Usage", "Size", "A to Z"];

fn human(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{bytes} B")
    }
}

fn span(secs: u64) -> String {
    match secs {
        0..=59 => format!("{secs} s"),
        60..=3599 => format!("{} min", secs / 60),
        _ => format!("{} h {} min", secs / 3600, secs % 3600 / 60),
    }
}

fn ago(last: u64) -> String {
    if last == 0 {
        return "never opened from here".into();
    }
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let s = now.saturating_sub(last);
    match s {
        0..=89 => "just now".into(),
        90..=5399 => format!("{} min ago", s / 60),
        5400..=129_599 => format!("{} h ago", s / 3600),
        _ => format!("{} days ago", s / 86_400),
    }
}

/// An icon-only button in a row: a hover plate, the shape, and a tooltip.
fn icon_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, tip: &str, enabled: bool, danger: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), if enabled { Sense::click() } else { Sense::hover() });
    if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(8), if danger { Color32::from_rgba_unmultiplied(220, 60, 70, 40) } else { pal.hover });
    }
    let c = if !enabled { pal.dim.gamma_multiply(0.4) } else if danger && resp.hovered() { Color32::from_rgb(235, 90, 100) } else { pal.text };
    icons::paint(ui.painter(), rect.shrink(6.0), icon, c);
    resp.on_hover_text(tip)
}

/// The app's icon: its picture, or a coloured tile with its first letter when the pack has none.
fn app_icon(ui: &mut egui::Ui, pal: &Palette, a: &AppInfo, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    match &a.icon_path {
        Some(p) => {
            egui::Image::new(format!("file://{}", p.display())).fit_to_exact_size(vec2(size, size)).paint_at(ui, rect);
        }
        None => {
            ui.painter().rect_filled(rect, CornerRadius::same((size * 0.24) as u8), pal.accent.gamma_multiply(0.55));
            let l: String = a.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, l, FontId::proportional(size * 0.5), Color32::WHITE);
        }
    }
}

fn sorted<'a>(apps: &'a [AppInfo], sort: usize, sizes: &HashMap<String, u64>, filter: &str, system: bool) -> Vec<&'a AppInfo> {
    let f = filter.trim().to_lowercase();
    let mut v: Vec<&AppInfo> = apps.iter().filter(|a| (system || !a.system || a.running.len() > 0 || a.net != NetMode::Allow) && (f.is_empty() || a.name.to_lowercase().contains(&f) || a.id.to_lowercase().contains(&f))).collect();
    match sort {
        0 => v.sort_by(|a, b| b.seconds.cmp(&a.seconds).then(b.launches.cmp(&a.launches)).then(b.last_used.cmp(&a.last_used)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))),
        1 => v.sort_by(|a, b| sizes.get(&b.id).unwrap_or(&0).cmp(sizes.get(&a.id).unwrap_or(&0)).then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))),
        _ => v.sort_by_key(|a| a.name.to_lowercase()),
    }
    v
}

impl App {
    pub(crate) fn page_apps(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        if self.app_sel.is_none() {
            if let Some(i) = ui::segmented(ui, pal, &["Installed", "Optional software", "Minos"], self.app_tab) {
                self.app_tab = i;
            }
        }
        match (self.app_tab, self.app_sel.clone()) {
            (0, Some(id)) => self.app_detail(ui, pal, ctx, &id),
            (0, None) => self.app_list(ui, pal, ctx),
            (1, _) => self.page_apps_software(ui, pal),
            _ => self.page_apps_minos(ui, pal, ctx),
        }
        self.app_confirm_window(ctx, pal);
    }

    fn app_list(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(apps) = self.data.installed.clone() else { return crate::loading(ui) };
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Sort by").color(pal.dim));
            if let Some(i) = ui::segmented(ui, pal, &SORTS, self.app_sort) {
                self.app_sort = i;
            }
            ui.add_space(8.0);
            ui.add(egui::TextEdit::singleline(&mut self.app_filter).hint_text("Find an app").desired_width(180.0));
            ui.add_space(8.0);
            ui.checkbox(&mut self.app_system, "Show system tools");
        });
        let list = sorted(&apps, self.app_sort, &self.app_sizes, &self.app_filter, self.app_system);
        let mut action: Option<(String, &'static str)> = None;
        ui::group(ui, pal, None, |ui| {
            if list.is_empty() {
                ui.add_space(12.0);
                ui.label(RichText::new("No apps match.").color(pal.dim));
                ui.add_space(12.0);
            }
            for (i, a) in list.iter().enumerate() {
                if i > 0 {
                    let r = ui.available_rect_before_wrap();
                    ui.painter().hline(r.left()..=r.right(), r.top(), Stroke::new(1.0, pal.border));
                }
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(rect.shrink2(vec2(-6.0, 2.0)), CornerRadius::same(8), pal.hover.gamma_multiply(0.6));
                }
                let mut left = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(rect.min + vec2(0.0, 10.0), vec2(rect.width() - 120.0, 36.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                app_icon(&mut left, pal, a, 34.0);
                left.add_space(8.0);
                left.vertical(|ui| {
                    ui.add_space(2.0);
                    ui.add(egui::Label::new(RichText::new(&a.name).size(14.0)).truncate());
                    let size = self.app_sizes.get(&a.id).map(|b| human(*b)).unwrap_or_default();
                    let mut bits: Vec<String> = vec![];
                    if !a.running.is_empty() {
                        bits.push("Running".into());
                    }
                    bits.push(if a.launches > 0 { format!("opened {} time{} here", a.launches, if a.launches == 1 { "" } else { "s" }) } else { "not opened from here yet".into() });
                    if a.seconds >= 60 {
                        bits.push(format!("used {}", span(a.seconds)));
                    }
                    if !size.is_empty() && size != "0 B" {
                        bits.push(size);
                    }
                    if a.net != NetMode::Allow {
                        bits.push(match a.net {
                            NetMode::Block => "no network".into(),
                            NetMode::Ethernet => "Ethernet only".into(),
                            _ => "Wi-Fi only".into(),
                        });
                    }
                    ui.add(egui::Label::new(RichText::new(bits.join(" · ")).size(12.0).color(pal.dim)).truncate());
                });
                let mut buttons = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_size(pos2(rect.right() - 110.0, rect.top() + 13.0), vec2(110.0, 30.0))).layout(egui::Layout::right_to_left(egui::Align::Center)));
                if icon_button(&mut buttons, pal, Icon::Trash, "Remove", true, true).clicked() {
                    action = Some((a.id.clone(), "remove"));
                }
                if icon_button(&mut buttons, pal, Icon::Close, "Close", !a.running.is_empty(), false).clicked() {
                    action = Some((a.id.clone(), "close"));
                }
                if icon_button(&mut buttons, pal, Icon::Restart, "Restart", !a.running.is_empty(), false).clicked() {
                    action = Some((a.id.clone(), "restart"));
                }
                if resp.clicked() && action.is_none() {
                    action = Some((a.id.clone(), "open"));
                }
            }
        });
        if let Some((id, what)) = action {
            let app = apps.iter().find(|a| a.id == id).cloned();
            match (what, app) {
                ("open", _) => self.app_sel = Some(id),
                ("remove", _) => self.app_confirm = Some(Confirm::Remove(id)),
                ("close", Some(a)) => self.change(ctx, move || stop_app(&a)),
                ("restart", Some(a)) => self.change(ctx, move || {
                    stop_app(&a)?;
                    std::thread::sleep(std::time::Duration::from_millis(900));
                    crate::sys::act("labyrinth-launch", &[&a.id])
                }),
                _ => {}
            }
        }
    }

    fn app_detail(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context, id: &str) {
        let Some(apps) = self.data.installed.clone() else { return crate::loading(ui) };
        let Some(a) = apps.iter().find(|a| a.id == id).cloned() else {
            self.app_sel = None;
            return;
        };
        let developer = labyrinth_conf::PanelConf::load().developer;
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if icon_button(ui, pal, Icon::Back, "All apps", true, false).clicked() {
                self.app_sel = None;
            }
            app_icon(ui, pal, &a, 40.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(&a.name).size(20.0).strong());
                if !a.comment.is_empty() {
                    ui.add(egui::Label::new(RichText::new(&a.comment).size(12.0).color(pal.dim)).truncate());
                }
            });
        });

        let mut net: Option<NetMode> = None;
        let mut startup: Option<bool> = None;
        let mut policy: Option<AppPolicy> = None;
        let mut clear_cache = false;
        let mut clear_data = false;
        let mut remove = false;
        let mut restart = false;
        let mut close = false;

        ui::group(ui, pal, Some("Network"), |ui| {
            let allowed = a.net != NetMode::Block;
            let top = ui.cursor().top();
            let left = ui.min_rect().left();
            ui::row(ui, pal, true, "Can use the network", Some(if allowed { "Switch it off, or click its name, to cut this app off from the network" } else { "This app cannot send anything off this computer" }), |ui| {
                let mut on = allowed;
                if ui::switch(ui, pal, &mut on).changed() {
                    net = Some(if on { NetMode::Allow } else { NetMode::Block });
                }
                if developer && allowed {
                    // Developer options: limit it to one kind of connection
                    let label = match a.net {
                        NetMode::Ethernet => "Ethernet only",
                        NetMode::Wifi => "Wi-Fi only",
                        _ => "Any connection",
                    };
                    egui::ComboBox::from_id_salt("net-mode").selected_text(label).show_ui(ui, |ui| {
                        for (m, l) in [(NetMode::Allow, "Any connection"), (NetMode::Ethernet, "Ethernet only"), (NetMode::Wifi, "Wi-Fi only")] {
                            if ui.selectable_label(a.net == m, l).clicked() {
                                net = Some(m);
                            }
                        }
                    });
                }
            });
            // clicking the name toggles it too (the part of the row left of the controls)
            let name_area = Rect::from_min_max(pos2(left, top), pos2(left + (ui.available_width() - 260.0).max(120.0), top + 44.0));
            if ui.interact(name_area, ui.id().with("net-name"), Sense::click()).clicked() {
                net = Some(if allowed { NetMode::Block } else { NetMode::Allow });
            }
            ui.label(RichText::new("Applies to the app when it is opened from the dock, the start menu or here. Changing it asks for your password. A running app picks it up after a restart.").size(11.5).color(pal.dim));
            ui.add_space(6.0);
        });

        ui::group(ui, pal, Some("General"), |ui| {
            ui::row(ui, pal, true, "Open when I log in", None, |ui| {
                let mut on = a.startup;
                if ui::switch(ui, pal, &mut on).changed() {
                    startup = Some(on);
                }
            });
            ui::row(ui, pal, false, "Status", Some(&format!("Opened {} time{} from here, last {}. Used for {} in total", a.launches, if a.launches == 1 { "" } else { "s" }, ago(a.last_used), span(a.seconds))), |ui| {
                if icon_button(ui, pal, Icon::Close, "Close", !a.running.is_empty(), false).clicked() {
                    close = true;
                }
                if icon_button(ui, pal, Icon::Restart, "Restart", !a.running.is_empty(), false).clicked() {
                    restart = true;
                }
                ui.label(RichText::new(if a.running.is_empty() { "Not running" } else { "Running" }).color(if a.running.is_empty() { pal.dim } else { pal.accent }));
            });
        });

        let dirs = appmgr::data_dirs(&a);
        let cache: u64 = dirs.iter().filter(|(_, k)| *k == Kind::Cache).map(|(p, _)| self.app_dir_size(p)).sum();
        let data: u64 = dirs.iter().filter(|(_, k)| *k == Kind::Data).map(|(p, _)| self.app_dir_size(p)).sum();
        ui::group(ui, pal, Some("Storage"), |ui| {
            ui::row(ui, pal, true, "Cache", Some(&format!("{} · temporary files the app rebuilds", human(cache))), |ui| {
                if ui::quiet(ui, pal, "Clear cache").clicked() {
                    clear_cache = true;
                }
            });
            ui::row(ui, pal, false, "Settings and data", Some(&format!("{} · the app's saved settings and files; clearing it is like a fresh install", human(data))), |ui| {
                if ui::quiet(ui, pal, "Clear data...").clicked() {
                    clear_data = true;
                }
            });
        });

        let sandbox_ok = std::path::Path::new("/usr/bin/bwrap").exists();
        ui::group(ui, pal, Some("Privacy sandbox (experimental)"), |ui| {
            if !sandbox_ok {
                ui.add_space(8.0);
                ui.label(RichText::new("Needs bubblewrap (package bubblewrap).").color(pal.dim));
                ui.add_space(8.0);
                return;
            }
            let mut p = a.policy.clone();
            let mut changed = false;
            ui::row(ui, pal, true, "Run in a sandbox", Some("The app starts in a restricted view of your files and programs. Takes effect the next time it opens"), |ui| {
                changed |= ui::switch(ui, pal, &mut p.sandbox).changed();
            });
            if p.sandbox {
                ui::row(ui, pal, false, "Files it can see", Some("All of your files, or only the folders you add (and its own settings)"), |ui| {
                    let sel = if p.files_all { 0 } else { 1 };
                    if let Some(i) = ui::segmented(ui, pal, &["All files", "Chosen folders"], sel) {
                        p.files_all = i == 0;
                        changed = true;
                    }
                });
                if !p.files_all {
                    let mut remove_at: Option<usize> = None;
                    for (i, f) in p.folders.iter().enumerate() {
                        ui::row(ui, pal, false, f, None, |ui| {
                            if icon_button(ui, pal, Icon::Close, "Stop sharing this folder", true, true).clicked() {
                                remove_at = Some(i);
                            }
                        });
                    }
                    if let Some(i) = remove_at {
                        p.folders.remove(i);
                        changed = true;
                    }
                    ui::row(ui, pal, false, "Add a folder", Some("Type or paste its path, for example ~/Documents"), |ui| {
                        let add = ui::action(ui, pal, "Add").clicked();
                        ui.add(egui::TextEdit::singleline(&mut self.app_folder).hint_text("~/Documents").desired_width(200.0));
                        if add && !self.app_folder.trim().is_empty() {
                            let raw = self.app_folder.trim().to_string();
                            let path = if let Some(rest) = raw.strip_prefix("~/") { format!("{}/{rest}", std::env::var("HOME").unwrap_or_default()) } else { raw };
                            if std::path::Path::new(&path).is_dir() && !p.folders.contains(&path) {
                                p.folders.push(path);
                                changed = true;
                            }
                            self.app_folder.clear();
                        }
                    });
                }
                ui::row(ui, pal, false, "Can see other programs", Some("Off: it cannot see what else is running"), |ui| {
                    changed |= ui::switch(ui, pal, &mut p.see_others).changed();
                });
            }
            if std::path::Path::new("/usr/bin/limbo").exists() {
                ui.label(RichText::new("Limbo is installed: Windows apps and anything you do not trust can run in a Limbo guest instead (see `limbo doctor`).").size(11.5).color(pal.dim));
                ui.add_space(6.0);
            }
            if changed {
                policy = Some(p);
            }
        });

        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Remove this app", Some("Uninstalls it from the computer"), |ui| {
                if ui::quiet(ui, pal, "Remove...").clicked() {
                    remove = true;
                }
            });
        });

        // apply
        if let Some(m) = net {
            let id = a.id.clone();
            self.change(ctx, move || {
                let _ = std::process::Command::new("systemctl").args(["--user", "start", &appmgr::slice_name(&id)]).status();
                crate::sys::act("pkexec", &["/usr/local/bin/labyrinth-appnet", "set", &id, m.word()])
            });
        }
        if let Some(on) = startup {
            let app = a.clone();
            self.change(ctx, move || appmgr::set_startup(&app, on));
        }
        if let Some(p) = policy {
            let id = a.id.clone();
            self.change(ctx, move || appmgr::save_policy(&id, &p));
        }
        if clear_cache {
            let app = a.clone();
            self.change(ctx, move || appmgr::clear_dirs(&app, Kind::Cache).map(|_| ()));
        }
        if clear_data {
            self.app_confirm = Some(Confirm::ClearData(a.id.clone()));
        }
        if remove {
            self.app_confirm = Some(Confirm::Remove(a.id.clone()));
        }
        if close {
            let app = a.clone();
            self.change(ctx, move || stop_app(&app));
        }
        if restart {
            let app = a.clone();
            self.change(ctx, move || {
                stop_app(&app)?;
                std::thread::sleep(std::time::Duration::from_millis(900));
                crate::sys::act("labyrinth-launch", &[&app.id])
            });
        }
    }

    /// The size of a folder, measured once in the background and remembered.
    fn app_dir_size(&mut self, p: &std::path::Path) -> u64 {
        if let Some(s) = self.dir_sizes.get(p) {
            return *s;
        }
        let tx = self.tx.clone();
        let path = p.to_path_buf();
        self.dir_sizes.insert(path.clone(), 0);
        std::thread::spawn(move || {
            let n = appmgr::dir_size(&path);
            let _ = tx.send(crate::Msg::DirSize(path, n));
        });
        0
    }

    fn app_confirm_window(&mut self, ctx: &egui::Context, pal: &Palette) {
        let Some(c) = self.app_confirm.clone() else { return };
        let apps = self.data.installed.clone().unwrap_or_default();
        let id = match &c {
            Confirm::Remove(i) | Confirm::ClearData(i) => i.clone(),
        };
        let Some(a) = apps.iter().find(|a| a.id == id).cloned() else {
            self.app_confirm = None;
            return;
        };
        let mut close = false;
        let mut go = false;
        let owner = if matches!(c, Confirm::Remove(_)) { self.app_owner(&a) } else { None };
        egui::Window::new(match c {
            Confirm::Remove(_) => format!("Remove {}?", a.name),
            Confirm::ClearData(_) => format!("Clear {}'s data?", a.name),
        })
        .collapsible(false)
        .resizable(false)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.set_max_width(420.0);
            match &c {
                Confirm::Remove(_) => {
                    match &owner {
                        Some((pkg, true)) => {
                            ui.label(format!("This removes the Flatpak {pkg}. Its saved data stays unless you clear it first."));
                        }
                        Some((pkg, false)) if appmgr::protected(pkg) => {
                            ui.label(RichText::new(format!("{} is part of the desktop (package {pkg}) and is not removed from here.", a.name)).color(pal.accent));
                        }
                        Some((pkg, false)) => {
                            ui.label(format!("This uninstalls the package {pkg} and the settings it keeps system-wide. Your own settings and files stay."));
                        }
                        None => {
                            ui.label(RichText::new("Settings cannot tell which package this app came from, so it will not remove it.").color(pal.accent));
                        }
                    };
                }
                Confirm::ClearData(_) => {
                    ui.label("These folders will be deleted. The app starts fresh the next time, as after a new install:");
                    ui.add_space(4.0);
                    for (p, k) in appmgr::data_dirs(&a).into_iter().filter(|(_, k)| *k == Kind::Data) {
                        let _ = k;
                        ui.label(RichText::new(p.display().to_string()).size(12.0).color(pal.dim));
                    }
                    ui.add_space(4.0);
                    ui.label("You will be asked for your password.");
                }
            }
            ui.add_space(8.0);
            let removable = match (&c, &owner) {
                (Confirm::Remove(_), Some((pkg, false))) => !appmgr::protected(pkg),
                (Confirm::Remove(_), Some((_, true))) => true,
                (Confirm::Remove(_), None) => false,
                _ => true,
            };
            ui.horizontal(|ui| {
                if ui.add_enabled(removable, egui::Button::new(match c { Confirm::Remove(_) => "Remove", Confirm::ClearData(_) => "Clear data" })).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if go {
            self.app_confirm = None;
            self.app_sel = if matches!(c, Confirm::Remove(_)) { None } else { self.app_sel.clone() };
            match c {
                Confirm::Remove(_) => {
                    if let Some((pkg, flat)) = owner {
                        self.change(ctx, move || {
                            if flat {
                                crate::sys::act("flatpak", &["uninstall", "-y", "--noninteractive", &pkg])
                            } else {
                                crate::sys::act("pkexec", &["pacman", "-Rns", "--noconfirm", &pkg])
                            }
                        });
                    }
                }
                Confirm::ClearData(_) => {
                    let app = a.clone();
                    self.change(ctx, move || {
                        crate::sys::act("pkexec", &["/usr/bin/true"])?;
                        appmgr::clear_dirs(&app, Kind::Data).map(|_| ())
                    });
                }
            }
        } else if close {
            self.app_confirm = None;
        }
    }

    /// Which package (or Flatpak) the app came from, looked up once.
    fn app_owner(&mut self, a: &AppInfo) -> Option<(String, bool)> {
        if let Some(o) = self.owners.get(&a.id) {
            return o.clone();
        }
        let o = appmgr::owner(a);
        self.owners.insert(a.id.clone(), o.clone());
        o
    }
}

/// Close an app: stop the scopes it runs in.
fn stop_app(a: &AppInfo) -> Result<(), String> {
    if a.running.is_empty() {
        return Ok(());
    }
    let mut args = vec!["--user".to_string(), "stop".to_string()];
    args.extend(a.running.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    crate::sys::act("systemctl", &refs)
}
