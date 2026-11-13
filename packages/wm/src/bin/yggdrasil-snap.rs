//! yggdrasil-snap left|right|up|down|top|bottom: put the focused window on that half of its screen, leaving room for the panel
//! and dock (their reserved space). Windows float in Yggdrasil's session, so this is how Super+Left/Right work, as on Windows.
//!
//!   yggdrasil-snap left|right     a half of the screen; on a tiling screen (Settings > Displays), move the focus that way instead
//!   yggdrasil-snap up|down        maximize (again: restore) / minimize; on a tiling screen, move the focus
//!   yggdrasil-snap top|bottom     the top or bottom half
//!
//! On whichever compositor runs (yggdrasil-wm). Tiling screens are Hyprland's (the `tiling` file); Yggdrasil's own compositor
//! tiles by workspace (workspaces.conf) and snaps there as on a floating screen.

use yggdrasil_wm::{Desk, Rect, Screen};

/// The free room of a screen: the screen less the strips the panels reserve, in logical pixels (whole ones, cut down).
fn free_room(m: &Screen) -> Rect {
    let s = m.scale.filter(|s| *s != 0.0).unwrap_or(1.0);
    let [l, t, r, b] = m.reserved;
    let (w, h) = if m.transform % 2 == 1 { (m.height, m.width) } else { (m.width, m.height) };
    let (l, t, r, b) = (l as f64, t as f64, r as f64, b as f64);
    ((m.x as f64 + l) as i32, (m.y as f64 + t) as i32, (w as f64 / s - l - r) as i32, (h as f64 / s - t - b) as i32)
}

/// Where the window goes for `side` in the free room.
fn half((x, y, w, h): Rect, side: &str) -> Option<Rect> {
    Some(match side {
        "left" => (x, y, w / 2, h),
        "right" => (x + w - w / 2, y, w / 2, h),
        "top" => (x, y, w, h / 2),
        "bottom" => (x, y + h - h / 2, w, h / 2),
        _ => return None,
    })
}

fn main() {
    let side = std::env::args().nth(1).unwrap_or_default();
    if !["left", "right", "up", "down", "top", "bottom"].contains(&side.as_str()) {
        eprintln!("usage: yggdrasil-snap left|right|up|down|top|bottom");
        std::process::exit(2);
    }
    std::process::exit(run(&side));
}

fn run(side: &str) -> i32 {
    let Some(desk) = Desk::detect() else { return 1 };
    let focused = desk.focused_screen();
    if desk.is_hyprland() && ["left", "right", "up", "down"].contains(&side) {
        if let Some(m) = &focused {
            if yggdrasil_conf::tiling::outputs().contains(&m.name) {
                desk.focus_direction(side.chars().next().unwrap_or('l'));
                return 0;
            }
        }
    }
    let done = match side {
        "up" => desk.maximize_active(),
        "down" => desk.minimize_active(),
        _ => match focused.map(|m| free_room(&m)).and_then(|room| half(room, side)) {
            Some(r) => desk.snap_active(r),
            None => false,
        },
    };
    if done {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(width: u32, height: u32, scale: Option<f64>, transform: u32, reserved: [i32; 4]) -> Screen {
        Screen {
            id: Some(0),
            name: "DP-1".into(),
            make: String::new(),
            model: String::new(),
            x: 2560,
            y: 0,
            width,
            height,
            scale,
            transform,
            reserved,
            focused: true,
            disabled: false,
            active_workspace: Some(1),
            special_workspace: None,
            special_workspace_id: None,
            refresh: None,
            modes: vec![],
        }
    }

    /// The same numbers the shell script's Python worked out (int() cuts towards zero; bash halves with whole numbers).
    #[test]
    fn halves_of_the_free_room_as_before() {
        let m = screen(1920, 1080, Some(1.25), 0, [0, 28, 0, 70]);
        let room = free_room(&m);
        assert_eq!(room, (2560, 28, 1536, 766));
        assert_eq!(half(room, "left"), Some((2560, 28, 768, 766)));
        assert_eq!(half(room, "right"), Some((2560 + 768, 28, 768, 766)));
        assert_eq!(half(room, "top"), Some((2560, 28, 1536, 383)));
        assert_eq!(half(room, "bottom"), Some((2560, 28 + 383, 1536, 383)));
        // an odd width: the right half starts after the left one's last pixel
        assert_eq!(half((0, 0, 1001, 11), "right"), Some((501, 0, 500, 11)));
        assert_eq!(half((0, 0, 1001, 11), "bottom"), Some((0, 6, 1001, 5)));
        // a screen turned sideways, and a scale of 0 or none taken as 1
        assert_eq!(free_room(&screen(2560, 1440, Some(0.0), 1, [0, 28, 0, 0])), (2560, 28, 1440, 2532));
        assert_eq!(free_room(&screen(2560, 1440, None, 0, [0, 0, 0, 0])), (2560, 0, 2560, 1440));
        assert_eq!(half(room, "up"), None);
    }
}
