//! Logos for apps that ship no icon of their own (terminal programs such as lazygit, Docker, Podman): the mark from Liki's brand
//! set (Simple Icons, CC0; the logos stay their owners' trademarks), drawn white on a tile of the brand's colour, the way Liki's
//! "brand" style draws them. Used only when no vendor icon was found, before the icon pack's plain default.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use eframe::egui::Color32;

fn dir() -> PathBuf {
    std::env::var_os("LIKI_BRAND_ICONS").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/usr/share/liki/brand-icons"))
}

fn colors() -> &'static HashMap<String, String> {
    static C: OnceLock<HashMap<String, String>> = OnceLock::new();
    C.get_or_init(|| std::fs::read_to_string(dir().join("colors.json")).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default())
}

fn hex(s: &str) -> Option<Color32> {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).ok()?;
    (s.trim_start_matches('#').len() == 6).then(|| Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8))
}

/// The mark recoloured white: `fill` on the root, which the logo's paths inherit.
pub fn white(svg: &str) -> Option<String> {
    let i = svg.find("<svg")? + 4;
    Some(format!("{} fill=\"#ffffff\"{}", &svg[..i], &svg[i..]))
}

/// A logo for the first name that has one: (its white SVG, the tile colour, a stable name for egui's image cache).
pub fn find(names: &[&str]) -> Option<(String, Color32, String)> {
    static CACHE: Mutex<Option<HashMap<String, Option<(String, Color32)>>>> = Mutex::new(None);
    for n in names {
        let slug = n.to_lowercase();
        if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
            continue;
        }
        let mut g = CACHE.lock().ok()?;
        let m = g.get_or_insert_with(HashMap::new);
        let hit = m
            .entry(slug.clone())
            .or_insert_with(|| {
                let svg = std::fs::read_to_string(dir().join("icons").join(format!("{slug}.svg"))).ok()?;
                let col = colors().get(&slug).and_then(|h| hex(h)).unwrap_or(Color32::from_rgb(60, 60, 70));
                Some((white(&svg)?, col))
            })
            .clone();
        if let Some((svg, col)) = hit {
            return Some((svg, col, format!("bytes://liki-brand-{slug}.svg")));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mark_turns_white_and_the_colour_is_read() {
        assert_eq!(white(r#"<svg role="img" viewBox="0 0 24 24"><path d="M0"/></svg>"#).unwrap(), r##"<svg fill="#ffffff" role="img" viewBox="0 0 24 24"><path d="M0"/></svg>"##);
        assert_eq!(hex("2496ED"), Some(Color32::from_rgb(0x24, 0x96, 0xED)));
        assert_eq!(hex("nope"), None);
    }

    #[test]
    fn the_shipped_set_has_the_terminal_tools() {
        std::env::set_var("LIKI_BRAND_ICONS", concat!(env!("CARGO_MANIFEST_DIR"), "/../liki/share/brand-icons"));
        for n in ["docker", "podman", "git", "github", "claude"] {
            assert!(find(&[n]).is_some(), "{n}");
        }
        assert!(find(&["../../etc/passwd", "no-such-brand"]).is_none());
    }
}
