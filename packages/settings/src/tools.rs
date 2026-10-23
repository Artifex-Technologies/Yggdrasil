//! Settings > Tools: applies the tools' settings (labyrinth_conf::tools) to the window manager. While KWin is the window manager,
//! Grab And Move is KWin's window mouse bindings, Window Hopper its "windows of the current application" shortcut, and the mouse
//! utilities are KWin's own effects (track mouse, shake cursor, mouse click). Nothing here starts a program that keeps running.

use labyrinth_conf::tools::{Modifier, ToolsConf};

fn kwin_config(group: &str, key: &str, value: &str) {
    let _ = crate::sys::run("kwriteconfig6", &["--file", "kwinrc", "--group", group, "--key", key, value]);
}

/// Qt key codes for the Window Hopper shortcut: Alt+` and Super+` (Qt::Key_QuoteLeft = 0x60).
const ALT_BACKTICK: i32 = 0x0800_0000 | 0x60;
const META_BACKTICK: i32 = 0x1000_0000 | 0x60;

pub fn apply(c: &ToolsConf) -> Result<(), String> {
    // Grab And Move: the modifier, left drag moves, right drag resizes; "Nothing" switches a button off
    kwin_config("MouseBindings", "CommandAllKey", if c.grab_modifier == Modifier::Alt { "Alt" } else { "Meta" });
    kwin_config("MouseBindings", "CommandAll1", if c.grab_move { "Move" } else { "Nothing" });
    kwin_config("MouseBindings", "CommandAll3", if c.grab_move && c.grab_resize { "Resize" } else { "Nothing" });
    // Mouse utilities: KWin effects, switched in the config and loaded or unloaded at once
    for (effect, on) in [("trackmouse", c.find_mouse), ("shakecursor", c.shake_find), ("mouseclick", c.click_highlight)] {
        kwin_config("Plugins", &format!("{effect}Enabled"), if on { "true" } else { "false" });
        let _ = crate::sys::run("qdbus6", &["org.kde.KWin", "/Effects", if on { "org.kde.kwin.Effects.loadEffect" } else { "org.kde.kwin.Effects.unloadEffect" }, effect]);
    }
    let _ = crate::sys::run("qdbus6", &["org.kde.KWin", "/KWin", "org.kde.KWin.reconfigure"]);
    // Window Hopper: KWin's own action, given its keys or none
    let keys = if c.window_hopper { format!("[{ALT_BACKTICK}, {META_BACKTICK}]") } else { "@ai []".to_string() };
    let action = "['kwin', 'Walk Through Windows of Current Application', 'KWin', 'Walk Through Windows of Current Application']";
    let _ = crate::sys::run("gdbus", &["call", "--session", "--dest", "org.kde.kglobalaccel", "--object-path", "/kglobalaccel",
        "--method", "org.kde.KGlobalAccel.setForeignShortcut", action, &keys]);
    c.save()
}

/// The colours picked lately (newest first), from the Color Picker's history file.
pub fn color_history() -> Vec<String> {
    let path = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
        .map(|d| d.join("labyrinth").join("colors.txt"));
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .map(|t| t.lines().map(|l| l.trim().to_string()).filter(|l| l.starts_with('#') && l.len() == 7).take(24).collect())
        .unwrap_or_default()
}
