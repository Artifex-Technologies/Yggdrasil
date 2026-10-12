//! Colors, after EdenExplorer's dark look: a near-black blue-grey, a purple accent for the active
//! tab and selection, and green / amber / red drive-usage bars. The light palette is a plain
//! counterpart, not a design of its own.

use eframe::egui::{self, Color32, CornerRadius};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub sidebar: Color32,
    pub strip: Color32,
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub tab_active: Color32,
    pub hover: Color32,
    pub select: Color32,
    pub row_alt: Color32,
    pub folder: Color32,
    pub file: Color32,
    pub drive: Color32,
    pub green: Color32,
    pub amber: Color32,
    pub red: Color32,
}

/// The default palette (Minos's own purple).
pub fn palette(dark: bool) -> Palette {
    palette_for(dark, None)
}

/// The palette with a different accent color: the accent itself, plus the active-tab and selection
/// colors mixed from it so they stay readable on the background.
pub fn palette_for(dark: bool, accent: Option<[u8; 3]>) -> Palette {
    let mut p = base_palette(dark);
    if let Some([r, g, b]) = accent {
        let a = Color32::from_rgb(r, g, b);
        p.accent = a;
        p.tab_active = mix(p.bg, a, if dark { 0.55 } else { 0.40 });
        p.select = mix(p.bg, a, if dark { 0.28 } else { 0.30 });
    }
    p
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// The accent color from KDE's settings text (`[General]` then `AccentColor=r,g,b`).
pub fn parse_kde_accent(text: &str) -> Option<[u8; 3]> {
    let mut in_general = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_general = line == "[General]";
            continue;
        }
        if !in_general {
            continue;
        }
        if let Some(v) = line.strip_prefix("AccentColor=") {
            let parts: Vec<u8> = v.trim().trim_matches('"').split(',').filter_map(|x| x.trim().parse().ok()).collect();
            if parts.len() == 3 {
                return Some([parts[0], parts[1], parts[2]]);
            }
        }
    }
    None
}

/// The system's accent color, if there is one to read (KDE Plasma's `kdeglobals`). None on other
/// desktops, where Minos keeps its own purple.
pub fn system_accent() -> Option<[u8; 3]> {
    let dir = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".config")))?;
    parse_kde_accent(&std::fs::read_to_string(dir.join("kdeglobals")).ok()?)
}

fn base_palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color32::from_rgb(16, 17, 22),
            sidebar: Color32::from_rgb(14, 15, 20),
            strip: Color32::from_rgb(11, 12, 16),
            field: Color32::from_rgb(24, 25, 32),
            border: Color32::from_rgb(38, 40, 52),
            text: Color32::from_rgb(232, 233, 238),
            dim: Color32::from_rgb(140, 143, 157),
            accent: Color32::from_rgb(124, 84, 214),
            tab_active: Color32::from_rgb(72, 44, 128),
            hover: Color32::from_rgb(32, 34, 44),
            select: Color32::from_rgb(52, 40, 92),
            row_alt: Color32::from_rgb(19, 20, 26),
            folder: Color32::from_rgb(240, 196, 64),
            file: Color32::from_rgb(190, 194, 206),
            drive: Color32::from_rgb(176, 180, 194),
            green: Color32::from_rgb(58, 158, 94),
            amber: Color32::from_rgb(201, 146, 58),
            red: Color32::from_rgb(180, 58, 69),
        }
    } else {
        Palette {
            bg: Color32::from_rgb(250, 250, 252),
            sidebar: Color32::from_rgb(243, 243, 247),
            strip: Color32::from_rgb(232, 232, 238),
            field: Color32::from_rgb(255, 255, 255),
            border: Color32::from_rgb(214, 214, 224),
            text: Color32::from_rgb(28, 29, 36),
            dim: Color32::from_rgb(110, 113, 128),
            accent: Color32::from_rgb(110, 70, 200),
            tab_active: Color32::from_rgb(196, 176, 238),
            hover: Color32::from_rgb(232, 232, 240),
            select: Color32::from_rgb(214, 200, 244),
            row_alt: Color32::from_rgb(244, 244, 248),
            folder: Color32::from_rgb(232, 178, 40),
            file: Color32::from_rgb(120, 126, 142),
            drive: Color32::from_rgb(110, 116, 132),
            green: Color32::from_rgb(46, 140, 80),
            amber: Color32::from_rgb(196, 130, 30),
            red: Color32::from_rgb(190, 50, 60),
        }
    }
}

/// Usage bar color: green below 75%, amber below 95%, red above (Eden's 68% green, 92% amber, 100% red).
pub fn usage_color(p: &Palette, fraction: f32) -> Color32 {
    if fraction < 0.75 {
        p.green
    } else if fraction < 0.95 {
        p.amber
    } else {
        p.red
    }
}

pub fn apply(ctx: &egui::Context, dark: bool, accent: Option<[u8; 3]>) {
    let p = palette_for(dark, accent);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = p.bg;
        v.window_fill = p.field;
        v.extreme_bg_color = p.field;
        v.faint_bg_color = p.row_alt;
        v.window_stroke = egui::Stroke::new(1.0, p.border);
        v.selection.bg_fill = p.select;
        v.selection.stroke = egui::Stroke::new(1.0, p.accent);
        v.override_text_color = Some(p.text);
        let r = CornerRadius::same(7);
        for w in [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
        v.widgets.inactive.weak_bg_fill = p.field;
        v.widgets.inactive.bg_fill = p.field;
        v.widgets.hovered.weak_bg_fill = p.hover;
        v.widgets.hovered.bg_fill = p.hover;
        v.widgets.active.weak_bg_fill = p.select;
        v.widgets.active.bg_fill = p.select;
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_kde_accent() {
        let text = "[General]\nColorScheme=WhiteSurDark\nAccentColor=124,84,214\n\n[KDE]\nwidgetStyle=kvantum\n";
        assert_eq!(parse_kde_accent(text), Some([124, 84, 214]));
    }

    #[test]
    fn ignores_an_accent_in_the_wrong_section() {
        assert_eq!(parse_kde_accent("[Other]\nAccentColor=1,2,3\n"), None);
    }

    #[test]
    fn rejects_malformed_values() {
        assert_eq!(parse_kde_accent("[General]\nAccentColor=12,34\n"), None);
        assert_eq!(parse_kde_accent("[General]\nAccentColor=a,b,c\n"), None);
        assert_eq!(parse_kde_accent("[General]\nAccentColor=300,0,0\n"), None);
        assert_eq!(parse_kde_accent(""), None);
    }

    #[test]
    fn tolerates_quotes_and_spaces() {
        assert_eq!(parse_kde_accent("[General]\nAccentColor=\"10, 20, 30\"\n"), Some([10, 20, 30]));
    }

    #[test]
    fn no_accent_gives_the_default_palette() {
        let a = palette(true);
        let b = palette_for(true, None);
        assert_eq!((a.accent, a.tab_active, a.select), (b.accent, b.tab_active, b.select));
    }

    #[test]
    fn a_system_accent_drives_the_highlights_and_stays_readable() {
        let p = palette_for(true, Some([40, 160, 90]));
        assert_eq!(p.accent, Color32::from_rgb(40, 160, 90));
        // The selection and active tab take the accent's hue but stay darker than the accent itself.
        assert!(p.select.g() > p.select.r() && p.select.g() > p.select.b());
        assert!(p.tab_active.g() > p.tab_active.r());
        assert!(p.select.g() < p.accent.g());
        // The background and text are untouched.
        assert_eq!(p.bg, palette(true).bg);
        assert_eq!(p.text, palette(true).text);
    }

    #[test]
    fn the_light_palette_gets_lighter_mixes() {
        let p = palette_for(false, Some([110, 70, 200]));
        assert!(p.select.r() > 190, "light selection should be pale, got {:?}", p.select);
    }
}
