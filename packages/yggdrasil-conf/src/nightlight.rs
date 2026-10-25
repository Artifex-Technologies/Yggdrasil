//! Night Light: the screen's warm tint, by hyprsunset (Hyprland's own). The notch's Controls tile turns it on and off and drops down
//! its settings; Settings > Displays has the same ones. Both run the same code from here.
//!
//! Everything is kept in ~/.config/hypr/hyprsunset.conf. Its first line holds the settings as they are shown:
//!
//!   # yggdrasil: temperature=4000 schedule=on on=20:00 off=07:00 fade=on minutes=60 light=off
//!
//! With a schedule the rest of the file is hyprsunset profiles, and a fade is written out as a staircase of one-minute profiles,
//! spaced evenly in mireds so every step is the same perceived shift (adapted from omarchy-nightlight, MIT, (c) 2026 Jeremy
//! Langer). Without a schedule there is one profile, at midnight, that holds the light as it was left (`light`), so it is the
//! same after signing in. (With no profile at all hyprsunset would start at 6000 K, a faint tint.)
//!
//! hyprsunset is spoken to on its own socket and found in /proc, so nothing else (hyprctl, pgrep) is started to ask it anything.
//! One thread does all the work (`spawn`): it writes the file a moment after the last change, restarts hyprsunset when the
//! schedule changed, and every 30 seconds starts hyprsunset if it is not running.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, RecvTimeoutError, Sender};
use std::time::Duration;

/// Daylight: no tint.
pub const NEUTRAL: u32 = 6500;
/// hyprsunset counts as on below this (an identity profile reads 6000).
pub const ON_BELOW: u32 = 6000;
/// The warmth's range.
pub const MIN_TEMP: u32 = 1500;
pub const MAX_TEMP: u32 = 5500;
pub const MIN_FADE: u32 = 5;
pub const MAX_FADE: u32 = 240;
/// hyprsunset times are HH:MM, so a fade has at most one step a minute, and no more than this many.
const MAX_STEPS: u32 = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub temp: u32,
    pub schedule: bool,
    /// Turn-on and turn-off times, in minutes into the day.
    pub on: u32,
    pub off: u32,
    /// The screen eases in and out over `minutes` instead of switching at once.
    pub fade: bool,
    pub minutes: u32,
    /// Without a schedule: the light is on.
    pub light: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { temp: 4000, schedule: false, on: 20 * 60, off: 7 * 60, fade: false, minutes: 60, light: false }
    }
}

// ---------------------------------------------------------------- the file

pub fn fmt_time(m: u32) -> String {
    let m = m % 1440;
    format!("{:02}:{:02}", m / 60, m % 60)
}

pub fn parse_time(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// The settings from the file's first line; None when the file is not one Yggdrasil wrote.
pub fn parse(text: &str) -> Option<Settings> {
    let line = text.lines().find_map(|l| l.strip_prefix("# yggdrasil:"))?;
    let mut s = Settings::default();
    for kv in line.split_whitespace() {
        let Some((k, v)) = kv.split_once('=') else { continue };
        match k {
            "temperature" => s.temp = v.parse::<u32>().map(|t| t.clamp(MIN_TEMP, MAX_TEMP)).unwrap_or(s.temp),
            "schedule" => s.schedule = v == "on",
            "on" => s.on = parse_time(v).unwrap_or(s.on),
            "off" => s.off = parse_time(v).unwrap_or(s.off),
            "fade" => s.fade = v == "on",
            "minutes" => s.minutes = v.parse::<u32>().map(|f| f.clamp(MIN_FADE, MAX_FADE)).unwrap_or(s.minutes),
            "light" => s.light = v == "on",
            _ => {}
        }
    }
    Some(s)
}

/// Minutes from `from` to `to` going forward round the clock (a whole day when they are the same).
fn span(from: u32, to: u32) -> u32 {
    match (to + 1440 - from) % 1440 {
        0 => 1440,
        w => w,
    }
}

/// The fade in, shortened so it ends before the turn-off time; and the fade out, before the next turn-on.
fn fade_in_len(s: &Settings) -> u32 {
    if s.fade { s.minutes.min(span(s.on, s.off) - 1) } else { 0 }
}

fn fade_out_len(s: &Settings) -> u32 {
    if s.fade { s.minutes.min(span(s.off, s.on) - 1) } else { 0 }
}

/// The two fades as the screen will go through them: (starts, ends, from K, to K), in minutes into the day; the fade in first.
/// None without a schedule or a fade.
pub fn ramps(s: &Settings) -> Option<[(u32, u32, u32, u32); 2]> {
    (s.schedule && s.fade).then(|| [(s.on, s.on + fade_in_len(s), NEUTRAL, s.temp), (s.off, s.off + fade_out_len(s), s.temp, NEUTRAL)])
}

/// The colour of light at `k` kelvin (Tanner Helland's fit).
pub fn kelvin_rgb(k: u32) -> (u8, u8, u8) {
    let t = k as f32 / 100.0;
    let r = if t <= 66.0 { 255.0 } else { 329.7 * (t - 60.0).powf(-0.1332) };
    let g = if t <= 66.0 { 99.47 * t.ln() - 161.12 } else { 288.12 * (t - 60.0).powf(-0.0755) };
    let b = if t >= 66.0 { 255.0 } else if t <= 19.0 { 0.0 } else { 138.52 * (t - 10.0).ln() - 305.04 };
    let c = |v: f32| v.clamp(0.0, 255.0) as u8;
    (c(r), c(g), c(b))
}

/// The temperature a fraction `f` of the way from `from` K to `to` K, as the fade steps go (evenly in mireds).
pub fn kelvin_between(from: u32, to: u32, f: f32) -> u32 {
    let (a, b) = (1e6 / from as f32, 1e6 / to as f32);
    (1e6 / (a + (b - a) * f.clamp(0.0, 1.0))).round() as u32
}

/// From `from` K to `to` K over `minutes`: (minutes after the start, temperature) for each step, in equal steps of mired (1e6/K).
pub fn fade_steps(from: u32, to: u32, minutes: u32) -> Vec<(u32, u32)> {
    let n = minutes.clamp(1, MAX_STEPS);
    (0..n).map(|k| (((k * minutes) as f64 / n as f64).round() as u32, kelvin_between(from, to, (k + 1) as f32 / n as f32))).collect()
}

/// The way back up: it has to land on daylight at the end, where the identity profile goes, so the steps sit one slot later
/// and the last one (daylight again) is left out.
fn fade_out_steps(target: u32, minutes: u32) -> Vec<(u32, u32)> {
    let all = fade_steps(target, NEUTRAL, minutes);
    let n = all.len() as f64;
    all.iter().take(all.len() - 1).enumerate().map(|(i, &(_, t))| ((((i + 1) as u32 * minutes) as f64 / n).round() as u32, t)).collect()
}

fn profile(time: u32, temp: Option<u32>) -> String {
    match temp {
        Some(t) => format!("\nprofile {{\n    time = {}\n    temperature = {t}\n}}\n", fmt_time(time)),
        None => format!("\nprofile {{\n    time = {}\n    identity = true\n}}\n", fmt_time(time)),
    }
}

/// The whole file.
pub fn build(s: &Settings) -> String {
    let onoff = |b: bool| if b { "on" } else { "off" };
    let mut out = format!(
        "# yggdrasil: temperature={} schedule={} on={} off={} fade={} minutes={} light={}\n# Night Light, written by Yggdrasil (the notch, or Settings > Displays). Change it there; edits here are replaced.\n",
        s.temp, onoff(s.schedule), fmt_time(s.on), fmt_time(s.off), onoff(s.fade), s.minutes, onoff(s.light)
    );
    if !s.schedule {
        return out + &profile(0, s.light.then_some(s.temp));
    }
    let fout = fade_out_len(s);
    if fout == 0 {
        out += &profile(s.off, None);
    } else {
        for (at, t) in fade_out_steps(s.temp, fout) {
            out += &profile(s.off + at, Some(t));
        }
        out += &profile(s.off + fout, None);
    }
    let fin = fade_in_len(s);
    if fin == 0 {
        out += &profile(s.on, Some(s.temp));
    } else {
        for (at, t) in fade_steps(NEUTRAL, s.temp, fin) {
            out += &profile(s.on + at, Some(t));
        }
    }
    out
}

fn conf_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("hypr/hyprsunset.conf")
}

/// While the light is switched off in the middle of a schedule with a fade, hyprsunset is stopped until this time (seconds since
/// 1970), since its staircase would warm the screen again a minute later. In a file, so it outlasts the program that set it.
fn hold_path() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state/yggdrasil/nightlight-off-until")
}

/// The settings in the file, or the defaults.
pub fn load() -> Settings {
    std::fs::read_to_string(conf_path()).ok().and_then(|t| parse(&t)).unwrap_or_default()
}

fn save(s: &Settings) {
    let path = conf_path();
    let Some(dir) = path.parent() else { return };
    let _ = std::fs::create_dir_all(dir);
    // a file of the user's own is kept aside the first time, not lost
    if std::fs::read_to_string(&path).is_ok_and(|t| parse(&t).is_none()) {
        let _ = std::fs::rename(&path, dir.join("hyprsunset.conf.before-yggdrasil"));
    }
    let tmp = dir.join(".hyprsunset.conf.yggdrasil");
    if std::fs::write(&tmp, build(s)).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

// ---------------------------------------------------------------- the clock

fn now() -> (i64, u32) {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&t, &mut tm);
        (t, (tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec) as u32)
    }
}

/// Seconds from `secs_of_day` until the clock next shows `minute` (minutes into the day; a whole day when it shows it now).
fn until(secs_of_day: u32, minute: u32) -> u32 {
    match ((minute % 1440) * 60 + 86400 - secs_of_day) % 86400 {
        0 => 86400,
        d => d,
    }
}

/// When the schedule next agrees the light is off (the end of the fade out), as seconds from now; None when the next thing
/// it does is to turn the light on anyway.
fn hold_for(s: &Settings, secs_of_day: u32) -> Option<u32> {
    if !s.schedule || !s.fade {
        return None;
    }
    let lit_until = until(secs_of_day, s.off + fade_out_len(s));
    (lit_until < until(secs_of_day, s.on)).then_some(lit_until)
}

/// Whether the settings have the light on at `secs_of_day` (with a schedule: from the turn-on time to the end of the fade out).
fn lit_at(s: &Settings, secs_of_day: u32) -> bool {
    if !s.schedule {
        return s.light;
    }
    (secs_of_day / 60 + 1440 - s.on) % 1440 < span(s.on, s.off) + fade_out_len(s)
}

// ---------------------------------------------------------------- hyprsunset

/// hyprsunset's socket: $XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.hyprsunset.sock.
fn socket_path() -> PathBuf {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
    match std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        Some(sig) => run.join("hypr").join(sig).join(".hyprsunset.sock"),
        None => run.join("hypr/.hyprsunset.sock"),
    }
}

/// One request to hyprsunset ("temperature", "temperature 4000", ...) and its reply; None when it is not running.
fn ask(req: &str) -> Option<String> {
    let mut s = UnixStream::connect(socket_path()).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(500))).ok()?;
    s.write_all(req.as_bytes()).ok()?;
    let mut buf = [0u8; 1024];
    let n = s.read(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf[..n]).into_owned())
}

/// hyprsunset's temperature now; None when it is not running.
pub fn probe() -> Option<u32> {
    ask("temperature").and_then(|r| parse_probe(&r))
}

/// The reply to "temperature" is the number alone; anything longer is an error message, whatever digits it holds.
pub fn parse_probe(s: &str) -> Option<u32> {
    let s = s.trim();
    (s.len() <= 8).then(|| s.trim_end_matches('K').trim().parse().ok()).flatten()
}

pub fn is_on() -> bool {
    probe().is_some_and(|t| t < ON_BELOW)
}

/// The user's hyprsunset processes.
fn pids() -> Vec<i32> {
    use std::os::unix::fs::MetadataExt;
    let uid = unsafe { libc::getuid() };
    let Ok(dir) = std::fs::read_dir("/proc") else { return vec![] };
    dir.flatten()
        .filter_map(|e| {
            let pid: i32 = e.file_name().to_str()?.parse().ok()?;
            if e.metadata().ok()?.uid() != uid {
                return None;
            }
            (std::fs::read_to_string(e.path().join("comm")).ok()?.trim_end() == "hyprsunset").then_some(pid)
        })
        .collect()
}

/// Starts hyprsunset (in a process group of its own, so it outlives whoever started it) and waits up to 3 s until it answers.
fn start() {
    use std::os::unix::process::CommandExt;
    // one that is already starting (from the notch or Settings, whichever looked first) is waited for, not doubled
    if pids().is_empty() {
        if let Ok(mut child) = Command::new("hyprsunset").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).process_group(0).spawn() {
            // reaped when it ends
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }
    for _ in 0..60 {
        if probe().is_some() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn stop() {
    for pid in pids() {
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    for _ in 0..60 {
        if pids().is_empty() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Sets the temperature now. A hyprsunset that has just started applies its own at the end of starting, over anything sent
/// before, so this sends again until it reads back.
fn set_temp(t: u32) {
    for _ in 0..10 {
        let _ = ask(&format!("temperature {t}"));
        if probe() == Some(t) {
            return;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}

// ---------------------------------------------------------------- the worker

pub enum Cmd {
    /// On or off now.
    Toggle(bool),
    /// The settings changed.
    Set(Settings),
}

struct Worker {
    cur: Settings,
    /// Settings changed but not written yet (written once nothing has changed for a moment).
    unsaved: Option<Settings>,
    /// The schedule hyprsunset is running is out of date; it is restarted as soon as the light is off, so there is no flash.
    restart: bool,
}

impl Worker {
    fn toggle(&mut self, on: bool) {
        let _ = std::fs::remove_file(hold_path());
        if !self.cur.schedule {
            // a fresh hyprsunset takes the new midnight profile; from neutral to warm or warm to neutral, so nothing flashes
            self.cur.light = on;
            save(&self.cur);
            self.restart = false;
            stop();
            start();
            return;
        }
        if pids().is_empty() {
            start();
        }
        if on {
            set_temp(self.cur.temp);
            return;
        }
        // the light goes neutral first, so while hyprsunset is still there it reads as off
        set_temp(NEUTRAL);
        let (t, sod) = now();
        if let Some(secs) = hold_for(&self.cur, sod) {
            stop();
            let p = hold_path();
            if let Some(d) = p.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(p, (t + secs as i64).to_string());
        }
    }

    /// A new temperature shows at once while the light is on; the rest waits for `flush`.
    fn set(&mut self, s: Settings) {
        if s.temp != self.cur.temp && is_on() {
            set_temp(s.temp);
        }
        self.unsaved = Some(s);
    }

    fn flush(&mut self) {
        let Some(s) = self.unsaved.take() else { return };
        let schedule_changed = (s.schedule, s.on, s.off, s.fade, s.minutes) != (self.cur.schedule, self.cur.on, self.cur.off, self.cur.fade, self.cur.minutes);
        let temp_changed = s.temp != self.cur.temp;
        self.cur = s;
        save(&self.cur);
        if schedule_changed {
            // a new schedule is followed from now: the light comes on or goes off as it says
            let _ = std::fs::remove_file(hold_path());
            self.restart = false;
            stop();
            start();
        } else if temp_changed {
            // shown already; the profiles hyprsunset holds catch up when the light is next off
            self.restart = true;
        }
    }

    /// Every 30 seconds (and at the start): hyprsunset running, unless it is held off, and showing what it should.
    fn keep(&mut self) {
        if !conf_path().exists() {
            return;
        }
        if let Some(until) = std::fs::read_to_string(hold_path()).ok().and_then(|s| s.trim().parse::<i64>().ok()) {
            if now().0 < until {
                return;
            }
            let _ = std::fs::remove_file(hold_path());
        }
        let live = probe();
        if live.is_none() {
            start();
        } else if self.restart && live.is_some_and(|t| t >= ON_BELOW) && !lit_at(&self.cur, now().1) {
            // not while it is off only because it was switched off: a fresh hyprsunset would follow the schedule and light it
            stop();
            start();
            self.restart = false;
        }
        // without a schedule the light stays as it was left (a profile not yet caught up could change it at midnight)
        if !self.cur.schedule {
            match probe() {
                Some(t) if self.cur.light && t != self.cur.temp => set_temp(self.cur.temp),
                Some(t) if !self.cur.light && t < ON_BELOW => set_temp(NEUTRAL),
                _ => {}
            }
        }
    }
}

/// The thread that runs hyprsunset; send it what the user changes. The notch and Settings each have one; whichever is idle
/// reads the file again before it acts, so a change made in the other is not undone.
pub fn spawn() -> Sender<Cmd> {
    let (tx, rx) = channel::<Cmd>();
    std::thread::spawn(move || {
        let mut w = Worker { cur: load(), unsaved: None, restart: false };
        w.keep();
        loop {
            let wait = if w.unsaved.is_some() { Duration::from_millis(600) } else { Duration::from_secs(30) };
            let msg = rx.recv_timeout(wait);
            if w.unsaved.is_none() {
                w.cur = load();
            }
            match msg {
                Ok(Cmd::Toggle(on)) => {
                    w.flush();
                    w.toggle(on);
                }
                Ok(Cmd::Set(s)) => w.set(s),
                Err(RecvTimeoutError::Timeout) if w.unsaved.is_some() => w.flush(),
                Err(RecvTimeoutError::Timeout) => w.keep(),
                Err(RecvTimeoutError::Disconnected) => {
                    w.flush();
                    return;
                }
            }
        }
    });
    tx
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(fade: bool, minutes: u32) -> Settings {
        Settings { schedule: true, fade, minutes, ..Default::default() }
    }

    #[test]
    fn header_round_trip() {
        let s = Settings { temp: 3600, schedule: true, on: 21 * 60 + 15, off: 6 * 60 + 45, fade: true, minutes: 45, light: true };
        assert_eq!(parse(&build(&s)), Some(s));
        assert_eq!(parse("profile {\n time = 20:00\n temperature = 4000\n}\n"), None);
        // out of range values are brought back in, bad times left at the default
        let p = parse("# yggdrasil: temperature=900 on=25:00 minutes=999").unwrap();
        assert_eq!((p.temp, p.on, p.minutes), (MIN_TEMP, 20 * 60, MAX_FADE));
    }

    #[test]
    fn no_schedule_holds_one_profile() {
        let off = build(&Settings::default());
        assert_eq!(off.matches("profile {").count(), 1);
        assert!(off.contains("time = 00:00\n    identity = true"));
        assert!(build(&Settings { light: true, temp: 3200, ..Default::default() }).contains("time = 00:00\n    temperature = 3200"));
    }

    #[test]
    fn plain_schedule() {
        // the fade switched off keeps its minutes but writes no steps
        let c = build(&sched(false, 60));
        assert!(c.contains("time = 07:00\n    identity = true"));
        assert!(c.contains("time = 20:00\n    temperature = 4000"));
        assert_eq!(c.matches("profile {").count(), 2);
    }

    #[test]
    fn fade_steps_are_even_in_mired_and_land() {
        let st = fade_steps(NEUTRAL, 4000, 60);
        assert_eq!(st.len(), 60);
        assert_eq!(st[0].0, 0);
        assert_eq!(st.last().unwrap(), &(59, 4000));
        let m: Vec<f64> = st.iter().map(|&(_, t)| 1e6 / t as f64).collect();
        let d0 = m[1] - m[0];
        assert!(m.windows(2).all(|w| ((w[1] - w[0]) - d0).abs() < 0.2));
        // longer than an hour: still 60 steps, spread out
        let long = fade_steps(NEUTRAL, 3000, 120);
        assert_eq!((long.len(), long[1].0), (60, 2));
    }

    #[test]
    fn fade_out_lands_on_identity() {
        let c = build(&sched(true, 30));
        assert!(c.contains("time = 07:30\n    identity = true"));
        let out = fade_out_steps(4000, 30);
        assert_eq!(out.len(), 29);
        // the target holds until the first step, a minute in, and each step is cooler than the one before
        assert_eq!(out[0].0, 1);
        assert!(out[0].1 > 4000 && out.windows(2).all(|w| w[1].1 > w[0].1));
        assert!(out.iter().all(|&(at, t)| at < 30 && t < NEUTRAL));
    }

    #[test]
    fn fades_are_cut_to_fit() {
        // on at 20:00, off at 20:30: the fade in has 29 minutes, the fade out the whole rest of the day
        let s = Settings { on: 20 * 60, off: 20 * 60 + 30, ..sched(true, 240) };
        assert_eq!((fade_in_len(&s), fade_out_len(&s)), (29, 240));
        assert_eq!(ramps(&s).unwrap()[0], (20 * 60, 20 * 60 + 29, NEUTRAL, 4000));
        assert_eq!(ramps(&sched(false, 60)), None);
    }

    #[test]
    fn hold_only_inside_the_lit_stretch() {
        let s = sched(true, 60);
        // 23:00: lit until 08:00, nine hours on
        assert_eq!(hold_for(&s, 23 * 3600), Some(9 * 3600));
        // 07:30, in the fade out: until 08:00
        assert_eq!(hold_for(&s, 7 * 3600 + 1800), Some(1800));
        // noon: the next thing is the turn-on at 20:00
        assert_eq!(hold_for(&s, 12 * 3600), None);
        assert_eq!(hold_for(&sched(false, 60), 23 * 3600), None);
    }

    #[test]
    fn lit_stretch() {
        let s = sched(true, 60);
        assert!(lit_at(&s, 20 * 3600) && lit_at(&s, 3 * 3600) && lit_at(&s, 7 * 3600 + 3540));
        assert!(!lit_at(&s, 8 * 3600) && !lit_at(&s, 19 * 3600 + 3540));
        assert!(lit_at(&Settings { light: true, ..Default::default() }, 12 * 3600));
    }

    #[test]
    fn probe_output() {
        assert_eq!(parse_probe("4000"), Some(4000));
        assert_eq!(parse_probe("Invalid temperature (should be an integer in range 1000-20000)"), None);
        assert_eq!(parse_probe(""), None);
    }

    #[test]
    fn colours_warm_to_white() {
        assert_eq!(kelvin_rgb(6600), (255, 255, 255));
        let (r, g, b) = kelvin_rgb(2000);
        assert!(r == 255 && g < 160 && b < 40);
        assert_eq!(kelvin_between(NEUTRAL, 4000, 1.0), 4000);
    }
}
