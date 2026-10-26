//! yggdrasil-greeter: the login screen. greetd starts it inside a small compositor (cage, or Yggdrasil's own later) and it draws
//! the same designs as the lock screen; what is typed goes to greetd (greetd.rs), which runs PAM and starts the session.
//!
//! Settings: /etc/yggdrasil/greeter.conf (`design = split`, `session = yggdrasil`, `wallpaper = /path/to/picture`). The session is the
//! name of a file in /usr/share/wayland-sessions (as other login screens list them), or a command; without one, the first session there.
//! Tab (or Down) and Shift+Tab (or Up) switch user while nothing is typed; Enter signs in; Escape clears what was typed.
//! Exit status: 0 the session was started (greetd takes over), 2 setup problem.

mod design;
mod draw;
mod greetd;
mod state;

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

use calloop::channel::{channel, Channel, Event as ChanEvent, Sender};
use calloop::timer::{TimeoutAction, Timer};
use calloop::EventLoop;
use calloop_wayland_source::WaylandSource;
use memmap2::MmapMut;
use tiny_skia::Pixmap;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::{self, WlBuffer},
    wl_compositor::WlCompositor,
    wl_keyboard::{self, WlKeyboard},
    wl_registry,
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};
use xkbcommon::xkb;

use greetd::{Fail, Greetd};
use state::{Action, Key, Lock};

struct User {
    name: String,
    avatar: Option<Pixmap>,
}

struct App {
    qh: QueueHandle<App>,
    conn: Connection,
    shm: WlShm,
    surface: WlSurface,
    size: (u32, u32),
    configured: bool,
    xkb: Option<xkb::State>,
    users: Vec<User>,
    sel: usize,
    lock_state: Lock,
    fonts: draw::Fonts,
    design: design::Design,
    wallpaper: Option<Pixmap>,
    walls: std::collections::HashMap<(u32, u32), (Pixmap, Pixmap)>,
    /// The sessions to choose from (F2), and the chosen one.
    sessions: Vec<Session>,
    sess_sel: usize,
    /// A sign-in is being checked: keys are ignored until it answers.
    busy: bool,
    tx: Sender<Result<(), Fail>>,
    done: Option<i32>,
    #[cfg(feature = "test-auth")]
    test_keys: Option<String>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(App: WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlSurface);

impl Dispatch<wl_buffer::WlBuffer, ()> for App {
    fn event(_: &mut Self, b: &WlBuffer, e: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = e {
            b.destroy();
        }
    }
}

impl Dispatch<XdgWmBase, ()> for App {
    fn event(_: &mut Self, base: &XdgWmBase, e: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = e {
            base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for App {
    fn event(app: &mut Self, s: &XdgSurface, e: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = e {
            s.ack_configure(serial);
            app.configured = true;
            app.draw();
        }
    }
}

impl Dispatch<XdgToplevel, ()> for App {
    fn event(app: &mut Self, _: &XdgToplevel, e: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            xdg_toplevel::Event::Configure { width, height, .. } if width > 0 && height > 0 => app.size = (width as u32, height as u32),
            // a login screen has no way to be closed
            xdg_toplevel::Event::Close => {}
            _ => {}
        }
    }
}

impl Dispatch<WlSeat, ()> for App {
    fn event(_: &mut Self, seat: &WlSeat, e: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(c) } = e {
            if c.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(qh, ());
            }
        }
    }
}

impl Dispatch<WlKeyboard, ()> for App {
    fn event(app: &mut Self, _: &WlKeyboard, e: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            wl_keyboard::Event::Keymap { format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                let ctx = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
                // SAFETY: the compositor hands over a read-only descriptor of exactly `size` bytes.
                if let Ok(Some(km)) = unsafe { xkb::Keymap::new_from_fd(&ctx, fd, size as usize, xkb::KEYMAP_FORMAT_TEXT_V1, xkb::KEYMAP_COMPILE_NO_FLAGS) } {
                    app.xkb = Some(xkb::State::new(&km));
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                if let Some(x) = &mut app.xkb {
                    x.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => app.key(key),
            _ => {}
        }
    }
}

impl App {
    fn key(&mut self, code: u32) {
        let Some(x) = &self.xkb else { return };
        let kc = xkb::Keycode::new(code + 8);
        let sym = x.key_get_one_sym(kc).raw();
        match sym {
            0xff09 | 0xff54 => return self.switch_user(1),  // Tab, Down
            0xfe20 | 0xff52 => return self.switch_user(-1), // Shift+Tab, Up
            0xffbf => return self.switch_session(),         // F2
            _ => {}
        }
        let k = match sym {
            0xff0d | 0xff8d => Key::Enter,
            0xff08 => Key::Backspace,
            0xff1b => Key::Escape,
            _ => match x.key_get_utf8(kc).chars().next() {
                Some(c) => Key::Char(c),
                None => return,
            },
        };
        self.apply_key(k);
    }

    /// F2: the next session, when more than one is installed.
    fn switch_session(&mut self) {
        if self.busy || self.sessions.len() < 2 {
            return;
        }
        self.sess_sel = (self.sess_sel + 1) % self.sessions.len();
        self.draw();
    }

    fn switch_user(&mut self, by: i32) {
        if self.busy || self.lock_state.dots() > 0 || self.users.len() < 2 {
            return;
        }
        let n = self.users.len() as i32;
        self.sel = ((self.sel as i32 + by).rem_euclid(n)) as usize;
        self.lock_state.message.clear();
        self.draw();
    }

    fn apply_key(&mut self, k: Key) {
        if self.busy {
            return;
        }
        match self.lock_state.key(k, Instant::now()) {
            Action::Nothing => {}
            Action::Redraw => self.draw(),
            Action::Submit(mut pw) => {
                let chosen = &self.sessions[self.sess_sel];
                let (user, cmd, env, tx) = (self.users[self.sel].name.clone(), chosen.cmd.clone(), chosen.env.clone(), self.tx.clone());
                self.busy = true;
                self.lock_state.message = "Signing in".into();
                self.draw();
                // PAM can take a while (and waits on purpose after a wrong password): never on the thread that draws
                std::thread::spawn(move || {
                    let r = Greetd::connect().map_err(Fail::Other).and_then(|mut g| g.login(&user, &pw, &cmd, &env));
                    zeroize::Zeroize::zeroize(&mut pw);
                    let _ = tx.send(r);
                });
            }
        }
    }

    fn answered(&mut self, r: Result<(), Fail>) {
        self.busy = false;
        match r {
            Ok(()) => self.done = Some(0),
            Err(Fail::Auth(_)) => {
                self.lock_state.result(false, Instant::now());
                self.draw();
            }
            Err(Fail::Other(e)) => {
                eprintln!("yggdrasil-greeter: {e}");
                self.lock_state.message = "Cannot sign in right now".into();
                self.draw();
            }
        }
    }

    fn draw(&mut self) {
        let (w, h) = self.size;
        if !self.configured || w == 0 || h == 0 {
            return;
        }
        let Some(mut pix) = Pixmap::new(w, h) else { return };
        let key = (w, h);
        if !self.walls.contains_key(&key) {
            if let Some(c) = self.wallpaper.as_ref().and_then(|p| design::cover(p, w, h)) {
                if let Some(b) = design::blur(&c) {
                    self.walls.insert(key, (c, b));
                }
            }
        }
        let (time, date) = design::clock_text();
        let user = &self.users[self.sel];
        let hint = if self.lock_state.message.is_empty() && self.lock_state.dots() == 0 { hint_text(&self.sessions, self.sess_sel, self.users.len()) } else { String::new() };
        let message = if hint.is_empty() { self.lock_state.message.as_str() } else { hint.as_str() };
        let scene = design::Scene {
            fonts: &self.fonts,
            user: &user.name,
            dots: self.lock_state.dots(),
            waiting: self.lock_state.waiting(Instant::now()),
            message,
            hint: !hint.is_empty(),
            time,
            date,
            greeting: design::greeting(design::current_hour()),
            wall: self.walls.get(&key).map(|x| &x.0),
            blurred: self.walls.get(&key).map(|x| &x.1),
            avatar: user.avatar.as_ref(),
            accent: design::accent(),
        };
        design::paint(self.design, &mut pix, &scene);
        let stride = w * 4;
        let size = (stride * h) as usize;
        let Ok(fd) = rustix::fs::memfd_create(c"yggdrasil-greeter", rustix::fs::MemfdFlags::CLOEXEC) else { return };
        if rustix::fs::ftruncate(&fd, size as u64).is_err() {
            return;
        }
        let file = std::fs::File::from(fd);
        let Ok(mut map) = (unsafe { MmapMut::map_mut(&file) }) else { return };
        for (dst, src) in map.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let pool = self.shm.create_pool(file.as_fd(), size as i32, &self.qh, ());
        let buf = pool.create_buffer(0, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        self.surface.attach(Some(&buf), 0, 0);
        self.surface.damage_buffer(0, 0, w as i32, h as i32);
        self.surface.commit();
    }
}

/// `key = value` lines of /etc/yggdrasil/greeter.conf (`GREETER_CONF` overrides the path, for tests).
fn conf(key: &str) -> Option<String> {
    let path = std::env::var("GREETER_CONF").unwrap_or_else(|_| "/etc/yggdrasil/greeter.conf".into());
    std::fs::read_to_string(path).ok()?.lines().filter_map(|l| l.split('#').next()?.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())
}

/// Normal accounts a person can sign in to: user ids from 1000, with a real shell. `YGGDRASIL_GREETER_USERS=a,b` names them for tests.
fn list_users() -> Vec<String> {
    if let Ok(v) = std::env::var("YGGDRASIL_GREETER_USERS") {
        return v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect();
    }
    let mut out = vec![];
    for l in std::fs::read_to_string("/etc/passwd").unwrap_or_default().lines() {
        let p: Vec<&str> = l.split(':').collect();
        if p.len() > 6 && p[2].parse::<u32>().map(|u| (1000..60000).contains(&u)).unwrap_or(false) && !p[6].ends_with("nologin") && !p[6].ends_with("false") {
            out.push(p[0].to_string());
        }
    }
    out
}

/// The account picture the desktop keeps where every program can read it (AccountsService), else none.
fn avatar_of(user: &str) -> Option<Pixmap> {
    ["/var/lib/AccountsService/icons/", "/usr/share/yggdrasil/avatars/"].iter().map(|d| std::path::PathBuf::from(d).join(user)).find(|p| p.is_file()).and_then(|p| design::load_picture(&p))
}

fn die(msg: &str) -> ! {
    eprintln!("yggdrasil-greeter: {msg}");
    std::process::exit(2);
}

/// Where Wayland sessions describe themselves (`NAME.desktop`: Exec, DesktopNames).
const SESSIONS: &str = "/usr/share/wayland-sessions";

/// A session description file: its command (field codes such as %U removed) and the environment greetd should give it.
pub fn parse_session(text: &str) -> Option<(Vec<String>, Vec<String>)> {
    let mut exec = None;
    let mut names = String::new();
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry {
            continue;
        }
        if let Some(v) = line.strip_prefix("Exec=") {
            exec = Some(v.split_whitespace().filter(|w| !(w.starts_with('%') && w.len() == 2)).map(String::from).collect::<Vec<_>>());
        } else if let Some(v) = line.strip_prefix("DesktopNames=") {
            names = v.trim_end_matches(';').to_string();
        }
    }
    let cmd = exec.filter(|c| !c.is_empty())?;
    let mut env = vec!["XDG_SESSION_TYPE=wayland".to_string()];
    if !names.is_empty() {
        env.push(format!("XDG_CURRENT_DESKTOP={}", names.replace(';', ":")));
        env.push(format!("XDG_SESSION_DESKTOP={}", names.split(';').next().unwrap_or("")));
    }
    Some((cmd, env))
}

/// A session to sign in to: its name as shown, and what greetd starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub name: String,
    pub cmd: Vec<String>,
    pub env: Vec<String>,
}

/// The line under the password box while nothing is typed: which session, and the keys that change things.
pub fn hint_text(sessions: &[Session], sel: usize, users: usize) -> String {
    let mut parts: Vec<String> = vec![];
    if let Some(s) = sessions.get(sel) {
        parts.push(if sessions.len() > 1 { format!("{} session \u{b7} F2 to change", s.name) } else { format!("{} session", s.name) });
    }
    if users > 1 {
        parts.push("Tab: another user".into());
    }
    parts.join(" \u{b7} ")
}

/// A session file's shown name (Name=).
fn session_name(text: &str) -> Option<String> {
    let mut in_entry = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
        } else if in_entry {
            if let Some(v) = line.strip_prefix("Name=") {
                return Some(v.trim().to_string());
            }
        }
    }
    None
}

/// The sessions to offer: every file in `dir` but the compositor bake-off's and bare Hyprland's (without Yggdrasil it is a black
/// screen with no way out); the configured one first. A configured command that is not a file is offered as itself.
pub fn sessions_in(dir: &std::path::Path, chosen: Option<&str>) -> Vec<Session> {
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "desktop")).collect()).unwrap_or_default();
    files.sort();
    let mut out: Vec<(String, Session)> = vec![];
    for p in files {
        let stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        if stem.starts_with("bakeoff-") || stem.starts_with("hyprland") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&p) else { continue };
        if let Some((cmd, env)) = parse_session(&text) {
            out.push((stem.clone(), Session { name: session_name(&text).unwrap_or(stem), cmd, env }));
        }
    }
    let chosen = chosen.map(str::trim).filter(|c| !c.is_empty());
    if let Some(c) = chosen {
        let name = c.trim_end_matches(".desktop");
        if let Some(i) = out.iter().position(|(stem, _)| stem == name) {
            let first = out.remove(i);
            out.insert(0, first);
        } else {
            let (cmd, env) = session_for(Some(c), dir);
            out.insert(0, (String::new(), Session { name: c.to_string(), cmd, env }));
        }
    }
    let mut list: Vec<Session> = out.into_iter().map(|(_, s)| s).collect();
    if list.is_empty() {
        let (cmd, env) = session_for(None, dir);
        list.push(Session { name: "Yggdrasil".into(), cmd, env });
    }
    list
}

/// The session to start: `chosen` names a file in `dir` (with or without .desktop) or is a command; without it, the first file there.
pub fn session_for(chosen: Option<&str>, dir: &std::path::Path) -> (Vec<String>, Vec<String>) {
    let from_file = |p: std::path::PathBuf| std::fs::read_to_string(p).ok().and_then(|t| parse_session(&t));
    if let Some(c) = chosen.map(str::trim).filter(|c| !c.is_empty()) {
        let name = c.trim_end_matches(".desktop");
        if !name.contains('/') && !name.contains(' ') {
            if let Some(s) = from_file(dir.join(format!("{name}.desktop"))) {
                return s;
            }
        }
        return (c.split_whitespace().map(String::from).collect(), vec!["XDG_SESSION_TYPE=wayland".to_string()]);
    }
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "desktop")).collect()).unwrap_or_default();
    files.sort();
    // no session files at all: Yggdrasil's own session
    files.into_iter().find_map(from_file).unwrap_or_else(|| {
        (vec!["yggdrasil-session".to_string()], vec!["XDG_SESSION_TYPE=wayland".to_string(), "XDG_CURRENT_DESKTOP=Yggdrasil".to_string(), "XDG_SESSION_DESKTOP=Yggdrasil".to_string()])
    })
}

fn main() {
    let conn = Connection::connect_to_env().unwrap_or_else(|e| die(&format!("no Wayland display: {e}")));
    let (globals, queue) = registry_queue_init::<App>(&conn).unwrap_or_else(|e| die(&e.to_string()));
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap_or_else(|_| die("no wl_compositor"));
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).unwrap_or_else(|_| die("no wl_shm"));
    let _seat: WlSeat = globals.bind(&qh, 1..=7, ()).unwrap_or_else(|_| die("no wl_seat"));
    let wm: XdgWmBase = globals.bind(&qh, 1..=6, ()).unwrap_or_else(|_| die("no xdg_wm_base"));
    let fonts = draw::Fonts::load().unwrap_or_else(|| die("no fonts found (fc-match sans-serif)"));

    let users: Vec<User> = list_users().into_iter().map(|name| User { avatar: avatar_of(&name), name }).collect();
    if users.is_empty() {
        die("no accounts to sign in to");
    }
    let wallpaper = conf("wallpaper").map(std::path::PathBuf::from).filter(|p| p.is_file()).or_else(yggdrasil_conf::integrations::default_wallpaper).and_then(|p| design::load_picture(&p));
    let sessions = sessions_in(std::path::Path::new(SESSIONS), conf("session").as_deref());

    let surface = compositor.create_surface(&qh, ());
    let xs = wm.get_xdg_surface(&surface, &qh, ());
    let top = xs.get_toplevel(&qh, ());
    top.set_title("Sign in".into());
    top.set_app_id("yggdrasil-greeter".into());
    top.set_fullscreen(None);
    surface.commit();

    let (tx, rx): (Sender<Result<(), Fail>>, Channel<Result<(), Fail>>) = channel();
    let mut app = App {
        qh: qh.clone(),
        conn: conn.clone(),
        shm,
        surface,
        size: (1280, 720),
        configured: false,
        xkb: None,
        users,
        sel: 0,
        lock_state: Lock::default(),
        fonts,
        design: conf("design").and_then(|d| design::Design::parse(&d)).unwrap_or(design::Design::Split),
        wallpaper,
        walls: Default::default(),
        sessions,
        sess_sel: 0,
        busy: false,
        tx,
        done: None,
        #[cfg(feature = "test-auth")]
        test_keys: std::env::var("YGGDRASIL_GREETER_TEST_KEYS").ok(),
    };
    let _ = &app.conn;
    let mut event_loop: EventLoop<App> = EventLoop::try_new().unwrap_or_else(|_| die("no event loop"));
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).unwrap_or_else(|_| die("cannot watch the Wayland connection"));
    event_loop.handle().insert_source(rx, |e, _, app| if let ChanEvent::Msg(r) = e { app.answered(r) }).unwrap_or_else(|_| die("no channel"));
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(1)), |_, _, app| {
            app.draw(); // the clock
            #[cfg(feature = "test-auth")]
            if app.configured {
                if let Some(keys) = app.test_keys.take() {
                    for c in keys.chars() {
                        app.apply_key(if c == '\n' { Key::Enter } else { Key::Char(c) });
                    }
                }
            }
            TimeoutAction::ToDuration(Duration::from_secs(1))
        })
        .unwrap_or_else(|_| die("no timer"));
    let status = loop {
        if event_loop.dispatch(Some(Duration::from_millis(200)), &mut app).is_err() {
            break 1;
        }
        if let Some(code) = app.done {
            break code;
        }
    };
    std::process::exit(status);
}

#[cfg(test)]
mod session_tests {
    #[test]
    fn sessions_offered_configured_first_without_the_bakeoff() {
        let d = std::env::temp_dir().join(format!("ygg-session-list-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("plasma.desktop"), "[Desktop Entry]\nName=Plasma (Wayland)\nExec=startplasma-wayland\nDesktopNames=KDE\n").unwrap();
        std::fs::write(d.join("yggdrasil.desktop"), "[Desktop Entry]\nName=Yggdrasil\nExec=/usr/bin/yggdrasil-session\nDesktopNames=Yggdrasil\n").unwrap();
        std::fs::write(d.join("bakeoff-kwin.desktop"), "[Desktop Entry]\nName=Bake-off\nExec=x\n").unwrap();
        std::fs::write(d.join("hyprland.desktop"), "[Desktop Entry]\nName=Hyprland\nExec=Hyprland\n").unwrap();
        let l = super::sessions_in(&d, Some("yggdrasil"));
        assert_eq!(l.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), vec!["Yggdrasil", "Plasma (Wayland)"]);
        assert_eq!(super::hint_text(&l, 0, 1), "Yggdrasil session \u{b7} F2 to change");
        assert_eq!(super::hint_text(&l, 1, 2), "Plasma (Wayland) session \u{b7} F2 to change \u{b7} Tab: another user");
        let _ = std::fs::remove_dir_all(&d);
    }

    use super::*;

    #[test]
    fn a_session_file_gives_command_and_environment() {
        let t = "[Desktop Entry]\nName=Plasma\nExec=/usr/lib/plasma-dbus-run-session-if-needed /usr/bin/startplasma-wayland %U\nDesktopNames=KDE;Plasma\n";
        let (cmd, env) = parse_session(t).unwrap();
        assert_eq!(cmd, vec!["/usr/lib/plasma-dbus-run-session-if-needed", "/usr/bin/startplasma-wayland"]);
        assert_eq!(env, vec!["XDG_SESSION_TYPE=wayland", "XDG_CURRENT_DESKTOP=KDE:Plasma", "XDG_SESSION_DESKTOP=KDE"]);
        assert!(parse_session("[Desktop Entry]\nName=x\n").is_none(), "no Exec");
    }

    #[test]
    fn the_session_is_chosen_by_name_or_command_or_the_first_one() {
        let d = std::env::temp_dir().join(format!("ygg-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("b.desktop"), "[Desktop Entry]\nExec=start-b\nDesktopNames=B\n").unwrap();
        std::fs::write(d.join("a.desktop"), "[Desktop Entry]\nExec=start-a\nDesktopNames=A\n").unwrap();
        assert_eq!(session_for(Some("b"), &d).0, vec!["start-b"]);
        assert_eq!(session_for(Some("b.desktop"), &d).1[1], "XDG_CURRENT_DESKTOP=B");
        assert_eq!(session_for(Some("my-session --flag"), &d).0, vec!["my-session", "--flag"], "a command");
        assert_eq!(session_for(None, &d).0, vec!["start-a"], "the first, in name order");
        assert_eq!(session_for(Some("../etc/x"), &d).0, vec!["../etc/x"], "never a path out of the folder");
        let _ = std::fs::remove_dir_all(&d);
    }
}
