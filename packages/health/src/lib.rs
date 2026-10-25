//! Break reminders: the settings, what has been sent, and the schedule (when each reminder is due, quiet hours, pausing).
//!
//! Interval reminders (eyes, movement, water, breathing) count from the last one; clock reminders (meal, wind-down) come once a
//! day at their time. Nothing is sent while paused, in quiet hours or while the session is locked or idle: an interval that comes
//! due then starts again from that moment (no burst of overdue nudges), and a clock reminder waits until you are back that day.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The reminders, in the order the window lists them.
pub const STREAMS: [&str; 7] = ["eye", "eyeexercise", "move", "water", "breath", "meal", "winddown"];

/// A clock reminder this late (the computer was off at its time) is let go for the day.
pub const CLOCK_GRACE_MIN: i64 = 180;

pub fn label(name: &str) -> &'static str {
    match name {
        "eye" => "Eyes",
        "eyeexercise" => "Eye exercise",
        "move" => "Move",
        "water" => "Water",
        "breath" => "Breathe",
        "meal" => "Meal",
        "winddown" => "Wind down",
        _ => "Reminder",
    }
}

pub fn about(name: &str) -> &'static str {
    match name {
        "eye" => "20-20-20: look 20 feet away for 20 seconds",
        "eyeexercise" => "A routine your eye-care professional gave you (off unless you turn it on)",
        "move" => "Stand up, stretch, check your posture",
        "water" => "Drink some water",
        "breath" => "A minute of slow breathing",
        "meal" => "Eat away from the screen",
        "winddown" => "Step away from screens before bed",
        _ => "",
    }
}

/// What a reminder says: (heading, text), one of a few so they do not all read the same.
pub fn messages(name: &str) -> &'static [(&'static str, &'static str)] {
    match name {
        "eye" => &[
            ("Eyes: 20-20-20", "Look at something 20 feet away for 20 seconds."),
            ("Eye break", "Focus on a distant object for 20 seconds, then blink slowly."),
            ("Rest your eyes", "Look far away for 20 seconds and let your focus relax."),
            ("Screen glare", "Match the screen's brightness to the room; avoid working in the dark."),
            ("Blink", "Blink fully a few times to wet your eyes again."),
        ],
        "eyeexercise" => &[("Eye exercise", "Do your 5-minute eye push-up exercise.")],
        "move" => &[
            ("Stand up", "Stretch, roll your shoulders and walk for a minute."),
            ("Posture check", "Sit up, drop your shoulders, flex your wrists."),
            ("Move", "Take a short walk: even 60 seconds resets your focus."),
            ("Neck and shoulders", "Gently roll your neck and shoulders to release stiffness."),
        ],
        "water" => &[
            ("Hydrate", "Drink some water."),
            ("Water break", "Time for a glass of water; keep one at your desk."),
            ("Sip", "Take a few sips of water to stay sharp."),
        ],
        "breath" => &[
            ("Breathe", "Box breathing: in 4 s, hold 4 s, out 4 s, hold 4 s. One minute."),
            ("Reset", "Pause and take five slow breaths. Drop your shoulders."),
            ("Stress reset", "One minute of slow breathing to clear your head."),
        ],
        "meal" => &[
            ("Meal time", "Step away and eat, away from the screen if you can."),
            ("Lunch break", "Eat away from the desk: a real break for eyes and posture."),
        ],
        "winddown" => &[
            ("Wind down", "Step away from the screen; dim the room and start getting ready for sleep."),
            ("Screens off soon", "Wrapping up? Give your eyes and brain a break before bed."),
            ("Sleep prep", "Dim the lights and stop scrolling: protect tomorrow's focus."),
        ],
        _ => &[],
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Stream {
    pub enabled: bool,
    /// Minutes between reminders (interval reminders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub every: Option<u32>,
    /// "HH:MM" once a day (clock reminders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<String>,
}

impl Stream {
    pub fn default_for(name: &str) -> Stream {
        let (enabled, every, at) = match name {
            "eye" => (true, Some(20), None),
            "eyeexercise" => (false, Some(300), None),
            "move" => (true, Some(45), None),
            "water" => (true, Some(90), None),
            "breath" => (true, Some(180), None),
            "meal" => (true, None, Some("14:00")),
            "winddown" => (true, None, Some("23:30")),
            _ => (false, Some(60), None),
        };
        Stream { enabled, every, at: at.map(String::from) }
    }
    pub fn at_min(&self) -> Option<i64> {
        self.at.as_deref().and_then(parse_hm)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quiet {
    pub enabled: bool,
    pub from: String,
    pub to: String,
}

impl Default for Quiet {
    fn default() -> Self {
        Quiet { enabled: false, from: "22:00".into(), to: "08:00".into() }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// The master switch.
    pub reminders: bool,
    /// A soft sound with each reminder.
    pub sound: bool,
    pub quiet: Quiet,
    /// Nothing is sent before this moment (seconds since 1970).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paused_until: Option<i64>,
    pub streams: BTreeMap<String, Stream>,
}

impl Default for Config {
    fn default() -> Self {
        Config { reminders: true, sound: true, quiet: Quiet::default(), paused_until: None, streams: BTreeMap::new() }.filled()
    }
}

impl Config {
    /// Every known reminder present (a settings file from an older version may lack some).
    pub fn filled(mut self) -> Self {
        for s in STREAMS {
            self.streams.entry(s.into()).or_insert_with(|| Stream::default_for(s));
        }
        self
    }
    pub fn stream(&self, name: &str) -> Stream {
        self.streams.get(name).cloned().unwrap_or_else(|| Stream::default_for(name))
    }
    pub fn paused(&self, now: i64) -> bool {
        self.paused_until.is_some_and(|u| now < u)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    /// When each interval reminder last went out (or was started again).
    pub last: BTreeMap<String, i64>,
    /// The day ("YYYY-MM-DD") each clock reminder last went out.
    pub fired_today: BTreeMap<String, String>,
}

/// The local time a decision is made at.
#[derive(Clone, Debug, PartialEq)]
pub struct Clock {
    pub epoch: i64,
    /// Local date, "YYYY-MM-DD".
    pub day: String,
    /// Seconds since local midnight.
    pub sod: i64,
}

impl Clock {
    pub fn now() -> Clock {
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            Clock {
                epoch: t as i64,
                day: format!("{:04}-{:02}-{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday),
                sod: (tm.tm_hour * 3600 + tm.tm_min * 60 + tm.tm_sec) as i64,
            }
        }
    }
    pub fn minute(&self) -> i64 {
        self.sod / 60
    }
    /// The local minute of the day at another moment (close to now: daylight saving changes are ignored).
    pub fn minute_at(&self, t: i64) -> i64 {
        (self.sod + (t - self.epoch)).rem_euclid(86_400) / 60
    }
    /// Local midnight at the start of tomorrow.
    pub fn tomorrow(&self) -> i64 {
        self.epoch - self.sod + 86_400
    }
}

/// "H:MM" or "HH:MM" as minutes since midnight.
pub fn parse_hm(s: &str) -> Option<i64> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m): (i64, i64) = (h.parse().ok()?, m.parse().ok()?);
    ((0..24).contains(&h) && (0..60).contains(&m)).then_some(h * 60 + m)
}

/// Whether `minute` (of the day) falls in quiet hours; they may cross midnight (22:00 to 08:00).
pub fn in_quiet(q: &Quiet, minute: i64) -> bool {
    let (Some(from), Some(to)) = (parse_hm(&q.from), parse_hm(&q.to)) else { return false };
    if !q.enabled || from == to {
        return false;
    }
    if from < to {
        (from..to).contains(&minute)
    } else {
        minute >= from || minute < to
    }
}

/// One step of the schedule at `clock`: the reminders to send now. `away` is asked only when something is due.
pub fn tick(cfg: &Config, st: &mut State, clock: &Clock, away: &mut dyn FnMut() -> bool) -> Vec<String> {
    let mut out = vec![];
    if !cfg.reminders {
        return out;
    }
    let hush = cfg.paused(clock.epoch) || in_quiet(&cfg.quiet, clock.minute());
    let mut away_now: Option<bool> = None;
    let mut is_away = || *away_now.get_or_insert_with(&mut *away);
    for name in STREAMS {
        let s = cfg.stream(name);
        if !s.enabled {
            continue;
        }
        if let Some(at) = s.at_min() {
            if st.fired_today.get(name) == Some(&clock.day) || clock.minute() < at {
                continue;
            }
            // paused, quiet, or long past (the computer was off): let it go for today
            if hush || clock.minute() - at > CLOCK_GRACE_MIN {
                st.fired_today.insert(name.into(), clock.day.clone());
                continue;
            }
            // away: still pending, it goes out when you are back today
            if is_away() {
                continue;
            }
            out.push(name.to_string());
            st.fired_today.insert(name.into(), clock.day.clone());
        } else if let Some(every) = s.every.filter(|e| *e > 0) {
            let Some(&last) = st.last.get(name) else {
                st.last.insert(name.into(), clock.epoch);
                continue;
            };
            if clock.epoch - last < every as i64 * 60 {
                continue;
            }
            if !hush && !is_away() {
                out.push(name.to_string());
            }
            // sent or not, it counts again from now
            st.last.insert(name.into(), clock.epoch);
        }
    }
    out
}

/// The next reminder and when it is due, or None (all off, or paused).
pub fn next_due(cfg: &Config, st: &State, clock: &Clock) -> Option<(String, i64)> {
    if !cfg.reminders || cfg.paused(clock.epoch) {
        return None;
    }
    let mut best: Option<(String, i64)> = None;
    for name in STREAMS {
        let s = cfg.stream(name);
        if !s.enabled {
            continue;
        }
        let due = if let Some(at) = s.at_min() {
            if in_quiet(&cfg.quiet, at) {
                continue;
            }
            let today = clock.epoch - clock.sod + at * 60;
            let done = st.fired_today.get(name) == Some(&clock.day) || clock.minute() - at > CLOCK_GRACE_MIN;
            if done {
                today + 86_400
            } else {
                today.max(clock.epoch)
            }
        } else if let Some(every) = s.every.filter(|e| *e > 0) {
            let step = every as i64 * 60;
            let mut due = st.last.get(name).copied().unwrap_or(clock.epoch) + step;
            // each time it comes due in quiet hours it starts again: the first one after them is the one sent
            let mut guard = 0;
            while in_quiet(&cfg.quiet, clock.minute_at(due)) && guard < 2000 {
                due += step;
                guard += 1;
            }
            if guard == 2000 {
                continue;
            }
            due.max(clock.epoch)
        } else {
            continue;
        };
        if best.as_ref().is_none_or(|(_, b)| due < *b) {
            best = Some((name.to_string(), due));
        }
    }
    best
}

/// "every 20 min", "every 3 h", "every 1 h 30 min", or "at 14:00".
pub fn cadence(s: &Stream) -> String {
    if let Some(at) = &s.at {
        return format!("at {at}");
    }
    format!("every {}", span(s.every.unwrap_or(0) as i64))
}

/// Minutes as "20 min", "3 h", "1 h 30 min".
pub fn span(min: i64) -> String {
    match (min / 60, min % 60) {
        (0, m) => format!("{m} min"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} min"),
    }
}

fn hm(min: i64) -> String {
    let m = min.rem_euclid(1440);
    format!("{:02}:{:02}", m / 60, m % 60)
}

/// The step an interval moves by: 5 minutes up to an hour, 15 above.
fn interval_step(every: i64, up: bool) -> i64 {
    let e = if up { every } else { every - 1 };
    if e < 60 { 5 } else { 15 }
}

/// What the top bar pill shows, with its card (Yggdrasil's bar-item JSON; the card's controls run `yggdrasil-health press ID`).
pub fn bar(cfg: &Config, st: &State, clock: &Clock) -> serde_json::Value {
    use serde_json::json;
    let mut v = if !cfg.reminders {
        json!({"text": "Off", "help": "Break reminders are off."})
    } else if let Some(u) = cfg.paused_until.filter(|_| cfg.paused(clock.epoch)) {
        if u - clock.epoch <= 12 * 3600 {
            json!({"text": "Paused {countdown}", "until": u, "state": "warn", "help": "Reminders are paused."})
        } else {
            json!({"text": "Paused", "state": "warn", "help": "Reminders are paused until tomorrow."})
        }
    } else {
        match next_due(cfg, st, clock) {
            Some((name, t)) => json!({"text": "{countdown}", "until": t, "help": format!("Next: {}", label(&name))}),
            None => json!({"text": "On", "help": "No reminder is turned on."}),
        }
    };
    v["card"] = serde_json::Value::Array(card(cfg, st, clock));
    v
}

/// The card's blocks (docs/INTEGRATIONS.md, "Cards you can use").
pub fn card(cfg: &Config, st: &State, clock: &Clock) -> Vec<serde_json::Value> {
    use serde_json::json;
    let mut c = vec![];
    // what comes next, or why nothing does
    if !cfg.reminders {
        c.push(json!({"big": "Off", "caption": "Reminders are off"}));
    } else if let Some(u) = cfg.paused_until.filter(|_| cfg.paused(clock.epoch)) {
        if u - clock.epoch <= 12 * 3600 {
            c.push(json!({"big": "{countdown}", "caption": "Paused"}));
        } else {
            c.push(json!({"big": "Paused", "caption": "until tomorrow"}));
        }
    } else if let Some((name, due)) = next_due(cfg, st, clock) {
        let mut big = json!({"big": "{countdown}", "caption": format!("{}: {}", label(&name), about(&name))});
        if let Some(every) = cfg.stream(&name).every.filter(|e| *e > 0) {
            big["progress"] = json!([due - every as i64 * 60, due]);
        }
        c.push(big);
    } else {
        c.push(json!({"big": "On", "caption": "No reminder is turned on"}));
    }
    c.push(json!({"buttons": [
        {"id": "reminders", "label": if cfg.reminders { "Reminders on" } else { "Reminders off" }, "accent": cfg.reminders},
        {"id": "quiet", "label": if cfg.quiet.enabled { "Quiet hours on" } else { "Quiet hours off" }, "accent": cfg.quiet.enabled},
        {"id": "sound", "label": if cfg.sound { "Sound on" } else { "Sound off" }, "accent": cfg.sound},
    ], "columns": 3}));

    c.push(json!({"section": "REMINDERS"}));
    let rows: Vec<_> = STREAMS
        .iter()
        .map(|n| {
            let s = cfg.stream(n);
            json!({"id": format!("toggle:{n}"), "title": label(n), "note": cadence(&s), "value": if s.enabled { "On" } else { "Off" }, "on": s.enabled})
        })
        .collect();
    c.push(json!({"list": rows}));

    let on_intervals: Vec<_> = STREAMS.iter().filter(|n| cfg.stream(n).enabled && cfg.stream(n).every.is_some()).collect();
    if !on_intervals.is_empty() {
        c.push(json!({"section": "INTERVALS"}));
        for n in on_intervals {
            let every = cfg.stream(n).every.unwrap_or(0) as i64;
            c.push(json!({"stepper": {"label": label(n), "value": span(every), "minus": format!("int:{n}:-"), "plus": format!("int:{n}:+")}}));
        }
    }
    let on_clock: Vec<_> = STREAMS.iter().filter(|n| cfg.stream(n).enabled && cfg.stream(n).at.is_some()).collect();
    if !on_clock.is_empty() || cfg.quiet.enabled {
        c.push(json!({"section": "TIMES"}));
        for n in on_clock {
            let at = cfg.stream(n).at.unwrap_or_default();
            c.push(json!({"stepper": {"label": label(n), "value": at, "minus": format!("at:{n}:-"), "plus": format!("at:{n}:+")}}));
        }
        if cfg.quiet.enabled {
            c.push(json!({"stepper": {"label": "Quiet from", "value": cfg.quiet.from, "minus": "quiet:from:-", "plus": "quiet:from:+"}}));
            c.push(json!({"stepper": {"label": "Quiet until", "value": cfg.quiet.to, "minus": "quiet:to:-", "plus": "quiet:to:+"}}));
        }
    }

    if cfg.reminders {
        c.push(json!({"section": "PAUSE"}));
        if cfg.paused(clock.epoch) {
            c.push(json!({"buttons": [{"id": "resume", "label": "Resume", "accent": true}], "columns": 1}));
        } else {
            c.push(json!({"buttons": [
                {"id": "pause:15", "label": "15 min"},
                {"id": "pause:60", "label": "1 h"},
                {"id": "pause:tomorrow", "label": "Until tomorrow"},
            ], "columns": 3}));
        }
    }
    c
}

/// Pauses for minutes ("15") or until midnight ("tomorrow").
pub fn pause(cfg: &mut Config, how: &str, clock: &Clock) -> bool {
    cfg.paused_until = match how {
        "tomorrow" => Some(clock.tomorrow()),
        m => match m.parse::<i64>() {
            Ok(m) if m > 0 => Some(clock.epoch + m * 60),
            _ => return false,
        },
    };
    true
}

/// A press on the card's control `id`: changes the settings. False for an id the card never offers.
pub fn press(cfg: &mut Config, id: &str, clock: &Clock) -> bool {
    let parts: Vec<&str> = id.split(':').collect();
    let stream = |n: &str| STREAMS.contains(&n);
    match parts.as_slice() {
        ["reminders"] => cfg.reminders = !cfg.reminders,
        ["sound"] => cfg.sound = !cfg.sound,
        ["quiet"] => cfg.quiet.enabled = !cfg.quiet.enabled,
        ["resume"] => cfg.paused_until = None,
        ["pause", how] => return pause(cfg, how, clock),
        ["toggle", n] if stream(n) => {
            let mut s = cfg.stream(n);
            s.enabled = !s.enabled;
            cfg.streams.insert(n.to_string(), s);
        }
        ["int", n, d @ ("-" | "+")] if stream(n) && cfg.stream(n).every.is_some() => {
            let mut s = cfg.stream(n);
            let every = s.every.unwrap_or(60) as i64;
            let up = *d == "+";
            let step = interval_step(every, up);
            let next = if up { every + step } else { every - step };
            s.every = Some(next.clamp(5, 600) as u32);
            cfg.streams.insert(n.to_string(), s);
        }
        ["at", n, d @ ("-" | "+")] if stream(n) && cfg.stream(n).at.is_some() => {
            let mut s = cfg.stream(n);
            let m = s.at_min().unwrap_or(720) + if *d == "+" { 15 } else { -15 };
            s.at = Some(hm(m));
            cfg.streams.insert(n.to_string(), s);
        }
        ["quiet", which @ ("from" | "to"), d @ ("-" | "+")] => {
            let t = if *which == "from" { &mut cfg.quiet.from } else { &mut cfg.quiet.to };
            let m = parse_hm(t).unwrap_or(0) + if *d == "+" { 30 } else { -30 };
            *t = hm(m);
        }
        _ => return false,
    }
    true
}

pub fn config_path() -> PathBuf {
    yggdrasil_conf::dirs::config().join("health.json")
}
pub fn state_dir() -> PathBuf {
    yggdrasil_conf::dirs::state().join("health")
}

pub fn load_config() -> Config {
    std::fs::read_to_string(config_path()).ok().and_then(|t| serde_json::from_str::<Config>(&t).ok()).unwrap_or_default().filled()
}
pub fn load_state() -> State {
    std::fs::read_to_string(state_dir().join("state.json")).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

/// Writes through a temporary file, so a reader never sees half a file.
pub fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

pub fn save_config(c: &Config) -> std::io::Result<()> {
    write_atomic(&config_path(), &serde_json::to_string_pretty(c).unwrap_or_default())
}
pub fn save_state(s: &State) -> std::io::Result<()> {
    write_atomic(&state_dir().join("state.json"), &serde_json::to_string(s).unwrap_or_default())
}
/// Rewrites the bar's file when what it says changed (the bar reads it again on every change).
pub fn write_bar(cfg: &Config, st: &State, clock: &Clock) {
    let path = state_dir().join("bar.json");
    let text = bar(cfg, st, clock).to_string();
    if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
        let _ = write_atomic(&path, &text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 2026-10-25 at 10:00 local, in a zone where that is also the epoch's time of day
    fn at(hm: &str) -> Clock {
        let m = parse_hm(hm).unwrap();
        let midnight = 1_793_404_800; // some midnight
        Clock { epoch: midnight + m * 60, day: "2026-10-25".into(), sod: m * 60 }
    }

    fn only(name: &str, s: Stream) -> Config {
        let mut c = Config::default();
        for v in c.streams.values_mut() {
            v.enabled = false;
        }
        c.streams.insert(name.into(), s);
        c
    }

    #[test]
    fn times_are_read_strictly() {
        assert_eq!(parse_hm("14:00"), Some(840));
        assert_eq!(parse_hm("7:05"), Some(425));
        assert_eq!(parse_hm("24:00"), None);
        assert_eq!(parse_hm("12:60"), None);
        assert_eq!(parse_hm("noon"), None);
    }

    #[test]
    fn quiet_hours_cross_midnight() {
        let q = Quiet { enabled: true, from: "22:00".into(), to: "08:00".into() };
        assert!(in_quiet(&q, 23 * 60));
        assert!(in_quiet(&q, 2 * 60));
        assert!(!in_quiet(&q, 8 * 60));
        assert!(!in_quiet(&q, 12 * 60));
        let day = Quiet { enabled: true, from: "12:00".into(), to: "13:00".into() };
        assert!(in_quiet(&day, 12 * 60 + 30) && !in_quiet(&day, 13 * 60));
        assert!(!in_quiet(&Quiet { enabled: false, ..q }, 23 * 60));
    }

    #[test]
    fn an_interval_starts_counting_then_fires_and_counts_again() {
        let cfg = only("eye", Stream { enabled: true, every: Some(20), at: None });
        let mut st = State::default();
        let mut here = || false;
        let t0 = at("10:00");
        assert!(tick(&cfg, &mut st, &t0, &mut here).is_empty(), "the first tick only starts the count");
        assert_eq!(next_due(&cfg, &st, &t0), Some(("eye".into(), t0.epoch + 1200)));
        assert!(tick(&cfg, &mut st, &at("10:19"), &mut here).is_empty());
        assert_eq!(tick(&cfg, &mut st, &at("10:20"), &mut here), ["eye"]);
        assert_eq!(next_due(&cfg, &st, &at("10:20")).unwrap().1, at("10:40").epoch);
    }

    #[test]
    fn away_or_paused_resets_intervals_without_a_burst() {
        let cfg = only("water", Stream { enabled: true, every: Some(90), at: None });
        let mut st = State::default();
        st.last.insert("water".into(), at("08:00").epoch);
        let mut away = || true;
        assert!(tick(&cfg, &mut st, &at("12:00"), &mut away).is_empty());
        assert_eq!(st.last["water"], at("12:00").epoch, "counts again from the return");

        let mut paused = cfg.clone();
        paused.paused_until = Some(at("14:00").epoch);
        let mut here = || false;
        assert!(tick(&paused, &mut st, &at("13:30"), &mut here).is_empty());
        assert_eq!(next_due(&paused, &st, &at("13:31")), None, "nothing is due while paused");
        assert_eq!(bar(&paused, &st, &at("13:31"))["text"], "Paused {countdown}");
        assert_eq!(bar(&paused, &st, &at("13:31"))["until"], at("14:00").epoch);
        // after the pause the count runs normally
        assert!(next_due(&paused, &st, &at("14:00")).is_some());
    }

    #[test]
    fn a_clock_reminder_fires_once_a_day_and_waits_for_you() {
        let cfg = only("meal", Stream { enabled: true, every: None, at: Some("14:00".into()) });
        let mut st = State::default();
        let mut away = || true;
        let mut here = || false;
        assert!(tick(&cfg, &mut st, &at("13:59"), &mut here).is_empty());
        assert!(tick(&cfg, &mut st, &at("14:00"), &mut away).is_empty(), "away: held");
        assert_eq!(tick(&cfg, &mut st, &at("14:30"), &mut here), ["meal"], "back the same day: sent once");
        assert!(tick(&cfg, &mut st, &at("15:00"), &mut here).is_empty());
        assert_eq!(next_due(&cfg, &st, &at("15:00")).unwrap().1, at("14:00").epoch + 86_400);
        // a computer switched on long after the time does not send it
        let mut st2 = State::default();
        assert!(tick(&cfg, &mut st2, &at("18:30"), &mut here).is_empty());
    }

    #[test]
    fn quiet_hours_hold_everything_and_push_the_next_one_past_them() {
        let mut cfg = only("eye", Stream { enabled: true, every: Some(20), at: None });
        cfg.quiet = Quiet { enabled: true, from: "22:00".into(), to: "08:00".into() };
        let mut st = State::default();
        st.last.insert("eye".into(), at("21:50").epoch);
        let mut here = || false;
        assert!(tick(&cfg, &mut st, &at("22:10"), &mut here).is_empty());
        let (_, due) = next_due(&cfg, &st, &at("22:10")).unwrap();
        let m = at("22:10").minute_at(due);
        assert!((8 * 60..8 * 60 + 20).contains(&m), "first one after quiet hours, got minute {m}");
        // a clock reminder inside quiet hours never shows as next
        let mut wd = only("winddown", Stream { enabled: true, every: None, at: Some("23:30".into()) });
        wd.quiet = cfg.quiet.clone();
        assert_eq!(next_due(&wd, &State::default(), &at("12:00")), None);
        assert!(tick(&wd, &mut st, &at("23:31"), &mut here).is_empty());
    }

    #[test]
    fn the_next_one_is_the_earliest_and_the_bar_counts_to_it() {
        let cfg = Config::default();
        let mut st = State::default();
        let t = at("10:00");
        tick(&cfg, &mut st, &t, &mut || false);
        let (name, due) = next_due(&cfg, &st, &t).unwrap();
        assert_eq!((name.as_str(), due), ("eye", t.epoch + 1200));
        let b = bar(&cfg, &st, &t);
        assert_eq!((b["text"].as_str(), b["until"].as_i64()), (Some("{countdown}"), Some(due)));
        let off = Config { reminders: false, ..cfg };
        assert_eq!(bar(&off, &st, &t)["text"], "Off");
    }

    #[test]
    fn the_card_lists_every_reminder_and_its_controls_change_the_settings() {
        let mut cfg = Config::default().filled();
        let st = State::default();
        let t = at("10:00");
        let card = bar(&cfg, &st, &t)["card"].clone();
        let blocks = card.as_array().unwrap();
        assert_eq!(blocks[0]["big"], "{countdown}");
        assert!(blocks[0]["caption"].as_str().unwrap().starts_with("Eyes"));
        assert_eq!(blocks[0]["progress"][1].as_i64(), Some(t.epoch + 1200));
        let rows = blocks.iter().find_map(|b| b["list"].as_array()).unwrap();
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0]["id"], "toggle:eye");
        assert_eq!(rows[0]["note"], "every 20 min");
        assert_eq!(rows[1]["on"], false, "eye exercise is off by default");
        // every id the card offers is one press understands
        let mut ids = vec![];
        for b in blocks {
            for k in ["buttons", "list"] {
                for i in b[k].as_array().into_iter().flatten() {
                    ids.push(i["id"].as_str().unwrap().to_string());
                }
            }
            if let Some(st) = b.get("stepper") {
                ids.push(st["minus"].as_str().unwrap().to_string());
                ids.push(st["plus"].as_str().unwrap().to_string());
            }
        }
        for id in &ids {
            let mut c = cfg.clone();
            assert!(press(&mut c, id, &t), "{id}");
        }

        assert!(press(&mut cfg, "toggle:eye", &t) && !cfg.stream("eye").enabled);
        assert!(press(&mut cfg, "int:move:+", &t));
        assert_eq!(cfg.stream("move").every, Some(50));
        assert!(press(&mut cfg, "int:water:-", &t));
        assert_eq!(cfg.stream("water").every, Some(75));
        assert!(press(&mut cfg, "int:move:-", &t) && press(&mut cfg, "int:move:-", &t));
        assert_eq!(cfg.stream("move").every, Some(40));
        assert!(press(&mut cfg, "at:winddown:+", &t) && press(&mut cfg, "at:winddown:+", &t));
        assert_eq!(cfg.stream("winddown").at.as_deref(), Some("00:00"), "wraps past midnight");
        assert!(press(&mut cfg, "quiet", &t) && cfg.quiet.enabled);
        assert!(press(&mut cfg, "quiet:from:-", &t));
        assert_eq!(cfg.quiet.from, "21:30");
        assert!(press(&mut cfg, "pause:15", &t));
        assert_eq!(cfg.paused_until, Some(t.epoch + 900));
        let paused = bar(&cfg, &st, &t);
        assert_eq!(paused["card"][0]["caption"], "Paused");
        assert!(paused["card"].as_array().unwrap().iter().any(|b| b["buttons"][0]["id"] == "resume"));
        assert!(press(&mut cfg, "resume", &t) && cfg.paused_until.is_none());
        assert!(press(&mut cfg, "sound", &t) && !cfg.sound);
        assert!(press(&mut cfg, "reminders", &t) && !cfg.reminders);
        assert_eq!(bar(&cfg, &st, &t)["card"][0]["big"], "Off");
        // what the card never offers is refused
        for bad in ["", "toggle:nope", "int:meal:+", "at:eye:+", "pause:-5", "quiet:at:+", "rm -rf"] {
            assert!(!press(&mut cfg, bad, &t), "{bad}");
        }
    }

    #[test]
    fn spans_read_naturally() {
        assert_eq!(span(20), "20 min");
        assert_eq!(span(180), "3 h");
        assert_eq!(span(90), "1 h 30 min");
        assert_eq!(cadence(&Stream::default_for("meal")), "at 14:00");
    }

    #[test]
    fn an_old_settings_file_gets_the_missing_reminders() {
        let c: Config = serde_json::from_str(r#"{"reminders":true,"streams":{"eye":{"enabled":false,"every":25}}}"#).unwrap();
        let c = c.filled();
        assert_eq!(c.stream("eye"), Stream { enabled: false, every: Some(25), at: None });
        assert_eq!(c.stream("meal").at.as_deref(), Some("14:00"));
        assert!(c.sound && !c.quiet.enabled);
    }
}
