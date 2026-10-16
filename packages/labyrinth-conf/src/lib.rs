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

/// How the start menu looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartTheme {
    /// Search, pinned apps in a grid, every app A to Z (like Windows 11). The default.
    Windows,
    /// Narrow: favourites, then your apps in folders by kind, and a power button.
    Folders,
}

/// Which screen edge the dock sits on (left and right are vertical).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DockPos {
    Bottom,
    Left,
    Right,
}

impl DockPos {
    pub fn vertical(self) -> bool {
        self != DockPos::Bottom
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PanelConf {
    /// The screen edge the dock sits on.
    pub dock_pos: DockPos,
    /// Dock icon size in pixels (the dock grows around it).
    pub dock_icon: u32,
    /// How far the dock floats off the screen edge, in pixels (0 = right on the edge).
    pub dock_gap: u32,
    pub start_theme: StartTheme,
    /// The clock shows 24 hours (with seconds in the calendar); off shows AM/PM.
    pub clock_24h: bool,
    /// Dock background opacity in percent; 0 hides the shelf and leaves just the icons.
    pub dock_opacity: u32,
    pub dock_color: ColorMode,
    pub bar_opacity: u32,
    pub bar_color: ColorMode,
    /// Fylgja (the notch) keeps what was copied, in memory only, so it can be pasted again.
    pub clipboard_history: bool,
    /// Minos's search looks inside the folders below the one you are in, not just that folder.
    pub search_subfolders: bool,
    /// Minos lists hidden files and folders (the ones starting with a dot).
    pub show_hidden: bool,
    /// Developer options are on (Settings > About, the logo clicked five times): extra, more technical settings show.
    pub developer: bool,
}

impl Default for PanelConf {
    fn default() -> Self {
        PanelConf { dock_pos: DockPos::Bottom, dock_icon: 44, dock_gap: 4, start_theme: StartTheme::Windows, clock_24h: true, dock_opacity: 69, dock_color: ColorMode::Theme, bar_opacity: 69, bar_color: ColorMode::Theme, clipboard_history: true, search_subfolders: true, show_hidden: false, developer: false }
    }
}

pub const DOCK_ICON_MIN: u32 = 32;
pub const DOCK_ICON_MAX: u32 = 72;
pub const DOCK_GAP_MAX: u32 = 40;

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
                "dock_pos" => {
                    c.dock_pos = match v {
                        "left" => DockPos::Left,
                        "right" => DockPos::Right,
                        "bottom" => DockPos::Bottom,
                        _ => c.dock_pos,
                    }
                }
                "dock_icon" => c.dock_icon = num.unwrap_or(c.dock_icon).clamp(DOCK_ICON_MIN, DOCK_ICON_MAX),
                "start_theme" => c.start_theme = if v == "folders" { StartTheme::Folders } else if v == "windows" { StartTheme::Windows } else { c.start_theme },
                "clock_24h" => c.clock_24h = !matches!(v, "no" | "false" | "off" | "0"),
                "dock_gap" => c.dock_gap = num.unwrap_or(c.dock_gap).min(DOCK_GAP_MAX),
                "dock_opacity" => c.dock_opacity = num.unwrap_or(c.dock_opacity).min(100),
                "dock_color" => c.dock_color = mode(v).unwrap_or(c.dock_color),
                "bar_opacity" => c.bar_opacity = num.unwrap_or(c.bar_opacity).min(100),
                "bar_color" => c.bar_color = mode(v).unwrap_or(c.bar_color),
                "clipboard_history" => c.clipboard_history = !matches!(v, "no" | "false" | "off" | "0"),
                "search_subfolders" => c.search_subfolders = !matches!(v, "no" | "false" | "off" | "0"),
                "show_hidden" => c.show_hidden = matches!(v, "yes" | "true" | "on" | "1"),
                "developer" => c.developer = matches!(v, "yes" | "true" | "on" | "1"),
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        format!(
            "# How the top bar and dock look. Edited by Labyrinth Settings (Appearance).\n\
             dock_pos = {}\ndock_icon = {}\ndock_gap = {}\nclock_24h = {}\nstart_theme = {}\ndock_opacity = {}\ndock_color = {}\nbar_opacity = {}\nbar_color = {}\nclipboard_history = {}\nsearch_subfolders = {}\nshow_hidden = {}\ndeveloper = {}\n",
            match self.dock_pos {
                DockPos::Bottom => "bottom",
                DockPos::Left => "left",
                DockPos::Right => "right",
            },
            self.dock_icon,
            self.dock_gap,
            if self.clock_24h { "yes" } else { "no" },
            if self.start_theme == StartTheme::Folders { "folders" } else { "windows" },
            self.dock_opacity,
            mode_name(self.dock_color),
            self.bar_opacity,
            mode_name(self.bar_color),
            if self.clipboard_history { "yes" } else { "no" },
            if self.search_subfolders { "yes" } else { "no" },
            if self.show_hidden { "yes" } else { "no" },
            if self.developer { "yes" } else { "no" }
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
        let c = PanelConf { dock_pos: DockPos::Right, dock_icon: 60, dock_gap: 0, start_theme: StartTheme::Folders, clock_24h: false, dock_opacity: 0, dock_color: ColorMode::Neutral, bar_opacity: 100, bar_color: ColorMode::Neutral, clipboard_history: false, search_subfolders: false, show_hidden: true, developer: true };
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
        assert_eq!(PanelConf::parse("dock_gap = 900").dock_gap, DOCK_GAP_MAX);
        assert_eq!(PanelConf::parse("dock_gap = 0").dock_gap, 0);
        assert_eq!(PanelConf::parse("start_theme = folders").start_theme, StartTheme::Folders);
        assert_eq!(PanelConf::parse("start_theme = nonsense").start_theme, StartTheme::Windows);
        assert!(PanelConf::parse("").clock_24h && !PanelConf::parse("clock_24h = no").clock_24h);
        assert!(PanelConf::parse("").clipboard_history && !PanelConf::parse("clipboard_history = no").clipboard_history);
        assert!(PanelConf::parse("").search_subfolders && !PanelConf::parse("search_subfolders = no").search_subfolders);
        assert_eq!(PanelConf::parse("dock_pos = left").dock_pos, DockPos::Left);
        assert_eq!(PanelConf::parse("dock_pos = sideways").dock_pos, DockPos::Bottom);
    }
}
