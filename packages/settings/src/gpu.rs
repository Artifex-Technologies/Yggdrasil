//! Settings > Power > Graphics card: an Afterburner-style page for an NVIDIA card. A live readout (temperature, power,
//! clocks, load, video memory, fans) with the last two minutes as small graphs, the selected power mode's power limit
//! (powerlimit), and the fans: the card's own control, one speed, or a curve (gpufan). The same panel fills the pop-out
//! window (`yggdrasil-settings --gpu-window`), a process of its own that stays open after Settings is closed. Nothing here
//! overclocks.
//!
//! The readout is one `nvidia-smi -lms 1000` running only while the panel is on screen (it ends with the panel).

use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Margin, Rect, RichText, Sense, Shape, Stroke};
use yggdrasil_ui::theme::{self, Palette};
use yggdrasil_ui::chrome;

use crate::{sys, ui};

const FIELDS: &str = "temperature.gpu,power.draw,power.limit,clocks.gr,clocks.mem,clocks.max.gr,clocks.max.mem,utilization.gpu,memory.used,memory.total,fan.speed,clocks_event_reasons.active";
/// Two minutes of readings, one a second.
const KEEP: usize = 120;
/// The fan curve's temperature axis, in degrees C.
const T_LO: f32 = 20.0;
const T_HI: f32 = 100.0;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Sample {
    pub temp: f32,
    pub power: f32,
    pub limit: f32,
    pub core: f32,
    pub mem: f32,
    pub core_max: f32,
    pub mem_max: f32,
    pub load: f32,
    pub vram: f32,
    pub vram_total: f32,
    pub fan: f32,
    pub reasons: u64,
}

/// One line of `nvidia-smi --query-gpu=FIELDS --format=csv,noheader,nounits`. A value the card does not report ("[N/A]")
/// reads as 0; a line that is not a reading (an error message) is None.
pub fn parse_sample(line: &str) -> Option<Sample> {
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() != 12 || f[0].parse::<f32>().is_err() {
        return None;
    }
    let n = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
    Some(Sample {
        temp: n(0),
        power: n(1),
        limit: n(2),
        core: n(3),
        mem: n(4),
        core_max: n(5),
        mem_max: n(6),
        load: n(7),
        vram: n(8),
        vram_total: n(9),
        fan: n(10),
        reasons: u64::from_str_radix(f[11].trim_start_matches("0x"), 16).unwrap_or(0),
    })
}

/// What is holding the clocks back, in words (NVML's clock event reasons), the most telling first. Empty: nothing, it runs
/// as fast as it is asked to.
pub fn reasons(bits: u64) -> Vec<&'static str> {
    [
        (0x40, "too hot (the card's own protection)"),
        (0x20, "too hot (the driver)"),
        (0x08, "the card slowing itself down"),
        (0x80, "the power supply asking it to slow down"),
        (0x04, "the power limit"),
        (0x01, "nothing to do"),
        (0x10, "sync boost"),
        (0x02, "the application's clocks"),
        (0x100, "the display's clocks"),
    ]
    .iter()
    .filter(|(b, _)| bits & b != 0)
    .map(|(_, s)| *s)
    .collect()
}

/// The fans' setting and state (`gpufan get`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fans {
    /// auto (the card's own control), fixed or curve
    pub mode: String,
    pub fixed: i64,
    pub curve: Vec<[i64; 2]>,
    /// The card's lowest speed, in percent.
    pub min: i64,
    /// From this temperature on the fans run at full speed whatever the setting.
    pub hot: i64,
    pub running: bool,
    /// Whether the card's fans could be read at all.
    pub ok: bool,
}

pub fn parse_fans(json: &str) -> Option<Fans> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let curve = v["curve"].as_array()?.iter().filter_map(|p| Some([p.get(0)?.as_i64()?, p.get(1)?.as_i64()?])).collect();
    Some(Fans {
        mode: v["mode"].as_str()?.to_string(),
        fixed: v["fixed"].as_i64().unwrap_or(60),
        curve,
        min: v["min"].as_i64().unwrap_or(0),
        hot: v["hot"].as_i64().unwrap_or(85),
        running: v["running"].as_bool().unwrap_or(false),
        ok: v["fans"].is_array(),
    })
}

/// Moves point `i` of a curve to (t, p), kept usable: at least 3 C from its neighbours, never slower than the point before it
/// nor faster than the one after, within the axis and between the card's minimum and 100 %.
pub fn move_point(pts: &mut [[i64; 2]], i: usize, t: i64, p: i64, min: i64) {
    let (t_lo, p_lo) = if i == 0 { (T_LO as i64, min) } else { (pts[i - 1][0] + 3, pts[i - 1][1].max(min)) };
    let (t_hi, p_hi) = if i + 1 == pts.len() { (T_HI as i64, 100) } else { (pts[i + 1][0] - 3, pts[i + 1][1]) };
    pts[i] = [t.clamp(t_lo, t_hi.max(t_lo)), p.clamp(p_lo, p_hi.max(p_lo))];
}

/// The curve as `gpufan set curve=...` takes it.
pub fn curve_arg(pts: &[[i64; 2]]) -> String {
    format!("curve={}", pts.iter().map(|p| format!("{}:{}", p[0], p[1])).collect::<Vec<_>>().join(","))
}

enum Msg {
    Sample(Sample),
    Limits(Option<sys::PowerLimits>, String),
    Fans(Option<Fans>),
    Done(Result<(), String>),
}

/// The running `nvidia-smi`; it ends when this is dropped.
struct Reader(Child);

impl Reader {
    fn start(tx: Sender<Msg>, ctx: egui::Context) -> Option<Reader> {
        let mut child = Command::new("nvidia-smi")
            .args(["--id=0", &format!("--query-gpu={FIELDS}"), "--format=csv,noheader,nounits", "-lms", "1000"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let out = child.stdout.take()?;
        std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(s) = parse_sample(&line) {
                    if tx.send(Msg::Sample(s)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            }
        });
        Some(Reader(child))
    }
}

impl Drop for Reader {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct Panel {
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    reader: Option<Reader>,
    history: VecDeque<Sample>,
    limits: Option<sys::PowerLimits>,
    /// The power mode that is selected now.
    active: String,
    fans: Option<Fans>,
    /// The curve as it is being edited, and whether it differs from the saved one.
    curve: Vec<[i64; 2]>,
    curve_edited: bool,
    drag: Option<usize>,
    status: String,
    busy: bool,
    loaded: Option<Instant>,
}

impl Panel {
    pub fn new(ctx: &egui::Context) -> Panel {
        let (tx, rx) = channel();
        let reader = Reader::start(tx.clone(), ctx.clone());
        let mut p = Panel { tx, rx, reader, history: VecDeque::new(), limits: None, active: String::new(), fans: None, curve: vec![], curve_edited: false, drag: None, status: String::new(), busy: false, loaded: None };
        p.load(ctx);
        p
    }

    /// Reads the power limits and the fans' setting again, in the background.
    fn load(&mut self, ctx: &egui::Context) {
        self.loaded = Some(Instant::now());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let text = sys::run("/usr/lib/yggdrasil/powerlimit", &["get"]).unwrap_or_default();
            let active = serde_json::from_str::<serde_json::Value>(&text).ok().and_then(|v| v["active"].as_str().map(String::from)).unwrap_or_default();
            let _ = tx.send(Msg::Limits(sys::parse_power_limits(&text), active));
            // a debug build may read the fans' answer from $YGGDRASIL_GPUFAN_FIXTURE (screenshots of each mode); release builds never do
            let fixture = if cfg!(debug_assertions) { std::env::var("YGGDRASIL_GPUFAN_FIXTURE").ok().and_then(|f| std::fs::read_to_string(f).ok()) } else { None };
            let fans = fixture.or_else(|| sys::run("/usr/lib/yggdrasil/gpufan", &["get"]));
            let _ = tx.send(Msg::Fans(fans.and_then(|s| parse_fans(&s))));
            ctx.request_repaint();
        });
    }

    /// Runs a change with pkexec (it asks for the password), then reads everything again.
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
                Msg::Limits(l, active) => {
                    self.limits = l;
                    self.active = active;
                }
                Msg::Fans(f) => {
                    if !self.curve_edited && self.drag.is_none() {
                        self.curve = f.as_ref().map(|f| f.curve.clone()).unwrap_or_default();
                    }
                    self.fans = f;
                }
                Msg::Done(r) => {
                    self.busy = false;
                    self.status = match r {
                        Ok(()) => String::new(),
                        Err(e) if e.is_empty() => "That did not work.".into(),
                        Err(e) => e,
                    };
                    self.curve_edited = false;
                    self.load(ctx);
                }
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        self.receive(ctx);
        if self.reader.is_none() && self.history.is_empty() {
            self.reader = Reader::start(self.tx.clone(), ctx.clone());
        }
        // the power mode and the fans may be changed elsewhere (the panel, another window): read them again now and then
        if !self.busy && self.drag.is_none() && self.loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.load(ctx);
        }
        ctx.request_repaint_after(Duration::from_secs(5));
        if !self.status.is_empty() {
            ui.colored_label(pal.red, &self.status);
        }
        self.readout(ui, pal);
        self.power(ui, pal, ctx);
        self.fans_ui(ui, pal, ctx);
    }

    fn readout(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        let Some(now) = self.history.back().copied() else {
            ui::group(ui, pal, Some("Now"), |ui| {
                ui::row(ui, pal, true, "Reading the card…", Some("nvidia-smi reports once a second"), |_| {});
            });
            return;
        };
        let h: Vec<Sample> = self.history.iter().copied().collect();
        let gib = |mib: f32| mib / 1024.0;
        ui::group(ui, pal, Some("Now"), |ui| {
            metric(ui, pal, true, "Temperature", &format!("{:.0} °C", now.temp), &h, |s| s.temp, 20.0, 100.0, " °C");
            metric(ui, pal, false, "Power", &format!("{:.0} W of {:.0} W", now.power, now.limit), &h, |s| s.power, 0.0, now.limit.max(1.0), " W");
            metric(ui, pal, false, "Core clock", &format!("{:.0} MHz", now.core), &h, |s| s.core, 0.0, now.core_max.max(1.0), " MHz");
            metric(ui, pal, false, "Memory clock", &format!("{:.0} MHz", now.mem), &h, |s| s.mem, 0.0, now.mem_max.max(1.0), " MHz");
            metric(ui, pal, false, "Load", &format!("{:.0} %", now.load), &h, |s| s.load, 0.0, 100.0, " %");
            metric(ui, pal, false, "Video memory", &format!("{:.1} of {:.0} GiB", gib(now.vram), gib(now.vram_total)), &h, |s| s.vram / 1024.0, 0.0, gib(now.vram_total).max(1.0), " GiB");
            metric(ui, pal, false, "Fans", &format!("{:.0} %", now.fan), &h, |s| s.fan, 0.0, 100.0, " %");
            let why = reasons(now.reasons);
            let (text, hot) = match now.reasons {
                0 => ("Nothing is holding the clocks back".to_string(), false),
                0x1 => ("Idle: the clocks are low because there is nothing for the card to do".to_string(), false),
                bits => (format!("Held back by {}", why.iter().copied().filter(|w| *w != "nothing to do").collect::<Vec<_>>().join(", ")), bits & 0x68 != 0),
            };
            ui::row(ui, pal, false, "Clocks", Some(&text), |ui| {
                if hot {
                    ui.label(RichText::new("Hot").color(pal.amber).strong());
                }
            });
        });
    }

    fn power(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(l) = self.limits.clone() else { return };
        let Some(g) = l.gpu else { return };
        let mode = self.active.clone();
        let name = match mode.as_str() { "performance" => "Performance".to_string(), "balanced" => "Balanced".into(), "power-saver" => "Power saver".into(), "" => "This".into(), o => o.to_string() };
        let mine = l.modes.get(&mode).and_then(|m| m.1);
        let top = g.hold.unwrap_or(g.max).max(g.min);
        let mut w = mine.unwrap_or(g.default).min(top) as f32;
        let mut set: Option<Option<i64>> = None;
        let sub = match g.hold {
            Some(h) => format!("Held at {h} W while the graphics card unlock is tried"),
            None => format!("The most power the card may draw in this mode ({}-{} W, default {} W)", g.min, g.max, g.default),
        };
        ui::group(ui, pal, Some("Power limit"), |ui| {
            slider_row(ui, pal, true, &format!("{name} mode"), &sub, |ui| {
                let r = ui::slider_range(ui, pal, &mut w, g.min as f32..=top as f32, " W");
                if !self.busy && (r.drag_stopped() || (r.changed() && !r.dragged())) {
                    set = Some(if w as i64 == g.default { None } else { Some(w as i64) });
                }
                if mine.is_some() && ui::icon_button(ui, pal, crate::icons::Icon::Restart, "Back to the default", !self.busy, false).clicked() {
                    set = Some(None);
                }
            });
        });
        if let (Some(v), false) = (set, mode.is_empty()) {
            let arg = format!("gpu={}", v.map(|n| n.to_string()).unwrap_or_else(|| "default".into()));
            self.act(ctx, "/usr/lib/yggdrasil/powerlimit", vec!["set".into(), mode, arg]);
        }
    }

    fn fans_ui(&mut self, ui: &mut egui::Ui, pal: &Palette, ctx: &egui::Context) {
        let Some(f) = self.fans.clone() else { return };
        if !f.ok {
            return;
        }
        let now = self.history.back().copied();
        let mut change: Option<String> = None;
        ui::group(ui, pal, Some("Fans"), |ui| {
            let sel = match f.mode.as_str() { "fixed" => 1, "curve" => 2, _ => 0 };
            let sub = match f.mode.as_str() {
                "fixed" => format!("Held at {} % by Yggdrasil", f.fixed.max(f.min)),
                "curve" => match now {
                    Some(s) => format!("Following your curve: {:.0} % at {:.0} °C", s.fan, s.temp),
                    None => "Following your curve".to_string(),
                },
                _ => "The card decides, as it does out of the box".to_string(),
            };
            ui::row(ui, pal, true, "Control", Some(&sub), |ui| {
                if let Some(i) = ui::segmented(ui, pal, &["Card", "Fixed", "Curve"], sel) {
                    if i != sel && !self.busy {
                        change = Some(match i { 1 => format!("fixed={}", f.fixed), 2 => curve_arg(&f.curve), _ => "auto".into() });
                    }
                }
            });
            if f.mode == "fixed" {
                let mut v = f.fixed.max(f.min) as f32;
                slider_row(ui, pal, false, "Speed", &format!("From {} % (the card's lowest) to 100 %", f.min), |ui| {
                    let r = ui::slider_range(ui, pal, &mut v, f.min as f32..=100.0, " %");
                    if !self.busy && (r.drag_stopped() || (r.changed() && !r.dragged())) && v as i64 != f.fixed {
                        change = Some(format!("fixed={}", v as i64));
                    }
                });
            }
            if f.mode == "curve" && self.curve.len() >= 2 {
                ui.add_space(6.0);
                self.curve_editor(ui, pal, &f, now);
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui.add_enabled(self.curve_edited && !self.busy, egui::Button::new("Apply")).clicked() {
                        change = Some(curve_arg(&self.curve));
                    }
                    if ui.add_enabled(self.curve_edited, egui::Button::new("Undo changes")).clicked() {
                        self.curve = f.curve.clone();
                        self.curve_edited = false;
                    }
                    ui.label(RichText::new("Drag a point; Apply saves it.").size(11.5).color(pal.dim));
                });
                ui.add_space(6.0);
            }
        });
        ui.label(RichText::new(format!("Never below {} %; at {} °C or more the fans run at full speed. If Yggdrasil's fan control stops or fails, the card takes its fans back. Changing this asks for your password.", f.min, f.hot)).size(11.5).color(pal.dim));
        if let Some(arg) = change {
            self.act(ctx, "/usr/lib/yggdrasil/gpufan", vec!["set".into(), arg]);
        }
    }

    /// The curve: temperature across, speed up; drag a point to move it. The card's lowest speed and the "full speed" range
    /// are shaded and named, and the card's temperature and speed now are marked.
    fn curve_editor(&mut self, ui: &mut egui::Ui, pal: &Palette, f: &Fans, now: Option<Sample>) {
        let w = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, 190.0), Sense::click_and_drag());
        let plot = Rect::from_min_max(pos2(rect.left() + 40.0, rect.top() + 8.0), pos2(rect.right() - 10.0, rect.bottom() - 22.0));
        let x = |t: f32| plot.left() + (t - T_LO) / (T_HI - T_LO) * plot.width();
        let y = |p: f32| plot.bottom() - p / 100.0 * plot.height();
        let painter = ui.painter_at(rect);
        let small = egui::FontId::proportional(11.0);
        // the grid: quiet lines every 25 % and every 20 C, labelled outside the plot
        for p in [0.0, 25.0, 50.0, 75.0, 100.0] {
            painter.hline(plot.x_range(), y(p), Stroke::new(1.0, pal.border));
            painter.text(pos2(plot.left() - 6.0, y(p)), egui::Align2::RIGHT_CENTER, format!("{p:.0} %"), small.clone(), pal.dim);
        }
        for t in [20.0, 40.0, 60.0, 80.0, 100.0] {
            painter.vline(x(t), plot.y_range(), Stroke::new(1.0, pal.border));
            // the last label ends at the plot's edge instead of running past it
            let align = if t >= T_HI { egui::Align2::RIGHT_TOP } else { egui::Align2::CENTER_TOP };
            painter.text(pos2(x(t), plot.bottom() + 4.0), align, format!("{t:.0} °C"), small.clone(), pal.dim);
        }
        // below the card's lowest speed, and from the hot point on, the curve has no say
        let min_rect = Rect::from_min_max(pos2(plot.left(), y(f.min as f32)), plot.right_bottom());
        painter.rect_filled(min_rect, 0.0, pal.hover.gamma_multiply(0.6));
        painter.text(pos2(plot.right() - 4.0, y(f.min as f32) + 2.0), egui::Align2::RIGHT_TOP, "below the card's lowest speed", small.clone(), pal.dim);
        let hot = Rect::from_min_max(pos2(x(f.hot as f32), plot.top()), plot.right_bottom());
        painter.rect_filled(hot, 0.0, pal.red.gamma_multiply(0.12));
        painter.text(pos2(plot.right() - 4.0, plot.top() + 14.0), egui::Align2::RIGHT_TOP, format!("full speed from {} °C", f.hot), small.clone(), pal.dim);
        // the curve, carried flat to both edges as gpufan reads it
        let pts: Vec<egui::Pos2> = self.curve.iter().map(|p| pos2(x(p[0] as f32), y(p[1].max(f.min) as f32))).collect();
        let mut line = vec![pos2(plot.left(), pts[0].y)];
        line.extend(pts.iter().copied());
        line.push(pos2(plot.right(), pts[pts.len() - 1].y));
        painter.add(Shape::line(line, Stroke::new(2.0, pal.accent)));
        // the card now
        if let Some(s) = now {
            let at = pos2(x(s.temp.clamp(T_LO, T_HI)), y(s.fan.clamp(0.0, 100.0)));
            painter.vline(at.x, plot.y_range(), Stroke::new(1.0, pal.dim.gamma_multiply(0.6)));
            painter.circle(at, 4.5, pal.text, Stroke::new(2.0, pal.field));
            painter.text(pos2(at.x + 6.0, plot.top() + 14.0), egui::Align2::LEFT_TOP, format!("now {:.0} °C, {:.0} %", s.temp, s.fan), small.clone(), pal.text);
        }
        // dragging a point
        let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
        if resp.drag_started() {
            self.drag = pointer.and_then(|p| pts.iter().enumerate().map(|(i, q)| (i, q.distance(p))).filter(|(_, d)| *d < 16.0).min_by(|a, b| a.1.total_cmp(&b.1)).map(|(i, _)| i));
        }
        if let (Some(i), Some(p), true) = (self.drag, pointer, resp.dragged()) {
            let t = (T_LO + (p.x - plot.left()) / plot.width() * (T_HI - T_LO)).round() as i64;
            let pct = ((plot.bottom() - p.y) / plot.height() * 100.0).round() as i64;
            let before = self.curve[i];
            move_point(&mut self.curve, i, t, pct, f.min);
            if self.curve[i] != before {
                self.curve_edited = true;
            }
        }
        if resp.drag_stopped() {
            self.drag = None;
        }
        let near = pointer.and_then(|p| pts.iter().position(|q| q.distance(p) < 16.0));
        for (i, q) in pts.iter().enumerate() {
            let big = self.drag == Some(i) || near == Some(i);
            painter.circle(*q, if big { 7.0 } else { 5.5 }, pal.accent, Stroke::new(2.0, pal.field));
        }
        if let Some(i) = self.drag.or(near) {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
            let c = self.curve[i];
            painter.text(pts[i] + vec2(10.0, -10.0), egui::Align2::LEFT_BOTTOM, format!("{} °C → {} %", c[0], c[1].max(f.min)), egui::FontId::proportional(12.0), pal.text);
        }
    }
}

/// A row with a slider: on the right as usual, or on a line of its own under the text when the panel is narrow (the pop-out
/// window), so the title never wraps letter by letter.
fn slider_row(ui: &mut egui::Ui, pal: &Palette, first: bool, title: &str, sub: &str, control: impl FnOnce(&mut egui::Ui)) {
    if ui.available_width() >= 600.0 {
        return ui::row(ui, pal, first, title, Some(sub), control);
    }
    ui::row(ui, pal, first, title, Some(sub), |_| {});
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 34.0), egui::Layout::right_to_left(egui::Align::Center), control);
    ui.add_space(4.0);
}

/// One reading: its name and value on the left, its last two minutes as a small graph on the right. Each graph has its own
/// fixed scale (shown on hover), never a second axis.
#[allow(clippy::too_many_arguments)]
fn metric(ui: &mut egui::Ui, pal: &Palette, first: bool, name: &str, value: &str, h: &[Sample], get: impl Fn(&Sample) -> f32, lo: f32, hi: f32, unit: &str) {
    if !first {
        let r = ui.available_rect_before_wrap();
        ui.painter().hline(r.left()..=r.right(), r.top(), Stroke::new(1.0, pal.border));
    }
    ui.allocate_ui_with_layout(vec2(ui.available_width(), 46.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
        ui.set_min_height(46.0);
        ui.allocate_ui_with_layout(vec2(150.0, 40.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_width(150.0);
            ui.label(RichText::new(name).size(12.0).color(pal.dim));
            ui.label(RichText::new(value).size(15.0).color(pal.text));
        });
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::hover());
        let painter = ui.painter_at(rect.expand(2.0));
        painter.hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, pal.border));
        if h.len() < 2 {
            return;
        }
        let step = rect.width() / (KEEP - 1) as f32;
        let start = KEEP - h.len();
        let at = |i: usize, s: &Sample| pos2(rect.left() + (start + i) as f32 * step, rect.bottom() - ((get(s) - lo) / (hi - lo)).clamp(0.0, 1.0) * rect.height());
        let line: Vec<egui::Pos2> = h.iter().enumerate().map(|(i, s)| at(i, s)).collect();
        // a light fill under the line (drawn as thin columns: the area is not convex)
        let fill = pal.accent.gamma_multiply(0.18);
        for w in line.windows(2) {
            painter.add(Shape::convex_polygon(vec![w[0], w[1], pos2(w[1].x, rect.bottom()), pos2(w[0].x, rect.bottom())], fill, Stroke::NONE));
        }
        painter.add(Shape::line(line.clone(), Stroke::new(2.0, pal.accent)));
        if let Some(p) = resp.hover_pos() {
            let i = (((p.x - rect.left()) / step).round() as usize).clamp(start, KEEP - 1) - start;
            let q = line[i];
            painter.vline(q.x, rect.y_range(), Stroke::new(1.0, pal.dim));
            painter.circle(q, 4.0, pal.accent, Stroke::new(2.0, pal.field));
            let ago = h.len() - 1 - i;
            let v = get(&h[i]);
            let when = if ago == 0 { "now".to_string() } else { format!("{ago} s ago") };
            resp.on_hover_text_at_pointer(format!("{when}: {}{unit}   (scale {lo:.0}-{hi:.0}{unit})", if v >= 100.0 || unit.trim() != "GiB" { format!("{v:.0}") } else { format!("{v:.1}") }));
        }
    });
}

/// Opens the pop-out window: Settings started again as its own process (`--gpu-window`), in a process group of its own, so it
/// stays open when Settings is closed.
pub fn pop_out() {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().ok().filter(|p| p.exists()).map(|p| p.into_os_string()).unwrap_or_else(|| "yggdrasil-settings".into());
    if let Ok(mut child) = Command::new(exe).arg("--gpu-window").process_group(0).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

struct Window {
    panel: Panel,
}

impl eframe::App for Window {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        let dark = theme::system_dark();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 16, right: 8, top: 6, bottom: 12 }))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Graphics card").size(16.0).strong());
                    chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                    chrome::window_controls(ui, &pal);
                });
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.set_max_width((ui.available_width() - 14.0).max(0.0));
                    self.panel.show(ui, &pal, &ctx);
                });
            });
        chrome::outline(&ctx, &pal);
    }
}

/// `yggdrasil-settings --gpu-window`: the pop-out. Only one at a time: a second start brings the first to the front and ends.
pub fn window_main() -> eframe::Result {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let lock = std::fs::File::create(format!("{dir}/yggdrasil-gpu-window.lock")).ok();
    if lock.as_ref().is_some_and(|f| f.try_lock().is_err()) {
        let _ = sys::act("hyprctl", &["dispatch", r#"hl.dsp.focus({ window = "class:yggdrasil-gpu" })"#]);
        return Ok(());
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([460.0, 760.0])
            .with_min_inner_size([380.0, 420.0])
            .with_title("Graphics card")
            .with_app_id("yggdrasil-gpu")
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("Graphics card", options, Box::new(|cc| Ok(Box::new(Window { panel: Panel::new(&cc.egui_ctx) }))))
    // the lock is held until here
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_reading_parses() {
        let s = parse_sample("25, 37.37, 290.00, 675, 810, 2160, 7000, 26, 1286, 11264, 24, 0x0000000000000001").unwrap();
        assert_eq!((s.temp, s.power, s.limit, s.core, s.mem), (25.0, 37.37, 290.0, 675.0, 810.0));
        assert_eq!((s.load, s.vram, s.vram_total, s.fan, s.reasons), (26.0, 1286.0, 11264.0, 24.0, 1));
        let na = parse_sample("40, [N/A], 290.00, 675, 810, 2160, 7000, 26, 1286, 11264, [N/A], [N/A]").unwrap();
        assert_eq!((na.power, na.fan, na.reasons), (0.0, 0.0, 0));
        assert!(parse_sample("Failed to initialize NVML: Driver/library version mismatch").is_none());
        assert!(parse_sample("").is_none());
    }

    #[test]
    fn what_holds_the_clocks_back() {
        assert_eq!(reasons(0x1), vec!["nothing to do"]);
        assert_eq!(reasons(0x4 | 0x40), vec!["too hot (the card's own protection)", "the power limit"]);
        assert!(reasons(0).is_empty());
    }

    #[test]
    fn fans_parse() {
        let f = parse_fans(r#"{"mode": "curve", "fixed": 60, "curve": [[40, 30], [60, 45], [75, 75], [85, 100]], "fans": [24, 0], "min": 24, "max": 100, "temp": 25, "running": true, "hot": 85}"#).unwrap();
        assert_eq!((f.mode.as_str(), f.min, f.hot, f.running, f.ok), ("curve", 24, 85, true, true));
        assert_eq!(f.curve, vec![[40, 30], [60, 45], [75, 75], [85, 100]]);
        let none = parse_fans(r#"{"mode": "auto", "fixed": 60, "curve": [[40, 30], [85, 100]], "fans": null, "error": "no card"}"#).unwrap();
        assert!(!none.ok, "no card: no fan controls");
        assert!(parse_fans("nope").is_none());
    }

    #[test]
    fn a_dragged_point_stays_in_order_and_in_range() {
        let mut c = vec![[40, 30], [60, 45], [75, 75], [85, 100]];
        move_point(&mut c, 1, 90, 10, 24);
        assert_eq!(c[1], [72, 30], "not past the next point (3 C before it), not slower than the one before");
        move_point(&mut c, 1, 10, 99, 24);
        assert_eq!(c[1], [43, 75], "not before the previous point, not faster than the next");
        move_point(&mut c, 0, 0, 0, 24);
        assert_eq!(c[0], [20, 24], "the axis starts at 20 C; never below the card's lowest speed");
        move_point(&mut c, 3, 200, 200, 24);
        assert_eq!(c[3], [100, 100]);
        assert_eq!(curve_arg(&c), "curve=20:24,43:75,75:75,100:100");
    }
}
