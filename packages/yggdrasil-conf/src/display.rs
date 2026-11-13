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

// ---------------------------------------------------------------- the login screen's copy
//
// The login screen runs as the `greeter` account and cannot read anyone's home folder, so the session hands it the choice:
// at each sign-in, and when the choice changes, the panel runs `pkexec /usr/lib/yggdrasil/greeter-display MAKE MODEL CONNECTOR`
// (polkit action org.yggdrasil.greeter.main-display: yes for whoever is signed in at this computer, no for anyone else), which
// writes this file as root, readable by all. It holds only the three values; the most recent sign-in's choice wins, and a
// sign-in without a choice leaves it as it is. The login screen treats it as untrusted: anything odd and it is not used.

/// Where the login screen's copy is kept (the folder is root's, 0755; the file 0644).
pub const SYSTEM_PATH: &str = "/var/lib/yggdrasil/greeter/display.conf";
/// The most bytes the copy may have: three short lines and a comment.
pub const SYSTEM_MAX: usize = 1024;
/// The most characters a maker's or model's name may have (EDID names are 13; the PNP list's makers are longer).
const NAME_MAX: usize = 64;

/// A maker's or model's name as the copy may hold it: 1 to 64 printable characters (ASCII, or letters and digits of other
/// scripts), no control or invisible formatting characters, no spaces around it.
fn system_name_ok(v: &str) -> bool {
    let n = v.chars().count();
    (1..=NAME_MAX).contains(&n)
        && v.trim() == v
        && v.chars().all(|c| c == ' ' || c.is_ascii_graphic() || (!c.is_ascii() && c.is_alphanumeric()))
}

/// A connector's name as the kernel and the compositors give it: DP-1, HDMI-A-1, eDP-1, HEADLESS-2.
fn system_connector_ok(v: &str) -> bool {
    (1..=32).contains(&v.len()) && v.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

/// A choice the copy can hold: a maker and a model (and a connector, or none), or a connector alone.
fn system_ok(c: &MainDisplay) -> bool {
    let named = !c.make.is_empty() || !c.model.is_empty();
    (!named || (system_name_ok(&c.make) && system_name_ok(&c.model)))
        && (c.connector.is_empty() || system_connector_ok(&c.connector))
        && !c.is_empty()
}

/// Reads the copy strictly: only comment lines, blank lines and `main_make`, `main_model`, `main_connector` (each at most once),
/// with values that pass the checks above. Anything else, and the whole file is ignored (None).
pub fn parse_system(text: &str) -> Option<MainDisplay> {
    if text.len() > SYSTEM_MAX {
        return None;
    }
    let mut c = MainDisplay::default();
    let mut seen = [false; 3];
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.trim_matches(' ').is_empty() || line.starts_with('#') {
            continue;
        }
        let (k, v) = line.split_once('=')?;
        let (k, v) = (k.trim_matches(' '), v.trim_matches(' '));
        let i = ["main_make", "main_model", "main_connector"].iter().position(|x| *x == k)?;
        if std::mem::replace(&mut seen[i], true) {
            return None;
        }
        *[&mut c.make, &mut c.model, &mut c.connector][i] = v.to_string();
    }
    system_ok(&c).then_some(c)
}

/// The copy's text for a choice; None when the choice could not be read back by `parse_system` (it is then not written).
pub fn system_text(c: &MainDisplay) -> Option<String> {
    let t = format!(
        "# The main display at the login screen, from the latest sign-in (written by /usr/lib/yggdrasil/greeter-display)\n\
         main_make = {}\nmain_model = {}\nmain_connector = {}\n",
        c.make, c.model, c.connector
    );
    (parse_system(&t).as_ref() == Some(c)).then_some(t)
}

/// The root helper that writes the copy (packages/lock, through pkexec).
pub const SYSTEM_HELPER: &str = "/usr/lib/yggdrasil/greeter-display";

/// What the session runs so the login screen's copy holds this account's choice (`pkexec` and the helper with the three values),
/// given what the copy holds now. None when there is nothing to do: no choice (another account's stays), the copy holds it
/// already, or it is a choice the copy could not hold.
pub fn share_command(choice: Option<&MainDisplay>, copy: Option<&MainDisplay>) -> Option<Vec<String>> {
    let c = choice.filter(|c| copy != Some(*c) && system_text(c).is_some())?;
    Some(vec!["pkexec".into(), SYSTEM_HELPER.into(), c.make.clone(), c.model.clone(), c.connector.clone()])
}

/// Reads the copy at `path` when it can be trusted: a plain file (never through a link, never a pipe that would block),
/// owned by `owner` (root, for the real one), writable by nobody else, no bigger than SYSTEM_MAX. Else None.
pub fn load_system(path: &std::path::Path, owner: u32) -> Option<MainDisplay> {
    use std::io::Read;
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
    let f = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK).open(path).ok()?;
    let m = f.metadata().ok()?;
    if !m.is_file() || m.uid() != owner || m.mode() & 0o022 != 0 || m.len() > SYSTEM_MAX as u64 {
        return None;
    }
    let mut buf = Vec::with_capacity(m.len() as usize);
    f.take(SYSTEM_MAX as u64 + 1).read_to_end(&mut buf).ok()?;
    parse_system(std::str::from_utf8(&buf).ok()?)
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

    fn choice(make: &str, model: &str, connector: &str) -> MainDisplay {
        MainDisplay { make: make.into(), model: model.into(), connector: connector.into() }
    }

    #[test]
    fn the_system_copy_is_written_and_read_back() {
        for c in [
            choice("Example Display", "UW34", "DP-1"),
            choice("Sample Screens", "Tall27", ""),
            choice("", "", "HEADLESS-2"),
            choice("Écrans d'Exemple", "Modèle 9 (rev. B)", "HDMI-A-1"),
        ] {
            let t = system_text(&c).unwrap_or_else(|| panic!("{c:?}"));
            assert_eq!(parse_system(&t), Some(c.clone()));
            assert_eq!(parse_system(&t.replace('\n', "\r\n")), Some(c), "Windows line ends");
        }
        let ok = "main_make = Example Display\nmain_model = UW34\nmain_connector = DP-1\n";
        assert_eq!(parse_system(&format!("# a comment\n\n{ok}  \n")).map(|c| c.model), Some("UW34".into()));
    }

    #[test]
    fn a_choice_the_copy_cannot_hold_is_not_written() {
        for c in [
            MainDisplay::default(),
            choice("Odd\nMaker", "M", "DP-1"),
            choice(" Padded", "M", "DP-1"),
            choice("Maker", "", "DP-1"),
            choice("Maker", "M", "DP 1"),
            choice("Maker", "M", "../../etc"),
            choice(&"x".repeat(65), "M", "DP-1"),
            choice("Maker", "M", &"D".repeat(33)),
            choice("Maker\u{202e}", "M", "DP-1"),
            choice("Maker", "M\u{200b}", "DP-1"),
        ] {
            assert_eq!(system_text(&c), None, "{c:?}");
        }
        assert!(system_text(&choice(&"x".repeat(64), "M", "DP-1")).is_some(), "64 is the most");
    }

    #[test]
    fn a_hostile_system_copy_is_ignored_whole() {
        let ok = "main_make = Example Display\nmain_model = UW34\nmain_connector = DP-1\n";
        assert!(parse_system(ok).is_some());
        let hostile = [
            String::new(),
            "# only a comment\n".to_string(),
            format!("{ok}main_make = Other\n"),                        // a key twice
            format!("{ok}wallpaper = /etc/shadow\n"),                  // a key the copy does not have
            format!("{ok}junk\n"),                                     // a line that is not key = value
            ok.replace("UW34", "UW\u{1b}[2J34"),                       // a terminal escape
            ok.replace("UW34", "UW\u{0}34"),                           // a NUL
            ok.replace("UW34", "\u{202e}43WU"),                        // right-to-left override
            ok.replace("UW34", "\tUW34"),                              // a tab
            ok.replace("DP-1", "DP-1;rm -rf /"),                       // not a connector name
            ok.replace("DP-1", "/dev/dri/card0"),                      // a path
            ok.replace("UW34", ""),                                    // a maker without a model
            ok.replace("UW34", &"9".repeat(65)),                       // too long
            format!("{ok}#{}\n", "x".repeat(SYSTEM_MAX)),              // too big as a whole
            "main_make = \nmain_model = \nmain_connector = \n".into(), // nothing chosen
            ok.replace("main_model", " main_model\u{a0}"),             // an odd key
            ok.replace('\n', "\r"),                                    // one long line
        ];
        for t in &hostile {
            assert_eq!(parse_system(t), None, "{t:?}");
        }
    }

    #[test]
    fn the_session_hands_its_choice_over_only_when_it_changed() {
        let mine = choice("Example Display", "UW34", "DP-1");
        let theirs = choice("Sample Screens", "Tall27", "DP-3");
        let cmd = share_command(Some(&mine), Some(&theirs)).unwrap();
        assert_eq!(cmd, ["pkexec", SYSTEM_HELPER, "Example Display", "UW34", "DP-1"], "the latest sign-in wins");
        assert!(share_command(Some(&mine), None).is_some(), "no copy yet");
        assert_eq!(share_command(Some(&mine), Some(&mine)), None, "already there: no pkexec at every sign-in");
        assert_eq!(share_command(None, Some(&theirs)), None, "no choice: another account's stays");
        assert_eq!(share_command(Some(&choice("Odd\nMaker", "M", "DP-1")), None), None, "one the copy cannot hold");
    }

    #[test]
    fn the_system_copy_is_read_only_when_it_can_be_trusted() {
        use std::os::unix::fs::PermissionsExt;
        let me = unsafe { libc::geteuid() };
        let d = std::env::temp_dir().join(format!("ygg-display-system-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("display.conf");
        let c = choice("Example Display", "UW34", "DP-1");
        std::fs::write(&f, system_text(&c).unwrap()).unwrap();
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_system(&f, me), Some(c.clone()));
        assert_eq!(load_system(&f, me.wrapping_add(1)), None, "owned by someone else");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o666)).unwrap();
        assert_eq!(load_system(&f, me), None, "writable by others");
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = d.join("link.conf");
        std::os::unix::fs::symlink(&f, &link).unwrap();
        assert_eq!(load_system(&link, me), None, "never through a link");
        let fifo = d.join("fifo.conf");
        let p = std::ffi::CString::new(fifo.to_str().unwrap()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(p.as_ptr(), 0o644) }, 0);
        assert_eq!(load_system(&fifo, me), None, "a pipe is not waited on");
        assert_eq!(load_system(&d, me), None, "a folder");
        assert_eq!(load_system(&d.join("missing"), me), None);
        std::fs::write(&f, format!("{}#{}\n", system_text(&c).unwrap(), "x".repeat(SYSTEM_MAX))).unwrap();
        assert_eq!(load_system(&f, me), None, "too big");
        std::fs::write(&f, [b"main_make = \xff\xfe\n".as_slice()].concat()).unwrap();
        assert_eq!(load_system(&f, me), None, "not UTF-8");
        let _ = std::fs::remove_dir_all(&d);
    }
}
