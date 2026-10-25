//! Bar add-ons: a program the user installed (from Agora, say) that puts a pill in the top bar and a card with its details.
//!
//! An add-on describes itself in an integration file of kind `bar-item` (docs/INTEGRATIONS.md):
//!
//!   kind = bar-item
//!   name = Agent usage
//!   command = /usr/bin/yggdrasil-agent-usage --bar
//!   interval = 300
//!
//! The panel runs `command` every `interval` seconds (at least 60) on a thread of its own, with a 30-second limit and 64 KiB of output,
//! and when the card is opened. It prints one JSON object:
//!
//!   {"text": "Claude 31% · 16h", "state": "ok" | "warn" | "alert" | "off",
//!    "lines": [{"title": "Weekly", "value": "31%", "percent": 31, "note": "resets in 16 h", "alert": false}],
//!    "help": "a sentence under the details"}
//!
//! "off" (or no text) hides the pill. The panel shows what the add-on printed; it never runs anything else for it.
//!
//! Other ways to feed the pill, instead of `command`:
//!
//!   file = %h/.local/state/yggdrasil/timer/bar.json   # the same JSON in a file, read again within a second of each change
//!   text = 2FA                                        # a fixed label (leave it out for an icon alone)
//!
//! and, for any of them:
//!
//!   icon = timer-symbolic        # an icon before the text (an icon theme name, tinted like the bar's icons)
//!   open = /usr/bin/yggdrasil-timer --toggle   # a click runs this (a window of the add-on's own) instead of opening the card
//!
//! The JSON may also name an `"icon"` (replacing the conf's while it does), and carry `"until": UNIX-SECONDS`: "{countdown}" in its text is then drawn as the time left (m:ss, or h:mm:ss), redrawn
//! every second by the panel itself, so a countdown needs no program running every second.

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub enum Feed {
    Command { cmd: Vec<String>, interval: u64 },
    File(std::path::PathBuf),
    Text(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Addon {
    pub id: String,
    pub name: String,
    pub feed: Feed,
    pub open: Option<Vec<String>>,
    pub icon: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Level {
    #[default]
    Ok,
    Warn,
    Alert,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Line {
    pub title: String,
    pub value: String,
    pub percent: Option<f32>,
    pub note: String,
    pub alert: bool,
}

/// What one add-on last printed.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    pub id: String,
    pub name: String,
    pub text: String,
    pub level: Level,
    pub lines: Vec<Line>,
    pub help: String,
    /// For "{countdown}" in the text: the moment it counts down to (seconds since 1970).
    pub until: Option<i64>,
    /// A click runs this instead of opening the card.
    pub open: Option<Vec<String>>,
    pub icon: String,
    /// The add-on said "off": no pill, not even its icon.
    pub off: bool,
}

impl View {
    /// The pill's text at `now`: "{countdown}" becomes the time left until `until`.
    pub fn display(&self, now: i64) -> String {
        match self.until {
            Some(u) if self.text.contains("{countdown}") => {
                let d = (u - now).max(0);
                let t = if d >= 3600 { format!("{}:{:02}:{:02}", d / 3600, d / 60 % 60, d % 60) } else { format!("{}:{:02}", d / 60, d % 60) };
                self.text.replace("{countdown}", &t)
            }
            _ => self.text.clone(),
        }
    }
}

fn expand(p: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_default();
    std::path::PathBuf::from(p.replace("%h", &home))
}

/// The installed add-ons.
pub fn load() -> Vec<Addon> {
    yggdrasil_conf::integrations::of_kind("bar-item")
        .into_iter()
        .filter_map(|i| {
            let name = i.get("name").unwrap_or("Add-on").to_string();
            let feed = if let Some(cmd) = i.command("command") {
                Feed::Command { cmd, interval: i.get("interval").and_then(|v| v.parse().ok()).unwrap_or(300u64).max(60) }
            } else if let Some(f) = i.get("file") {
                Feed::File(expand(f))
            } else {
                // a fixed label, or just the icon (a button that opens the add-on's window)
                Feed::Text(clip(i.get("text").or(i.get("icon").map(|_| ""))?, 40))
            };
            Some(Addon { id: i.id(), name, feed, open: i.command("open"), icon: i.get("icon").unwrap_or("").to_string() })
        })
        .collect()
}

fn clip(s: &str, n: usize) -> String {
    s.chars().filter(|c| !c.is_control()).take(n).collect()
}

/// Reads what an add-on printed. Text is cut short and stripped of control characters: it is drawn as it is.
pub fn parse(id: &str, name: &str, out: &str) -> View {
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap_or(serde_json::Value::Null);
    let level = match v["state"].as_str() {
        Some("warn") => Level::Warn,
        Some("alert") => Level::Alert,
        _ => Level::Ok,
    };
    let text = if v["state"].as_str() == Some("off") { String::new() } else { clip(v["text"].as_str().unwrap_or(""), 40) };
    let lines = v["lines"]
        .as_array()
        .map(|a| {
            a.iter()
                .take(12)
                .map(|l| Line {
                    title: clip(l["title"].as_str().unwrap_or(""), 40),
                    value: clip(l["value"].as_str().unwrap_or(""), 24),
                    percent: l["percent"].as_f64().map(|p| p.clamp(0.0, 100.0) as f32),
                    note: clip(l["note"].as_str().unwrap_or(""), 80),
                    alert: l["alert"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    View { id: id.into(), name: name.into(), text, level, lines, help: clip(v["help"].as_str().unwrap_or(""), 240), until: v["until"].as_i64(), off: v["state"].as_str() == Some("off"), icon: icon_name(v["icon"].as_str().unwrap_or("")), ..Default::default() }
}

/// An icon theme name the add-on asked for (letters, digits, - and _ only: a name, never a path).
fn icon_name(s: &str) -> String {
    if s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') { s.to_string() } else { String::new() }
}

fn file_stamp(p: &std::path::Path) -> Option<(std::time::SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// What an add-on shows now, read from its feed.
fn read(a: &Addon) -> View {
    let mut view = match &a.feed {
        Feed::Command { cmd, .. } => run(cmd).map(|o| parse(&a.id, &a.name, &o)).unwrap_or_else(|| View { id: a.id.clone(), name: a.name.clone(), help: "The add-on did not answer.".into(), ..Default::default() }),
        Feed::File(p) => {
            let text = std::fs::File::open(p).ok().and_then(|f| {
                let mut s = String::new();
                f.take(64 << 10).read_to_string(&mut s).ok().map(|_| s)
            });
            // no file yet: just the icon, if there is one (the add-on writes the file when it has something to say)
            parse(&a.id, &a.name, text.as_deref().unwrap_or("{}"))
        }
        Feed::Text(t) => View { id: a.id.clone(), name: a.name.clone(), text: t.clone(), ..Default::default() },
    };
    view.open = a.open.clone();
    // the add-on's own JSON may change the icon (the weather's follows the sky); the conf's is the default
    if view.icon.is_empty() {
        view.icon = a.icon.clone();
    }
    view
}

/// Runs the command with a time and size limit; its output, or None.
fn run(cmd: &[String]) -> Option<String> {
    let mut child = Command::new(&cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let mut out = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(64 << 10).read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(200)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    String::from_utf8(reader.join().ok()?).ok()
}

/// Polls every add-on on one thread; `now` set to true asks for a refresh at once (the card was opened). The list of add-ons is read
/// again every 30 seconds, so one installed or removed (from Agora) appears or goes without signing out.
pub fn spawn(shared: Arc<Mutex<Vec<View>>>, now: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut addons: Vec<Addon> = vec![];
        let mut due: Vec<Instant> = vec![];
        let mut stamps: Vec<Option<(std::time::SystemTime, u64)>> = vec![];
        let mut listed = Instant::now() - Duration::from_secs(60);
        loop {
            if listed.elapsed() >= Duration::from_secs(30) {
                listed = Instant::now();
                let fresh = load();
                if fresh != addons {
                    addons = fresh;
                    due = addons.iter().map(|_| Instant::now()).collect();
                    stamps = addons.iter().map(|_| None).collect();
                    if let Ok(mut v) = shared.lock() {
                        v.retain(|x| addons.iter().any(|a| a.id == x.id));
                    }
                }
            }
            let forced = now.swap(false, Ordering::Relaxed);
            for (i, a) in addons.iter().enumerate() {
                match &a.feed {
                    Feed::Command { interval, .. } => {
                        if !forced && Instant::now() < due[i] {
                            continue;
                        }
                        due[i] = Instant::now() + Duration::from_secs(*interval);
                    }
                    // a file: read again when it changed (a stat each second)
                    Feed::File(p) => {
                        let st = file_stamp(p);
                        if st == stamps[i] && Instant::now() < due[i] {
                            continue;
                        }
                        stamps[i] = st;
                        due[i] = Instant::now() + Duration::from_secs(3600);
                    }
                    Feed::Text(_) => {
                        if Instant::now() < due[i] {
                            continue;
                        }
                        due[i] = Instant::now() + Duration::from_secs(3600);
                    }
                }
                let view = read(a);
                if let Ok(mut v) = shared.lock() {
                    match v.iter_mut().find(|x| x.id == a.id) {
                        Some(x) => *x = view,
                        None => v.push(view),
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_is_read_and_cut() {
        let v = parse("agent-usage", "Agent usage", r#"{"text":"Claude 31% · 16h","state":"alert","lines":[{"title":"Weekly","value":"31%","percent":131,"note":"resets in 16 h","alert":true}],"help":"From Claude Code's sign-in."}"#);
        assert_eq!(v.text, "Claude 31% · 16h");
        assert_eq!(v.level, Level::Alert);
        assert_eq!(v.lines[0].percent, Some(100.0));
        assert!(v.lines[0].alert);
        assert_eq!(parse("x", "X", r#"{"text":"hidden","state":"off"}"#).text, "");
        assert_eq!(parse("x", "X", "not json").text, "");
        assert_eq!(parse("x", "X", "{\"text\":\"a\\u001b[31mb\"}").text, "a[31mb", "control characters are dropped");
    }

    #[test]
    fn a_countdown_is_drawn_from_until() {
        let v = parse("timer", "Timer", r#"{"text":"Tea {countdown}","until":1000}"#);
        assert_eq!(v.display(1000 - 125), "Tea 2:05");
        assert_eq!(v.display(1000 - 3725), "Tea 1:02:05");
        assert_eq!(v.display(2000), "Tea 0:00");
        assert_eq!(parse("x", "X", r#"{"text":"plain"}"#).display(0), "plain");
    }

    #[test]
    fn a_file_feeds_the_pill_and_open_and_icon_come_along() {
        let dir = std::env::temp_dir().join(format!("addon-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("bar.json");
        let a = Addon { id: "timer".into(), name: "Timer".into(), feed: Feed::File(f.clone()), open: Some(vec!["true".into()]), icon: "timer-symbolic".into() };
        assert_eq!((read(&a).text.as_str(), read(&a).off), ("", false), "no file yet: no text, the icon still shows");
        std::fs::write(&f, r#"{"state":"off","text":"x"}"#).unwrap();
        assert!(read(&a).off);
        std::fs::write(&f, r#"{"text":"Tea {countdown}","until":99}"#).unwrap();
        let v = read(&a);
        assert_eq!((v.text.as_str(), v.until, v.icon.as_str(), v.open.is_some()), ("Tea {countdown}", Some(99), "timer-symbolic", true));
        std::fs::write(&f, r#"{"text":"12°","icon":"weather-showers-symbolic"}"#).unwrap();
        assert_eq!(read(&a).icon, "weather-showers-symbolic");
        std::fs::write(&f, r#"{"text":"x","icon":"../../etc/passwd"}"#).unwrap();
        assert_eq!(read(&a).icon, "timer-symbolic", "a path is not an icon name");
        let s = Addon { feed: Feed::Text("2FA".into()), ..a };
        assert_eq!(read(&s).text, "2FA");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_slow_or_noisy_command_is_cut_off() {
        assert_eq!(run(&["sh".into(), "-c".into(), "echo '{\"text\":\"ok\"}'".into()]).as_deref().map(str::trim), Some("{\"text\":\"ok\"}"));
        let big = run(&["sh".into(), "-c".into(), "head -c 200000 /dev/zero | tr '\\0' a".into()]).unwrap();
        assert_eq!(big.len(), 64 << 10);
    }
}
