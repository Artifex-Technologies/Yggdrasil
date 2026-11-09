//! gpufan: the NVIDIA graphics card's fans. A port of packages/settings/helpers/python/gpufan with the same commands,
//! setting, output, exit codes and messages; the help text below is the Python docstring.

use std::path::{Path, PathBuf};

use crate::json::{Json, Obj};
use crate::nvml::{self, OpenError};
use crate::py::{self, OsErr};
use crate::sys::{self, Ctx};

pub const HELP: &str = r#"gpufan: the NVIDIA graphics card's fans (Settings > Power > Graphics card).

Modes:
  auto     the card's own control (the default; nothing of ours runs)
  fixed    one speed, in percent
  curve    a speed for each temperature: points (degrees C, percent) joined by straight lines

Commands:
  gpufan get                                         the fans now and the setting, as JSON (no root needed)
  gpufan set auto | fixed=PCT | curve=T:P,T:P,...    save the setting and start or stop the service (root: pkexec)
  gpufan run                                         as the root service yggdrasil-gpufan: keep the fans on the setting
  gpufan reset                                       give the fans back to the card (root; the service does it when it stops)

Safety: never below the card's own minimum; at HOT_C or more the fans run at 100 % whatever the setting; if the temperature
cannot be read the fans go back to the card and the service ends with an error (systemd starts it again); whenever the service
stops, also after a crash (ExecStopPost), the fans go back to the card. Nothing here touches clocks or power.

The setting is /etc/yggdrasil/gpu-fan.json: {"mode": "curve", "fixed": 60, "curve": [[40, 30], [60, 45], [75, 75], [85, 100]]}.
Fans are driven through NVML (libnvidia-ml), which needs no X server and works on Wayland.
"#;

pub const DRY_RUN_HELP: &str = r#"
Dry run (any command): --dry-run reads as usual and writes nothing; each write (a fan's speed, the setting, systemctl) is
printed as "dry run: ...". --root DIR takes every system path under DIR and the card from DIR/nvml.json (a fake tree, no
hardware at all); --steps N runs N rounds of `run` on a virtual clock, without waiting.
"#;

pub const CONF: &str = "/etc/yggdrasil/gpu-fan.json";
pub const SERVICE: &str = "yggdrasil-gpufan.service";
pub const HOT_C: i64 = 85;
/// a lower speed waits until the card is this much cooler than when the speed was last raised
pub const SETTLE_C: i64 = 3;
pub const EVERY_S: i64 = 2;
/// the speed is set again now and then (after a suspend the card may have taken its fans back)
pub const REAPPLY_S: f64 = 30.0;
pub const DEFAULT_CURVE: [[i64; 2]; 4] = [[40, 30], [60, 45], [75, 75], [85, 100]];
pub const MODES: [&str; 3] = ["auto", "fixed", "curve"];

pub type Curve = Vec<[i64; 2]>;

#[derive(Clone, Debug, PartialEq)]
pub struct Cfg {
    pub mode: String,
    pub fixed: i64,
    pub curve: Curve,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NvmlError(pub String);

/// The card as the loop uses it (tests give a fake one).
pub trait Card {
    fn fans(&mut self) -> Result<u32, NvmlError>;
    fn minmax(&mut self) -> Result<(i64, i64), NvmlError>;
    fn temp(&mut self) -> Result<i64, NvmlError>;
    fn set_speed(&mut self, i: u32, pct: i64) -> Result<(), NvmlError>;
    fn set_default(&mut self, i: u32) -> Result<(), NvmlError>;
}

/// The first NVIDIA card, through libnvidia-ml.
pub struct Nvml(nvml::Card);

fn failed(name: &str, code: i32) -> NvmlError {
    NvmlError(format!("{name} failed (NVML error {code})"))
}

impl Nvml {
    pub fn open(ctx: &Ctx) -> Result<Nvml, NvmlError> {
        match nvml::Card::open(ctx) {
            Ok(c) => Ok(Nvml(c)),
            Err(OpenError::Library(e)) => Err(NvmlError(format!("libnvidia-ml is not available ({e})"))),
            Err(OpenError::Init(c)) => Err(failed("nvmlInit_v2", c)),
            Err(OpenError::Handle(c)) => Err(failed("nvmlDeviceGetHandleByIndex_v2", c)),
        }
    }

    pub fn speed(&mut self, i: u32) -> Result<i64, NvmlError> {
        self.0.fan_speed(i).map(|v| v as i64).map_err(|c| failed("nvmlDeviceGetFanSpeed_v2", c))
    }
}

impl Card for Nvml {
    fn fans(&mut self) -> Result<u32, NvmlError> {
        self.0.num_fans().map_err(|c| failed("nvmlDeviceGetNumFans", c))
    }
    fn minmax(&mut self) -> Result<(i64, i64), NvmlError> {
        self.0.min_max_fan_speed().map(|(a, b)| (a as i64, b as i64)).map_err(|c| failed("nvmlDeviceGetMinMaxFanSpeed", c))
    }
    fn temp(&mut self) -> Result<i64, NvmlError> {
        self.0.temp().map(|v| v as i64).map_err(|c| failed("nvmlDeviceGetTemperature", c))
    }
    fn set_speed(&mut self, i: u32, pct: i64) -> Result<(), NvmlError> {
        self.0.set_fan_speed(i, pct.clamp(0, u32::MAX as i64) as u32).map_err(|c| failed("nvmlDeviceSetFanSpeed_v2", c))
    }
    fn set_default(&mut self, i: u32) -> Result<(), NvmlError> {
        self.0.set_default_fan_speed(i).map_err(|c| failed("nvmlDeviceSetDefaultFanSpeed_v2", c))
    }
}

// ---------------------------------------------------------------- the setting (pure, tested)

/// '40:30,60:45' -> [[40, 30], [60, 45]]; Err when it is not a usable curve.
pub fn parse_curve(text: &str) -> Result<Curve, String> {
    let mut pts = Vec::new();
    for part in text.split(',') {
        let s = py::strip(part);
        let (t, p) = match s.split_once(':') {
            Some((t, p)) if py::isdigit(py::strip(t)) && py::isdigit(py::strip(p)) => (t, p),
            _ => return Err(format!("{} is not TEMPERATURE:PERCENT", py::repr(part))),
        };
        pts.push(Json::Arr(vec![Json::Int(py::int(t)?), Json::Int(py::int(p)?)]));
    }
    check_curve(&Json::Arr(pts))
}

pub fn check_curve(pts: &Json) -> Result<Curve, String> {
    crate::casefan::check_curve(pts)
}

/// The saved setting with anything unusable replaced by the default.
pub fn clean(d: &Json) -> Cfg {
    let empty = Obj::new();
    let d = d.obj().unwrap_or(&empty);
    let mode = d.at("mode").str().filter(|m| MODES.contains(m)).unwrap_or("auto").to_string();
    // isinstance(fixed, int) took True and False too (as 1 and 0)
    let fixed = d.at("fixed").int_or_bool().filter(|f| (0..=100).contains(f)).unwrap_or(60);
    let curve = check_curve(d.at("curve")).unwrap_or_else(|_| DEFAULT_CURVE.to_vec());
    Cfg { mode, fixed, curve }
}

pub fn to_json(cfg: &Cfg) -> Obj {
    let curve = Json::Arr(cfg.curve.iter().map(|p| Json::Arr(vec![Json::Int(p[0]), Json::Int(p[1])])).collect());
    Obj::new().with("mode", cfg.mode.as_str().into()).with("fixed", cfg.fixed.into()).with("curve", curve)
}

pub fn load(ctx: &Ctx, path: &Path) -> Cfg {
    match ctx.read_json(path) {
        Ok(v) => clean(&v),
        Err(_) => clean(&Json::Null),
    }
}

pub fn save(ctx: &Ctx, cfg: &Cfg, path: &Path) -> Result<(), OsErr> {
    ctx.write_json(path, &to_json(&clean(&to_json(cfg).into())).into())
}

/// The curve's speed at t degrees: the first point's below it, the last point's above it, straight lines between.
pub fn curve_at(pts: &[[i64; 2]], t: i64) -> i64 {
    crate::casefan::curve_at(pts, t)
}

/// The speed the setting asks for at t degrees, never below the card's minimum, always 100 % when hot.
pub fn target(cfg: &Cfg, t: i64, lo: i64) -> i64 {
    if t >= HOT_C {
        return 100;
    }
    let want = if cfg.mode == "fixed" { cfg.fixed } else { curve_at(&cfg.curve, t) };
    lo.max(100.min(want))
}

/// last = (speed, temperature when it was set). Up at once; down only once the card is SETTLE_C cooler, so the fans do not
/// hunt up and down around a point of the curve.
pub fn settle(last: Option<(i64, i64)>, want: i64, t: i64) -> i64 {
    match last {
        Some((speed, at)) if !(want >= speed || t <= at - SETTLE_C) => speed,
        _ => want,
    }
}

// ---------------------------------------------------------------- the service's loop

/// Everything the loop needs from outside besides the card, so tests can drive it.
pub trait Env {
    /// The setting when it changed since the last call (None while unchanged); a new setting applies at once.
    fn read_cfg(&mut self) -> Option<Cfg>;
    fn load(&mut self) -> Cfg;
    fn sleep(&mut self, s: i64);
    fn should_stop(&mut self) -> bool;
    fn clock(&mut self) -> f64;
}

/// Every fan back to the card's own control (errors ignored: this is the way out).
pub fn give_back(nv: &mut dyn Card) {
    if let Ok(n) = nv.fans() {
        for i in 0..n {
            let _ = nv.set_default(i);
        }
    }
}

/// The service's loop. Returns the exit code; the fans are given back to the card on every way out.
pub fn run(nv: &mut dyn Card, env: &mut dyn Env) -> i32 {
    let result: Result<(), NvmlError> = (|| {
        let n = nv.fans()?;
        let (lo, _) = nv.minmax()?;
        let (mut last, mut applied): (Option<(i64, i64)>, f64) = (None, 0.0);
        let mut cfg = match env.read_cfg() {
            Some(c) => c,
            None => env.load(),
        };
        while !env.should_stop() {
            if let Some(new) = env.read_cfg() {
                cfg = new;
                last = None;
            }
            if cfg.mode == "auto" {
                return Ok(());
            }
            let t = nv.temp()?;
            let want = settle(last, target(&cfg, t, lo), t);
            if last.is_none_or(|l| want != l.0) || env.clock() - applied >= REAPPLY_S {
                for i in 0..n {
                    nv.set_speed(i, want)?;
                }
                if last.is_none_or(|l| want != l.0) {
                    last = Some((want, t));
                }
                applied = env.clock();
            }
            env.sleep(EVERY_S);
        }
        Ok(())
    })();
    let code = match result {
        Ok(()) => 0,
        Err(NvmlError(e)) => {
            crate::warn!("gpufan: {e}; the fans are back on the card's own control");
            1
        }
    };
    give_back(nv);
    code
}

// ---------------------------------------------------------------- commands

fn conf(ctx: &Ctx) -> PathBuf {
    ctx.path(CONF)
}

pub fn cmd_get(ctx: &Ctx) -> i32 {
    let cfg = load(ctx, &conf(ctx));
    let running = ctx.ask("systemctl", &["is-active", "--quiet", SERVICE]).map(|(c, _)| c) == Some(0);
    let mut out = to_json(&cfg)
        .with("fans", Json::Null)
        .with("min", Json::Null)
        .with("max", Json::Null)
        .with("temp", Json::Null)
        .with("running", running.into())
        .with("hot", HOT_C.into());
    let r: Result<(), NvmlError> = (|| {
        let mut nv = Nvml::open(ctx)?;
        let n = nv.fans()?;
        let mut fans = Vec::new();
        for i in 0..n {
            fans.push(Json::Int(nv.speed(i)?));
        }
        out.set("fans", Json::Arr(fans));
        let (lo, hi) = nv.minmax()?;
        out.set("min", lo.into());
        out.set("max", hi.into());
        out.set("temp", nv.temp()?.into());
        Ok(())
    })();
    if let Err(NvmlError(e)) = r {
        out.set("error", e.into());
    }
    crate::out!("{}", crate::json::dumps(&out.into()));
    0
}

pub fn cmd_set(ctx: &Ctx, arg: &str) -> i32 {
    if !ctx.is_root() {
        crate::warn!("gpufan: set needs root (Settings runs it with pkexec)");
        return 1;
    }
    let mut cfg = load(ctx, &conf(ctx));
    let r: Result<(), String> = (|| {
        if arg == "auto" {
            cfg.mode = "auto".into();
        } else if let Some(v) = arg.strip_prefix("fixed=") {
            if !py::isdigit(v) || !(0..=100).contains(&py::int(v)?) {
                return Err("fixed needs a percent, 0-100".into());
            }
            cfg.mode = "fixed".into();
            cfg.fixed = py::int(v)?;
        } else if let Some(v) = arg.strip_prefix("curve=") {
            let c = parse_curve(v)?;
            cfg.mode = "curve".into();
            cfg.curve = c;
        } else {
            return Err("use auto, fixed=PCT or curve=T:P,T:P,...".into());
        }
        Ok(())
    })();
    if let Err(e) = r {
        crate::warn!("gpufan: {e}");
        return 2;
    }
    if let Err(e) = save(ctx, &cfg, &conf(ctx)) {
        crate::warn!("gpufan: {e}");
        return 1;
    }
    if cfg.mode == "auto" {
        ctx.act("systemctl", &["disable", "--now", SERVICE]); // stopping it gives the fans back (ExecStopPost)
        return cmd_reset(ctx);
    }
    ctx.act("systemctl", &["enable", SERVICE]);
    if ctx.act("systemctl", &["restart", SERVICE]) == 0 {
        0
    } else {
        1
    }
}

pub fn cmd_reset(ctx: &Ctx) -> i32 {
    if !ctx.is_root() {
        crate::warn!("gpufan: reset needs root");
        return 1;
    }
    let Ok(mut nv) = Nvml::open(ctx) else { return 0 };
    give_back(&mut nv);
    0
}

struct Service<'a> {
    ctx: &'a Ctx,
    seen: Option<Option<(i64, i64)>>,
}

impl Env for Service<'_> {
    fn read_cfg(&mut self) -> Option<Cfg> {
        let m = self.ctx.mtime(&conf(self.ctx));
        if self.seen == Some(m) {
            return None;
        }
        self.seen = Some(m);
        Some(load(self.ctx, &conf(self.ctx)))
    }
    fn load(&mut self) -> Cfg {
        load(self.ctx, &conf(self.ctx))
    }
    fn sleep(&mut self, s: i64) {
        self.ctx.sleep(s as f64)
    }
    fn should_stop(&mut self) -> bool {
        self.ctx.should_stop()
    }
    fn clock(&mut self) -> f64 {
        self.ctx.monotonic()
    }
}

pub fn cmd_run(ctx: &Ctx) -> i32 {
    if !ctx.is_root() {
        crate::warn!("gpufan: run is the root service (yggdrasil-gpufan)");
        return 1;
    }
    sys::catch_stop_signals();
    let mut nv = match Nvml::open(ctx) {
        Ok(n) => n,
        Err(NvmlError(e)) => {
            crate::warn!("gpufan: {e}");
            return 1;
        }
    };
    run(&mut nv, &mut Service { ctx, seen: None })
}

pub fn main(args: Vec<String>) -> i32 {
    let (ctx, argv) = match Ctx::from_args(args) {
        Ok(x) => x,
        Err(e) => {
            crate::warn!("gpufan: {e}");
            return 2;
        }
    };
    if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
        crate::out!("{HELP}{DRY_RUN_HELP}");
        return if argv.len() >= 2 { 0 } else { 2 };
    }
    match (argv[1].as_str(), argv.len()) {
        ("get", 2) => cmd_get(&ctx),
        ("set", 3) => cmd_set(&ctx, &argv[2]),
        ("run", 2) => cmd_run(&ctx),
        ("reset", 2) => cmd_reset(&ctx),
        _ => {
            crate::warn!("gpufan: unknown command (see --help)");
            2
        }
    }
}

#[cfg(test)]
mod tests;
