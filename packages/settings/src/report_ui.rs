//! About > Report a problem: a window where the person says what went wrong and may add recent screenshots, then gets one
//! zip (the yggdrasil-report library: what the computer is, recent errors, nothing personal) and sends it to the system's
//! support with Email it. Where it goes comes from the operating system's `kind = report` integration; without one the file
//! is made all the same, for whoever helps them.

use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, SystemTime};

use eframe::egui::{self, vec2, RichText};
use yggdrasil_report::{Mail, Request, Support, AREAS, WHAT_IT_HOLDS};

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
    making: Option<mpsc::Receiver<Result<PathBuf, String>>>,
    made: Option<Result<PathBuf, String>>,
    mail: Option<Mail>,
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

    fn result(&mut self, ui: &mut egui::Ui, pal: &Palette, made: Result<PathBuf, String>) {
        let path = match made {
            Ok(p) => p,
            Err(e) => {
                ui.colored_label(pal.red, format!("The report could not be made: {e}"));
                ui.add_space(8.0);
                if ui::quiet(ui, pal, "Back").clicked() {
                    self.made = None;
                }
                return;
            }
        };
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
        match (&email, self.mail) {
            (Some(to), None) => {
                ui.label(format!("Send it to {to} so the problem can be fixed."));
            }
            (Some(to), Some(Mail::Attached)) => {
                ui.label(format!("Your mail app opened a message to {to} with the report attached: press Send there."));
            }
            (Some(to), Some(Mail::AttachYourself)) => {
                ui.add(
                    egui::Label::new(format!(
                        "A new message to {to} opened, and the folder with the report: attach {name} to the message, then send it."
                    ))
                    .wrap(),
                );
            }
            (Some(to), Some(Mail::NoMailApp)) => {
                ui.add(egui::Label::new(format!("No mail app is set up here. Send {name} to {to} from any mail or chat app.")).wrap());
            }
            (None, _) => {
                ui.add(egui::Label::new("Send this file to whoever helps you with this computer.").wrap());
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if let Some(to) = &email {
                if ui::action(ui, pal, &format!("Email it to {to}")).clicked() {
                    let subject = format!("Problem report: {}", AREAS[self.area]);
                    let said: String = self.text.trim().chars().take(1000).collect();
                    let body = format!("{said}\n\n(The full report is the attached file, {name}.)");
                    self.mail = Some(yggdrasil_report::email(to, &path, &subject, &body));
                }
            }
            if ui::quiet(ui, pal, "Show the file").clicked() {
                yggdrasil_report::show(&path);
            }
            if ui::quiet(ui, pal, "Done").clicked() {
                self.open = false;
            }
        });
    }
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
