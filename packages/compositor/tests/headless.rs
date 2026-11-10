//! The headless backend, end to end: the compositor is started with `--headless` in a private runtime folder (never the running
//! desktop's), and a small Wayland client in this test talks to it like a panel and a program would.
//!
//! - the outputs appear with the size, scale, name and place asked for (wl_output and xdg-output)
//! - a client maps a layer surface (a bar) and a toplevel (a window), and the compositor lists them
//! - the screenshot is not blank: the bar's red and the window's blue are where they were put
//! - a size or scale change through the control socket reaches the clients (output events, a new configure, the fractional scale)

use std::fs::File;
use std::io::{BufRead, BufReader, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer::WlBuffer, wl_compositor::WlCompositor, wl_output, wl_output::WlOutput, wl_registry::WlRegistry, wl_shm, wl_shm::WlShm, wl_shm_pool::WlShmPool, wl_surface::WlSurface};
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, QueueHandle};
use wayland_protocols::wp::fractional_scale::v1::client::{wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1, wp_fractional_scale_v1, wp_fractional_scale_v1::WpFractionalScaleV1};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_surface::XdgSurface, xdg_toplevel, xdg_toplevel::XdgToplevel, xdg_wm_base, xdg_wm_base::XdgWmBase};
use wayland_protocols::xdg::xdg_output::zv1::client::{zxdg_output_manager_v1::ZxdgOutputManagerV1, zxdg_output_v1, zxdg_output_v1::ZxdgOutputV1};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_shell_v1::ZwlrLayerShellV1, zwlr_layer_surface_v1, zwlr_layer_surface_v1::ZwlrLayerSurfaceV1};

// ------------------------------------------------------------------ the compositor

struct Compositor {
    child: Child,
    dir: PathBuf,
    wayland: PathBuf,
    ctl: PathBuf,
}

impl Compositor {
    /// Start `yggdrasil-compositor --headless ARGS` in a folder of its own (short: a socket path must fit in 108 bytes).
    fn start(args: &[&str]) -> Compositor {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ygg-hl-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("run")).unwrap();
        std::fs::create_dir_all(dir.join("home")).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(dir.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut child = Command::new(env!("CARGO_BIN_EXE_yggdrasil-compositor"))
            .arg("--headless")
            .args(args)
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("WAYLAND_SOCKET")
            .env_remove("DISPLAY")
            .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
            .env_remove("XDG_CONFIG_HOME")
            .env("XDG_RUNTIME_DIR", dir.join("run"))
            .env("HOME", dir.join("home"))
            .env("YGGDRASIL_COMPOSITOR_XWAYLAND", "0")
            .stdout(Stdio::piped())
            .spawn()
            .expect("start the compositor");
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
        // keep reading (and dropping) what it prints, so it never blocks on a full pipe
        std::thread::spawn(move || for _ in lines {});
        Compositor { child, dir, wayland: wayland.unwrap(), ctl: ctl.unwrap() }
    }

    /// One command on the control socket: the lines before "ok", or the error.
    fn ctl(&self, cmd: &str) -> Result<Vec<String>, String> {
        let mut s = UnixStream::connect(&self.ctl).map_err(|e| format!("the control socket: {e}"))?;
        s.write_all(format!("{cmd}\n").as_bytes()).map_err(|e| e.to_string())?;
        let mut lines: Vec<String> = BufReader::new(s).lines().map(|l| l.unwrap()).collect();
        match lines.pop().as_deref() {
            Some("ok") => Ok(lines),
            Some(e) => Err(e.to_string()),
            None => Err("no answer".into()),
        }
    }

    fn connect(&self) -> Client {
        Client::connect(&self.wayland)
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

// ------------------------------------------------------------------ the client

#[derive(Default, Debug, Clone)]
struct Out {
    name: String,
    mode: (i32, i32),
    scale: i32,
    logical: (i32, i32),
    position: (i32, i32),
}

#[derive(Default)]
struct State {
    outputs: Vec<Out>,
    /// the layer surface's configures (width, height), each one acked
    layer_sizes: Vec<(u32, u32)>,
    toplevel_configured: bool,
    toplevel_size: (i32, i32),
    /// what the fractional scale object said last (in 120ths)
    preferred_scale: Option<u32>,
    closed: bool,
}

struct Client {
    queue: EventQueue<State>,
    qh: QueueHandle<State>,
    state: State,
    compositor: WlCompositor,
    shm: WlShm,
    layer_shell: ZwlrLayerShellV1,
    wm_base: XdgWmBase,
    fractional: WpFractionalScaleManagerV1,
    wl_outputs: Vec<WlOutput>,
    dir: PathBuf,
}

impl Client {
    fn connect(socket: &Path) -> Client {
        let conn = Connection::from_socket(UnixStream::connect(socket).expect("the Wayland socket")).unwrap();
        let (globals, queue) = registry_queue_init::<State>(&conn).unwrap();
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 4..=6, ()).expect("wl_compositor");
        let shm: WlShm = globals.bind(&qh, 1..=1, ()).expect("wl_shm");
        let layer_shell: ZwlrLayerShellV1 = globals.bind(&qh, 1..=4, ()).expect("wlr-layer-shell");
        let wm_base: XdgWmBase = globals.bind(&qh, 1..=5, ()).expect("xdg-shell");
        let fractional: WpFractionalScaleManagerV1 = globals.bind(&qh, 1..=1, ()).expect("fractional scale");
        let xdg_mgr: ZxdgOutputManagerV1 = globals.bind(&qh, 1..=3, ()).expect("xdg-output");
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
        let mut c = Client { queue, qh, state, compositor, shm, layer_shell, wm_base, fractional, wl_outputs, dir };
        c.roundtrip();
        c.roundtrip();
        c
    }

    fn roundtrip(&mut self) {
        self.queue.roundtrip(&mut self.state).expect("roundtrip");
    }

    /// Round trips until `done` holds (or 10 s pass).
    fn until(&mut self, what: &str, done: impl Fn(&State) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !done(&self.state) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            self.roundtrip();
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// A buffer of one colour (ARGB), in a file the compositor maps.
    fn buffer(&self, w: i32, h: i32, argb: u32) -> WlBuffer {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = self.dir.join(format!("buf-{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
        let mut f = File::options().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
        let px = argb.to_le_bytes();
        let data: Vec<u8> = std::iter::repeat_n(px, (w * h) as usize).flatten().collect();
        f.write_all(&data).unwrap();
        let _ = std::fs::remove_file(&path);
        let pool = self.shm.create_pool(f.as_fd(), w * h * 4, &self.qh, ());
        let b = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, &self.qh, ());
        pool.destroy();
        b
    }

    /// A bar on the Top layer, anchored to the top, left and right edges (or all four with `height` 0) of the first output.
    fn layer(&mut self, height: u32) -> (WlSurface, ZwlrLayerSurfaceV1) {
        let surface = self.compositor.create_surface(&self.qh, ());
        let _fs = self.fractional.get_fractional_scale(&surface, &self.qh, ());
        let ls = self.layer_shell.get_layer_surface(&surface, Some(&self.wl_outputs[0]), zwlr_layer_shell_v1::Layer::Top, "test-bar".into(), &self.qh, ());
        use zwlr_layer_surface_v1::Anchor;
        let anchor = if height == 0 { Anchor::Top | Anchor::Bottom | Anchor::Left | Anchor::Right } else { Anchor::Top | Anchor::Left | Anchor::Right };
        ls.set_anchor(anchor);
        ls.set_size(0, height);
        surface.commit();
        self.until("the layer surface's first configure", |s| !s.layer_sizes.is_empty());
        (surface, ls)
    }
}

impl Dispatch<WlRegistry, GlobalListContents> for State {
    fn event(_: &mut Self, _: &WlRegistry, _: wayland_client::protocol::wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
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

impl Dispatch<ZwlrLayerSurfaceV1, ()> for State {
    fn event(st: &mut Self, ls: &ZwlrLayerSurfaceV1, event: zwlr_layer_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, width, height } => {
                ls.ack_configure(serial);
                st.layer_sizes.push((width, height));
            }
            zwlr_layer_surface_v1::Event::Closed => st.closed = true,
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

impl Dispatch<XdgSurface, ()> for State {
    fn event(st: &mut Self, xs: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            xs.ack_configure(serial);
            st.toplevel_configured = true;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(st: &mut Self, _: &XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_toplevel::Event::Configure { width, height, .. } = event {
            st.toplevel_size = (width, height);
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

delegate_noop!(State: ignore WlCompositor);
delegate_noop!(State: ignore WlShm);
delegate_noop!(State: ignore WlShmPool);
delegate_noop!(State: ignore WlBuffer);
delegate_noop!(State: ignore WlSurface);
delegate_noop!(State: ignore ZwlrLayerShellV1);
delegate_noop!(State: ignore ZxdgOutputManagerV1);
delegate_noop!(State: ignore WpFractionalScaleManagerV1);

// ------------------------------------------------------------------ screenshots

struct Picture {
    w: usize,
    h: usize,
    rgb: Vec<u8>,
}

impl Picture {
    fn load(path: &Path) -> Picture {
        let decoder = png::Decoder::new(File::open(path).expect("the screenshot"));
        let mut reader = decoder.read_info().unwrap();
        let mut rgb = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut rgb).unwrap();
        assert_eq!(info.color_type, png::ColorType::Rgb);
        Picture { w: info.width as usize, h: info.height as usize, rgb }
    }
    fn at(&self, x: usize, y: usize) -> (u8, u8, u8) {
        let i = (y * self.w + x) * 3;
        (self.rgb[i], self.rgb[i + 1], self.rgb[i + 2])
    }
}

/// "x,y" and "wxh" from the control socket's lists.
fn pair(s: &str, sep: char) -> (i32, i32) {
    let (a, b) = s.split_once(sep).unwrap();
    (a.parse().unwrap(), b.parse().unwrap())
}

// ------------------------------------------------------------------ the tests

#[test]
fn outputs_appear_with_the_size_and_scale_asked_for() {
    let comp = Compositor::start(&["--output", "1600x900", "--output", "1280x720@1.5"]);
    let c = comp.connect();
    let outs = &c.state.outputs;
    assert_eq!(outs.len(), 2, "two outputs: {outs:?}");
    assert_eq!(outs[0].name, "HEADLESS-1");
    assert_eq!(outs[0].mode, (1600, 900));
    assert_eq!(outs[0].scale, 1);
    assert_eq!(outs[0].logical, (1600, 900));
    assert_eq!(outs[0].position, (0, 0));
    assert_eq!(outs[1].name, "HEADLESS-2");
    assert_eq!(outs[1].mode, (1280, 720));
    assert_eq!(outs[1].scale, 2, "wl_output rounds 1.5 up");
    assert_eq!(outs[1].logical, (853, 480), "xdg-output: 1280x720 at 1.5");
    assert_eq!(outs[1].position, (1600, 0), "side by side");

    let listed = comp.ctl("outputs").unwrap();
    assert_eq!(listed, ["HEADLESS-1 1600x900 scale=1 at=0,0 logical=1600x900", "HEADLESS-2 1280x720 scale=1.5 at=1600,0 logical=853x480"]);

    // several outputs of one size
    let comp = Compositor::start(&["--outputs", "3", "--size", "640x480", "--scale", "2"]);
    let c = comp.connect();
    assert_eq!(c.state.outputs.len(), 3);
    assert!(c.state.outputs.iter().all(|o| o.mode == (640, 480) && o.scale == 2 && o.logical == (320, 240)));
    assert_eq!(c.state.outputs[2].position, (640, 0));
}

#[test]
fn a_panel_and_a_window_are_mapped_and_drawn() {
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();

    // the bar: red, 800x40 across the top
    let (bar, _ls) = c.layer(40);
    assert_eq!(c.state.layer_sizes[0], (800, 40));
    let red = c.buffer(800, 40, 0xffff0000);
    bar.attach(Some(&red), 0, 0);
    bar.damage_buffer(0, 0, 800, 40);
    bar.commit();

    // the window: blue, 200x150
    let surface = c.compositor.create_surface(&c.qh, ());
    let xs = c.wm_base.get_xdg_surface(&surface, &c.qh, ());
    let top = xs.get_toplevel(&c.qh, ());
    top.set_app_id("org.yggdrasil.test".into());
    top.set_title("Headless test".into());
    surface.commit();
    c.until("the window's first configure", |s| s.toplevel_configured);
    let blue = c.buffer(200, 150, 0xff0000ff);
    surface.attach(Some(&blue), 0, 0);
    surface.damage_buffer(0, 0, 200, 150);
    surface.commit();
    c.roundtrip();
    c.roundtrip();

    let layers = comp.ctl("layers").unwrap();
    assert!(layers.iter().any(|l| l == "HEADLESS-1\ttest-bar\tTop\t0,0\t800x40"), "the bar is mapped: {layers:?}");
    let windows = comp.ctl("windows").unwrap();
    let win = windows.iter().find(|l| l.starts_with("org.yggdrasil.test\tHeadless test\t")).unwrap_or_else(|| panic!("the window is mapped: {windows:?}"));
    let parts: Vec<&str> = win.split('\t').collect();
    let (x, y) = pair(parts[2], ',');
    assert_eq!(pair(parts[3], 'x'), (200, 150));

    let shot = comp.dir.join("shot.png");
    comp.ctl(&format!("screenshot HEADLESS-1 {}", shot.display())).unwrap();
    let pic = Picture::load(&shot);
    assert_eq!((pic.w, pic.h), (800, 600));
    assert_eq!(pic.at(400, 20), (255, 0, 0), "the bar's red at the top");
    assert_eq!(pic.at(5, 5), (255, 0, 0));
    let (cx, cy) = ((x + 100) as usize, (y + 75) as usize);
    assert!(cy > 40 && cy < 600, "the window is below the bar: {x},{y}");
    assert_eq!(pic.at(cx, cy), (0, 0, 255), "the window's blue at its middle ({cx},{cy})");
    // the background elsewhere: neither of the two
    let bg = pic.at(790, 590);
    assert!(bg != (255, 0, 0) && bg != (0, 0, 255), "the background shows where nothing is: {bg:?}");

    // errors are answered, not ignored
    assert!(comp.ctl("screenshot HEADLESS-9 /nowhere.png").unwrap_err().contains("no output"));
    assert!(comp.ctl("frobnicate").unwrap_err().contains("unknown command"));
}

#[test]
fn a_resize_and_a_scale_change_reach_the_clients() {
    let comp = Compositor::start(&["--size", "800x600"]);
    let mut c = comp.connect();
    let (_bg, _ls) = c.layer(0);
    assert_eq!(c.state.layer_sizes.last(), Some(&(800, 600)));
    assert_eq!(c.state.preferred_scale, Some(120), "fractional scale 1.0 (in 120ths)");

    comp.ctl("resize HEADLESS-1 1024x768").unwrap();
    c.until("the new size", |s| s.layer_sizes.last() == Some(&(1024, 768)));
    assert_eq!(c.state.outputs[0].mode, (1024, 768));
    assert_eq!(c.state.outputs[0].logical, (1024, 768));
    assert_eq!(comp.ctl("outputs").unwrap(), ["HEADLESS-1 1024x768 scale=1 at=0,0 logical=1024x768"]);

    comp.ctl("scale HEADLESS-1 2").unwrap();
    c.until("the new scale", |s| s.layer_sizes.last() == Some(&(512, 384)));
    c.until("the preferred scale", |s| s.preferred_scale == Some(240));
    assert_eq!(c.state.outputs[0].scale, 2);
    assert_eq!(c.state.outputs[0].logical, (512, 384));

    comp.ctl("scale HEADLESS-1 1.25").unwrap();
    c.until("a fractional scale", |s| s.preferred_scale == Some(150));
    assert!(comp.ctl("resize HEADLESS-1 huge").is_err());
    assert!(!c.state.closed);
}

#[test]
fn the_screenshot_follows_the_scale() {
    // a bar 30 logical pixels high on an output at scale 2 covers 60 rows of the picture, all of its width
    let comp = Compositor::start(&["--size", "640x480", "--scale", "2"]);
    let mut c = comp.connect();
    let (bar, _ls) = c.layer(30);
    assert_eq!(c.state.layer_sizes[0], (320, 30));
    let green = c.buffer(640, 60, 0xff00ff00);
    bar.set_buffer_scale(2);
    bar.attach(Some(&green), 0, 0);
    bar.damage_buffer(0, 0, 640, 60);
    bar.commit();
    c.roundtrip();
    let shot = comp.dir.join("scaled.png");
    comp.ctl(&format!("screenshot HEADLESS-1 {}", shot.display())).unwrap();
    let pic = Picture::load(&shot);
    assert_eq!((pic.w, pic.h), (640, 480));
    assert_eq!(pic.at(639, 59), (0, 255, 0));
    assert_eq!(pic.at(0, 0), (0, 255, 0));
    assert_ne!(pic.at(320, 61), (0, 255, 0), "nothing of the bar below its 60 rows");
}

#[test]
fn quit_ends_it_and_leaves_no_socket_behind() {
    let mut comp = Compositor::start(&[]);
    comp.ctl("quit").unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while comp.child.try_wait().unwrap().is_none() {
        assert!(Instant::now() < deadline, "it did not end");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!comp.wayland.exists(), "the Wayland socket is removed");
    assert!(!comp.ctl.exists(), "the control socket is removed");
}

#[test]
fn no_runtime_folder_is_refused() {
    // (the desktop's own /run/user/UID is refused too: a unit test in headless.rs, so this test never starts anything there)
    let out = Command::new(env!("CARGO_BIN_EXE_yggdrasil-compositor"))
        .arg("--headless")
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env_remove("HYPRLAND_INSTANCE_SIGNATURE")
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("XDG_RUNTIME_DIR"));
}
