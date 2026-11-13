//! The main display: the screen the dock lives on, where notifications pop up and where the start menu, the on-screen volume and
//! the other panels open when nothing says otherwise. The top bar and the wallpaper are on every screen either way.
//!
//! Neither Hyprland nor Yggdrasil's compositor has a main screen of its own, so the screen at the top left of the space is the
//! main one until another is chosen in Settings > Displays ("Make main"). The choice is kept in `~/.config/yggdrasil/display.conf`
//! by the monitor's maker and model, so it follows the monitor to another port; the connector name it was on only decides between
//! two monitors of the same make and model. When the chosen monitor is not connected, the top-left screen is the main one again.
//!
//!   # The main display (Settings > Displays).
//!   main_make = Example Display
//!   main_model = UW34
//!   main_connector = DP-1
//!
//! No file (or an empty choice): the top-left screen. The panel watches the file and the screens and moves the dock at once.

use std::path::PathBuf;

/// The chosen main display, as the monitor tells its maker and model, and the connector it was on when chosen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MainDisplay {
    pub make: String,
    pub model: String,
    pub connector: String,
}

/// A connected screen as the choice is matched against it: its connector name, maker and model (empty when the compositor did
/// not say), and its top-left corner in the space of all screens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seen<'a> {
    pub connector: &'a str,
    pub make: &'a str,
    pub model: &'a str,
    pub x: i32,
    pub y: i32,
}

/// The file's name in the config folder (the panel watches the folder for it).
pub const FILE: &str = "display.conf";

pub fn path() -> PathBuf {
    crate::dirs::config().join(FILE)
}

/// One line of text for the file: no line breaks or other control characters, no surrounding spaces.
fn clean(v: &str) -> String {
    v.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string()
}

impl MainDisplay {
    /// Nothing to match on: no choice.
    pub fn is_empty(&self) -> bool {
        self.make.is_empty() && self.model.is_empty() && self.connector.is_empty()
    }

    /// The choice for a screen as it is seen now.
    pub fn of(s: &Seen) -> MainDisplay {
        MainDisplay { make: clean(s.make), model: clean(s.model), connector: clean(s.connector) }
    }

    pub fn parse(text: &str) -> Option<MainDisplay> {
        let mut c = MainDisplay::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = clean(v);
            match k.trim() {
                "main_make" => c.make = v,
                "main_model" => c.model = v,
                "main_connector" => c.connector = v,
                _ => {}
            }
        }
        (!c.is_empty()).then_some(c)
    }

    pub fn to_text(&self) -> String {
        format!(
            "# The main display (Settings > Displays): the dock, notifications and the start menu are there.\n\
             main_make = {}\nmain_model = {}\nmain_connector = {}\n",
            clean(&self.make),
            clean(&self.model),
            clean(&self.connector)
        )
    }
}

/// The saved choice; None when there is none (the top-left screen is the main one).
pub fn load() -> Option<MainDisplay> {
    std::fs::read_to_string(path()).ok().and_then(|t| MainDisplay::parse(&t))
}

/// Keeps the choice (written beside and renamed, so the panel never reads half a file).
pub fn save(c: &MainDisplay) -> std::io::Result<()> {
    let p = path();
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("conf.tmp");
    std::fs::write(&tmp, c.to_text())?;
    std::fs::rename(tmp, p)
}

/// Which of the connected screens is the main one (its place in `screens`): the chosen monitor when it is connected (`chosen`),
/// else the one at the top left of the space (left first, then up). None only when there is no screen.
pub fn pick(choice: Option<&MainDisplay>, screens: &[Seen]) -> Option<usize> {
    choice.and_then(|c| chosen(c, screens)).or_else(|| top_left(screens, 0..screens.len()))
}

/// The chosen monitor among the connected screens, when it is one of them: by maker and model, and the connector it was on
/// decides between two alike (else the one further left). A choice saved without maker and model (a compositor that did not
/// say) is matched by its connector.
pub fn chosen(c: &MainDisplay, screens: &[Seen]) -> Option<usize> {
    if c.is_empty() {
        return None;
    }
    let by_name = c.make.is_empty() && c.model.is_empty();
    let alike: Vec<usize> = (0..screens.len())
        .filter(|&i| {
            let s = &screens[i];
            if by_name {
                s.connector == c.connector
            } else {
                s.make.trim() == c.make && s.model.trim() == c.model
            }
        })
        .collect();
    alike.iter().copied().find(|&i| screens[i].connector == c.connector).or_else(|| top_left(screens, alike.iter().copied()))
}

fn top_left(screens: &[Seen], among: impl Iterator<Item = usize>) -> Option<usize> {
    among.min_by_key(|&i| (screens[i].x, screens[i].y, i))
}

#[cfg(test)]
mod tests {
    use super::*;

    // made-up monitors: a wide one and a tall one
    fn wide(connector: &str, x: i32) -> Seen<'_> {
        Seen { connector, make: "Example Display", model: "UW34", x, y: 0 }
    }
    fn tall(connector: &str, x: i32) -> Seen<'_> {
        Seen { connector, make: "Sample Screens", model: "Tall27", x, y: 0 }
    }

    #[test]
    fn with_no_choice_the_top_left_screen_is_main() {
        assert_eq!(pick(None, &[tall("DP-3", 3440), wide("DP-1", 0)]), Some(1));
        let above = Seen { y: -1440, ..wide("DP-2", 0) };
        assert_eq!(pick(None, &[wide("DP-1", 0), above]), Some(1), "the same x: the higher one");
        assert_eq!(pick(None, &[Seen { x: -1920, ..tall("HDMI-A-1", 0) }, wide("DP-1", 0)]), Some(0), "left first");
        assert_eq!(pick(None, &[]), None);
        assert_eq!(pick(Some(&MainDisplay::default()), &[tall("DP-3", 3440), wide("DP-1", 0)]), Some(1), "an empty choice is none");
    }

    #[test]
    fn the_choice_is_kept() {
        let screens = [wide("DP-1", 0), tall("DP-3", 3440)];
        let c = MainDisplay::of(&screens[1]);
        assert_eq!(pick(Some(&c), &screens), Some(1));
        let back = MainDisplay::parse(&c.to_text()).unwrap();
        assert_eq!(back, c, "saved and read again");
        assert_eq!(pick(Some(&back), &screens), Some(1));
    }

    #[test]
    fn a_missing_screen_falls_back_to_the_top_left() {
        let c = MainDisplay::of(&tall("DP-3", 3440));
        assert_eq!(pick(Some(&c), &[wide("DP-1", 0)]), Some(0), "the chosen monitor is unplugged");
        let other = Seen { make: "Another Maker", ..tall("DP-3", 3440) };
        assert_eq!(pick(Some(&c), &[wide("DP-1", 0), other]), Some(0), "another monitor on its port is not it");
        assert_eq!(pick(Some(&c), &[]), None);
        assert_eq!(chosen(&c, &[wide("DP-1", 0)]), None, "not chosen: the fallback is pick's");
    }

    #[test]
    fn the_choice_moves_with_replugging() {
        let c = MainDisplay::of(&tall("DP-3", 3440));
        assert_eq!(pick(Some(&c), &[wide("DP-1", 0), tall("HDMI-A-1", 3440)]), Some(1), "the same monitor on another port");
        assert_eq!(pick(Some(&c), &[tall("DP-2", -2560), wide("DP-1", 0)]), Some(0), "and on the other side");
        // two alike: the connector it was on decides, else the one further left
        assert_eq!(pick(Some(&c), &[tall("DP-2", 3440), tall("DP-3", 6000), wide("DP-1", 0)]), Some(1));
        assert_eq!(pick(Some(&c), &[tall("DP-4", 6000), tall("DP-2", 3440), wide("DP-1", 0)]), Some(1));
    }

    #[test]
    fn a_choice_without_maker_or_model_goes_by_connector() {
        let c = MainDisplay { connector: "DP-3".into(), ..Default::default() };
        let blank = |connector, x| Seen { connector, make: "", model: "", x, y: 0 };
        assert_eq!(pick(Some(&c), &[blank("DP-1", 0), blank("DP-3", 1920)]), Some(1));
        assert_eq!(pick(Some(&c), &[blank("DP-1", 0)]), Some(0));
    }

    #[test]
    fn the_file_is_one_value_a_line() {
        assert_eq!(MainDisplay::parse(""), None);
        assert_eq!(MainDisplay::parse("# nothing chosen\nmain_make =\n"), None);
        let c = MainDisplay { make: "Odd\nMaker".into(), model: " M=1 ".into(), connector: "DP-1".into() };
        let t = c.to_text();
        assert_eq!(t.lines().filter(|l| !l.starts_with('#')).count(), 3, "a line break in a name cannot add a line: {t}");
        let back = MainDisplay::parse(&t).unwrap();
        assert_eq!((back.make.as_str(), back.model.as_str(), back.connector.as_str()), ("Odd Maker", "M=1", "DP-1"));
        assert_eq!(MainDisplay::parse("junk\nmain_model = UW34\nfoo = bar\n").map(|c| c.model), Some("UW34".into()));
    }
}
