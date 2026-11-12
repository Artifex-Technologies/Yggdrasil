//! What Yggdrasil's own tools ask of the compositor and tell it to do, whichever compositor runs: the windows, the workspaces
//! (the panel's desktops), the screens, the pointer, and moving, minimizing, placing and closing windows.
//!
//! Two backends, chosen at run time by what is running (`Desk::detect`):
//! - **Yggdrasil's own compositor** (packages/compositor): its control socket, `$XDG_RUNTIME_DIR/yggdrasil-compositor-
//!   $WAYLAND_DISPLAY.ctl`, the one that belongs to the display this program is on. Its window list for the panel is the standard
//!   wlr-foreign-toplevel protocol (the panel's own code); the rest goes through the socket.
//! - **Hyprland**: its socket, `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock`, with the requests hyprctl sends
//!   and the answers read as before (hyprland.rs).
//!
//! Every exchange is bounded in time and size (sock.rs), and a socket is only spoken to when it is this user's own.

pub mod hyprland;
pub mod own;
pub mod sock;

use std::ffi::OsString;
use std::path::PathBuf;

pub use hyprland::Hyprland;
pub use own::Own;

/// A rectangle in logical pixels: x, y, width, height.
pub type Rect = (i32, i32, i32, i32);

/// The compositor that runs.
#[derive(Clone, Debug, PartialEq)]
pub enum Desk {
    Hyprland(Hyprland),
    Own(Own),
}

/// Which screen a window is on: Hyprland numbers them, Yggdrasil's compositor names them.
#[derive(Clone, Debug, PartialEq)]
pub enum MonitorRef {
    Id(i64),
    Name(String),
}

/// A window, as the compositor lists it.
#[derive(Clone, Debug, PartialEq)]
pub struct Window {
    /// Hyprland's address ("0x..."), or the number Yggdrasil's compositor gives it.
    pub id: String,
    /// The app id (Wayland) or class (X11).
    pub class: String,
    pub initial_class: String,
    pub title: String,
    pub workspace: Option<i64>,
    /// On Hyprland's hidden "minimized" workspace, or minimized.
    pub minimized: bool,
    /// On a hidden workspace (Hyprland's "special:..."), or minimized: not on any desktop.
    pub special: bool,
    pub at: Option<(f64, f64)>,
    pub size: Option<(f64, f64)>,
    pub floating: Option<bool>,
    pub fullscreen: bool,
    pub mapped: bool,
    pub hidden: bool,
    pub monitor: Option<MonitorRef>,
    /// 0 for the window used last, then 1, ...; i64::MAX when the compositor does not say.
    pub focus_order: i64,
    pub pid: Option<u32>,
}

/// A screen.
#[derive(Clone, Debug, PartialEq)]
pub struct Screen {
    pub id: Option<i64>,
    pub name: String,
    /// The top-left corner in the space of all screens (logical pixels).
    pub x: i32,
    pub y: i32,
    /// The mode's size (physical pixels, before the transform).
    pub width: u32,
    pub height: u32,
    /// None when the compositor did not say (each caller has its default).
    pub scale: Option<f64>,
    pub transform: u32,
    /// The strips panels reserve: left, top, right, bottom (logical pixels).
    pub reserved: [i32; 4],
    pub focused: bool,
    pub disabled: bool,
    /// The workspace it shows.
    pub active_workspace: Option<i64>,
    /// A hidden workspace open over it (Hyprland's "special:minimized").
    pub special_workspace: Option<String>,
}

impl Screen {
    /// As window zones see it (yggdrasil_conf::zones).
    pub fn monitor(&self) -> yggdrasil_conf::zones::Monitor {
        yggdrasil_conf::zones::Monitor {
            name: self.name.clone(),
            x: self.x,
            y: self.y,
            width: self.width,
            height: self.height,
            scale: self.scale.unwrap_or(1.0) as f32,
            transform: self.transform,
            reserved: self.reserved,
        }
    }

    pub fn is(&self, r: &MonitorRef) -> bool {
        match r {
            MonitorRef::Id(i) => self.id == Some(*i),
            MonitorRef::Name(n) => self.name == *n,
        }
    }
}

/// The desktops (numbered workspaces that hold a window or are shown), in order, and the position of the one shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Desktops {
    pub ids: Vec<i64>,
    pub at: usize,
}

impl Desktops {
    /// The desktop shown, when there is one.
    pub fn current(&self) -> Option<i64> {
        self.ids.get(self.at).copied()
    }
}

/// A name that may be part of a path: no folder separator, not "." or "..".
fn plain(name: &OsString) -> Option<String> {
    let s = name.to_str()?;
    (!s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\0')).then(|| s.to_string())
}

impl Desk {
    /// The compositor this program runs on, from its environment; None when neither is found (another desktop, or none).
    pub fn detect() -> Option<Desk> {
        Desk::from_env(|k| std::env::var_os(k))
    }

    /// As `detect`, from the variables `var` gives. Yggdrasil's compositor first: its control socket belongs to the Wayland
    /// display this program is on, so a program inside it never acts on a Hyprland it was started from.
    pub fn from_env(var: impl Fn(&str) -> Option<OsString>) -> Option<Desk> {
        let run = PathBuf::from(var("XDG_RUNTIME_DIR")?);
        if let Some(display) = var("WAYLAND_DISPLAY").as_ref().and_then(plain) {
            let ctl = run.join(format!("yggdrasil-compositor-{display}.ctl"));
            if sock::ours(&ctl) {
                return Some(Desk::Own(Own { ctl }));
            }
        }
        let sig = var("HYPRLAND_INSTANCE_SIGNATURE").as_ref().and_then(plain)?;
        let socket = run.join("hypr").join(sig).join(".socket.sock");
        sock::ours(&socket).then_some(Desk::Hyprland(Hyprland { socket, runtime: run }))
    }

    pub fn is_hyprland(&self) -> bool {
        matches!(self, Desk::Hyprland(_))
    }

    pub fn windows(&self) -> Vec<Window> {
        match self {
            Desk::Hyprland(h) => h.windows(),
            Desk::Own(o) => o.windows(),
        }
    }

    /// The window in front. `quick`: asked while a window is dragged, so the answer must come at once.
    pub fn active_window(&self, quick: bool) -> Option<Window> {
        match self {
            Desk::Hyprland(h) => h.active_window(quick),
            Desk::Own(o) => o.active_window(),
        }
    }

    pub fn screens(&self) -> Vec<Screen> {
        match self {
            Desk::Hyprland(h) => h.screens(false),
            Desk::Own(o) => o.screens(),
        }
    }

    /// The screens, asked while a window is dragged: the answer must come at once.
    pub fn screens_quick(&self) -> Vec<Screen> {
        match self {
            Desk::Hyprland(h) => h.screens(true),
            Desk::Own(o) => o.screens(),
        }
    }

    /// The screen that has the focus.
    pub fn focused_screen(&self) -> Option<Screen> {
        self.screens().into_iter().find(|s| s.focused)
    }

    /// Where the pointer is (global logical pixels).
    pub fn cursor(&self) -> Option<(f64, f64)> {
        match self {
            Desk::Hyprland(h) => h.cursor(),
            Desk::Own(o) => o.cursor(),
        }
    }

    pub fn desktops(&self) -> Desktops {
        match self {
            Desk::Hyprland(h) => h.desktops(),
            Desk::Own(o) => o.desktops(),
        }
    }

    /// The most desktops there can be (None: no limit of the compositor's own).
    pub fn max_desktops(&self) -> Option<usize> {
        match self {
            Desk::Hyprland(_) => None,
            Desk::Own(_) => Some(4),
        }
    }

    /// Shows desktop `id` (a number).
    pub fn show_desktop(&self, id: &str) {
        match self {
            Desk::Hyprland(h) => h.show_desktop(id),
            Desk::Own(o) => o.show_desktop(id),
        }
    }

    /// Moves a window to desktop `to`, staying where one is.
    pub fn send_to_desktop(&self, window: &str, to: &str) {
        match self {
            Desk::Hyprland(h) => h.send_to_desktop(window, to),
            Desk::Own(o) => o.send_to_desktop(window, to),
        }
    }

    /// The window the panel's window list means (wlr-foreign-toplevel gives an app id and a title): the same class and title, or
    /// the class alone when only one window has it.
    pub fn find(&self, app_id: &str, title: &str) -> Option<Window> {
        find_in(self.windows(), app_id, title)
    }

    /// The dock's minimize where the compositor needs one of its own: true when done here. False on a compositor whose
    /// wlr-foreign-toplevel minimize works (Yggdrasil's): the caller asks that way.
    pub fn minimize_app(&self, app_id: &str, title: &str) -> bool {
        match self {
            Desk::Hyprland(h) => self.find(app_id, title).is_some_and(|w| h.minimize(&w)),
            Desk::Own(_) => false,
        }
    }

    /// The dock's activate where the compositor needs one of its own (Hyprland: back from the hidden "minimized" workspace, and
    /// raised); false where wlr-foreign-toplevel's activate does it all.
    pub fn restore_app(&self, app_id: &str, title: &str) -> bool {
        match self {
            Desk::Hyprland(h) => self.find(app_id, title).is_some_and(|w| h.restore(&w)),
            Desk::Own(_) => false,
        }
    }

    /// Puts a window at a rectangle (global logical pixels) as a floating window. Each request with the compositor's answer.
    pub fn place(&self, window: &str, floating: bool, rect: Rect) -> Vec<(String, Option<String>)> {
        match self {
            Desk::Hyprland(h) => h.place(window, floating, rect),
            Desk::Own(o) => o.place(window, rect),
        }
    }

    /// Whether a Super key is held now.
    pub fn super_held(&self) -> Option<bool> {
        match self {
            Desk::Hyprland(h) => h.keys_down(&["Super_L", "Super_R"], "yggdrasil-zones-super"),
            Desk::Own(o) => o.super_held(),
        }
    }

    /// How long the pointer may rest before it is hidden (seconds; 0 = never).
    pub fn pointer_timeout(&self) -> Option<f32> {
        match self {
            Desk::Hyprland(h) => h.pointer_timeout(),
            Desk::Own(o) => o.pointer_timeout(),
        }
    }

    pub fn set_pointer_timeout(&self, secs: f32) {
        match self {
            Desk::Hyprland(h) => h.set_pointer_timeout(secs),
            Desk::Own(o) => o.set_pointer_timeout(secs),
        }
    }

    /// Moves the focus to the window that way ('l', 'r', 'u', 'd'): tiling screens on Hyprland. False where it cannot.
    pub fn focus_direction(&self, d: char) -> bool {
        match self {
            Desk::Hyprland(h) => h.focus_direction(d),
            Desk::Own(_) => false,
        }
    }

    /// Maximize the window in front, or bring it back from maximized.
    pub fn maximize_active(&self) -> bool {
        match self {
            Desk::Hyprland(h) => h.maximize_active(),
            Desk::Own(o) => o.maximize_active(),
        }
    }

    pub fn minimize_active(&self) -> bool {
        match self {
            Desk::Hyprland(h) => h.minimize_active(),
            Desk::Own(o) => o.minimize_active(),
        }
    }

    pub fn close_active(&self) -> bool {
        match self {
            Desk::Hyprland(h) => h.close_active(),
            Desk::Own(o) => o.close_active(),
        }
    }

    /// Puts the window in front at a rectangle (a half of its screen).
    pub fn snap_active(&self, rect: Rect) -> bool {
        match self {
            Desk::Hyprland(h) => h.snap_active(rect),
            Desk::Own(o) => o.snap_active(rect),
        }
    }
}

/// The window for an app id and title: the same class (or first class) and title; else the class alone when only one window
/// has it.
pub fn find_in(windows: Vec<Window>, app_id: &str, title: &str) -> Option<Window> {
    let class = |w: &Window| w.class == app_id || w.initial_class == app_id;
    if let Some(w) = windows.iter().find(|w| class(w) && w.title == title) {
        return Some(w.clone());
    }
    let mut same = windows.into_iter().filter(class);
    let first = same.next()?;
    same.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: &str, class: &str, title: &str) -> Window {
        Window {
            id: id.into(),
            class: class.into(),
            initial_class: class.into(),
            title: title.into(),
            workspace: Some(1),
            minimized: false,
            special: false,
            at: None,
            size: None,
            floating: None,
            fullscreen: false,
            mapped: true,
            hidden: false,
            monitor: None,
            focus_order: i64::MAX,
            pid: None,
        }
    }

    #[test]
    fn the_window_of_a_list_entry_is_found() {
        let list = vec![win("0x1", "term", "one"), win("0x2", "term", "two"), win("0x3", "editor", "notes")];
        assert_eq!(find_in(list.clone(), "term", "two").map(|w| w.id), Some("0x2".into()));
        assert_eq!(find_in(list.clone(), "editor", "renamed").map(|w| w.id), Some("0x3".into()), "the only one of its class");
        assert_eq!(find_in(list, "term", "renamed"), None, "two of that class: which one is not known");
    }

    #[test]
    fn nothing_is_found_without_a_socket_of_this_users() {
        let d = std::env::temp_dir().join(format!("ygg-wm-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("hypr/sig")).unwrap();
        let env = |hypr: Option<&str>, wl: Option<&str>| {
            let d = d.clone();
            let (hypr, wl) = (hypr.map(OsString::from), wl.map(OsString::from));
            move |k: &str| match k {
                "XDG_RUNTIME_DIR" => Some(d.clone().into_os_string()),
                "HYPRLAND_INSTANCE_SIGNATURE" => hypr.clone(),
                "WAYLAND_DISPLAY" => wl.clone(),
                _ => None,
            }
        };
        assert_eq!(Desk::from_env(env(Some("sig"), Some("wayland-1"))), None, "no socket");
        // a plain file where the socket should be is not spoken to
        std::fs::write(d.join("hypr/sig/.socket.sock"), "").unwrap();
        assert_eq!(Desk::from_env(env(Some("sig"), None)), None);
        std::fs::remove_file(d.join("hypr/sig/.socket.sock")).unwrap();
        let _h = std::os::unix::net::UnixListener::bind(d.join("hypr/sig/.socket.sock")).unwrap();
        assert!(Desk::from_env(env(Some("sig"), Some("wayland-1"))).is_some_and(|d| d.is_hyprland()));
        assert_eq!(Desk::from_env(env(Some("../sig"), None)), None, "a signature is a name, not a path");
        // the own compositor's socket for this display wins over Hyprland's
        let _o = std::os::unix::net::UnixListener::bind(d.join("yggdrasil-compositor-wayland-1.ctl")).unwrap();
        assert!(matches!(Desk::from_env(env(Some("sig"), Some("wayland-1"))), Some(Desk::Own(_))));
        assert!(Desk::from_env(env(Some("sig"), Some("wayland-2"))).is_some_and(|d| d.is_hyprland()), "another display's is not ours");
        let _ = std::fs::remove_dir_all(&d);
    }
}
