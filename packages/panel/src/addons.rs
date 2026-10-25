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

use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct Addon {
    pub id: String,
    pub name: String,
    pub command: Vec<String>,
    pub interval: u64,
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
}

/// The installed add-ons.
pub fn load() -> Vec<Addon> {
    yggdrasil_conf::integrations::of_kind("bar-item")
        .into_iter()
        .filter_map(|i| {
            let command = i.command("command")?;
            let name = i.get("name").unwrap_or("Add-on").to_string();
            let interval = i.get("interval").and_then(|v| v.parse().ok()).unwrap_or(300u64).max(60);
            Some(Addon { id: i.id(), name, command, interval })
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
    View { id: id.into(), name: name.into(), text, level, lines, help: clip(v["help"].as_str().unwrap_or(""), 240) }
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
        let mut listed = Instant::now() - Duration::from_secs(60);
        loop {
            if listed.elapsed() >= Duration::from_secs(30) {
                listed = Instant::now();
                let fresh = load();
                if fresh != addons {
                    addons = fresh;
                    due = addons.iter().map(|_| Instant::now()).collect();
                    if let Ok(mut v) = shared.lock() {
                        v.retain(|x| addons.iter().any(|a| a.id == x.id));
                    }
                }
            }
            let forced = now.swap(false, Ordering::Relaxed);
            for (i, a) in addons.iter().enumerate() {
                if !forced && Instant::now() < due[i] {
                    continue;
                }
                due[i] = Instant::now() + Duration::from_secs(a.interval);
                let view = run(&a.command).map(|o| parse(&a.id, &a.name, &o)).unwrap_or_else(|| View {
                    id: a.id.clone(),
                    name: a.name.clone(),
                    help: "The add-on did not answer.".into(),
                    ..Default::default()
                });
                if let Ok(mut v) = shared.lock() {
                    match v.iter_mut().find(|x| x.id == a.id) {
                        Some(x) => *x = view,
                        None => v.push(view),
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
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
    fn a_slow_or_noisy_command_is_cut_off() {
        assert_eq!(run(&["sh".into(), "-c".into(), "echo '{\"text\":\"ok\"}'".into()]).as_deref().map(str::trim), Some("{\"text\":\"ok\"}"));
        let big = run(&["sh".into(), "-c".into(), "head -c 200000 /dev/zero | tr '\\0' a".into()]).unwrap();
        assert_eq!(big.len(), 64 << 10);
    }
}
