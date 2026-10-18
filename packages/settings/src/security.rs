//! The Security page's data: what Heimdall says (its alerts over the local API), and the malware scan of a folder (`heimdalld scan-files`).
//! Everything here is read-only apart from the scan, which only reports.

use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub struct Alert {
    /// Seconds since 1970.
    pub time: i64,
    pub detector: String,
    /// "info", "warning" or "critical".
    pub severity: String,
    pub summary: String,
    /// For connection alerts: the program and the address.
    pub process: String,
    pub remote: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Overview {
    /// None when Heimdall does not answer.
    pub running: bool,
    pub alerts: Vec<Alert>,
    /// ClamAV's signature database date (seconds since 1970), when ClamAV is installed.
    pub signatures: Option<i64>,
    pub clamav: bool,
    /// Files the malware scan moved to quarantine.
    pub quarantine: Vec<Quarantined>,
    /// Move certain finds to quarantine (true) or only warn (false).
    pub auto_quarantine: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Quarantined {
    pub id: String,
    pub original: String,
    pub signature: String,
    pub when: String,
}

/// The JSON of `labyrinth-quarantine list --json`.
pub fn parse_quarantine(json: &str) -> Vec<Quarantined> {
    let v: Value = serde_json::from_str(json).unwrap_or(Value::Null);
    v.as_array().map(|a| a.iter().filter_map(|i| Some(Quarantined { id: i["id"].as_str()?.to_string(), original: i["original"].as_str()?.to_string(), signature: i["signature"].as_str().unwrap_or("").to_string(), when: i["when"].as_str().unwrap_or("").to_string() })).collect()).unwrap_or_default()
}

fn scan_conf_path() -> std::path::PathBuf {
    std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/labyrinth/scan.conf")
}

/// `action = notify` in scan.conf turns automatic quarantine off.
pub fn auto_quarantine_on(conf: &str) -> bool {
    !conf.lines().any(|l| { let l = l.trim(); !l.starts_with('#') && l.replace(' ', "") == "action=notify" })
}

pub fn set_auto_quarantine(on: bool) -> Result<(), String> {
    let p = scan_conf_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }
    std::fs::write(&p, if on { "action = quarantine\n" } else { "action = notify\n" }).map_err(|e| e.to_string())
}

pub fn quarantine_restore(id: &str) -> Result<(), String> {
    crate::sys::act("labyrinth-quarantine", &["restore", id])
}

pub fn quarantine_delete(id: &str) -> Result<(), String> {
    crate::sys::act("labyrinth-quarantine", &["delete", id])
}

/// Days since 1970-01-01 for a civil date (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// "2026-10-16T14:41:14.948Z" (UTC) to seconds since 1970.
pub fn parse_time(t: &str) -> Option<i64> {
    let (date, rest) = t.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let clock = rest.trim_end_matches('Z').split(['.', '+']).next()?;
    let mut c = clock.split(':');
    let (h, mi, s): (i64, i64, i64) = (c.next()?.parse().ok()?, c.next()?.parse().ok()?, c.next()?.parse().ok()?);
    Some(days_from_civil(y, m, day) * 86400 + h * 3600 + mi * 60 + s)
}

/// "now", "5 min ago", "3 h ago", "2 d ago".
pub fn ago(then: i64, now: i64) -> String {
    let d = (now - then).max(0);
    match d {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", d / 60),
        3600..=86399 => format!("{} h ago", d / 3600),
        _ => format!("{} d ago", d / 86400),
    }
}

/// The alerts out of `GET /v1/alerts` (a list, or an object with an "alerts" list), newest first.
pub fn parse_alerts(body: &str) -> Vec<Alert> {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let list = v.as_array().cloned().or_else(|| v.get("alerts").and_then(|a| a.as_array()).cloned()).unwrap_or_default();
    let mut out: Vec<Alert> = list
        .iter()
        .filter_map(|a| {
            let fields = a.get("event").and_then(|e| e.get("Fields").or_else(|| e.get("fields")));
            let field = |k: &str| fields.and_then(|f| f.get(k)).and_then(|x| x.as_str()).unwrap_or("").to_string();
            Some(Alert {
                time: parse_time(a.get("time")?.as_str()?)?,
                detector: a.get("detector_id")?.as_str()?.to_string(),
                severity: a.get("severity")?.as_str()?.to_string(),
                summary: a.get("summary").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                process: field("process"),
                remote: if field("remote_addr").is_empty() { String::new() } else { format!("{}:{}", field("remote_addr"), field("remote_port")) },
            })
        })
        .collect();
    out.sort_by(|a, b| b.time.cmp(&a.time));
    out
}

/// Reads Heimdall's local API and ClamAV's state.
pub fn overview() -> Overview {
    let run = |cmd: &str, args: &[&str]| crate::sys::run(cmd, args);
    let health = run("curl", &["-s", "-m", "3", "http://127.0.0.1:8930/healthz"]).unwrap_or_default();
    let running = health.contains("ok");
    let alerts = if running { run("curl", &["-s", "-m", "5", "http://127.0.0.1:8930/v1/alerts?n=300"]).map(|b| parse_alerts(&b)).unwrap_or_default() } else { vec![] };
    let mtime = |p: &str| std::fs::metadata(p).ok().and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64);
    let signatures = ["/var/lib/clamav/daily.cld", "/var/lib/clamav/daily.cvd"].iter().find_map(|p| mtime(p));
    let clamav = run("which", &["clamscan"]).map(|s| !s.trim().is_empty()).unwrap_or(false);
    let quarantine = run("labyrinth-quarantine", &["list", "--json"]).map(|b| parse_quarantine(&b)).unwrap_or_default();
    let auto_quarantine = auto_quarantine_on(&std::fs::read_to_string(scan_conf_path()).unwrap_or_default());
    Overview { running, alerts, signatures, clamav, quarantine, auto_quarantine }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScanReport {
    pub infected: Vec<String>,
    pub summary: String,
    pub error: String,
}

/// Parses the output of `heimdalld scan-files`: "INFECTED path: Signature" lines and the closing "scanned N files ..." line.
pub fn parse_scan(stdout: &str, stderr: &str, code: i32) -> ScanReport {
    let infected = stdout.lines().filter_map(|l| l.strip_prefix("INFECTED ")).map(|s| s.to_string()).collect();
    let summary = stdout.lines().rev().find(|l| l.starts_with("scanned ")).unwrap_or("").to_string();
    let error = if code == 2 { stderr.lines().next().unwrap_or("The scan could not run.").to_string() } else { String::new() };
    ScanReport { infected, summary, error }
}

/// Scans a folder with Heimdall's scanner (ClamAV). Runs as the user, so it can read the user's own files; changed files only after the first run.
pub fn scan_folder(folder: &str) -> ScanReport {
    let state = std::env::var("XDG_STATE_HOME").map(std::path::PathBuf::from).unwrap_or_else(|_| std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state")).join("labyrinth");
    let _ = std::fs::create_dir_all(&state);
    let out = std::process::Command::new("heimdalld").args(["scan-files", "--state", &state.join("scan-state.json").to_string_lossy(), folder]).output();
    match out {
        Ok(o) => parse_scan(&String::from_utf8_lossy(&o.stdout), &String::from_utf8_lossy(&o.stderr), o.status.code().unwrap_or(2)),
        Err(e) => ScanReport { error: format!("heimdalld could not start: {e}"), ..Default::default() },
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quarantine_list_and_setting() {
        let q = parse_quarantine(r#"[{"id":"abc123def456","original":"/home/u/Downloads/a.exe","signature":"Win.Trojan.X","when":"2026-10-16 10:00:00"}]"#);
        assert_eq!(q.len(), 1);
        assert_eq!((q[0].id.as_str(), q[0].signature.as_str()), ("abc123def456", "Win.Trojan.X"));
        assert!(parse_quarantine("nonsense").is_empty());
        assert!(auto_quarantine_on(""));
        assert!(auto_quarantine_on("action = quarantine\n"));
        assert!(!auto_quarantine_on("action = notify\n"));
        assert!(auto_quarantine_on("# action = notify\n"));
    }

    use super::*;

    #[test]
    fn times_are_read_and_shown_short() {
        assert_eq!(parse_time("1970-01-02T00:00:00Z"), Some(86400));
        assert_eq!(parse_time("2026-10-16T14:41:14.948852883Z"), Some(1792161674));
        assert_eq!(parse_time("garbage"), None);
        assert_eq!(ago(100, 130), "just now");
        assert_eq!(ago(0, 300), "5 min ago");
        assert_eq!(ago(0, 7200), "2 h ago");
        assert_eq!(ago(0, 3 * 86400), "3 d ago");
    }

    #[test]
    fn alerts_are_read_newest_first_with_connection_details() {
        let body = r#"[
          {"time":"2026-10-16T10:00:00Z","detector_id":"file-integrity","severity":"critical","summary":"watched file changed","event":{"Fields":{"path":"/usr/bin/x"}}},
          {"time":"2026-10-16T11:00:00Z","detector_id":"connection-watch","severity":"info","summary":"curl connected","event":{"Fields":{"process":"curl","remote_addr":"5.0.0.10","remote_port":"443"}}},
          {"nonsense": true}
        ]"#;
        let a = parse_alerts(body);
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].detector, "connection-watch");
        assert_eq!((a[0].process.as_str(), a[0].remote.as_str()), ("curl", "5.0.0.10:443"));
        assert_eq!(a[1].severity, "critical");
        assert!(parse_alerts("not json").is_empty());
        assert_eq!(parse_alerts(&format!("{{\"alerts\":{body}}}")).len(), 2);
    }

    #[test]
    fn scan_output_is_read() {
        let r = parse_scan("INFECTED /tmp/a: Eicar-Signature\nscanned 3 files in 1s: 1 infected, 0 could not be scanned\n", "", 1);
        assert_eq!(r.infected, vec!["/tmp/a: Eicar-Signature".to_string()]);
        assert!(r.summary.starts_with("scanned 3 files"));
        assert!(r.error.is_empty());
        let e = parse_scan("", "heimdalld scan-files: ClamAV is not installed\n", 2);
        assert!(e.error.contains("not installed"));
    }
}

/// One line of the password logs.
#[derive(Clone, Debug, PartialEq)]
pub struct LogEntry {
    /// Seconds since 1970 (0 when unknown).
    pub time: i64,
    pub title: String,
    pub detail: String,
    /// A failed attempt.
    pub failed: bool,
}

/// Parses `journalctl -o short-unix` lines for sudo into entries (newest first): what was run, as whom, and failed attempts.
pub fn parse_sudo(journal: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    for line in journal.lines() {
        let Some((ts, rest)) = line.split_once(' ') else { continue };
        let time = ts.split('.').next().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
        let Some((head, msg)) = rest.split_once("]: ") else { continue };
        if !head.contains(" sudo[") {
            continue;
        }
        let msg = msg.trim();
        if let Some((who, tail)) = msg.split_once(" : ") {
            if let Some(cmd) = tail.split("COMMAND=").nth(1) {
                let user = tail.split("USER=").nth(1).and_then(|s| s.split(" ;").next()).unwrap_or("root");
                let bad = tail.contains("incorrect password");
                let title = if bad { format!("Wrong password: {}", cmd.trim()) } else { cmd.trim().to_string() };
                out.push(LogEntry { time, title, detail: format!("{} as {}", who.trim(), user.trim()), failed: bad || tail.contains("NOT in sudoers") });
                continue;
            }
            if tail.contains("NOT in sudoers") {
                out.push(LogEntry { time, title: "Not allowed to use sudo".into(), detail: who.trim().to_string(), failed: true });
            }
        } else if msg.contains("authentication failure") {
            let user = msg.rsplit("user=").next().unwrap_or("").trim();
            out.push(LogEntry { time, title: "Wrong password".into(), detail: format!("sudo, user {user}"), failed: true });
        }
    }
    out.reverse();
    out
}

/// Sudo history from the journal of the last 30 days.
pub fn sudo_history() -> Vec<LogEntry> {
    crate::sys::run("journalctl", &["--no-pager", "-o", "short-unix", "--since", "-30d", "_COMM=sudo"]).map(|j| parse_sudo(&j)).unwrap_or_default()
}

/// Failed sign-in attempts (lock screen, login, polkit) from the journal; "Login", "lock" and "polkit" lines.
pub fn parse_failed_logins(journal: &str) -> Vec<LogEntry> {
    let mut out = Vec::new();
    for line in journal.lines() {
        let Some((ts, rest)) = line.split_once(' ') else { continue };
        let time = ts.split('.').next().and_then(|t| t.parse::<i64>().ok()).unwrap_or(0);
        let Some((head, msg)) = rest.split_once("]: ") else { continue };
        let prog = head.split_whitespace().nth(1).unwrap_or("").split('[').next().unwrap_or("");
        if prog == "sudo" || !(msg.contains("authentication failure") || msg.contains("FAILED LOGIN") || msg.contains("Failed password")) {
            continue;
        }
        let user = msg.rsplit("user=").next().filter(|_| msg.contains("user=")).unwrap_or("").trim();
        let place = match prog {
            "labyrinth-lock" | "kscreenlocker_greet" => "Lock screen",
            "sddm-helper" | "sddm-helper-start" | "login" => "Login screen",
            "polkit-agent-helper-1" | "labyrinth-polkit" => "Administrator question",
            "sshd" => "SSH",
            other => other,
        };
        out.push(LogEntry { time, title: format!("Wrong password at {place}"), detail: if user.is_empty() { prog.to_string() } else { format!("user {user}") }, failed: true });
    }
    out.reverse();
    out
}

/// Sign-ins (from the login records) followed by failed attempts (from the journal).
pub fn login_history() -> (Vec<LogEntry>, Vec<LogEntry>) {
    let mut ok = Vec::new();
    if let Some(t) = crate::sys::run("last", &["-n", "40", "-F", "-w"]) {
        for l in t.lines() {
            let p: Vec<&str> = l.split_whitespace().collect();
            if p.len() < 8 || p[0] == "reboot" || p[0] == "wtmp" {
                continue;
            }
            // user tty ... Fri Oct 16 19:13:00 2026 ...
            let Some(d) = p.iter().position(|w| matches!(*w, "Mon" | "Tue" | "Wed" | "Thu" | "Fri" | "Sat" | "Sun")) else { continue };
            if p.len() < d + 5 {
                continue;
            }
            let when = p[d..d + 5].join(" ");
            let ended = if l.contains("still logged in") { "still signed in".to_string() } else { "signed out".to_string() };
            ok.push(LogEntry { time: 0, title: format!("{} on {}", p[0], p[1]), detail: format!("{when} · {ended}"), failed: false });
        }
    }
    let bad = crate::sys::run("journalctl", &["--no-pager", "-o", "short-unix", "--since", "-30d", "-g", "authentication failure|FAILED LOGIN|Failed password"]).map(|j| parse_failed_logins(&j)).unwrap_or_default();
    (ok, bad)
}

#[cfg(test)]
mod log_tests {
    use super::*;

    #[test]
    fn sudo_lines() {
        let j = "1792281112.048605 Orthanc sudo[3504705]: claude-agent : PWD=/x ; USER=root ; TSID=00005E ; COMMAND=/usr/bin/install -m755 a b\n\
                 1792165618.051855 Orthanc sudo[1339003]:  zlothra : 1 incorrect password attempt ; PWD=/x ; USER=root ; COMMAND=/usr/bin/ls\n\
                 1792281112.9 Orthanc sudo[3]: pam_unix(sudo:session): session closed for user root\n";
        let e = parse_sudo(j);
        assert_eq!(e.len(), 2);
        assert!(e[1].title.contains("install"));
        assert_eq!(e[1].detail, "claude-agent as root");
        assert!(e[0].failed && e[0].title.starts_with("Wrong password"));
    }

    #[test]
    fn failed_logins_skip_sudo() {
        let j = "1792170078.37 Orthanc labyrinth-lock[1]: pam_unix(login:auth): authentication failure; logname=z uid=1000 user=zlothra\n\
                 1792170079.0 Orthanc sudo[2]: pam_unix(sudo:auth): authentication failure; user=zlothra\n";
        let e = parse_failed_logins(j);
        assert_eq!(e.len(), 1);
        assert!(e[0].title.contains("Lock screen"));
    }
}
