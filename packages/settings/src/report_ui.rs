//! About > Report a problem: a window where the person says what went wrong and may add recent screenshots, then gets one
//! zip (the yggdrasil-report library: what the computer is, recent errors, nothing personal) and sends it to the system's
//! support: Email it (a mail program with the zip attached), or the address, subject and message to copy into any mail
//! service, or, for someone with a GitHub account, a public issue. Where it goes comes from the operating system's
//! `kind = report` integration; without one the file is made all the same, for whoever helps them.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use eframe::egui::{self, vec2, RichText};
use yggdrasil_report::{Made, Mail, Request, Support, AREAS, WHAT_IT_HOLDS};

use crate::theme::Palette;
use crate::ui;

#[derive(Default)]
pub struct ReportUi {
    pub open: bool,
    area: usize,
    text: String,
    /// Recent screenshots (Pictures/Screenshots, the last day), each ticked to go in the report or not.
    pictures: Vec<(PathBuf, bool)>,
    support: Option<Support>,
    making: Option<mpsc::Receiver<Result<Made, String>>>,
    made: Option<Result<Made, String>>,
    mail: Option<Mail>,
    /// Open a GitHub issue was pressed (and whether the browser opened).
    github: Option<bool>,
    /// The text box takes the keyboard when the window opens.
    focus: bool,
}

impl ReportUi {
    /// Opens the window with an empty form.
    pub fn start(&mut self) {
        *self = ReportUi { open: true, focus: true, support: Support::find(), pictures: recent_screenshots(), ..ReportUi::default() };
    }

    pub fn window(&mut self, ctx: &egui::Context, pal: &Palette) {
        if !self.open {
            return;
        }
        if let Some(rx) = &self.making {
            match rx.try_recv() {
                Ok(r) => {
                    self.made = Some(r);
                    self.making = None;
                }
                Err(_) => ctx.request_repaint_after(Duration::from_millis(150)),
            }
        }
        let mut open = true;
        egui::Window::new("Report a problem")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(460.0);
                if self.making.is_some() {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Making the report: a few seconds.");
                    });
                } else if let Some(made) = self.made.clone() {
                    self.result(ui, pal, made);
                } else {
                    self.form(ui, pal, ctx);
                }
            });
        if !open {
            self.open = false;
        }
    }

    fn form(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        ui.label(RichText::new("What is it about?").strong());
        egui::ComboBox::from_id_salt("report-area").width(300.0).selected_text(AREAS[self.area]).show_ui(ui, |ui| {
            for (i, a) in AREAS.iter().enumerate() {
                ui.selectable_value(&mut self.area, i, *a);
            }
        });
        ui.add_space(8.0);
        ui.label(RichText::new("What happened?").strong());
        let text = ui.add(
            egui::TextEdit::multiline(&mut self.text)
                .hint_text("What you did, what happened, and what you expected instead")
                .desired_rows(5)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut self.focus) {
            text.request_focus();
        }
        ui.add_space(8.0);
        ui.label(RichText::new("Pictures").strong());
        if self.pictures.is_empty() {
            ui.add(
                egui::Label::new(
                    RichText::new("To show the problem, press Print Screen first: the picture appears here to add.").color(pal.dim),
                )
                .wrap(),
            );
        } else {
            for (p, on) in &mut self.pictures {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                ui.checkbox(on, name);
            }
        }
        if ui::quiet(ui, pal, "Look again").clicked() {
            let ticked: Vec<PathBuf> = self.pictures.iter().filter(|(_, on)| *on).map(|(p, _)| p.clone()).collect();
            self.pictures = recent_screenshots().into_iter().map(|(p, _)| (p.clone(), ticked.contains(&p))).collect();
        }
        ui.add_space(8.0);
        ui.add(egui::Label::new(RichText::new(WHAT_IT_HOLDS).size(12.0).color(pal.dim)).wrap());
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            let ready = self.text.trim().chars().count() >= 3;
            if ui.add_enabled_ui(ready, |ui| ui::action(ui, pal, "Create report")).inner.clicked() {
                let req = Request {
                    area: AREAS[self.area].to_string(),
                    text: self.text.clone(),
                    pictures: self.pictures.iter().filter(|(_, on)| *on).map(|(p, _)| p.clone()).collect(),
                };
                let support = self.support.clone();
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(yggdrasil_report::make(&req, support.as_ref()));
                    ctx.request_repaint();
                });
                self.making = Some(rx);
            }
            if ui::quiet(ui, pal, "Cancel").clicked() {
                self.open = false;
            }
            if !ready {
                ui.label(RichText::new("Say what happened first").size(12.0).color(pal.dim));
            }
        });
    }

    fn result(&mut self, ui: &mut egui::Ui, pal: &Palette, made: Result<Made, String>) {
        let made = match made {
            Ok(m) => m,
            Err(e) => {
                ui.colored_label(pal.red, format!("The report could not be made: {e}"));
                ui.add_space(8.0);
                if ui::quiet(ui, pal, "Back").clicked() {
                    self.made = None;
                }
                return;
            }
        };
        let path = made.path.clone();
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let home = std::env::var("HOME").map(PathBuf::from).ok();
        let folder = match path.parent() {
            Some(p) if home.as_deref() == Some(p) => "your home folder".to_string(),
            Some(p) => p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            None => String::new(),
        };
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        ui.label(RichText::new("Your report is ready").strong().size(16.0));
        ui.label(format!("Saved as {name} in {folder} ({}).", kib(size)));
        ui.add_space(8.0);
        let email = self.support.as_ref().and_then(|s| s.email.clone());
        let github = self.support.as_ref().and_then(|s| s.github.clone());
        if let Some(to) = &email {
            if ui::action(ui, pal, &format!("Email it to {to}")).clicked() {
                self.mail = Some(yggdrasil_report::email(to, &path, &made.subject, &made.message));
            }
            let said = match self.mail {
                None => None,
                Some(Mail::Attached) => Some("Your mail app opened a message with the report attached: press Send there.".to_string()),
                Some(Mail::AttachYourself) => {
                    Some(format!("A new message opened, and the folder with the report: attach {name} to it, then send it."))
                }
                Some(Mail::NoMailApp) => Some("No mail app is set up here: copy these into any mail service instead.".to_string()),
            };
            if let Some(s) = said {
                ui.add(egui::Label::new(RichText::new(s).color(pal.dim)).wrap());
            }
            ui.add_space(8.0);
            ui.label(RichText::new("Or paste these into any mail service, and attach the file:").strong());
            copy_line(ui, pal, "To", to);
            copy_line(ui, pal, "Subject", &made.subject);
            ui.horizontal(|ui| {
                ui.add_sized(vec2(60.0, 18.0), egui::Label::new(RichText::new("Message").color(pal.dim)));
                if ui::quiet(ui, pal, "Copy").clicked() {
                    ui.ctx().copy_text(made.message.clone());
                }
            });
            ui.add(egui::TextEdit::multiline(&mut made.message.as_str()).desired_rows(4).desired_width(f32::INFINITY));
        } else {
            ui.add(egui::Label::new("Send this file to whoever helps you with this computer.").wrap());
        }
        if let Some(repo) = &github {
            ui.add_space(10.0);
            ui.add(
                egui::Label::new(
                    RichText::new("Have a GitHub account? You can open an issue there instead. GitHub issues are public, and they show your account's name.")
                        .color(pal.dim),
                )
                .wrap(),
            );
            if ui::quiet(ui, pal, "Open a GitHub issue").clicked() {
                self.github = Some(yggdrasil_report::github_issue(repo, &made.subject, &made.message, &path));
            }
            match self.github {
                Some(true) => {
                    ui.label(
                        RichText::new(format!("Your browser opened the issue: drag {name} into it to attach the report.")).color(pal.dim),
                    );
                }
                Some(false) => {
                    ui.colored_label(pal.red, "No browser could be opened.");
                }
                None => {}
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui::quiet(ui, pal, "Show the file").clicked() {
                yggdrasil_report::show(&path);
            }
            if ui::quiet(ui, pal, "Done").clicked() {
                self.open = false;
            }
        });
    }
}

/// One value to copy: its title, the value (selectable) and a Copy button.
fn copy_line(ui: &mut egui::Ui, pal: &Palette, title: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(vec2(60.0, 18.0), egui::Label::new(RichText::new(title).color(pal.dim)));
        ui.add(egui::TextEdit::singleline(&mut &*value).desired_width(280.0));
        if ui::quiet(ui, pal, "Copy").clicked() {
            ui.ctx().copy_text(value.to_string());
        }
    });
}

fn kib(bytes: u64) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / 1048576.0)
    } else {
        format!("{} KB", bytes.div_ceil(1024))
    }
}

/// The screenshots of the last day (Pictures/Screenshots, where Print Screen saves them), newest first, at most six; none ticked.
fn recent_screenshots() -> Vec<(PathBuf, bool)> {
    let pictures = yggdrasil_report::run(&["xdg-user-dir", "PICTURES"], Duration::from_secs(5))
        .map(|s| PathBuf::from(s.trim()))
        .filter(|p| p.is_dir())
        .or_else(|| std::env::var("HOME").ok().map(|h| PathBuf::from(h).join("Pictures")));
    let Some(dir) = pictures.map(|p| p.join("Screenshots")) else { return vec![] };
    let day_ago = SystemTime::now() - Duration::from_secs(24 * 3600);
    let mut shots: Vec<(SystemTime, PathBuf)> = std::fs::read_dir(dir)
        .map(|r| {
            r.flatten()
                .filter_map(|e| {
                    let p = e.path();
                    let image = p
                        .extension()
                        .and_then(|x| x.to_str())
                        .is_some_and(|x| matches!(x.to_ascii_lowercase().as_str(), "png" | "jpg" | "jpeg" | "webp"));
                    let when = e.metadata().ok()?.modified().ok()?;
                    (image && when >= day_ago).then_some((when, p))
                })
                .collect()
        })
        .unwrap_or_default();
    shots.sort_by_key(|s| std::cmp::Reverse(s.0));
    shots.into_iter().take(6).map(|(_, p)| (p, false)).collect()
}
