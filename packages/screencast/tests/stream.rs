//! The stream on a real PipeWire: a PipeWire of its own (a minimal configuration: no devices, nothing of the desktop's audio),
//! the headless compositor, yggdrasil-screencast on its screen, and a viewer here that links itself to the stream's node and
//! reads the first frame: the screen's size, and the compositor's background colour. Skipped where PipeWire or the compositor's
//! build is not there (tests/common), except under YGGDRASIL_TEST_SCREENCAST_REQUIRED (CI), where that fails.

use std::cell::RefCell;
use std::process::Stdio;
use std::rc::Rc;
use std::time::Duration;

use pipewire as pw;
use pw::{properties::properties, spa, spa::pod::Pod};

mod common;
use common::{line_with, World};

/// The first frame the viewer read: its size in bytes and its first pixels.
type Got = Rc<RefCell<Option<(usize, Vec<u8>)>>>;
/// The screen's output port, and the viewer's node with its input port.
type Ports = Rc<RefCell<(Option<u32>, Option<(u32, u32)>)>>;

#[test]
fn a_screen_reaches_a_pipewire_viewer() {
    let Some(mut w) = World::new("ygg-sc-stream") else { return };
    let sock = w.start_pipewire();
    let (display, _) = w.start_compositor(&["--size", "320x240"]);
    let mut sc = w.command(env!("CARGO_BIN_EXE_yggdrasil-screencast"));
    sc.args(["--output", "HEADLESS-1"]).env("WAYLAND_DISPLAY", &display).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut sc = sc.spawn().expect("yggdrasil-screencast starts");
    let node: u32 = line_with(&mut sc, "node ", 20).expect("the stream's node").parse().expect("a node number");
    w.keep(sc);

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
