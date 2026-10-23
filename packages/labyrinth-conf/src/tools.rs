//! Tools: small built-in helpers in the spirit of PowerToys, rewritten for this desktop and kept light (each one is a setting of
//! the window manager, or a tiny helper started only when its key is pressed; nothing runs in the background for them).
//! `~/.config/labyrinth/tools.conf`, written by Settings > Tools, read by the panel (hotkeys) and Minos (File Locksmith).
//! While KWin is the window manager the window tools are KWin's own settings; the compositor reads the same file when it takes over.

use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Modifier {
    Super,
    Alt,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorFormat {
    Hex,
    Rgb,
    Hsl,
}

impl ColorFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            ColorFormat::Hex => "hex",
            ColorFormat::Rgb => "rgb",
            ColorFormat::Hsl => "hsl",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolsConf {
    /// Grab And Move: hold the modifier and drag anywhere in a window to move it (right button: resize).
    pub grab_move: bool,
    pub grab_modifier: Modifier,
    pub grab_resize: bool,
    /// Window Hopper: Alt+` (and Super+`) cycles through the windows of the app in front.
    pub window_hopper: bool,
    /// Mouse utilities: a spotlight on the pointer while Ctrl+Super are held; shake the mouse to make the pointer big; rings on clicks.
    pub find_mouse: bool,
    pub shake_find: bool,
    pub click_highlight: bool,
    /// Color Picker: Super+Shift+C picks a colour anywhere on screen and copies it.
    pub color_picker: bool,
    pub color_format: ColorFormat,
    /// File Locksmith: "What is using this?" in Minos's menu for a file or folder.
    pub file_locksmith: bool,
}

impl Default for ToolsConf {
    fn default() -> Self {
        ToolsConf {
            grab_move: true,
            grab_modifier: Modifier::Super,
            grab_resize: true,
            window_hopper: true,
            find_mouse: true,
            shake_find: true,
            click_highlight: false,
            color_picker: true,
            color_format: ColorFormat::Hex,
            file_locksmith: true,
        }
    }
}

fn yes(v: &str) -> bool {
    matches!(v, "yes" | "true" | "on" | "1")
}

fn yn(b: bool) -> &'static str {
    if b { "yes" } else { "no" }
}

impl ToolsConf {
    pub fn parse(text: &str) -> ToolsConf {
        let mut c = ToolsConf::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "grab_move" => c.grab_move = yes(v),
                "grab_modifier" => c.grab_modifier = if v == "alt" { Modifier::Alt } else { Modifier::Super },
                "grab_resize" => c.grab_resize = yes(v),
                "window_hopper" => c.window_hopper = yes(v),
                "find_mouse" => c.find_mouse = yes(v),
                "shake_find" => c.shake_find = yes(v),
                "click_highlight" => c.click_highlight = yes(v),
                "color_picker" => c.color_picker = yes(v),
                "color_format" => {
                    c.color_format = match v {
                        "rgb" => ColorFormat::Rgb,
                        "hsl" => ColorFormat::Hsl,
                        _ => ColorFormat::Hex,
                    }
                }
                "file_locksmith" => c.file_locksmith = yes(v),
                _ => {}
            }
        }
        c
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Written by Labyrinth Settings > Tools. Read by the panel (hotkeys) and Minos.\n\
             grab_move = {}\ngrab_modifier = {}\ngrab_resize = {}\nwindow_hopper = {}\nfind_mouse = {}\nshake_find = {}\nclick_highlight = {}\n\
             color_picker = {}\ncolor_format = {}\nfile_locksmith = {}\n",
            yn(self.grab_move),
            if self.grab_modifier == Modifier::Alt { "alt" } else { "super" },
            yn(self.grab_resize),
            yn(self.window_hopper),
            yn(self.find_mouse),
            yn(self.shake_find),
            yn(self.click_highlight),
            yn(self.color_picker),
            self.color_format.as_str(),
            yn(self.file_locksmith)
        )
    }

    pub fn path() -> Option<PathBuf> {
        let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("labyrinth").join("tools.conf"))
    }

    pub fn load() -> ToolsConf {
        Self::path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| Self::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> Result<(), String> {
        let p = Self::path().ok_or("no config folder")?;
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
        let tmp = p.with_extension("conf.tmp");
        std::fs::write(&tmp, self.to_text()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &p).map_err(|e| e.to_string())
    }

    pub fn modified() -> Option<std::time::SystemTime> {
        std::fs::metadata(Self::path()?).ok()?.modified().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_round_trip() {
        assert_eq!(ToolsConf::parse(""), ToolsConf::default());
        let mut c = ToolsConf::default();
        c.grab_modifier = Modifier::Alt;
        c.color_format = ColorFormat::Hsl;
        c.click_highlight = true;
        c.window_hopper = false;
        assert_eq!(ToolsConf::parse(&c.to_text()), c);
        assert_eq!(ToolsConf::parse("grab_move = no # off\nnonsense\n").grab_move, false);
    }
}
