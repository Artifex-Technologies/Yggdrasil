//! Settings > Sarina's agents: the small programs Sarina wrote (Agent Forge), whether you approved their current version, and what the forge
//! did lately. Agents only ever run inside Limbo's disposable guests. Approving happens in a terminal (`sarina forge-approve`), where every file
//! is shown first and you type "yes": this page only opens that terminal, so nothing a program does here can approve an agent.

use std::path::PathBuf;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct Agent {
    pub name: String,
    pub language: String,
    pub purpose: String,
    pub command: String,
    pub files: u64,
    pub hash: String,
    pub approved: bool,
    pub problem: String,
}

#[derive(Clone, Debug, Default)]
pub struct State {
    pub on: bool,
    /// The daily check-up timer is switched on (only the owner does that, here).
    pub checkup: bool,
    /// The daily reflection on talks (what she learns about the owner) is switched on.
    pub learn: bool,
    pub agents: Vec<Agent>,
    /// Newest first: "time  action  agent  outcome  detail".
    pub activity: Vec<String>,
}

/// Sarina is optional: the page only exists when she is installed.
pub fn available() -> bool {
    static A: OnceLock<bool> = OnceLock::new();
    *A.get_or_init(|| ["/usr/bin/sarina", "/usr/local/bin/sarina"].iter().any(|p| std::path::Path::new(p).exists()))
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default()
}

fn forge_on() -> bool {
    let p = home().join(".config/labyrinth/sarina.env");
    std::fs::read_to_string(p).map(|t| t.lines().any(|l| l.trim() == "SARINA_FORGE=on")).unwrap_or(false)
}

pub fn parse_list(json: &str) -> Vec<Agent> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    v.as_array()
        .map(|a| {
            a.iter()
                .map(|r| Agent {
                    name: r["name"].as_str().unwrap_or("").into(),
                    language: r["language"].as_str().unwrap_or("").into(),
                    purpose: r["purpose"].as_str().unwrap_or("").into(),
                    command: r["command"].as_str().unwrap_or("").into(),
                    files: r["files"].as_u64().unwrap_or(0),
                    hash: r["hash"].as_str().unwrap_or("").into(),
                    approved: r["approved"].as_bool().unwrap_or(false),
                    problem: r["problem"].as_str().unwrap_or("").into(),
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn parse_audit(text: &str, n: usize) -> Vec<String> {
    let mut out: Vec<String> = text
        .lines()
        .filter_map(|l| serde_json::from_str::<serde_json::Value>(l).ok())
        .map(|e| {
            let g = |k: &str| e[k].as_str().unwrap_or("").to_string();
            let t = g("time").replace('T', " ").trim_end_matches('Z').to_string();
            let detail = g("detail");
            format!("{}  {} {}: {}{}", t, g("action"), g("agent"), g("outcome"), if detail.is_empty() { String::new() } else { format!(" ({detail})") })
        })
        .collect();
    out.reverse();
    out.truncate(n);
    out
}

pub fn load() -> State {
    let agents = crate::sys::run("sarina", &["forge-list", "--json"]).map(|s| parse_list(&s)).unwrap_or_default();
    let audit = std::fs::read_to_string(home().join(".local/state/sarina/forge/forge-audit.jsonl")).unwrap_or_default();
    let enabled = |t: &str| crate::sys::run("systemctl", &["--user", "is-enabled", t]).map(|s| s.trim() == "enabled").unwrap_or(false);
    State { on: forge_on(), checkup: enabled("labyrinth-sarina-checkup.timer"), learn: enabled("labyrinth-sarina-reflect.timer"), agents, activity: parse_audit(&audit, 15) }
}

/// Opens a terminal where the owner reviews every file of the agent and types "yes" to approve exactly that version.
pub fn open_review(name: &str) {
    if name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        crate::sys::spawn("ghostty", &["-e", "sarina", "forge-approve", name]);
    }
}

pub fn remove(name: &str) -> Result<(), String> {
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
        return Err("not an agent name".into());
    }
    crate::sys::act("sarina", &["forge-remove", name])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_list_and_audit() {
        let a = parse_list(r#"[{"name":"hello","language":"python","purpose":"Says hi.","command":"python3 main.py","files":1,"hash":"abc","approved":true}]"#);
        assert_eq!(a.len(), 1);
        assert!(a[0].approved && a[0].files == 1 && a[0].name == "hello");
        assert!(parse_list("not json").is_empty());
        let log = "{\"time\":\"2026-10-22T10:00:00Z\",\"action\":\"create\",\"agent\":\"hello\",\"outcome\":\"ok\",\"detail\":\"1 files\"}\n{\"time\":\"2026-10-22T10:01:00Z\",\"action\":\"run\",\"agent\":\"hello\",\"outcome\":\"ok\",\"detail\":\"\"}\n";
        let lines = parse_audit(log, 10);
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("run hello: ok"));
    }

    #[test]
    fn odd_names_are_not_passed_on() {
        assert!(remove("../x").is_err());
        assert!(remove("a b").is_err());
    }
}

/// Switches the daily check-up on (installs the user units from /usr/share/labyrinth once) or off.
pub fn set_checkup(on: bool) -> Result<(), String> {
    set_daily("labyrinth-sarina-checkup", on)
}

/// Switches the daily reflection on talks on or off (Entities/About Zlothra in her vault).
pub fn set_learn(on: bool) -> Result<(), String> {
    set_daily("labyrinth-sarina-reflect", on)
}

fn set_daily(unit: &str, on: bool) -> Result<(), String> {
    let timer = format!("{unit}.timer");
    if on {
        let dir = home().join(".config/systemd/user");
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        for f in [format!("{unit}.service"), timer.clone()] {
            let dst = dir.join(&f);
            if !dst.exists() {
                std::fs::copy(PathBuf::from("/usr/share/labyrinth").join(&f), &dst).map_err(|e| format!("cannot install {f}: {e}"))?;
            }
        }
        let _ = crate::sys::act("systemctl", &["--user", "daemon-reload"]);
        crate::sys::act("systemctl", &["--user", "enable", "--now", &timer])
    } else {
        crate::sys::act("systemctl", &["--user", "disable", "--now", &timer])
    }
}
