//! The stream on a real PipeWire: a PipeWire of its own (a minimal configuration: no devices, nothing of the desktop's audio),
//! the headless compositor, yggdrasil-screencast on its screen, and a viewer here that links itself to the stream's node and
//! reads the first frame: the screen's size, and the compositor's background colour. Skipped where PipeWire or the compositor's
//! build (YGGDRASIL_COMPOSITOR, else target/debug/yggdrasil-compositor) is not there, except under
//! YGGDRASIL_TEST_SCREENCAST_REQUIRED (CI), where that fails.

use std::cell::RefCell;
use std::io::{BufRead, BufReader};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use pipewire as pw;
use pw::{properties::properties, spa, spa::pod::Pod};

const CONF: &str = "context.properties = { core.daemon = true core.name = pipewire-0 support.dbus = false mem.allow-mlock = false }
context.spa-libs = { support.* = support/libspa-support }
context.modules = [
  { name = libpipewire-module-protocol-native }
  { name = libpipewire-module-access }
  { name = libpipewire-module-client-node }
  { name = libpipewire-module-adapter }
  { name = libpipewire-module-metadata }
  { name = libpipewire-module-link-factory }
]
";

/// The first frame the viewer read: its size in bytes and its first pixels.
type Got = Rc<RefCell<Option<(usize, Vec<u8>)>>>;
/// The screen's output port, and the viewer's node with its input port.
type Ports = Rc<RefCell<(Option<u32>, Option<(u32, u32)>)>>;

struct World {
    dir: PathBuf,
    children: Vec<Child>,
}

impl Drop for World {
    fn drop(&mut self) {
        // the last started first: the streamer before its compositor and PipeWire
        for c in self.children.iter_mut().rev() {
            let _ = c.kill();
            let _ = c.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn compositor() -> Option<PathBuf> {
    let p = std::env::var_os("YGGDRASIL_COMPOSITOR")
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/yggdrasil-compositor"));
    p.is_file().then_some(p)
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(name).is_file()))
}

/// The first line of a child's output that starts with `prefix`, within `secs`.
fn line_with(child: &mut Child, prefix: &str, secs: u64) -> Option<String> {
    let out = child.stdout.take()?;
    let (tx, rx) = mpsc::channel();
    let prefix = prefix.to_string();
    std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(v) = l.strip_prefix(&prefix) {
                let _ = tx.send(v.to_string());
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(secs)).ok()
}

#[test]
fn a_screen_reaches_a_pipewire_viewer() {
    // CI sets YGGDRASIL_TEST_SCREENCAST_REQUIRED: there it may not skip
    let required = std::env::var_os("YGGDRASIL_TEST_SCREENCAST_REQUIRED").is_some();
    let Some(comp) = compositor() else {
        assert!(!required, "no compositor build (YGGDRASIL_COMPOSITOR, else target/debug/yggdrasil-compositor)");
        eprintln!("skipped: no compositor build (YGGDRASIL_COMPOSITOR)");
        return;
    };
    if !on_path("pipewire") {
        assert!(!required, "PipeWire is not installed");
        eprintln!("skipped: PipeWire is not installed");
        return;
    }
    // a short folder: the sockets' paths must fit
    let dir = PathBuf::from(format!("/tmp/ygg-sc-test-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("conf")).unwrap();
    std::fs::write(dir.join("conf/pipewire.conf"), CONF).unwrap();
    let mut w = World { dir: dir.clone(), children: vec![] };
    let env = |c: &mut Command| {
        // SAFETY: only an async-signal-safe prctl between fork and exec
        unsafe {
            c.pre_exec(|| rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::KILL)).map_err(Into::into));
        }
        c.env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &dir)
            .env("XDG_RUNTIME_DIR", &dir)
            .env("PIPEWIRE_RUNTIME_DIR", &dir);
    };
    let mut pw_daemon = Command::new("pipewire");
    env(&mut pw_daemon);
    pw_daemon.env("PIPEWIRE_CONFIG_DIR", dir.join("conf")).stdout(Stdio::null()).stderr(Stdio::null());
    w.children.push(pw_daemon.spawn().expect("pipewire starts"));

    let mut c = Command::new(comp);
    env(&mut c);
    c.args(["--headless", "--size", "320x240"]).env("YGGDRASIL_COMPOSITOR_XWAYLAND", "0").stdout(Stdio::piped()).stderr(Stdio::null());
    let mut comp = c.spawn().expect("the compositor starts");
    let display = line_with(&mut comp, "WAYLAND_DISPLAY=", 20).expect("the compositor's display");
    w.children.push(comp);
    // PipeWire's socket is there by now (it started first)
    let sock = dir.join("pipewire-0");
    for _ in 0..100 {
        if sock.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut sc = Command::new(env!("CARGO_BIN_EXE_yggdrasil-screencast"));
    env(&mut sc);
    sc.args(["--output", "HEADLESS-1"]).env("WAYLAND_DISPLAY", &display).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut sc = sc.spawn().expect("yggdrasil-screencast starts");
    let node: u32 = line_with(&mut sc, "node ", 20).expect("the stream's node").parse().expect("a node number");
    w.children.push(sc);

    // the viewer: a stream of its own, linked to the screen's node by hand (no session manager here)
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).unwrap();
    let context = pw::context::ContextRc::new(&mainloop, None).unwrap();
    let core = context.connect_rc(Some(properties! { *pw::keys::REMOTE_NAME => sock.to_str().unwrap() })).expect("our PipeWire");
    let stream = pw::stream::StreamRc::new(
        core.clone(),
        "screencast-viewer",
        properties! { *pw::keys::MEDIA_TYPE => "Video", *pw::keys::MEDIA_CATEGORY => "Capture", *pw::keys::MEDIA_ROLE => "Screen" },
    )
    .unwrap();
    let got: Got = Rc::new(RefCell::new(None));
    // the two ports, as the registry tells them: the screen's output port and the viewer's input port with its node
    // (link-factory needs both ports); this PipeWire has no other input port than the viewer's
    let ports: Ports = Rc::new(RefCell::new((None, None)));
    let link: Rc<RefCell<Option<pw::link::Link>>> = Rc::new(RefCell::new(None));
    let registry = core.get_registry_rc().unwrap();
    let (p, l, core2) = (ports.clone(), link.clone(), core.clone());
    let _reg = registry
        .add_listener_local()
        .global(move |g| {
            if g.type_ != pw::types::ObjectType::Port {
                return;
            }
            let Some(props) = g.props else { return };
            let Some(owner) = props.get("node.id").and_then(|n| n.parse::<u32>().ok()) else { return };
            match props.get("port.direction") {
                Some("out") if owner == node => p.borrow_mut().0 = Some(g.id),
                Some("in") => p.borrow_mut().1 = Some((owner, g.id)),
                _ => {}
            }
            let found = *p.borrow();
            let free = l.borrow().is_none();
            if let (Some(out), Some((viewer, inp)), true) = (found.0, found.1, free) {
                let made = core2.create_object::<pw::link::Link>(
                    "link-factory",
                    &properties! {
                        "link.output.node" => node.to_string(),
                        "link.output.port" => out.to_string(),
                        "link.input.node" => viewer.to_string(),
                        "link.input.port" => inp.to_string(),
                        "object.linger" => "false",
                    },
                );
                *l.borrow_mut() = made.ok();
            }
        })
        .register();
    let (g, ml) = (got.clone(), mainloop.clone());
    let _listener = stream
        .add_local_listener_with_user_data(())
        .process(move |s, _| {
            let Some(mut b) = s.dequeue_buffer() else { return };
            let Some(d) = b.datas_mut().first_mut() else { return };
            let size = d.chunk().size() as usize;
            if size == 0 {
                return;
            }
            let first = d.data().map(|p| p[..8].to_vec()).unwrap_or_default();
            *g.borrow_mut() = Some((size, first));
            ml.quit();
        })
        .register()
        .unwrap();
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(spa::param::format::FormatProperties::MediaType, Id, spa::param::format::MediaType::Video),
        spa::pod::property!(spa::param::format::FormatProperties::MediaSubtype, Id, spa::param::format::MediaSubtype::Raw),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 320, height: 240 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 16384, height: 16384 }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    let bytes = spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
        .unwrap()
        .0
        .into_inner();
    let mut params = [Pod::from_bytes(&bytes).unwrap()];
    stream.connect(spa::utils::Direction::Input, None, pw::stream::StreamFlags::MAP_BUFFERS, &mut params).unwrap();
    let ml = mainloop.clone();
    let timer = mainloop.loop_().add_timer(move |_| ml.quit());
    let _ = timer.update_timer(Some(Duration::from_secs(15)), None);
    mainloop.run();

    let (size, first) = got.borrow().clone().expect("a frame reached the viewer within 15 s");
    assert_eq!(size, 320 * 240 * 4, "one whole screen of 32-bit pixels");
    // the compositor's background, (12, 12, 20): blue, green, red in memory
    assert_eq!(&first[..3], &[20, 12, 12], "the screen's own pixels: {first:?}");
    drop(w);
}
