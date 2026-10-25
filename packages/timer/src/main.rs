//! yggdrasil-timer: countdown timers for Yggdrasil's top bar.
//!
//!   yggdrasil-timer                    the window (the pill in the bar opens it; a second click closes it)
//!   yggdrasil-timer start DUR [LABEL]  start a timer ("7" minutes, "1:30", "90s", "1h30m")
//!   yggdrasil-timer ring ID GEN        run by the systemd timer set at start: marks it done, notifies, plays a sound
//!   yggdrasil-timer reconcile          at login: finished timers are marked done, running ones get their alarm back
//!
//! The pill is the panel's bar add-on reading ~/.local/state/yggdrasil/timer/bar.json (timer.conf), so nothing stays running.

mod timers;

use std::process::ExitCode;
use std::time::{Duration, SystemTime};

use eframe::egui::{self, vec2, Align, Color32, CornerRadius, FontId, Layout, Margin, RichText};
use timers::{State, Store, Timer};
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

const PRESETS: [(&str, i64); 7] = [("1 min", 60), ("5 min", 300), ("10 min", 600), ("15 min", 900), ("25 min", 1500), ("45 min", 2700), ("1 h", 3600)];

fn store() -> Store {
    Store::new(timers::dir())
}

/// Finished timers whose alarm was lost (the computer was off) are marked done; running ones whose alarm is gone get it back.
fn reconcile(s: &Store) {
    let now = timers::now();
    // a few seconds of grace, so an alarm that is firing right now still rings
    let Ok((st, _)) = s.change(|st| timers::expire(st, now - 5)) else { return };
    for t in st.timers.iter().filter(|t| t.running()) {
        if !timers::armed(t) {
            timers::arm(t, now);
        }
    }
}

fn ring(s: &Store, id: u64, generation: u32) -> ExitCode {
    let now = timers::now();
    let r = s.change(|st| {
        let t = st.timers.iter_mut().find(|t| t.id == id && t.generation == generation && t.running() && t.until.unwrap_or(0) <= now + 2)?;
        t.done = true;
        Some(t.clone())
    });
    if let Ok((_, Some(t))) = r {
        timers::alert(&t);
    }
    ExitCode::SUCCESS
}

/// Changes one timer and moves its alarm to match.
fn apply(s: &Store, id: u64, change: impl FnOnce(&mut State, i64) -> Option<Timer>) -> Option<State> {
    let now = timers::now();
    let (st, (old, new)) = s
        .change(|st| {
            let old = st.timers.iter().find(|t| t.id == id).cloned();
            (old, change(st, now))
        })
        .ok()?;
    let still = |o: &Timer| new.as_ref().is_some_and(|n| st.timers.iter().any(|t| t.id == n.id) && n.running() && n.generation == o.generation);
    if let Some(o) = old.as_ref().filter(|o| o.running() && !still(o)) {
        timers::disarm(o);
    }
    if let Some(n) = new.as_ref().filter(|n| n.running() && st.timers.iter().any(|t| t.id == n.id)) {
        if old.as_ref().is_none_or(|o| !o.running() || o.generation != n.generation) {
            timers::arm(n, now);
        }
    }
    Some(st)
}

fn start(s: &Store, secs: i64, label: &str) -> Option<State> {
    let now = timers::now();
    let (st, t) = s.change(|st| timers::start(st, secs, label, now)).ok()?;
    timers::arm(&t, now);
    Some(st)
}

struct App {
    store: Store,
    st: State,
    stamp: Option<SystemTime>,
    input: String,
    label: String,
    note: String,
    focused: bool,
}

impl App {
    fn new() -> App {
        let store = store();
        reconcile(&store);
        let st = store.load();
        App { stamp: Self::stamp_of(&store), store, st, input: String::new(), label: String::new(), note: String::new(), focused: false }
    }

    fn stamp_of(s: &Store) -> Option<SystemTime> {
        std::fs::metadata(s.state_path()).and_then(|m| m.modified()).ok()
    }

    /// An alarm (or `start` from a terminal) may have changed the timers while the window is open.
    fn refresh(&mut self) {
        let stamp = Self::stamp_of(&self.store);
        if stamp != self.stamp {
            self.stamp = stamp;
            self.st = self.store.load();
        }
    }

    fn took(&mut self, st: Option<State>) {
        match st {
            Some(st) => {
                self.st = st;
                self.stamp = Self::stamp_of(&self.store);
            }
            None => self.note = format!("Could not save the timers in {}.", self.store.dir.display()),
        }
    }

    fn start_input(&mut self) {
        match timers::parse_duration(&self.input) {
            Some(secs) => {
                let label = std::mem::take(&mut self.label);
                let st = start(&self.store, secs, &label);
                self.took(st);
                self.input.clear();
                self.note.clear();
            }
            None if self.input.trim().is_empty() => self.note = "Type a length, or pick one above.".into(),
            None => self.note = "Try 7 (minutes), 1:30, 90s or 1h30m.".into(),
        }
    }
}

fn small(ui: &mut egui::Ui, pal: &Palette, text: &str) -> bool {
    ui.add(egui::Button::new(RichText::new(text).size(12.5).color(pal.text)).min_size(vec2(0.0, 26.0))).clicked()
}

enum Do {
    Toggle(u64),
    Add(u64),
    Restart(u64),
    Remove(u64),
}

fn card(ui: &mut egui::Ui, pal: &Palette, t: &Timer, now: i64) -> Option<Do> {
    let mut out = None;
    let (fill, colour, status) = if t.done {
        (pal.red.gamma_multiply(0.18), pal.red, "Done")
    } else if t.paused() {
        (pal.field, pal.amber, "Paused")
    } else {
        (pal.field, pal.text, "")
    };
    egui::Frame::new().fill(fill).corner_radius(CornerRadius::same(8)).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                let sub = if status.is_empty() { timers::name(t) } else { format!("{} · {status}", timers::name(t)) };
                ui.label(RichText::new(sub).size(12.0).color(pal.dim));
                ui.label(RichText::new(timers::clock(t.remaining(now))).font(FontId::monospace(26.0)).color(colour));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if t.done {
                    if small(ui, pal, "Dismiss") {
                        out = Some(Do::Remove(t.id));
                    }
                    if small(ui, pal, "Again") {
                        out = Some(Do::Restart(t.id));
                    }
                } else {
                    if small(ui, pal, "Cancel") {
                        out = Some(Do::Remove(t.id));
                    }
                    if small(ui, pal, "+1 min") {
                        out = Some(Do::Add(t.id));
                    }
                    if small(ui, pal, if t.paused() { "Resume" } else { "Pause" }) {
                        out = Some(Do::Toggle(t.id));
                    }
                }
            });
        });
    });
    out
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        self.refresh();
        ctx.request_repaint_after(Duration::from_millis(if self.st.timers.iter().any(|t| t.running()) { 250 } else { 1000 }));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let now = timers::now();
        let mut todo: Option<Do> = None;

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 12, right: 10, top: 6, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Timer").strong().color(pal.text));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            ui.add_space(6.0);

            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (name, secs) in PRESETS {
                    if ui.add(egui::Button::new(RichText::new(name).size(13.0)).min_size(vec2(0.0, 28.0))).clicked() {
                        let label = std::mem::take(&mut self.label);
                        let st = start(&self.store, secs, &label);
                        self.took(st);
                        self.note.clear();
                    }
                }
            });
            ui.add_space(8.0);
            let mut go = false;
            ui.horizontal(|ui| {
                let d = ui.add(egui::TextEdit::singleline(&mut self.input).hint_text("7, 1:30, 90s, 1h30m").desired_width(118.0).margin(vec2(8.0, 6.0)));
                if !self.focused {
                    d.request_focus();
                    self.focused = true;
                }
                let w = ui.available_width() - 64.0;
                let l = ui.add(egui::TextEdit::singleline(&mut self.label).hint_text("Label (optional)").desired_width(w.max(60.0)).margin(vec2(8.0, 6.0)));
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                go = (d.lost_focus() || l.lost_focus()) && enter;
                if ui.add(egui::Button::new(RichText::new("Start").color(Color32::WHITE)).fill(pal.accent).min_size(vec2(56.0, 30.0))).clicked() {
                    go = true;
                }
            });
            if go {
                self.start_input();
            }
            if !self.note.is_empty() {
                ui.add_space(4.0);
                ui.label(RichText::new(self.note.as_str()).color(pal.amber).size(12.0));
            }
            ui.add_space(10.0);

            let mut shown: Vec<&Timer> = self.st.timers.iter().collect();
            // finished first, then the one ending soonest, paused last
            shown.sort_by_key(|t| (!t.done, t.paused(), t.until.unwrap_or(i64::MAX), t.id));
            let footer_h = 22.0;
            egui::ScrollArea::vertical().max_height(ui.available_height() - footer_h).auto_shrink([false, false]).show(ui, |ui| {
                if shown.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| ui.label(RichText::new("No timers. Pick a length above.").color(pal.dim)));
                }
                for t in shown {
                    if let Some(d) = card(ui, &pal, t, now) {
                        todo = Some(d);
                    }
                    ui.add_space(6.0);
                }
            });
            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.label(RichText::new("Timers ring with this window closed · Esc closes").color(pal.dim).size(11.5));
            });
        });
        chrome::outline(&ctx, &pal);

        if let Some(d) = todo {
            let st = match d {
                Do::Toggle(id) => apply(&self.store, id, |st, now| timers::toggle(st, id, now)),
                Do::Add(id) => apply(&self.store, id, |st, now| timers::add(st, id, 60, now)),
                Do::Restart(id) => apply(&self.store, id, |st, now| timers::restart(st, id, now)),
                Do::Remove(id) => apply(&self.store, id, |st, _| {
                    timers::remove(st, id);
                    None
                }),
            };
            self.took(st);
        }
    }
}

fn window() -> ExitCode {
    // one window at a time: a second click on the pill closes it
    if yggdrasil_ui::single::toggle("yggdrasil-timer") {
        return ExitCode::SUCCESS;
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([400.0, 460.0])
            .with_min_inner_size([340.0, 300.0])
            .with_title("Timer")
            .with_app_id("yggdrasil-timer")
            .with_decorations(false),
        ..Default::default()
    };
    match eframe::run_native("Timer", options, Box::new(|_| Ok(Box::new(App::new())))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("yggdrasil-timer: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let s = store();
    match args.first().map(String::as_str) {
        None => window(),
        Some("ring") => match (args.get(1).and_then(|v| v.parse().ok()), args.get(2).and_then(|v| v.parse().ok())) {
            (Some(id), Some(generation)) => ring(&s, id, generation),
            _ => ExitCode::from(2),
        },
        Some("reconcile") => {
            reconcile(&s);
            ExitCode::SUCCESS
        }
        Some("start") => match args.get(1).and_then(|d| timers::parse_duration(d)) {
            Some(secs) => {
                if start(&s, secs, &args[2..].join(" ")).is_some() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            None => {
                eprintln!("yggdrasil-timer: give a length: 7 (minutes), 1:30, 90s, 1h30m");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: yggdrasil-timer [start DURATION [LABEL] | reconcile | ring ID GEN]");
            ExitCode::from(2)
        }
    }
}
