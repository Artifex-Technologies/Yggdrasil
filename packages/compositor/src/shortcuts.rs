//! Keyboard shortcuts handled by the compositor itself (instead of KDE's `kglobalacceld`): the Super key on its own, window switching,
//! snapping, closing, starting programs and the panel's own actions (over its D-Bus service).
//!
//! The built-in table below follows Settings > Keyboard Shortcuts. `~/.config/yggdrasil/shortcuts.conf` changes it, one binding per line:
//!
//!     Super+E = exec minos          # a program
//!     Alt+Tab = next-window         # one of the actions below
//!     Super+Q = none                # removes a built-in binding
//!
//! Actions: start, clipboard, sarina, calendar, fylgja N, close, maximize, restore, fullscreen, minimize, snap-left, snap-right,
//! next-window, prev-window, toggle-float, workspace N, move-to-workspace N, next-workspace, prev-workspace, lock, quit, exec COMMAND, none. Modifiers: Super (or Logo, Win), Ctrl, Alt, Shift.

use smithay::input::keyboard::{keysyms, xkb, ModifiersState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    ToggleStart,
    Clipboard,
    Sarina,
    Calendar,
    Fylgja(u32),
    Close,
    Maximize,
    Restore,
    Fullscreen,
    Minimize,
    SnapLeft,
    SnapRight,
    NextWindow,
    PrevWindow,
    /// Alt let go while the switcher is open: activate the chosen window.
    CommitSwitcher,
    /// Escape while the switcher is open.
    CancelSwitcher,
    /// Float or tile the focused window on a tiling workspace.
    ToggleFloat,
    Workspace(usize),
    MoveToWorkspace(usize),
    WorkspaceNext,
    WorkspacePrev,
    Lock,
    Quit,
    Exec(String),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Mods {
    pub logo: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl Mods {
    pub fn of(m: &ModifiersState) -> Mods {
        Mods { logo: m.logo, ctrl: m.ctrl, alt: m.alt, shift: m.shift }
    }
}

#[derive(Clone, Debug)]
pub struct Binding {
    pub mods: Mods,
    /// Lower-case keysym value.
    pub sym: u32,
    pub action: Action,
}

pub fn parse_action(s: &str) -> Option<Action> {
    let s = s.trim();
    let (word, rest) = s.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((s, ""));
    Some(match word {
        "none" => Action::None,
        "start" => Action::ToggleStart,
        "clipboard" => Action::Clipboard,
        "sarina" => Action::Sarina,
        "calendar" => Action::Calendar,
        "fylgja" => Action::Fylgja(rest.parse().ok()?),
        "close" => Action::Close,
        "maximize" => Action::Maximize,
        "restore" => Action::Restore,
        "fullscreen" => Action::Fullscreen,
        "minimize" => Action::Minimize,
        "snap-left" => Action::SnapLeft,
        "snap-right" => Action::SnapRight,
        "next-window" => Action::NextWindow,
        "prev-window" => Action::PrevWindow,
        "toggle-float" => Action::ToggleFloat,
        "workspace" => Action::Workspace(rest.parse::<usize>().ok()?.checked_sub(1)?),
        "move-to-workspace" => Action::MoveToWorkspace(rest.parse::<usize>().ok()?.checked_sub(1)?),
        "next-workspace" => Action::WorkspaceNext,
        "prev-workspace" => Action::WorkspacePrev,
        "lock" => Action::Lock,
        "quit" => Action::Quit,
        "exec" if !rest.is_empty() => Action::Exec(rest.to_string()),
        _ => return None,
    })
}

/// `Super+Shift+Left` -> modifiers and the key.
pub fn parse_combo(s: &str) -> Option<(Mods, u32)> {
    let mut mods = Mods::default();
    let mut key = None;
    for part in s.split('+').map(str::trim) {
        match part.to_lowercase().as_str() {
            "super" | "logo" | "win" | "meta" => mods.logo = true,
            "ctrl" | "control" => mods.ctrl = true,
            "alt" => mods.alt = true,
            "shift" => mods.shift = true,
            "" => return None,
            _ => {
                if key.is_some() {
                    return None;
                }
                let sym = xkb::keysym_from_name(part, xkb::KEYSYM_CASE_INSENSITIVE);
                if sym.raw() == keysyms::KEY_NoSymbol {
                    return None;
                }
                key = Some(lower(sym.raw()));
            }
        }
    }
    key.map(|k| (mods, k))
}

/// Letters compare in lower case, so Shift does not change which key a binding means.
pub fn lower(raw: u32) -> u32 {
    if (0x41..=0x5a).contains(&raw) {
        raw + 0x20
    } else {
        raw
    }
}

fn b(combo: &str, action: Action) -> Binding {
    let (mods, sym) = parse_combo(combo).expect("built-in shortcut");
    Binding { mods, sym, action }
}

pub fn defaults() -> Vec<Binding> {
    vec![
        b("Super+E", Action::Exec("minos".into())),
        b("Super+I", Action::Exec("labyrinth-settings".into())),
        b("Super+Return", Action::Exec("ghostty".into())),
        b("Super+L", Action::Lock),
        b("Super+V", Action::Clipboard),
        b("Super+S", Action::Sarina),
        b("Super+C", Action::Calendar),
        b("Super+F", Action::Fullscreen),
        b("Super+Q", Action::Close),
        b("Alt+F4", Action::Close),
        b("Super+Up", Action::Maximize),
        b("Super+Down", Action::Restore),
        b("Super+Left", Action::SnapLeft),
        b("Super+Right", Action::SnapRight),
        b("Alt+Tab", Action::NextWindow),
        b("Alt+Shift+Tab", Action::PrevWindow),
        b("Super+T", Action::ToggleFloat),
        b("Super+Ctrl+Left", Action::WorkspacePrev),
        b("Super+Ctrl+Right", Action::WorkspaceNext),
        b("Super+Ctrl+1", Action::Workspace(0)),
        b("Super+Ctrl+2", Action::Workspace(1)),
        b("Super+Ctrl+3", Action::Workspace(2)),
        b("Super+Ctrl+4", Action::Workspace(3)),
        b("Super+Ctrl+Shift+1", Action::MoveToWorkspace(0)),
        b("Super+Ctrl+Shift+2", Action::MoveToWorkspace(1)),
        b("Super+Ctrl+Shift+3", Action::MoveToWorkspace(2)),
        b("Super+Ctrl+Shift+4", Action::MoveToWorkspace(3)),
        // the multimedia keys (labyrinth-mediakey: PipeWire volume, the running music player, brightness)
        b("XF86AudioRaiseVolume", Action::Exec("labyrinth-mediakey volume-up".into())),
        b("XF86AudioLowerVolume", Action::Exec("labyrinth-mediakey volume-down".into())),
        b("XF86AudioMute", Action::Exec("labyrinth-mediakey mute".into())),
        b("XF86AudioMicMute", Action::Exec("labyrinth-mediakey mic-mute".into())),
        b("XF86AudioPlay", Action::Exec("labyrinth-mediakey play-pause".into())),
        b("XF86AudioNext", Action::Exec("labyrinth-mediakey next".into())),
        b("XF86AudioPrev", Action::Exec("labyrinth-mediakey previous".into())),
        b("XF86AudioStop", Action::Exec("labyrinth-mediakey stop".into())),
        b("XF86MonBrightnessUp", Action::Exec("labyrinth-mediakey brightness-up".into())),
        b("XF86MonBrightnessDown", Action::Exec("labyrinth-mediakey brightness-down".into())),
    ]
}

/// The built-in table with the user's lines applied. Lines that make no sense are skipped (and listed in the second value).
pub fn load(text: &str) -> (Vec<Binding>, Vec<String>) {
    let mut v = defaults();
    let mut bad = vec![];
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let Some((combo, action)) = line.split_once('=') else {
            bad.push(line.to_string());
            continue;
        };
        match (parse_combo(combo), parse_action(action)) {
            (Some((mods, sym)), Some(action)) => {
                v.retain(|x| !(x.mods == mods && x.sym == sym));
                if action != Action::None {
                    v.push(Binding { mods, sym, action });
                }
            }
            _ => bad.push(line.to_string()),
        }
    }
    (v, bad)
}

pub fn config_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("LABYRINTH_COMPOSITOR_SHORTCUTS") {
        return p.into();
    }
    yggdrasil_conf::dirs::config().join("shortcuts.conf")
}

pub fn find(bindings: &[Binding], mods: Mods, sym: u32) -> Option<&Binding> {
    bindings.iter().find(|x| x.mods == mods && x.sym == sym)
}

pub fn is_super(sym: u32) -> bool {
    sym == keysyms::KEY_Super_L || sym == keysyms::KEY_Super_R
}

pub fn is_alt(sym: u32) -> bool {
    sym == keysyms::KEY_Alt_L || sym == keysyms::KEY_Alt_R
}

/// Is this key one of the modifiers that hold the switcher open?
pub fn is_held_modifier(sym: u32, hold: Mods) -> bool {
    (hold.alt && is_alt(sym)) || (hold.ctrl && (sym == keysyms::KEY_Control_L || sym == keysyms::KEY_Control_R)) || (hold.logo && is_super(sym))
}

pub fn is_escape(sym: u32) -> bool {
    sym == keysyms::KEY_Escape
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn combos_parse() {
        let (m, k) = parse_combo("Super+Shift+Left").unwrap();
        assert!(m.logo && m.shift && !m.ctrl);
        assert_eq!(k, keysyms::KEY_Left);
        assert!(parse_combo("Super+").is_none());
        assert!(parse_combo("Super+Nonsense").is_none());
        assert!(parse_combo("Ctrl+A+B").is_none());
    }

    #[test]
    fn letters_are_case_free() {
        assert_eq!(parse_combo("Super+E"), parse_combo("super+e"));
    }

    #[test]
    fn actions_parse() {
        assert_eq!(parse_action("exec minos /tmp"), Some(Action::Exec("minos /tmp".into())));
        assert_eq!(parse_action("fylgja 4"), Some(Action::Fylgja(4)));
        assert_eq!(parse_action("exec"), None);
        assert_eq!(parse_action("workspace 2"), Some(Action::Workspace(1)));
        assert_eq!(parse_action("workspace 0"), None);
        assert_eq!(parse_action("dance"), None);
    }

    #[test]
    fn the_user_changes_and_removes_bindings() {
        let (v, bad) = load("Super+E = exec thunar\nSuper+Q = none\nAlt+X = close\nnonsense\n");
        let (m, e) = parse_combo("Super+E").unwrap();
        assert_eq!(find(&v, m, e).unwrap().action, Action::Exec("thunar".into()));
        let (m, q) = parse_combo("Super+Q").unwrap();
        assert!(find(&v, m, q).is_none());
        let (m, x) = parse_combo("Alt+X").unwrap();
        assert_eq!(find(&v, m, x).unwrap().action, Action::Close);
        assert_eq!(bad, vec!["nonsense".to_string()]);
    }

    #[test]
    fn the_built_in_table_has_no_duplicates() {
        let d = defaults();
        for (i, a) in d.iter().enumerate() {
            assert!(!d[i + 1..].iter().any(|x| x.mods == a.mods && x.sym == a.sym), "{:?}", a);
        }
    }
}
