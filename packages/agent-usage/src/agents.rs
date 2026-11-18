//! `yggdrasil-agent-usage --agents`: the Agents bar item (agents.conf). The pill is the number of agents running now and a ring
//! for the busiest limit any of them is near; its card lists every agent with its state (working, idle, stopped), its usage and
//! reset time where known, and when it last did something. Read-only: nothing here starts, stops or signals an agent (launching
//! one stays `sarina fleet-launch` in a terminal, which asks first).
//!
//! Where it looks, all on this computer and all cheap:
//!   - Claude Code's sessions: `~/.claude/sessions/PID.json`, Claude Code's own note of each session it runs (its status
//!     "busy" or "idle"). Only those `.json` files are read (never the `.key` files beside them), each up to 64 KiB, and a
//!     session counts as running only while its process is the same one (`/proc/PID/stat`'s start time = `procStart`). Its
//!     last activity is also the time of its transcript (`~/.claude/projects/FOLDER/ID.jsonl`, and the folder of its helpers):
//!     only their times are looked at, never what is in them.
//!   - Claude's limits: the same reading as the Agent usage pill (its four-minute cache, `crate::collect`).
//!   - Sarina and the other agents' usage records: `sarina fleet-status --json` (Sarina's docs/ECOSYSTEM.md, section 1). Its
//!     text is someone else's and is treated as untrusted: control, bidi and zero-width characters removed, cut short.
//!
//! Without Sarina (or a Sarina whose fleet-status has no --json) the card has the Claude Code sessions and Claude's limits;
//! with nothing at all the pill is off.

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum State {
    Working,
    Idle,
    Stopped,
}

impl State {
    fn word(self) -> &'static str {
        match self {
            State::Working => "Working",
            State::Idle => "Idle",
            State::Stopped => "Stopped",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub name: String,
    pub state: State,
    /// When it last did something (seconds since 1970).
    pub last: Option<i64>,
    /// Its busiest limit (0-100) and what to say about it ("Session (5-hour) 55%, resets 2:14 PM").
    pub percent: Option<f64>,
    pub usage: String,
}

/// Text from another program, made safe to draw: no control, bidi or zero-width characters, at most `n` characters.
pub fn clean(s: &str, n: usize) -> String {
    let bad =
        |c: char| c.is_control() || matches!(c, '\u{200b}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\u{feff}');
    s.chars().filter(|c| !bad(*c)).collect::<String>().trim().chars().take(n).collect()
}

/// "just now", "3m ago", "2h ago", "4d ago".
pub fn ago(t: i64, now: i64) -> String {
    match (now - t).max(0) {
        d if d < 60 => "just now".into(),
        d if d < 3600 => format!("{}m ago", d / 60),
        d if d < 2 * 86400 => format!("{}h ago", d / 3600),
        d => format!("{}d ago", d / 86400),
    }
}

/// A process's start time (clock ticks since boot, `/proc/PID/stat` field 22), or None when there is no such process.
fn start_time(proc_root: &Path, pid: u32) -> Option<String> {
    let text = std::fs::read_to_string(proc_root.join(pid.to_string()).join("stat")).ok()?;
    // the name in parentheses may hold spaces: the fields after the last ')' start at field 3
    let rest = &text[text.rfind(')')? + 1..];
    rest.split_whitespace().nth(19).map(str::to_string)
}

/// A small regular file of a folder (not a link, a pipe or anything big), as text.
fn small_file(p: &Path, max: u64) -> Option<String> {
    let m = std::fs::symlink_metadata(p).ok()?;
    if !m.file_type().is_file() || m.len() > max {
        return None;
    }
    let mut s = String::new();
    std::fs::File::open(p).ok()?.take(max).read_to_string(&mut s).ok()?;
    Some(s)
}

/// When a session last wrote its transcript or started a helper: the times of `projects/FOLDER/ID.jsonl` and
/// `projects/FOLDER/ID/subagents` beside the sessions folder (FOLDER is the session's working folder with every character but
/// letters and digits as '-', as Claude Code names it). Only times; nothing is opened.
fn transcript_time(sessions: &Path, v: &Value) -> Option<i64> {
    let id = v["sessionId"].as_str().filter(|s| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))?;
    let cwd = v["cwd"].as_str().filter(|c| c.starts_with('/') && c.len() <= 4096)?;
    let folder: String = cwd.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect();
    let base = sessions.parent()?.join("projects").join(folder);
    let secs = |p: std::path::PathBuf| {
        std::fs::metadata(p).ok()?.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_secs() as i64)
    };
    [secs(base.join(format!("{id}.jsonl"))), secs(base.join(id).join("subagents"))].into_iter().flatten().max()
}

/// Claude Code's sessions from its folder of notes: running ones, and ones that stopped within the last day.
pub fn claude_sessions(dir: &Path, proc_root: &Path, now: i64) -> Vec<Agent> {
    let Ok(rd) = std::fs::read_dir(dir) else { return vec![] };
    let mut out = vec![];
    for e in rd.flatten().take(256) {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        // PID.json only (the .key files beside them are never opened)
        let Some(stem) = name.strip_suffix(".json") else { continue };
        let Ok(file_pid) = stem.parse::<u32>() else { continue };
        let Some(v) = small_file(&p, 64 << 10).and_then(|t| serde_json::from_str::<Value>(&t).ok()) else { continue };
        let pid = v["pid"].as_u64().and_then(|x| u32::try_from(x).ok()).unwrap_or(file_pid);
        let ms = |k: &str| v[k].as_i64().map(|x| if x > 100_000_000_000 { x / 1000 } else { x });
        let wrote = transcript_time(dir, &v);
        let last = [ms("updatedAt"), ms("statusUpdatedAt"), ms("startedAt"), wrote].into_iter().flatten().max().filter(|t| *t <= now + 60);
        let alive = match (start_time(proc_root, pid), v["procStart"].as_str()) {
            (Some(have), Some(want)) => have == want,
            // no start time noted: its process is there (and no older note says otherwise)
            (Some(_), None) => true,
            (None, _) => false,
        };
        let state = match (alive, v["status"].as_str().unwrap_or("")) {
            (false, _) => State::Stopped,
            (true, "busy" | "working" | "running" | "active") => State::Working,
            (true, _) => State::Idle,
        };
        if state == State::Stopped && last.is_none_or(|t| now - t > 86400) {
            continue;
        }
        let session = clean(v["name"].as_str().unwrap_or(""), 28);
        let name = if session.is_empty() { "Claude Code".to_string() } else { format!("Claude Code · {session}") };
        out.push(Agent { name, state, last, percent: None, usage: String::new() });
    }
    out
}

/// Claude's busiest window from the Agent usage record (its card lines): percent, and "Session (5-hour) 55%, resets 2:14 PM".
pub fn claude_busiest(usage: &Value) -> Option<(f64, String)> {
    let lines = usage["lines"].as_array()?;
    let top = lines.iter().filter(|l| l["percent"].is_number()).max_by(|a, b| {
        a["percent"].as_f64().unwrap_or(0.0).partial_cmp(&b["percent"].as_f64().unwrap_or(0.0)).unwrap_or(std::cmp::Ordering::Equal)
    })?;
    let p = top["percent"].as_f64()?.clamp(0.0, 100.0);
    // the note's first part: "resets 2:14 PM (in 1h)", without " (in ...)" and the pace that follows
    let note = top["note"].as_str().unwrap_or("").split(" · ").next().unwrap_or("").split(" (in ").next().unwrap_or("").to_string();
    let title = top["title"].as_str().unwrap_or("Claude");
    let mut text = format!("{title} {p:.0}%");
    if !note.is_empty() {
        text += &format!(", {note}");
    }
    Some((p, text))
}

/// Sarina and the other agents from `sarina fleet-status --json`. `claude_known`: Claude's limits come from the Agent usage
/// reading, so a "claude" record would only say it again.
pub fn fleet(doc: &Value, now: i64, claude_known: bool) -> Vec<Agent> {
    let mut out = vec![];
    let me = &doc["self"];
    if me.is_object() {
        let last = crate::parse_time(&me["lastActive"]).filter(|t| *t <= now + 60);
        let running = me["running"].as_bool() == Some(true);
        let state = match last {
            Some(t) if now - t <= 120 => State::Working,
            _ if running => State::Idle,
            _ => State::Stopped,
        };
        if running || last.is_some() {
            let name = clean(me["name"].as_str().unwrap_or("Sarina"), 28);
            out.push(Agent {
                name: if name.is_empty() { "Sarina".into() } else { name },
                state,
                last,
                percent: None,
                usage: String::new(),
            });
        }
    }
    for a in doc["agents"].as_array().into_iter().flatten().take(32) {
        let id = a["id"].as_str().unwrap_or("");
        if id.is_empty() || id == "sarina" || (claude_known && id == "claude") {
            continue;
        }
        let name = match clean(a["name"].as_str().unwrap_or(""), 28) {
            n if n.is_empty() => clean(id, 28),
            n => n,
        };
        let stale = a["stale"].as_bool() != Some(false);
        let last = crate::parse_time(&a["updatedAt"]).filter(|t| *t <= now + 60);
        // the busiest window it reported (an unknown percent is null and counts for nothing)
        let busiest = a["limits"]
            .as_array()
            .into_iter()
            .flatten()
            .take(16)
            .filter_map(|l| Some((l["percentUsed"].as_f64().filter(|p| p.is_finite())?.clamp(0.0, 100.0), l)))
            .max_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
        let (percent, usage) = match busiest.filter(|_| !stale) {
            Some((p, l)) => {
                let mut text = format!("{} {p:.0}%", clean(l["label"].as_str().unwrap_or("limit"), 24));
                if let Some(r) = crate::parse_time(&l["resetsAt"]) {
                    text += &format!(", resets {}", crate::clock(r, now));
                }
                (Some(p), text)
            }
            None => (None, clean(a["statusText"].as_str().unwrap_or(""), 40)),
        };
        out.push(Agent { name, state: if stale { State::Stopped } else { State::Idle }, last, percent, usage });
    }
    out
}

/// The bar's record: the pill (agents running now, a ring for the busiest limit) and the card.
pub fn record(mut agents: Vec<Agent>, usage: &Value, fleet_note: &str, now: i64) -> Value {
    let claude = claude_busiest(usage);
    // every Claude Code session draws from the same account's limits
    for a in agents.iter_mut().filter(|a| a.name.starts_with("Claude Code")) {
        if let Some((p, text)) = &claude {
            a.percent = Some(*p);
            a.usage = text.clone();
        }
    }
    if let Some((p, text)) = &claude {
        if !agents.iter().any(|a| a.name.starts_with("Claude Code")) {
            agents.push(Agent { name: "Claude".into(), state: State::Stopped, last: None, percent: Some(*p), usage: text.clone() });
        }
    }
    if agents.is_empty() {
        return json!({"text": "", "state": "off", "help": "No agent is running or has a record here."});
    }
    agents.sort_by(|a, b| a.state.cmp(&b.state).then(b.last.cmp(&a.last)));
    let running = agents.iter().filter(|a| a.state != State::Stopped).count();
    let busiest = agents.iter().filter_map(|a| a.percent).fold(None, |m: Option<f64>, p| Some(m.map_or(p, |m| m.max(p))));
    let ahead = usage["state"].as_str() == Some("alert");
    let state = match busiest {
        Some(p) if p >= 90.0 => "alert",
        _ if ahead => "alert",
        Some(p) if p >= 75.0 => "warn",
        _ => "ok",
    };
    let shown = agents.len().min(12);
    let lines: Vec<Value> = agents
        .iter()
        .take(shown)
        .map(|a| {
            let mut note = match (a.name.as_str(), a.last) {
                ("Claude", None) => "no Claude Code session open".to_string(),
                (_, Some(t)) => format!("active {}", ago(t, now)),
                (_, None) => String::new(),
            };
            if !a.usage.is_empty() {
                note = if note.is_empty() { a.usage.clone() } else { format!("{note} · {}", a.usage) };
            }
            let mut l = json!({"title": a.name, "value": a.state.word(), "note": note, "alert": a.percent.is_some_and(|p| p >= 90.0)});
            if let Some(p) = a.percent {
                l["percent"] = json!(p);
            }
            l
        })
        .collect();
    let mut help = String::from("Read-only: nothing here starts or stops an agent.");
    if agents.len() > shown {
        help += &format!(" {} more not shown.", agents.len() - shown);
    }
    if !fleet_note.is_empty() {
        help += &format!(" {fleet_note}");
    }
    let mut rec = json!({
        "text": format!("Agents {running}"),
        "state": state,
        "lines": lines,
        "card": [{"buttons": [{"id": "howto", "label": "How to start one", "close": true}]}],
        "help": help,
        // while the card is open it is read again every half minute
        "refresh_at": now + 30,
    });
    if let Some(p) = busiest {
        rec["ring"] = json!(p);
    }
    rec
}

/// Runs `sarina fleet-status --json` (10 seconds, 1 MiB at most): its JSON, or why there is none.
fn fleet_status() -> Result<Value, String> {
    let program = std::env::var_os("YGGDRASIL_AGENTS_SARINA")
        .map(std::path::PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| ["/usr/bin/sarina", "/usr/local/bin/sarina"].iter().map(std::path::PathBuf::from).find(|p| p.is_file()))
        .ok_or_else(String::new)?;
    let mut child = Command::new(program)
        .args(["fleet-status", "--json"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| "Sarina could not be asked.".to_string())?;
    let mut out = child.stdout.take().ok_or_else(String::new)?;
    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = (&mut out).take(1 << 20).read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break Some(s),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let buf = reader.join().unwrap_or_default();
    match status {
        Some(s) if s.success() => serde_json::from_slice(&buf).map_err(|_| "Sarina's answer could not be read.".to_string()),
        Some(_) => Err("This Sarina's fleet-status has no --json yet (update Sarina for her row and the other agents').".into()),
        None => Err("Sarina did not answer in time.".into()),
    }
}

/// Everything, read now.
pub fn collect(now: i64) -> Value {
    let usage = crate::collect();
    let mut agents = claude_sessions(&crate::claude_dir().join("sessions"), Path::new("/proc"), now);
    let claude_known = claude_busiest(&usage).is_some();
    let note = match fleet_status() {
        Ok(doc) => {
            agents.extend(fleet(&doc, now, claude_known));
            String::new()
        }
        Err(why) => why,
    };
    record(agents, &usage, &note, now)
}

/// A press on the card (`act ID`): only "How to start one", which opens this package's README (fleet-launch is described
/// there; it runs in a terminal and asks first).
pub fn act(id: &str) {
    if id == "howto" {
        let _ = Command::new("xdg-open")
            .arg("/usr/share/doc/yggdrasil-agent-usage/README.md")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("agents-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn proc_entry(root: &Path, pid: u32, start: &str) {
        let d = root.join(pid.to_string());
        std::fs::create_dir_all(&d).unwrap();
        // a name with spaces and a ')' in it, as a program may choose
        let fields: Vec<String> = (3..=25).map(|i| if i == 22 { start.to_string() } else { i.to_string() }).collect();
        std::fs::write(d.join("stat"), format!("{pid} (odd ) name) {}\n", fields.join(" "))).unwrap();
    }

    #[test]
    fn untrusted_text_is_cleaned() {
        assert_eq!(clean("\u{1b}]0;x\u{7}Cla\u{202e}ude\u{200b}  ", 40), "]0;xClaude");
        assert_eq!(clean("abcdef", 3), "abc");
        assert_eq!(ago(100, 130), "just now");
        assert_eq!(ago(0, 3 * 60), "3m ago");
        assert_eq!(ago(0, 5 * 3600), "5h ago");
        assert_eq!(ago(0, 3 * 86400), "3d ago");
    }

    #[test]
    fn claude_code_sessions_are_read_and_checked_against_their_process() {
        let now = 1_800_000_000i64;
        let (dir, proc_root) = (scratch("claude").join("sessions"), scratch("proc"));
        std::fs::create_dir_all(&dir).unwrap();
        let note = |pid: u32, start: &str, status: &str, name: &str, updated: i64| {
            json!({"pid": pid, "procStart": start, "status": status, "name": name, "updatedAt": updated * 1000, "kind": "interactive"})
                .to_string()
        };
        std::fs::write(dir.join("10.json"), note(10, "500", "busy", "repo-a", now - 5)).unwrap();
        // a session whose note is old but whose transcript was written a minute ago
        let mut n = serde_json::from_str::<Value>(&note(11, "600", "idle", "repo\u{202e}-b", now - 600)).unwrap();
        n["sessionId"] = json!("0a1b-2c");
        n["cwd"] = json!("/home/alex/My_repo.git");
        let tr = dir.join("../projects/-home-alex-My-repo-git");
        std::fs::create_dir_all(&tr).unwrap();
        let f = std::fs::File::create(tr.join("0a1b-2c.jsonl")).unwrap();
        f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs((now - 60) as u64)).unwrap();
        std::fs::write(dir.join("11.json"), n.to_string()).unwrap();
        // its process ended and another took its number: stopped
        std::fs::write(dir.join("12.json"), note(12, "700", "busy", "repo-c", now - 3600)).unwrap();
        // stopped long ago: left out
        std::fs::write(dir.join("13.json"), note(13, "800", "idle", "old", now - 3 * 86400)).unwrap();
        // not session notes: a key, junk, a link, a big file
        std::fs::write(dir.join("10.abc.key"), "secret").unwrap();
        std::fs::write(dir.join("14.json"), "{nope").unwrap();
        std::os::unix::fs::symlink(dir.join("10.json"), dir.join("15.json")).unwrap();
        std::fs::write(dir.join("16.json"), " ".repeat(70 << 10)).unwrap();
        proc_entry(&proc_root, 10, "500");
        proc_entry(&proc_root, 11, "600");
        proc_entry(&proc_root, 12, "999");
        let mut got = claude_sessions(&dir, &proc_root, now);
        got.sort_by(|a, b| a.name.cmp(&b.name));
        let seen: Vec<(&str, State)> = got.iter().map(|a| (a.name.as_str(), a.state)).collect();
        assert_eq!(
            seen,
            [("Claude Code · repo-a", State::Working), ("Claude Code · repo-b", State::Idle), ("Claude Code · repo-c", State::Stopped)]
        );
        assert_eq!(got[0].last, Some(now - 5));
        assert_eq!(got[1].last, Some(now - 60), "the transcript's time");
        assert!(claude_sessions(&dir.join("missing"), &proc_root, now).is_empty());
        let _ = std::fs::remove_dir_all(dir.parent().unwrap());
        let _ = std::fs::remove_dir_all(&proc_root);
    }

    fn usage(now: i64) -> Value {
        json!({"text": "Claude 31% · 2d", "state": "ok", "lines": [
            {"title": "Weekly (7-day)", "value": "31%", "percent": 31.0, "note": "resets Tue 9:00 AM (in 2d 0h) · an even pace would be 70% by now"},
            {"title": "Session (5-hour)", "value": "55%", "percent": 55.0, "note": format!("resets {} (in 1h)", crate::clock(now + 3600, now))}]})
    }

    #[test]
    fn claudes_busiest_window_leads() {
        let now = 1_800_000_000;
        let (p, text) = claude_busiest(&usage(now)).unwrap();
        assert_eq!(p, 55.0);
        assert!(text.starts_with("Session (5-hour) 55%, resets ") && !text.contains("(in"), "{text}");
        assert!(claude_busiest(&json!({"text": "", "state": "off"})).is_none());
    }

    #[test]
    fn the_fleet_is_read_as_untrusted() {
        let now = 1_800_000_000;
        let at = |t: i64| {
            // RFC 3339 of t, as fleet-status writes it
            let (d, s) = (t.div_euclid(86400), t.rem_euclid(86400));
            let (y, m, day) = civil(d);
            format!("{y:04}-{m:02}-{day:02}T{:02}:{:02}:{:02}Z", s / 3600, s / 60 % 60, s % 60)
        };
        let doc = json!({
            "agents": [
                {"id": "claude", "name": "Claude", "stale": false, "updatedAt": at(now - 60), "limits": [{"label": "w", "percentUsed": 10.0}]},
                {"id": "codex", "name": "Co\u{1b}[2Jdex", "stale": false, "updatedAt": at(now - 300),
                 "limits": [{"label": "Weekly", "percentUsed": 92.0, "resetsAt": at(now + 7200)}, {"label": "x", "percentUsed": null}]},
                {"id": "gemini", "name": "", "stale": true, "updatedAt": at(now - 7200), "limits": [{"label": "w", "percentUsed": 99.0}], "statusText": "prepaid"},
                {"id": "sarina", "name": "Sarina", "stale": false},
                {"name": "no id"}
            ],
            "self": {"id": "sarina", "name": "Sarina", "running": true, "lastActive": at(now - 30)}
        });
        let got = fleet(&doc, now, true);
        let names: Vec<&str> = got.iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["Sarina", "Co[2Jdex", "gemini"], "Claude's record is left to the Agent usage reading");
        assert_eq!(got[0].state, State::Working);
        assert_eq!((got[1].state, got[1].percent), (State::Idle, Some(92.0)));
        assert!(got[1].usage.starts_with("Weekly 92%, resets "), "{}", got[1].usage);
        assert_eq!(
            (got[2].state, got[2].percent, got[2].usage.as_str()),
            (State::Stopped, None, "prepaid"),
            "a stale record's limits are not current"
        );
        assert_eq!(fleet(&doc, now, false).len(), 4, "without Claude's own reading its record is shown");
        let idle = fleet(&json!({"self": {"running": true, "lastActive": at(now - 3600)}}), now, true);
        assert_eq!(idle[0].state, State::Idle);
        let off = fleet(&json!({"self": {"running": false, "lastActive": ""}}), now, true);
        assert!(off.is_empty(), "a Sarina that never ran here has no row");
        assert!(fleet(&json!("junk"), now, true).is_empty());
    }

    fn civil(z: i64) -> (i64, i64, i64) {
        let z = z + 719468;
        let era = z.div_euclid(146097);
        let doe = z - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (yoe + era * 400 + if m <= 2 { 1 } else { 0 }, m, d)
    }

    #[test]
    fn the_pill_counts_running_agents_and_rings_the_busiest_limit() {
        let now = 1_800_000_000;
        let agents = vec![
            Agent { name: "Claude Code · a".into(), state: State::Idle, last: Some(now - 600), percent: None, usage: String::new() },
            Agent { name: "Claude Code · b".into(), state: State::Working, last: Some(now - 5), percent: None, usage: String::new() },
            Agent { name: "Codex".into(), state: State::Stopped, last: Some(now - 7200), percent: None, usage: "prepaid".into() },
        ];
        let rec = record(agents, &usage(now), "", now);
        assert_eq!(rec["text"], "Agents 2");
        assert_eq!(rec["ring"], 55.0);
        assert_eq!(rec["state"], "ok");
        let lines = rec["lines"].as_array().unwrap();
        assert_eq!(lines[0]["title"], "Claude Code · b", "working first");
        assert_eq!(lines[0]["value"], "Working");
        assert_eq!(lines[0]["percent"], 55.0);
        assert!(lines[0]["note"].as_str().unwrap().starts_with("active just now · Session (5-hour) 55%"));
        assert_eq!(lines[2]["note"], "active 2h ago · prepaid");
        assert!(lines[2].get("percent").is_none());
        assert_eq!(rec["card"][0]["buttons"][0]["id"], "howto", "the card's one control opens the docs; nothing launches");
        assert_eq!(rec["refresh_at"], now + 30);

        let busy = vec![Agent { name: "Codex".into(), state: State::Idle, last: None, percent: Some(93.0), usage: String::new() }];
        let off = json!({"text": "", "state": "off"});
        let rec = record(busy, &off, "note", now);
        assert_eq!((rec["state"].as_str(), rec["ring"].as_f64()), (Some("alert"), Some(93.0)));
        assert!(rec["help"].as_str().unwrap().ends_with(" note"));
        let many: Vec<Agent> = (0..15)
            .map(|i| Agent { name: format!("A{i}"), state: State::Idle, last: Some(now - i), percent: None, usage: String::new() })
            .collect();
        let rec = record(many, &off, "", now);
        assert_eq!(rec["lines"].as_array().unwrap().len(), 12);
        assert!(rec["help"].as_str().unwrap().contains("3 more not shown."));
        assert!(rec.get("ring").is_none(), "no limit known: no ring");
    }

    #[test]
    fn with_only_claudes_limits_it_still_shows_and_with_nothing_it_hides() {
        let now = 1_800_000_000;
        let rec = record(vec![], &usage(now), "", now);
        assert_eq!(rec["text"], "Agents 0");
        assert_eq!(rec["lines"][0]["title"], "Claude");
        assert_eq!(rec["lines"][0]["note"].as_str().unwrap().split(" · ").next(), Some("no Claude Code session open"));
        let none = record(vec![], &json!({"state": "off", "text": ""}), "", now);
        assert_eq!(none["state"], "off");
    }
}
