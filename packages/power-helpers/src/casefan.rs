//! casefan: the case fans and the pump on a Corsair Commander Pro. A port of packages/settings/helpers/python/casefan with
//! the same commands, setting, output, exit codes and messages; the help text below is the Python docstring.

use std::path::{Path, PathBuf};

use crate::json::{Json, Obj};
use crate::nvml::{Card, OpenError};
use crate::py::{self, OsErr};
use crate::sys::{self, Ctx};

pub const HELP: &str = r#"casefan: the case fans and the pump on a Corsair Commander Pro (Settings > Power, next to the graphics card's fans).

Each connected port of the Commander Pro has a role:
  pump   one speed, never below PUMP_MIN_PCT (the graphics card's liquid cooler on the owner's PC)
  fan    a speed for each temperature, from two curves: one for the processor, one for the graphics card; the fan runs at
         whichever of the two asks for more, so it follows the part that is working hardest

Commands:
  casefan get                     the ports, temperatures and setting, as JSON (no root needed)
  casefan set FILE|-              save the setting from a JSON file (- reads it from stdin, as Settings sends it) and restart the
                                  service (root: pkexec)
  casefan run                     as the root service yggdrasil-casefan: keep the ports on the setting
  casefan reset [failed]          ports to their stop speed (root; the service does it when it stops): fans STOP_FAN_PCT and the
                                  pump 100 % after a clean stop, everything 100 % after a failure

Safety: the pump never runs below PUMP_MIN_PCT; at CPU_HOT_C or GPU_HOT_C every port runs at 100 %; if a temperature cannot be
read, every port goes to 100 % and the service ends with an error (systemd starts it again); after a crash (ExecStopPost with a
failed result) every port runs at 100 %. Speeds come down only once the part is SETTLE_C cooler, so fans do not hunt.

The Commander Pro keeps the last speed it was given until it loses power; it does not go back to its own stored curves by
itself, which is why a stop always sets a speed instead of leaving the ports as they were.

The setting is /etc/yggdrasil/case-fans.json:
  {"ports": {"1": {"name": "Graphics card pump", "role": "pump", "fixed": 100},
             "6": {"name": "Top exhaust", "role": "fan", "cpu": [[50, 25], ...], "gpu": [[45, 25], ...]}}}
A fan may add "follow": "5" to run on port 5's curves (the same percent; port 5 must be a fan that follows nobody).
Ports not in the setting are left alone.
"#;

pub const DRY_RUN_HELP: &str = r#"
Dry run (any command): --dry-run reads as usual and writes nothing; each write (a port's speed, the setting, systemctl) is
printed as "dry run: ...". --root DIR takes every system path under DIR and the graphics card from DIR/nvml.json (a fake tree,
no hardware at all); --steps N runs N rounds of `run` on a virtual clock, without waiting.
"#;

pub const CONF: &str = "/etc/yggdrasil/case-fans.json";
pub const SERVICE: &str = "yggdrasil-casefan.service";
pub const HWMON: &str = "/sys/class/hwmon";
pub const CPU_HOT_C: i64 = 92;
pub const GPU_HOT_C: i64 = 83;
pub const SETTLE_C: i64 = 3;
pub const EVERY_S: i64 = 2;
pub const REAPPLY_S: i64 = 30;
pub const PUMP_MIN_PCT: i64 = 60;
pub const FAN_MIN_PCT: i64 = 20;
pub const STOP_FAN_PCT: i64 = 60;
// Calm, not jumpy (my request, 2026-11-04: the exhaust rose and fell every 10-20 s as each eval case started and stopped): the
// curves read temperatures averaged over about SMOOTH_S seconds, and a fan's speed glides toward its target by at most
// RAMP_UP_PCT (faster) or RAMP_DOWN_PCT (slower) per step. The hot override still uses the raw readings and acts at once.
pub const SMOOTH_S: i64 = 30;
pub const RAMP_UP_PCT: i64 = 4;
pub const RAMP_DOWN_PCT: i64 = 1;
// Quiet when idle, firm under load: the 3000 RPM exhaust fan is about 750 RPM at 25 %.
pub const DEFAULT_CPU: [[i64; 2]; 5] = [[50, 25], [65, 35], [75, 55], [85, 80], [90, 100]];
pub const DEFAULT_GPU: [[i64; 2]; 5] = [[45, 25], [55, 35], [65, 55], [72, 80], [78, 100]];

pub type Curve = Vec<[i64; 2]>;

#[derive(Clone, Debug, PartialEq)]
pub enum Role {
    Pump { fixed: i64 },
    Fan { cpu: Curve, gpu: Curve, follow: Option<String> },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Port {
    pub name: String,
    pub role: Role,
}

impl Port {
    pub fn is_pump(&self) -> bool {
        matches!(self.role, Role::Pump { .. })
    }
}

/// The setting: the ports in the order the file lists them.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Cfg {
    pub ports: Vec<(String, Port)>,
}

impl Cfg {
    pub fn port(&self, n: &str) -> Option<&Port> {
        self.ports.iter().find(|(k, _)| k == n).map(|(_, p)| p)
    }
}

/// A reason the service cannot go on: a temperature that cannot be read, a port that cannot be written, or a setting the
/// Python helper would have crashed on.
#[derive(Clone, Debug, PartialEq)]
pub enum Fail {
    Sensor(String),
    Os(OsErr),
    Setting(String),
}

impl std::fmt::Display for Fail {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Fail::Sensor(s) | Fail::Setting(s) => f.write_str(s),
            Fail::Os(e) => write!(f, "{e}"),
        }
    }
}

impl From<OsErr> for Fail {
    fn from(e: OsErr) -> Fail {
        Fail::Os(e)
    }
}

// ---------------------------------------------------------------- hardware

/// The Commander Pro's hwmon folder, or None.
pub fn find_cpro(ctx: &Ctx, root: &Path) -> Option<PathBuf> {
    for d in ctx.names(root, "hwmon", "") {
        let d = root.join(d);
        if let Ok(name) = ctx.read_text(&d.join("name")) {
            if py::strip(&name) == "corsaircpro" {
                return Some(d);
            }
        }
    }
    None
}

/// A connected port: its label, its fan's speed, and the speed last given to it (None when the driver has none to report).
#[derive(Clone, Debug, PartialEq)]
pub struct Connected {
    pub label: String,
    pub rpm: i64,
    pub pct: Option<i64>,
}

/// The connected ports (the driver lists only connected ones), in the order sorted(glob("fan*_label")) gives.
pub fn ports(ctx: &Ctx, cpro: &Path) -> Vec<(String, Connected)> {
    let mut out = Vec::new();
    for lab in ctx.names(cpro, "fan", "_label") {
        let n = lab["fan".len()..lab.len() - "_label".len()].to_string();
        let Ok(label) = ctx.read_text(&cpro.join(&lab)) else { continue };
        let Some(rpm) = ctx.read_int(&cpro.join(format!("fan{n}_input"))) else { continue };
        let pct = ctx.read_int(&cpro.join(format!("pwm{n}"))).map(|v| py::round(v as f64 * 100.0 / 255.0));
        out.push((n, Connected { label: py::strip(&label).to_string(), rpm, pct }));
    }
    out
}

pub fn set_pct(ctx: &Ctx, cpro: &Path, n: &str, pct: i64) -> Result<(), OsErr> {
    let v = py::round(pct.clamp(0, 100) as f64 * 255.0 / 100.0);
    ctx.write_text(&cpro.join(format!("pwm{n}")), &v.to_string())
}

/// The processor's package temperature (coretemp "Package id 0", or k10temp Tctl), in degrees C.
pub fn cpu_temp(ctx: &Ctx, root: &Path) -> Result<i64, Fail> {
    for d in ctx.names(root, "hwmon", "") {
        let d = root.join(d);
        let Ok(name) = ctx.read_text(&d.join("name")) else { continue };
        if !["coretemp", "k10temp", "zenpower"].contains(&py::strip(&name)) {
            continue;
        }
        for lab in ctx.names(&d, "temp", "_label") {
            let Ok(label) = ctx.read_text(&d.join(&lab)) else { continue };
            let label = py::strip(&label);
            if label.starts_with("Package id") || label == "Tctl" || label == "Tdie" {
                let input = d.join(lab.replace("_label", "_input"));
                if let Some(v) = ctx.read_int(&input) {
                    return Ok(v.div_euclid(1000));
                }
            }
        }
    }
    Err(Fail::Sensor("the processor's temperature cannot be read".into()))
}

/// The first NVIDIA card's temperature, through libnvidia-ml (no X server needed).
pub struct Gpu(Card);

impl Gpu {
    pub fn open(ctx: &Ctx) -> Result<Gpu, Fail> {
        match Card::open(ctx) {
            Ok(c) => Ok(Gpu(c)),
            Err(OpenError::Library(e)) => Err(Fail::Sensor(format!("libnvidia-ml is not available ({e})"))),
            Err(_) => Err(Fail::Sensor("the graphics card cannot be reached through NVML".into())),
        }
    }

    pub fn temp(&self) -> Result<i64, Fail> {
        self.0.temp().map(|t| t as i64).map_err(|_| Fail::Sensor("the graphics card's temperature cannot be read".into()))
    }
}

// ---------------------------------------------------------------- the setting (pure, tested)

fn curve_json(c: &[[i64; 2]]) -> Json {
    Json::Arr(c.iter().map(|p| Json::Arr(vec![Json::Int(p[0]), Json::Int(p[1])])).collect())
}

pub fn check_curve(pts: &Json) -> Result<Curve, String> {
    let a = match pts {
        Json::Arr(a) if (2..=8).contains(&a.len()) => a,
        _ => return Err("a curve has 2 to 8 points".into()),
    };
    let mut out = Vec::new();
    for pt in a {
        let (t, p) = match pt.arr().map(|v| v.as_slice()) {
            Some([t, p]) => match (t.int(), p.int()) {
                (Some(t), Some(p)) => (t, p),
                _ => return Err("each point is [degrees, percent]".into()),
            },
            _ => return Err("each point is [degrees, percent]".into()),
        };
        if !(20..=100).contains(&t) || !(0..=100).contains(&p) {
            return Err("temperatures are 20-100 C and speeds 0-100 %".into());
        }
        out.push([t, p]);
    }
    for w in out.windows(2) {
        if w[1][0] <= w[0][0] {
            return Err("the temperatures must rise from point to point".into());
        }
        if w[1][1] < w[0][1] {
            return Err("the speed may not drop as the temperature rises".into());
        }
    }
    Ok(out)
}

/// The saved setting with anything unusable replaced by a safe default; a port with a bad entry keeps its role's default.
/// Err where the Python helper would have crashed ("ports" that is not an object): the caller treats it as a failure.
pub fn clean(d: &Json) -> Result<Cfg, String> {
    let empty = Obj::new();
    let d = d.obj().unwrap_or(&empty);
    let raw = match d.at("ports") {
        Json::Obj(o) => o.clone(),
        v if !v.truthy() => Obj::new(),
        v => return Err(format!("'{}' object has no attribute 'items'", v.type_name())),
    };
    let mut out = Cfg::default();
    for (n, p) in raw.iter() {
        let Some(p) = p.obj() else { continue };
        if !(py::isdigit(n) && n.parse::<u64>().is_ok_and(|v| (1..=6).contains(&v))) {
            continue;
        }
        let name: String = match p.at("name") {
            Json::Str(s) => s.chars().take(60).collect(),
            _ => format!("Port {n}").chars().take(60).collect(),
        };
        let role = if p.at("role").str() == Some("pump") {
            let fixed = p.at("fixed").int().unwrap_or(100);
            Role::Pump { fixed: fixed.clamp(i64::MIN, 100).max(PUMP_MIN_PCT) }
        } else {
            Role::Fan {
                cpu: check_curve(p.at("cpu")).unwrap_or_else(|_| DEFAULT_CPU.to_vec()),
                gpu: check_curve(p.at("gpu")).unwrap_or_else(|_| DEFAULT_GPU.to_vec()),
                follow: None,
            }
        };
        out.ports.push((n.to_string(), Port { name, role }));
    }
    // a fan may follow another fan ("follow": "5"): it gets that fan's curves, so both run at the same percent (each at its own
    // top speed). Only a fan that follows nobody can be followed: no chains, no loops.
    for i in 0..out.ports.len() {
        let n = out.ports[i].0.clone();
        let Some(lead) = raw.at(&n).obj().and_then(|p| p.at("follow").str()).map(String::from) else { continue };
        if out.ports[i].1.is_pump() || lead == n {
            continue;
        }
        let Some(leader) = out.port(&lead).cloned() else { continue };
        let Role::Fan { cpu, gpu, .. } = leader.role else { continue };
        if raw.at(&lead).obj().is_some_and(|p| p.at("follow").str().is_some()) {
            continue;
        }
        out.ports[i].1.role = Role::Fan { cpu, gpu, follow: Some(lead) };
    }
    Ok(out)
}

/// The setting as JSON, in the Python helper's key order.
pub fn to_json(cfg: &Cfg) -> Json {
    let mut ports = Obj::new();
    for (n, p) in &cfg.ports {
        let mut o = Obj::new().with("name", p.name.as_str().into());
        match &p.role {
            Role::Pump { fixed } => {
                o = o.with("role", "pump".into()).with("fixed", (*fixed).into());
            }
            Role::Fan { cpu, gpu, follow } => {
                o = o.with("role", "fan".into()).with("cpu", curve_json(cpu)).with("gpu", curve_json(gpu));
                if let Some(l) = follow {
                    o = o.with("follow", l.as_str().into());
                }
            }
        }
        ports.set(n, o.into());
    }
    Obj::new().with("ports", ports.into()).into()
}

pub fn load(ctx: &Ctx, path: &Path) -> Result<Cfg, String> {
    match ctx.read_json(path) {
        Ok(v) => clean(&v),
        Err(_) => clean(&Json::Null),
    }
}

pub fn save(ctx: &Ctx, cfg: &Cfg, path: &Path) -> Result<(), OsErr> {
    ctx.write_json(path, &to_json(cfg))
}

pub fn curve_at(pts: &[[i64; 2]], t: i64) -> i64 {
    if t <= pts[0][0] {
        return pts[0][1];
    }
    for w in pts.windows(2) {
        let ([t0, p0], [t1, p1]) = (w[0], w[1]);
        if t <= t1 {
            return py::round(p0 as f64 + ((p1 - p0) * (t - t0)) as f64 / (t1 - t0) as f64);
        }
    }
    pts[pts.len() - 1][1]
}

/// The speed a port should run at, with the processor at cpu and the card at gpu degrees.
pub fn target(port: &Port, cpu: i64, gpu: i64) -> i64 {
    if cpu >= CPU_HOT_C || gpu >= GPU_HOT_C {
        return 100;
    }
    match &port.role {
        Role::Pump { fixed } => PUMP_MIN_PCT.max(*fixed),
        Role::Fan { cpu: c, gpu: g, .. } => FAN_MIN_PCT.max(100.min(curve_at(c, cpu).max(curve_at(g, gpu)))),
    }
}

/// last = (speed, heat when it was set), heat = the hotter part's margin. Up at once; down only SETTLE_C later.
pub fn settle(last: Option<(i64, f64)>, want: i64, heat: f64) -> i64 {
    match last {
        Some((speed, h)) if !(want >= speed || heat <= h - SETTLE_C as f64) => speed,
        _ => want,
    }
}

/// An exponential running average: about SMOOTH_S seconds of memory at one reading every EVERY_S seconds.
pub fn smooth(avg: Option<f64>, t: i64) -> f64 {
    match avg {
        None => t as f64,
        Some(a) => a + (t as f64 - a) * (EVERY_S as f64 / SMOOTH_S as f64).min(1.0),
    }
}

/// The next speed on the way from cur to want: up by RAMP_UP_PCT, down by RAMP_DOWN_PCT at most.
pub fn ramp(cur: Option<i64>, want: i64) -> i64 {
    match cur {
        None => want,
        Some(c) if want > c => want.min(c + RAMP_UP_PCT),
        Some(c) => want.max(c - RAMP_DOWN_PCT),
    }
}

/// The stop speed of a port (None: a port the setting does not name, taken as a fan).
pub fn stop_speed(port: Option<&Port>, failed: bool) -> i64 {
    if failed || port.is_some_and(Port::is_pump) {
        100
    } else {
        STOP_FAN_PCT
    }
}

// ---------------------------------------------------------------- the service's loop

/// Everything the loop needs from outside, so tests can drive it.
pub trait Env {
    /// The setting when it changed since the last call (the first call always gives it), else None. Err: an unusable setting.
    fn read_cfg(&mut self) -> Option<Result<Cfg, String>>;
    /// The setting file as it is (when read_cfg gives nothing at the start).
    fn load(&mut self) -> Result<Cfg, String>;
    fn read_cpu(&mut self) -> Result<i64, Fail>;
    fn read_gpu(&mut self) -> Result<i64, Fail>;
    fn write(&mut self, port: &str, pct: i64) -> Result<(), OsErr>;
    fn sleep(&mut self, s: i64);
    fn should_stop(&mut self) -> bool;
    fn clock(&mut self) -> f64;
}

/// The service's loop. Returns the exit code; on a sensor or write failure every configured port is set to 100 % first.
pub fn run(env: &mut dyn Env) -> i32 {
    let first = match env.read_cfg() {
        Some(r) => r,
        None => env.load(),
    };
    let mut cfg = match first {
        Ok(c) => c,
        Err(e) => {
            crate::warn!("casefan: {e}");
            return 1;
        }
    };
    let mut last: Vec<(String, (i64, f64))> = Vec::new();
    let mut cur: Vec<(String, i64)> = Vec::new();
    let (mut applied, mut avg_cpu, mut avg_gpu) = (0.0f64, None, None);
    let get = |v: &Vec<(String, i64)>, n: &str| v.iter().find(|(k, _)| k == n).map(|(_, s)| *s);
    let result: Result<(), Fail> = (|| {
        while !env.should_stop() {
            if let Some(new) = env.read_cfg() {
                // the Python helper crashed here on an unusable setting, leaving the ports as they were; this one fails safe
                cfg = new.map_err(Fail::Setting)?;
                last.clear();
            }
            let cpu = env.read_cpu()?;
            let gpu = env.read_gpu()?;
            let (ac, ag) = (smooth(avg_cpu, cpu), smooth(avg_gpu, gpu));
            avg_cpu = Some(ac);
            avg_gpu = Some(ag);
            let hot = cpu >= CPU_HOT_C || gpu >= GPU_HOT_C; // raw readings: the override never waits for the average
            let heat = (ac - CPU_HOT_C as f64).max(ag - GPU_HOT_C as f64); // how close the hotter part is to its limit
            let again = env.clock() - applied >= REAPPLY_S as f64;
            for (n, port) in &cfg.ports {
                let prev = last.iter().find(|(k, _)| k == n).map(|(_, l)| *l);
                let speed = if hot {
                    put(&mut last, n, (100, heat));
                    100
                } else {
                    let want = settle(prev, target(port, py::round(ac), py::round(ag)), heat);
                    if prev.is_none_or(|p| want != p.0) {
                        put(&mut last, n, (want, heat));
                    }
                    if port.is_pump() {
                        want
                    } else {
                        ramp(get(&cur, n), want)
                    }
                };
                if get(&cur, n) != Some(speed) || again {
                    env.write(n, speed)?;
                    put(&mut cur, n, speed);
                }
            }
            if again {
                applied = env.clock();
            }
            env.sleep(EVERY_S);
        }
        Ok(())
    })();
    match result {
        Ok(()) => 0,
        Err(e) => {
            crate::warn!("casefan: {e}; every port is at full speed");
            for (n, _) in &cfg.ports {
                let _ = env.write(n, 100);
            }
            1
        }
    }
}

fn put<T>(v: &mut Vec<(String, T)>, k: &str, x: T) {
    match v.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = x,
        None => v.push((k.to_string(), x)),
    }
}

// ---------------------------------------------------------------- commands

fn hwmon(ctx: &Ctx) -> PathBuf {
    ctx.path(HWMON)
}

fn conf(ctx: &Ctx) -> PathBuf {
    ctx.path(CONF)
}

fn systemctl_ask(ctx: &Ctx, args: &[&str]) -> i32 {
    ctx.ask("systemctl", args).map(|(c, _)| c).unwrap_or(-1)
}

pub fn cmd_get(ctx: &Ctx) -> i32 {
    let cfg = match load(ctx, &conf(ctx)) {
        Ok(c) => c,
        Err(e) => {
            crate::warn!("casefan: {e}");
            return 1;
        }
    };
    let cpro = find_cpro(ctx, &hwmon(ctx));
    let mut out = match to_json(&cfg) {
        Json::Obj(o) => o,
        _ => Obj::new(),
    };
    let connected = cpro.as_ref().map(|c| {
        let mut o = Obj::new();
        for (n, p) in ports(ctx, c) {
            o.set(&n, Obj::new().with("label", p.label.into()).with("rpm", p.rpm.into()).with("pct", p.pct.into()).into());
        }
        Json::Obj(o)
    });
    out.set("connected", connected.unwrap_or(Json::Null));
    out.set("cpu", Json::Null);
    out.set("gpu", Json::Null);
    out.set("running", (systemctl_ask(ctx, &["is-active", "--quiet", SERVICE]) == 0).into());
    out.set("hot", Obj::new().with("cpu", CPU_HOT_C.into()).with("gpu", GPU_HOT_C.into()).into());
    match cpu_temp(ctx, &hwmon(ctx)) {
        Ok(t) => out.set("cpu", t.into()),
        Err(e) => out.set("error", e.to_string().into()),
    }
    match Gpu::open(ctx).and_then(|g| g.temp()) {
        Ok(t) => out.set("gpu", t.into()),
        Err(e) => out.set("error", e.to_string().into()),
    }
    crate::out!("{}", crate::json::dumps(&out.into()));
    0
}

/// Reads the setting from a file or ("-") from stdin, at most 65536 characters as sys.stdin.read(65536) did.
fn read_setting(path: &str, stdin: &mut dyn std::io::Read) -> Result<Json, String> {
    use std::io::Read;
    let text = if path == "-" {
        let mut b = Vec::new();
        stdin.take(65536 * 4).read_to_end(&mut b).map_err(|e| e.to_string())?;
        let s = String::from_utf8(b).map_err(|e| e.to_string())?;
        s.chars().take(65536).collect::<String>()
    } else {
        let b = std::fs::read(path).map_err(|e| OsErr::at(&e, Path::new(path)).to_string())?;
        String::from_utf8(b).map_err(|e| e.to_string())?
    };
    crate::json::parse(&text)
}

pub fn cmd_set(ctx: &Ctx, path: &str) -> i32 {
    cmd_set_from(ctx, path, &mut std::io::stdin())
}

/// `set`, with the standard input passed in (tests give it a text).
pub fn cmd_set_from(ctx: &Ctx, path: &str, stdin: &mut dyn std::io::Read) -> i32 {
    if !ctx.is_root() {
        crate::warn!("casefan: set needs root (Settings runs it with pkexec)");
        return 1;
    }
    let d = match read_setting(path, stdin) {
        Ok(d) => d,
        Err(e) => {
            crate::warn!("casefan: {e}");
            return 2;
        }
    };
    let cfg = match clean(&d) {
        Ok(c) => c,
        Err(e) => {
            crate::warn!("casefan: {e}");
            return 1;
        }
    };
    if let Err(e) = save(ctx, &cfg, &conf(ctx)) {
        crate::warn!("casefan: {e}");
        return 1;
    }
    ctx.act("systemctl", &["enable", SERVICE]);
    if ctx.act("systemctl", &["restart", SERVICE]) == 0 {
        0
    } else {
        1
    }
}

pub fn cmd_reset(ctx: &Ctx, failed: bool) -> i32 {
    if !ctx.is_root() {
        crate::warn!("casefan: reset needs root");
        return 1;
    }
    let Some(cpro) = find_cpro(ctx, &hwmon(ctx)) else { return 0 };
    // an unusable setting (the Python helper crashed here and left the ports as they were): there is no telling which port is
    // the pump, so every port goes to 100 %, as after a failure
    let cfg = load(ctx, &conf(ctx));
    let failed = failed || cfg.is_err();
    let cfg = cfg.unwrap_or_default();
    for (n, _) in ports(ctx, &cpro) {
        let _ = set_pct(ctx, &cpro, &n, stop_speed(cfg.port(&n), failed));
    }
    0
}

struct Service<'a> {
    ctx: &'a Ctx,
    cpro: PathBuf,
    gpu: Gpu,
    seen: Option<Option<(i64, i64)>>,
}

impl Env for Service<'_> {
    fn read_cfg(&mut self) -> Option<Result<Cfg, String>> {
        let m = self.ctx.mtime(&conf(self.ctx));
        if self.seen == Some(m) {
            return None;
        }
        self.seen = Some(m);
        Some(load(self.ctx, &conf(self.ctx)))
    }

    fn load(&mut self) -> Result<Cfg, String> {
        load(self.ctx, &conf(self.ctx))
    }

    fn read_cpu(&mut self) -> Result<i64, Fail> {
        cpu_temp(self.ctx, &hwmon(self.ctx))
    }

    fn read_gpu(&mut self) -> Result<i64, Fail> {
        self.gpu.temp()
    }

    fn write(&mut self, port: &str, pct: i64) -> Result<(), OsErr> {
        set_pct(self.ctx, &self.cpro, port, pct)
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
        crate::warn!("casefan: run is the root service (yggdrasil-casefan)");
        return 1;
    }
    let Some(cpro) = find_cpro(ctx, &hwmon(ctx)) else {
        crate::warn!("casefan: no Corsair Commander Pro (corsair_cpro driver) found");
        return 1;
    };
    sys::catch_stop_signals();
    let gpu = match Gpu::open(ctx) {
        Ok(g) => g,
        Err(e) => {
            crate::warn!("casefan: {e}");
            return 1;
        }
    };
    run(&mut Service { ctx, cpro, gpu, seen: None })
}

pub fn main(args: Vec<String>) -> i32 {
    let (ctx, argv) = match Ctx::from_args(args) {
        Ok(x) => x,
        Err(e) => {
            crate::warn!("casefan: {e}");
            return 2;
        }
    };
    if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
        crate::out!("{HELP}{DRY_RUN_HELP}");
        return if argv.len() >= 2 { 0 } else { 2 };
    }
    let c = argv[1].as_str();
    match (c, argv.len()) {
        ("get", 2) => cmd_get(&ctx),
        ("set", 3) => cmd_set(&ctx, &argv[2]),
        ("run", 2) => cmd_run(&ctx),
        ("reset", 2 | 3) => {
            let failed = (argv.len() == 3 && argv[2] == "failed") || std::env::var("SERVICE_RESULT").unwrap_or_else(|_| "success".into()) != "success";
            cmd_reset(&ctx, failed)
        }
        _ => {
            crate::warn!("casefan: unknown command (see --help)");
            2
        }
    }
}

#[cfg(test)]
mod tests;
