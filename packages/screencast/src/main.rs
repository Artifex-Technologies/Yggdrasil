//! yggdrasil-screencast: screen sharing on Yggdrasil's own compositor. It copies one screen through the standard Wayland
//! capture protocols (ext-image-capture-source, ext-image-copy-capture: what the compositor serves for the panel's previews)
//! and publishes the frames as a PipeWire video stream, which yggdrasil-portal hands to the program that asked
//! (org.freedesktop.impl.portal.ScreenCast).
//!
//!   yggdrasil-screencast --output NAME [--cursor]
//!
//! It prints `node N` (the stream's PipeWire node) once the stream is there, then runs until the screen goes, the compositor
//! ends, or it is stopped (SIGTERM: the portal's Session.Close). `--cursor` paints the pointer into the frames.
//!
//! Frames come as the screen changes: the compositor answers a capture only once something new is drawn, so nothing is copied
//! or sent while the screen stands still. Two threads: the capture (Wayland, blocking) and PipeWire's loop; the newest frame
//! goes from one to the other, a frame not yet sent is replaced by a newer one.

mod capture;

use std::sync::{Arc, Mutex};

use pipewire as pw;
use pw::spa;
use pw::{properties::properties, spa::pod::Pod};

/// The newest frame: its pixels (rows of `stride` bytes), and whether it was sent yet.
pub struct Frame {
    pub pixels: Vec<u8>,
    pub fresh: bool,
}

/// What the capture says to the stream's side.
pub enum Msg {
    /// a new frame is in the shared `Frame`
    Frame,
    /// the capture ended (the screen went, the compositor ended): the reason
    Ended(String),
}

/// The options: the screen's name, and whether the pointer is painted in.
#[derive(Debug, PartialEq)]
pub struct Opts {
    pub output: String,
    pub cursor: bool,
}

pub fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut output = None;
    let mut cursor = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--output" => output = Some(it.next().ok_or("--output needs a screen's name")?.clone()),
            "--cursor" => cursor = true,
            other => return Err(format!("{other}: not an option (yggdrasil-screencast --output NAME [--cursor])")),
        }
    }
    Ok(Opts { output: output.ok_or("--output NAME: the screen to share")?, cursor })
}

/// The stream's format: BGRx or BGRA (the order wl_shm's XRGB8888 and ARGB8888 keep in memory), the screen's size, any rate
/// up to 60 a second (frames come when the screen changes).
fn format_param(format: spa::param::video::VideoFormat, w: u32, h: u32) -> Vec<u8> {
    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(spa::param::format::FormatProperties::MediaType, Id, spa::param::format::MediaType::Video),
        spa::pod::property!(spa::param::format::FormatProperties::MediaSubtype, Id, spa::param::format::MediaSubtype::Raw),
        spa::pod::property!(spa::param::format::FormatProperties::VideoFormat, Id, format),
        spa::pod::property!(spa::param::format::FormatProperties::VideoSize, Rectangle, spa::utils::Rectangle { width: w, height: h }),
        spa::pod::property!(spa::param::format::FormatProperties::VideoFramerate, Fraction, spa::utils::Fraction { num: 0, denom: 1 }),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 60, denom: 1 },
            spa::utils::Fraction { num: 1, denom: 1 },
            spa::utils::Fraction { num: 60, denom: 1 }
        ),
    );
    serialize(obj)
}

/// The buffers: 2 to 8 of one frame each, in shared memory (memfd) or plain memory.
fn buffers_param(stride: u32, h: u32) -> Vec<u8> {
    use spa::pod::{ChoiceValue, Property, PropertyFlags, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags};
    let int = |key: u32, v: i32| Property { key, flags: PropertyFlags::empty(), value: Value::Int(v) };
    let obj = spa::pod::Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![
            Property {
                key: spa::sys::SPA_PARAM_BUFFERS_buffers,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Int(Choice(ChoiceFlags::empty(), ChoiceEnum::Range { default: 4, min: 2, max: 8 }))),
            },
            int(spa::sys::SPA_PARAM_BUFFERS_blocks, 1),
            int(spa::sys::SPA_PARAM_BUFFERS_size, (stride * h) as i32),
            int(spa::sys::SPA_PARAM_BUFFERS_stride, stride as i32),
            Property {
                key: spa::sys::SPA_PARAM_BUFFERS_dataType,
                flags: PropertyFlags::empty(),
                value: Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Flags {
                        default: 1 << spa::sys::SPA_DATA_MemFd,
                        flags: vec![1 << spa::sys::SPA_DATA_MemFd, 1 << spa::sys::SPA_DATA_MemPtr],
                    },
                ))),
            },
        ],
    };
    serialize(obj)
}

fn serialize(obj: spa::pod::Object) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &spa::pod::Value::Object(obj))
        .map(|(c, _)| c.into_inner())
        .unwrap_or_default()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let opts = match parse_args(&args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("yggdrasil-screencast: {e}");
            std::process::exit(2);
        }
    };
    if let Err(e) = run(opts) {
        eprintln!("yggdrasil-screencast: {e}");
        std::process::exit(1);
    }
}

fn run(opts: Opts) -> Result<(), String> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).map_err(|e| format!("PipeWire's loop: {e}"))?;
    let context = pw::context::ContextRc::new(&mainloop, None).map_err(|e| format!("PipeWire: {e}"))?;
    let core = context.connect_rc(None).map_err(|e| format!("PipeWire is not reachable: {e}"))?;

    // the capture starts first: the stream's format is the screen's size and the compositor's pixel format
    let frame = Arc::new(Mutex::new(Frame { pixels: vec![], fresh: false }));
    let (tx, rx) = pw::channel::channel::<Msg>();
    let shape = capture::start(&opts.output, opts.cursor, frame.clone(), tx)?;
    let (w, h, stride) = (shape.width, shape.height, shape.stride);
    let video = if shape.alpha { spa::param::video::VideoFormat::BGRA } else { spa::param::video::VideoFormat::BGRx };

    let stream = pw::stream::StreamRc::new(
        core,
        "yggdrasil-screencast",
        properties! {
            *pw::keys::MEDIA_CLASS => "Video/Source",
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
            *pw::keys::NODE_NAME => "yggdrasil-screencast",
            *pw::keys::NODE_DESCRIPTION => format!("Screen {}", opts.output).as_str(),
        },
    )
    .map_err(|e| format!("the stream: {e}"))?;

    let told = std::cell::Cell::new(false);
    let ml = mainloop.clone();
    let _listener = stream
        .add_local_listener_with_user_data(frame.clone())
        .state_changed(move |s, frame, _, new| {
            // a viewer connected: the newest frame again at once (the compositor sends the next only when the screen changes)
            if matches!(new, pw::stream::StreamState::Streaming) {
                let mut f = frame.lock().unwrap_or_else(|e| e.into_inner());
                if !f.pixels.is_empty() {
                    f.fresh = true;
                    drop(f);
                    let _ = s.trigger_process();
                }
            }
            // the node exists from Paused on: the portal is told its number once
            if matches!(new, pw::stream::StreamState::Paused | pw::stream::StreamState::Streaming) && !told.get() {
                told.set(true);
                println!("node {}", s.node_id());
                use std::io::Write;
                let _ = std::io::stdout().flush();
            }
            if let pw::stream::StreamState::Error(e) = new {
                eprintln!("yggdrasil-screencast: the stream failed: {e}");
                ml.quit();
            }
        })
        .param_changed(move |s, _, id, param| {
            // the format is agreed: the buffers are asked for, one frame each
            if id != spa::param::ParamType::Format.as_raw() || param.is_none() {
                return;
            }
            let bytes = buffers_param(stride, h);
            if let Some(p) = Pod::from_bytes(&bytes) {
                let _ = s.update_params(&mut [p]);
            }
        })
        .process(move |s, frame| {
            // a cycle: the newest frame, when there is one not sent yet
            let mut f = frame.lock().unwrap_or_else(|e| e.into_inner());
            if !f.fresh {
                return;
            }
            let Some(mut buffer) = s.dequeue_buffer() else { return };
            let datas = buffer.datas_mut();
            let Some(d) = datas.first_mut() else { return };
            let n = f.pixels.len();
            if let Some(dst) = d.data() {
                let k = n.min(dst.len());
                dst[..k].copy_from_slice(&f.pixels[..k]);
                let c = d.chunk_mut();
                *c.offset_mut() = 0;
                *c.size_mut() = k as u32;
                *c.stride_mut() = stride as i32;
            }
            f.fresh = false;
            // the buffer goes back to the stream (queued) when it is dropped here
        })
        .register()
        .map_err(|e| format!("the stream's listener: {e}"))?;

    let format = format_param(video, w, h);
    let mut params = [Pod::from_bytes(&format).ok_or("the stream's format")?];
    stream
        .connect(spa::utils::Direction::Output, None, pw::stream::StreamFlags::DRIVER | pw::stream::StreamFlags::MAP_BUFFERS, &mut params)
        .map_err(|e| format!("the stream: {e}"))?;

    // a new frame: a cycle is started (this stream drives); the capture ending ends the stream
    let (s2, ml2) = (stream.clone(), mainloop.clone());
    let _attached = rx.attach(mainloop.loop_(), move |m| match m {
        Msg::Frame => {
            let _ = s2.trigger_process();
        }
        Msg::Ended(why) => {
            eprintln!("yggdrasil-screencast: {why}");
            ml2.quit();
        }
    });
    // SIGTERM and SIGINT (the portal closing the session) end it cleanly
    let ml3 = mainloop.clone();
    let _term = mainloop.loop_().add_signal_local(pw::loop_::Signal::TERM, move || ml3.quit());
    let ml4 = mainloop.clone();
    let _int = mainloop.loop_().add_signal_local(pw::loop_::Signal::INT, move || ml4.quit());
    mainloop.run();
    let _ = stream.disconnect();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options() {
        let a = |v: &[&str]| parse_args(&v.iter().map(|s| s.to_string()).collect::<Vec<_>>());
        assert_eq!(a(&["--output", "DP-1"]), Ok(Opts { output: "DP-1".into(), cursor: false }));
        assert_eq!(a(&["--cursor", "--output", "eDP-1"]), Ok(Opts { output: "eDP-1".into(), cursor: true }));
        assert!(a(&[]).is_err() && a(&["--output"]).is_err() && a(&["--window", "1"]).is_err());
    }

    #[test]
    fn the_params_are_pods() {
        let f = format_param(spa::param::video::VideoFormat::BGRx, 1920, 1080);
        assert!(Pod::from_bytes(&f).is_some());
        let b = buffers_param(1920 * 4, 1080);
        assert!(Pod::from_bytes(&b).is_some());
    }
}
