//! Light or dark, and the accent colour: one small file, `~/.config/yggdrasil/appearance.conf`, that every part of the desktop reads
//! (the panel, the apps, the portal that tells other programs). Settings > Appearance writes it through `ygg-theme apply`.
//!
//! ```text
//! style = dark          # or light
//! accent = 124,84,214   # r,g,b; absent: the operating system's default accent
//! ```
//!
//! Until the file exists, the values are read from KDE's `kdeglobals` (where they were kept before), once, so nothing changes on
//! machines that still have it.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Appearance {
    pub dark: bool,
    /// The chosen accent; None means the operating system's default (integrations::default_accent).
    pub accent: Option<[u8; 3]>,
}

impl Default for Appearance {
    fn default() -> Self {
        Appearance { dark: true, accent: None }
    }
}

pub fn path() -> PathBuf {
    crate::dirs::config().join("appearance.conf")
}

fn rgb(v: &str) -> Option<[u8; 3]> {
    let parts: Vec<u8> = v.trim().trim_matches('"').split(',').filter_map(|x| x.trim().parse().ok()).collect();
    (parts.len() == 3).then(|| [parts[0], parts[1], parts[2]])
}

/// Reads `style` and `accent` lines.
pub fn parse(text: &str) -> Appearance {
    let mut a = Appearance::default();
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "style" => a.dark = v.trim() != "light",
            "accent" => a.accent = rgb(v),
            _ => {}
        }
    }
    a
}

/// The same two values from KDE's settings text (`ColorScheme=` names a dark or light scheme, `AccentColor=r,g,b` in `[General]`).
pub fn parse_kdeglobals(text: &str) -> Appearance {
    let mut a = Appearance::default();
    let mut general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            general = line == "[General]";
            continue;
        }
        if !general {
            continue;
        }
        if let Some(v) = line.strip_prefix("ColorScheme=") {
            let v = v.trim().to_lowercase();
            a.dark = v.is_empty() || v.contains("dark");
        } else if let Some(v) = line.strip_prefix("AccentColor=") {
            a.accent = rgb(v);
        }
    }
    a
}

fn load_from(own: &Path, kdeglobals: &Path) -> Appearance {
    if let Ok(t) = std::fs::read_to_string(own) {
        return parse(&t);
    }
    std::fs::read_to_string(kdeglobals).map(|t| parse_kdeglobals(&t)).unwrap_or_default()
}

/// The current choice (cheap: one small file read).
pub fn load() -> Appearance {
    let kde = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    load_from(&path(), &kde.join("kdeglobals"))
}

pub fn save(a: &Appearance) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut text = format!("style = {}\n", if a.dark { "dark" } else { "light" });
    if let Some([r, g, b]) = a.accent {
        text.push_str(&format!("accent = {r},{g},{b}\n"));
    }
    let tmp = p.with_extension("conf.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_its_own_file_and_falls_back_to_kde() {
        assert_eq!(parse("style = light\naccent = 10, 20, 30\n"), Appearance { dark: false, accent: Some([10, 20, 30]) });
        assert_eq!(parse("# nothing\n"), Appearance::default());
        assert_eq!(parse("accent = 300,0,0\n").accent, None, "out of range");
        let kde = "[General]\nColorScheme=YggdrasilLight\nAccentColor=124,84,214\n[KDE]\nAccentColor=1,2,3\n";
        assert_eq!(parse_kdeglobals(kde), Appearance { dark: false, accent: Some([124, 84, 214]) });
        assert!(parse_kdeglobals("[General]\nColorScheme=BreezeDark\n").dark);

        let d = std::env::temp_dir().join(format!("ygg-appearance-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let (own, kg) = (d.join("appearance.conf"), d.join("kdeglobals"));
        std::fs::write(&kg, kde).unwrap();
        assert_eq!(load_from(&own, &kg), Appearance { dark: false, accent: Some([124, 84, 214]) }, "no own file yet: KDE's");
        std::fs::write(&own, "style = dark\n").unwrap();
        assert_eq!(load_from(&own, &kg), Appearance { dark: true, accent: None }, "the own file wins once it exists");
        let _ = std::fs::remove_dir_all(&d);
    }
}
