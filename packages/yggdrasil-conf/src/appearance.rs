//! Light or dark, the accent colour and the icon pack: one small file, `~/.config/yggdrasil/appearance.conf`, that every part of the
//! desktop reads (the panel, the lock screen, the apps, the portal that tells other programs). Settings > Appearance writes it (the
//! style and accent through `ygg-theme apply`).
//!
//! ```text
//! style = dark          # or light
//! accent = 124,84,214   # r,g,b; absent: the operating system's default accent
//! icons = Liki          # the icon pack; absent: Liki
//! cursor = macOS        # the pointer theme (ygg-theme); absent: Yggdrasil's own
//! ```
//!
//! Nothing is read from KDE's settings files (2026-11-08): the values that were kept there were moved here.

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

fn load_from(own: &Path) -> Appearance {
    std::fs::read_to_string(own).map(|t| parse(&t)).unwrap_or_default()
}

/// The current choice (cheap: one small file read).
pub fn load() -> Appearance {
    load_from(&path())
}

/// The icon pack chosen in Settings > Appearance > Icons (`icons = NAME`); None when none was chosen (Liki is used then).
pub fn parse_icons(text: &str) -> Option<String> {
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = line.split_once('=') {
            let v = v.trim();
            if k.trim() == "icons" && !v.is_empty() && !v.contains('/') && !v.starts_with('.') {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// The icon pack in use (see `parse_icons`).
pub fn icon_pack() -> Option<String> {
    std::fs::read_to_string(path()).ok().and_then(|t| parse_icons(&t))
}

/// The file's text with the icon pack set to `id`, every other line kept.
pub fn render_icons(id: &str, old: &str) -> String {
    let mut text = String::new();
    for line in old.lines() {
        let key = line.split('#').next().unwrap_or("").split_once('=').map(|(k, _)| k.trim());
        if key != Some("icons") && !line.trim().is_empty() {
            text.push_str(line);
            text.push('\n');
        }
    }
    text.push_str(&format!("icons = {id}\n"));
    text
}

/// Choose the icon pack (Settings > Appearance > Icons).
pub fn set_icon_pack(id: &str) -> std::io::Result<()> {
    if id.is_empty() || id.contains('/') || id.starts_with('.') || id.contains('\n') {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "not an icon pack name"));
    }
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let tmp = p.with_extension("conf.tmp");
    std::fs::write(&tmp, render_icons(id, &old))?;
    std::fs::rename(tmp, p)
}

pub fn save(a: &Appearance) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let old = std::fs::read_to_string(&p).unwrap_or_default();
    let tmp = p.with_extension("conf.tmp");
    std::fs::write(&tmp, render(a, &old))?;
    std::fs::rename(tmp, p)
}

/// The file's text for these values: `style` and `accent` written anew, every other line of the old text (the icon pack, the
/// pointer theme ygg-theme keeps here, comments) kept as it was.
pub fn render(a: &Appearance, old: &str) -> String {
    let mut text = format!("style = {}\n", if a.dark { "dark" } else { "light" });
    if let Some([r, g, b]) = a.accent {
        text.push_str(&format!("accent = {r},{g},{b}\n"));
    }
    for line in old.lines() {
        let key = line.split('#').next().unwrap_or("").split_once('=').map(|(k, _)| k.trim());
        if !matches!(key, Some("style" | "accent")) && !line.trim().is_empty() {
            text.push_str(line);
            text.push('\n');
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn render_keeps_other_lines() {
        let a = Appearance { dark: false, accent: Some([1, 2, 3]) };
        assert_eq!(render(&a, "style = dark\naccent = 9,9,9\ncursor = macOS\n"), "style = light\naccent = 1,2,3\ncursor = macOS\n");
        assert_eq!(render(&Appearance { dark: true, accent: None }, ""), "style = dark\n");
    }

    #[test]
    fn reads_its_own_file() {
        assert_eq!(parse("style = light\naccent = 10, 20, 30\n"), Appearance { dark: false, accent: Some([10, 20, 30]) });
        assert_eq!(parse("# nothing\n"), Appearance::default());
        assert_eq!(parse("accent = 300,0,0\n").accent, None, "out of range");

        let d = std::env::temp_dir().join(format!("ygg-appearance-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let own = d.join("appearance.conf");
        std::fs::write(d.join("kdeglobals"), "[General]\nColorScheme=YggdrasilLight\nAccentColor=1,2,3\n").unwrap();
        assert_eq!(load_from(&own), Appearance::default(), "no file: the defaults, whatever KDE's file says");
        std::fs::write(&own, "style = light\n").unwrap();
        assert_eq!(load_from(&own), Appearance { dark: false, accent: None });
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn icon_pack_line() {
        assert_eq!(parse_icons("style = dark\nicons = Papirus-Dark\n").as_deref(), Some("Papirus-Dark"));
        assert_eq!(parse_icons("style = dark\n"), None);
        assert_eq!(parse_icons("icons = ../etc\n"), None, "a name, not a path");
        assert_eq!(render_icons("Liki", "style = dark\nicons = Old\naccent = 1,2,3\n"), "style = dark\naccent = 1,2,3\nicons = Liki\n");
        assert_eq!(render_icons("Liki", ""), "icons = Liki\n");
        let a = Appearance { dark: true, accent: Some([1, 2, 3]) };
        assert_eq!(render(&a, "icons = Liki\n"), "style = dark\naccent = 1,2,3\nicons = Liki\n", "a style change keeps the pack");
    }
}
