//! yggdrasil-agent-usage: a bar add-on (Yggdrasil's docs/INTEGRATIONS.md, kind bar-item) showing how much of Claude's limits is
//! used: the weekly (7-day) window in the bar, red when it is ahead of an even pace through the week; the 5-hour window and any
//! model-scoped windows in its card. Adapted from Omarchy's agent-usage plugin (robzolkos/omarchy-agent-usage, MIT) and Omarchy's
//! collector.
//!
//! Where the numbers come from: Anthropic's usage endpoint (api.anthropic.com/api/oauth/usage), asked with Claude Code's own
//! sign-in (~/.claude/.credentials.json). The token goes only to that address, in a header handed to curl on its standard input
//! (never on a command line other programs can read), and is never stored or refreshed here: Claude Code renews it itself.
//! Answers are cached (~/.cache/yggdrasil/agent-usage/claude.json) and the endpoint is asked at most every four minutes.
//!
//!   yggdrasil-agent-usage --bar     the JSON the bar reads
//!   yggdrasil-agent-usage           the same, readable

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

const ENDPOINT: &str = "https://api.anthropic.com/api/oauth/usage";
const REUSE_SECS: i64 = 240;
const WEEK: f64 = 7.0 * 86400.0;

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn claude_dir() -> PathBuf {
    std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from).unwrap_or_else(|| home().join(".claude"))
}

fn cache_path() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".cache"));
    base.join("yggdrasil/agent-usage/claude.json")
}

/// Days since 1970-01-01 of a civil date.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// A reset time: RFC 3339 ("2026-10-27T14:00:00.123+00:00", "...Z") or Unix seconds or milliseconds. Seconds since 1970.
fn parse_time(v: &Value) -> Option<i64> {
    if let Some(n) = v.as_i64().or_else(|| v.as_str().and_then(|s| s.parse().ok())) {
        return Some(if n > 1_000_000_000_000 { n / 1000 } else { n });
    }
    let s = v.as_str()?;
    if s.len() < 19 {
        return None;
    }
    let num = |a: usize, b: usize| s.get(a..b)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (num(0, 4)?, num(5, 7)?, num(8, 10)?, num(11, 13)?, num(14, 16)?, num(17, 19)?);
    let mut t = days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + se;
    // the offset after the seconds and any fraction: Z, or +hh:mm / -hh:mm
    let rest = s[19..].trim_start_matches(|c: char| c == '.' || c.is_ascii_digit());
    if let Some(sign) = rest.chars().next().filter(|c| *c == '+' || *c == '-') {
        let oh: i64 = rest.get(1..3)?.parse().ok()?;
        let om: i64 = rest.get(4..6).and_then(|x| x.parse().ok()).unwrap_or(0);
        let off = oh * 3600 + om * 60;
        t -= if sign == '+' { off } else { -off };
    }
    Some(t)
}

/// "45m", "16h", "3d 4h": how long until a time.
fn until(t: i64, now: i64) -> String {
    let d = (t - now).max(0);
    match d {
        d if d < 3600 => format!("{}m", (d / 60).max(1)),
        d if d < 2 * 86400 => format!("{}h", (d + 1800) / 3600),
        d => format!("{}d {}h", d / 86400, (d % 86400) / 3600),
    }
}

/// One limit window from the endpoint: utilization 0..100 and the reset time.
struct Window {
    title: String,
    percent: f64,
    resets: Option<i64>,
}

fn window(title: &str, b: &Value) -> Option<Window> {
    let p = b["utilization"].as_f64().or_else(|| b["percent"].as_f64())?;
    Some(Window { title: title.into(), percent: p.clamp(0.0, 100.0), resets: parse_time(&b["resets_at"]) })
}

fn windows(payload: &Value) -> Vec<Window> {
    let mut out = vec![];
    if let Some(w) = window("Weekly (7-day)", &payload["seven_day"]) {
        out.push(w);
    }
    if let Some(w) = window("Session (5-hour)", &payload["five_hour"]) {
        out.push(w);
    }
    // windows that only one model draws from (the "limits" array), "Opus Weekly" and the like
    for e in payload["limits"].as_array().into_iter().flatten() {
        let model = &e["scope"]["model"];
        let name = model["display_name"].as_str().or_else(|| model["id"].as_str()).unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        let kind = e["kind"].as_str().unwrap_or("").to_lowercase();
        let w = if kind.contains("week") || kind.contains("day") { " Weekly" } else if kind.contains("hour") || kind.contains("session") { " Session" } else { "" };
        if let Some(win) = window(&format!("{name}{w}"), e) {
            out.push(win);
        }
    }
    out
}

/// How far through the week the reset implies we are, as the share (0..100) an even pace would have used.
fn even_pace(resets: i64, now: i64) -> f64 {
    let left = ((resets - now) as f64).clamp(0.0, WEEK);
    (1.0 - left / WEEK) * 100.0
}

/// The bar's record from the endpoint's answer (and a note when it is not fresh).
fn record(payload: &Value, now: i64, note: &str) -> Value {
    let ws = windows(payload);
    let Some(week) = ws.iter().find(|w| w.title.starts_with("Weekly")) else {
        return json!({"text": "Claude ?", "state": "warn", "lines": [], "help": if note.is_empty() { "Anthropic's answer had no weekly limit." } else { note }});
    };
    let pace = week.resets.map(|r| even_pace(r, now));
    let ahead = pace.is_some_and(|p| week.percent >= 10.0 && week.percent > p + 2.0);
    let mut text = format!("Claude {:.0}%", week.percent);
    if let Some(r) = week.resets {
        text += &format!(" · {}", until(r, now));
    }
    let lines: Vec<Value> = ws
        .iter()
        .map(|w| {
            let mut n = w.resets.map(|r| format!("resets in {}", until(r, now))).unwrap_or_default();
            let mut alert = false;
            if w.title.starts_with("Weekly") {
                if let Some(p) = pace {
                    n += &format!(" · an even pace would be {p:.0}% by now");
                    alert = ahead;
                }
            }
            json!({"title": w.title, "value": format!("{:.0}%", w.percent), "percent": w.percent, "note": n, "alert": alert || w.percent >= 90.0})
        })
        .collect();
    let mut help = String::from("From Claude Code's own sign-in, asked of Anthropic only (as Claude Code does), at most every 4 minutes.");
    if !note.is_empty() {
        help = format!("{note} {help}");
    }
    json!({"text": text, "state": if ahead || week.percent >= 90.0 { "alert" } else { "ok" }, "lines": lines, "help": help})
}

fn read_json(p: &PathBuf) -> Value {
    std::fs::read_to_string(p).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or(Value::Null)
}

/// Asks the endpoint: (HTTP status, body). The token reaches curl on its standard input, as a header file.
fn ask(token: &str) -> Option<(u16, String)> {
    let mut child = Command::new("curl")
        .args(["-sS", "--max-time", "10", "--proto", "=https", "-H", "@-", "-w", "\n%{http_code}", ENDPOINT])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    {
        let mut stdin = child.stdin.take()?;
        write!(stdin, "Authorization: Bearer {token}\nanthropic-beta: oauth-2025-04-20\nAccept: application/json\n").ok()?;
    }
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).to_string();
    let (body, code) = text.rsplit_once('\n')?;
    Some((code.trim().parse().ok()?, body.to_string()))
}

fn collect() -> Value {
    let creds = read_json(&claude_dir().join(".credentials.json"));
    let login = &creds["claudeAiOauth"];
    let token = login["accessToken"].as_str().unwrap_or("");
    if token.is_empty() {
        // no Claude Code sign-in on this account: nothing to show (the pill stays away)
        return json!({"text": "", "state": "off", "help": "Claude Code is not signed in on this account."});
    }
    let t = now();
    let cache_file = cache_path();
    let cache = read_json(&cache_file);
    let cached_at = cache["fetched_at"].as_i64().unwrap_or(0);
    if t - cached_at < REUSE_SECS && cache["payload"].is_object() {
        return record(&cache["payload"], t, "");
    }
    let expires = login["expiresAt"].as_i64().unwrap_or(i64::MAX) / 1000;
    let stale = |why: &str| {
        if cache["payload"].is_object() {
            record(&cache["payload"], t, &format!("{why} These numbers are from {} ago.", until(t, t + (t - cached_at))))
        } else {
            json!({"text": "Claude ?", "state": "warn", "lines": [], "help": why})
        }
    };
    if expires < t {
        return stale("Claude Code's sign-in has expired; Claude Code renews it the next time it runs.");
    }
    match ask(token) {
        Some((200, body)) => {
            let payload: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
            if !payload.is_object() {
                return stale("Anthropic's answer could not be read.");
            }
            let _ = std::fs::create_dir_all(cache_file.parent().unwrap());
            let _ = std::fs::write(&cache_file, json!({"fetched_at": t, "payload": payload}).to_string());
            record(&payload, t, "")
        }
        Some((429, _)) => stale("Anthropic is limiting how often usage is asked right now."),
        Some((401 | 403, _)) => stale("Anthropic refused Claude Code's sign-in; start Claude Code (or claude auth login) to sign in again."),
        Some((code, _)) => stale(&format!("Anthropic's usage address answered {code}.")),
        None => stale("Anthropic could not be reached."),
    }
}

fn main() {
    let rec = collect();
    if std::env::args().any(|a| a == "--bar") {
        println!("{rec}");
        return;
    }
    println!("{}", rec["text"].as_str().unwrap_or(""));
    for l in rec["lines"].as_array().into_iter().flatten() {
        println!("  {:<22} {:>5}   {}", l["title"].as_str().unwrap_or(""), l["value"].as_str().unwrap_or(""), l["note"].as_str().unwrap_or(""));
    }
    println!("{}", rec["help"].as_str().unwrap_or(""));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_are_read() {
        assert_eq!(parse_time(&json!("1970-01-02T00:00:00Z")), Some(86400));
        assert_eq!(parse_time(&json!("1970-01-02T01:00:00.5+01:00")), Some(86400));
        assert_eq!(parse_time(&json!("2026-10-27T14:00:00-06:00")), parse_time(&json!("2026-10-27T20:00:00Z")));
        assert_eq!(parse_time(&json!(1_800_000_000_000i64)), Some(1_800_000_000));
        assert_eq!(until(3600 * 16, 0), "16h");
        assert_eq!(until(86400 * 3 + 3600 * 4, 0), "3d 4h");
    }

    #[test]
    fn the_week_leads_and_turns_red_ahead_of_pace() {
        let now = 1_000_000;
        // 2 days left of 7: an even pace is about 71% by now
        let resets = now + 2 * 86400;
        let p = |used: f64| json!({"seven_day": {"utilization": used, "resets_at": resets}, "five_hour": {"utilization": 12.0, "resets_at": now + 3600},
                                   "limits": [{"kind": "weekly_scoped", "percent": 40.0, "resets_at": resets, "scope": {"model": {"display_name": "Opus"}}}]});
        let ok = record(&p(50.0), now, "");
        assert_eq!(ok["text"], "Claude 50% · 2d 0h");
        assert_eq!(ok["state"], "ok");
        assert_eq!(ok["lines"].as_array().unwrap().len(), 3);
        assert_eq!(ok["lines"][2]["title"], "Opus Weekly");
        let ahead = record(&p(85.0), now, "");
        assert_eq!(ahead["state"], "alert");
        assert_eq!(ahead["lines"][0]["alert"], true);
    }
}
