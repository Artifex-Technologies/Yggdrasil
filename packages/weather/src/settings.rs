//! Weather's settings window: where the weather is for (found automatically, the time zone's city, or a place you pick) and
//! the units. Opened from the Weather card in the top bar (its Settings button) or the app list; no terminal needed.

use std::sync::mpsc::{channel, Receiver};
use std::time::Duration;

use eframe::egui::{self, vec2, Margin, RichText};
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme;
use yggdrasil_weather::config::{self, Config};
use yggdrasil_weather::locate;
use yggdrasil_weather::openmeteo::Place;

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Auto,
    Timezone,
    Manual,
}

enum Job {
    Detect(Result<locate::Found, String>),
    Search(Result<Vec<Place>, String>),
}

struct App {
    current: Option<Config>,
    mode: Mode,
    units: String,
    query: String,
    results: Vec<Place>,
    rx: Option<Receiver<Job>>,
    busy: &'static str,
    note: (String, bool),
}

fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

impl App {
    fn new() -> Self {
        let saved = config::config_path().and_then(|p| config::load(&p));
        let mode = match saved.as_ref().map(|c| c.mode.as_str()) {
            Some("auto") => Mode::Auto,
            Some("manual") => Mode::Manual,
            _ => Mode::Timezone,
        };
        // nothing saved yet: the time zone's city is what the bar uses
        let current = saved.clone().or_else(|| {
            locate::from_timezone().ok().map(|f| Config { name: f.name, lat: f.lat, lon: f.lon, units: "metric".into(), mode: "timezone".into(), detected_at: 0 })
        });
        let units = saved.map(|c| c.units).unwrap_or_else(|| "metric".into());
        App { current, mode, units, query: String::new(), results: vec![], rx: None, busy: "", note: (String::new(), false) }
    }

    fn save(&mut self, c: Config) {
        let Some(path) = config::config_path() else {
            self.note = ("There is no settings folder on this system.".into(), true);
            return;
        };
        match config::save(&path, &c) {
            Ok(()) => {
                self.note = (format!("Saved: {} ({}). The top bar updates within a few minutes.", c.name, c.how()), false);
                self.current = Some(c);
            }
            Err(e) => self.note = (format!("Could not save: {e}"), true),
        }
    }

    fn spawn(&mut self, busy: &'static str, f: impl FnOnce() -> Job + Send + 'static) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(f());
        });
        self.rx = Some(rx);
        self.busy = busy;
        self.note = (String::new(), false);
    }

    fn receive(&mut self) {
        let Some(job) = self.rx.as_ref().and_then(|r| r.try_recv().ok()) else { return };
        self.rx = None;
        self.busy = "";
        match job {
            Job::Detect(Ok(f)) => {
                let units = self.units.clone();
                self.save(Config { name: f.name, lat: f.lat, lon: f.lon, units, mode: "auto".into(), detected_at: now() });
            }
            Job::Detect(Err(e)) => self.note = (format!("{e}. The place was not changed."), true),
            Job::Search(Ok(p)) if p.is_empty() => {
                self.results = p;
                self.note = ("No place by that name. Try the city's name alone.".into(), true);
            }
            Job::Search(Ok(p)) => self.results = p,
            Job::Search(Err(e)) => self.note = (e, true),
        }
    }

    fn use_timezone(&mut self) {
        match locate::from_timezone() {
            Ok(f) => {
                let units = self.units.clone();
                self.save(Config { name: f.name, lat: f.lat, lon: f.lon, units, mode: "timezone".into(), detected_at: 0 });
            }
            Err(e) => self.note = (format!("{e}."), true),
        }
    }

    fn set_units(&mut self, u: &str) {
        self.units = u.into();
        if let Some(mut c) = self.current.clone() {
            c.units = u.into();
            self.save(c);
        }
    }
}

fn choice(ui: &mut egui::Ui, pal: &theme::Palette, on: bool, title: &str, text: &str) -> bool {
    let mut clicked = ui.radio(on, RichText::new(title).strong().color(pal.text)).clicked();
    ui.indent(title, |ui| {
        clicked |= ui.add(egui::Label::new(RichText::new(text).color(pal.dim).size(12.0)).wrap().sense(egui::Sense::click())).clicked();
    });
    ui.add_space(6.0);
    clicked
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        self.receive();
        if self.rx.is_some() {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }

        egui::CentralPanel::default().frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 14, right: 10, top: 6, bottom: 12 })).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new("Weather").strong().color(pal.text));
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
            ui.add_space(8.0);
            match &self.current {
                Some(c) => ui.label(RichText::new(format!("Showing weather for {} ({})", c.name, c.how())).color(pal.text)),
                None => ui.label(RichText::new("No place yet.").color(pal.amber)),
            };
            ui.add_space(12.0);
            let idle = self.rx.is_none();

            if choice(ui, &pal, self.mode == Mode::Auto, "Automatic", "Finds you from nearby Wi-Fi networks through beaconDB, an open, non-profit location database. Their network IDs (not names) and your IP address go to beaconDB when it checks (every few hours).") {
                self.mode = Mode::Auto;
            }
            if self.mode == Mode::Auto {
                ui.indent("auto-use", |ui| {
                    if ui.add_enabled(idle, egui::Button::new("Find my place now")).clicked() {
                        self.spawn("Asking beaconDB…", || Job::Detect(locate::from_wifi()));
                    }
                });
                ui.add_space(6.0);
            }
            if choice(ui, &pal, self.mode == Mode::Timezone, "From time zone", "Rough (the time zone's main city); works offline and sends nothing.") {
                self.mode = Mode::Timezone;
                self.use_timezone();
            }
            if choice(ui, &pal, self.mode == Mode::Manual, "Choose a place", "Search for your town; only the name you type goes to Open-Meteo's place search.") {
                self.mode = Mode::Manual;
            }
            if self.mode == Mode::Manual {
                ui.indent("manual", |ui| {
                    let resp = ui.add(egui::TextEdit::singleline(&mut self.query).hint_text("Town or city").desired_width(ui.available_width() - 80.0).margin(vec2(8.0, 6.0)));
                    let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let go = ui.add_enabled(idle && !self.query.trim().is_empty(), egui::Button::new("Search")).clicked();
                    if (enter || go) && idle && !self.query.trim().is_empty() {
                        let q = self.query.trim().to_string();
                        self.spawn("Searching…", move || Job::Search(locate::search_places(&q)));
                    }
                    let mut picked = None;
                    for (i, p) in self.results.iter().enumerate() {
                        if ui.selectable_label(false, RichText::new(p.label()).color(pal.text)).clicked() {
                            picked = Some(i);
                        }
                    }
                    if let Some(i) = picked {
                        let p = self.results.remove(i);
                        self.results.clear();
                        let units = self.units.clone();
                        self.save(Config { name: p.label(), lat: p.latitude, lon: p.longitude, units, mode: "manual".into(), detected_at: 0 });
                    }
                });
            }

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Units").strong().color(pal.text));
                ui.add_space(12.0);
                if ui.radio(self.units == "metric", "Celsius").clicked() && self.units != "metric" {
                    self.set_units("metric");
                }
                if ui.radio(self.units == "us", "Fahrenheit").clicked() && self.units != "us" {
                    self.set_units("us");
                }
            });

            ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                let (text, col) = if !self.busy.is_empty() {
                    (self.busy.to_string(), pal.dim)
                } else if self.note.0.is_empty() {
                    ("Esc closes".to_string(), pal.dim)
                } else {
                    (self.note.0.clone(), if self.note.1 { pal.amber } else { pal.green })
                };
                ui.add(egui::Label::new(RichText::new(text).color(col).size(12.0)).wrap());
            });
        });
        chrome::outline(&ctx, &pal);
    }
}

fn main() -> eframe::Result {
    // the Weather card's Settings button opens the window; pressing it again closes it
    if yggdrasil_ui::single::toggle("yggdrasil-weather-settings") {
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 520.0])
            .with_min_inner_size([380.0, 420.0])
            .with_title("Weather")
            .with_app_id("yggdrasil-weather-settings")
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Weather", options, Box::new(|_| Ok(Box::new(App::new()))))
}
