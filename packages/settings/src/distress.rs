//! Settings > Security > Distress password. The page talks to the root helper `labyrinth-distress` (through pkexec); nothing is
//! kept on the page's side except what is being edited, and nothing is sent until Save. The account password is held only while
//! the page is open and is wiped when it closes.

use crate::ui;
use eframe::egui::{self, vec2, RichText};
use labyrinth_ui::theme::Palette;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::mpsc::{channel, Receiver};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Cfg {
    pub enabled: bool,
    pub hash: String,
    pub count: u32,
    pub window_secs: u32,
    pub expires: i64,
    pub silent_login: bool,
    pub decoy: bool,
    pub in_prompts: bool,
    pub extra: String,
    pub commands: Vec<String>,
    pub wipe_keys: bool,
    pub shutdown: bool,
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg { enabled: false, hash: String::new(), count: 1, window_secs: 60, expires: 0, silent_login: true, decoy: false, in_prompts: false, extra: "none".into(), commands: vec![], wipe_keys: true, shutdown: true }
    }
}

#[derive(Default)]
pub struct State {
    /// The account password typed to open the page, then kept (in memory only) while it is open.
    gate: String,
    unlocked: Option<String>,
    cfg: Cfg,
    saved: Cfg,
    events: Vec<Value>,
    new_pw: String,
    new_pw2: String,
    /// Valid for: 0 keep as it is, 1 forever, 2 a day, 3 a week, 4 a month.
    valid: usize,
    commands_text: String,
    /// Turning it on asks "are you sure" and the password.
    confirming_enable: bool,
    enable_pw: String,
    /// Saving section 2 asks again.
    confirming_save: bool,
    second_pw: String,
    plan: Option<Vec<String>>,
    message: String,
    busy: bool,
    rx: Option<Receiver<(String, Value)>>,
}

const EXTRAS: [(&str, &str); 5] = [("none", "None"), ("commands", "Run commands"), ("wipe_keys", "Wipe keys"), ("factory_reset", "Factory reset"), ("wipe_system", "Wipe system")];

fn call(cmd: &'static str, req: Value) -> Receiver<(String, Value)> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        use std::io::Write;
        let out = (|| -> Result<Value, String> {
            let mut child = std::process::Command::new("pkexec").arg("/usr/bin/labyrinth-distress").arg(cmd).stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().map_err(|_| "pkexec is not available".to_string())?;
            child.stdin.take().ok_or("no input")?.write_all(req.to_string().as_bytes()).map_err(|e| e.to_string())?;
            let o = child.wait_with_output().map_err(|e| e.to_string())?;
            serde_json::from_slice(&o.stdout).map_err(|_| "The distress password helper is not installed or did not answer".to_string())
        })()
        .unwrap_or_else(|e| json!({"ok": false, "error": e}));
        let _ = tx.send((cmd.to_string(), out));
    });
    rx
}

fn wipe(s: &mut String) {
    // overwrite before dropping
    unsafe { s.as_bytes_mut().iter_mut().for_each(|b| std::ptr::write_volatile(b, 0)) };
    s.clear();
}

impl State {
    /// Called when the page is left: nothing secret stays in memory.
    pub fn close(&mut self) {
        for s in [&mut self.gate, &mut self.new_pw, &mut self.new_pw2, &mut self.enable_pw, &mut self.second_pw] {
            wipe(s);
        }
        if let Some(mut p) = self.unlocked.take() {
            wipe(&mut p);
        }
        *self = State::default();
    }

    fn poll(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.rx else { return };
        match rx.try_recv() {
            Ok((cmd, v)) => {
                self.rx = None;
                self.busy = false;
                let ok = v["ok"].as_bool().unwrap_or(false);
                if !ok {
                    self.message = v["error"].as_str().unwrap_or("That did not work").to_string();
                    if cmd == "open" {
                        wipe(&mut self.gate);
                    }
                    return;
                }
                match cmd.as_str() {
                    "open" => {
                        self.cfg = serde_json::from_value(v["config"].clone()).unwrap_or_default();
                        self.saved = self.cfg.clone();
                        self.events = v["events"].as_array().cloned().unwrap_or_default();
                        self.commands_text = self.cfg.commands.join("\n");
                        self.unlocked = Some(std::mem::take(&mut self.gate));
                        self.message.clear();
                    }
                    "save" => {
                        self.cfg = serde_json::from_value(v["config"].clone()).unwrap_or_default();
                        self.saved = self.cfg.clone();
                        self.valid = 0;
                        wipe(&mut self.new_pw);
                        wipe(&mut self.new_pw2);
                        wipe(&mut self.second_pw);
                        self.confirming_save = false;
                        self.message = "Saved".into();
                    }
                    "plan" => self.plan = Some(v["steps"].as_array().map(|a| a.iter().filter_map(|s| s.as_str().map(String::from)).collect()).unwrap_or_default()),
                    _ => {}
                }
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
            Err(_) => {
                self.rx = None;
                self.busy = false;
            }
        }
    }

    fn start(&mut self, cmd: &'static str, req: Value) {
        self.busy = true;
        self.message.clear();
        self.rx = Some(call(cmd, req));
    }

    pub fn page(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        self.poll(ctx);
        if self.unlocked.is_none() {
            return self.gate_page(ui, pal);
        }
        let mut dirty = self.cfg != self.saved || self.valid != 0 || !self.new_pw.is_empty();
        dirty |= self.commands_text.lines().map(str::to_string).collect::<Vec<_>>() != self.saved.commands && !(self.commands_text.is_empty() && self.saved.commands.is_empty());

        ui::group(ui, pal, None, |ui| {
            ui::row(ui, pal, true, "Use a distress password", Some("A second password. Typed where your real one is asked for, it does something else instead. Nothing here is saved until you press Save"), |ui| {
                let mut on = self.cfg.enabled;
                if ui::switch(ui, pal, &mut on).changed() {
                    if on {
                        self.confirming_enable = true;
                    } else {
                        self.cfg.enabled = false;
                    }
                }
            });
        });
        if self.confirming_enable {
            ui.add_space(8.0);
            ui::group(ui, pal, Some("Turn on the distress password?"), |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new("When someone types the distress password, Labyrinth does what you choose below, and it does not tell them. Make sure you can remember both passwords and that you choose what it does carefully. Type your account password to continue.").size(12.5).color(pal.dim));
                ui.add_space(6.0);
                ui.add(egui::TextEdit::singleline(&mut self.enable_pw).password(true).hint_text("Your account password").desired_width(280.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui::action(ui, pal, "Yes, turn it on").clicked() {
                        // Checked for real by the helper at Save; here it must at least match the password that opened the page.
                        if Some(&self.enable_pw) == self.unlocked.as_ref() {
                            self.cfg.enabled = true;
                            self.confirming_enable = false;
                            wipe(&mut self.enable_pw);
                            self.message.clear();
                        } else {
                            self.message = "That is not your password".into();
                        }
                    }
                    if ui::quiet(ui, pal, "Cancel").clicked() {
                        self.confirming_enable = false;
                        wipe(&mut self.enable_pw);
                    }
                });
                ui.add_space(6.0);
            });
        }
        if self.cfg.enabled && !self.confirming_enable {
            self.body(ui, pal);
        }
        if !self.message.is_empty() {
            ui.add_space(8.0);
            ui.label(RichText::new(&self.message).color(if self.message == "Saved" { pal.green } else { pal.red }));
        }
        let has_extra = self.cfg.enabled && self.cfg.extra != "none";
        if self.confirming_save {
            ui.add_space(8.0);
            ui::group(ui, pal, Some("Save the second section?"), |ui| {
                ui.add_space(6.0);
                ui.label(RichText::new(self.warning()).size(13.0).color(pal.red));
                ui.add_space(6.0);
                ui.add(egui::TextEdit::singleline(&mut self.second_pw).password(true).hint_text("Your account password, once more").desired_width(280.0));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.busy && !self.second_pw.is_empty(), egui::Button::new(RichText::new("I understand. Save").color(pal.on_accent())).fill(pal.red)).clicked() {
                        self.send_save();
                    }
                    if ui::quiet(ui, pal, "Cancel").clicked() {
                        self.confirming_save = false;
                        wipe(&mut self.second_pw);
                    }
                });
                ui.add_space(6.0);
            });
        } else if self.unlocked.is_some() && !self.confirming_enable {
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(dirty && !self.busy, egui::Button::new(RichText::new("Save").color(pal.on_accent())).fill(pal.accent).min_size(vec2(90.0, 30.0))).clicked() {
                    if has_extra {
                        self.confirming_save = true;
                    } else {
                        self.send_save();
                    }
                }
                if ui.add_enabled(dirty, egui::Button::new("Discard changes").min_size(vec2(90.0, 30.0))).clicked() {
                    self.cfg = self.saved.clone();
                    self.commands_text = self.cfg.commands.join("\n");
                    self.valid = 0;
                    wipe(&mut self.new_pw);
                    wipe(&mut self.new_pw2);
                    self.message.clear();
                }
                if self.busy {
                    ui.spinner();
                }
            });
        }
        self.log_group(ui, pal);
    }

    fn warning(&self) -> String {
        match self.cfg.extra.as_str() {
            "commands" => "When the distress password is used, the commands you wrote run as the administrator, with nobody asking. A mistake in one can break this computer.".into(),
            "wipe_keys" => "When the distress password is used, the keys that open your encrypted disk are erased. If you chose this, the data on that disk can NEVER be opened again, by anyone, including you.".into(),
            "factory_reset" => "When the distress password is used, every account and everything in it is deleted and the computer is reset to a fresh start. This cannot be undone.".into(),
            _ => "When the distress password is used, every internal disk is wiped and the computer shuts down. There will be no operating system left and nothing can be recovered.".into(),
        }
    }

    fn send_save(&mut self) {
        let mut cfg = self.cfg.clone();
        cfg.commands = self.commands_text.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        if self.new_pw != self.new_pw2 {
            self.message = "The two distress passwords are not the same".into();
            return;
        }
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        match self.valid {
            1 => cfg.expires = 0,
            2 => cfg.expires = now + 86_400,
            3 => cfg.expires = now + 7 * 86_400,
            4 => cfg.expires = now + 30 * 86_400,
            _ => {}
        }
        let req = json!({"password": self.unlocked.clone().unwrap_or_default(), "second_password": self.second_pw, "config": cfg, "new_password": self.new_pw});
        self.start("save", req);
    }

    fn gate_page(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui::group(ui, pal, None, |ui| {
            ui.add_space(6.0);
            ui.label(RichText::new("Type your account password to open this page.").size(13.0));
            ui.add_space(6.0);
            let r = ui.add(egui::TextEdit::singleline(&mut self.gate).password(true).hint_text("Your account password").desired_width(280.0));
            let go = ui.add_enabled(!self.busy && !self.gate.is_empty(), egui::Button::new(RichText::new("Open").color(pal.on_accent())).fill(pal.accent).min_size(vec2(90.0, 30.0))).clicked() || (r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !self.gate.is_empty());
            if go && !self.busy {
                let req = json!({"password": self.gate});
                self.start("open", req);
            }
            if self.busy {
                ui.spinner();
            }
            if !self.message.is_empty() {
                ui.label(RichText::new(&self.message).color(pal.red));
            }
            ui.add_space(6.0);
        });
    }

    fn body(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        ui.add_space(8.0);
        ui::group(ui, pal, Some("The distress password"), |ui| {
            let sub = if self.saved.hash.is_empty() { "None set yet" } else { "One is set. Type a new one below to replace it" };
            ui::row(ui, pal, true, "New distress password", Some(sub), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_pw).password(true).desired_width(200.0));
            });
            ui::row(ui, pal, false, "Type it again", Some("At least 6 characters, and different from your real password"), |ui| {
                ui.add(egui::TextEdit::singleline(&mut self.new_pw2).password(true).desired_width(200.0));
            });
        });
        ui.add_space(8.0);
        ui::group(ui, pal, Some("When it counts"), |ui| {
            ui::row(ui, pal, true, "How many times", Some("Typed once, or several times within a time. Fewer than that looks exactly like a wrong password"), |ui| {
                let mut multi = self.cfg.count > 1;
                if ui::segmented(ui, pal, &["Once", "Several times"], multi as usize).is_some() {
                    multi = !multi;
                    self.cfg.count = if multi { 3 } else { 1 };
                }
            });
            if self.cfg.count > 1 {
                ui::row(ui, pal, false, "Entries needed", None, |ui| {
                    let mut v = self.cfg.count as f32;
                    if ui::slider_range(ui, pal, &mut v, 2.0..=10.0, "").changed() {
                        self.cfg.count = v as u32;
                    }
                });
                ui::row(ui, pal, false, "Within", Some("Seconds"), |ui| {
                    let mut v = self.cfg.window_secs as f32;
                    if ui::slider_range(ui, pal, &mut v, 10.0..=600.0, " s").changed() {
                        self.cfg.window_secs = v as u32;
                    }
                });
            }
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let cur = if self.saved.expires == 0 { "Now: forever".to_string() } else if self.saved.expires > now { format!("Now: another {}", crate::security::ago(now - (self.saved.expires - now), now).replace(" ago", "")) } else { "Now: it has expired".to_string() };
            ui::row(ui, pal, false, "How long it works", Some(&cur), |ui| {
                egui::ComboBox::from_id_salt("distress-valid").selected_text(["Keep as it is", "Forever", "One day", "One week", "One month"][self.valid]).show_ui(ui, |ui| {
                    for (i, t) in ["Keep as it is", "Forever", "One day", "One week", "One month"].iter().enumerate() {
                        ui.selectable_value(&mut self.valid, i, *t);
                    }
                });
            });
        });
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Section 1 · Saving needs no extra password"), |ui| {
            let mut pick = if self.cfg.silent_login { 1 } else if self.cfg.decoy { 2 } else { 0 };
            ui::row(ui, pal, true, "What it does", Some("Silent login: you are let in as normal while Heimdall raises a hidden alert. Decoy session: your real account is locked and a decoy one opens"), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &["Nothing", "Silent login", "Decoy session"], pick) {
                    pick = i;
                }
            });
            self.cfg.silent_login = pick == 1;
            self.cfg.decoy = pick == 2;
            if self.cfg.decoy {
                ui::row(ui, pal, false, "About the decoy session", Some("Not available yet: it needs Labyrinth's own session manager. Until then choosing it changes nothing (your account is never locked)"), |_| {});
            }
            ui::row(ui, pal, false, "Also in terminals and other password prompts", Some("Not available yet: it needs Labyrinth's password module. Today it works on Labyrinth's own lock screen"), |ui| {
                let mut off = false;
                ui.add_enabled_ui(false, |ui| ui::switch(ui, pal, &mut off));
            });
        });
        ui.add_space(8.0);
        ui::group(ui, pal, Some("Section 2 · Saving asks for your password again"), |ui| {
            let cur = EXTRAS.iter().position(|(k, _)| *k == self.cfg.extra).unwrap_or(0);
            let labels: Vec<&str> = EXTRAS.iter().map(|(_, l)| *l).collect();
            ui::row(ui, pal, true, "Also do this", Some("Works with Section 1, or on its own. With Nothing chosen above, this is all it does"), |ui| {
                egui::ComboBox::from_id_salt("distress-extra").selected_text(labels[cur]).show_ui(ui, |ui| {
                    for (i, (k, l)) in EXTRAS.iter().enumerate() {
                        if ui.selectable_label(i == cur, *l).clicked() {
                            self.cfg.extra = k.to_string();
                            self.plan = None;
                        }
                    }
                });
            });
            match self.cfg.extra.as_str() {
                "commands" => {
                    ui::row(ui, pal, false, "Commands", Some("One per line, run in order as the administrator"), |_| {});
                    ui.add(egui::TextEdit::multiline(&mut self.commands_text).desired_rows(4).desired_width(f32::INFINITY).code_editor());
                    ui.add_space(6.0);
                }
                "wipe_keys" => {
                    ui::row(ui, pal, false, "Wipe the keys", Some("Erase the disk encryption keys and your SSH, GPG and keyring files"), |ui| {
                        ui::switch(ui, pal, &mut self.cfg.wipe_keys);
                    });
                    ui::row(ui, pal, false, "Shut down", None, |ui| {
                        ui::switch(ui, pal, &mut self.cfg.shutdown);
                    });
                }
                "factory_reset" => {
                    ui::row(ui, pal, false, "Factory reset", Some("Every account and its files are removed, settings are reset and the computer restarts into first-time setup"), |_| {});
                }
                "wipe_system" => {
                    ui::row(ui, pal, false, "Wipe the whole system", Some("The partition tables and encryption keys of every internal disk are destroyed and the computer powers off. No operating system is left"), |_| {});
                }
                _ => {}
            }
            if self.cfg.extra != "none" {
                ui::row(ui, pal, false, "See exactly what it would do on this computer", None, |ui| {
                    if ui::quiet(ui, pal, "Show").clicked() {
                        let mut c = self.cfg.clone();
                        c.commands = self.commands_text.lines().map(str::to_string).collect();
                        self.start("plan", json!({"config": c}));
                    }
                });
                if let Some(p) = &self.plan {
                    for l in p {
                        ui.label(RichText::new(format!("· {l}")).size(12.0).color(pal.dim));
                    }
                    if p.is_empty() {
                        ui.label(RichText::new("Nothing").size(12.0).color(pal.dim));
                    }
                    ui.add_space(4.0);
                }
            }
        });
    }

    fn log_group(&self, ui: &mut egui::Ui, pal: &Palette) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        ui.add_space(8.0);
        ui::group(ui, pal, Some("What it has done"), |ui| {
            if self.events.is_empty() {
                ui::row(ui, pal, true, "Nothing yet", Some("Changes and every time it was used are listed here"), |_| {});
            }
            for (i, e) in self.events.iter().take(30).enumerate() {
                let d = e["detail"].as_str().unwrap_or("");
                ui::row(ui, pal, i == 0, d, Some(&format!("{} · {}", e["kind"].as_str().unwrap_or(""), crate::security::ago(e["time"].as_i64().unwrap_or(0), now))), |_| {});
            }
        });
    }
}
