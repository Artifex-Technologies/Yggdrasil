//! The privacy pack's switches that live with the person, not the system: `$XDG_CONFIG_HOME/heimdall/privacy.json`
//! (`~/.config/heimdall/privacy.json`), shared with Heimdall's downloads guard, which follows the file by itself:
//!
//! ```json
//! {"downloads_guard": true, "folders_guard": true, "remove_hidden_info": true, "lookalike_warnings": true}
//! ```
//!
//! Every switch is on unless the file says `false`: a missing file, a missing key or a value that is not a boolean is on.
//! Settings writes it (Security > Privacy); Minos reads `remove_hidden_info`, Settings and Agora read `lookalike_warnings`.
//! A write keeps every other key in the file and replaces it whole (a temporary file beside it, then a rename).

use std::path::{Path, PathBuf};

/// Heimdall's downloads guard: a file that contacts a server when opened is reported.
pub const DOWNLOADS_GUARD: &str = "downloads_guard";
/// The same guard on the person's own folders (Desktop, Documents, Pictures, Music, Videos) and the notice when a drive is
/// connected; it does nothing while `downloads_guard` is off.
pub const FOLDERS_GUARD: &str = "folders_guard";
/// Minos' right-click > Remove hidden info.
pub const REMOVE_HIDDEN_INFO: &str = "remove_hidden_info";
/// The warning beside names that imitate others (letters of another alphabet, hidden characters).
pub const LOOKALIKE_WARNINGS: &str = "lookalike_warnings";

/// The file's path.
pub fn path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_else(|| "/".into())).join(".config"));
    base.join("heimdall").join("privacy.json")
}

/// One switch from the file's text: on unless it is the boolean false.
pub fn read_in(text: &str, key: &str) -> bool {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => v.get(key).and_then(|x| x.as_bool()).unwrap_or(true),
        Err(_) => true,
    }
}

/// One switch (on when the file cannot be read).
pub fn get(key: &str) -> bool {
    get_at(&path(), key)
}

pub fn get_at(file: &Path, key: &str) -> bool {
    std::fs::read_to_string(file).map(|t| read_in(&t, key)).unwrap_or(true)
}

/// The file's text with one switch set, every other key kept (a file that is not a JSON object starts again).
pub fn set_in(text: &str, key: &str, on: bool) -> String {
    let mut m = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(serde_json::Value::Object(m)) => m,
        _ => serde_json::Map::new(),
    };
    m.insert(key.to_string(), serde_json::Value::Bool(on));
    let mut s = serde_json::to_string_pretty(&serde_json::Value::Object(m)).unwrap_or_default();
    s.push('\n');
    s
}

/// Set one switch.
pub fn set(key: &str, on: bool) -> Result<(), String> {
    set_at(&path(), key, on)
}

pub fn set_at(file: &Path, key: &str, on: bool) -> Result<(), String> {
    let dir = file.parent().ok_or("no folder for the privacy settings")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("could not make {}: {e}", dir.display()))?;
    let old = std::fs::read_to_string(file).unwrap_or_default();
    let tmp = dir.join(format!(".privacy.json.{}.tmp", std::process::id()));
    std::fs::write(&tmp, set_in(&old, key, on)).map_err(|e| format!("could not write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, file).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("could not save {}: {e}", file.display())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn everything_is_on_unless_the_file_says_false() {
        assert!(read_in("", DOWNLOADS_GUARD), "no file");
        assert!(read_in(r#"{"downloads_guard": false}"#, FOLDERS_GUARD), "folders_guard missing: on");
        assert!(!read_in(r#"{"folders_guard": false}"#, FOLDERS_GUARD));
        assert!(read_in("not json", DOWNLOADS_GUARD));
        assert!(read_in("{}", REMOVE_HIDDEN_INFO), "no key");
        assert!(read_in(r#"{"remove_hidden_info": "no"}"#, REMOVE_HIDDEN_INFO), "not a boolean");
        assert!(read_in(r#"{"remove_hidden_info": 0}"#, REMOVE_HIDDEN_INFO));
        assert!(!read_in(r#"{"remove_hidden_info": false}"#, REMOVE_HIDDEN_INFO));
        assert!(read_in(r#"{"remove_hidden_info": false}"#, LOOKALIKE_WARNINGS), "another key");
        assert!(read_in("[false]", LOOKALIKE_WARNINGS));
    }

    #[test]
    fn a_write_keeps_the_other_keys() {
        let t = set_in(r#"{"downloads_guard": false, "folders": ["~/Torrents"]}"#, LOOKALIKE_WARNINGS, false);
        let v: serde_json::Value = serde_json::from_str(&t).unwrap();
        assert_eq!(v["downloads_guard"], false);
        assert_eq!(v["folders"][0], "~/Torrents");
        assert_eq!(v["lookalike_warnings"], false);
        let t = set_in("[1, 2]", DOWNLOADS_GUARD, true);
        assert_eq!(serde_json::from_str::<serde_json::Value>(&t).unwrap(), serde_json::json!({"downloads_guard": true}));
    }

    #[test]
    fn the_file_is_written_whole() {
        let d = std::env::temp_dir().join(format!("ygg-privacy-prefs-{}", std::process::id()));
        let f = d.join("heimdall/privacy.json");
        let _ = std::fs::remove_dir_all(&d);
        assert!(get_at(&f, REMOVE_HIDDEN_INFO));
        set_at(&f, REMOVE_HIDDEN_INFO, false).unwrap();
        assert!(!get_at(&f, REMOVE_HIDDEN_INFO));
        assert!(get_at(&f, DOWNLOADS_GUARD));
        set_at(&f, DOWNLOADS_GUARD, false).unwrap();
        set_at(&f, REMOVE_HIDDEN_INFO, true).unwrap();
        assert!(get_at(&f, REMOVE_HIDDEN_INFO) && !get_at(&f, DOWNLOADS_GUARD));
        assert_eq!(std::fs::read_dir(f.parent().unwrap()).unwrap().count(), 1, "no temporary file left");
        let _ = std::fs::remove_dir_all(&d);
    }
}
