//! Keyboard shortcuts handled by the compositor itself: the same table as the Hyprland session's (packages/session/hyprland/
//! yggdrasil.lua; a test below holds them together), with the same descriptions, the same programs for the keys that start one
//! (`yggdrasil-panelctl ...`, `yggdrasil-shot`, `loginctl lock-session` ...) and the window keys done here as the session's
//! `yggdrasil-close` and `yggdrasil-snap` do them on Hyprland.
//!
//! - A binding on a modifier key itself (Super+Super_L: the start menu; Ctrl+Control_L: a Ctrl tap for Find my mouse) runs when
//!   that key is let go with nothing else pressed in between, and the key still reaches the program (Hyprland's `release` and
//!   `non_consuming` binds).
//! - The volume, brightness and magnifier keys repeat while held (Hyprland's `repeating`), after the keyboard's delay and at its
//!   rate (Settings > Peripherals' input.conf, input.rs; Hyprland's 600 ms and 25 a second when it says nothing).
//! - Only the media keys work on the lock screen (`works_locked`; Hyprland's `locked`).
//!
//! `~/.config/yggdrasil/shortcuts.conf` changes the table, one binding per line:
//!
//!     Super+E = exec minos          # a program
//!     Alt+Tab = next-window         # one of the actions below
//!     Super+Q = none                # removes a built-in binding
//!
//! Actions: start, clipboard, assistant, calendar, fylgja N, close, maximize, restore, fullscreen, minimize, minimized (show the
//! minimized windows, again: hide them), snap-left, snap-right, next-window, prev-window, toggle-float, workspace N,
//! move-to-workspace N, next-workspace, prev-workspace, lock, quit, exec COMMAND, none. Modifiers: Super (or Logo, Win), Ctrl,
//! Alt, Shift.

use smithay::input::keyboard::{keysyms, xkb, ModifiersState};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    ToggleStart,
    Clipboard,
    Assistant,
    Calendar,
    Fylgja(u32),
    Close,
    Maximize,
    Restore,
    Fullscreen,
    Minimize,
    /// Show the minimized windows over the desktop, again: hide them (Hyprland's special workspace "minimized").
    ShowMinimized,
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
    /// What it does, in words (the shortcut lists show it): the session's own words for the built-in table.
    pub desc: String,
    /// Runs again while the key is held.
    pub repeat: bool,
}

impl Binding {
    /// A binding on a modifier key itself: it runs when the key is let go on its own (a tap), and the key is not taken.
    pub fn tap(&self) -> bool {
        is_modifier(self.sym)
    }
}

pub fn parse_action(s: &str) -> Option<Action> {
    let s = s.trim();
    let (word, rest) = s.split_once(char::is_whitespace).map(|(a, b)| (a, b.trim())).unwrap_or((s, ""));
    Some(match word {
        "none" => Action::None,
        "start" => Action::ToggleStart,
        "clipboard" => Action::Clipboard,
        "assistant" | "sarina" => Action::Assistant, // "sarina": what older config files say
        "calendar" => Action::Calendar,
        "fylgja" => Action::Fylgja(rest.parse().ok()?),
        "close" => Action::Close,
        "maximize" => Action::Maximize,
        "restore" => Action::Restore,
        "fullscreen" => Action::Fullscreen,
        "minimize" => Action::Minimize,
        "minimized" => Action::ShowMinimized,
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

/// A binding as the shortcut lists read it (Settings > Keyboard Shortcuts, the panel's card): Hyprland's modifier numbers (Shift
/// 1, Ctrl 4, Alt 8, Super 64), the key's xkb name, what it does, and its kind (`release` for a tap, `repeat`, `locked`; comma
/// separated, `-` for none).
pub fn listed(b: &Binding) -> (u32, String, String, String) {
    let m = &b.mods;
    let mask = m.shift as u32 | (m.ctrl as u32) << 2 | (m.alt as u32) << 3 | (m.logo as u32) << 6;
    let kinds: Vec<&str> =
        [(b.tap(), "release"), (b.repeat, "repeat"), (works_locked(b.sym), "locked")].into_iter().filter(|k| k.0).map(|k| k.1).collect();
    let kind = if kinds.is_empty() { "-".to_string() } else { kinds.join(",") };
    (mask, xkb::keysym_get_name(xkb::Keysym::new(b.sym)), b.desc.clone(), kind)
}

/// The mouse's bindings, built in (Super and the left button moves the window under the pointer, Super and the right button
/// resizes it: state.rs's super_drag), as the lists show them: Hyprland's mask, the button as Hyprland names it, what it does.
pub const MOUSE: [(u32, &str, &str); 2] = [(64, "mouse:272", "Move the window"), (64, "mouse:273", "Resize the window")];

/// What an action does, in words.
pub fn describe(a: &Action) -> String {
    match a {
        Action::None => "Nothing".into(),
        Action::ToggleStart => "Start menu".into(),
        Action::Clipboard => "Clipboard history".into(),
        Action::Assistant => "Ask the assistant".into(),
        Action::Calendar => "Calendar".into(),
        Action::Fylgja(n) => format!("Fylgja, section {n}"),
        Action::Close => "Close the window".into(),
        Action::Maximize => "Maximize (again: restore)".into(),
        Action::Restore => "Restore, or minimize".into(),
        Action::Fullscreen => "Full screen".into(),
        Action::Minimize => "Minimize".into(),
        Action::ShowMinimized => "Show minimized windows".into(),
        Action::SnapLeft => "Left half".into(),
        Action::SnapRight => "Right half".into(),
        Action::NextWindow => "Switch windows".into(),
        Action::PrevWindow => "Switch windows, backwards".into(),
        Action::CommitSwitcher | Action::CancelSwitcher => "Window switcher".into(),
        Action::ToggleFloat => "Float or tile the window".into(),
        Action::Workspace(n) => format!("Desktop {}", n + 1),
        Action::MoveToWorkspace(n) => format!("Move the window to desktop {}", n + 1),
        Action::WorkspaceNext => "Next desktop".into(),
        Action::WorkspacePrev => "Previous desktop".into(),
        Action::Lock => "Lock the screen".into(),
        Action::Quit => "End the session".into(),
        Action::Exec(cmd) => format!("Run {cmd}"),
    }
}

fn b(combo: &str, action: Action, desc: &str) -> Binding {
    let (mods, sym) = parse_combo(combo).expect("built-in shortcut");
    Binding { mods, sym, action, desc: desc.into(), repeat: false }
}

fn exec(cmd: &str) -> Action {
    Action::Exec(cmd.into())
}

/// The session's table (yggdrasil.lua), in its order and with its words.
pub fn defaults() -> Vec<Binding> {
    const ASK: &str = "Talk to the assistant (tap to talk hands-free, tap again to end)";
    const CLOSE: &str = "Close the window (programs that keep running: hide it)";
    const SHOT: &str = "Screenshot (area, window or screen)";
    const FIND: &str = "Find my mouse (press Ctrl twice)";
    let mut v = vec![
        b("Super+Super_L", Action::ToggleStart, "Start menu"),
        b("Super+S", Action::Assistant, ASK),
        b("Ctrl+Alt+S", Action::Assistant, ASK),
        b("Super+V", Action::Clipboard, "Clipboard history and snippets"),
        b("Super+C", Action::Calendar, "Calendar"),
        b("Super+N", exec("yggdrasil-panelctl notifications"), "Notifications"),
        b("Super+E", exec("minos"), "Files (Minos)"),
        b("Super+I", exec("yggdrasil-settings"), "Settings"),
        b("Super+Return", exec("ghostty"), "Terminal"),
        b("Super+L", Action::Lock, "Lock the screen"),
        b("Ctrl+Shift+Escape", exec("yggdrasil-taskmanager"), "Task Manager (again: brings it to the front)"),
        b("Alt+F4", Action::Close, CLOSE),
        b("Super+Q", Action::Close, CLOSE),
        b("Super+F", Action::Fullscreen, "Full screen"),
        b("Super+Up", Action::Maximize, "Maximize (again: restore), or focus up on a tiling screen"),
        b("Super+Down", Action::Minimize, "Minimize, or focus down on a tiling screen"),
        b("Super+W", Action::Close, "Close the window (as in Omarchy)"),
        b("Super+Shift+M", Action::ShowMinimized, "Show minimized windows"),
        b("Super+Left", Action::SnapLeft, "Left half, or focus left on a tiling screen"),
        b("Super+Right", Action::SnapRight, "Right half, or focus right on a tiling screen"),
        b("Super+T", Action::ToggleFloat, "Tile or float the window"),
        b("Alt+Tab", Action::NextWindow, "Next window"),
        b("Alt+Shift+Tab", Action::PrevWindow, "Previous window"),
        b("Super+Ctrl+Left", Action::WorkspacePrev, "Previous desktop"),
        b("Super+Ctrl+Right", Action::WorkspaceNext, "Next desktop"),
    ];
    for n in 1..=4 {
        v.push(b(&format!("Super+Ctrl+{n}"), Action::Workspace(n - 1), &format!("Desktop {n}")));
        v.push(b(&format!("Super+Ctrl+Shift+{n}"), Action::MoveToWorkspace(n - 1), &format!("Move the window to desktop {n}")));
    }
    v.extend([
        b("Print", exec("yggdrasil-shot"), SHOT),
        b("Super+Shift+S", exec("yggdrasil-shot"), SHOT),
        b("Super+Shift+C", exec("yggdrasil-colorpicker"), "Color picker"),
        b("Super+slash", exec("yggdrasil-panelctl shortcuts"), "Keyboard shortcuts"),
        Binding { repeat: true, ..b("Super+Alt+equal", exec("yggdrasil-zoom in"), "Magnifier: zoom in around the pointer") },
        Binding { repeat: true, ..b("Super+Alt+minus", exec("yggdrasil-zoom out"), "Magnifier: zoom out") },
        b("Super+Alt+0", exec("yggdrasil-zoom off"), "Magnifier off"),
        b("Super+Alt+S", exec("yggdrasil-screen-reader"), "Screen reader (Orca) on or off"),
        b("Ctrl+Control_L", exec("yggdrasil-panelctl cursor-tap"), FIND),
        b("Ctrl+Control_R", exec("yggdrasil-panelctl cursor-tap"), FIND),
        b("Super+Shift+Z", exec("yggdrasil-panelctl zones"), "Window zones for the window in front"),
    ]);
    // the multimedia keys (yggdrasil-mediakey: PipeWire volume, the running music player, brightness), also on the lock screen;
    // no words of their own (the lists name them after their key), volume and brightness repeat
    for (key, what) in [
        ("XF86AudioRaiseVolume", "volume-up"),
        ("XF86AudioLowerVolume", "volume-down"),
        ("XF86AudioMute", "mute"),
        ("XF86AudioMicMute", "mic-mute"),
        ("XF86AudioPlay", "play-pause"),
        ("XF86AudioPause", "play-pause"),
        ("XF86AudioNext", "next"),
        ("XF86AudioPrev", "previous"),
        ("XF86AudioStop", "stop"),
        ("XF86MonBrightnessUp", "brightness-up"),
        ("XF86MonBrightnessDown", "brightness-down"),
    ] {
        let repeat = what.contains("volume") || what.contains("brightness");
        v.push(Binding { repeat, ..b(key, exec(&format!("yggdrasil-mediakey {what}")), "") });
    }
    // a laptop's other top-row keys (as the Hyprland session's yggdrasil.lua): screens, airplane mode, notifications, the star key
    for (key, what, words) in [
        ("XF86Display", "display", "Screens"),
        ("XF86RFKill", "airplane", "Airplane mode"),
        ("XF86WLAN", "airplane", "Airplane mode"),
        ("XF86NotificationCenter", "notifications", "Notifications"),
        ("XF86Favorites", "favorite", "Star key"),
    ] {
        v.push(b(key, exec(&format!("yggdrasil-mediakey {what}")), words));
    }
    v
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
                    let desc = describe(&action);
                    v.push(Binding { mods, sym, action, desc, repeat: false });
                }
            }
            _ => bad.push(line.to_string()),
        }
    }
    (v, bad)
}

pub fn config_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("YGGDRASIL_COMPOSITOR_SHORTCUTS") {
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

/// A modifier key itself (a tap binding's key).
pub fn is_modifier(sym: u32) -> bool {
    use keysyms as k;
    [
        k::KEY_Super_L,
        k::KEY_Super_R,
        k::KEY_Control_L,
        k::KEY_Control_R,
        k::KEY_Alt_L,
        k::KEY_Alt_R,
        k::KEY_Shift_L,
        k::KEY_Shift_R,
        k::KEY_Meta_L,
        k::KEY_Meta_R,
    ]
    .contains(&sym)
}

pub fn is_alt(sym: u32) -> bool {
    sym == keysyms::KEY_Alt_L || sym == keysyms::KEY_Alt_R
}

/// Is this key one of the modifiers that hold the switcher open?
pub fn is_held_modifier(sym: u32, hold: Mods) -> bool {
    (hold.alt && is_alt(sym))
        || (hold.ctrl && (sym == keysyms::KEY_Control_L || sym == keysyms::KEY_Control_R))
        || (hold.logo && is_super(sym))
}

pub fn is_escape(sym: u32) -> bool {
    sym == keysyms::KEY_Escape
}

/// The media keys (volume, microphone, player, brightness) work on the lock screen too, whatever they are bound to (the session's
/// Hyprland binds them with `locked = true`); no other shortcut runs while the screen is locked.
pub fn works_locked(sym: u32) -> bool {
    use keysyms as k;
    [
        k::KEY_XF86AudioRaiseVolume,
        k::KEY_XF86AudioLowerVolume,
        k::KEY_XF86AudioMute,
        k::KEY_XF86AudioMicMute,
        k::KEY_XF86AudioPlay,
        k::KEY_XF86AudioPause,
        k::KEY_XF86AudioNext,
        k::KEY_XF86AudioPrev,
        k::KEY_XF86AudioStop,
        k::KEY_XF86MonBrightnessUp,
        k::KEY_XF86MonBrightnessDown,
    ]
    .contains(&sym)
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

    /// The built-in table is the Hyprland session's: every bind of yggdrasil.lua (keys, modifiers, words, release) is here and
    /// nothing else, the mouse's two built in; the same keys repeat.
    #[test]
    fn the_table_is_the_sessions() {
        let lua = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../session/hyprland/yggdrasil.lua")).unwrap();
        let session = yggdrasil_conf::shortcuts::lua_binds(&lua);
        let key = |mask: u32, k: &str| (mask, k.to_ascii_lowercase());
        let mut want: Vec<(u32, String, String, bool)> = vec![];
        for b in &session {
            // a second action on keys that already have words (Alt+Tab also raises the window) is the same shortcut
            if b.desc.is_empty() && session.iter().any(|o| key(o.modmask, &o.key) == key(b.modmask, &b.key) && !o.desc.is_empty()) {
                continue;
            }
            want.push((b.modmask, b.key.to_ascii_lowercase(), b.desc.clone(), b.release));
        }
        let mut have: Vec<(u32, String, String, bool)> = defaults()
            .iter()
            .map(|b| {
                let (mask, k, desc, kind) = listed(b);
                (mask, k.to_ascii_lowercase(), desc, kind.split(',').any(|x| x == "release"))
            })
            .collect();
        have.extend(MOUSE.iter().map(|(mask, k, desc)| (*mask, k.to_string(), desc.to_string(), false)));
        want.sort();
        have.sort();
        let missing: Vec<_> = want.iter().filter(|w| !have.contains(w)).collect();
        let extra: Vec<_> = have.iter().filter(|h| !want.contains(h)).collect();
        assert!(missing.is_empty() && extra.is_empty(), "not in the compositor: {missing:?}\nnot in the session: {extra:?}");
        assert_eq!(have.len(), 62, "{have:?}");

        // repeating: the session's `repeating = true` binds (the magnifier) and the volume and brightness keys of its media table
        let mut repeating: Vec<(u32, String)> = lua
            .lines()
            .filter(|l| l.trim_start().starts_with("cmd(\"") && l.contains("repeating = true"))
            .filter_map(|l| yggdrasil_conf::shortcuts::lua_binds(l).into_iter().next())
            .map(|b| key(b.modmask, &b.key))
            .collect();
        repeating.extend(
            ["XF86AudioRaiseVolume", "XF86AudioLowerVolume", "XF86MonBrightnessUp", "XF86MonBrightnessDown"].iter().map(|k| key(0, k)),
        );
        let mut ours: Vec<(u32, String)> = defaults().iter().filter(|b| b.repeat).map(listed).map(|(m, k, _, _)| key(m, &k)).collect();
        repeating.sort();
        ours.sort();
        assert_eq!(ours, repeating);
        assert!(lua.contains("local repeating = what:find(\"volume\") ~= nil or what:find(\"brightness\") ~= nil"), "the session's rule");
    }

    #[test]
    fn taps_are_the_bindings_on_a_modifier_key() {
        let d = defaults();
        let taps: Vec<String> = d.iter().filter(|b| b.tap()).map(|b| listed(b).1).collect();
        assert_eq!(taps, ["Super_L", "Control_L", "Control_R"]);
        let (m, s) = parse_combo("Super+Super_L").unwrap();
        assert!(m.logo && is_modifier(s));
        let mute = d.iter().find(|b| listed(b).1 == "XF86AudioMute").unwrap();
        assert_eq!(listed(mute).3, "locked");
        let up = d.iter().find(|b| listed(b).1 == "XF86AudioRaiseVolume").unwrap();
        assert_eq!(listed(up).3, "repeat,locked");
        assert_eq!(listed(&d[0]).3, "release");
        assert_eq!(listed(&d[1]).3, "-");
    }

    #[test]
    fn the_built_in_table_has_no_duplicates() {
        let d = defaults();
        for (i, a) in d.iter().enumerate() {
            assert!(!d[i + 1..].iter().any(|x| x.mods == a.mods && x.sym == a.sym), "{:?}", a);
        }
    }
}
