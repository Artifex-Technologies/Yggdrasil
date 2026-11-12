//! yggdrasil-leakwatch: warns when a program's memory keeps growing (see lib.rs for how).
//!
//!   yggdrasil-leakwatch sample [--dry-run]   take one sample, warn about new suspects (the timer runs this every 5 minutes)
//!   yggdrasil-leakwatch report [--json]      what is followed, how each has changed, and which look like leaks
//!
//! History: $XDG_STATE_HOME/yggdrasil/leakwatch/state.json. Settings: $XDG_CONFIG_HOME/yggdrasil/leakwatch.conf
//! (`ignore = name, name` for programs that may grow; a warning about one is never sent).

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};
use yggdrasil_leakwatch::*;

fn home_dir(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(fallback))
}

fn state_path() -> PathBuf {
    home_dir("XDG_STATE_HOME", ".local/state").join("yggdrasil/leakwatch/state.json")
}

fn load() -> State {
    std::fs::read_to_string(state_path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
}

fn save(st: &State) -> std::io::Result<()> {
    let p = state_path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(st)?)?;
    std::fs::rename(tmp, p)
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn notify(title: &str, body: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "Leak watch", "-u", "normal", "-i", "dialog-warning", title, body])
        .stdin(Stdio::null())
        .status();
}

fn sample(dry_run: bool) -> i32 {
    let root = std::env::var_os("LEAKWATCH_PROC").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/proc"));
    let readings = scan(&root);
    if readings.is_empty() {
        eprintln!("yggdrasil-leakwatch: nothing readable under {}", root.display());
        return 1;
    }
    let t = now();
    let mut st = load();
    st.record(t, &readings);
    let conf = std::fs::read_to_string(home_dir("XDG_CONFIG_HOME", ".config").join("yggdrasil/leakwatch.conf")).unwrap_or_default();
    let skip = ignored(&conf);
    for s in st.series.values_mut() {
        let Some(v) = judge(s) else { continue };
        if !v.suspect || skip.iter().any(|n| *n == s.name) || s.warned.is_some_and(|w| t.saturating_sub(w) < 24 * 3600) {
            continue;
        }
        let (title, body) = warning(s, &v);
        if dry_run {
            println!("would warn: {title}: {body}");
        } else {
            eprintln!("yggdrasil-leakwatch: {title}: {body}");
            notify(&title, &body);
            s.warned = Some(t);
        }
    }
    if dry_run {
        println!("{} readings, {} followed (dry run: nothing saved)", readings.len(), st.series.len());
        return 0;
    }
    match save(&st) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("yggdrasil-leakwatch: cannot save {}: {e}", state_path().display());
            1
        }
    }
}

fn report(json: bool) -> i32 {
    let st = load();
    let mut rows: Vec<(&Series, Verdict)> =
        st.series.values().filter(|s| s.points.len() >= 2).filter_map(|s| judge(s).map(|v| (s, v))).collect();
    rows.sort_by(|a, b| b.1.per_hour_kb.total_cmp(&a.1.per_hour_kb));
    if json {
        let out: Vec<serde_json::Value> = rows
            .iter()
            .map(|(s, v)| {
                serde_json::json!({"name": s.name, "kind": s.kind, "pid": s.pid, "holder": s.holder, "first_kb": v.first_kb,
                    "last_kb": v.last_kb, "hours": v.hours, "per_hour_kb": v.per_hour_kb, "suspect": v.suspect})
            })
            .collect();
        println!("{}", serde_json::Value::Array(out));
        return 0;
    }
    if rows.is_empty() {
        println!("Nothing followed yet: the first samples come 10 minutes after login, then every 5 minutes.");
        return 0;
    }
    println!("{:<34} {:>9} {:>9} {:>7} {:>11}", "Program", "then", "now", "hours", "per hour");
    for (s, v) in rows.iter().take(25) {
        let name = match s.kind {
            Kind::Process => s.name.clone(),
            Kind::Buffers => format!("{} (images in {})", s.name, s.holder),
        };
        let mark = if v.suspect { "  <- keeps growing" } else { "" };
        println!(
            "{:<34} {:>9} {:>9} {:>7.1} {:>11}{mark}",
            name,
            human(v.first_kb),
            human(v.last_kb),
            v.hours,
            human(v.per_hour_kb as u64)
        );
    }
    println!("\"then\" and \"now\" are the lowest readings of the first and last hour, so short peaks do not count.");
    0
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let has = |f: &str| args.iter().any(|a| a == f);
    let code = match args.first().map(String::as_str) {
        Some("sample") => sample(has("--dry-run")),
        Some("report") => report(has("--json")),
        _ => {
            eprintln!("usage: yggdrasil-leakwatch sample [--dry-run] | report [--json]");
            2
        }
    };
    std::process::exit(code);
}
