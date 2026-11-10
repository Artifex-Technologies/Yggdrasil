//! The headless backend (the "hidden test mode"): virtual outputs of a chosen size and scale, drawn in memory with the software
//! renderer (pixman), no screen, no GPU and no input devices. It lets Yggdrasil's own programs (the panel first) be tested on this
//! compositor in a CI container, and is the first step towards replacing Hyprland with it.
//!
//!   yggdrasil-compositor --headless [--outputs N] [--size WxH] [--scale S] [--output WxH[@S]]... [--run COMMAND ...]
//!       --outputs N      N outputs side by side (default 1), each --size (default 1920x1080) at --scale (default 1)
//!       --output WxH@S   one output of its own size and scale; repeat for more (instead of --outputs/--size/--scale)
//!   (`--backend headless` is the same as `--headless`.)
//!
//! It expects a private XDG_RUNTIME_DIR (a test's own folder) and refuses the shared /run/user/UID one unless
//! YGGDRASIL_COMPOSITOR_SHARED_RUNTIME=1; WAYLAND_DISPLAY, DISPLAY and the other compositors' variables are cleared before
//! anything starts, so nothing it starts can reach the running desktop. XWayland is left out unless YGGDRASIL_COMPOSITOR_XWAYLAND=1.
//!
//! A control socket in that runtime folder (printed as YGGDRASIL_COMPOSITOR_CTL=...; only this user can open it) takes one command
//! per connection, and `yggdrasil-compositor ctl COMMAND...` sends one:
//!   outputs                      one line per output: NAME WxH scale=S at=X,Y logical=WxH
//!   resize NAME WxH              change an output's size (the programs are told, panels laid out again)
//!   scale NAME S                 change an output's scale
//!   screenshot NAME FILE.png     draw what is pending and write the output's picture as a PNG
//!   windows                      one line per window: app id, title, place and size (tab separated)
//!   layers                       one line per panel or background: output, namespace, layer, place and size (tab separated)
//!   quit                         end the compositor
//! The answer is the lines, then "ok" or "error: ...".

use std::cell::RefCell;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{Bind, Offscreen};
use smithay::desktop::layer_map_for_output;
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction};
use smithay::reexports::pixman::Image;
use smithay::reexports::wayland_server::DisplayHandle;
use smithay::utils::Transform;

use crate::state::{self, App};

/// One virtual output: its size in pixels and its scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OutputSpec {
    pub w: i32,
    pub h: i32,
    pub scale: f64,
}

/// The backend was asked for: `--headless`, `--backend headless` or `--backend=headless`.
pub fn wanted(opts: &[String]) -> bool {
    opts.iter().any(|a| a == "--headless" || a == "--backend=headless") || opts.windows(2).any(|w| w[0] == "--backend" && w[1] == "headless")
}

/// "1600x900" (both at least 1, at most 16384).
pub fn parse_size(s: &str) -> Option<(i32, i32)> {
    let (w, h) = s.split_once('x')?;
    let (w, h) = (w.trim().parse::<i32>().ok()?, h.trim().parse::<i32>().ok()?);
    ((1..=16384).contains(&w) && (1..=16384).contains(&h)).then_some((w, h))
}

/// A scale between 0.25 and 8.
pub fn parse_scale(s: &str) -> Option<f64> {
    let v = s.trim().parse::<f64>().ok()?;
    (v.is_finite() && (0.25..=8.0).contains(&v)).then_some(v)
}

/// "1600x900" or "1600x900@1.5".
pub fn parse_output(s: &str) -> Option<OutputSpec> {
    let (size, scale) = match s.split_once('@') {
        Some((a, b)) => (a, parse_scale(b)?),
        None => (s, 1.0),
    };
    let (w, h) = parse_size(size)?;
    Some(OutputSpec { w, h, scale })
}

/// The outputs asked for on the command line.
pub fn outputs_from(opts: &[String]) -> Result<Vec<OutputSpec>, String> {
    let mut count = 1usize;
    let mut size = (1920, 1080);
    let mut scale = 1.0;
    let mut listed = vec![];
    let mut i = 0;
    let value = |i: usize, name: &str| opts.get(i + 1).cloned().ok_or_else(|| format!("{name} needs a value"));
    while i < opts.len() {
        match opts[i].as_str() {
            "--outputs" => {
                let v = value(i, "--outputs")?;
                count = v.parse().ok().filter(|n| (1..=16).contains(n)).ok_or_else(|| format!("--outputs {v}: give 1 to 16"))?;
                i += 1;
            }
            "--size" => {
                let v = value(i, "--size")?;
                size = parse_size(&v).ok_or_else(|| format!("--size {v}: give WIDTHxHEIGHT, like 1600x900"))?;
                i += 1;
            }
            "--scale" => {
                let v = value(i, "--scale")?;
                scale = parse_scale(&v).ok_or_else(|| format!("--scale {v}: give a number from 0.25 to 8, like 1.5"))?;
                i += 1;
            }
            "--output" => {
                let v = value(i, "--output")?;
                listed.push(parse_output(&v).ok_or_else(|| format!("--output {v}: give WIDTHxHEIGHT or WIDTHxHEIGHT@SCALE"))?);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if listed.is_empty() {
        listed = vec![OutputSpec { w: size.0, h: size.1, scale }; count];
    }
    Ok(listed)
}

/// Refuse the shared runtime folder, and clear everything that leads to the running desktop (before any thread or child exists).
pub fn check_env() -> Result<PathBuf, String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).ok_or("--headless needs XDG_RUNTIME_DIR: give it a private folder of its own (mode 0700)")?;
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX);
    runtime_ok(&dir, uid, std::env::var_os("YGGDRASIL_COMPOSITOR_SHARED_RUNTIME").is_some())?;
    if !dir.is_dir() {
        return Err(format!("XDG_RUNTIME_DIR {} is not a folder", dir.display()));
    }
    for v in ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK", "NIRI_SOCKET"] {
        std::env::remove_var(v);
    }
    Ok(dir)
}

/// The desktop's own runtime folder (/run/user/UID, however it is spelled) is not for tests, unless asked for.
fn runtime_ok(dir: &Path, uid: u32, allow_shared: bool) -> Result<(), String> {
    let plain: PathBuf = dir.components().collect();
    let shared = plain == Path::new(&format!("/run/user/{uid}")) || std::fs::canonicalize(dir).is_ok_and(|d| d == Path::new(&format!("/run/user/{uid}")));
    if shared && !allow_shared {
        return Err(format!("--headless will not use the desktop's own runtime folder {}: give it a private XDG_RUNTIME_DIR (or set YGGDRASIL_COMPOSITOR_SHARED_RUNTIME=1)", dir.display()));
    }
    Ok(())
}

/// Make the outputs (wl_output and xdg-output globals), side by side from 0,0, named HEADLESS-1, HEADLESS-2, ...
pub fn make_outputs(dh: &DisplayHandle, specs: &[OutputSpec]) -> Vec<Output> {
    let mut x = 0;
    specs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let o = Output::new(format!("HEADLESS-{}", i + 1), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Yggdrasil".into(), model: "headless".into() });
            let _ = o.create_global::<App>(dh);
            let mode = Mode { size: (s.w, s.h).into(), refresh: 60_000 };
            o.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Fractional(s.scale)), Some((x, 0).into()));
            o.set_preferred(mode);
            x += state::logical_size(&o).w;
            o
        })
        .collect()
}

/// One output's picture in memory and what is known to be on it.
struct Screen {
    output: Output,
    image: Image<'static, 'static>,
    damage: OutputDamageTracker,
    /// the picture holds the last frame (buffer age 1); false after it was made new
    drawn: bool,
}

pub struct Headless {
    renderer: PixmanRenderer,
    screens: Vec<Screen>,
    /// a frame is already on its way (the wake-ups before it are folded into it)
    pending: bool,
    last: Instant,
    start: Instant,
}

/// About 60 frames a second at most, however often programs commit.
const FRAME: Duration = Duration::from_millis(16);

fn new_image(renderer: &mut PixmanRenderer, o: &Output) -> Result<Image<'static, 'static>, String> {
    let size = o.current_mode().map(|m| m.size).unwrap_or((1, 1).into());
    renderer.create_buffer(Fourcc::Xrgb8888, (size.w, size.h).into()).map_err(|e| format!("cannot make a {}x{} picture: {e}", size.w, size.h))
}

impl Headless {
    fn new(outputs: &[Output]) -> Result<Headless, String> {
        let mut renderer = PixmanRenderer::new().map_err(|e| format!("the software renderer did not start: {e}"))?;
        let mut screens = vec![];
        for o in outputs {
            screens.push(Screen { output: o.clone(), image: new_image(&mut renderer, o)?, damage: OutputDamageTracker::from_output(o), drawn: false });
        }
        Ok(Headless { renderer, screens, pending: false, last: Instant::now() - FRAME, start: Instant::now() })
    }

    /// Draw every output (only what changed), then tell the programs their frame is done.
    fn render(&mut self, app: &mut App) {
        for s in &mut self.screens {
            let size = s.output.current_mode().map(|m| m.size).unwrap_or_default();
            if s.image.width() as i32 != size.w || s.image.height() as i32 != size.h {
                match new_image(&mut self.renderer, &s.output) {
                    Ok(img) => {
                        s.image = img;
                        s.damage = OutputDamageTracker::from_output(&s.output);
                        s.drawn = false;
                    }
                    Err(e) => {
                        eprintln!("yggdrasil-compositor: {e}");
                        continue;
                    }
                }
            }
            let age = if s.drawn { 1 } else { 0 };
            let Ok(mut target) = self.renderer.bind(&mut s.image) else { continue };
            let done = if app.locked {
                let elements = app.lock_elements(&mut self.renderer, &s.output);
                s.damage.render_output(&mut self.renderer, &mut target, age, &elements, [0.0f32, 0.0, 0.0, 1.0]).is_ok()
            } else {
                let elements = app.scene(&mut self.renderer, &s.output);
                s.damage.render_output(&mut self.renderer, &mut target, age, &elements, [0.05f32, 0.05, 0.08, 1.0]).is_ok()
            };
            s.drawn = done;
        }
        self.last = Instant::now();
        app.send_frames(self.start.elapsed());
        app.space.refresh();
        app.foreign_refresh();
        app.popups.cleanup();
        let _ = app.display_handle.flush_clients();
    }

    fn screen(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.output.name() == name)
    }

    /// Lay the outputs out side by side again (one changed size or scale) and tell everyone.
    fn relayout(&mut self, app: &mut App) {
        let mut x = 0;
        for s in &self.screens {
            s.output.change_current_state(None, None, None, Some((x, 0).into()));
            app.space.map_output(&s.output, (x, 0));
            x += state::logical_size(&s.output).w;
        }
        app.output_resized();
    }
}

/// Write an output's picture as an 8-bit RGB PNG.
fn write_png(image: &Image<'static, 'static>, path: &Path) -> Result<(), String> {
    let (w, h, stride) = (image.width(), image.height(), image.stride());
    // SAFETY: the picture is w x h pixels of 4 bytes, rows `stride` bytes apart, alive while `image` is borrowed here
    let bytes = unsafe { std::slice::from_raw_parts(image.data() as *const u8, stride * h) };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in bytes.chunks(stride).take(h) {
        for px in row[..w * 4].chunks_exact(4) {
            let v = u32::from_ne_bytes([px[0], px[1], px[2], px[3]]);
            rgb.extend_from_slice(&[(v >> 16) as u8, (v >> 8) as u8, v as u8]);
        }
    }
    let file = std::fs::File::create(path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().map_err(|e| e.to_string())?;
    writer.write_image_data(&rgb).map_err(|e| e.to_string())?;
    Ok(())
}

/// Run the headless backend until `quit` (or the loop is stopped). The outputs are already mapped in the space.
pub fn run(mut event_loop: EventLoop<'static, App>, mut app: App, outputs: Vec<Output>, ping_source: smithay::reexports::calloop::ping::PingSource, runtime: PathBuf) -> Result<(), Box<dyn std::error::Error>> {
    let h = Rc::new(RefCell::new(Headless::new(&outputs)?));
    let handle = event_loop.handle();

    // a wake-up (a commit, a setting, a command) draws soon: right away when the last frame is old enough, else at the next tick
    let (h2, handle2) = (h.clone(), handle.clone());
    handle.insert_source(ping_source, move |_, _, _app| {
        let mut b = h2.borrow_mut();
        if b.pending {
            return;
        }
        b.pending = true;
        let delay = (b.last + FRAME).saturating_duration_since(Instant::now());
        let h3 = h2.clone();
        let _ = handle2.insert_source(Timer::from_duration(delay), move |_, _, app| {
            let mut b = h3.borrow_mut();
            b.pending = false;
            b.render(app);
            TimeoutAction::Drop
        });
    })?;
    app.wake();

    let ctl = runtime.join(format!("yggdrasil-compositor-{}.ctl", app.socket));
    let _ = std::fs::remove_file(&ctl);
    let listener = UnixListener::bind(&ctl).map_err(|e| format!("cannot make the control socket {}: {e}", ctl.display()))?;
    std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o600))?;
    listener.set_nonblocking(true)?;
    println!("YGGDRASIL_COMPOSITOR_CTL={}", ctl.display());
    let _ = std::io::stdout().flush();
    let h4 = h.clone();
    handle.insert_source(Generic::new(listener, Interest::READ, LoopMode::Level), move |_, l, app| {
        while let Ok((stream, _)) = l.accept() {
            serve(stream, app, &mut h4.borrow_mut());
        }
        Ok(PostAction::Continue)
    })?;

    event_loop.run(None, &mut app, |app| {
        let _ = app.display_handle.flush_clients();
    })?;
    // the loop's sources hold handles to the loop itself, so they are never dropped and the Wayland socket would stay behind
    let _ = std::fs::remove_file(&ctl);
    let _ = std::fs::remove_file(runtime.join(&app.socket));
    let _ = std::fs::remove_file(runtime.join(format!("{}.lock", app.socket)));
    Ok(())
}

/// One command from the control socket, answered on it.
fn serve(stream: UnixStream, app: &mut App, h: &mut Headless) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(2)));
    let mut line = String::new();
    let mut reader = BufReader::new(&stream).take(4096);
    if reader.read_line(&mut line).is_err() {
        return;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let answer = match command(&words, app, h) {
        Ok(lines) => lines.into_iter().map(|l| l + "\n").collect::<String>() + "ok\n",
        Err(e) => format!("error: {e}\n"),
    };
    let mut s = &stream;
    let _ = s.write_all(answer.as_bytes());
}

fn command(words: &[&str], app: &mut App, h: &mut Headless) -> Result<Vec<String>, String> {
    let output = |name: &str, h: &Headless| h.screen(name).map(|s| s.output.clone()).ok_or_else(|| format!("no output {name}"));
    match words {
        ["outputs"] => Ok(h
            .screens
            .iter()
            .map(|s| {
                let o = &s.output;
                let m = o.current_mode().map(|m| m.size).unwrap_or_default();
                let l = state::logical_size(o);
                let at = o.current_location();
                format!("{} {}x{} scale={} at={},{} logical={}x{}", o.name(), m.w, m.h, o.current_scale().fractional_scale(), at.x, at.y, l.w, l.h)
            })
            .collect()),
        ["resize", name, size] => {
            let o = output(name, h)?;
            let (w, hh) = parse_size(size).ok_or_else(|| format!("{size}: give WIDTHxHEIGHT"))?;
            let mode = Mode { size: (w, hh).into(), refresh: 60_000 };
            let old = o.current_mode();
            o.change_current_state(Some(mode), None, None, None);
            o.set_preferred(mode);
            if let Some(old) = old.filter(|m| *m != mode) {
                o.delete_mode(old);
            }
            h.relayout(app);
            Ok(vec![])
        }
        ["scale", name, s] => {
            let o = output(name, h)?;
            let v = parse_scale(s).ok_or_else(|| format!("{s}: give a number from 0.25 to 8"))?;
            o.change_current_state(None, None, Some(Scale::Fractional(v)), None);
            h.relayout(app);
            Ok(vec![])
        }
        ["screenshot", name, path] => {
            output(name, h)?;
            h.render(app);
            let s = h.screen(name).ok_or("the output went away")?;
            write_png(&s.image, Path::new(path))?;
            Ok(vec![])
        }
        ["windows"] => Ok(app
            .space
            .elements()
            .map(|w| {
                let (title, app_id) = state::title_app(w);
                let at = app.space.element_location(w).unwrap_or_default();
                let size = w.geometry().size;
                format!("{app_id}\t{title}\t{},{}\t{}x{}", at.x, at.y, size.w, size.h)
            })
            .collect()),
        ["layers"] => {
            let mut lines = vec![];
            for s in &h.screens {
                let map = layer_map_for_output(&s.output);
                for l in map.layers() {
                    let geo = map.layer_geometry(l).unwrap_or_default();
                    lines.push(format!("{}\t{}\t{:?}\t{},{}\t{}x{}", s.output.name(), l.namespace(), l.layer(), geo.loc.x, geo.loc.y, geo.size.w, geo.size.h));
                }
            }
            Ok(lines)
        }
        ["quit"] => {
            app.quit();
            Ok(vec![])
        }
        [] => Err("no command".into()),
        _ => Err(format!("unknown command {:?} (outputs, resize NAME WxH, scale NAME S, screenshot NAME FILE, windows, layers, quit)", words.join(" "))),
    }
}

/// `yggdrasil-compositor ctl [--socket PATH] COMMAND...`: send one command to a headless compositor and print the answer. The
/// socket is --socket, else $YGGDRASIL_COMPOSITOR_CTL, else the only one in $XDG_RUNTIME_DIR. Exit 0 on "ok", 1 on an error,
/// 2 when no compositor was found.
pub fn ctl(args: &[String]) -> i32 {
    let (path, words): (Option<PathBuf>, &[String]) = match args {
        [flag, p, rest @ ..] if flag == "--socket" => (Some(PathBuf::from(p)), rest),
        _ => (None, args),
    };
    let path = path.or_else(|| std::env::var_os("YGGDRASIL_COMPOSITOR_CTL").map(PathBuf::from)).or_else(|| {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")?;
        let found: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("yggdrasil-compositor-") && n.ends_with(".ctl"))).collect();
        (found.len() == 1).then(|| found[0].clone())
    });
    let Some(path) = path else {
        eprintln!("yggdrasil-compositor ctl: no headless compositor found (give --socket PATH or YGGDRASIL_COMPOSITOR_CTL)");
        return 2;
    };
    let mut stream = match UnixStream::connect(&path) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("yggdrasil-compositor ctl: {}: {e}", path.display());
            return 2;
        }
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
    if stream.write_all(format!("{}\n", words.join(" ")).as_bytes()).is_err() {
        return 2;
    }
    let mut answer = String::new();
    let _ = stream.read_to_string(&mut answer);
    let mut lines: Vec<&str> = answer.lines().collect();
    match lines.pop() {
        Some("ok") => {
            for l in lines {
                println!("{l}");
            }
            0
        }
        Some(e) => {
            eprintln!("yggdrasil-compositor ctl: {}", e.trim_start_matches("error: "));
            1
        }
        None => {
            eprintln!("yggdrasil-compositor ctl: no answer");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn sizes_and_scales_are_read() {
        assert_eq!(parse_size("1600x900"), Some((1600, 900)));
        assert_eq!(parse_size("0x900"), None);
        assert_eq!(parse_size("1600"), None);
        assert_eq!(parse_output("1280x720@1.5"), Some(OutputSpec { w: 1280, h: 720, scale: 1.5 }));
        assert_eq!(parse_output("1280x720@0"), None);
        assert_eq!(parse_scale("nan"), None);
    }

    #[test]
    fn outputs_on_the_command_line() {
        assert_eq!(outputs_from(&v(&["--headless"])).unwrap(), vec![OutputSpec { w: 1920, h: 1080, scale: 1.0 }]);
        let two = outputs_from(&v(&["--headless", "--outputs", "2", "--size", "800x600", "--scale", "2"])).unwrap();
        assert_eq!(two, vec![OutputSpec { w: 800, h: 600, scale: 2.0 }; 2]);
        let listed = outputs_from(&v(&["--output", "1600x900", "--output", "1280x720@1.25"])).unwrap();
        assert_eq!(listed, vec![OutputSpec { w: 1600, h: 900, scale: 1.0 }, OutputSpec { w: 1280, h: 720, scale: 1.25 }]);
        assert!(outputs_from(&v(&["--size", "big"])).is_err());
        assert!(outputs_from(&v(&["--outputs"])).is_err());
    }

    #[test]
    fn the_desktops_own_runtime_folder_is_refused() {
        assert!(runtime_ok(Path::new("/run/user/1000"), 1000, false).is_err());
        assert!(runtime_ok(Path::new("/run/user/1000/"), 1000, false).is_err());
        assert!(runtime_ok(Path::new("/run/user/1000"), 1000, true).is_ok(), "unless asked for");
        assert!(runtime_ok(Path::new("/tmp/ygg-test/run"), 1000, false).is_ok());
        assert!(runtime_ok(Path::new("/run/user/1001"), 1000, false).is_ok(), "another user's is not ours (and is not readable anyway)");
    }

    #[test]
    fn the_backend_is_chosen() {
        assert!(wanted(&v(&["--headless"])));
        assert!(wanted(&v(&["--backend", "headless"])));
        assert!(wanted(&v(&["--backend=headless"])));
        assert!(!wanted(&v(&["--backend", "winit"])));
    }
}
