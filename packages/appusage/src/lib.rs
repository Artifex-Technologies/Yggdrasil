//! labyrinth-appusage in Rust: how long each app has been running, for Settings > Apps ("sort by usage").
//!
//! The Python script it replaces looked every 30 seconds at this user's running systemd scopes (`systemctl --user list-units
//! --type=scope --state=running`) and added 30 seconds to every app that had one. This one is told instead: each scope is a
//! folder in this user's cgroup tree (/sys/fs/cgroup/user.slice/user-UID.slice/user@UID.service/...), and inotify says when
//! one is made or removed. Between those events nothing runs: the process sleeps in poll() and only wakes to write the time
//! down (every five minutes while an app runs, when an app stops, and when the service is stopped).
//!
//! Kept exactly from the script: which scope belongs to which desktop entry (match_unit), which desktop entries there are
//! (desktop_ids, read again at most every ten minutes), the file (~/.local/share/labyrinth/app-usage.json, `seconds` next to
//! the launch counts labyrinth-launch writes, every other key kept, written whole through a .tmp file), and `--once`.
//!
//! Without a cgroup v2 user tree (no systemd user manager) it falls back to the script's way: systemctl every 30 seconds.

pub mod watch;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use pycompat::json::{self, Map, Value};

/// The script's sampling interval: `--once` adds this much, and the fallback polls this often.
pub const INTERVAL: u64 = 30;

/// $XDG_DATA_HOME, else ~/.local/share.
pub fn data_home() -> PathBuf {
    match std::env::var_os("XDG_DATA_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => home().join(".local/share"),
    }
}

fn home() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME") {
        return PathBuf::from(h);
    }
    // as expanduser does without HOME: the password database's home folder
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            let dir = std::ffi::CStr::from_ptr((*pw).pw_dir);
            return PathBuf::from(std::ffi::OsStr::from_encoded_bytes_unchecked(dir.to_bytes()));
        }
    }
    PathBuf::from("~")
}

/// The usage file: DATA_HOME/labyrinth/app-usage.json.
pub fn usage_path() -> PathBuf {
    data_home().join("labyrinth").join("app-usage.json")
}

/// Every desktop entry's id (the file name without .desktop) in the folders the script read.
pub fn desktop_ids() -> BTreeSet<String> {
    let dh = data_home();
    let mut dirs = vec![dh.join("applications")];
    let xdg = std::env::var("XDG_DATA_DIRS").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(xdg.split(':').filter(|d| !d.is_empty()).map(|d| Path::new(d).join("applications")));
    dirs.push(dh.join("flatpak/exports/share/applications"));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    let mut ids = BTreeSet::new();
    for d in dirs {
        let Ok(rd) = fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if let Some(n) = e.file_name().to_str() {
                if let Some(id) = n.strip_suffix(".desktop") {
                    ids.insert(id.to_string());
                }
            }
        }
    }
    ids
}

/// re.sub(r"[^A-Za-z0-9_]", "_", app_id)
pub fn sanitize(app_id: &str) -> String {
    app_id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect()
}

/// \d+ in a Python pattern: one or more decimal digits of any script.
fn all_digits(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| pycompat::decimal(c).is_some())
}

/// The desktop id a scope belongs to, or None. Handles lapp_<id>_N.scope (labyrinth-launch's), app-<id>-PID.scope and
/// app-<id>@HASH.scope (other launchers, with \x2d for a dash), and app-flatpak-<id>-N.scope: the script's two patterns,
///   lapp_(.+)_(\d+)\.scope                                 matched against each id's sanitize()
///   app-(?:flatpak-)?(.+?)(?:-\d+|@[0-9a-f]+)\.scope       the shortest id that leaves a -N or @HASH, flatpak- taken off first
/// When two ids sanitize to the same name the script took whichever its set gave first (that changes from run to run); this
/// takes the first in sorted order.
pub fn match_unit(unit: &str, ids: &BTreeSet<String>) -> Option<String> {
    let unit = unit.replace("\\x2d", "-");
    if let Some(mid) = unit.strip_prefix("lapp_").and_then(|r| r.strip_suffix(".scope")) {
        // (.+) is greedy and \d+ has no underscore: the split is at the last underscore
        if let Some((san, n)) = mid.rsplit_once('_') {
            if !san.is_empty() && !san.contains('\n') && all_digits(n) {
                return ids.iter().find(|i| sanitize(i) == san).cloned();
            }
        }
        // no match of the first pattern: the second one cannot match either (it needs "app-")
        return None;
    }
    let rest = unit.strip_prefix("app-")?;
    // (?:flatpak-)? is tried taken first; only when nothing matches that way is it left in
    let group = rest.strip_prefix("flatpak-").and_then(shortest_id).or_else(|| shortest_id(rest))?;
    ids.contains(group).then(|| group.to_string())
}

/// (.+?)(?:-\d+|@[0-9a-f]+)\.scope as a full match of `s`: the shortest non-empty start (without a newline) whose rest is
/// -DIGITS.scope or @HEX.scope.
fn shortest_id(s: &str) -> Option<&str> {
    for (i, _) in s.char_indices().skip(1) {
        let (id, tail) = s.split_at(i);
        if id.contains('\n') {
            return None;
        }
        if let Some(t) = tail.strip_suffix(".scope") {
            if let Some(d) = t.strip_prefix('-') {
                if all_digits(d) {
                    return Some(id);
                }
            }
            if let Some(h) = t.strip_prefix('@') {
                if !h.is_empty() && h.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
                    return Some(id);
                }
            }
        }
    }
    None
}

/// The running scopes, as the script asked systemd for them (the fallback, and --once).
pub fn running_scopes() -> Vec<String> {
    let mut child = match Command::new("systemctl")
        .args(["--user", "list-units", "--type=scope", "--state=running", "--no-legend", "--plain", "--no-pager"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(c) => c,
        Err(_) => return vec![],
    };
    // the script waited at most 10 seconds
    let out = wait_output(&mut child, Duration::from_secs(10)).unwrap_or_default();
    let text = String::from_utf8_lossy(&out);
    text.lines().filter_map(|l| pycompat::split(l).first().map(|s| s.to_string())).collect()
}

fn wait_output(child: &mut std::process::Child, limit: Duration) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut buf = vec![];
        let _ = stdout.read_to_end(&mut buf);
        buf
    });
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if start.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
    reader.join().ok()
}

/// What went wrong writing the usage down.
#[derive(Debug)]
pub enum WriteError {
    /// The file holds JSON that is not an object, or an app's entry the script could not have added to (its `seconds`
    /// not a number, or the entry itself not an object): the script stopped with an error there; this leaves the file alone.
    Unusable(String),
    Io(io::Error),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            WriteError::Unusable(m) => write!(f, "{m}"),
            WriteError::Io(e) => write!(f, "{e}"),
        }
    }
}

/// int(x) of a JSON value as the script applied it to `seconds` (None: int() would have raised).
fn py_int(v: &Value) -> Option<i128> {
    match v {
        Value::Bool(b) => Some(*b as i128),
        Value::Number(n) => n.as_i64().map(|i| i as i128).or_else(|| n.as_u64().map(|u| u as i128)).or_else(|| {
            let f = n.as_f64()?;
            f.is_finite().then(|| f.trunc() as i128)
        }),
        Value::String(s) => pycompat::int(s),
        _ => None,
    }
}

/// A Python value's truth (`usage.get(a) or {}`).
fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Adds whole seconds to each app in the usage file, keeping every other key; the script's sample() after it had worked out
/// which apps were running. An unreadable or broken file counts as empty (as it did for the script).
pub fn add_seconds(path: &Path, add: &BTreeMap<String, i64>) -> Result<(), WriteError> {
    if add.is_empty() {
        return Ok(());
    }
    let mut usage = match fs::read(path).ok().and_then(|b| String::from_utf8(b).ok()).and_then(|t| json::loads(&t).ok()) {
        Some(Value::Object(m)) => m,
        Some(_) => return Err(WriteError::Unusable(format!("{} does not hold an object", path.display()))),
        None => Map::new(),
    };
    for (app, secs) in add {
        let mut e = match usage.get(app) {
            Some(v) if truthy(v) => match v {
                Value::Object(m) => m.clone(),
                _ => return Err(WriteError::Unusable(format!("{app}'s entry is not an object"))),
            },
            _ => Map::new(),
        };
        let before = match e.get("seconds") {
            None => 0,
            Some(v) => py_int(v).ok_or_else(|| WriteError::Unusable(format!("{app}'s seconds is not a number")))?,
        };
        let total = before.saturating_add(*secs as i128);
        let total = Value::from(i64::try_from(total).unwrap_or(if total < 0 { i64::MIN } else { i64::MAX }));
        e.insert("seconds".into(), total);
        usage.insert(app.clone(), Value::Object(e));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(WriteError::Io)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    fs::write(&tmp, json::dumps(&Value::Object(usage))).map_err(WriteError::Io)?;
    fs::rename(&tmp, path).map_err(WriteError::Io)
}

/// The script's sample(seconds, ids, units, path): adds `seconds` to every app that has a running scope (once per app, however
/// many windows it has) and returns those apps.
pub fn sample(seconds: i64, ids: &BTreeSet<String>, units: &[String], path: &Path) -> Result<BTreeSet<String>, WriteError> {
    let apps: BTreeSet<String> = units.iter().filter_map(|u| match_unit(u, ids)).collect();
    let add = apps.iter().map(|a| (a.clone(), seconds)).collect();
    add_seconds(path, &add)?;
    Ok(apps)
}

#[cfg(test)]
mod tests;
