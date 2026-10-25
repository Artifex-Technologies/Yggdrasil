//! The timers: what is running, the pill the panel draws, and the alarms.
//!
//! Nothing runs while a timer counts down. Starting one writes `state.json` (the timers) and `bar.json` (the pill: the panel
//! counts down to its `until` by itself) and asks systemd for a one-off user timer that runs `yggdrasil-timer ring ID GEN` at the
//! end. Pausing or cancelling stops that systemd timer.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Timer {
    pub id: u64,
    #[serde(default)]
    pub label: String,
    /// The length it was started with, in seconds (for Restart).
    pub duration: i64,
    /// Running: when it ends (seconds since 1970).
    #[serde(default)]
    pub until: Option<i64>,
    /// Paused: the seconds that were left.
    #[serde(default)]
    pub left: Option<i64>,
    #[serde(default)]
    pub done: bool,
    /// Bumped each time its alarm is set again, so an old alarm that still fires is ignored.
    #[serde(default)]
    pub generation: u32,
}

impl Timer {
    pub fn running(&self) -> bool {
        !self.done && self.until.is_some()
    }

    pub fn paused(&self) -> bool {
        !self.done && self.until.is_none()
    }

    /// Seconds left at `now`.
    pub fn remaining(&self, now: i64) -> i64 {
        if self.done {
            0
        } else {
            match self.until {
                Some(u) => (u - now).max(0),
                None => self.left.unwrap_or(0),
            }
        }
    }

    fn unit(&self) -> String {
        format!("yggdrasil-timer-{}-{}", self.id, self.generation)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub timers: Vec<Timer>,
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

/// "7" (minutes), "1:30" (m:ss), "1:02:03" (h:mm:ss), "90s", "1h30m", "1h 5m 10s", "25 min": the seconds, or None.
pub fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim().to_lowercase();
    if s.is_empty() {
        return None;
    }
    let secs = if s.contains(':') {
        let parts: Vec<i64> = s.split(':').map(|p| p.trim().parse().ok()).collect::<Option<_>>()?;
        match parts.as_slice() {
            [m, sec] if *sec < 60 => m * 60 + sec,
            [h, m, sec] if *m < 60 && *sec < 60 => h * 3600 + m * 60 + sec,
            _ => return None,
        }
    } else if let Ok(m) = s.parse::<i64>() {
        m * 60
    } else {
        let (mut total, mut num, mut unit) = (0i64, String::new(), String::new());
        let mut flush = |num: &mut String, unit: &mut String| -> Option<()> {
            if num.is_empty() {
                return if unit.is_empty() { Some(()) } else { None };
            }
            let n: i64 = num.parse().ok()?;
            total += n * match unit.as_str() {
                "h" | "hr" | "hrs" | "hour" | "hours" => 3600,
                "m" | "min" | "mins" | "minute" | "minutes" | "" => 60,
                "s" | "sec" | "secs" | "second" | "seconds" => 1,
                _ => return None,
            };
            num.clear();
            unit.clear();
            Some(())
        };
        for c in s.chars() {
            if c.is_ascii_digit() {
                if !unit.is_empty() {
                    flush(&mut num, &mut unit)?;
                }
                num.push(c);
            } else if c.is_alphabetic() {
                if num.is_empty() {
                    return None;
                }
                unit.push(c);
            } else if !c.is_whitespace() {
                return None;
            }
        }
        flush(&mut num, &mut unit)?;
        total
    };
    (secs > 0 && secs <= 100 * 3600).then_some(secs)
}

/// m:ss, or h:mm:ss from an hour up.
pub fn clock(secs: i64) -> String {
    let d = secs.max(0);
    if d >= 3600 {
        format!("{}:{:02}:{:02}", d / 3600, d / 60 % 60, d % 60)
    } else {
        format!("{}:{:02}", d / 60, d % 60)
    }
}

/// "5 min", "1 h 30 min", "90 s" for a timer with no label.
pub fn describe(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = vec![];
    if h > 0 {
        out.push(format!("{h} h"));
    }
    if m > 0 {
        out.push(format!("{m} min"));
    }
    if s > 0 {
        out.push(format!("{s} s"));
    }
    out.join(" ")
}

fn short(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(n - 1).collect::<String>())
    }
}

/// The pill: a finished timer first (red, until dismissed), else the one that ends soonest, else a paused one, else no text: the
/// panel then shows the timer's icon alone, still there to click.
pub fn bar_json(st: &State, now: i64) -> serde_json::Value {
    let label = |t: &Timer| if t.label.trim().is_empty() { String::new() } else { format!("{} ", short(t.label.trim(), 14)) };
    let done: Vec<&Timer> = st.timers.iter().filter(|t| t.done).collect();
    let mut running: Vec<&Timer> = st.timers.iter().filter(|t| t.running()).collect();
    running.sort_by_key(|t| t.until);
    let paused: Vec<&Timer> = st.timers.iter().filter(|t| t.paused()).collect();
    let more = |n: usize| if n > 1 { format!(" +{}", n - 1) } else { String::new() };
    if let Some(t) = done.first() {
        return json!({"text": format!("{}Done{}", label(t), more(done.len())), "state": "alert"});
    }
    if let Some(t) = running.first() {
        return json!({"text": format!("{}{{countdown}}{}", label(t), more(running.len() + paused.len())), "state": "ok", "until": t.until});
    }
    if let Some(t) = paused.first() {
        return json!({"text": format!("{}Paused {}{}", label(t), clock(t.remaining(now)), more(paused.len())), "state": "warn"});
    }
    json!({"text": "", "state": "ok"})
}

/// The timer the window shows (one at a time): a finished one, else the running one, else a paused one.
pub fn current(st: &State) -> Option<&Timer> {
    st.timers.iter().find(|t| t.done).or_else(|| st.timers.iter().filter(|t| t.running()).min_by_key(|t| t.until)).or_else(|| st.timers.iter().find(|t| t.paused()))
}

/// "25m", "1h30m", "1m30s": a length as the window's custom field shows it (parse_duration reads it back).
pub fn compact(secs: i64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    let mut out = String::new();
    if h > 0 {
        out.push_str(&format!("{h}h"));
    }
    if m > 0 || (h == 0 && s == 0) {
        out.push_str(&format!("{m}m"));
    }
    if s > 0 {
        out.push_str(&format!("{s}s"));
    }
    out
}

// ---------------------------------------------------------------- files

pub fn dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/state")
    });
    base.join("yggdrasil/timer")
}

fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    let mut f = File::create(&tmp)?;
    f.write_all(data)?;
    f.sync_all().ok();
    fs::rename(&tmp, path)
}

/// The timers on disk, held under a lock while `change` runs, then written with the pill. The window, an alarm and the
/// reconcile at login can all touch them at once.
pub struct Store {
    pub dir: PathBuf,
}

impl Store {
    pub fn new(dir: PathBuf) -> Store {
        Store { dir }
    }

    pub fn state_path(&self) -> PathBuf {
        self.dir.join("state.json")
    }

    pub fn load(&self) -> State {
        fs::read_to_string(self.state_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn change<R>(&self, change: impl FnOnce(&mut State) -> R) -> std::io::Result<(State, R)> {
        fs::create_dir_all(&self.dir)?;
        let lock = File::create(self.dir.join(".lock"))?;
        lock.lock()?;
        let mut st = self.load();
        let r = change(&mut st);
        write_atomic(&self.state_path(), serde_json::to_string_pretty(&st)?.as_bytes())?;
        write_atomic(&self.dir.join("bar.json"), bar_json(&st, now()).to_string().as_bytes())?;
        Ok((st, r))
    }
}

// ---------------------------------------------------------------- changes (pure: the alarms are set by the caller)

pub fn start(st: &mut State, secs: i64, label: &str, now: i64) -> Timer {
    let id = st.timers.iter().map(|t| t.id).max().unwrap_or(0).max(now as u64 * 10) + 1;
    let t = Timer { id, label: label.trim().to_string(), duration: secs, until: Some(now + secs), ..Default::default() };
    st.timers.push(t.clone());
    t
}

fn find(st: &mut State, id: u64) -> Option<&mut Timer> {
    st.timers.iter_mut().find(|t| t.id == id)
}

/// Pause or resume. Returns the timer as it is now.
pub fn toggle(st: &mut State, id: u64, now: i64) -> Option<Timer> {
    let t = find(st, id)?;
    if t.running() {
        t.left = Some(t.remaining(now));
        t.until = None;
    } else if t.paused() {
        t.until = Some(now + t.left.take().unwrap_or(0));
        t.generation += 1;
    }
    Some(t.clone())
}

pub fn add(st: &mut State, id: u64, secs: i64, now: i64) -> Option<Timer> {
    let t = find(st, id)?;
    if t.running() {
        t.until = Some(t.remaining(now) + now + secs);
        t.generation += 1;
    } else if t.paused() {
        t.left = Some(t.left.unwrap_or(0) + secs);
    }
    Some(t.clone())
}

pub fn restart(st: &mut State, id: u64, now: i64) -> Option<Timer> {
    let t = find(st, id)?;
    *t = Timer { until: Some(now + t.duration), left: None, done: false, generation: t.generation + 1, ..t.clone() };
    Some(t.clone())
}

pub fn remove(st: &mut State, id: u64) -> Option<Timer> {
    let i = st.timers.iter().position(|t| t.id == id)?;
    Some(st.timers.remove(i))
}

/// Timers whose end has passed are done (an alarm lost to a restart). Returns them.
pub fn expire(st: &mut State, now: i64) -> Vec<Timer> {
    let mut out = vec![];
    for t in st.timers.iter_mut().filter(|t| t.running() && t.until.unwrap_or(0) <= now) {
        t.done = true;
        out.push(t.clone());
    }
    out
}

// ---------------------------------------------------------------- alarms

fn me() -> String {
    std::env::current_exe().ok().map(|p| p.to_string_lossy().into_owned()).filter(|p| !p.contains(" (deleted)")).unwrap_or_else(|| "/usr/bin/yggdrasil-timer".into())
}

fn quiet(cmd: &mut Command) -> bool {
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Asks systemd to run `yggdrasil-timer ring ID GEN` when `t` ends.
pub fn arm(t: &Timer, now: i64) -> bool {
    let Some(until) = t.until else { return false };
    quiet(Command::new("systemd-run").args([
        "--user",
        "--quiet",
        "--collect",
        &format!("--unit={}", t.unit()),
        &format!("--on-active={}s", (until - now).max(1)),
        "--timer-property=AccuracySec=1s",
        &me(),
        "ring",
        &t.id.to_string(),
        &t.generation.to_string(),
    ]))
}

pub fn disarm(t: &Timer) {
    // stopping the .timer leaves its .service never started; generation numbers keep a late one harmless anyway
    quiet(Command::new("systemctl").args(["--user", "stop", &format!("{}.timer", t.unit())]));
}

pub fn armed(t: &Timer) -> bool {
    quiet(Command::new("systemctl").args(["--user", "--quiet", "is-active", &format!("{}.timer", t.unit())]))
}

const SOUNDS: [&str; 3] = [
    "/usr/share/sounds/freedesktop/stereo/alarm-clock-elapsed.oga",
    "/usr/share/sounds/freedesktop/stereo/complete.oga",
    "/usr/share/sounds/freedesktop/stereo/bell.oga",
];

/// The timer's own icon (installed with it), else the theme's alarm.
pub const ICON: &str = "/usr/share/yggdrasil/icons/timer-symbolic.svg";

/// The alert: a notification, and a sound when the system has one.
pub fn alert(t: &Timer) {
    let body = if t.label.trim().is_empty() { format!("{} is up.", describe(t.duration)) } else { format!("{} ({}) is up.", t.label.trim(), describe(t.duration)) };
    let icon = if Path::new(ICON).is_file() { ICON } else { "alarm-symbolic" };
    quiet(Command::new("notify-send").args(["-a", "Timer", "-i", icon, "-u", "critical", "Time's up", &body]));
    if quiet(Command::new("canberra-gtk-play").args(["-i", "alarm-clock-elapsed", "-d", "Timer"])) {
        return;
    }
    if let Some(f) = SOUNDS.iter().find(|f| Path::new(f).is_file()) {
        let _ = quiet(Command::new("pw-play").arg(f)) || quiet(Command::new("paplay").arg(f));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_are_read() {
        for (s, want) in [
            ("7", Some(420)),
            (" 25 ", Some(1500)),
            ("1:30", Some(90)),
            ("1:02:03", Some(3723)),
            ("90s", Some(90)),
            ("1h30m", Some(5400)),
            ("1h 5m 10s", Some(3910)),
            ("25 min", Some(1500)),
            ("2 hours", Some(7200)),
            ("1h30", Some(5400)),
            ("0", None),
            ("", None),
            ("1:75", None),
            ("abc", None),
            ("5x", None),
            ("-5", None),
            ("200h", None),
        ] {
            assert_eq!(parse_duration(s), want, "{s:?}");
        }
    }

    #[test]
    fn compact_lengths_read_back() {
        for (secs, text) in [(1500, "25m"), (5400, "1h30m"), (90, "1m30s"), (3600, "1h"), (45, "45s")] {
            assert_eq!(compact(secs), text);
            assert_eq!(parse_duration(text), Some(secs), "{text}");
        }
    }

    #[test]
    fn clocks_and_names() {
        assert_eq!(clock(125), "2:05");
        assert_eq!(clock(3725), "1:02:05");
        assert_eq!(describe(5400), "1 h 30 min");
        assert_eq!(describe(90), "1 min 30 s");
    }

    #[test]
    fn the_pill_follows_the_timers() {
        let mut st = State::default();
        assert_eq!(bar_json(&st, 0), json!({"text": "", "state": "ok"}), "idle: the icon alone");
        assert!(current(&st).is_none());
        let tea = start(&mut st, 300, "Tea", 1000);
        let eggs = start(&mut st, 120, "", 1000);
        assert_ne!(tea.id, eggs.id);
        // the one ending soonest, with the others counted
        assert_eq!(bar_json(&st, 1000), json!({"text": "{countdown} +1", "state": "ok", "until": 1120}));
        toggle(&mut st, eggs.id, 1060).unwrap();
        assert_eq!(bar_json(&st, 1060), json!({"text": "Tea {countdown} +1", "state": "ok", "until": 1300}));
        remove(&mut st, tea.id);
        assert_eq!(bar_json(&st, 1070)["text"], "Paused 1:00");
        // resumed: the minute it had left
        let t = toggle(&mut st, eggs.id, 2000).unwrap();
        assert_eq!((t.until, t.generation), (Some(2060), 1));
        let t = add(&mut st, eggs.id, 60, 2000).unwrap();
        assert_eq!(t.until, Some(2120));
        assert!(expire(&mut st, 2119).is_empty());
        assert_eq!(expire(&mut st, 2120).len(), 1);
        assert_eq!(bar_json(&st, 2120), json!({"text": "Done", "state": "alert"}));
        assert_eq!(current(&st).map(|t| t.id), Some(eggs.id), "a finished timer is the one shown");
        let t = restart(&mut st, eggs.id, 3000).unwrap();
        assert_eq!((t.until, t.done), (Some(3120), false));
    }

    #[test]
    fn the_store_writes_state_and_pill() {
        let d = std::env::temp_dir().join(format!("ygg-timer-test-{}", std::process::id()));
        let s = Store::new(d.clone());
        let (_, t) = s.change(|st| start(st, 60, "Pasta", now())).unwrap();
        assert_eq!(s.load().timers, vec![t.clone()]);
        let bar: serde_json::Value = serde_json::from_str(&fs::read_to_string(d.join("bar.json")).unwrap()).unwrap();
        assert_eq!(bar["text"], "Pasta {countdown}");
        assert_eq!(bar["until"], t.until.unwrap());
        fs::remove_dir_all(&d).ok();
    }
}
