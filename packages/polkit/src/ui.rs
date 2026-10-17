//! The password box: a small window in Labyrinth's look. It stays hidden until polkit asks for something.

use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Align, Key, Layout, RichText, TextEdit};
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
                    self.dialog = Some(Dialog { message, action_id, user, asking: None, input: String::new(), notes: vec![] });
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

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        self.receive();
        let pal = theme::palette(theme::system_dark());
        let Some(d) = &mut self.dialog else {
            // the request is over: this process only exists for one request, so it ends and the window goes with it
            if self.started {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            return;
        };
        self.started = true;
        let mut answer: Option<Option<String>> = None;
        egui::CentralPanel::default().show(ui, |ui| draw(ui, d, &pal, &mut answer));
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

fn draw(ui: &mut egui::Ui, d: &mut Dialog, pal: &theme::Palette, answer: &mut Option<Option<String>>) {
    if ui.input(|i| i.viewport().close_requested()) {
        *answer = Some(None);
    }
    ui.vertical(|ui| {
        ui.add_space(6.0);
        ui.label(RichText::new("Authentication required").size(17.0).strong());
        ui.add_space(6.0);
        ui.label(RichText::new(if d.message.is_empty() { "A program is asking for administrator rights." } else { d.message.as_str() }).color(pal.text));
        ui.label(RichText::new(format!("Action: {}", d.action_id)).small().color(pal.dim));
        ui.add_space(10.0);
        for (t, err) in &d.notes {
            ui.label(RichText::new(t.as_str()).small().color(if *err { pal.red } else { pal.dim }));
        }
        if let Some((prompt, echo, _)) = &d.asking {
            ui.label(RichText::new(format!("{} ({})", prompt.trim_end_matches(':'), d.user)).color(pal.dim));
            let resp = ui.add(TextEdit::singleline(&mut d.input).password(!*echo).desired_width(f32::INFINITY));
            resp.request_focus();
            ui.add_space(10.0);
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let enter = ui.input(|i| i.key_pressed(Key::Enter));
                if ui.button("Authenticate").clicked() || enter {
                    *answer = Some(Some(d.input.clone()));
                }
                if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    *answer = Some(None);
                }
            });
        } else {
            ui.label(RichText::new("Checking…").color(pal.dim));
        }
    });
}
