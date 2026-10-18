//! labyrinth-greeter: the login screen. greetd starts it inside a small compositor (cage, or Labyrinth's own later) and it draws
//! the same designs as the lock screen; what is typed goes to greetd (greetd.rs), which runs PAM and starts the session.
//!
//! Settings: /etc/labyrinth/greeter.conf (`design = split`, `session = startplasma-wayland`, `wallpaper = /path/to/picture`).
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
    session: Vec<String>,
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
                let (user, cmd, tx) = (self.users[self.sel].name.clone(), self.session.clone(), self.tx.clone());
                self.busy = true;
                self.lock_state.message = "Signing in".into();
                self.draw();
                // PAM can take a while (and waits on purpose after a wrong password): never on the thread that draws
                std::thread::spawn(move || {
                    let r = Greetd::connect().map_err(Fail::Other).and_then(|mut g| g.login(&user, &pw, &cmd, &[]));
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
                eprintln!("labyrinth-greeter: {e}");
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
        let hint = if self.users.len() > 1 && self.lock_state.message.is_empty() && self.lock_state.dots() == 0 { "Tab: another user" } else { "" };
        let message = if hint.is_empty() { self.lock_state.message.as_str() } else { hint };
        let scene = design::Scene {
            fonts: &self.fonts,
            user: &user.name,
            dots: self.lock_state.dots(),
            waiting: self.lock_state.waiting(Instant::now()),
            message,
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
        let Ok(fd) = rustix::fs::memfd_create(c"labyrinth-greeter", rustix::fs::MemfdFlags::CLOEXEC) else { return };
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

/// `key = value` lines of /etc/labyrinth/greeter.conf (`GREETER_CONF` overrides the path, for tests).
fn conf(key: &str) -> Option<String> {
    let path = std::env::var("GREETER_CONF").unwrap_or_else(|_| "/etc/labyrinth/greeter.conf".into());
    std::fs::read_to_string(path).ok()?.lines().filter_map(|l| l.split('#').next()?.split_once('=')).find(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_string())
}

/// Normal accounts a person can sign in to: user ids from 1000, with a real shell. `LABYRINTH_GREETER_USERS=a,b` names them for tests.
fn list_users() -> Vec<String> {
    if let Ok(v) = std::env::var("LABYRINTH_GREETER_USERS") {
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
    ["/var/lib/AccountsService/icons/", "/usr/share/labyrinth/avatars/"].iter().map(|d| std::path::PathBuf::from(d).join(user)).find(|p| p.is_file()).and_then(|p| design::load_picture(&p))
}

fn die(msg: &str) -> ! {
    eprintln!("labyrinth-greeter: {msg}");
    std::process::exit(2);
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
    let wallpaper = conf("wallpaper").map(std::path::PathBuf::from).filter(|p| p.is_file()).or_else(|| Some(std::path::PathBuf::from("/usr/share/labyrinth/wallpapers/labyrinth-purple.png"))).and_then(|p| design::load_picture(&p));
    let session: Vec<String> = conf("session").unwrap_or_else(|| "startplasma-wayland".into()).split_whitespace().map(String::from).collect();

    let surface = compositor.create_surface(&qh, ());
    let xs = wm.get_xdg_surface(&surface, &qh, ());
    let top = xs.get_toplevel(&qh, ());
    top.set_title("Labyrinth sign-in".into());
    top.set_app_id("labyrinth-greeter".into());
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
        session,
        busy: false,
        tx,
        done: None,
        #[cfg(feature = "test-auth")]
        test_keys: std::env::var("LABYRINTH_GREETER_TEST_KEYS").ok(),
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
