//! Where the login screen draws on several screens.
//!
//! cage (`-m extend`, its default) puts all screens side by side and makes the one full-screen window as big as all of them
//! together: the window's top-left corner is the top-left corner of the space. Drawn as one picture, the sign-in ended up
//! wherever the design put it in that whole space (Split: at the far right). Instead the window is black, and the sign-in is a
//! picture of its own (a subsurface) over the main display only: the one chosen in Settings > Displays when it is connected
//! (the system copy, yggdrasil_conf::display::load_system), else the screen at the top left, the same rule as the panel's.
//! The screens' places come from xdg-output (their logical position and size), their names from wl_output.

use yggdrasil_conf::display::{pick, MainDisplay, Seen};

/// A rectangle in pixels of the compositor's space (a screen) or of the window (the sign-in).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

/// A connected screen: its connector name, maker and model (empty when not told), and where it is.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Screen {
    pub connector: String,
    pub make: String,
    pub model: String,
    pub rect: Rect,
}

/// The part of the window (`window`, its size) where the sign-in goes: the main display's rectangle when the window covers
/// exactly all the screens together (cage), else the whole window (one screen, a compositor that keeps a full-screen window
/// on one of them, or places not told yet).
pub fn sign_in_area(screens: &[Screen], window: (u32, u32), choice: Option<&MainDisplay>) -> Rect {
    let whole = Rect { x: 0, y: 0, w: window.0 as i32, h: window.1 as i32 };
    let known: Vec<&Screen> = screens.iter().filter(|s| s.rect.w > 0 && s.rect.h > 0).collect();
    let (Some(x0), Some(y0)) = (known.iter().map(|s| s.rect.x).min(), known.iter().map(|s| s.rect.y).min()) else {
        return whole;
    };
    let x1 = known.iter().map(|s| s.rect.x.saturating_add(s.rect.w)).max().unwrap_or(x0);
    let y1 = known.iter().map(|s| s.rect.y.saturating_add(s.rect.h)).max().unwrap_or(y0);
    if (x1.saturating_sub(x0), y1.saturating_sub(y0)) != (whole.w, whole.h) {
        return whole;
    }
    let seen: Vec<Seen> =
        known.iter().map(|s| Seen { connector: &s.connector, make: &s.make, model: &s.model, x: s.rect.x, y: s.rect.y }).collect();
    let Some(i) = pick(choice, &seen) else { return whole };
    let r = known[i].rect;
    Rect { x: r.x - x0, y: r.y - y0, ..r }
}

#[cfg(test)]
mod tests {
    use super::*;

    // made-up monitors
    fn screen(connector: &str, make: &str, model: &str, x: i32, y: i32, w: i32, h: i32) -> Screen {
        Screen { connector: connector.into(), make: make.into(), model: model.into(), rect: Rect { x, y, w, h } }
    }
    fn wide(x: i32) -> Screen {
        screen("DP-1", "Example Display", "UW34", x, 0, 3440, 1440)
    }
    fn tall(x: i32) -> Screen {
        screen("DP-3", "Sample Screens", "Tall27", x, 0, 2560, 2880)
    }
    fn small(x: i32, y: i32) -> Screen {
        screen("HDMI-A-1", "Demo Panels", "Mini16", x, y, 1920, 1080)
    }
    fn choice_of(s: &Screen) -> MainDisplay {
        MainDisplay { make: s.make.clone(), model: s.model.clone(), connector: s.connector.clone() }
    }
    fn overlaps(a: Rect, b: Rect) -> bool {
        a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h
    }
    /// The window cage gives: all the screens together.
    fn window(s: &[Screen]) -> ((u32, u32), i32, i32) {
        let x0 = s.iter().map(|s| s.rect.x).min().unwrap();
        let y0 = s.iter().map(|s| s.rect.y).min().unwrap();
        let x1 = s.iter().map(|s| s.rect.x + s.rect.w).max().unwrap();
        let y1 = s.iter().map(|s| s.rect.y + s.rect.h).max().unwrap();
        (((x1 - x0) as u32, (y1 - y0) as u32), x0, y0)
    }
    /// The sign-in is exactly on the main screen, and every other screen is left black: the sign-in touches none of them.
    fn check(screens: &[Screen], choice: Option<&MainDisplay>, main: usize) {
        let (win, x0, y0) = window(screens);
        let a = sign_in_area(screens, win, choice);
        let m = screens[main].rect;
        assert_eq!(a, Rect { x: m.x - x0, y: m.y - y0, ..m }, "on screen {main}");
        for (i, s) in screens.iter().enumerate().filter(|(i, _)| *i != main) {
            let r = Rect { x: s.rect.x - x0, y: s.rect.y - y0, ..s.rect };
            assert!(r.x >= 0 && r.y >= 0 && r.x + r.w <= win.0 as i32 && r.y + r.h <= win.1 as i32, "screen {i} is in the black window");
            assert!(!overlaps(a, r), "the sign-in reaches screen {i}");
        }
    }

    #[test]
    fn without_a_choice_the_sign_in_is_on_the_top_left_screen() {
        // the case that put it at the far right: a wide screen and a tall one to its right
        check(&[wide(0), tall(3440)], None, 0);
        // whatever order the compositor lists them in
        check(&[tall(3440), wide(0)], None, 1);
        check(&[small(0, 0), wide(1920), tall(5360)], None, 0);
        // the same left edge: the higher one
        check(&[small(0, 1440), wide(0)], None, 1);
    }

    #[test]
    fn the_chosen_screen_gets_the_sign_in() {
        let three = [small(0, 0), wide(1920), tall(5360)];
        for main in 0..3 {
            check(&three, Some(&choice_of(&three[main])), main);
        }
        // to the left of the first screen and higher up: the window starts there
        let odd = [wide(0), tall(-2560), small(3440, -400)];
        check(&odd, Some(&choice_of(&odd[1])), 1);
        check(&odd, Some(&choice_of(&odd[2])), 2);
        check(&odd, None, 1);
        // by connector alone
        check(&three, Some(&MainDisplay { connector: "DP-3".into(), ..Default::default() }), 2);
    }

    #[test]
    fn a_choice_not_connected_falls_back_to_the_top_left() {
        let gone = MainDisplay { make: "Unplugged Co".into(), model: "Away".into(), connector: "DP-2".into() };
        check(&[tall(3440), wide(0)], Some(&gone), 1);
        // its make and model on another connector is still it; another monitor on its connector is not
        let moved = MainDisplay { connector: "HDMI-A-2".into(), ..choice_of(&tall(0)) };
        check(&[wide(0), tall(3440)], Some(&moved), 1);
        let other = MainDisplay { make: "Unplugged Co".into(), ..choice_of(&tall(0)) };
        check(&[wide(0), tall(3440)], Some(&other), 0);
    }

    #[test]
    fn one_screen_or_unknown_places_use_the_whole_window() {
        let whole = Rect { x: 0, y: 0, w: 3440, h: 1440 };
        assert_eq!(sign_in_area(&[wide(0)], (3440, 1440), None), whole);
        assert_eq!(sign_in_area(&[], (3440, 1440), None), whole, "no screens told yet");
        let unplaced = [Screen { rect: Rect::default(), ..wide(0) }, Screen { rect: Rect::default(), ..tall(0) }];
        assert_eq!(sign_in_area(&unplaced, (3440, 1440), None), whole, "no xdg-output");
        // a compositor that keeps the full-screen window on one screen: the window is that screen, not all of them
        assert_eq!(sign_in_area(&[wide(0), tall(3440)], (3440, 1440), Some(&choice_of(&tall(0)))), whole);
        // a mirror: both at the same place show the sign-in
        assert_eq!(sign_in_area(&[wide(0), Screen { connector: "DP-2".into(), ..wide(0) }], (3440, 1440), None), whole);
    }
}
