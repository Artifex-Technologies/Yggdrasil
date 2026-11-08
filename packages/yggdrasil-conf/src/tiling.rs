//! Which screens tile (Omarchy-style: windows side by side, their own workspaces, a slim bar without the dock) while the others keep
//! floating windows and the dock. One small file, `~/.config/yggdrasil/tiling`, one screen (connector name, e.g. `HDMI-A-1`) per line:
//! Settings > Displays writes it, and the panel and the compositor read it.
//! No file: no screen tiles.

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

fn load_from(own: &Path) -> Vec<String> {
    std::fs::read_to_string(own).map(|t| parse(&t)).unwrap_or_default()
}

/// The screens that tile.
pub fn outputs() -> Vec<String> {
    load_from(&path())
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
    fn names_from_the_file() {
        assert_eq!(parse("# tiles\nHDMI-A-1\nDP-2, DP-3\n\nHDMI-A-1\nbad name!\n"), vec!["HDMI-A-1", "DP-2", "DP-3"]);
        let d = std::env::temp_dir().join(format!("ygg-tiling-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let own = d.join("tiling");
        std::fs::write(d.join("kwinrc"), "[Script-labyrinth-screen-keys]\ntilingOutputs=HDMI-A-1\n").unwrap();
        assert!(load_from(&own).is_empty(), "no file: none tile, whatever KWin's file says");
        std::fs::write(&own, "HDMI-A-1\n").unwrap();
        assert_eq!(load_from(&own), vec!["HDMI-A-1"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
