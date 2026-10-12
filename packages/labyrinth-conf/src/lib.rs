//! ~/.config/labyrinth/panel.conf: how the top bar and the dock look. Labyrinth Settings writes
//! it and labyrinth-panel re-reads it about once a second, so changes show at once. Plain
//! `key = value` lines; unknown keys are ignored and values are clamped to what makes sense, so a
//! hand-edited file can never break the panel.

use std::path::PathBuf;

/// How a panel's colour is chosen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ColorMode {
    /// Tinted with the Labyrinth purple (follows the dark/light style too). The default.
    Theme,
    /// Plain dark or light, following the system style only.
    Neutral,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelConf {
    /// Dock icon size in pixels (the dock grows around it).
    pub dock_icon: u32,
    /// Dock background opacity in percent; 0 hides the shelf and leaves just the icons.
    pub dock_opacity: u32,
    pub dock_color: ColorMode,
    pub bar_opacity: u32,
    pub bar_color: ColorMode,
}

impl Default for PanelConf {
    fn default() -> Self {
        PanelConf { dock_icon: 44, dock_opacity: 69, dock_color: ColorMode::Theme, bar_opacity: 69, bar_color: ColorMode::Theme }
    }
}

pub const DOCK_ICON_MIN: u32 = 32;
pub const DOCK_ICON_MAX: u32 = 72;

fn mode(v: &str) -> Option<ColorMode> {
    match v {
        "theme" => Some(ColorMode::Theme),
        "neutral" => Some(ColorMode::Neutral),
        _ => None,
    }
}

fn mode_name(m: ColorMode) -> &'static str {
    match m {
        ColorMode::Theme => "theme",
        ColorMode::Neutral => "neutral",
    }
}

impl PanelConf {
    pub fn parse(text: &str) -> PanelConf {
        let mut c = PanelConf::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let num = v.trim_end_matches('%').trim().parse::<u32>().ok();
            match k {
                "dock_icon" => c.dock_icon = num.unwrap_or(c.dock_icon).clamp(DOCK_ICON_MIN, DOCK_ICON_MAX),
                "dock_opacity" => c.dock_opacity = num.unwrap_or(c.dock_opacity).min(100),
                "dock_color" => c.dock_color = mode(v).unwrap_or(c.dock_color),
                "bar_opacity" => c.bar_opacity = num.unwrap_or(c.bar_opacity).min(100),
                "bar_color" => c.bar_color = mode(v).unwrap_or(c.bar_color),
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        format!(
            "# How the top bar and dock look. Edited by Labyrinth Settings (Appearance).\n\
             dock_icon = {}\ndock_opacity = {}\ndock_color = {}\nbar_opacity = {}\nbar_color = {}\n",
            self.dock_icon,
            self.dock_opacity,
            mode_name(self.dock_color),
            self.bar_opacity,
            mode_name(self.bar_color)
        )
    }

    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("labyrinth").join("panel.conf"))
    }

    pub fn load() -> PanelConf {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path().ok_or("no config folder")?;
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        // Write beside it and rename, so the panel never reads a half-written file.
        let tmp = p.with_extension("conf.tmp");
        std::fs::write(&tmp, self.to_text()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    /// When the file last changed, for cheap change detection.
    pub fn modified() -> Option<std::time::SystemTime> {
        std::fs::metadata(Self::path()?).ok()?.modified().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_empty_or_missing() {
        assert_eq!(PanelConf::parse(""), PanelConf::default());
        assert_eq!(PanelConf::default().dock_icon, 44);
    }

    #[test]
    fn round_trips() {
        let c = PanelConf { dock_icon: 60, dock_opacity: 0, dock_color: ColorMode::Neutral, bar_opacity: 100, bar_color: ColorMode::Neutral };
        assert_eq!(PanelConf::parse(&c.to_text()), c);
    }

    #[test]
    fn values_are_clamped_and_junk_is_ignored() {
        let c = PanelConf::parse("dock_icon = 500\ndock_opacity = 250\nbar_opacity = 12%\ndock_color = rainbow\nnonsense\nfoo = bar\n# dock_icon = 40\n");
        assert_eq!(c.dock_icon, DOCK_ICON_MAX);
        assert_eq!(c.dock_opacity, 100);
        assert_eq!(c.bar_opacity, 12);
        assert_eq!(c.dock_color, ColorMode::Theme);
        assert_eq!(PanelConf::parse("dock_icon = 4").dock_icon, DOCK_ICON_MIN);
        assert_eq!(PanelConf::parse("dock_icon = abc").dock_icon, 44);
    }
}
