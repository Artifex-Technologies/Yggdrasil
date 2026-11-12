//! The dock's pinned apps: `~/.config/yggdrasil/dock` (one desktop id per line), else the operating system's or the
//! administrator's default, else the desktop's own. The panel shows them and re-reads the file when it changes; Agora pins and
//! unpins an app from its page.

use std::path::PathBuf;

pub fn path() -> PathBuf {
    crate::dirs::config().join("dock")
}

fn parse(text: &str) -> Vec<String> {
    text.lines()
        .map(|l| l.trim().trim_end_matches(".desktop").replace("finder", "minos"))
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .collect()
}

/// The pinned apps, in order.
pub fn pinned() -> Vec<String> {
    for p in [path(), crate::dirs::system_config().join("dock"), crate::dirs::system_data().join("dock")] {
        if let Ok(t) = std::fs::read_to_string(p) {
            let v = parse(&t);
            if !v.is_empty() {
                return v;
            }
        }
    }
    vec![]
}

/// Writes the pinned apps (one id per line) to the user's file.
pub fn save_pinned(ids: &[String]) {
    let p = path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, ids.join("\n") + "\n");
}

/// The list with `id` pinned at the end (`on`) or taken out; unchanged when it already is so.
pub fn with(mut ids: Vec<String>, id: &str, on: bool) -> Vec<String> {
    let id = id.trim_end_matches(".desktop");
    let has = ids.iter().any(|x| x == id);
    if on && !has {
        ids.push(id.to_string());
    } else if !on {
        ids.retain(|x| x != id);
    }
    ids
}

/// Pins or unpins `id` (the default list becomes the user's own on the first change).
pub fn set_pinned(id: &str, on: bool) {
    save_pinned(&with(pinned(), id, on));
}

/// When the user's file last changed (the panel compares it to know when to read the list again).
pub fn modified() -> Option<std::time::SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pinning_adds_once_at_the_end_and_unpinning_takes_out() {
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(with(ids(&["minos", "vivaldi-stable"]), "code.desktop", true), ids(&["minos", "vivaldi-stable", "code"]));
        assert_eq!(with(ids(&["minos", "code"]), "code", true), ids(&["minos", "code"]));
        assert_eq!(with(ids(&["minos", "code"]), "code", false), ids(&["minos"]));
        assert_eq!(parse("# mine\nfinder.desktop\n\n code \n"), ids(&["minos", "code"]));
    }
}
