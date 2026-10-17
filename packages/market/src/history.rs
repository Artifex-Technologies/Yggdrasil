//! What has been installed through the Marketplace: an entry is added when an install (or reinstall) *finishes*. A cancelled or failed one
//! is never added. Kept in ~/.local/share/labyrinth/market/history.json, newest first.

use std::path::PathBuf;

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub id: String,
    pub name: String,
    /// "installed" or "reinstalled"
    pub what: String,
    pub when: String,
    pub at: i64,
}

fn path() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share"))).unwrap_or_default().join("labyrinth/market/history.json")
}

pub fn load() -> Vec<Item> {
    let v: Value = std::fs::read_to_string(path()).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null);
    v.as_array().map(|a| a.iter().map(|x| Item { id: s(x, "id"), name: s(x, "name"), what: s(x, "what"), when: s(x, "when"), at: x.get("at").and_then(|n| n.as_i64()).unwrap_or(0) }).filter(|i| !i.id.is_empty()).collect()).unwrap_or_default()
}

fn s(v: &Value, k: &str) -> String {
    v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

fn save(items: &[Item]) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let v: Vec<Value> = items.iter().map(|i| json!({"id": i.id, "name": i.name, "what": i.what, "when": i.when, "at": i.at})).collect();
    let _ = std::fs::write(p, serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// Add a finished install to the top of the history (an app installed again moves up instead of appearing twice).
pub fn record(id: &str, name: &str, what: &str) {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let when = std::process::Command::new("date").arg("+%b %-d, %Y").output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default();
    let mut items = load();
    items.retain(|i| i.id != id);
    items.insert(0, Item { id: id.to_string(), name: name.to_string(), what: what.to_string(), when, at: now });
    items.truncate(500);
    save(&items);
}

/// Forget one entry, or all.
pub fn forget(id: Option<&str>) {
    let mut items = load();
    match id {
        Some(i) => items.retain(|x| x.id != i),
        None => items.clear(),
    }
    save(&items);
}
