//! labyrinth-lock: the screen locker.
//!
//!   labyrinth-lock          lock the screen now; returns (exit 0) when the right password was typed
//!
//! It speaks the standard Wayland session-lock protocol (`ext-session-lock-v1`): the compositor shows nothing of the session
//! except what this program draws, and keeps the session locked even if this program crashes or is killed. A password is checked
//! through PAM (see auth.rs); Escape only clears what was typed; a few wrong passwords in a row make you wait.
//!
//! Exit status: 0 unlocked, 1 the compositor refused or ended the lock without an unlock, 2 setup problem.

mod auth;
mod draw;
mod state;

use std::os::fd::AsFd;
use std::time::{Duration, Instant};

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
    wl_output::WlOutput,
    wl_registry,
    wl_seat::{self, WlSeat},
    wl_shm::{self, WlShm},
    wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1,
    ext_session_lock_surface_v1::{self, ExtSessionLockSurfaceV1},
    ext_session_lock_v1::{self, ExtSessionLockV1},
};
use xkbcommon::xkb;

use auth::Authenticator;
use state::{Action, Key, Lock};

struct LockSurf {
    wl: WlSurface,
    ls: ExtSessionLockSurfaceV1,
    w: u32,
    h: u32,
}

struct App {
    qh: QueueHandle<App>,
    conn: Connection,
    shm: WlShm,
    lock: ExtSessionLockV1,
    surfaces: Vec<LockSurf>,
    locked: bool,
    lock_state: Lock,
    xkb: Option<xkb::State>,
    auth: Box<dyn Authenticator>,
    user: String,
    fonts: draw::Fonts,
    /// Set when the program should end, with its exit status.
    done: Option<i32>,
    /// Tests only (feature `test-auth`): keys to type by themselves once the screen is locked, from LABYRINTH_LOCK_TEST_KEYS.
    #[cfg(feature = "test-auth")]
    test_keys: Option<(Instant, String)>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(App: WlCompositor);
delegate_noop!(App: ignore WlShm);
delegate_noop!(App: ignore WlShmPool);
delegate_noop!(App: ignore WlSurface);
delegate_noop!(App: ignore WlOutput);
delegate_noop!(App: ExtSessionLockManagerV1);

impl Dispatch<wl_buffer::WlBuffer, ()> for App {
    fn event(_: &mut Self, b: &WlBuffer, e: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = e {
            b.destroy(); // each frame has its own buffer: it is done with once the compositor lets go
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for App {
    fn event(app: &mut Self, _: &ExtSessionLockV1, e: ext_session_lock_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match e {
            ext_session_lock_v1::Event::Locked => {
                app.locked = true;
            }
            ext_session_lock_v1::Event::Finished => {
                // refused (another locker is running) or ended by the compositor
                eprintln!("labyrinth-lock: the compositor ended the lock{}", if app.locked { "" } else { " (another locker may be running)" });
                app.done = Some(1);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, usize> for App {
    fn event(app: &mut Self, ls: &ExtSessionLockSurfaceV1, e: ext_session_lock_surface_v1::Event, idx: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, width, height } = e {
            ls.ack_configure(serial);
            if let Some(s) = app.surfaces.get_mut(*idx) {
                s.w = width;
                s.h = height;
            }
            app.draw(*idx);
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
        if !self.locked {
            return;
        }
        let Some(x) = &self.xkb else { return };
        let kc = xkb::Keycode::new(code + 8);
        let sym = x.key_get_one_sym(kc);
        let k = match sym.raw() {
            0xff0d | 0xff8d => Key::Enter,         // Return, KP_Enter
            0xff08 => Key::Backspace,              // BackSpace
            0xff1b => Key::Escape,                 // Escape
            _ => match x.key_get_utf8(kc).chars().next() {
                Some(c) => Key::Char(c),
                None => return,
            },
        };
        self.apply_key(k);
    }

    /// What a key does once it is known (shared by the keyboard and the test injection).
    fn apply_key(&mut self, k: Key) {
        match self.lock_state.key(k, Instant::now()) {
            Action::Nothing => {}
            Action::Redraw => self.redraw_all(),
            Action::Submit(mut pw) => {
                let ok = self.auth.check(&self.user, &pw).unwrap_or(false);
                zeroize::Zeroize::zeroize(&mut pw);
                self.lock_state.result(ok, Instant::now());
                if ok {
                    self.lock.unlock_and_destroy();
                    let _ = self.conn.roundtrip(); // the compositor must see the unlock before this program ends
                    self.done = Some(0);
                } else {
                    self.redraw_all();
                }
            }
        }
    }

    fn redraw_all(&mut self) {
        for i in 0..self.surfaces.len() {
            self.draw(i);
        }
    }

    fn draw(&mut self, idx: usize) {
        let (w, h) = (self.surfaces[idx].w, self.surfaces[idx].h);
        if w == 0 || h == 0 {
            return;
        }
        let Some(mut pix) = Pixmap::new(w, h) else { return };
        self.paint(&mut pix);
        // a shared-memory buffer holding the picture, handed to the compositor
        let stride = w * 4;
        let size = (stride * h) as usize;
        let Ok(fd) = rustix::fs::memfd_create(c"labyrinth-lock", rustix::fs::MemfdFlags::CLOEXEC) else { return };
        if rustix::fs::ftruncate(&fd, size as u64).is_err() {
            return;
        }
        let file = std::fs::File::from(fd);
        let Ok(mut map) = (unsafe { MmapMut::map_mut(&file) }) else { return };
        // tiny-skia is premultiplied RGBA; the Argb8888 buffer wants BGRA bytes
        for (dst, src) in map.chunks_exact_mut(4).zip(pix.data().chunks_exact(4)) {
            dst[0] = src[2];
            dst[1] = src[1];
            dst[2] = src[0];
            dst[3] = src[3];
        }
        let pool = self.shm.create_pool(file.as_fd(), size as i32, &self.qh, ());
        let buf = pool.create_buffer(0, w as i32, h as i32, stride as i32, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        let s = &self.surfaces[idx];
        s.wl.attach(Some(&buf), 0, 0);
        s.wl.damage_buffer(0, 0, w as i32, h as i32);
        s.wl.commit();
    }

    fn paint(&self, pix: &mut Pixmap) {
        let (w, h) = (pix.width() as f32, pix.height() as f32);
        pix.fill(tiny_skia::Color::from_rgba8(10, 10, 16, 255));
        let (time, date) = clock_text();
        let f = &self.fonts;
        let tw = draw::text_width(&f.bold, 104.0, &time);
        draw::draw_text(pix, &f.bold, 104.0, (w - tw) / 2.0, h * 0.30, (240, 240, 246, 255), &time);
        let dw = draw::text_width(&f.regular, 24.0, &date);
        draw::draw_text(pix, &f.regular, 24.0, (w - dw) / 2.0, h * 0.30 + 82.0, (160, 160, 176, 255), &date);
        // the password field
        let (fw, fh) = (380.0, 52.0);
        let (fx, fy) = ((w - fw) / 2.0, h * 0.52);
        let waiting = self.lock_state.waiting(Instant::now());
        draw::fill_rounded(pix, fx, fy, fw, fh, 14.0, (28, 28, 38, 255));
        let n = self.lock_state.dots();
        if n == 0 {
            let hint = if waiting { "Please wait…" } else { "Password" };
            draw::draw_text(pix, &f.regular, 18.0, fx + 20.0, fy + fh / 2.0, (120, 120, 138, 255), hint);
        } else {
            let shown = n.min(22);
            let total = shown as f32 * 20.0;
            for i in 0..shown {
                draw::fill_circle(pix, fx + (fw - total) / 2.0 + i as f32 * 20.0 + 10.0, fy + fh / 2.0, 6.0, (240, 240, 246, 255));
            }
        }
        let uw = draw::text_width(&f.bold, 18.0, &self.user);
        draw::draw_text(pix, &f.bold, 18.0, (w - uw) / 2.0, fy - 30.0, (200, 200, 214, 255), &self.user);
        if !self.lock_state.message.is_empty() {
            let m = &self.lock_state.message;
            let mw = draw::text_width(&f.regular, 16.0, m);
            draw::draw_text(pix, &f.regular, 16.0, (w - mw) / 2.0, fy + fh + 34.0, (232, 84, 84, 255), m);
        }
    }
}

/// The time ("14:05") and date ("Friday, 16 October 2026") in the user's locale and time zone.
fn clock_text() -> (String, String) {
    let fmt = |f: &str| -> String {
        let mut buf = [0u8; 96];
        unsafe {
            let t = libc::time(std::ptr::null_mut());
            let mut tm: libc::tm = std::mem::zeroed();
            libc::localtime_r(&t, &mut tm);
            let cf = std::ffi::CString::new(f).unwrap();
            let n = libc::strftime(buf.as_mut_ptr() as *mut libc::c_char, buf.len(), cf.as_ptr(), &tm);
            String::from_utf8_lossy(&buf[..n]).into_owned()
        }
    };
    (fmt("%H:%M"), fmt("%A, %-d %B %Y"))
}

fn user_name() -> String {
    std::env::var("USER").ok().filter(|u| !u.is_empty()).unwrap_or_else(|| {
        let mut buf = vec![0u8; 4096];
        let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
        let mut res: *mut libc::passwd = std::ptr::null_mut();
        let rc = unsafe { libc::getpwuid_r(libc::getuid(), &mut pw, buf.as_mut_ptr() as *mut libc::c_char, buf.len(), &mut res) };
        if rc == 0 && !res.is_null() {
            unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned()
        } else {
            String::new()
        }
    })
}

fn main() {
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("labyrinth-lock: no Wayland display: {e}");
            std::process::exit(2);
        }
    };
    let (globals, mut queue) = match registry_queue_init::<App>(&conn) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("labyrinth-lock: {e}");
            std::process::exit(2);
        }
    };
    let qh = queue.handle();
    let compositor: WlCompositor = globals.bind(&qh, 1..=6, ()).unwrap_or_else(|_| die("no wl_compositor"));
    let shm: WlShm = globals.bind(&qh, 1..=1, ()).unwrap_or_else(|_| die("no wl_shm"));
    let _seat: WlSeat = globals.bind(&qh, 1..=7, ()).unwrap_or_else(|_| die("no wl_seat"));
    let manager: ExtSessionLockManagerV1 = globals.bind(&qh, 1..=1, ()).unwrap_or_else(|_| die("the compositor does not offer ext-session-lock-v1"));
    let outputs: Vec<WlOutput> = globals.contents().with_list(|l| l.iter().filter(|g| g.interface == "wl_output").map(|g| globals.registry().bind::<WlOutput, _, _>(g.name, g.version.min(4), &qh, ())).collect());
    let fonts = draw::Fonts::load().unwrap_or_else(|| die("no fonts found (fc-match sans-serif)"));

    #[cfg(feature = "test-auth")]
    let authenticator: Box<dyn Authenticator> = Box::new(auth::Fake);
    #[cfg(not(feature = "test-auth"))]
    let authenticator: Box<dyn Authenticator> = Box::new(auth::Pam { service: std::env::var("LABYRINTH_LOCK_PAM").unwrap_or_else(|_| "login".into()) });

    // Lock first, then cover every output with a lock surface (the compositor says "locked" once they are shown).
    let lock = manager.lock(&qh, ());
    let mut app = App { qh: qh.clone(), conn: conn.clone(), shm, lock: lock.clone(), surfaces: vec![], locked: false, lock_state: Lock::default(), xkb: None, auth: authenticator, user: user_name(), fonts, done: None, #[cfg(feature = "test-auth")] test_keys: std::env::var("LABYRINTH_LOCK_TEST_KEYS").ok().map(|k| (Instant::now(), k)) };
    for (i, out) in outputs.iter().enumerate() {
        let wl = compositor.create_surface(&qh, ());
        let ls = lock.get_lock_surface(&wl, out, &qh, i);
        app.surfaces.push(LockSurf { wl, ls, w: 0, h: 0 });
    }
    if app.surfaces.is_empty() {
        lock.unlock_and_destroy();
        let _ = conn.roundtrip();
        die("no outputs to lock");
    }

    let mut event_loop: EventLoop<App> = EventLoop::try_new().unwrap_or_else(|_| die("no event loop"));
    let signal = event_loop.get_signal();
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).unwrap_or_else(|_| die("cannot watch the Wayland connection"));
    // the clock
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(1)), |_, _, app| {
            if app.locked {
                app.redraw_all();
                #[cfg(feature = "test-auth")]
                if let Some((_, keys)) = app.test_keys.take() {
                    for c in keys.chars() {
                        let k = if c == '\n' { Key::Enter } else if c == '\u{8}' { Key::Backspace } else { Key::Char(c) };
                        app.apply_key(k);
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
        let _ = &signal;
    };
    std::process::exit(status);
}

fn die(msg: &str) -> ! {
    eprintln!("labyrinth-lock: {msg}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_clock_has_two_parts() {
        let (t, d) = super::clock_text();
        assert_eq!(t.len(), 5);
        assert!(d.contains(' '));
    }
}
