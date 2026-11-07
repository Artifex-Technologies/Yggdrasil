//! Electron apps (VS Code, Obsidian, ...) take part in autocorrect only when started with `--enable-wayland-ime`: without it they
//! never talk to an input method on Wayland. Their launchers read flags from a file, one per line: `~/.config/electron-flags.conf`
//! for every app on Arch's `electron` packages (Obsidian among them), `~/.config/code-flags.conf` for VS Code. This adds the flag to
//! both, under a line saying it was Yggdrasil, and takes out only that pair, so flags put there by hand are never touched.
//!
//! Off unless switched on (Settings > Keyboard > Spelling and languages): with some Electron versions the flag has caused doubled
//! or lost keys, and an app reads its flags only when it starts.

use std::path::{Path, PathBuf};

pub const FLAG: &str = "--enable-wayland-ime";
const MARK: &str = "# added by Yggdrasil for autocorrect in Electron apps (Settings > Keyboard > Spelling and languages)";
/// The flag files, in the config folder.
pub const FILES: [&str; 2] = ["electron-flags.conf", "code-flags.conf"];

fn config_dir() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"))
}

fn has_flag(text: &str) -> bool {
    text.lines().any(|l| l.split_whitespace().next() == Some(FLAG))
}

/// The file's text with the flag added (None: it has the flag already, ours or written by hand).
pub fn with_flag(text: &str) -> Option<String> {
    if has_flag(text) {
        return None;
    }
    let mut out = text.to_string();
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(MARK);
    out.push('\n');
    out.push_str(FLAG);
    out.push('\n');
    Some(out)
}

/// The file's text without the pair this module added (a flag written by hand stays).
pub fn without_flag(text: &str) -> String {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        if lines[i] == MARK && lines.get(i + 1).is_some_and(|l| l.trim() == FLAG) {
            i += 2;
            continue;
        }
        out.push(lines[i]);
        i += 1;
    }
    let mut s = out.join("\n");
    if !s.is_empty() {
        s.push('\n');
    }
    s
}

/// Whether every flag file in `dir` carries the flag (an app started now would take part).
pub fn is_on_in(dir: &Path) -> bool {
    FILES.iter().all(|f| std::fs::read_to_string(dir.join(f)).is_ok_and(|t| has_flag(&t)))
}

pub fn is_on() -> bool {
    is_on_in(&config_dir())
}

/// Adds (on) or takes out (off) the flag in every flag file in `dir`. A file left empty by taking it out is removed.
pub fn set_in(dir: &Path, on: bool) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for f in FILES {
        let path = dir.join(f);
        let text = std::fs::read_to_string(&path).unwrap_or_default();
        if on {
            if let Some(t) = with_flag(&text) {
                std::fs::write(&path, t)?;
            }
        } else if path.exists() {
            let t = without_flag(&text);
            if t.trim().is_empty() {
                std::fs::remove_file(&path)?;
            } else if t != text {
                std::fs::write(&path, t)?;
            }
        }
    }
    Ok(())
}

pub fn set(on: bool) -> std::io::Result<()> {
    set_in(&config_dir(), on)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flag_is_added_once_and_only_ours_is_taken_out() {
        let added = with_flag("").unwrap();
        assert_eq!(added, format!("{MARK}\n{FLAG}\n"));
        assert!(with_flag(&added).is_none(), "already there");
        assert!(with_flag("--enable-wayland-ime\n").is_none(), "written by hand counts too");
        let mine = "--ozone-platform=wayland";
        let both = with_flag(mine).unwrap();
        assert_eq!(both, format!("{mine}\n{MARK}\n{FLAG}\n"));
        assert_eq!(without_flag(&both), format!("{mine}\n"));
        assert_eq!(without_flag("--enable-wayland-ime\n"), "--enable-wayland-ime\n", "a hand-written flag stays");
    }

    #[test]
    fn switching_on_and_off_in_a_config_folder() {
        let dir = std::env::temp_dir().join(format!("ygg-electron-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("code-flags.conf"), "--disable-gpu\n").unwrap();
        assert!(!is_on_in(&dir));
        set_in(&dir, true).unwrap();
        assert!(is_on_in(&dir));
        assert!(std::fs::read_to_string(dir.join("code-flags.conf")).unwrap().starts_with("--disable-gpu\n"));
        set_in(&dir, false).unwrap();
        assert!(!is_on_in(&dir));
        assert_eq!(std::fs::read_to_string(dir.join("code-flags.conf")).unwrap(), "--disable-gpu\n");
        assert!(!dir.join("electron-flags.conf").exists(), "a file only we wrote goes away again");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
