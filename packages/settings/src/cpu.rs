//! Settings > Power > Processor: the processor's own page, like the Graphics card's. A live readout (package temperature and
//! the hottest core, clocks, load, package power) with two minutes of small graphs; the limits (the selected mode's speed
//! cap, turbo, the long- and short-term power limits); and, under the crash guard, the turbo multipliers of a K processor
//! (tune). The same panel fills the pop-out window (`yggdrasil-settings --cpu-window`, readout.rs).
//!
//! The readout is read from sysfs and /proc once a second, only while the panel is on screen; the package power comes from
//! the tuning service (the energy counter is root-only).

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::readout::{self, KEEP};
use crate::tune::{self, Act, Tune};
use crate::{sys, ui};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    /// The package's temperature, and the hottest core's, in degrees C.
    pub temp: f32,
    pub hottest: f32,
    /// The cores' clocks now: the average and the fastest, in MHz.
    pub avg_mhz: f32,
    pub max_mhz: f32,
    pub load: f32,
    /// The package's power in watts, when the tuning service measures it.
    pub power: Option<f32>,
}

/// The first line of /proc/stat as (busy, total) jiffies.
pub fn parse_stat(text: &str) -> Option<(u64, u64)> {
    let f: Vec<u64> = text.lines().next()?.strip_prefix("cpu ")?.split_whitespace().take(8).filter_map(|x| x.parse().ok()).collect();
    if f.len() < 8 {
        return None;
    }
    let total: u64 = f.iter().sum();
    Some((total - f[3] - f[4], total))
}

/// The share of time busy between two readings of /proc/stat, in percent.
pub fn load_between(a: (u64, u64), b: (u64, u64)) -> f32 {
    let total = b.1.saturating_sub(a.1);
    if total == 0 {
        return 0.0;
    }
    b.0.saturating_sub(a.0) as f32 * 100.0 / total as f32
}

/// The package's temperature and the hottest core's, from coretemp's (label, degrees) pairs.
pub fn temps(pairs: &[(String, f32)]) -> (Option<f32>, Option<f32>) {
    let package = pairs.iter().find(|(l, _)| l.starts_with("Package")).map(|p| p.1);
    let hottest = pairs.iter().filter(|(l, _)| l.starts_with("Core")).map(|p| p.1).fold(None, |m: Option<f32>, t| Some(m.map_or(t, |m| m.max(t))));
    (package.or(hottest), hottest)
}

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

fn coretemp() -> Vec<(String, f32)> {
    let Ok(dirs) = std::fs::read_dir("/sys/class/hwmon") else { return vec![] };
    for d in dirs.flatten() {
        let p = d.path();
        if read(&format!("{}/name", p.display())).is_some_and(|n| n.trim() == "coretemp") {
            let mut out = vec![];
            for i in 1..=64 {
                let (Some(l), Some(v)) = (read(&format!("{}/temp{i}_label", p.display())), read(&format!("{}/temp{i}_input", p.display()))) else { continue };
                if let Ok(milli) = v.trim().parse::<f32>() {
                    out.push((l.trim().to_string(), milli / 1000.0));
                }
            }
            return out;
        }
    }
    vec![]
}

fn clocks() -> (f32, f32) {
    let mut all = vec![];
    if let Ok(dirs) = std::fs::read_dir("/sys/devices/system/cpu") {
        for d in dirs.flatten() {
            let name = d.file_name().to_string_lossy().to_string();
            if name.strip_prefix("cpu").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit())) {
                if let Some(khz) = read(&format!("{}/cpufreq/scaling_cur_freq", d.path().display())).and_then(|s| s.trim().parse::<f32>().ok()) {
                    all.push(khz / 1000.0);
                }
            }
        }
    }
    if all.is_empty() {
        return (0.0, 0.0);
    }
    (all.iter().sum::<f32>() / all.len() as f32, all.iter().copied().fold(0.0, f32::max))
}

/// The package power the tuning service wrote (/run/yggdrasil/tune.json), if it is fresh.
fn package_power() -> Option<f32> {
    let v: serde_json::Value = serde_json::from_str(&read("/run/yggdrasil/tune.json")?).ok()?;
    let t = v["time"].as_f64()?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs_f64();
    (now - t < 10.0).then(|| v["power_w"].as_f64().map(|w| w as f32)).flatten()
}

enum Msg {
    Sample(Sample),
    Loaded(Option<sys::PowerLimits>, String, Option<Tune>),
    Done(Result<(), String>),
}

/// The once-a-second reader; it ends when this is dropped.
struct Reader(Arc<AtomicBool>);

impl Reader {
    fn start(tx: Sender<Msg>, ctx: egui::Context) -> Reader {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let mut last = read("/proc/stat").and_then(|s| parse_stat(&s));
            while !flag.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_secs(1));
                let stat = read("/proc/stat").and_then(|s| parse_stat(&s));
                let load = match (last, stat) {
                    (Some(a), Some(b)) => load_between(a, b),
                    _ => 0.0,
                };
                last = stat;
                let (package, hottest) = temps(&coretemp());
                let (avg_mhz, max_mhz) = clocks();
                let s = Sample { temp: package.unwrap_or(0.0), hottest: hottest.unwrap_or(0.0), avg_mhz, max_mhz, load, power: package_power() };
                if tx.send(Msg::Sample(s)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Reader(stop)
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub struct Panel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    _reader: Reader,
    history: VecDeque<Sample>,
    limits: Option<sys::PowerLimits>,
    active: String,
    tune: Option<Tune>,
    /// The multipliers as they are being set, before Try.
    edit: Option<Vec<i64>>,
    by_cores: bool,
    status: String,
    busy: bool,
    loaded: Option<Instant>,
}

impl Panel {
    pub fn new(ctx: &egui::Context) -> Panel {
        let (tx, rx) = channel();
        let reader = Reader::start(tx.clone(), ctx.clone());
        let mut p = Panel { tx, rx, _reader: reader, history: VecDeque::new(), limits: None, active: String::new(), tune: None, edit: None, by_cores: false, status: String::new(), busy: false, loaded: None };
        p.load(ctx);
        p
    }

    fn load(&mut self, ctx: &egui::Context) {
        self.loaded = Some(Instant::now());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let text = sys::run("/usr/lib/yggdrasil/powerlimit", &["get"]).unwrap_or_default();
            let active = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["active"].as_str().map(String::from)).unwrap_or_default();
            let _ = tx.send(Msg::Loaded(sys::parse_power_limits(&text), active, tune::get()));
            ctx.request_repaint();
        });
    }

    fn act(&mut self, ctx: &egui::Context, tool: &'static str, args: Vec<String>) {
        self.busy = true;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let mut all = vec![tool.to_string()];
            all.extend(args);
            let refs: Vec<&str> = all.iter().map(String::as_str).collect();
            let _ = tx.send(Msg::Done(sys::act("pkexec", &refs)));
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Sample(s) => {
                    self.history.push_back(s);
                    while self.history.len() > KEEP {
                        self.history.pop_front();
                    }
                }
                Msg::Loaded(l, active, t) => {
                    self.limits = l;
                    self.active = active;
                    self.tune = t;
                }
                Msg::Done(r) => {
                    self.busy = false;
                    self.status = match r {
                        Ok(()) => String::new(),
                        Err(e) if e.is_empty() => "That did not work.".into(),
                        Err(e) => e,
                    };
                    self.edit = None;
                    self.load(ctx);
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        self.receive(ctx);
        if !self.busy && self.loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.load(ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(5));
        if !self.status.is_empty() {
            ui.colored_label(pal.red, &self.status);
        }
        self.readout(ui, pal);
        self.limits_ui(ui, pal, ctx);
        self.multipliers(ui, pal, ctx);
    }

    fn readout(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(now) = self.history.back().copied() else {
            ui::group(ui, pal, Some("Now"), |ui| {
                ui::row(ui, pal, true, "Reading the processor…", Some("Once a second"), |_| {});
            });
            return;
        };
        let h: Vec<Sample> = self.history.iter().copied().collect();
        let top = self.tune.as_ref().map(|t| t.ratio_range.1 as f32 * 100.0).unwrap_or(5000.0).max(now.max_mhz);
        let pmax = self.tune.as_ref().and_then(|t| t.pl.1).unwrap_or(150) as f32;
        ui::group(ui, pal, Some("Now"), |ui| {
            readout::metric(ui, pal, true, "Temperature", &format!("{:.0} °C", now.temp), &format!("hottest core {:.0}", now.hottest), &h, |s| s.temp, 20.0, 100.0, " °C");
            readout::metric(ui, pal, false, "Clock (average)", &format!("{:.1} GHz", now.avg_mhz / 1000.0), &format!("fastest {:.1}", now.max_mhz / 1000.0), &h, |s| s.avg_mhz, 0.0, top, " MHz");
            readout::metric(ui, pal, false, "Load", &format!("{:.0} %", now.load), "", &h, |s| s.load, 0.0, 100.0, " %");
            if let Some(w) = now.power {
                readout::metric(ui, pal, false, "Power", &format!("{w:.0} W"), "", &h, |s| s.power.unwrap_or(0.0), 0.0, pmax.max(w), " W");
            }
        });
    }

    fn limits_ui(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let mut cap: Option<Option<i64>> = None;
        let mut turbo: Option<bool> = None;
        let mut power: Option<String> = None;
        let mode = self.active.clone();
        let name = match mode.as_str() { "performance" => "Performance".to_string(), "balanced" => "Balanced".into(), "power-saver" => "Power saver".into(), "" => "This".into(), o => o.to_string() };
        let busy = self.busy;
        let t = self.tune.clone();
        ui::group(ui, pal, Some("Limits"), |ui| {
            let mut first = true;
            if let Some(l) = &self.limits {
                let mine = l.modes.get(&mode).and_then(|m| m.0);
                let mut c = mine.unwrap_or(100) as f32;
                readout::slider_row(ui, pal, true, &format!("Speed cap · {name} mode"), "The most it may clock in this mode, as a share of its top speed", |ui| {
                    let r = ui::slider_range(ui, pal, &mut c, 20.0..=100.0, " %");
                    if !busy && (r.drag_stopped() || (r.changed() && !r.dragged())) {
                        cap = Some(if c as i64 >= 100 { None } else { Some(c as i64) });
                    }
                    if mine.is_some() && ui::icon_button(ui, pal, crate::icons::Icon::Restart, "Back to the default", !busy, false).clicked() {
                        cap = Some(None);
                    }
                });
                first = false;
            }
            let Some(t) = &t else { return };
            if let Some(mut on) = t.turbo {
                let base = t.base.map(|b| format!(" (above {:.1} GHz)", b as f32 / 10.0)).unwrap_or_default();
                ui::row(ui, pal, first, "Turbo", Some(&format!("Faster than the base clock{base} when there is room")), |ui| {
                    if ui::switch(ui, pal, &mut on).changed() && !busy {
                        turbo = Some(on);
                    }
                });
                first = false;
            }
            for (i, key, title, what) in [(0, "pl1", "Long-term power", "What it may draw for as long as it likes"), (1, "pl2", "Short-term power", "What it may draw in bursts")] {
                let now = if i == 0 { t.pl.0 } else { t.pl.1 };
                let stock = if i == 0 { t.stock_pl.0 } else { t.stock_pl.1 };
                let Some(w0) = now else { continue };
                let mut w = w0 as f32;
                let bios = stock.map(|s| format!(" (the BIOS's: {s} W)")).unwrap_or_default();
                readout::slider_row(ui, pal, first, title, &format!("{what}{bios}"), |ui| {
                    let r = ui::slider_range(ui, pal, &mut w, t.power_range.0 as f32..=t.power_range.1 as f32, " W");
                    if !busy && (r.drag_stopped() || (r.changed() && !r.dragged())) && w as i64 != w0 {
                        power = Some(format!("{key}={}", w as i64));
                    }
                    if stock.is_some_and(|s| s != w0) && ui::icon_button(ui, pal, crate::icons::Icon::Restart, "Back to the BIOS's value", !busy, false).clicked() {
                        power = Some(format!("{key}=default"));
                    }
                });
                first = false;
            }
        });
        if let (Some(v), false) = (cap, mode.is_empty()) {
            let arg = format!("cpu={}", v.map(|n| n.to_string()).unwrap_or_else(|| "default".into()));
            self.act(ctx, "/usr/lib/yggdrasil/powerlimit", vec!["set".into(), mode, arg]);
        } else if let Some(on) = turbo {
            self.act(ctx, "/usr/lib/yggdrasil/tune", vec!["set".into(), "turbo".into(), if on { "on" } else { "off" }.into()]);
        } else if let Some(arg) = power {
            self.act(ctx, "/usr/lib/yggdrasil/tune", vec!["set".into(), "power".into(), arg]);
        }
    }

    fn multipliers(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(t) = self.tune.clone() else { return };
        if !t.service {
            ui::group(ui, pal, Some("Multipliers"), |ui| {
                ui::row(ui, pal, true, "The tuning service is not running", Some("It applies the multipliers and guards against a freeze (yggdrasil-tune)"), |ui| {
                    if ui.add_enabled(!self.busy, egui::Button::new("Start it")).clicked() {
                        self.busy = true;
                        let (tx, c) = (self.tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            let _ = tx.send(Msg::Done(sys::act("pkexec", &["systemctl", "enable", "--now", "yggdrasil-tune.service"])));
                            c.request_repaint();
                        });
                    }
                });
            });
            return;
        }
        let (Some(now), true) = (t.ratios.clone(), t.programmable) else { return };
        let edit = self.edit.get_or_insert_with(|| now.clone());
        let (lo, hi) = t.ratio_range;
        let ghz = |r: i64| format!("{:.1} GHz", r as f32 / 10.0);
        let mut act = None;
        ui::group(ui, pal, Some("Multipliers"), |ui| {
            let mut all = edit[7] as f32;
            readout::slider_row(ui, pal, true, "All cores", &format!("Every core at up to {} ({}× 100 MHz)", ghz(edit[7]), edit[7]), |ui| {
                if ui::slider_range(ui, pal, &mut all, lo as f32..=hi as f32, "×").changed() {
                    edit.iter_mut().for_each(|r| *r = all as i64);
                }
            });
            ui::row(ui, pal, false, "By how many cores are busy", Some("Turbo goes higher when fewer cores are busy"), |ui| {
                ui::switch(ui, pal, &mut self.by_cores);
            });
            if self.by_cores {
                for i in 0..8 {
                    let mut v = edit[i] as f32;
                    let label = if i == 0 { "1 core busy".to_string() } else { format!("{} cores busy", i + 1) };
                    readout::slider_row(ui, pal, false, &label, &ghz(edit[i]), |ui| {
                        if ui::slider_range(ui, pal, &mut v, lo as f32..=hi as f32, "×").changed() {
                            tune::set_ratio(edit, i, v as i64);
                        }
                    });
                }
            }
            let trying = t.trial_ratios.is_some();
            let kept = t.kept_ratios.as_ref().is_some_and(|k| Some(k) != t.stock_ratios.as_ref());
            act = tune::guard_rows(ui, pal, t.cpu_off.as_deref(), trying, *edit != now, kept, self.busy);
        });
        let own = t.stock_ratios.as_ref().map(|s| format!("The processor's own: {} with 1-2 cores, {} with all.", ghz(s[0]), ghz(s[7]))).unwrap_or_default();
        ui.label(RichText::new(format!("{own} Higher needs more voltage than the BIOS may give: go up in small steps. Multipliers above its own can freeze the computer; the crash guard then starts it with your previous values.")).size(11.5).color(pal.dim));
        if let Some(a) = act {
            let mut args = a.args("cpu");
            if a == Act::Try {
                args.push(format!("ratios={}", self.edit.as_ref().map(|e| e.iter().map(|r| r.to_string()).collect::<Vec<_>>().join(",")).unwrap_or_default()));
            }
            self.act(ctx, "/usr/lib/yggdrasil/tune", args);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn load_from_proc_stat() {
        let a = parse_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3\n").unwrap();
        assert_eq!(a, (150, 1000));
        let b = parse_stat("cpu  200 0 100 1500 100 0 0 0 0 0\n").unwrap();
        assert_eq!(load_between(a, b), 150.0 * 100.0 / 900.0);
        assert_eq!(load_between(a, a), 0.0);
        assert!(parse_stat("intr 1 2 3").is_none());
    }

    #[test]
    fn package_and_hottest_core() {
        let p = vec![("Package id 0".to_string(), 48.0), ("Core 0".to_string(), 45.0), ("Core 3".to_string(), 52.0)];
        assert_eq!(temps(&p), (Some(48.0), Some(52.0)));
        assert_eq!(temps(&p[1..]), (Some(52.0), Some(52.0)), "no package sensor: the hottest core stands in");
        assert_eq!(temps(&[]), (None, None));
    }
}
