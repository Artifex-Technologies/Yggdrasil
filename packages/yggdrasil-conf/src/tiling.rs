//! Which screens tile (Omarchy-style: windows side by side, their own workspaces, a slim bar without the dock) while the others keep
//! floating windows and the dock. One small file, `~/.config/yggdrasil/tiling`, one screen (connector name, e.g. `HDMI-A-1`) per line:
//! Settings > Displays writes it, and the panel and the compositor read it.
//!
//! Until the file exists, the choice made under KDE is read once (KWin's `kwinrc`, `[Script-labyrinth-screen-keys] tilingOutputs`).

use std::path::{Path, PathBuf};

pub fn path() -> PathBuf {
    crate::dirs::config().join("tiling")
}

/// Screen names from the file's text: one per line (commas also work), `#` comments.
pub fn parse(text: &str) -> Vec<String> {
    let mut out: Vec<String> = vec![];
    for line in text.lines() {
        for name in line.split('#').next().unwrap_or("").split(',') {
            let n = name.trim();
            if !n.is_empty() && n.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && !out.iter().any(|o| o == n) {
                out.push(n.to_string());
            }
        }
    }
    out
}

/// The same list from KWin's settings text.
pub fn parse_kwinrc(text: &str) -> Vec<String> {
    let mut group = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            group = line == "[Script-labyrinth-screen-keys]";
            continue;
        }
        if group {
            if let Some(v) = line.strip_prefix("tilingOutputs=") {
                return parse(v);
            }
        }
    }
    vec![]
}

fn load_from(own: &Path, kwinrc: &Path) -> Vec<String> {
    match std::fs::read_to_string(own) {
        Ok(t) => parse(&t),
        Err(_) => std::fs::read_to_string(kwinrc).map(|t| parse_kwinrc(&t)).unwrap_or_default(),
    }
}

/// The screens that tile.
pub fn outputs() -> Vec<String> {
    let kde = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    load_from(&path(), &kde.join("kwinrc"))
}

pub fn set_outputs(names: &[String]) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut text = String::from("# Screens that tile (Settings > Displays). One per line.\n");
    for n in parse(&names.join("\n")) {
        text.push_str(&n);
        text.push('\n');
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_the_file_or_from_kde() {
        assert_eq!(parse("# tiles\nHDMI-A-1\nDP-2, DP-3\n\nHDMI-A-1\nbad name!\n"), vec!["HDMI-A-1", "DP-2", "DP-3"]);
        assert_eq!(parse_kwinrc("[Other]\ntilingOutputs=X\n[Script-labyrinth-screen-keys]\ntilingOutputs=HDMI-A-1,DP-2\n"), vec!["HDMI-A-1", "DP-2"]);
        let d = std::env::temp_dir().join(format!("ygg-tiling-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (own, kw) = (d.join("tiling"), d.join("kwinrc"));
        std::fs::write(&kw, "[Script-labyrinth-screen-keys]\ntilingOutputs=HDMI-A-1\n").unwrap();
        assert_eq!(load_from(&own, &kw), vec!["HDMI-A-1"], "no own file yet: KDE's choice");
        std::fs::write(&own, "").unwrap();
        assert!(load_from(&own, &kw).is_empty(), "an empty own file means none tile");
        let _ = std::fs::remove_dir_all(&d);
    }
}
