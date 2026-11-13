//! tune: the processor's turbo multipliers, power limits and turbo, and the graphics card's clock offsets, with the crash
//! guard. A port of packages/settings/helpers/python/tune with the same commands, files, output, exit codes and log lines;
//! the help text below is the Python docstring.
//!
//! One change inside `watch`: the kernel's messages (for the graphics driver's Xid errors) come straight from /dev/kmsg, not
//! from a `journalctl -k -f` child that held about 44 MB; journalctl is the fallback when /dev/kmsg cannot be opened, and
//! the service logs which one it uses.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::json::{Json, Obj};
use crate::nvml::{self, OpenError, CLOCK_GRAPHICS, CLOCK_MEM};
use crate::py::{self, OsErr};
use crate::sys::{self, Ctx};

pub const HELP: &str = r#"tune: the processor's turbo multipliers, power limits and turbo, and the graphics card's clock offsets (Settings > Power >
Processor / Graphics card, Developer options), with a crash guard.

Overclocking can freeze the computer, so changes go in two steps: `try` applies values now, for this session only; `keep` makes
them start with the computer. The guard: whenever values other than the defaults are in use, a marker with this start's boot id
is kept in /var/lib/yggdrasil/tune/tuned, and only a clean shutdown removes it. If a start finds the marker from an earlier start,
that one froze, was reset or lost power while tuned: nothing is applied (the defaults stay) and Settings says why, until
`reenable`. A driver error from the graphics card (an NVIDIA "Xid") while its clocks are tuned puts them back at once.
Power limits and turbo cannot freeze anything (lower holds back; the processor protects itself from heat) and are not guarded.

Commands (root ones through pkexec, a password):
  tune get                                     everything, as JSON (no root)
  tune try cpu ratios=R1,R2,...,R8             turbo multiplier with 1..8 cores busy (each no higher than the one before)
  tune try gpu core=MHZ mem=MHZ cap=MHZ|none   clock offsets, and a highest core clock (none: no cap)
  tune keep cpu|gpu                            the values being tried start with the computer from now on
  tune undo cpu|gpu                            back to the kept values (or the defaults) now
  tune reset cpu|gpu                           back to the defaults, now and at every start
  tune reenable                                after the guard switched tuning off: use the kept values again
  tune set power pl1=W|default pl2=W|default   the processor's long- and short-term power limits
  tune set turbo on|off
  tune watch                                   the root service yggdrasil-tune: applies at start, guards, measures package power

Files: /etc/yggdrasil/tuning.json (kept values), /var/lib/yggdrasil/tune/ (tuned, status.json, stock.json),
/run/yggdrasil/tune.json (what the service sees now, for Settings: multipliers, offsets, package power).
"#;

pub const DRY_RUN_HELP: &str = r#"
Dry run (any command): --dry-run reads as usual and writes nothing; each write (an MSR, a power limit, a clock offset, a file,
a log line) is printed as "dry run: ...". --root DIR takes every system path under DIR (/dev/cpu/N/msr can be a plain file)
and the graphics card from DIR/nvml.json (a fake tree, no hardware at all); --steps N runs N rounds of `watch` on a virtual
clock, without waiting.
"#;

pub const CONF: &str = "/etc/yggdrasil/tuning.json";
pub const STATE: &str = "/var/lib/yggdrasil/tune";
pub const RUN: &str = "/run/yggdrasil/tune.json";
pub const RAPL: &str = "/sys/class/powercap/intel-rapl:0";
pub const PSTATE: &str = "/sys/devices/system/cpu/intel_pstate";
pub const DEV_CPU: &str = "/dev/cpu";
pub const BOOT_ID: &str = "/proc/sys/kernel/random/boot_id";
pub const KMSG: &str = "/dev/kmsg";
pub const MSR_PLATFORM_INFO: u64 = 0xCE;
pub const MSR_TURBO_RATIO_LIMIT: u64 = 0x1AD;
/// multipliers may go this far above the processor's own highest turbo
pub const RATIO_HEADROOM: i64 = 3;
pub const RATIO_FLOOR: i64 = 8;
/// offsets offered, in MHz: well inside what the driver accepts, wide enough for a real overclock
pub const GPU_CORE: (i64, i64) = (-300, 300);
pub const GPU_MEM: (i64, i64) = (-1000, 2000);
pub const POWER_W: (i64, i64) = (35, 250);
pub const EVERY_S: f64 = 2.0;

/// The way out of a command where the Python helper raised an exception it did not catch: "tune: ..." and exit code 1.
#[derive(Debug, Clone, PartialEq)]
pub struct Crash(pub String);

impl From<OsErr> for Crash {
    fn from(e: OsErr) -> Crash {
        Crash(e.to_string())
    }
}

type R<T> = Result<T, Crash>;

// ---------------------------------------------------------------- multipliers (MSR_TURBO_RATIO_LIMIT)

/// MSR_TURBO_RATIO_LIMIT: byte i is the highest multiplier with i+1 cores busy.
pub fn ratios_of(value: u64) -> Vec<i64> {
    (0..8).map(|i| ((value >> (8 * i)) & 0xFF) as i64).collect()
}

pub fn value_of(ratios: &[i64]) -> u64 {
    ratios.iter().take(8).enumerate().fold(0u64, |v, (i, r)| v | (((*r & 0xFF) as u64) << (8 * i)))
}

#[derive(Debug, Clone, PartialEq)]
pub enum CheckErr {
    /// a ValueError: refused, with its message
    Value(String),
    /// a TypeError (a state file with the wrong kinds of values)
    Type(String),
}

/// max(stock) as Python takes it, for the highest multiplier allowed.
fn py_max(stock: &Json) -> Result<i64, String> {
    match stock {
        Json::Arr(a) if !a.is_empty() => {
            let mut best: Option<i64> = None;
            for x in a {
                match x.int_or_bool() {
                    Some(v) => best = Some(best.map_or(v, |b| b.max(v))),
                    None => return Err(format!("'>' not supported between instances of '{}' and 'int'", x.type_name())),
                }
            }
            Ok(best.unwrap_or(0))
        }
        Json::Arr(_) => Err("max() iterable argument is empty".into()),
        Json::Str(_) | Json::Obj(_) => Err("can only concatenate str (not \"int\") to str".into()),
        v => Err(format!("'{}' object is not iterable", v.type_name())),
    }
}

/// The multipliers asked for, as 8 numbers; Err when they are not usable.
pub fn check_ratios(ratios: &[Json], stock: &Json) -> Result<Vec<i64>, CheckErr> {
    if ratios.len() != 8 || !ratios.iter().all(|r| r.int().is_some()) {
        return Err(CheckErr::Value("eight multipliers, for 1 to 8 busy cores".into()));
    }
    let r: Vec<i64> = ratios.iter().filter_map(Json::int).collect();
    let top = (py_max(stock).map_err(CheckErr::Type)?.saturating_add(RATIO_HEADROOM)).min(60);
    if r.iter().any(|x| !(RATIO_FLOOR..=top).contains(x)) {
        return Err(CheckErr::Value(format!("multipliers go from {RATIO_FLOOR} to {top} on this processor")));
    }
    if r.windows(2).any(|w| w[1] > w[0]) {
        return Err(CheckErr::Value("a multiplier for more busy cores may not be higher than the one for fewer".into()));
    }
    Ok(r)
}

fn ints_json(v: &[i64]) -> Json {
    Json::Arr(v.iter().map(|x| Json::Int(*x)).collect())
}

// ---------------------------------------------------------------- the graphics card (NVML)

#[derive(Clone, Debug, PartialEq)]
pub struct GpuVals {
    pub core: i64,
    pub mem: i64,
    pub cap: Option<i64>,
}

pub const GPU_DEFAULT: GpuVals = GpuVals { core: 0, mem: 0, cap: None };

impl GpuVals {
    pub fn json(&self) -> Json {
        Obj::new().with("core", self.core.into()).with("mem", self.mem.into()).with("cap", self.cap.into()).into()
    }
}

pub fn check_gpu(v: &Json) -> Result<GpuVals, String> {
    let Some(o) = v.obj() else { return Err("core, mem and cap".into()) };
    let zero = Json::Int(0);
    let core = o.get("core").unwrap_or(&zero).int_or_bool().filter(|c| (GPU_CORE.0..=GPU_CORE.1).contains(c));
    let Some(core) = core else { return Err(format!("the core offset goes from {} to {} MHz", GPU_CORE.0, GPU_CORE.1)) };
    let mem = o.get("mem").unwrap_or(&zero).int_or_bool().filter(|m| (GPU_MEM.0..=GPU_MEM.1).contains(m));
    let Some(mem) = mem else { return Err(format!("the memory offset goes from {} to {} MHz", GPU_MEM.0, GPU_MEM.1)) };
    let cap = match o.at("cap") {
        Json::Null => None,
        c => match c.int_or_bool().filter(|c| (300..=3000).contains(c)) {
            Some(c) => Some(c),
            None => return Err("the clock cap is 300-3000 MHz, or none".into()),
        },
    };
    Ok(GpuVals { core, mem, cap })
}

/// `not v or (core, mem, cap) == (0, 0, None)`
pub fn is_default_gpu(v: &Json) -> bool {
    if !v.truthy() {
        return true;
    }
    let Some(o) = v.obj() else { return false };
    let zero = Json::Int(0);
    o.get("core").unwrap_or(&zero).py_eq(&zero) && o.get("mem").unwrap_or(&zero).py_eq(&zero) && o.at("cap").is_null()
}

/// The first NVIDIA card, its errors worded as tune's Python Gpu class worded them.
pub struct Gpu(nvml::Card);

fn nvml_err(what: &str, code: i32) -> OsErr {
    OsErr::Msg(format!("NVML: {what} failed (error {code})"))
}

impl Gpu {
    pub fn open(ctx: &Ctx) -> Result<Gpu, OsErr> {
        match nvml::Card::open(ctx) {
            Ok(c) => Ok(Gpu(c)),
            Err(OpenError::Library(e)) => Err(OsErr::Msg(e)),
            Err(OpenError::Init(c)) => Err(nvml_err("init", c)),
            Err(OpenError::Handle(c)) => Err(nvml_err("the card", c)),
        }
    }

    pub fn offset(&self, kind: u32) -> Result<i64, OsErr> {
        self.0.clock_offset(kind).map(|v| v as i64).map_err(|c| nvml_err("reading a clock offset", c))
    }

    pub fn set_offset(&self, kind: u32, mhz: i64) -> Result<(), OsErr> {
        self.0.set_clock_offset(kind, mhz as i32).map_err(|c| nvml_err("setting a clock offset", c))
    }

    pub fn set_cap(&self, mhz: Option<i64>) -> Result<(), OsErr> {
        match mhz {
            None => self.0.reset_locked_clocks().map_err(|c| nvml_err("removing the clock cap", c)),
            Some(m) => self.0.set_locked_clocks(m as u32).map_err(|c| nvml_err("setting the clock cap", c)),
        }
    }

    pub fn max_core(&self) -> Result<i64, OsErr> {
        self.0.max_clock(CLOCK_GRAPHICS).map(|v| v as i64).map_err(|c| nvml_err("reading the highest clock", c))
    }
}

// ---------------------------------------------------------------- the outside, replaceable for tests

/// What touches hardware or the world outside the files: the real thing by default, stand-ins in the tests (they are the
/// functions the Python tests replaced).
pub trait Hw {
    fn is_root(&self, t: &Tune) -> bool {
        t.ctx.is_root()
    }

    fn log(&self, t: &Tune, event: &str) {
        t.ctx.act("logger", &["-t", "yggdrasil-tune", "-p", "user.notice", "--", event]);
    }

    fn boot_id(&self, t: &Tune) -> String {
        t.ctx.read_text(&t.ctx.path(BOOT_ID)).map(|s| py::strip(&s).to_string()).unwrap_or_default()
    }

    fn now(&self) -> String {
        local_minute()
    }

    fn rdmsr(&self, t: &Tune, reg: u64) -> Result<u64, OsErr> {
        let p = t.ctx.path(&format!("{DEV_CPU}/0/msr"));
        let b = read_at(&p, reg)?;
        Ok(u64::from_le_bytes(b))
    }

    fn wrmsr(&self, t: &Tune, reg: u64, value: u64) -> Result<(), OsErr> {
        for c in cpus(t.ctx) {
            let p = t.ctx.path(&format!("{DEV_CPU}/{c}/msr"));
            if t.ctx.dry {
                t.ctx.say(&format!("write {} at {:#x}: {:#018x}", p.display(), reg, value));
                continue;
            }
            write_at(&p, reg, &value.to_le_bytes())?;
        }
        Ok(())
    }

    /// (base multiplier, whether the turbo multipliers may be changed) from MSR_PLATFORM_INFO.
    fn platform(&self, t: &Tune) -> Result<(i64, bool), OsErr> {
        let v = self.rdmsr(t, MSR_PLATFORM_INFO)?;
        Ok((((v >> 8) & 0xFF) as i64, v & (1 << 28) != 0))
    }

    fn apply_gpu(&self, t: &Tune, v: &GpuVals) -> Result<(), OsErr> {
        let g = Gpu::open(t.ctx)?;
        g.set_offset(CLOCK_GRAPHICS, v.core)?;
        g.set_offset(CLOCK_MEM, v.mem)?;
        g.set_cap(v.cap)
    }

    fn publish(&self, t: &Tune, power_w: Option<f64>, gpu: Option<&Gpu>) -> Result<(), OsErr> {
        t.publish_now(power_w, gpu)
    }

    fn write_text(&self, t: &Tune, path: &Path, text: &str) -> Result<(), OsErr> {
        t.ctx.write_text(path, text)
    }

    /// (long-term W, short-term W) now, from the powercap files (readable without root).
    fn rapl_limits(&self, t: &Tune) -> (Option<i64>, Option<i64>) {
        let w = |i: u8| t.ctx.read_int(&t.rapl(&format!("constraint_{i}_power_limit_uw"))).map(|v| py::round(v as f64 / 1e6));
        (w(0), w(1))
    }
}

/// The real hardware.
pub struct System;

impl Hw for System {}

fn read_at(p: &Path, at: u64) -> Result<[u8; 8], OsErr> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::File::open(p).map_err(|e| OsErr::at(&e, p))?;
    let mut b = [0u8; 8];
    let n = f.read_at(&mut b, at).map_err(|e| OsErr::Errno { code: e.raw_os_error().unwrap_or(libc::EIO), path: None, path2: None })?;
    if n != 8 {
        return Err(OsErr::Msg("unpack requires a buffer of 8 bytes".into()));
    }
    Ok(b)
}

fn write_at(p: &Path, at: u64, b: &[u8]) -> Result<(), OsErr> {
    use std::os::unix::fs::FileExt;
    let f = std::fs::OpenOptions::new().write(true).open(p).map_err(|e| OsErr::at(&e, p))?;
    f.write_at(b, at).map_err(|e| OsErr::Errno { code: e.raw_os_error().unwrap_or(libc::EIO), path: None, path2: None })?;
    Ok(())
}

/// The processors that have an msr device, in order.
fn cpus(ctx: &Ctx) -> Vec<u32> {
    let mut v: Vec<u32> = ctx.names(&ctx.path(DEV_CPU), "", "").iter().filter(|n| py::isdigit(n)).filter_map(|n| n.parse().ok()).collect();
    v.sort();
    v
}

/// time.strftime("%Y-%m-%d %H:%M"), local time.
fn local_minute() -> String {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        // tzset: glibc keeps the zone it first read; this notices a new /etc/localtime (the time zone changed)
        unsafe extern "C" {
            fn tzset();
        }
        tzset();
        libc::localtime_r(&t, &mut tm);
        format!("{:04}-{:02}-{:02} {:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min)
    }
}

// ---------------------------------------------------------------- what is kept, tried and guarded

/// The kept values: {"cpu": {"ratios": [...]} or null, "gpu": {...} or null, "power": {"pl1", "pl2"}, "turbo": bool}.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Conf {
    pub cpu: Option<Vec<i64>>,
    pub gpu: Option<GpuVals>,
    pub pl1: Option<i64>,
    pub pl2: Option<i64>,
    pub turbo: Option<bool>,
}

impl Conf {
    fn cpu_json(&self) -> Json {
        match &self.cpu {
            Some(r) => Obj::new().with("ratios", ints_json(r)).into(),
            None => Json::Null,
        }
    }

    fn gpu_json(&self) -> Json {
        self.gpu.as_ref().map(GpuVals::json).unwrap_or(Json::Null)
    }

    pub fn json(&self) -> Json {
        Obj::new()
            .with("cpu", self.cpu_json())
            .with("gpu", self.gpu_json())
            .with("power", Obj::new().with("pl1", self.pl1.into()).with("pl2", self.pl2.into()).into())
            .with("turbo", self.turbo.into())
            .into()
    }
}

/// The helper's state and commands: files under ctx's root, hardware through hw.
pub struct Tune<'a> {
    pub ctx: &'a Ctx,
    pub hw: &'a dyn Hw,
}

fn json_number(v: &Json) -> Option<f64> {
    match v {
        Json::Int(i) => Some(*i as f64),
        Json::Bool(b) => Some(*b as i64 as f64),
        Json::Float(f) => Some(*f),
        Json::Big(s) => Some(if s.starts_with('-') { f64::NEG_INFINITY } else { f64::INFINITY }),
        _ => None,
    }
}

impl<'a> Tune<'a> {
    pub fn new(ctx: &'a Ctx, hw: &'a dyn Hw) -> Tune<'a> {
        Tune { ctx, hw }
    }

    pub fn conf_path(&self) -> PathBuf {
        self.ctx.path(CONF)
    }

    pub fn state(&self, name: &str) -> PathBuf {
        self.ctx.path(STATE).join(name)
    }

    pub fn run_path(&self) -> PathBuf {
        self.ctx.path(RUN)
    }

    pub fn rapl(&self, name: &str) -> PathBuf {
        self.ctx.path(RAPL).join(name)
    }

    fn pstate(&self, name: &str) -> PathBuf {
        self.ctx.path(PSTATE).join(name)
    }

    fn log(&self, event: &str) {
        self.hw.log(self, event);
    }

    /// read_json(path, default): the object, or None when the file is missing, not JSON or not an object.
    pub fn read_json(&self, p: &Path) -> Option<Obj> {
        match self.ctx.read_json(p) {
            Ok(Json::Obj(o)) => Some(o),
            _ => None,
        }
    }

    pub fn write_json(&self, p: &Path, o: &Obj) -> Result<(), OsErr> {
        self.ctx.write_json(p, &Json::Obj(o.clone()))
    }

    pub fn stock(&self) -> Obj {
        self.read_json(&self.state("stock.json")).unwrap_or_default()
    }

    /// stock().get("ratios") or [50]
    fn stock_ratios(&self) -> Json {
        match self.stock().at("ratios") {
            r if r.truthy() => r.clone(),
            _ => Json::Arr(vec![Json::Int(50)]),
        }
    }

    pub fn conf(&self) -> Conf {
        let d = self.read_json(&self.conf_path()).unwrap_or_default();
        let mut out = Conf::default();
        if let Some(r) = d.at("cpu").obj().and_then(|c| c.at("ratios").arr()) {
            // the same range check `tune try` makes: the root service writes these to the processor at every start, so a
            // value put into the file by other means must not get there unchecked
            match check_ratios(r, &self.stock_ratios()) {
                Ok(v) => out.cpu = Some(v),
                Err(CheckErr::Value(e) | CheckErr::Type(e)) => self.log(&format!("kept multipliers ignored: {e}")),
            }
        }
        if !d.at("gpu").is_null() {
            out.gpu = check_gpu(d.at("gpu")).ok();
        }
        let empty = Obj::new();
        let p = d.at("power").obj().unwrap_or(&empty);
        let ok = |k: &str| p.at(k).int_or_bool().filter(|w| (POWER_W.0..=POWER_W.1).contains(w));
        out.pl1 = ok("pl1");
        out.pl2 = ok("pl2");
        if let Json::Bool(b) = d.at("turbo") {
            out.turbo = Some(*b);
        }
        out
    }

    pub fn status(&self) -> Obj {
        self.read_json(&self.state("status.json")).unwrap_or_default()
    }

    pub fn set_status(&self, kv: Vec<(&str, Json)>) -> Result<(), OsErr> {
        let mut s = self.status();
        for (k, v) in kv {
            s.set(k, v);
        }
        self.write_json(&self.state("status.json"), &s)
    }

    pub fn mark_tuned(&self) -> Result<(), OsErr> {
        let o = Obj::new().with("boot_id", self.hw.boot_id(self).into()).with("since", self.hw.now().into());
        self.write_json(&self.state("tuned"), &o)
    }

    /// After a change: the guard's marker exists exactly while something other than the defaults is in use, so a crash with
    /// nothing tuned never switches tuning off.
    pub fn refresh_mark(&self) -> Result<(), OsErr> {
        let st = self.status();
        if self.tuned_now(&self.conf(), &st, st.at("trial")) {
            self.mark_tuned()
        } else {
            self.ctx.remove(&self.state("tuned"))
        }
    }

    /// At start: true when the last start ended without a clean shutdown while tuned (then the marker is cleared and tuning
    /// is switched off until reenable).
    pub fn guard_check(&self, me: &str) -> Result<bool, OsErr> {
        let Some(t) = self.read_json(&self.state("tuned")) else { return Ok(false) };
        if t.at("boot_id").py_eq(&Json::Str(me.to_string())) {
            return Ok(false);
        }
        self.ctx.remove(&self.state("tuned"))?;
        let since = t.get("since").map(Json::py_str).unwrap_or_else(|| "?".into());
        let why = format!("The computer did not shut down cleanly while tuned (from {since}), so this start uses the defaults");
        self.set_status(vec![("cpu_off", why.as_str().into()), ("gpu_off", why.as_str().into()), ("trial", Json::Null)])?;
        self.log(&why);
        Ok(true)
    }

    /// Whether any value other than the defaults is in use.
    pub fn tuned_now(&self, c: &Conf, st: &Obj, trial: &Json) -> bool {
        let tried = |k: &str| trial.obj().map(|t| t.at(k).clone()).filter(Json::truthy);
        let cpu = tried("cpu").unwrap_or_else(|| if st.at("cpu_off").truthy() { Json::Null } else { c.cpu_json() });
        let gpu = tried("gpu").unwrap_or_else(|| if st.at("gpu_off").truthy() { Json::Null } else { c.gpu_json() });
        let cpu_tuned = cpu.truthy() && {
            let r = cpu.obj().map(|o| o.at("ratios").clone()).unwrap_or(Json::Null);
            !r.py_eq(self.stock().at("ratios"))
        };
        cpu_tuned || !is_default_gpu(&gpu)
    }

    /// What Settings shows: read back from the hardware (root), into /run/yggdrasil/tune.json. `gpu`: an open card to reuse.
    pub fn publish_now(&self, power_w: Option<f64>, gpu: Option<&Gpu>) -> Result<(), OsErr> {
        let mut out = Obj::new()
            .with("cpu", Json::Null)
            .with("gpu", Json::Null)
            .with("power_w", power_w.map(Json::Float).unwrap_or(Json::Null))
            .with("time", Json::Float(self.ctx.time()));
        let cpu = (|| {
            let (base, programmable) = self.hw.platform(self)?;
            let ratios = ratios_of(self.hw.rdmsr(self, MSR_TURBO_RATIO_LIMIT)?);
            Ok::<_, OsErr>(
                Obj::new().with("ratios", ints_json(&ratios)).with("base", base.into()).with("programmable", programmable.into()),
            )
        })();
        match cpu {
            Ok(c) => out.set("cpu", c.into()),
            Err(e) => out.set("cpu_error", e.to_string().into()),
        }
        let opened;
        let g = match gpu {
            Some(g) => Ok(g),
            None => {
                opened = Gpu::open(self.ctx);
                opened.as_ref().map_err(Clone::clone)
            }
        };
        let read = g.and_then(|g| {
            Ok(Obj::new()
                .with("core", g.offset(CLOCK_GRAPHICS)?.into())
                .with("mem", g.offset(CLOCK_MEM)?.into())
                .with("max_core", g.max_core()?.into()))
        });
        match read {
            Ok(o) => out.set("gpu", o.into()),
            Err(e) => out.set("gpu_error", e.to_string().into()),
        }
        self.write_json(&self.run_path(), &out)
    }

    /// A graphics driver error (Xid) while the card's clocks are tuned puts them back at the defaults at once.
    pub fn on_kernel_line(&self, line: &str) -> Result<bool, OsErr> {
        if !line.contains("NVRM: Xid") {
            return Ok(false);
        }
        let st = self.status();
        let trial = st.at("trial");
        let gpu = match trial.obj().map(|t| t.at("gpu").clone()).filter(Json::truthy) {
            Some(g) => g,
            None if st.at("gpu_off").truthy() => Json::Null,
            None => self.conf().gpu_json(),
        };
        if is_default_gpu(&gpu) {
            return Ok(false);
        }
        let shown: String = py::strip(line).chars().take(120).collect();
        let why =
            format!("The graphics driver reported an error while the card's clocks were tuned ({shown}); they are back at the defaults");
        let _ = self.hw.apply_gpu(self, &GPU_DEFAULT);
        let mut rest = trial.obj().cloned().unwrap_or_default();
        rest.remove("gpu");
        let rest = if rest.is_empty() { Json::Null } else { rest.into() };
        self.set_status(vec![("gpu_off", why.as_str().into()), ("trial", rest)])?;
        self.log(&why);
        Ok(true)
    }

    // ------------------------------------------------------------ commands

    fn need_root(&self, what: &str) -> bool {
        if !self.hw.is_root(self) {
            crate::warn!("tune: {what} needs root (Settings runs it with pkexec)");
            return false;
        }
        true
    }

    fn turbo_on(&self) -> Option<bool> {
        self.ctx.read_int(&self.pstate("no_turbo")).map(|v| v == 0)
    }

    fn set_rapl(&self, pl1: &Json, pl2: &Json) -> Result<(), OsErr> {
        for (i, w) in [(0, pl1), (1, pl2)] {
            if w.is_null() {
                continue;
            }
            // int(w): a number, or a number written as text
            let w = match w {
                Json::Str(s) => py::int(s).ok(),
                w => json_number(w).filter(|f| f.is_finite()).map(|f| f.trunc() as i64),
            };
            if let Some(w) = w {
                self.hw.write_text(
                    self,
                    &self.rapl(&format!("constraint_{i}_power_limit_uw")),
                    &(w.saturating_mul(1_000_000)).to_string(),
                )?;
            }
        }
        Ok(())
    }

    fn set_turbo(&self, on: bool) -> Result<(), OsErr> {
        self.hw.write_text(self, &self.pstate("no_turbo"), if on { "0" } else { "1" })
    }

    pub fn cmd_get(&self) -> R<i32> {
        let c = self.conf();
        let st = self.status();
        let run = self.read_json(&self.run_path()).unwrap_or_default();
        let (pl1, pl2) = self.hw.rapl_limits(self);
        let s = self.stock();
        let ratios = match s.at("ratios") {
            r if r.truthy() => r.clone(),
            _ => Json::Arr(vec![Json::Int(50)]),
        };
        let top = (py_max(&ratios).map_err(Crash)?.saturating_add(RATIO_HEADROOM)).min(60);
        let pair = |a: i64, b: i64| Json::Arr(vec![a.into(), b.into()]);
        let service = self.ctx.ask("systemctl", &["is-active", "--quiet", "yggdrasil-tune"]).map(|(c, _)| c) == Some(0);
        let out = Obj::new()
            .with("kept", c.json())
            .with("status", st.into())
            .with("now", run.into())
            .with("stock", s.into())
            .with("rapl", Obj::new().with("pl1", pl1.into()).with("pl2", pl2.into()).into())
            .with("turbo", self.turbo_on().into())
            .with(
                "ranges",
                Obj::new()
                    .with("gpu_core", pair(GPU_CORE.0, GPU_CORE.1))
                    .with("gpu_mem", pair(GPU_MEM.0, GPU_MEM.1))
                    .with("power", pair(POWER_W.0, POWER_W.1))
                    .with("ratio", pair(RATIO_FLOOR, top))
                    .into(),
            )
            .with("service", service.into());
        crate::out!("{}", crate::json::dumps(&out.into()));
        Ok(0)
    }

    pub fn cmd_try(&self, what: &str, args: &[String]) -> R<i32> {
        if !self.need_root("try") {
            return Ok(1);
        }
        let kv = match parse_kv(args) {
            Ok(k) => k,
            Err(e) => return Ok(fail(&e, 2)),
        };
        let get = |k: &str| kv.iter().rev().find(|(x, _)| x == k).map(|(_, v)| v.as_str());
        let (value, apply): (Json, Either) = if what == "cpu" {
            let mut asked = Vec::new();
            for x in get("ratios").unwrap_or("").split(',') {
                if py::isdigit(py::strip(x).trim_start_matches('-')) {
                    match py::int(x) {
                        Ok(v) => asked.push(Json::Int(v)),
                        Err(e) => return Ok(fail(&e, 2)),
                    }
                }
            }
            let ratios = match check_ratios(&asked, &self.stock_ratios()) {
                Ok(r) => r,
                Err(CheckErr::Value(e)) => return Ok(fail(&e, 2)),
                Err(CheckErr::Type(e)) => return Err(Crash(e)),
            };
            let (_, programmable) = self.hw.platform(self)?;
            if !programmable {
                return Ok(fail("this processor's turbo multipliers cannot be changed (a locked processor, or the BIOS)", 2));
            }
            (Obj::new().with("ratios", ints_json(&ratios)).into(), Either::Cpu(ratios))
        } else {
            let cap = get("cap").unwrap_or("none");
            let parsed = (|| {
                let core = py::int(get("core").unwrap_or("0"))?;
                let mem = py::int(get("mem").unwrap_or("0"))?;
                let cap = if cap == "none" { Json::Null } else { Json::Int(py::int(cap)?) };
                check_gpu(&Obj::new().with("core", core.into()).with("mem", mem.into()).with("cap", cap).into())
            })();
            match parsed {
                Ok(g) => (g.json(), Either::Gpu(g)),
                Err(e) => return Ok(fail(&e, 2)),
            }
        };
        let st = self.status();
        let mut trial = st.at("trial").obj().cloned().unwrap_or_default();
        trial.set(what, value.clone());
        self.mark_tuned()?; // first: if this freezes the computer, the next start uses the defaults
        self.set_status(vec![("trial", trial.into()), (&format!("{what}_off"), Json::Null)])?;
        let applied = match &apply {
            Either::Cpu(r) => self.hw.wrmsr(self, MSR_TURBO_RATIO_LIMIT, value_of(r)),
            Either::Gpu(g) => self.hw.apply_gpu(self, g),
        };
        if let Err(e) = applied {
            self.cmd_undo(what)?;
            return Ok(fail(&format!("could not apply it ({e}); put back"), 1));
        }
        self.log(&format!("trying {what}: {}", value.repr()));
        self.hw.publish(self, None, None)?;
        Ok(0)
    }

    /// The kept values for one part, or the defaults when nothing is kept.
    pub fn apply_kept(&self, what: &str, c: Option<&Conf>) -> Result<(), OsErr> {
        let own;
        let c = match c {
            Some(c) => c,
            None => {
                own = self.conf();
                &own
            }
        };
        if what == "cpu" {
            let ratios: Option<Vec<i64>> = match &c.cpu {
                Some(r) if !r.is_empty() => Some(r.clone()),
                _ => match self.stock().at("ratios") {
                    Json::Arr(a) if !a.is_empty() => a.iter().map(Json::int_or_bool).collect(),
                    _ => None,
                },
            };
            if let Some(r) = ratios {
                self.hw.wrmsr(self, MSR_TURBO_RATIO_LIMIT, value_of(&r))?;
            }
            Ok(())
        } else {
            self.hw.apply_gpu(self, c.gpu.as_ref().unwrap_or(&GPU_DEFAULT))
        }
    }

    pub fn cmd_keep(&self, what: &str) -> R<i32> {
        if !self.need_root("keep") {
            return Ok(1);
        }
        let st = self.status();
        let mut trial = st.at("trial").obj().cloned().unwrap_or_default();
        let Some(v) = trial.remove(what) else { return Ok(fail(&format!("nothing is being tried for {what}"), 0)) };
        let mut d = self.read_json(&self.conf_path()).unwrap_or_default();
        d.set(what, v.clone());
        self.write_json(&self.conf_path(), &d)?;
        self.set_status(vec![("trial", if trial.is_empty() { Json::Null } else { trial.into() })])?;
        self.log(&format!("kept {what}: {}", v.py_str()));
        Ok(0)
    }

    pub fn cmd_undo(&self, what: &str) -> R<i32> {
        if !self.need_root("undo") {
            return Ok(1);
        }
        let st = self.status();
        let mut trial = st.at("trial").obj().cloned().unwrap_or_default();
        trial.remove(what);
        self.set_status(vec![("trial", if trial.is_empty() { Json::Null } else { trial.into() })])?;
        if let Err(e) = self.apply_kept(what, None) {
            return Ok(fail(&format!("could not put it back ({e})"), 1));
        }
        self.refresh_mark()?;
        self.log(&format!("undid the {what} trial"));
        self.hw.publish(self, None, None)?;
        Ok(0)
    }

    pub fn cmd_reset(&self, what: &str) -> R<i32> {
        if !self.need_root("reset") {
            return Ok(1);
        }
        let mut d = self.read_json(&self.conf_path()).unwrap_or_default();
        d.set(what, Json::Null);
        self.write_json(&self.conf_path(), &d)?;
        let st = self.status();
        let mut trial = st.at("trial").obj().cloned().unwrap_or_default();
        trial.remove(what);
        self.set_status(vec![("trial", if trial.is_empty() { Json::Null } else { trial.into() }), (&format!("{what}_off"), Json::Null)])?;
        if let Err(e) = self.apply_kept(what, None) {
            return Ok(fail(&format!("could not put the defaults back ({e})"), 1));
        }
        self.refresh_mark()?;
        self.log(&format!("{what} back to the defaults"));
        self.hw.publish(self, None, None)?;
        Ok(0)
    }

    pub fn cmd_reenable(&self) -> R<i32> {
        if !self.need_root("reenable") {
            return Ok(1);
        }
        self.set_status(vec![("cpu_off", Json::Null), ("gpu_off", Json::Null)])?;
        let c = self.conf();
        self.refresh_mark()?;
        for what in ["cpu", "gpu"] {
            if let Err(e) = self.apply_kept(what, Some(&c)) {
                return Ok(fail(&format!("could not apply the kept {what} values ({e})"), 1));
            }
        }
        self.log("tuning switched on again by the owner");
        self.hw.publish(self, None, None)?;
        Ok(0)
    }

    pub fn cmd_set(&self, what: &str, args: &[String]) -> R<i32> {
        if !self.need_root("set") {
            return Ok(1);
        }
        let mut d = self.read_json(&self.conf_path()).unwrap_or_default();
        let r: Result<(), SetErr> = (|| {
            if what == "power" {
                let kv = parse_kv(args).map_err(SetErr::Value)?;
                let mut p = d.at("power").obj().cloned().unwrap_or_default();
                for k in ["pl1", "pl2"] {
                    let Some(v) = kv.iter().rev().find(|(x, _)| x == k).map(|(_, v)| v.as_str()) else { continue };
                    if v == "default" {
                        p.set(k, Json::Null);
                    } else if py::isdigit(v) && (POWER_W.0..=POWER_W.1).contains(&py::int(v).unwrap_or(-1)) {
                        p.set(k, py::int(v).unwrap_or(0).into());
                    } else {
                        return Err(SetErr::Value(format!("{k} is {}-{} W or default", POWER_W.0, POWER_W.1)));
                    }
                }
                let s = self.stock();
                let or = |a: &Json, b: &Json| if a.truthy() { a.clone() } else { b.clone() };
                let pl1 = or(p.at("pl1"), s.at("pl1"));
                let pl2 = or(p.at("pl2"), s.at("pl2"));
                if pl1.truthy() && pl2.truthy() {
                    match (json_number(&pl1), json_number(&pl2)) {
                        (Some(a), Some(b)) if b < a => {
                            return Err(SetErr::Value("the short-term limit may not be below the long-term one".into()))
                        }
                        (Some(_), Some(_)) => {}
                        _ => {
                            return Err(SetErr::Crash(format!(
                                "'<' not supported between instances of '{}' and '{}'",
                                pl2.type_name(),
                                pl1.type_name()
                            )))
                        }
                    }
                }
                d.set("power", p.into());
                self.write_json(&self.conf_path(), &d).map_err(SetErr::Os)?;
                self.set_rapl(&pl1, &pl2).map_err(SetErr::Os)?;
            } else if what == "turbo" && args.len() == 1 && (args[0] == "on" || args[0] == "off") {
                let on = args[0] == "on";
                d.set("turbo", on.into());
                self.write_json(&self.conf_path(), &d).map_err(SetErr::Os)?;
                self.set_turbo(on).map_err(SetErr::Os)?;
            } else {
                return Err(SetErr::Usage);
            }
            Ok(())
        })();
        match r {
            Ok(()) => {}
            Err(SetErr::Usage) => return Ok(fail("set power pl1=W pl2=W, or set turbo on|off", 2)),
            Err(SetErr::Value(e)) => return Ok(fail(&e, 2)),
            Err(SetErr::Os(e)) => return Ok(fail(&format!("the processor refused it ({e}): the BIOS may lock it"), 1)),
            Err(SetErr::Crash(e)) => return Err(Crash(e)),
        }
        self.log(&format!("set {what}: {}", py::repr_list(args)));
        Ok(0)
    }

    /// The processor's own values, read once per start of the computer, before anything is applied: a restart of the
    /// computer puts them back, a restart of the service alone does not (its second start keeps what the first one read).
    pub fn watch_stock(&self) -> Result<Obj, OsErr> {
        let mut s = self.stock();
        let me = self.hw.boot_id(self);
        if s.at("boot_id").py_eq(&Json::Str(me.clone())) && s.at("ratios").truthy() {
            return Ok(s);
        }
        s.set("boot_id", me.into());
        let read = (|| {
            s.set("ratios", ints_json(&ratios_of(self.hw.rdmsr(self, MSR_TURBO_RATIO_LIMIT)?)));
            let (base, programmable) = self.hw.platform(self)?;
            s.set("base", base.into());
            s.set("programmable", programmable.into());
            Ok::<_, OsErr>(())
        })();
        if let Err(e) = read {
            self.log(&format!("cannot read the processor's multipliers: {e}"));
        }
        let (pl1, pl2) = self.hw.rapl_limits(self);
        s.set("pl1", pl1.into());
        s.set("pl2", pl2.into());
        self.write_json(&self.state("stock.json"), &s)?;
        Ok(s)
    }

    pub fn cmd_watch(&self) -> R<i32> {
        if !self.need_root("watch") {
            return Ok(1);
        }
        let me = self.hw.boot_id(self);
        sys::catch_stop_signals();
        self.watch_stock()?;
        let guarded = self.guard_check(&me)?;
        let c = self.conf();
        let st = self.status();
        self.set_status(vec![("trial", Json::Null)])?; // a trial never outlives its session
        let limits = (|| {
            self.set_rapl(&Json::from(c.pl1), &Json::from(c.pl2))?;
            if let Some(t) = c.turbo {
                self.set_turbo(t)?;
            }
            Ok::<_, OsErr>(())
        })();
        if let Err(e) = limits {
            self.log(&format!("power limits or turbo: {e}"));
        }
        if !guarded {
            self.refresh_mark()?;
            for what in ["cpu", "gpu"] {
                if !st.at(&format!("{what}_off")).truthy() {
                    if let Err(e) = self.apply_kept(what, Some(&c)) {
                        self.log(&format!("applying the kept {what} values: {e}"));
                    }
                }
            }
        }
        // the kernel's messages, for the graphics driver's errors (Xid) while its clocks are tuned; and the package's power
        let mut kernel = Kernel::open(self.ctx);
        match &kernel {
            Some(k) => self.log(&format!("reading the kernel's messages from {}", k.source)),
            None => self.log("the kernel's messages cannot be read: graphics driver errors go unnoticed"),
        }
        let gpu = Gpu::open(self.ctx).ok();
        let span = self.ctx.read_int(&self.rapl("max_energy_range_uj")).filter(|v| *v != 0).unwrap_or(1 << 32);
        let mut last_e = self.ctx.read_int(&self.rapl("energy_uj"));
        let mut last_t = self.ctx.monotonic();
        let mut power: Option<f64> = None;
        let mut next_at = last_t + EVERY_S;
        let looped: R<()> = (|| {
            while !self.ctx.should_stop() {
                let wait = (next_at - self.ctx.monotonic()).max(0.0);
                if let Some(k) = kernel.as_mut() {
                    match k.wait(self.ctx, wait) {
                        Event::Lines(lines) => {
                            for l in lines {
                                self.on_kernel_line(&l)?;
                            }
                            continue;
                        }
                        Event::Ended => {
                            kernel = None; // the source ended: keep measuring without it
                            continue;
                        }
                        Event::Interrupted => continue,
                        Event::Timeout => {}
                    }
                } else {
                    self.ctx.wait(wait);
                    if self.ctx.stop_requested() {
                        continue;
                    }
                }
                let e = self.ctx.read_int(&self.rapl("energy_uj"));
                let t = self.ctx.monotonic();
                if let (Some(e), Some(le)) = (e, last_e) {
                    if t > last_t {
                        power = Some(py::round1((e - le).rem_euclid(span) as f64 / 1e6 / (t - last_t)));
                    }
                }
                (last_e, last_t, next_at) = (e, t, t + EVERY_S);
                self.hw.publish(self, power, gpu.as_ref())?;
                self.ctx.round_done();
            }
            Ok(())
        })();
        drop(kernel);
        drop(gpu);
        // a clean shutdown ends the guard; a stop of the service alone (an update) keeps it
        let state = self.ctx.ask("systemctl", &["is-system-running"]).map(|(_, out)| py::strip(&out).to_string()).unwrap_or_default();
        if state == "stopping" {
            self.ctx.remove(&self.state("tuned"))?;
        }
        looped?;
        Ok(0)
    }
}

enum Either {
    Cpu(Vec<i64>),
    Gpu(GpuVals),
}

enum SetErr {
    Usage,
    Value(String),
    Os(OsErr),
    Crash(String),
}

fn fail(msg: &str, code: i32) -> i32 {
    crate::warn!("tune: {msg}");
    code
}

pub fn parse_kv(args: &[String]) -> Result<Vec<(String, String)>, String> {
    let mut out = Vec::new();
    for a in args {
        match a.split_once('=') {
            Some((k, v)) => out.push((k.to_string(), v.to_string())),
            None => return Err(format!("{} is not KEY=VALUE", py::repr(a))),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------- the kernel's messages

/// What came from the kernel's messages while waiting.
pub enum Event {
    Lines(Vec<String>),
    Timeout,
    Interrupted,
    /// the source ended (journalctl exited, a fake file was read to its end)
    Ended,
}

/// The kernel's messages: /dev/kmsg (one record per read, only those after the start), or a `journalctl -k -f` child when
/// /dev/kmsg cannot be opened. A dry run against a fake tree reads ROOT/dev/kmsg as a plain file of lines, from its start.
pub struct Kernel {
    fd: OwnedFd,
    /// /dev/kmsg itself: each read is one record "PRIORITY,SEQ,TIME,FLAGS;message" (plus " KEY=value" lines)
    records: bool,
    child: Option<Child>,
    buf: Vec<u8>,
    pub source: String,
}

impl Kernel {
    pub fn open(ctx: &Ctx) -> Option<Kernel> {
        let path = ctx.path(KMSG);
        let kmsg_err = match open_nonblock(&path) {
            Ok(fd) => {
                let mut st: libc::stat = unsafe { std::mem::zeroed() };
                let records = unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } == 0 && (st.st_mode & libc::S_IFMT) == libc::S_IFCHR;
                if records {
                    // only what comes from now on (as journalctl -n 0)
                    unsafe { libc::lseek(fd.as_raw_fd(), 0, libc::SEEK_END) };
                }
                return Some(Kernel { fd, records, child: None, buf: Vec::new(), source: path.display().to_string() });
            }
            Err(e) => e,
        };
        if ctx.fake_root() {
            return None;
        }
        // the fallback: what the Python helper always ran
        let mut child = Command::new("journalctl")
            .args(["-k", "-f", "-n", "0", "-o", "cat"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        let out = child.stdout.take()?;
        let fd: OwnedFd = out.into();
        unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFL, libc::fcntl(fd.as_raw_fd(), libc::F_GETFL) | libc::O_NONBLOCK) };
        Some(Kernel {
            fd,
            records: false,
            child: Some(child),
            buf: Vec::new(),
            source: format!("journalctl ({}: {kmsg_err})", path.display()),
        })
    }

    /// Waits up to `secs` for messages, or a stop (the stop signal, or in the merged service this part switched off). With
    /// --steps nothing is waited for: the virtual clock moves on instead.
    pub fn wait(&mut self, ctx: &Ctx, secs: f64) -> Event {
        ctx.beat();
        let virtual_clock = ctx.steps.is_some();
        let ms = if virtual_clock { 0 } else { (secs * 1000.0).ceil().min(i32::MAX as f64) as i32 };
        let mut pfds = vec![libc::pollfd { fd: self.fd.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        if !virtual_clock {
            pfds.extend(ctx.stop_fds().into_iter().map(|fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }));
        }
        let n = unsafe { libc::poll(pfds.as_mut_ptr(), pfds.len() as libc::nfds_t, ms) };
        if n < 0 || (n > 0 && pfds[0].revents == 0) {
            return Event::Interrupted;
        }
        if n == 0 {
            if virtual_clock {
                ctx.wait(secs);
            }
            return Event::Timeout;
        }
        let mut chunk = [0u8; 8192];
        let r = unsafe { libc::read(self.fd.as_raw_fd(), chunk.as_mut_ptr() as *mut libc::c_void, chunk.len()) };
        if r < 0 {
            return match std::io::Error::last_os_error().raw_os_error() {
                // EPIPE: records were overwritten before they were read; the next read goes on with the oldest left
                Some(libc::EAGAIN) | Some(libc::EPIPE) | Some(libc::EINTR) => Event::Interrupted,
                _ => Event::Ended,
            };
        }
        if r == 0 {
            return Event::Ended;
        }
        let data = &chunk[..r as usize];
        if self.records {
            return Event::Lines(kmsg_message(data).into_iter().collect());
        }
        self.buf.extend_from_slice(data);
        let mut lines = Vec::new();
        while let Some(i) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=i).collect();
            lines.push(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
        }
        Event::Lines(lines)
    }
}

impl Drop for Kernel {
    fn drop(&mut self) {
        if let Some(c) = &mut self.child {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn open_nonblock(p: &Path) -> std::io::Result<OwnedFd> {
    let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).map_err(|_| std::io::Error::from_raw_os_error(libc::EINVAL))?;
    let fd = unsafe { libc::open(c.as_ptr(), libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// The message of one /dev/kmsg record ("4,1532,1234567,-;NVRM: Xid ...\n SUBSYSTEM=pci\n"): the text after the first ';'
/// on the first line, with the kernel's \xNN escapes undone (as journald shows it).
pub fn kmsg_message(record: &[u8]) -> Option<String> {
    let first = record.split(|b| *b == b'\n').next()?;
    let semi = first.iter().position(|b| *b == b';')?;
    let msg = &first[semi + 1..];
    let mut out = Vec::with_capacity(msg.len());
    let mut i = 0;
    while i < msg.len() {
        if msg[i] == b'\\' && msg.get(i + 1) == Some(&b'x') {
            if let Some(v) = msg.get(i + 2..i + 4).and_then(|h| std::str::from_utf8(h).ok()).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 4;
                continue;
            }
        }
        out.push(msg[i]);
        i += 1;
    }
    Some(String::from_utf8_lossy(&out).into_owned())
}

pub fn main(args: Vec<String>) -> i32 {
    let (ctx, argv) = match Ctx::from_args(args) {
        Ok(x) => x,
        Err(e) => {
            crate::warn!("tune: {e}");
            return 2;
        }
    };
    if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
        crate::out!("{HELP}{DRY_RUN_HELP}");
        return if argv.len() >= 2 { 0 } else { 2 };
    }
    let t = Tune::new(&ctx, &System);
    let c = argv[1].as_str();
    let rest = &argv[2..];
    let part = |i: usize| rest.get(i).map(String::as_str).filter(|p| *p == "cpu" || *p == "gpu");
    let r = match c {
        "get" if rest.is_empty() => t.cmd_get(),
        "try" if part(0).is_some() => t.cmd_try(&rest[0], &rest[1..]),
        "keep" if rest.len() == 1 && part(0).is_some() => t.cmd_keep(&rest[0]),
        "undo" if rest.len() == 1 && part(0).is_some() => t.cmd_undo(&rest[0]),
        "reset" if rest.len() == 1 && part(0).is_some() => t.cmd_reset(&rest[0]),
        "reenable" if rest.is_empty() => t.cmd_reenable(),
        "set" if !rest.is_empty() => t.cmd_set(&rest[0], &rest[1..]),
        "watch" if rest.is_empty() => t.cmd_watch(),
        _ => {
            crate::warn!("tune: unknown command (see --help)");
            return 2;
        }
    };
    match r {
        Ok(code) => code,
        Err(Crash(e)) => {
            crate::warn!("tune: {e}");
            1
        }
    }
}

#[cfg(test)]
mod tests;
