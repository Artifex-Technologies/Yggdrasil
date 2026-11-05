//! Settings > Power > Fans: the case fans and the pump on a fan controller (casefan; today a Corsair Commander Pro). Each port
//! the setting names is a pump (one speed, never below the pump's floor) or a fan that follows two curves, one for the
//! processor and one for the graphics card, running at whichever asks for more. A fan gets Quiet, Balanced or Cool, or its own
//! curves (curve.rs). The temperatures and each port's speed are read every few seconds while the page is on screen; Apply
//! sends the whole setting to `pkexec casefan set -` (it asks for the password), which saves it and restarts the service.

use std::collections::BTreeMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, RichText};
use yggdrasil_ui::theme::Palette;

use crate::curve::{self, Marks};
use crate::ui;

/// Where the helper may be: the package's place, or /usr/local where it was installed by hand before the package had it.
const HELPERS: [&str; 2] = ["/usr/lib/yggdrasil/casefan", "/usr/local/lib/yggdrasil/casefan"];
/// casefan's floors (PUMP_MIN_PCT, FAN_MIN_PCT): it enforces them too.
pub const PUMP_MIN: i64 = 60;
pub const FAN_MIN: i64 = 20;
/// A fan that reports no speed cannot be seen stopping, so its curves never go below this.
pub const BLIND_MIN: i64 = 35;

type Curve = Vec<[i64; 2]>;

/// The three profiles: (name, processor curve, graphics card curve). Balanced is casefan's default.
pub fn profiles() -> [(&'static str, Curve, Curve); 3] {
    [
        ("Quiet", vec![[55, 20], [70, 30], [80, 50], [88, 80], [92, 100]], vec![[50, 20], [62, 30], [70, 50], [76, 80], [80, 100]]),
        ("Balanced", vec![[50, 25], [65, 35], [75, 55], [85, 80], [90, 100]], vec![[45, 25], [55, 35], [65, 55], [72, 80], [78, 100]]),
        ("Cool", vec![[40, 35], [55, 50], [70, 70], [80, 90], [88, 100]], vec![[40, 35], [50, 50], [60, 70], [68, 90], [75, 100]]),
    ]
}

/// A curve with every speed raised to at least `min`.
pub fn floored(c: &[[i64; 2]], min: i64) -> Curve {
    c.iter().map(|p| [p[0], p[1].max(min)]).collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub name: String,
    /// "pump" or "fan"
    pub role: String,
    pub fixed: i64,
    pub cpu: Curve,
    pub gpu: Curve,
}

#[derive(Clone, Debug, Default)]
pub struct Connected {
    pub label: String,
    pub rpm: i64,
    pub pct: Option<i64>,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub ports: BTreeMap<String, Port>,
    /// None when no fan controller was found.
    pub connected: Option<BTreeMap<String, Connected>>,
    pub cpu: Option<i64>,
    pub gpu: Option<i64>,
    pub running: bool,
    pub hot_cpu: i64,
    pub hot_gpu: i64,
    pub error: Option<String>,
}

fn curve_of(v: &serde_json::Value) -> Curve {
    v.as_array().map(|a| a.iter().filter_map(|p| Some([p.get(0)?.as_i64()?, p.get(1)?.as_i64()?])).collect()).unwrap_or_default()
}

/// `casefan get`'s answer.
pub fn parse(json: &str) -> Option<State> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut ports = BTreeMap::new();
    for (n, p) in v["ports"].as_object()? {
        ports.insert(n.clone(), Port {
            name: p["name"].as_str().unwrap_or("").to_string(),
            role: p["role"].as_str().unwrap_or("fan").to_string(),
            fixed: p["fixed"].as_i64().unwrap_or(100),
            cpu: curve_of(&p["cpu"]),
            gpu: curve_of(&p["gpu"]),
        });
    }
    let connected = v["connected"].as_object().map(|o| {
        o.iter()
            .map(|(n, c)| (n.clone(), Connected { label: c["label"].as_str().unwrap_or("").to_string(), rpm: c["rpm"].as_i64().unwrap_or(0), pct: c["pct"].as_i64() }))
            .collect()
    });
    Some(State {
        ports,
        connected,
        cpu: v["cpu"].as_i64(),
        gpu: v["gpu"].as_i64(),
        running: v["running"].as_bool().unwrap_or(false),
        hot_cpu: v["hot"]["cpu"].as_i64().unwrap_or(92),
        hot_gpu: v["hot"]["gpu"].as_i64().unwrap_or(83),
        error: v["error"].as_str().map(String::from),
    })
}

/// The setting as `casefan set -` reads it.
pub fn to_json(ports: &BTreeMap<String, Port>) -> String {
    let mut out = serde_json::Map::new();
    for (n, p) in ports {
        let v = if p.role == "pump" {
            serde_json::json!({"name": p.name, "role": "pump", "fixed": p.fixed})
        } else {
            serde_json::json!({"name": p.name, "role": "fan", "cpu": p.cpu, "gpu": p.gpu})
        };
        out.insert(n.clone(), v);
    }
    serde_json::json!({ "ports": out }).to_string()
}

/// Which profile a fan's curves are (with its floor applied), or None for its own curves.
pub fn profile_of(p: &Port, min: i64) -> Option<usize> {
    profiles().iter().position(|(_, c, g)| floored(c, min) == p.cpu && floored(g, min) == p.gpu)
}

enum Msg {
    State(Option<State>),
    Done(Result<(), String>),
}

pub struct Panel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    state: Option<State>,
    /// The ports as they are being edited, and whether they differ from the saved ones.
    edit: BTreeMap<String, Port>,
    edited: bool,
    /// The port whose curves are open, which of its two curves, and the point being dragged.
    open: Option<String>,
    which_gpu: bool,
    drag: Option<usize>,
    status: String,
    busy: bool,
    loaded: Option<Instant>,
}

/// Whether the fan control is installed (the Power page links to Fans only then).
pub fn installed() -> bool {
    helper().is_some()
}

fn helper() -> Option<&'static str> {
    HELPERS.into_iter().find(|p| std::path::Path::new(p).exists())
}

impl Panel {
    pub fn new(ctx: &egui::Context) -> Panel {
        let (tx, rx) = channel();
        let mut p = Panel { tx, rx, state: None, edit: BTreeMap::new(), edited: false, open: None, which_gpu: false, drag: None, status: String::new(), busy: false, loaded: None };
        p.load(ctx);
        p
    }

    fn load(&mut self, ctx: &egui::Context) {
        self.loaded = Some(Instant::now());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let text = helper().and_then(|h| crate::sys::run(h, &["get"]));
            let _ = tx.send(Msg::State(text.and_then(|t| parse(&t))));
            ctx.request_repaint();
        });
    }

    /// Sends the whole setting to the helper through pkexec, then reads everything again.
    fn apply(&mut self, ctx: &egui::Context) {
        let Some(h) = helper() else { return };
        self.busy = true;
        let body = to_json(&self.edit);
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = (|| {
                let mut child = Command::new("pkexec").args([h, "set", "-"]).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|_| "pkexec is not available".to_string())?;
                child.stdin.take().ok_or("no stdin")?.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                match out.status.code() {
                    Some(0) => Ok(()),
                    Some(126) | Some(127) => Err("Not changed: the password was not given.".to_string()),
                    _ => Err(format!("Not changed: {}", String::from_utf8_lossy(&out.stderr).trim())),
                }
            })();
            let _ = tx.send(Msg::Done(r));
            ctx.request_repaint();
        });
    }

    fn receive(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::State(s) => {
                    if let Some(s) = &s {
                        if !self.edited && self.drag.is_none() {
                            self.edit = s.ports.clone();
                        }
                    }
                    self.state = s;
                }
                Msg::Done(r) => {
                    self.busy = false;
                    match r {
                        Ok(()) => {
                            self.edited = false;
                            self.status.clear();
                        }
                        Err(e) => self.status = e,
                    }
                    self.load(ctx);
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        self.receive(ctx);
        if !self.busy && self.drag.is_none() && self.loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.load(ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(3));
        if helper().is_none() {
            ui::group(ui, pal, None, |ui| {
                ui::row(ui, pal, true, "Case fan control is not installed", Some("It comes with Yggdrasil's Settings package (yggdrasil-casefan)"), |_| {});
            });
            return;
        }
        let Some(st) = self.state.clone() else {
            ui::group(ui, pal, None, |ui| ui::row(ui, pal, true, "Reading the fans…", None, |_| {}));
            return;
        };
        let Some(connected) = st.connected.clone() else {
            ui::group(ui, pal, None, |ui| {
                ui::row(ui, pal, true, "No fan controller found", Some("Case fans are controlled through a Corsair Commander Pro. Fans on the motherboard follow its own settings, in the firmware setup"), |_| {});
            });
            return;
        };
        if !self.status.is_empty() {
            ui.colored_label(pal.red, &self.status);
        }
        let deg = |t: Option<i64>| t.map(|t| format!("{t} °C")).unwrap_or_else(|| "not read".into());
        ui::group(ui, pal, Some("Now"), |ui| {
            ui::row(ui, pal, true, "Processor", None, |ui| {
                ui.label(deg(st.cpu));
            });
            ui::row(ui, pal, false, "Graphics card", None, |ui| {
                ui.label(deg(st.gpu));
            });
            let sub = if st.running {
                "Following the temperatures".to_string()
            } else {
                "Stopped: the fans stay at the last speed they were given. Apply starts it again".to_string()
            };
            ui::row(ui, pal, false, "Fan control", Some(&sub), |ui| {
                ui.label(if st.running { "On" } else { "Off" });
            });
            if let Some(e) = &st.error {
                ui::row(ui, pal, false, "A sensor cannot be read", Some(&format!("{e}. Every port runs at full speed until it can.")), |_| {});
            }
        });
        let names: Vec<String> = self.edit.keys().cloned().collect();
        for n in names {
            let c = connected.get(&n).cloned().unwrap_or_default();
            let blind = c.rpm == 0;
            let min = if blind { BLIND_MIN } else { FAN_MIN };
            let Some(mut p) = self.edit.get(&n).cloned() else { continue };
            let title = if p.name.is_empty() { format!("Port {n}") } else { p.name.clone() };
            let mut changed = false;
            ui::group(ui, pal, Some(&title), |ui| {
                let speed = match (c.pct, connected.contains_key(&n)) {
                    (_, false) => "not connected".to_string(),
                    (Some(pct), _) if blind => format!("{pct} %, no speed signal"),
                    (Some(pct), _) => format!("{pct} %, {} RPM", c.rpm),
                    (None, _) if blind => "no speed signal".to_string(),
                    (None, _) => format!("{} RPM", c.rpm),
                };
                ui::row(ui, pal, true, "Speed now", Some(&format!("Port {n} ({})", if p.role == "pump" { "pump" } else { "fan" })), |ui| {
                    ui.label(speed);
                });
                if p.role == "pump" {
                    let mut v = p.fixed.max(PUMP_MIN) as f32;
                    ui::row(ui, pal, false, "Pump speed", Some(&format!("Always the same; never below {PUMP_MIN} %, so the liquid keeps moving")), |ui| {
                        let r = ui::slider_range(ui, pal, &mut v, PUMP_MIN as f32..=100.0, " %");
                        if (r.drag_stopped() || (r.changed() && !r.dragged())) && v as i64 != p.fixed {
                            p.fixed = v as i64;
                            changed = true;
                        }
                    });
                    return;
                }
                let sel = profile_of(&p, min).unwrap_or(3);
                let sub = match sel {
                    0 => "Slow until the parts are warm; fine for desktop work and films",
                    1 => "Quiet when idle, firm under load",
                    2 => "Faster sooner: cooler parts, more noise",
                    _ => "Your own curves",
                };
                ui::row(ui, pal, false, "Profile", Some(sub), |ui| {
                    if let Some(i) = ui::segmented(ui, pal, &["Quiet", "Balanced", "Cool", "Custom"], sel) {
                        if i < 3 && i != sel {
                            let pr = &profiles()[i];
                            p.cpu = floored(&pr.1, min);
                            p.gpu = floored(&pr.2, min);
                            changed = true;
                        }
                        if i == 3 {
                            self.open = Some(n.clone());
                        }
                    }
                });
                let is_open = self.open.as_deref() == Some(n.as_str());
                ui::row(ui, pal, false, "Curves", Some("It runs at whichever curve asks for more: it follows the part working hardest"), |ui| {
                    if ui::quiet(ui, pal, if is_open { "Hide" } else { "Edit…" }).clicked() {
                        self.open = if is_open { None } else { Some(n.clone()) };
                        self.drag = None;
                    }
                });
                if is_open {
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        if let Some(i) = ui::segmented(ui, pal, &["Processor", "Graphics card"], self.which_gpu as usize) {
                            if (i == 1) != self.which_gpu {
                                self.which_gpu = i == 1;
                                self.drag = None;
                            }
                        }
                    });
                    ui.add_space(4.0);
                    let (temp, hot) = if self.which_gpu { (st.gpu, st.hot_gpu) } else { (st.cpu, st.hot_cpu) };
                    let label = if blind { "no speed signal: kept at 35 % or more" } else { "below the fans' lowest speed" };
                    let marks = Marks { min, min_label: label, hot, now: temp.zip(c.pct).map(|(t, s)| (t as f32, s as f32)) };
                    let pts = if self.which_gpu { &mut p.gpu } else { &mut p.cpu };
                    if curve::editor(ui, pal, pts, &mut self.drag, &marks) {
                        changed = true;
                    }
                    ui.label(RichText::new("Drag a point. The speed follows temperatures averaged over half a minute and glides, so it does not jump with every short load.").size(11.5).color(pal.dim));
                    ui.add_space(6.0);
                }
            });
            if changed {
                self.edit.insert(n.clone(), p);
                self.edited = true;
            }
        }
        // connected ports the setting leaves alone
        let free: Vec<(String, Connected)> = connected.iter().filter(|(n, _)| !self.edit.contains_key(*n)).map(|(n, c)| (n.clone(), c.clone())).collect();
        if !free.is_empty() {
            ui::group(ui, pal, Some("Other connected ports"), |ui| {
                for (i, (n, c)) in free.iter().enumerate() {
                    let sub = format!("{}, {} RPM. Left alone: it keeps the speed it has", c.label, c.rpm);
                    ui::row(ui, pal, i == 0, &format!("Port {n}"), Some(&sub), |ui| {
                        if ui::quiet(ui, pal, "Control it as a fan").clicked() {
                            let min = if c.rpm == 0 { BLIND_MIN } else { FAN_MIN };
                            let pr = &profiles()[1];
                            self.edit.insert(n.clone(), Port { name: String::new(), role: "fan".into(), fixed: 100, cpu: floored(&pr.1, min), gpu: floored(&pr.2, min) });
                            self.edited = true;
                        }
                    });
                }
            });
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.add_enabled(self.edited && !self.busy, egui::Button::new("Apply")).clicked() {
                self.apply(ctx);
            }
            if ui.add_enabled(self.edited && !self.busy, egui::Button::new("Undo changes")).clicked() {
                self.edit = st.ports.clone();
                self.edited = false;
                self.drag = None;
            }
            if self.busy {
                ui.label(RichText::new("Saving…").size(11.5).color(pal.dim));
            }
        });
        ui.label(
            RichText::new(format!(
                "At {} °C on the processor or {} °C on the graphics card every port runs at full speed. If a temperature cannot be read or the fan control fails, everything runs at full speed. Applying asks for your password.",
                st.hot_cpu, st.hot_gpu
            ))
            .size(11.5)
            .color(pal.dim),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GET: &str = r#"{"ports": {"1": {"name": "Graphics card pump (AIO)", "role": "pump", "fixed": 100},
        "5": {"name": "80mm Noctua", "role": "fan", "cpu": [[50, 35], [70, 50], [85, 80], [90, 100]], "gpu": [[45, 35], [65, 50], [75, 80], [80, 100]]},
        "6": {"name": "Top exhaust", "role": "fan", "cpu": [[50, 25], [65, 35], [75, 55], [85, 80], [90, 100]], "gpu": [[45, 25], [55, 35], [65, 55], [72, 80], [78, 100]]}},
        "connected": {"1": {"label": "fan1 4pin", "rpm": 3833, "pct": 100}, "5": {"label": "fan5 4pin", "rpm": 0, "pct": 41}, "6": {"label": "fan6 4pin", "rpm": 1204, "pct": 33}},
        "cpu": 59, "gpu": 45, "running": true, "hot": {"cpu": 92, "gpu": 83}}"#;

    #[test]
    fn the_helpers_answer_parses() {
        let s = parse(GET).unwrap();
        assert_eq!(s.ports.len(), 3);
        assert_eq!(s.ports["1"].role, "pump");
        assert_eq!(s.connected.as_ref().unwrap()["5"].rpm, 0);
        assert_eq!(s.connected.as_ref().unwrap()["6"].pct, Some(33));
        assert_eq!((s.cpu, s.gpu, s.running, s.hot_cpu, s.hot_gpu), (Some(59), Some(45), true, 92, 83));
        let none = parse(r#"{"ports": {}, "connected": null, "cpu": 40, "gpu": null, "running": false, "error": "no card"}"#).unwrap();
        assert!(none.connected.is_none());
        assert_eq!(none.error.as_deref(), Some("no card"));
        assert!(parse("nope").is_none());
    }

    #[test]
    fn profiles_are_recognised_with_the_floor() {
        let s = parse(GET).unwrap();
        assert_eq!(profile_of(&s.ports["6"], FAN_MIN), Some(1), "the exhaust is on Balanced (casefan's default)");
        assert_eq!(profile_of(&s.ports["5"], BLIND_MIN), None, "the 80mm fan has its own curves");
        let mut quiet = s.ports["5"].clone();
        quiet.cpu = floored(&profiles()[0].1, BLIND_MIN);
        quiet.gpu = floored(&profiles()[0].2, BLIND_MIN);
        assert_eq!(profile_of(&quiet, BLIND_MIN), Some(0));
        assert!(quiet.cpu.iter().all(|p| p[1] >= BLIND_MIN), "a fan with no speed signal never goes below 35 %");
    }

    #[test]
    fn every_profile_rises_and_ends_at_full_speed() {
        for (name, c, g) in profiles() {
            for curve in [c, g] {
                assert!(curve.windows(2).all(|w| w[1][0] > w[0][0] && w[1][1] >= w[0][1]), "{name}");
                assert_eq!(curve.last().unwrap()[1], 100, "{name}");
                assert!(curve[0][1] >= FAN_MIN, "{name}");
            }
        }
    }

    #[test]
    fn the_setting_goes_back_as_casefan_reads_it() {
        let s = parse(GET).unwrap();
        let v: serde_json::Value = serde_json::from_str(&to_json(&s.ports)).unwrap();
        assert_eq!(v["ports"]["1"], serde_json::json!({"name": "Graphics card pump (AIO)", "role": "pump", "fixed": 100}));
        assert_eq!(v["ports"]["6"]["cpu"][0], serde_json::json!([50, 25]));
        assert!(v["ports"]["6"].get("fixed").is_none());
        assert_eq!(parse(&format!(r#"{{"ports": {}, "connected": {{}}}}"#, v["ports"])).unwrap().ports, s.ports);
    }
}
