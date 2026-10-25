//! Health: break reminders. `yggdrasil-health` opens the settings window (running it again closes it); `daemon` is the user
//! service that sends the reminders and keeps the top bar's file current.

use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use eframe::egui::{self, vec2, Margin, RichText};
use yggdrasil_health::*;
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme;

const APP_ID: &str = "yggdrasil-health";
const ICON: &str = "/usr/share/yggdrasil/icons/health-symbolic.svg";
const SOUND: &str = "/usr/share/sounds/freedesktop/stereo/message.oga";

/// The session is locked or idle: the screensaver interface (Yggdrasil's lock and most desktops) or logind's hints.
fn away() -> bool {
    let ss = Command::new("busctl")
        .args(["--user", "call", "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver", "GetActive"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().ends_with("true"))
        .unwrap_or(false);
    if ss {
        return true;
    }
    let Ok(id) = std::env::var("XDG_SESSION_ID") else { return false };
    Command::new("loginctl")
        .args(["show-session", &id, "-p", "IdleHint", "-p", "LockedHint", "--value"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == "yes"))
        .unwrap_or(false)
}

/// Sends one reminder: a quiet notification (never waiting on it), and a soft sound if that is on.
fn send(name: &str, sound: bool, seed: i64) {
    let msgs = messages(name);
    let Some((head, body)) = msgs.get(seed.rem_euclid(msgs.len().max(1) as i64) as usize) else { return };
    let _ = Command::new("notify-send").args(["-a", "Health", "-u", "low", "-i", ICON, head, body]).stdin(Stdio::null()).spawn().map(|mut c| {
        std::thread::spawn(move || c.wait());
    });
    if sound && std::path::Path::new(SOUND).is_file() {
        let _ = Command::new("paplay").arg(SOUND).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|mut c| {
            std::thread::spawn(move || c.wait());
        });
    }
}

/// The service: wakes when the next reminder is due (at least once a minute, to pick up changed settings and a return from
/// idle), sends what is due and rewrites the bar's file when it changed.
fn daemon() -> ExitCode {
    loop {
        let cfg = load_config();
        let mut st = load_state();
        let before = st.clone();
        let clock = Clock::now();
        for name in tick(&cfg, &mut st, &clock, &mut away) {
            send(&name, cfg.sound, clock.epoch / 60);
        }
        if st != before {
            let _ = save_state(&st);
        }
        write_bar(&cfg, &st, &clock);
        let wait = next_due(&cfg, &st, &clock).map(|(_, t)| t - clock.epoch).unwrap_or(60).clamp(1, 60);
        std::thread::sleep(Duration::from_secs(wait as u64));
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: yggdrasil-health [daemon | summary | fire STREAM | pause MINUTES|tomorrow | resume]");
    ExitCode::from(2)
}

/// Saves the settings and updates the bar at once (the service follows within a minute).
fn store(cfg: &Config) {
    let _ = save_config(cfg);
    write_bar(cfg, &load_state(), &Clock::now());
}

fn pause(cfg: &mut Config, how: &str) -> bool {
    let c = Clock::now();
    cfg.paused_until = match how {
        "tomorrow" => Some(c.tomorrow()),
        m => match m.parse::<i64>() {
            Ok(m) if m > 0 => Some(c.epoch + m * 60),
            _ => return false,
        },
    };
    true
}

// ---------------------------------------------------------------- the window

struct App {
    cfg: Config,
    /// Text being typed into the time fields, by key; saved once it reads as a time.
    edits: std::collections::HashMap<String, String>,
}

fn when(secs: i64) -> String {
    match secs {
        s if s < 60 => "now".into(),
        s if s < 3600 => format!("in {} min", s / 60),
        s => format!("in {} h {:02} min", s / 3600, s / 60 % 60),
    }
}

impl App {
    fn time_field(&mut self, ui: &mut egui::Ui, key: &str, value: &str) -> Option<String> {
        let buf = self.edits.entry(key.to_string()).or_insert_with(|| value.to_string());
        let ok = parse_hm(buf).is_some();
        let resp = ui.add(egui::TextEdit::singleline(buf).desired_width(52.0).text_color_opt((!ok).then_some(egui::Color32::from_rgb(232, 120, 96))));
        (resp.changed() && ok).then(|| {
            let m = parse_hm(buf).unwrap_or(0);
            format!("{:02}:{:02}", m / 60, m % 60)
        })
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        ctx.request_repaint_after(Duration::from_secs(1));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let before = self.cfg.clone();
        let clock = Clock::now();
        let st = load_state();

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 14, right: 10, top: 6, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Health").strong().color(pal.text));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            ui.add_space(6.0);
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.cfg.reminders, "Reminders");
                    ui.add_space(12.0);
                    ui.add_enabled(self.cfg.reminders, egui::Checkbox::new(&mut self.cfg.sound, "Sound"));
                });

                // what comes next, or why nothing does
                let status = if !self.cfg.reminders {
                    RichText::new("Reminders are off.").color(pal.dim)
                } else if self.cfg.paused(clock.epoch) {
                    let u = self.cfg.paused_until.unwrap_or(0);
                    RichText::new(if u >= clock.tomorrow() { "Paused until tomorrow.".to_string() } else { format!("Paused, back {}.", when(u - clock.epoch)) }).color(pal.amber)
                } else {
                    match next_due(&self.cfg, &st, &clock) {
                        Some((n, t)) => RichText::new(format!("Next: {} {}", label(&n), when(t - clock.epoch))).color(pal.text),
                        None => RichText::new("No reminder is turned on.").color(pal.dim),
                    }
                };
                ui.label(status);

                ui.add_enabled_ui(self.cfg.reminders, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        if self.cfg.paused(clock.epoch) {
                            if ui.button("Resume").clicked() {
                                self.cfg.paused_until = None;
                            }
                        } else {
                            ui.label(RichText::new("Pause").color(pal.dim));
                            for (text, how) in [("15 min", "15"), ("1 hour", "60"), ("2 hours", "120"), ("Until tomorrow", "tomorrow")] {
                                if ui.button(text).clicked() {
                                    pause(&mut self.cfg, how);
                                }
                            }
                        }
                    });

                    ui.separator();
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut self.cfg.quiet.enabled, "Quiet hours");
                        let (from, to) = (self.cfg.quiet.from.clone(), self.cfg.quiet.to.clone());
                        ui.add_enabled_ui(self.cfg.quiet.enabled, |ui| {
                            ui.label(RichText::new("from").color(pal.dim));
                            if let Some(v) = self.time_field(ui, "quiet.from", &from) {
                                self.cfg.quiet.from = v;
                            }
                            ui.label(RichText::new("to").color(pal.dim));
                            if let Some(v) = self.time_field(ui, "quiet.to", &to) {
                                self.cfg.quiet.to = v;
                            }
                        });
                    });
                    ui.separator();

                    for name in STREAMS {
                        let mut s = self.cfg.stream(name);
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut s.enabled, RichText::new(label(name)).strong());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button("Try").on_hover_text("Send this reminder now").clicked() {
                                    send(name, self.cfg.sound, clock.epoch);
                                }
                            });
                        });
                        ui.label(RichText::new(about(name)).color(pal.dim).size(12.0));
                        ui.add_enabled_ui(s.enabled, |ui| {
                            if let Some(at) = s.at.clone() {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("at").color(pal.dim));
                                    if let Some(v) = self.time_field(ui, name, &at) {
                                        s.at = Some(v);
                                    }
                                });
                            } else if let Some(mut every) = s.every {
                                let max = if name == "eyeexercise" { 720 } else { 240 };
                                ui.add(egui::Slider::new(&mut every, 5..=max).suffix(" min").step_by(5.0));
                                s.every = Some(every);
                            }
                        });
                        if s != self.cfg.stream(name) {
                            self.cfg.streams.insert(name.to_string(), s);
                        }
                        ui.add_space(4.0);
                    }
                    ui.label(
                        RichText::new("Health only sends reminders. It does not prescribe exercises: how and how often is up to you and your doctor or eye-care professional.")
                            .color(pal.dim)
                            .size(11.5),
                    );
                });
            });
        });
        chrome::outline(&ctx, &pal);

        if self.cfg != before {
            store(&self.cfg);
        }
    }
}

fn window() -> ExitCode {
    if yggdrasil_ui::single::toggle(APP_ID) {
        return ExitCode::SUCCESS;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([420.0, 640.0])
            .with_min_inner_size([360.0, 360.0])
            .with_title("Health")
            .with_app_id(APP_ID)
            .with_decorations(false),
        ..Default::default()
    };
    let app = App { cfg: load_config(), edits: Default::default() };
    match eframe::run_native("Health", options, Box::new(|_| Ok(Box::new(app)))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yggdrasil-health: {e}");
            ExitCode::from(1)
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => window(),
        Some("daemon") => daemon(),
        Some("summary") => {
            let (cfg, st, c) = (load_config(), load_state(), Clock::now());
            let next = next_due(&cfg, &st, &c).map(|(n, t)| serde_json::json!({"stream": n, "secs": t - c.epoch}));
            println!("{}", serde_json::json!({"config": cfg, "next": next}));
            ExitCode::SUCCESS
        }
        Some("fire") => match args.get(1).filter(|n| STREAMS.contains(&n.as_str())) {
            Some(n) => {
                send(n, load_config().sound, Clock::now().epoch);
                // let the notification start before exiting
                std::thread::sleep(Duration::from_millis(300));
                ExitCode::SUCCESS
            }
            None => usage(),
        },
        Some("pause") => {
            let mut cfg = load_config();
            if !args.get(1).is_some_and(|h| pause(&mut cfg, h)) {
                return usage();
            }
            store(&cfg);
            ExitCode::SUCCESS
        }
        Some("resume") => {
            let mut cfg = load_config();
            cfg.paused_until = None;
            store(&cfg);
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
