//! The one question: the keyring still has your previous password.

use std::sync::mpsc::{channel, Receiver, Sender};

use eframe::egui::{self, Key, RichText, TextEdit};
use labyrinth_ui::theme;
use zeroize::Zeroize;

use crate::{auth, secret};

enum Msg {
    Done(Result<String, String>),
}

#[derive(PartialEq, Eq)]
enum Screen {
    Form,
    ConfirmFresh,
    Finished,
}

pub struct App {
    screen: Screen,
    previous: String,
    current: String,
    error: String,
    info: String,
    busy: bool,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
}

impl App {
    pub fn new(ctx: &egui::Context) -> App {
        theme::apply(ctx, theme::system_dark(), theme::system_accent());
        let (tx, rx) = channel();
        App { screen: Screen::Form, previous: String::new(), current: String::new(), error: String::new(), info: String::new(), busy: false, tx, rx }
    }

    fn user() -> String {
        std::env::var("USER").unwrap_or_default()
    }

    /// Check the current login password with PAM, then act. Runs on a worker thread.
    fn run(&mut self, ctx: &egui::Context, fresh: bool) {
        if self.busy {
            return;
        }
        self.busy = true;
        self.error.clear();
        let (mut prev, mut cur) = (std::mem::take(&mut self.previous), std::mem::take(&mut self.current));
        let tx = self.tx.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let result = (|| -> Result<String, String> {
                let pam = auth::Pam { service: "login".into() };
                use auth::Authenticator;
                if !pam.check(&App::user(), &cur)? {
                    return Err("That is not your current login password.".into());
                }
                if fresh {
                    let moved = secret::start_fresh(&cur)?;
                    return Ok(format!("A new, empty keyring is ready. The old one was kept as {moved}."));
                }
                let kr = secret::Keyring::open().map_err(|_| "The keyring service is not running.".to_string())?;
                kr.change_password(&prev, &cur).map_err(|e| if e.contains("previous") { "That is not the previous password the keyring uses.".to_string() } else { e })?;
                Ok("Done. Your keyring now uses your current login password.".into())
            })();
            prev.zeroize();
            cur.zeroize();
            let _ = tx.send(Msg::Done(result));
            ctx.request_repaint();
        });
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        while let Ok(Msg::Done(r)) = self.rx.try_recv() {
            self.busy = false;
            match r {
                Ok(text) => {
                    self.info = text;
                    self.screen = Screen::Finished;
                }
                Err(e) => {
                    self.error = e;
                    if self.screen == Screen::ConfirmFresh {
                        self.screen = Screen::Form;
                    }
                }
            }
        }
        let pal = theme::palette(theme::system_dark());
        ui.add_space(8.0);
        match self.screen {
            Screen::Finished => {
                ui.label(RichText::new("Keyring").size(18.0).strong());
                ui.add_space(8.0);
                ui.label(self.info.as_str());
                ui.add_space(14.0);
                if ui.button("Close").clicked() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            Screen::ConfirmFresh => {
                ui.label(RichText::new("Start a new keyring?").size(18.0).strong());
                ui.add_space(8.0);
                ui.label("Passwords and keys saved in the old keyring (browser sign-ins, saved Wi-Fi passwords, app tokens) will not be available any more. The old keyring file is kept, not deleted, so it can be opened later if you remember the password.");
                ui.add_space(8.0);
                ui.label(RichText::new("Your current login password").color(pal.dim));
                ui.add(TextEdit::singleline(&mut self.current).password(true).desired_width(f32::INFINITY));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.busy && !self.current.is_empty(), egui::Button::new("Start a new keyring")).clicked() {
                        self.run(&ctx, true);
                    }
                    if ui.button("Back").clicked() {
                        self.screen = Screen::Form;
                    }
                });
            }
            Screen::Form => {
                ui.label(RichText::new("Your keyring needs one thing").size(18.0).strong());
                ui.add_space(8.0);
                ui.label("The keyring that holds saved passwords and keys for your apps is still locked with your previous login password. Enter it once, together with your current password, and it will follow your current password from now on.");
                ui.add_space(10.0);
                ui.label(RichText::new("Previous password").color(pal.dim));
                ui.add(TextEdit::singleline(&mut self.previous).password(true).desired_width(f32::INFINITY));
                ui.add_space(6.0);
                ui.label(RichText::new("Current login password").color(pal.dim));
                let r = ui.add(TextEdit::singleline(&mut self.current).password(true).desired_width(f32::INFINITY));
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                if !self.error.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new(self.error.as_str()).color(pal.red));
                }
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    let ready = !self.busy && !self.previous.is_empty() && !self.current.is_empty();
                    if ui.add_enabled(ready, egui::Button::new(if self.busy { "Working…" } else { "Update keyring" })).clicked() || (enter && ready) {
                        self.run(&ctx, false);
                    }
                    if ui.button("I don't remember it").clicked() {
                        self.error.clear();
                        self.screen = Screen::ConfirmFresh;
                    }
                    if ui.button("Not now").clicked() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                });
            }
        }
    }
}
