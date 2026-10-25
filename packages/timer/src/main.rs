//! yggdrasil-timer: countdown timers for Yggdrasil's top bar.
//!
//!   yggdrasil-timer                    the timer card (the pill in the bar drops it down; a second click closes it): one timer,
//!                                      a preset or a length of your own
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
use yggdrasil_ui::dropdown;
use yggdrasil_ui::theme::{self, Palette};

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

const PRESETS: [(&str, i64); 6] = [("1 min", 60), ("5 min", 300), ("10 min", 600), ("15 min", 900), ("25 min", 1500), ("45 min", 2700)];

struct App {
    store: Store,
    st: State,
    stamp: Option<SystemTime>,
    /// The custom length, as typed or stepped ("25m", "90s", "1h30m").
    custom: String,
    note: String,
    /// Opened from the bar: a dropdown that closes when it loses focus.
    drop: bool,
    blur: dropdown::Blur,
    /// The height the content took last frame (the window is sized to it).
    fitted: f32,
}

impl App {
    fn new(drop: bool) -> App {
        let store = store();
        reconcile(&store);
        let st = store.load();
        App { stamp: Self::stamp_of(&store), store, st, custom: "25m".into(), note: String::new(), drop, blur: Default::default(), fitted: 0.0 }
    }

    fn stamp_of(s: &Store) -> Option<SystemTime> {
        std::fs::metadata(s.state_path()).and_then(|m| m.modified()).ok()
    }

    /// An alarm (or `start` from a terminal) may have changed the timer while the window is open.
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
                self.note.clear();
            }
            None => self.note = format!("Could not save the timer in {}.", self.store.dir.display()),
        }
    }

    /// One timer at a time: starting one ends the one before it.
    fn start(&mut self, secs: i64) {
        let ids: Vec<u64> = self.st.timers.iter().map(|t| t.id).collect();
        for id in ids {
            let st = apply(&self.store, id, |st, _| {
                timers::remove(st, id);
                None
            });
            self.took(st);
        }
        let st = start(&self.store, secs, "");
        self.took(st);
    }

    fn step(&mut self, by: i64) {
        let secs = timers::parse_duration(&self.custom).unwrap_or(1500);
        // whole minutes up and down (a minute at least)
        let next = if by > 0 { (secs / 60 + 1) * 60 } else { ((secs + 59) / 60 - 1).max(1) * 60 };
        self.custom = timers::compact(next.min(100 * 3600));
    }
}

/// The timer's glyph (the bar's icon): an open circle, a dot above the gap, a hand to the upper left.
fn glyph(p: &egui::Painter, c: egui::Pos2, size: f32, col: Color32) {
    let k = size / 24.0;
    let at = |x: f32, y: f32| egui::pos2(c.x + (x - 12.0) * k, c.y + (y - 13.0) * k);
    let stroke = egui::Stroke::new((2.0 * k).max(1.2), col);
    let arc: Vec<egui::Pos2> = (0..=40)
        .map(|i| {
            let a = (-60.0f32 + 300.0 * i as f32 / 40.0).to_radians();
            at(12.0 + 8.0 * a.cos(), 13.0 + 8.0 * a.sin())
        })
        .collect();
    p.add(egui::Shape::line(arc, stroke));
    p.line_segment([at(12.0, 13.0), at(8.5, 9.5)], stroke);
    p.circle_filled(at(12.0, 4.0), 1.3 * k, col);
}

/// A button with the timer's glyph before its text.
fn preset(ui: &mut egui::Ui, text: &str, width: f32, col: Color32) -> bool {
    let r = ui.add(egui::Button::new(RichText::new(format!("      {text}")).size(13.0)).min_size(vec2(width, 34.0)));
    let tw = ui.fonts_mut(|f| f.layout_no_wrap(text.to_string(), FontId::proportional(13.0), col).size().x);
    let x = r.rect.center().x - (tw + 22.0) / 2.0 + 8.0;
    glyph(ui.painter(), egui::pos2(x, r.rect.center().y), 15.0, col);
    r.clicked()
}

fn section(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(11.0).strong().color(pal.dim).extra_letter_spacing(0.8));
}

enum Do {
    Toggle(u64),
    Add(u64),
    Restart(u64),
    Remove(u64),
    Start(i64),
}

impl eframe::App for App {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0; 4]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        let dark = true;
        let accent = theme::system_accent();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        if self.drop {
            self.blur.check(&ctx);
        }
        self.refresh();
        let now = timers::now();
        let cur = timers::current(&self.st).cloned();
        ctx.request_repaint_after(Duration::from_millis(if cur.as_ref().is_some_and(|t| t.running()) { 250 } else { 1000 }));
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        let mut todo: Option<Do> = None;

        let used = egui::CentralPanel::default()
            .frame(dropdown::frame(dark))
            .show(ui, |ui| {
                // buttons like the screenshot's: dark, a faint edge, lighter under the pointer
                let w = &mut ui.visuals_mut().widgets;
                for (st, fill) in [(&mut w.inactive, 12), (&mut w.hovered, 26), (&mut w.active, 36)] {
                    st.weak_bg_fill = Color32::from_white_alpha(fill);
                    st.bg_fill = Color32::from_white_alpha(fill);
                    st.bg_stroke = egui::Stroke::new(1.0, Color32::from_white_alpha(if fill > 12 { 50 } else { 26 }));
                    st.corner_radius = CornerRadius::same(7);
                    st.fg_stroke.color = pal.text;
                }
                ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
                let full = ui.available_width();

                // the countdown
                let (clock, caption, colour, frac) = match &cur {
                    Some(t) if t.done => ("0:00".to_string(), format!("{} timer · time's up", timers::describe(t.duration)), pal.red, 1.0),
                    Some(t) => {
                        let left = t.remaining(now);
                        let what = format!("{} timer", timers::describe(t.duration));
                        let caption = if t.paused() { format!("{what} · paused") } else { what };
                        (timers::clock(left), caption, if t.paused() { pal.amber } else { pal.text }, 1.0 - left as f32 / t.duration.max(1) as f32)
                    }
                    None => {
                        let secs = timers::parse_duration(&self.custom).unwrap_or(1500);
                        (timers::clock(secs), "Ready".to_string(), pal.dim, 0.0)
                    }
                };
                egui::Frame::new().fill(Color32::from_white_alpha(8)).stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(18))).corner_radius(CornerRadius::same(10)).inner_margin(Margin::symmetric(14, 14)).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.vertical_centered(|ui| {
                        ui.label(RichText::new(clock).font(FontId::monospace(44.0)).strong().color(colour));
                        ui.label(RichText::new(caption).size(12.0).color(pal.dim));
                    });
                    ui.add_space(4.0);
                    let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 4.0), egui::Sense::hover());
                    ui.painter().rect_filled(bar, CornerRadius::same(2), Color32::from_white_alpha(22));
                    if frac > 0.0 {
                        let mut done = bar;
                        done.set_width(bar.width() * frac.clamp(0.0, 1.0));
                        ui.painter().rect_filled(done, CornerRadius::same(2), if cur.as_ref().is_some_and(|t| t.done) { pal.red } else { pal.accent });
                    }
                });

                // pause, a minute more, reset
                let third = (full - 16.0) / 3.0;
                ui.horizontal(|ui| {
                    let id = cur.as_ref().map(|t| t.id);
                    let (first, first_do) = match &cur {
                        Some(t) if t.done => ("↻  Again", id.map(Do::Restart)),
                        Some(t) if t.paused() => ("▶  Resume", id.map(Do::Toggle)),
                        _ => ("⏸  Pause", id.map(Do::Toggle)),
                    };
                    if ui.add_enabled(cur.is_some(), egui::Button::new(first).min_size(vec2(third, 32.0))).clicked() {
                        todo = first_do;
                    }
                    if ui.add_enabled(cur.as_ref().is_some_and(|t| !t.done), egui::Button::new("+1 min").min_size(vec2(third, 32.0))).clicked() {
                        todo = id.map(Do::Add);
                    }
                    if ui.add_enabled(cur.is_some(), egui::Button::new("■  Reset").min_size(vec2(third, 32.0))).clicked() {
                        todo = id.map(Do::Remove);
                    }
                });

                ui.add_space(4.0);
                section(ui, &pal, "PRESETS");
                let half = (full - 8.0) / 2.0;
                for row in PRESETS.chunks(2) {
                    ui.horizontal(|ui| {
                        for (name, secs) in row {
                            if preset(ui, name, half, pal.text) {
                                todo = Some(Do::Start(*secs));
                            }
                        }
                    });
                }

                ui.add_space(2.0);
                ui.separator();
                section(ui, &pal, "CUSTOM");
                let mut go = false;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Duration").color(pal.dim));
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new("+").min_size(vec2(28.0, 28.0))).clicked() {
                            self.step(1);
                        }
                        let field = ui.add(egui::TextEdit::singleline(&mut self.custom).horizontal_align(Align::Center).desired_width(70.0).margin(vec2(6.0, 5.0)));
                        go = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.add(egui::Button::new("−").min_size(vec2(28.0, 28.0))).clicked() {
                            self.step(-1);
                        }
                    });
                });
                let parsed = timers::parse_duration(&self.custom);
                let label = match parsed {
                    Some(secs) => format!("▶  Start ({})", timers::describe(secs)),
                    None => "Type 25m, 90s or 1h30m".to_string(),
                };
                if ui.add_enabled(parsed.is_some(), egui::Button::new(RichText::new(label).strong()).min_size(vec2(full, 34.0))).clicked() {
                    go = true;
                }
                if let (true, Some(secs)) = (go, parsed) {
                    todo = Some(Do::Start(secs));
                }
                if !self.note.is_empty() {
                    ui.label(RichText::new(self.note.as_str()).color(pal.amber).size(12.0));
                }
            })
            .response
            .rect;

        // the window is as tall as what it shows
        let want = used.height().ceil();
        if (want - self.fitted).abs() > 1.0 {
            self.fitted = want;
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(vec2(WIDTH, want)));
        }

        if let Some(d) = todo {
            match d {
                Do::Start(secs) => self.start(secs),
                Do::Toggle(id) => self.took(apply(&self.store, id, |st, now| timers::toggle(st, id, now))),
                Do::Add(id) => self.took(apply(&self.store, id, |st, now| timers::add(st, id, 60, now))),
                Do::Restart(id) => self.took(apply(&self.store, id, |st, now| timers::restart(st, id, now))),
                Do::Remove(id) => self.took(apply(&self.store, id, |st, _| {
                    timers::remove(st, id);
                    None
                })),
            }
        }
    }
}

const WIDTH: f32 = 320.0;

fn window() -> ExitCode {
    // one window at a time: a second click on the pill closes it
    if yggdrasil_ui::single::toggle("yggdrasil-timer") {
        return ExitCode::SUCCESS;
    }
    let drop = dropdown::request("yggdrasil-timer");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([WIDTH, 470.0])
            .with_title("Timer")
            .with_app_id("yggdrasil-timer")
            .with_decorations(false)
            .with_transparent(true)
            .with_resizable(false),
        ..Default::default()
    };
    match eframe::run_native("Timer", options, Box::new(move |_| Ok(Box::new(App::new(drop))))) {
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
