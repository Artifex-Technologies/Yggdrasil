//! powerlimit: each power mode's processor and graphics card limits. A port of packages/settings/helpers/powerlimit with the
//! same commands, files, environment variables, output, exit codes and messages; the help text below is the Python
//! docstring. Like the Python one it asks power-profiles-daemon for the mode through busctl, and the card's power limits
//! through nvidia-smi, which also sets them (`nvidia-smi -pl`): it only runs when the mode, the limits or the power hold
//! change, so `watch` holds no card of its own.

use std::path::{Path, PathBuf};

use crate::json::{self, Json, Obj};
use crate::py;
use crate::sys::{self, Ctx};

pub const HELP: &str = r#"powerlimit: processor and graphics-card limits for each power mode (Settings > Power, Developer options).

For each power mode (performance, balanced, power-saver) a limit can be set:

  cpu   the processor may not clock above this percent of its top speed (1-100)
  gpu   an NVIDIA graphics card may not draw more than this many watts (within what the card allows)

The limits of the mode that is active are applied; when the mode changes, the new mode's limits are applied (a mode
with no limit restores the full speed and the card's default power). Nothing here overclocks: it only holds things back.

Commands:
  powerlimit set MODE cpu=PERCENT|default gpu=WATTS|default   change one mode (pkexec: needs root)
  powerlimit watch                                            as root service (yggdrasil-powerlimit): apply now, and on every mode change
  powerlimit apply                                            apply the active mode's limits once
  powerlimit get                                              print what the hardware allows and what is set (JSON)

The limits are kept in /etc/yggdrasil/power-limits.json: {"balanced": {"cpu": 80, "gpu": 200}, ...} (read from the desktop's old
place, /etc/labyrinth/power-limits.json, until the first change).

While the graphics-card unlock (gpuunlock) is armed or being measured, it leaves a power hold: no mode then lets the card draw more
than its default watts (or the lower cap the hold names). The modes' own limits stay saved and apply again when the hold ends.
"#;

pub const DRY_RUN_HELP: &str = r#"
Dry run (any command): --dry-run reads as usual and writes nothing; each write (a processor's speed cap, the limits file,
nvidia-smi -pl) is printed as "dry run: ...". --root DIR takes every system path under DIR, and the answers of busctl and
nvidia-smi from DIR/commands.json when it names them (a fake tree, no hardware at all); --steps N runs N rounds of `watch` on
a virtual clock, without waiting.
"#;

pub const STATE: &str = "/etc/yggdrasil/power-limits.json";
pub const OLD_STATE: &str = "/etc/labyrinth/power-limits.json";
pub const CPUFREQ: &str = "/sys/devices/system/cpu/cpufreq";
pub const HOLD: &str = "/var/lib/yggdrasil/gpu-unlock/power-hold";
pub const MODES: [&str; 3] = ["performance", "balanced", "power-saver"];
pub const EVERY_S: f64 = 3.0;
/// The environment variables the Python tests set (and this one honours): each replaces its path as it is.
pub const STATE_ENV: &str = "YGGDRASIL_POWERLIMIT_STATE";
pub const CPUFREQ_ENV: &str = "YGGDRASIL_CPUFREQ";
pub const HOLD_ENV: &str = "YGGDRASIL_POWERLIMIT_HOLD";
const PROFILES: [(&str, &str); 2] =
    [("org.freedesktop.UPower.PowerProfiles", "/org/freedesktop/UPower/PowerProfiles"), ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles")];
const QUERY: [&str; 2] = ["--query-gpu=name,power.limit,power.default_limit,power.min_limit,power.max_limit", "--format=csv,noheader,nounits"];

fn env_or(ctx: &Ctx, var: &str, path: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).unwrap_or_else(|| ctx.path(path))
}

pub fn state_path(ctx: &Ctx) -> PathBuf {
    env_or(ctx, STATE_ENV, STATE)
}

pub fn old_state_path(ctx: &Ctx) -> PathBuf {
    ctx.path(OLD_STATE)
}

pub fn cpufreq(ctx: &Ctx) -> PathBuf {
    env_or(ctx, CPUFREQ_ENV, CPUFREQ)
}

pub fn hold_path(ctx: &Ctx) -> PathBuf {
    env_or(ctx, HOLD_ENV, HOLD)
}

// ---------------------------------------------------------------- the limits file

/// The limits, cleaned: {mode: {"cpu": 1-100, "gpu": 10-1000}} with anything else left out (the values as the file has them:
/// isinstance(v, int) took true as 1). `path`: None reads the file in use (the old place until the new file exists).
pub fn load_state(ctx: &Ctx, path: Option<&Path>) -> Obj {
    let state = state_path(ctx);
    let path = match path {
        Some(p) => p.to_path_buf(),
        None if !ctx.exists(&state) && std::env::var_os(STATE_ENV).is_none() => old_state_path(ctx),
        None => state,
    };
    let Ok(d) = ctx.read_json(&path) else { return Obj::new() };
    let mut out = Obj::new();
    for (mode, lim) in d.obj().map(|o| o.iter().collect::<Vec<_>>()).unwrap_or_default() {
        let (true, Some(lim)) = (MODES.contains(&mode), lim.obj()) else { continue };
        let mut clean = Obj::new();
        if lim.at("cpu").int_or_bool().is_some_and(|c| (1..=100).contains(&c)) {
            clean.set("cpu", lim.at("cpu").clone());
        }
        if lim.at("gpu").int_or_bool().is_some_and(|g| (10..=1000).contains(&g)) {
            clean.set("gpu", lim.at("gpu").clone());
        }
        if !clean.is_empty() {
            out.set(mode, clean.into());
        }
    }
    out
}

/// json.dump(state, indent=1, sort_keys=True) into the file (the new place), mode 0644, through a temporary file.
pub fn save_state(ctx: &Ctx, state: &Obj, path: Option<&Path>) -> Result<(), py::OsErr> {
    let p = path.map(Path::to_path_buf).unwrap_or_else(|| state_path(ctx));
    ctx.write_json(&p, &json::sorted(&Json::Obj(state.clone())))
}

// ---------------------------------------------------------------- the power mode, the processor, the card

/// The power mode power-profiles-daemon has active (busctl, at most 5 s each; the newer bus name first), or "".
pub fn active_mode(ctx: &Ctx) -> String {
    for (name, path) in PROFILES {
        let Some((_, out)) = ctx.ask_within("busctl", &["get-property", name, path, name, "ActiveProfile"], Some(5.0)) else { continue };
        if let Some(rest) = py::strip(&out).strip_prefix("s ") {
            return py::strip(rest).trim_matches('"').to_string();
        }
    }
    String::new()
}

/// The clock ceiling for a percent of the top speed, never below what the processor can run at.
pub fn cpu_target_khz(percent: i64, min_khz: i64, max_khz: i64) -> i64 {
    let want = py::round((max_khz as i128 * percent as i128) as f64 / 100.0);
    min_khz.max(max_khz.min(want))
}

/// Sets every core's ceiling (None restores the full speed). Returns how many policies were written.
pub fn apply_cpu(ctx: &Ctx, percent: Option<i64>, root: Option<&Path>) -> usize {
    let root = root.map(Path::to_path_buf).unwrap_or_else(|| cpufreq(ctx));
    let mut n = 0;
    for pol in ctx.names(&root, "policy", "") {
        let pol = root.join(pol);
        let (Some(lo), Some(hi)) = (ctx.read_int(&pol.join("cpuinfo_min_freq")), ctx.read_int(&pol.join("cpuinfo_max_freq"))) else { continue };
        let want = match percent {
            None => hi,
            Some(p) => cpu_target_khz(p, lo, hi),
        };
        if ctx.write_text(&pol.join("scaling_max_freq"), &want.to_string()).is_ok() {
            n += 1;
        }
    }
    n
}

/// The first NVIDIA card's power limits in watts, as nvidia-smi gives them.
#[derive(Clone, Debug, PartialEq)]
pub struct GpuInfo {
    pub name: String,
    pub watts: i64,
    pub default: i64,
    pub min: i64,
    pub max: i64,
}

/// One line of `nvidia-smi --query-gpu=name,power.limit,... --format=csv,noheader,nounits`, or None ("[N/A]", too few
/// fields). An infinite number (Python crashed on round(inf)) is None too.
pub fn parse_gpu_info(out: &str) -> Option<GpuInfo> {
    let fields: Vec<&str> = py::first_line(out)?.split(',').map(py::strip).collect();
    let [name, cur, dflt, lo, hi] = fields.as_slice() else { return None };
    let w = |s: &str| py::float(s).ok().filter(|f| f.is_finite()).map(py::round);
    Some(GpuInfo { name: name.to_string(), watts: w(cur)?, default: w(dflt)?, min: w(lo)?, max: w(hi)? })
}

/// The card's limits through nvidia-smi (at most 10 s), or None.
pub fn gpu_info(ctx: &Ctx) -> Option<GpuInfo> {
    let (rc, out) = ctx.ask_within("nvidia-smi", &QUERY, Some(10.0))?;
    if rc != 0 || py::strip(&out).is_empty() {
        return None;
    }
    parse_gpu_info(&out)
}

/// The most watts the power hold allows (the card's default, or the lower cap it names), or None when there is no hold. A hold
/// file that cannot be read still holds at the default. The value as Python held it (min(default, watts) keeps a true).
pub fn hold_cap(ctx: &Ctx, default: i64, path: Option<&Path>) -> Option<Json> {
    let p = path.map(Path::to_path_buf).unwrap_or_else(|| hold_path(ctx));
    let b = match ctx.read(&p) {
        Ok(b) => b,
        Err(e) if e.raw_os_error() == Some(libc::ENOENT) => return None,
        Err(_) => return Some(Json::Int(default)),
    };
    let Ok(d) = json::parse_bytes(&b) else { return Some(Json::Int(default)) };
    let w = d.obj().map(|o| o.at("watts").clone()).unwrap_or(Json::Null);
    Some(match w.int_or_bool() {
        Some(v) if v > 0 && v < default => w,
        _ => Json::Int(default),
    })
}

/// The limit to set: the mode's (None: the card's default) within what the card allows, and never above a hold's cap.
pub fn gpu_target(watts: Option<i64>, dflt: i64, lo: i64, hi: i64, cap: Option<i64>) -> i64 {
    let want = match watts {
        None => dflt,
        Some(w) => lo.max(hi.min(w)),
    };
    match cap {
        None => want,
        Some(c) => lo.max(want.min(c)),
    }
}

/// Sets the card's power limit within what it allows (None restores the card's default), never above a power hold.
pub fn apply_gpu(ctx: &Ctx, watts: Option<i64>) -> bool {
    let Some(g) = gpu_info(ctx) else { return false };
    let cap = hold_cap(ctx, g.default, None).map(|c| c.int_or_bool().unwrap_or(g.default));
    let want = gpu_target(watts, g.default, g.min, g.max, cap);
    if want == g.watts {
        return true;
    }
    ctx.act("nvidia-smi", &["-pl", &want.to_string()]) == 0
}

/// The limits of a mode (None or "": the active one), applied. Returns the mode and its limits.
pub fn apply(ctx: &Ctx, state: Option<Obj>, mode: Option<String>) -> (String, Obj) {
    let state = state.unwrap_or_else(|| load_state(ctx, None));
    let mode = mode.filter(|m| !m.is_empty()).unwrap_or_else(|| active_mode(ctx));
    let lim = state.at(&mode).obj().cloned().unwrap_or_default();
    apply_cpu(ctx, lim.at("cpu").int_or_bool(), None);
    apply_gpu(ctx, lim.at("gpu").int_or_bool());
    (mode, lim)
}

// ---------------------------------------------------------------- commands

/// A number given as digits: an int, or (beyond i64) the digits as Python's int would write them.
fn int_json(digits: &str) -> Json {
    match digits.parse::<i64>() {
        Ok(v) => Json::Int(v),
        Err(_) => {
            let t = digits.trim_start_matches('0');
            Json::Big(if t.is_empty() { "0".into() } else { t.into() })
        }
    }
}

pub fn cmd_set(ctx: &Ctx, mode: &str, kv: &[String]) -> i32 {
    if !ctx.is_root() {
        crate::warn!("powerlimit: set needs root (run it with pkexec)");
        return 1;
    }
    if !MODES.contains(&mode) {
        crate::warn!("powerlimit: unknown mode");
        return 2;
    }
    let mut state = load_state(ctx, None);
    let mut lim = state.at(mode).obj().cloned().unwrap_or_default();
    for item in kv {
        let (key, val) = item.split_once('=').unwrap_or((item.as_str(), ""));
        if key != "cpu" && key != "gpu" {
            crate::warn!("powerlimit: unknown limit {}", py::repr(key));
            return 2;
        }
        if val == "default" {
            lim.remove(key);
        } else if py::isdigit(val) {
            lim.set(key, int_json(val));
        } else {
            crate::warn!("powerlimit: {key} needs a number or 'default'");
            return 2;
        }
    }
    if lim.is_empty() {
        state.remove(mode);
    } else {
        state.set(mode, lim.into());
    }
    if let Err(e) = save_state(ctx, &state, None) {
        crate::warn!("powerlimit: {e}");
        return 1;
    }
    let state = load_state(ctx, None);
    if mode == active_mode(ctx) {
        apply(ctx, Some(state), Some(mode.to_string()));
    }
    0
}

pub fn cmd_get(ctx: &Ctx) -> i32 {
    crate::out!("{}", json::dumps(&get(ctx)));
    0
}

/// What `get` prints: the processor's top speed, the card's limits and hold, the active mode and the limits.
pub fn get(ctx: &Ctx) -> Json {
    let root = cpufreq(ctx);
    let mut top: Option<i64> = None;
    for pol in ctx.names(&root, "policy", "") {
        if let Some(hi) = ctx.read_int(&root.join(pol).join("cpuinfo_max_freq")).filter(|v| *v != 0) {
            top = Some(top.unwrap_or(0).max(hi));
        }
    }
    let gpu = match gpu_info(ctx) {
        None => Json::Null,
        Some(g) => {
            let hold = hold_cap(ctx, g.default, None).unwrap_or(Json::Null);
            let o = Obj::new().with("name", g.name.into()).with("watts", g.watts.into()).with("default", g.default.into());
            o.with("min", g.min.into()).with("max", g.max.into()).with("hold", hold).into()
        }
    };
    let out = Obj::new().with("cpu_max_khz", top.into()).with("gpu", gpu).with("active", active_mode(ctx).into());
    out.with("limits", load_state(ctx, None).into()).into()
}

pub fn cmd_apply(ctx: &Ctx) -> i32 {
    let (mode, lim) = apply(ctx, None, None);
    crate::out!("({}, {})", py::repr(&mode), Json::Obj(lim).repr());
    0
}

/// What the watch compares from one look to the next: the mode, and when the limits file and the hold last changed
/// (os.path.getmtime; None: not there).
type Seen = (String, Option<(i64, i64)>, Option<(i64, i64)>);

/// The root service: applies now, and again whenever the mode, the limits file or the power hold changes (looked at every
/// EVERY_S seconds). Ends at a stop (exit code 0).
pub fn cmd_watch(ctx: &Ctx) -> i32 {
    let mut last: Option<Seen> = None;
    while !ctx.should_stop() {
        let (state, old) = (state_path(ctx), old_state_path(ctx));
        let changed = if ctx.exists(&state) {
            ctx.mtime(&state)
        } else if ctx.exists(&old) {
            ctx.mtime(&old)
        } else {
            None
        };
        let key = (active_mode(ctx), changed, ctx.mtime(&hold_path(ctx)));
        if last.as_ref() != Some(&key) {
            apply(ctx, None, None);
            last = Some(key);
        }
        ctx.sleep(EVERY_S);
    }
    0
}

pub fn main(args: Vec<String>) -> i32 {
    let (ctx, argv) = match Ctx::from_args(args) {
        Ok(x) => x,
        Err(e) => {
            crate::warn!("powerlimit: {e}");
            return 2;
        }
    };
    if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
        crate::out!("{HELP}{DRY_RUN_HELP}");
        return if argv.len() >= 2 { 0 } else { 2 };
    }
    match argv[1].as_str() {
        "set" if argv.len() >= 4 => cmd_set(&ctx, &argv[2], &argv[3..]),
        "get" => cmd_get(&ctx),
        "apply" => cmd_apply(&ctx),
        "watch" => {
            sys::catch_stop_signals();
            cmd_watch(&ctx)
        }
        _ => {
            crate::warn!("powerlimit: unknown command (see --help)");
            2
        }
    }
}

#[cfg(test)]
mod tests;
