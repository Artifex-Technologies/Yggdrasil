//! Windows come back where they were, as in the Hyprland session (`packages/session/hyprland/yggdrasil.lua`, "window-places"):
//! when a program's last window closes, its place and size are kept, one line per program in
//! `~/.local/state/yggdrasil/window-places`:
//!
//!     org.example.Editor<TAB>120<TAB>80<TAB>900<TAB>640
//!
//! (app id or X11 class, x, y, width, height in the layout's logical pixels), and its next window opens there when the middle of
//! that spot is still on a screen (kept clear of the bar and the dock after). Only floating windows count: not tiled, maximized,
//! full screen or minimized, at least 120 x 80; a dialog or a second window of a program that has one open is left where the
//! program puts it. The same file and lines as the Hyprland session's, so a place kept on one compositor is found on the other.

use std::collections::BTreeMap;
use std::path::PathBuf;

use smithay::utils::{Logical, Rectangle};

pub type Rect = Rectangle<i32, Logical>;

pub fn path() -> PathBuf {
    yggdrasil_conf::dirs::state().join("window-places")
}

/// The lines of the file (the Lua pattern `^(%S+)\t(%-?%d+)\t(%-?%d+)\t(%d+)\t(%d+)$`); anything else is skipped.
pub fn parse(text: &str) -> BTreeMap<String, Rect> {
    let mut out = BTreeMap::new();
    for line in text.lines() {
        let p: Vec<&str> = line.split('\t').collect();
        let [class, x, y, w, h] = p.as_slice() else { continue };
        let digits = |s: &str, sign: bool| {
            let d = if sign { s.strip_prefix('-').unwrap_or(s) } else { s };
            !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())
        };
        if class.is_empty()
            || class.chars().any(char::is_whitespace)
            || !digits(x, true)
            || !digits(y, true)
            || !digits(w, false)
            || !digits(h, false)
        {
            continue;
        }
        let (Ok(x), Ok(y), Ok(w), Ok(h)) = (x.parse::<i32>(), y.parse::<i32>(), w.parse::<i32>(), h.parse::<i32>()) else { continue };
        out.insert(class.to_string(), Rectangle::new((x, y).into(), (w, h).into()));
    }
    out
}

pub fn format(places: &BTreeMap<String, Rect>) -> String {
    places.iter().map(|(c, r)| format!("{c}\t{}\t{}\t{}\t{}\n", r.loc.x, r.loc.y, r.size.w, r.size.h)).collect()
}

pub fn load() -> BTreeMap<String, Rect> {
    std::fs::read_to_string(path()).map(|t| parse(&t)).unwrap_or_default()
}

/// Write the whole list (a file next to it first, then renamed over it, as the session does).
pub fn save(places: &BTreeMap<String, Rect>) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_file_name("window-places.tmp");
    std::fs::write(&tmp, format(places))?;
    std::fs::rename(tmp, p)
}

/// Big enough to be kept (the session skips anything under 120 x 80).
pub fn worth_keeping(r: Rect) -> bool {
    r.size.w >= 120 && r.size.h >= 80
}

/// A class that can be a key of the file: not empty, no white space (it would break the line).
pub fn usable_class(c: &str) -> bool {
    !c.is_empty() && !c.chars().any(char::is_whitespace)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    #[test]
    fn the_sessions_lines_are_read_and_written_alike() {
        // as the Lua writes them: "%s\t%d\t%d\t%d\t%d\n"
        let text = "code\t-1720\t40\t1600\t900\norg.example.Editor\t120\t80\t900\t640\n";
        let p = parse(text);
        assert_eq!(p.get("code"), Some(&r(-1720, 40, 1600, 900)), "a place on a screen to the left");
        assert_eq!(p.get("org.example.Editor"), Some(&r(120, 80, 900, 640)));
        assert_eq!(format(&p), text, "written back the same (sorted by class)");
        assert!(parse("a b\t1\t2\t3\t4\nx\t1\t2\t-3\t4\ny\t1\t2\t3\nz\t1.5\t2\t3\t4\n").is_empty(), "none of these is a line");
        assert!(worth_keeping(r(0, 0, 120, 80)) && !worth_keeping(r(0, 0, 119, 600)) && !worth_keeping(r(0, 0, 600, 79)));
        assert!(usable_class("steam_app_47780") && !usable_class("") && !usable_class("two words"));
    }
}
