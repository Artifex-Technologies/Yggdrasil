//! What the headless tests share: the compositor started in a private folder (never the running desktop's), a small Wayland
//! client that speaks the protocols Yggdrasil's programs use, screenshots, and a time limit for every test.
//!
//! Nothing here opens a screen, a GPU or an input device: the compositor draws in memory (pixman), and keys and the pointer are
//! made inside it through its control socket. Every process started here dies with the test (PR_SET_PDEATHSIG), and a test that
//! runs past its limit ends the whole test program, so a hang never outlives CI's step.
#![allow(dead_code)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use wayland_client::backend::ObjectId;
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_keyboard, wl_keyboard::WlKeyboard, wl_output, wl_output::WlOutput, wl_pointer,
    wl_pointer::WlPointer, wl_registry::WlRegistry, wl_seat, wl_seat::WlSeat, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool,
    wl_surface::WlSurface,
};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1, ext_idle_notification_v1::ExtIdleNotificationV1, ext_idle_notifier_v1::ExtIdleNotifierV1,
};
use wayland_protocols::ext::session_lock::v1::client::{
    ext_session_lock_manager_v1::ExtSessionLockManagerV1, ext_session_lock_surface_v1,
    ext_session_lock_surface_v1::ExtSessionLockSurfaceV1, ext_session_lock_v1, ext_session_lock_v1::ExtSessionLockV1,
};
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1, wp_fractional_scale_v1::WpFractionalScaleV1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface, xdg_surface::XdgSurface, xdg_toplevel, xdg_toplevel::XdgToplevel, xdg_wm_base, xdg_wm_base::XdgWmBase,
};
use wayland_protocols::xdg::xdg_output::zv1::client::{
    zxdg_output_manager_v1::ZxdgOutputManagerV1, zxdg_output_v1, zxdg_output_v1::ZxdgOutputV1,
};
use wayland_protocols_wlr::layer_shell::v1::client::{
    zwlr_layer_shell_v1::ZwlrLayerShellV1, zwlr_layer_surface_v1, zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
};
use wayland_protocols_wlr::output_power_management::v1::client::{
    zwlr_output_power_manager_v1::ZwlrOutputPowerManagerV1, zwlr_output_power_v1, zwlr_output_power_v1::ZwlrOutputPowerV1,
};
use wayland_protocols_wlr::screencopy::v1::client::{
    zwlr_screencopy_frame_v1, zwlr_screencopy_frame_v1::ZwlrScreencopyFrameV1, zwlr_screencopy_manager_v1::ZwlrScreencopyManagerV1,
};

pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
pub use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{Anchor, KeyboardInteractivity};

// ------------------------------------------------------------------ time limits and children

/// Ends the whole test program when a test runs past its limit (its children die with it). Dropped at the end of the test.
pub struct Limit(Arc<AtomicBool>);

pub fn limit(secs: u64, name: &'static str) -> Limit {
    let done = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    std::thread::spawn(move || {
        let end = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < end {
            if d.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        eprintln!("test {name}: still running after {secs} s, ending the test program");
        std::process::exit(124);
    });
    Limit(done)
}

impl Drop for Limit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

/// A child that is killed when the thread that started it ends (the test), even if the test panics or is cut off.
pub fn dies_with_test(cmd: &mut Command) -> &mut Command {
    // SAFETY: prctl is async-signal-safe and touches nothing of the parent
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL);
            Ok(())
        })
    }
}

/// The variables that lead to the running desktop: never passed on.
pub const DESKTOP_VARS: &[&str] = &[
    "WAYLAND_DISPLAY",
    "WAYLAND_SOCKET",
    "DISPLAY",
    "HYPRLAND_INSTANCE_SIGNATURE",
    "SWAYSOCK",
    "I3SOCK",
    "NIRI_SOCKET",
    "DBUS_SESSION_BUS_ADDRESS",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_STATE_HOME",
    "XDG_CACHE_HOME",
    "XDG_CURRENT_DESKTOP",
    "XDG_SESSION_DESKTOP",
    "YGGDRASIL_COMPOSITOR_CTL",
];

// ------------------------------------------------------------------ the compositor

pub struct Compositor {
    pub child: Child,
    pub dir: PathBuf,
    pub wayland: PathBuf,
    pub ctl: PathBuf,
    /// what it printed (both streams), for a failing test to show
    pub log: Arc<std::sync::Mutex<Vec<String>>>,
}

impl Compositor {
    /// Start `yggdrasil-compositor --headless ARGS` in a folder of its own (short: a socket path must fit in 108 bytes).
    pub fn start(args: &[&str]) -> Compositor {
        Compositor::start_with(args, |_| {})
    }

    /// The same, with `setup` run on the test's folder first (its `home` holds the compositor's config: home/.config/yggdrasil).
    pub fn start_with(args: &[&str], setup: impl FnOnce(&Path)) -> Compositor {
        Compositor::start_env(args, &[], setup)
    }

    /// The same, with variables of the test's own for the compositor (PATH: the programs its shortcuts find).
    ///
    /// Every test's compositor starts with "reduce motion" on (panel.conf, as Settings writes it), so windows show at once and
    /// screenshots need no waiting; the animation tests (tests/look.rs) remove that line in `setup`.
    pub fn start_env(args: &[&str], env: &[(&str, &str)], setup: impl FnOnce(&Path)) -> Compositor {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ygg-hl-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::create_dir_all(dir.join("home/.config/yggdrasil")).unwrap();
        std::fs::write(dir.join("home/.config/yggdrasil/panel.conf"), "reduce_motion = yes\n").unwrap();
        setup(&dir);
        // YGGDRASIL_COMPOSITOR: another build of it (tests/apps-podman.sh runs the tests in a container)
        let bin = std::env::var_os("YGGDRASIL_COMPOSITOR")
            .map(PathBuf::from)
            .unwrap_or_else(|| env!("CARGO_BIN_EXE_yggdrasil-compositor").into());
        let mut cmd = Command::new(bin);
        cmd.arg("--headless").args(args);
        for v in DESKTOP_VARS {
            cmd.env_remove(v);
        }
        cmd.env("XDG_RUNTIME_DIR", dir.join("run"))
            .env("HOME", dir.join("home"))
            .env("YGGDRASIL_COMPOSITOR_XWAYLAND", "0")
            .envs(env.iter().copied())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let mut child = dies_with_test(&mut cmd).spawn().expect("start the compositor");
        let log = Arc::new(std::sync::Mutex::new(vec![]));
        let l2 = log.clone();
        let err = child.stderr.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(err).lines().map_while(Result::ok) {
                l2.lock().unwrap().push(line);
            }
        });
        let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let (mut wayland, mut ctl) = (None, None);
        while wayland.is_none() || ctl.is_none() {
            let line = lines.next().expect("the compositor ended before it was ready").unwrap();
            if let Some(v) = line.strip_prefix("WAYLAND_DISPLAY=") {
                wayland = Some(dir.join("run").join(v));
            } else if let Some(v) = line.strip_prefix("YGGDRASIL_COMPOSITOR_CTL=") {
                ctl = Some(PathBuf::from(v));
            }
        }
        // keep reading what it prints, so it never blocks on a full pipe
        let l3 = log.clone();
        std::thread::spawn(move || {
            for line in lines.map_while(Result::ok) {
                l3.lock().unwrap().push(line);
            }
        });
        Compositor { child, dir, wayland: wayland.unwrap(), ctl: ctl.unwrap(), log }
    }

    /// One command on the control socket: the lines before "ok", or the error.
    pub fn ctl(&self, cmd: &str) -> Result<Vec<String>, String> {
        let mut s = UnixStream::connect(&self.ctl).map_err(|e| format!("the control socket: {e}"))?;
        s.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        s.write_all(format!("{cmd}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut lines: Vec<String> = BufReader::new(s).lines().map_while(Result::ok).collect();
        match lines.pop().as_deref() {
            Some("ok") => Ok(lines),
            Some(e) => Err(e.to_string()),
            None => Err("no answer".into()),
        }
    }

    /// A command that must succeed.
    pub fn must(&self, cmd: &str) -> Vec<String> {
        self.ctl(cmd).unwrap_or_else(|e| panic!("{cmd}: {e}"))
    }

    pub fn connect(&self) -> Client {
        Client::connect(&self.wayland)
    }

    /// The windows as the control socket lists them.
    pub fn windows(&self) -> Vec<WinInfo> {
        self.must("windows").iter().map(|l| WinInfo::parse(l)).collect()
    }

    pub fn window(&self, app_id: &str) -> Option<WinInfo> {
        self.windows().into_iter().find(|w| w.app_id == app_id)
    }

    pub fn layers(&self) -> Vec<LayerInfo> {
        self.must("layers").iter().map(|l| LayerInfo::parse(l)).collect()
    }

    pub fn focus(&self) -> String {
        self.must("focus").join("\n")
    }

    /// Wait (up to `secs`) until `done` holds for the compositor; `what` names it in the failure.
    pub fn wait(&self, secs: u64, what: &str, done: impl Fn(&Compositor) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(secs);
        while !done(self) {
            if Instant::now() > deadline {
                panic!(
                    "timed out waiting for {what}\nwindows: {:?}\nlayers: {:?}\nfocus: {}\ncompositor log (end):\n{}",
                    self.ctl("windows"),
                    self.ctl("layers"),
                    self.focus(),
                    self.log_tail(30)
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    pub fn log_tail(&self, n: usize) -> String {
        let l = self.log.lock().unwrap();
        l[l.len().saturating_sub(n)..].join("\n")
    }

    pub fn screenshot(&self, output: &str) -> Picture {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = self.dir.join(format!("shot-{}.png", N.fetch_add(1, Ordering::SeqCst)));
        self.must(&format!("screenshot {output} {}", path.display()));
        Picture::load(&path)
    }
}

impl Drop for Compositor {
    fn drop(&mut self) {
        let _ = self.ctl("quit");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A line of `windows`: app id, title, place, size, workspace (1-based), states.
#[derive(Debug, Clone)]
pub struct WinInfo {
    pub app_id: String,
    pub title: String,
    pub at: (i32, i32),
    pub size: (i32, i32),
    pub workspace: u32,
    pub states: Vec<String>,
}

impl WinInfo {
    fn parse(l: &str) -> WinInfo {
        let p: Vec<&str> = l.split('\t').collect();
        assert_eq!(p.len(), 6, "a windows line: {l:?}");
        WinInfo {
            app_id: p[0].into(),
            title: p[1].into(),
            at: pair(p[2], ','),
            size: pair(p[3], 'x'),
            workspace: p[4].parse().unwrap(),
            states: p[5].split(',').map(String::from).collect(),
        }
    }
    pub fn has(&self, state: &str) -> bool {
        self.states.iter().any(|s| s == state)
    }
    pub fn rect(&self) -> (i32, i32, i32, i32) {
        (self.at.0, self.at.1, self.size.0, self.size.1)
    }
}

/// A line of `layers`: output, namespace, layer, place, size.
#[derive(Debug, Clone)]
pub struct LayerInfo {
    pub output: String,
    pub namespace: String,
    pub layer: String,
    pub at: (i32, i32),
    pub size: (i32, i32),
}

impl LayerInfo {
    fn parse(l: &str) -> LayerInfo {
        let p: Vec<&str> = l.split('\t').collect();
        assert_eq!(p.len(), 5, "a layers line: {l:?}");
        LayerInfo { output: p[0].into(), namespace: p[1].into(), layer: p[2].into(), at: pair(p[3], ','), size: pair(p[4], 'x') }
    }
}

/// "x,y" and "wxh" from the control socket's lists.
pub fn pair(s: &str, sep: char) -> (i32, i32) {
    let (a, b) = s.split_once(sep).unwrap_or_else(|| panic!("{s:?} is not a pair"));
    (a.parse().unwrap(), b.parse().unwrap())
}

// ------------------------------------------------------------------ the client

#[derive(Default, Debug, Clone)]
pub struct Out {
    pub name: String,
    pub mode: (i32, i32),
    pub scale: i32,
    pub logical: (i32, i32),
    pub position: (i32, i32),
}

/// One xdg toplevel's side of things.
#[derive(Default, Debug, Clone)]
pub struct Win {
    pub configured: bool,
    /// the last configure's size (0, 0: the client chooses)
    pub size: (i32, i32),
    /// the last configure's states (xdg_toplevel::State values)
    pub states: Vec<u32>,
    pub close_asked: bool,
}

/// One screen copy's side of things (wlr-screencopy).
#[derive(Default, Debug, Clone)]
pub struct CopyState {
    /// format, width, height, stride
    pub buffer: Option<(u32, u32, u32, u32)>,
    pub buffer_done: bool,
    pub damage: Vec<(u32, u32, u32, u32)>,
    pub ready: bool,
    pub failed: bool,
}

#[derive(Default)]
pub struct State {
    pub outputs: Vec<Out>,
    /// every layer surface's configures (width, height), each acked, in the order the surfaces were made
    pub layer_sizes: Vec<(u32, u32)>,
    pub layer_configures: HashMap<usize, Vec<(u32, u32)>>,
    pub layer_closed: bool,
    pub wins: Vec<Win>,
    /// the last toplevel's first configure arrived, and its size (kept for the older tests)
    pub toplevel_configured: bool,
    pub toplevel_size: (i32, i32),
    /// what the fractional scale object said last (in 120ths)
    pub preferred_scale: Option<u32>,
    pub closed: bool,
    /// the surface the keyboard is on (wl_keyboard enter/leave), and the keys pressed while there: (surface, kernel code)
    pub kb_focus: Option<ObjectId>,
    pub keys: Vec<(ObjectId, u32)>,
    pub keymap: bool,
    pub pointer_on: Option<ObjectId>,
    /// ext-session-lock
    pub locked: bool,
    pub lock_finished: bool,
    pub lock_sizes: HashMap<usize, (u32, u32)>,
    /// wlr-screencopy
    pub copies: Vec<CopyState>,
    /// wlr-output-power: the last mode per control (0 off, 1 on), and failures
    pub power: HashMap<usize, u32>,
    /// ext-idle-notify: idled and resumed events, per notification
    pub idled: HashMap<usize, u32>,
    pub resumed: HashMap<usize, u32>,
}

pub struct Client {
    pub conn: Connection,
    pub queue: EventQueue<State>,
    pub qh: QueueHandle<State>,
    pub state: State,
    pub globals: GlobalList,
    pub compositor: WlCompositor,
    pub shm: WlShm,
    pub layer_shell: ZwlrLayerShellV1,
    pub wm_base: XdgWmBase,
    pub fractional: WpFractionalScaleManagerV1,
    pub wl_outputs: Vec<WlOutput>,
    pub seat: WlSeat,
    pub dir: PathBuf,
}

/// A window of the test's own: an xdg toplevel with one colour.
pub struct Window {
    pub index: usize,
    pub surface: WlSurface,
    pub xdg: XdgSurface,
    pub toplevel: XdgToplevel,
}

pub struct LayerSurf {
    pub index: usize,
    pub surface: WlSurface,
    pub layer: ZwlrLayerSurfaceV1,
}

impl Client {
    pub fn connect(socket: &Path) -> Client {
        let conn = Connection::from_socket(UnixStream::connect(socket).expect("the Wayland socket")).unwrap();
        let (globals, queue) = registry_queue_init::<State>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).expect("wlr-layer-shell");
        let wm_base: XdgWmBase = globals.bind(&qh, 1..=5, ()).expect("xdg-shell");
        let fractional: WpFractionalScaleManagerV1 = globals.bind(&qh, 1..=1, ()).expect("fractional scale");
        let xdg_mgr: ZxdgOutputManagerV1 = globals.bind(&qh, 1..=3, ()).expect("xdg-output");
        let seat: WlSeat = globals.bind(&qh, 1..=7, ()).expect("wl_seat");
        let mut state = State::default();
        let mut wl_outputs = vec![];
        for g in globals.contents().clone_list().into_iter().filter(|g| g.interface == "wl_output") {
            let i = state.outputs.len();
            state.outputs.push(Out::default());
            let o: WlOutput = globals.registry().bind(g.name, g.version.min(4), &qh, i);
            xdg_mgr.get_xdg_output(&o, &qh, i);
            wl_outputs.push(o);
        }
        let dir = socket.parent().unwrap().to_path_buf();
        let mut c = Client { conn, queue, qh, state, globals, compositor, shm, layer_shell, wm_base, fractional, wl_outputs, seat, dir };
        c.roundtrip();
        c.roundtrip();
        c
    }

    pub fn roundtrip(&mut self) {
        self.queue.roundtrip(&mut self.state).expect("roundtrip");
    }

    /// Round trips until `done` holds (or 10 s pass).
    pub fn until(&mut self, what: &str, done: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&self.state) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.roundtrip();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Take the keyboard and the pointer (the events land in `state`).
    pub fn input(&mut self) -> (WlKeyboard, WlPointer) {
        let k = self.seat.get_keyboard(&self.qh, ());
        let p = self.seat.get_pointer(&self.qh, ());
        self.roundtrip();
        (k, p)
    }

    /// A buffer of one colour (ARGB), in a file the compositor maps.
    pub fn buffer(&self, w: i32, h: i32, argb: u32) -> WlBuffer {
        let (pool, f) = self.pool(w * h * 4);
        let px = argb.to_le_bytes();
        let data: Vec<u8> = std::iter::repeat_n(px, (w * h) as usize).flatten().collect();
        (&f).write_all(&data).unwrap();
        let b = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        b
    }

    /// A shared-memory pool of `size` bytes, and its file.
    pub fn pool(&self, size: i32) -> (WlShmPool, File) {
        static N: AtomicUsize = AtomicUsize::new(0);
        let path = self.dir.join(format!("buf-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
        let f = File::options().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        f.set_len(size as u64).unwrap();
        let _ = std::fs::remove_file(&path);
        let pool = self.shm.create_pool(f.as_fd(), size, &self.qh, ());
        (pool, f)
    }

    /// A bar on the Top layer, anchored to the top, left and right edges (or all four with `height` 0) of the first output.
    pub fn layer(&mut self, height: u32) -> (WlSurface, ZwlrLayerSurfaceV1) {
        let anchor = if height == 0 {
            Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right
        } else {
            Anchor::Top | Anchor::Left | Anchor::Right
        };
        let l = self.layer_with(0, Layer::Top, "test-bar", anchor, (0, height), 0, KeyboardInteractivity::None);
        (l.surface, l.layer)
    }

    /// A layer surface: on output `output`, the layer, namespace, anchor, size, exclusive zone and keyboard wish; returns when it
    /// has its first configure (not yet drawn: see `draw_layer`).
    #[allow(clippy::too_many_arguments)]
    pub fn layer_with(
        &mut self,
        output: usize,
        layer: Layer,
        namespace: &str,
        anchor: Anchor,
        size: (u32, u32),
        zone: i32,
        keyboard: KeyboardInteractivity,
    ) -> LayerSurf {
        let surface = self.compositor.create_surface(&self.qh, ());
        let _fs = self.fractional.get_fractional_scale(&surface, &self.qh, ());
        static N: AtomicUsize = AtomicUsize::new(0);
        let index = N.fetch_add(1, Ordering::SeqCst);
        let ls = self.layer_shell.get_layer_surface(&surface, Some(&self.wl_outputs[output]), layer, namespace.into(), &self.qh, index);
        ls.set_anchor(anchor);
        ls.set_size(size.0, size.1);
        ls.set_exclusive_zone(zone);
        ls.set_keyboard_interactivity(keyboard);
        surface.commit();
        self.until("the layer surface's first configure", |s| s.layer_configures.contains_key(&index));
        LayerSurf { index, surface, layer: ls }
    }

    /// Draw a layer surface in one colour at the size it was given.
    pub fn draw_layer(&mut self, l: &LayerSurf, argb: u32) {
        let (w, h) = *self.state.layer_configures[&l.index].last().unwrap();
        let b = self.buffer(w as i32, h as i32, argb);
        l.surface.attach(Some(&b), 0, 0);
        l.surface.damage_buffer(0, 0, w as i32, h as i32);
        l.surface.commit();
        self.roundtrip();
    }

    /// A window: an xdg toplevel with this app id and title. Returns after its first configure (not yet drawn: see `draw`).
    pub fn window(&mut self, app_id: &str, title: &str) -> Window {
        let index = self.state.wins.len();
        self.state.wins.push(Win::default());
        let surface = self.compositor.create_surface(&self.qh, ());
        let xdg = self.wm_base.get_xdg_surface(&surface, &self.qh, index);
        let toplevel = xdg.get_toplevel(&self.qh, index);
        toplevel.set_app_id(app_id.into());
        toplevel.set_title(title.into());
        surface.commit();
        self.until("the window's first configure", |s| s.wins[index].configured);
        Window { index, surface, xdg, toplevel }
    }

    /// Draw a window in one colour: at the size it was last given, or `size` when it may choose.
    pub fn draw(&mut self, w: &Window, size: (i32, i32), argb: u32) -> (i32, i32) {
        let given = self.state.wins[w.index].size;
        let (bw, bh) = if given.0 > 0 && given.1 > 0 { given } else { size };
        let b = self.buffer(bw, bh, argb);
        w.surface.attach(Some(&b), 0, 0);
        w.surface.damage_buffer(0, 0, bw, bh);
        w.surface.commit();
        self.roundtrip();
        (bw, bh)
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: wayland_client::protocol::wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlOutput, usize> for State {
    fn event(st: &mut Self, _: &WlOutput, event: wl_output::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let o = &mut st.outputs[*i];
        match event {
            wl_output::Event::Mode { width, height, .. } => o.mode = (width, height),
            wl_output::Event::Scale { factor } => o.scale = factor,
            wl_output::Event::Name { name } => o.name = name,
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, usize> for State {
    fn event(st: &mut Self, _: &ZxdgOutputV1, event: zxdg_output_v1::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        let o = &mut st.outputs[*i];
        match event {
            zxdg_output_v1::Event::LogicalSize { width, height } => o.logical = (width, height),
            zxdg_output_v1::Event::LogicalPosition { x, y } => o.position = (x, y),
            _ => {}
        }
    }
}

impl Dispatch<ZwlrLayerSurfaceV1, usize> for State {
    fn event(
        st: &mut Self,
        ls: &ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                ls.ack_configure(serial);
                st.layer_sizes.push((width, height));
                st.layer_configures.entry(*i).or_default().push((width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => {
                st.closed = true;
                st.layer_closed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(_: &mut Self, wm: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, usize> for State {
    fn event(st: &mut Self, xs: &XdgSurface, event: xdg_surface::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            xs.ack_configure(serial);
            st.wins[*i].configured = true;
            st.toplevel_configured = true;
        }
    }
}

impl Dispatch<XdgToplevel, usize> for State {
    fn event(st: &mut Self, _: &XdgToplevel, event: xdg_toplevel::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            xdg_toplevel::Event::Configure { width, height, states } => {
                st.wins[*i].size = (width, height);
                st.wins[*i].states = states.as_chunks::<4>().0.iter().map(|c| u32::from_ne_bytes(*c)).collect();
                st.toplevel_size = (width, height);
            }
            xdg_toplevel::Event::Close => st.wins[*i].close_asked = true,
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for State {
    fn event(st: &mut Self, _: &WpFractionalScaleV1, event: wp_fractional_scale_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            st.preferred_scale = Some(scale);
        }
    }
}

impl Dispatch<WlSeat, ()> for State {
    fn event(_: &mut Self, _: &WlSeat, _: wl_seat::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlKeyboard, ()> for State {
    fn event(st: &mut Self, _: &WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap { .. } => st.keymap = true,
            wl_keyboard::Event::Enter { surface, .. } => st.kb_focus = Some(surface.id()),
            wl_keyboard::Event::Leave { .. } => st.kb_focus = None,
            wl_keyboard::Event::Key { key, state: WEnum::Value(wl_keyboard::KeyState::Pressed), .. } => {
                if let Some(f) = &st.kb_focus {
                    st.keys.push((f.clone(), key));
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<WlPointer, ()> for State {
    fn event(st: &mut Self, _: &WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { surface, .. } => st.pointer_on = Some(surface.id()),
            wl_pointer::Event::Leave { .. } => st.pointer_on = None,
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockV1, ()> for State {
    fn event(st: &mut Self, _: &ExtSessionLockV1, event: ext_session_lock_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            ext_session_lock_v1::Event::Locked => st.locked = true,
            ext_session_lock_v1::Event::Finished => st.lock_finished = true,
            _ => {}
        }
    }
}

impl Dispatch<ExtSessionLockSurfaceV1, usize> for State {
    fn event(
        st: &mut Self,
        ls: &ExtSessionLockSurfaceV1,
        event: ext_session_lock_surface_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let ext_session_lock_surface_v1::Event::Configure { serial, width, height } = event {
            ls.ack_configure(serial);
            st.lock_sizes.insert(*i, (width, height));
        }
    }
}

impl Dispatch<ZwlrScreencopyFrameV1, usize> for State {
    fn event(
        st: &mut Self,
        _: &ZwlrScreencopyFrameV1,
        event: zwlr_screencopy_frame_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let c = &mut st.copies[*i];
        match event {
            zwlr_screencopy_frame_v1::Event::Buffer { format, width, height, stride } => {
                let f = match format {
                    WEnum::Value(f) => f as u32,
                    WEnum::Unknown(u) => u,
                };
                c.buffer = Some((f, width, height, stride));
            }
            zwlr_screencopy_frame_v1::Event::BufferDone => c.buffer_done = true,
            zwlr_screencopy_frame_v1::Event::Damage { x, y, width, height } => c.damage.push((x, y, width, height)),
            zwlr_screencopy_frame_v1::Event::Ready { .. } => c.ready = true,
            zwlr_screencopy_frame_v1::Event::Failed => c.failed = true,
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputPowerV1, usize> for State {
    fn event(st: &mut Self, _: &ZwlrOutputPowerV1, event: zwlr_output_power_v1::Event, i: &usize, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_output_power_v1::Event::Mode { mode } => {
                st.power.insert(*i, if matches!(mode, WEnum::Value(zwlr_output_power_v1::Mode::Off)) { 0 } else { 1 });
            }
            zwlr_output_power_v1::Event::Failed => {
                st.power.insert(*i, u32::MAX);
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtIdleNotificationV1, usize> for State {
    fn event(
        st: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        i: &usize,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => *st.idled.entry(*i).or_default() += 1,
            ext_idle_notification_v1::Event::Resumed => *st.resumed.entry(*i).or_default() += 1,
            _ => {}
        }
    }
}

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore ZwlrLayerShellV1);
delegate_noop!(State: ignore ZxdgOutputManagerV1);
delegate_noop!(State: ignore WpFractionalScaleManagerV1);
delegate_noop!(State: ExtSessionLockManagerV1);
delegate_noop!(State: ZwlrScreencopyManagerV1);
delegate_noop!(State: ZwlrOutputPowerManagerV1);
delegate_noop!(State: ExtIdleNotifierV1);

// ------------------------------------------------------------------ screenshots

pub struct Picture {
    pub w: usize,
    pub h: usize,
    pub rgb: Vec<u8>,
}

impl Picture {
    pub fn load(path: &Path) -> Picture {
        let decoder = png::Decoder::new(File::open(path).expect("the screenshot"));
        let mut reader = decoder.read_info().unwrap();
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).unwrap();
        let rgb = match info.color_type {
            png::ColorType::Rgb => buf[..info.buffer_size()].to_vec(),
            // grim writes RGBA
            png::ColorType::Rgba => buf[..info.buffer_size()].as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect(),
            t => panic!("a screenshot in {t:?}"),
        };
        Picture { w: info.width as usize, h: info.height as usize, rgb }
    }

    /// A picture from XRGB8888 rows (a screen copy).
    pub fn from_xrgb(w: usize, h: usize, stride: usize, data: &[u8]) -> Picture {
        let mut rgb = Vec::with_capacity(w * h * 3);
        for row in data.chunks(stride).take(h) {
            for px in row[..w * 4].as_chunks::<4>().0 {
                rgb.extend_from_slice(&[px[2], px[1], px[0]]);
            }
        }
        Picture { w, h, rgb }
    }

    pub fn at(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * self.w + x) * 3;
        (self.rgb[i], self.rgb[i + 1], self.rgb[i + 2])
    }

    /// How many pixels of a rectangle have exactly this colour.
    pub fn count(&self, (x, y, w, h): (i32, i32, i32, i32), colour: (u8, u8, u8)) -> usize {
        let mut n = 0;
        for yy in y.max(0)..(y + h).min(self.h as i32) {
            for xx in x.max(0)..(x + w).min(self.w as i32) {
                if self.at(xx as usize, yy as usize) == colour {
                    n += 1;
                }
            }
        }
        n
    }

    /// The share of a rectangle's pixels that are not `colour` (0 to 1).
    pub fn share_not(&self, r: (i32, i32, i32, i32), colour: (u8, u8, u8)) -> f64 {
        let total = (r.2.max(0) * r.3.max(0)) as f64;
        if total == 0.0 {
            return 0.0;
        }
        1.0 - self.count(r, colour) as f64 / total
    }
}

/// The colour the compositor clears its background with (0.05, 0.05, 0.08), as the screenshot has it.
pub fn background(pic: &Picture) -> (u8, u8, u8) {
    pic.at(pic.w - 1, pic.h - 1)
}
