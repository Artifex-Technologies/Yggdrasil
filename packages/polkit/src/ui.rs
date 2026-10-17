//! The password box: a small window in Labyrinth's look. It stays hidden until polkit asks for something.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, pos2, vec2, Align, Color32, CornerRadius, Key, Layout, Margin, Rect, RichText, Sense, Stroke, TextEdit};
use labyrinth_ui::chrome;
use labyrinth_ui::theme;
use zeroize::Zeroize;

use crate::agent::UiMsg;

pub struct Dialog {
    message: String,
    action_id: String,
    user: String,
    /// The prompt being answered (its text, whether typing is shown), and where the answer goes.
    asking: Option<(String, bool, Sender<Option<String>>)>,
    input: String,
    notes: Vec<(String, bool)>,
    show_details: bool,
}

pub struct App {
    rx: Receiver<UiMsg>,
    dialog: Option<Dialog>,
    ctx_slot: Arc<Mutex<Option<egui::Context>>>,
    /// A request has begun (so an empty dialog afterwards means it is over).
    started: bool,
}

impl App {
    pub fn new(rx: Receiver<UiMsg>, ctx_slot: Arc<Mutex<Option<egui::Context>>>, ctx: &egui::Context) -> App {
        theme::apply(ctx, theme::system_dark(), theme::system_accent());
        *ctx_slot.lock().unwrap() = Some(ctx.clone());
        App { rx, dialog: None, ctx_slot, started: false }
    }

    fn receive(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                UiMsg::Begin { message, action_id, user } => {
                    self.dialog = Some(Dialog { message, action_id, user, asking: None, input: String::new(), notes: vec![], show_details: false });
                }
                UiMsg::Prompt { text, echo, reply } => {
                    if let Some(d) = &mut self.dialog {
                        d.asking = Some((text, echo, reply));
                        d.input.zeroize();
                    }
                }
                UiMsg::Message { text, error } => {
                    if let Some(d) = &mut self.dialog {
                        d.notes.push((text, error));
                    }
                }
                UiMsg::Done { .. } => {
                    if let Some(mut d) = self.dialog.take() {
                        d.input.zeroize();
                    }
                }
            }
        }
    }
}

/// What polkit's message says, in plain words when it is a package command from the Marketplace (or pacman by hand).
pub fn describe(message: &str) -> Option<String> {
    let idx = message.find("pacman ")?;
    let rest = &message[idx + "pacman ".len()..];
    let rest = rest.split(['\'', '`', '"']).next().unwrap_or(rest);
    let tokens: Vec<&str> = rest.split_whitespace().collect();
    let pkgs: Vec<&str> = tokens.iter().copied().filter(|t| !t.starts_with('-')).collect();
    let remove = tokens.iter().any(|t| t.starts_with("-R"));
    let install = tokens.iter().any(|t| t.starts_with("-S") || *t == "-U");
    let what = if pkgs.is_empty() { "software".to_string() } else { pkgs.join(", ") };
    let by = if message.contains("labyrinth-pkg-root") { "The Marketplace wants to " } else { "A program wants to " };
    Some(if remove {
        format!("{by}remove {what}.")
    } else if install {
        format!("{by}install or reinstall {what}.")
    } else {
        format!("{by}change installed software.")
    })
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        self.receive();
        let dark = theme::system_dark();
        let accent = theme::system_accent();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        let Some(d) = &mut self.dialog else {
            // the request is over: this process only exists for one request, so it ends and the window goes with it
            if self.started {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        };
        self.started = true;
        let mut answer: Option<Option<String>> = None;
        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 26, right: 26, top: 6, bottom: 20 })).show(ui, |ui| draw(ui, d, &pal, &mut answer));
        chrome::outline(&ctx, &pal);
        if let Some(a) = answer {
            if let Some((_, _, reply)) = d.asking.take() {
                let _ = reply.send(a);
            }
            d.input.zeroize();
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
        let _ = &self.ctx_slot;
    }
}

/// A padlock in a rounded accent square.
fn padlock(ui: &mut egui::Ui, pal: &theme::Palette, size: f32) {
    let (r, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same((size * 0.26) as u8), pal.accent);
    let ink = pal.on_accent();
    let c = r.center();
    let body = Rect::from_center_size(c + vec2(0.0, size * 0.1), vec2(size * 0.44, size * 0.32));
    p.rect_filled(body, CornerRadius::same(4), ink);
    // the shackle: half a ring above the body
    let (cx, cy, rad) = (c.x, body.top(), size * 0.15);
    let pts: Vec<egui::Pos2> = (0..=16).map(|k| {
        let a = std::f32::consts::PI * (1.0 + k as f32 / 16.0);
        pos2(cx + rad * a.cos(), cy + rad * a.sin())
    }).collect();
    p.add(egui::Shape::line(pts, Stroke::new(size * 0.07, ink)));
    p.circle_filled(body.center(), size * 0.045, pal.accent);
}

fn draw(ui: &mut egui::Ui, d: &mut Dialog, pal: &theme::Palette, answer: &mut Option<Option<String>>) {
    if ui.input(|i| i.viewport().close_requested()) {
        *answer = Some(None);
    }
    // the top strip: drag the window by it; the red dot cancels
    ui.horizontal(|ui| {
        chrome::drag_area(ui, ui.available_width() - 26.0);
        let (r, resp) = ui.allocate_exact_size(vec2(18.0, 24.0), Sense::click());
        let c = r.center();
        ui.painter().circle_filled(c, 6.5, Color32::from_rgb(255, 95, 87));
        if resp.hovered() {
            let ink = Stroke::new(1.4, Color32::from_black_alpha(150));
            ui.painter().line_segment([c - vec2(2.6, 2.6), c + vec2(2.6, 2.6)], ink);
            ui.painter().line_segment([c + vec2(-2.6, 2.6), c + vec2(2.6, -2.6)], ink);
        }
        if resp.on_hover_text("Cancel").clicked() {
            *answer = Some(None);
        }
    });
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        padlock(ui, pal, 48.0);
        ui.add_space(8.0);
        ui.vertical(|ui| {
            ui.label(RichText::new("Administrator password needed").size(19.0).strong());
            let summary = describe(&d.message).unwrap_or_else(|| "A program is asking for administrator rights.".to_string());
            ui.add(egui::Label::new(RichText::new(summary).color(pal.dim)).wrap());
        });
    });
    ui.add_space(4.0);
    if ui.add(egui::Label::new(RichText::new(if d.show_details { "Hide details" } else { "Show details" }).size(12.5).color(pal.accent)).sense(Sense::click())).clicked() {
        d.show_details = !d.show_details;
    }
    if d.show_details {
        egui::Frame::new().fill(pal.field).corner_radius(CornerRadius::same(8)).inner_margin(Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(egui::Label::new(RichText::new(if d.message.is_empty() { "(no description)" } else { d.message.as_str() }).monospace().size(11.5).color(pal.dim)).wrap());
            ui.label(RichText::new(format!("Action: {}", d.action_id)).monospace().size(11.0).color(pal.dim));
        });
    }
    ui.add_space(12.0);
    for (t, err) in &d.notes {
        ui.label(RichText::new(t.as_str()).size(12.5).color(if *err { pal.red } else { pal.dim }));
    }
    if let Some((prompt, echo, _)) = &d.asking {
        ui.label(RichText::new(format!("{} for {}", prompt.trim_end_matches(':'), d.user)).size(13.0).color(pal.dim));
        ui.add_space(4.0);
        let resp = ui.add(TextEdit::singleline(&mut d.input).password(!*echo).desired_width(f32::INFINITY).margin(vec2(12.0, 9.0)));
        resp.request_focus();
        ui.add_space(14.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let enter = ui.input(|i| i.key_pressed(Key::Enter));
            let go = egui::Button::new(RichText::new("Authenticate").size(14.5).strong().color(pal.on_accent())).fill(pal.accent).corner_radius(CornerRadius::same(8)).min_size(vec2(140.0, 36.0));
            if ui.add(go).clicked() || enter {
                *answer = Some(Some(d.input.clone()));
            }
            if ui.add(egui::Button::new(RichText::new("Cancel").size(14.5)).corner_radius(CornerRadius::same(8)).min_size(vec2(96.0, 36.0))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                *answer = Some(None);
            }
        });
    } else {
        ui.label(RichText::new("Checking...").color(pal.dim));
    }
}

#[cfg(test)]
mod tests {
    use super::describe;

    #[test]
    fn a_marketplace_remove_is_put_in_words() {
        let m = "Authentication is needed to run `/usr/local/bin/labyrinth-pkg-root /run ... 5.cancel pacman -Rs --noconfirm krita' as the super user";
        assert_eq!(describe(m).unwrap(), "The Marketplace wants to remove krita.");
    }

    #[test]
    fn installs_name_their_packages() {
        let m = "Authentication is needed to run `/usr/local/bin/labyrinth-pkg-root /run/user/1000/labyrinth-market/x.cancel pacman -S --needed --noconfirm vlc gimp' as the super user";
        assert_eq!(describe(m).unwrap(), "The Marketplace wants to install or reinstall vlc, gimp.");
    }

    #[test]
    fn other_messages_are_left_to_the_details() {
        assert_eq!(describe("Authentication is needed to change the system clock"), None);
    }
}
