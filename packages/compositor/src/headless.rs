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
//!   outputs                      one line per output: NAME WxH scale=S at=X,Y logical=WxH, then " power=off" for one that is
//!                                off (wlr-output-power) and " disabled" for one switched off (Settings, wlr-output-management)
//!   resize NAME WxH              the output becomes a screen of that size: its own (preferred) mode, which it is set to
//!   scale NAME S                 change an output's scale
//!   add-output WxH[@S]           a screen is plugged in (named HEADLESS-N, the next number), placed as Settings' file says or
//!                                to the right of the others
//!   remove-output NAME           a screen is unplugged: its panels are closed, its windows move to the first screen
//!   screenshot NAME FILE.png [pointer]
//!                                draw what is pending and write the output's picture as a PNG; with "pointer", the pointer
//!                                painted over it, as the screen shows it (pointer.rs: drawn as a cursor plane, not in the picture)
//!   pointer-look                 what the pointer looks like: theme NAME SIZE SHAPE, surface WxH X,Y (a program's own picture and
//!                                its hotspot) or hidden (resting: the pointer timeout; program: the program hid it)
//!   windows                      one line per window: app id, title, place, size, workspace, states (tab separated; the
//!                                states are active, floating or tiled, maximized, fullscreen, left, right, minimized)
//!   layers                       one line per panel or background: output, namespace, layer, place and size (tab separated)
//!   free                         one line per output: the room the panels leave free for windows (output, place, size)
//!   focus                        what has the keyboard: window APP TITLE, layer NAMESPACE OUTPUT, lock OUTPUT, or none
//!   workspace                    the workspace showing (1 to 4)
//!   lock                         "locked" or "unlocked", then one line per lock surface: output and size
//!   key COMBO...                 press and let go of keys, one combination after another: super+ctrl+2, Return, a,
//!                                XF86AudioMute, ctrl (the left one; rctrl the right one)
//!   key-down COMBO, key-up COMBO press keys and keep them down (a held key repeats its shortcut), let them go
//!   type TEXT                    type text (letters, digits, space and common punctuation, US layout)
//!   pointer X Y                  move the pointer to X,Y (logical, in the space of all outputs)
//!   click [left|right|middle]    press and let go of a button of the virtual mouse where the pointer is (left-handed in
//!                                input.conf: its left one is the right button)
//!   pointer-move DX DY           move the virtual mouse: the pointer goes as far as its speed in input.conf says
//!   scroll DY [DX]               turn its wheel (input.conf's scroll factor and natural scrolling applied)
//!   input                        the mouse and keyboard settings in force (input.rs: Settings > Peripherals' input.conf)
//!   power NAME on|off            switch an output on or off (as wlr-output-power does)
//!   clock [stop|run|+MS]         the clock the animations run on: stop it, move a stopped one on, let it run again; without
//!                                a word: "running" or "stopped" and its time (ms). For tests: a frame in an animation
//!   quit                         end the compositor
//! and the desktop commands Yggdrasil's own tools use through yggdrasil-wm (wmctl.rs: clients, active, workspaces, workspace N,
//! activate, close, minimize, maximize, send, place, cursor, screens, screens all, pointer-timeout, modifiers, layers, binds,
//! reload, paste, cursor-theme, keyboard, exit).
//! The answer is the lines, then "ok" or "error: ...".
//!
//! The input is made here, inside the compositor, for the tests: no input device is opened, and nothing reaches the running
//! desktop. wlr-screencopy (screencopy.rs), ext-image-copy-capture (capture.rs: single windows, drawn on their own, and screens)
//! and wlr-output-power (outputpower.rs) are served from the pictures in memory, and
//! the screens are listed and changed through wlr-output-management (outputmgmt.rs) and Settings' monitors.lua (outputs.rs).

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
use smithay::output::{Mode, Output, PhysicalProperties, Scale, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction};
use smithay::reexports::pixman::Image;
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
    opts.iter().any(|a| a == "--headless" || a == "--backend=headless")
        || opts.windows(2).any(|w| w[0] == "--backend" && w[1] == "headless")
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
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .ok_or("--headless needs XDG_RUNTIME_DIR: give it a private folder of its own (mode 0700)")?;
    use std::os::unix::fs::MetadataExt;
    let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(u32::MAX);
    runtime_ok(&dir, uid, std::env::var_os("YGGDRASIL_COMPOSITOR_SHARED_RUNTIME").is_some())?;
    if !dir.is_dir() {
        return Err(format!("XDG_RUNTIME_DIR {} is not a folder", dir.display()));
    }
    for v in ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK", "NIRI_SOCKET"] {
        std::env::remove_var(v);
    }
    // the desktop's session bus is not passed on either (a program started here would act on the running desktop's services);
    // a test gives a bus of its own
    if let Some(bus) = std::env::var_os("DBUS_SESSION_BUS_ADDRESS") {
        if desktop_bus(&bus.to_string_lossy(), uid) && std::env::var_os("YGGDRASIL_COMPOSITOR_SHARED_RUNTIME").is_none() {
            std::env::remove_var("DBUS_SESSION_BUS_ADDRESS");
        }
    }
    // nor the system bus: Super+L runs the session's `loginctl lock-session`, which through logind would lock the running
    // desktop; here it finds no bus and does nothing
    if std::env::var_os("YGGDRASIL_COMPOSITOR_SHARED_RUNTIME").is_none() {
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", format!("unix:path={}", dir.join("no-system-bus").display()));
    }
    Ok(dir)
}

/// The running desktop's session bus: the one in /run/user/UID (unix:path=/run/user/1000/bus, the usual address).
fn desktop_bus(address: &str, uid: u32) -> bool {
    address.contains(&format!("/run/user/{uid}/")) || address.contains("abstract=") || address.starts_with("systemd:")
}

/// The desktop's own runtime folder (/run/user/UID, however it is spelled) is not for tests, unless asked for.
fn runtime_ok(dir: &Path, uid: u32, allow_shared: bool) -> Result<(), String> {
    let plain: PathBuf = dir.components().collect();
    let shared = plain == Path::new(&format!("/run/user/{uid}"))
        || std::fs::canonicalize(dir).is_ok_and(|d| d == Path::new(&format!("/run/user/{uid}")));
    if shared && !allow_shared {
        return Err(format!("--headless will not use the desktop's own runtime folder {}: give it a private XDG_RUNTIME_DIR (or set YGGDRASIL_COMPOSITOR_SHARED_RUNTIME=1)", dir.display()));
    }
    Ok(())
}

/// A virtual screen HEADLESS-n of this size and scale (its wl_output global and its place come with `App::add_output`).
pub fn make_output(n: usize, s: &OutputSpec) -> Output {
    let o = Output::new(
        format!("HEADLESS-{n}"),
        PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Yggdrasil".into(), model: "headless".into() },
    );
    let mode = Mode { size: (s.w, s.h).into(), refresh: 60_000 };
    o.change_current_state(Some(mode), Some(Transform::Normal), Some(Scale::Fractional(s.scale)), None);
    o.set_preferred(mode);
    o
}

/// One output's picture in memory and what is known to be on it.
struct Screen {
    output: Output,
    image: Image<'static, 'static>,
    damage: OutputDamageTracker,
    /// the picture holds the last frame (buffer age 1); false after it was made new
    drawn: bool,
    /// something on it changed since a copy that waits for a change (copy_with_damage) was last made
    changed: bool,
}

pub struct Headless {
    renderer: PixmanRenderer,
    /// a picture for each output in the space (screens that are switched off or gone have none)
    screens: Vec<Screen>,
    /// the number the next screen plugged in gets (HEADLESS-n; never one used before)
    next: usize,
    /// a frame is already on its way (the wake-ups before it are folded into it)
    pending: bool,
    last: Instant,
    start: Instant,
}

/// About 60 frames a second at most, however often programs commit.
const FRAME: Duration = Duration::from_millis(16);

fn new_image(renderer: &mut PixmanRenderer, o: &Output) -> Result<Image<'static, 'static>, String> {
    let size = o.current_mode().map(|m| m.size).unwrap_or((1, 1).into());
    renderer
        .create_buffer(Fourcc::Xrgb8888, (size.w, size.h).into())
        .map_err(|e| format!("cannot make a {}x{} picture: {e}", size.w, size.h))
}

impl Headless {
    fn new(next: usize) -> Result<Headless, String> {
        let renderer = PixmanRenderer::new().map_err(|e| format!("the software renderer did not start: {e}"))?;
        Ok(Headless { renderer, screens: vec![], next, pending: false, last: Instant::now() - FRAME, start: Instant::now() })
    }

    /// A picture for every output in the space, none for those that left it.
    fn sync(&mut self, app: &App) {
        self.screens.retain(|s| app.space.outputs().any(|o| *o == s.output));
        for o in app.space.outputs() {
            if self.screens.iter().any(|s| s.output == *o) {
                continue;
            }
            match new_image(&mut self.renderer, o) {
                Ok(image) => self.screens.push(Screen {
                    output: o.clone(),
                    image,
                    damage: OutputDamageTracker::from_output(o),
                    drawn: false,
                    changed: true,
                }),
                Err(e) => eprintln!("yggdrasil-compositor: {e}"),
            }
        }
    }

    /// Draw every output (only what changed), then tell the programs their frame is done.
    fn render(&mut self, app: &mut App) {
        self.sync(app);
        // a window that has just closed: its last picture, from the screen, for its closing animation
        for c in app.closing.iter_mut().filter(|c| c.picture.is_none()) {
            if let Some((rect, picture)) = self.picture_of(c.rect, app.look.border) {
                c.rect = rect;
                c.picture = Some(picture);
            }
        }
        app.closing.retain(|c| c.picture.is_some());
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
            let changed = |r: Result<smithay::backend::renderer::damage::RenderOutputResult<'_>, _>| match r {
                Ok(r) => Some(r.damage.is_some_and(|d| !d.is_empty())),
                Err(_) => None,
            };
            let done = if !app.power.is_on(&s.output) {
                // switched off (wlr-output-power): black, as a screen that is off
                let none: Vec<state::PixElem> = vec![];
                changed(s.damage.render_output(&mut self.renderer, &mut target, age, &none, [0.0f32, 0.0, 0.0, 1.0]))
            } else if app.locked {
                let elements = app.lock_elements(&mut self.renderer, &s.output);
                changed(s.damage.render_output(&mut self.renderer, &mut target, age, &elements, [0.0f32, 0.0, 0.0, 1.0]))
            } else {
                let elements = app.scene(&mut self.renderer, &s.output);
                changed(s.damage.render_output(&mut self.renderer, &mut target, age, &elements, [0.05f32, 0.05, 0.08, 1.0]))
            };
            s.drawn = done.is_some();
            s.changed |= done.unwrap_or(false) || age == 0;
            if done == Some(true) && !app.capture.sessions.is_empty() {
                app.capture_output_changed(&s.output, false);
            }
        }
        self.serve_copies(app);
        self.serve_captures(app);
        self.last = Instant::now();
        // an animation running: the next frame (at most one every 16 ms)
        if app.animating() {
            app.wake();
        }
        app.send_frames(self.start.elapsed());
        app.space.refresh();
        app.foreign_refresh();
        app.popups.cleanup();
        let _ = app.display_handle.flush_clients();
    }

    /// The screen copies asked for (wlr-screencopy), made from the pictures just drawn. A copy that waits for a change waits on.
    fn serve_copies(&mut self, app: &mut App) {
        if app.screencopy.pending.is_empty() {
            return;
        }
        let when: Duration = smithay::utils::Clock::<smithay::utils::Monotonic>::new().now().into();
        let mut served: Vec<String> = vec![];
        for c in std::mem::take(&mut app.screencopy.pending) {
            let Some(s) = self.screens.iter().find(|s| s.output == c.output) else {
                c.frame.failed();
                continue;
            };
            let key = smithay::reexports::wayland_server::Resource::client(&c.frame).map(|cl| (cl.id(), s.output.name()));
            let first = key.as_ref().is_some_and(|k| !app.screencopy.seen.contains(k));
            if c.with_damage && !first && !s.changed {
                app.screencopy.pending.push(c);
                continue;
            }
            if let Some(k) = key {
                app.screencopy.seen.insert(k);
            }
            if c.with_damage {
                served.push(s.output.name());
            }
            let pointer = if c.cursor { app.pointer_picture(&s.output) } else { None };
            let (bytes, stride) = pixels(&s.image);
            crate::screencopy::fill(&c, bytes, stride, pointer.as_ref().map(|(p, at)| (&**p, *at)), when);
        }
        for s in &mut self.screens {
            if served.contains(&s.output.name()) {
                s.changed = false;
            }
        }
    }

    /// The window and screen copies asked for (ext-image-copy-capture), made after the frame is drawn. A copy after the first in a
    /// session waits until its source changed; one whose source changed size fails, with the new size told (the program asks
    /// again with a buffer of that size).
    fn serve_captures(&mut self, app: &mut App) {
        use crate::capture::Source;
        use smithay::reexports::wayland_protocols::ext::image_copy_capture::v1::server::ext_image_copy_capture_frame_v1::FailureReason;
        if app.capture.pending.is_empty() {
            return;
        }
        let when: Duration = smithay::utils::Clock::<smithay::utils::Monotonic>::new().now().into();
        for p in std::mem::take(&mut app.capture.pending) {
            let Some(i) = app.capture.sessions.iter().position(|s| s.resource == p.session) else {
                p.frame.failed(FailureReason::Stopped);
                continue;
            };
            let source = app.capture.sessions[i].source.clone();
            let size = app.capture_size(&source);
            let s = &mut app.capture.sessions[i];
            let Some(size) = size else {
                if !s.stopped {
                    s.stopped = true;
                    s.resource.stopped();
                }
                p.frame.failed(FailureReason::Stopped);
                continue;
            };
            if size != s.told {
                s.told = size;
                crate::capture::constraints(&s.resource, size);
                p.frame.failed(FailureReason::BufferConstraints);
                continue;
            }
            if !s.fresh && !s.dirty {
                app.capture.pending.push(p);
                continue;
            }
            s.fresh = false;
            s.dirty = false;
            let cursors = s.cursors;
            match source {
                Source::Window(surface) => {
                    // nothing of the session is copied while it is locked
                    let w = app.window_for(&surface).filter(|_| !app.locked);
                    let Some(w) = w else {
                        p.frame.failed(FailureReason::Unknown);
                        continue;
                    };
                    let scale = app.window_output(&w).current_scale().fractional_scale();
                    match self.window_picture(&w, size, scale) {
                        Some(image) => {
                            let (bytes, stride) = pixels(&image);
                            crate::capture::fill(&p, bytes, stride, size, None, when);
                        }
                        None => p.frame.failed(FailureReason::Unknown),
                    }
                }
                Source::Output(o) => {
                    let Some(scr) = self.screens.iter().find(|x| x.output == o) else {
                        p.frame.failed(FailureReason::Stopped);
                        continue;
                    };
                    let pointer = if cursors { app.pointer_picture(&o) } else { None };
                    let (bytes, stride) = pixels(&scr.image);
                    crate::capture::fill(&p, bytes, stride, size, pointer.as_ref().map(|(p, at)| (&**p, *at)), when);
                }
                Source::Gone => p.frame.failed(FailureReason::Stopped),
            }
        }
    }

    /// A window drawn on its own, `size` pixels at `scale` (its own picture and its menus, without the compositor's border and
    /// shadow; transparent where it draws nothing): its copy, whether it is covered, on another desktop or minimized. (Drawn into
    /// a picture of its own and then copied: Smithay cannot draw into a program's shared memory while it reads the window's.)
    fn window_picture(&mut self, w: &smithay::desktop::Window, size: (u32, u32), scale: f64) -> Option<Image<'static, 'static>> {
        use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
        use smithay::backend::renderer::element::AsRenderElements;
        let mut image = self.renderer.create_buffer(Fourcc::Argb8888, (size.0 as i32, size.1 as i32).into()).ok()?;
        let geo = w.geometry();
        let at = smithay::utils::Point::<i32, smithay::utils::Physical>::from((
            -(geo.loc.x as f64 * scale).round() as i32,
            -(geo.loc.y as f64 * scale).round() as i32,
        ));
        let elements: Vec<WaylandSurfaceRenderElement<PixmanRenderer>> =
            w.render_elements(&mut self.renderer, at, smithay::utils::Scale::from(scale), 1.0);
        let mut damage = OutputDamageTracker::new((size.0 as i32, size.1 as i32), scale, Transform::Normal);
        let mut target = self.renderer.bind(&mut image).ok()?;
        damage.render_output(&mut self.renderer, &mut target, 0, &elements, [0.0f32, 0.0, 0.0, 0.0]).ok()?;
        drop(target);
        Some(image)
    }

    /// The pixels of the screen under `rect` (logical, with `border` around it): what a window looked like as it closed. The
    /// rectangle they cover (cut to the screen) and the picture with the screen's name.
    fn picture_of(
        &self,
        rect: crate::look::RectF,
        border: i32,
    ) -> Option<(crate::look::RectF, (smithay::backend::renderer::element::memory::MemoryRenderBuffer, String))> {
        let mid = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
        let s = self.screens.iter().find(|s| state::output_rect(&s.output).to_f64().contains(mid))?;
        if !s.drawn {
            return None;
        }
        let (scale, origin) = (s.output.current_scale().fractional_scale(), s.output.current_location());
        let b = border as f64;
        let outer = crate::look::RectF { x: rect.x - b, y: rect.y - b, w: rect.w + 2.0 * b, h: rect.h + 2.0 * b };
        let p = outer.physical(origin, scale);
        let screen =
            smithay::utils::Rectangle::<i32, smithay::utils::Physical>::from_size((s.image.width() as i32, s.image.height() as i32).into());
        let p = p.intersection(screen).filter(|p| p.size.w > 0 && p.size.h > 0)?;
        let (bytes, stride) = pixels(&s.image);
        let mut data = Vec::with_capacity((p.size.w * p.size.h * 4) as usize);
        for y in p.loc.y..p.loc.y + p.size.h {
            let row = y as usize * stride;
            data.extend_from_slice(&bytes[row + p.loc.x as usize * 4..row + (p.loc.x + p.size.w) as usize * 4]);
        }
        let buffer = smithay::backend::renderer::element::memory::MemoryRenderBuffer::from_slice(
            &data,
            Fourcc::Xrgb8888,
            (p.size.w, p.size.h),
            1,
            Transform::Normal,
            None,
        );
        let covered = crate::look::RectF {
            x: origin.x as f64 + p.loc.x as f64 / scale,
            y: origin.y as f64 + p.loc.y as f64 / scale,
            w: p.size.w as f64 / scale,
            h: p.size.h as f64 / scale,
        };
        Some((covered, (buffer, s.output.name())))
    }

    /// Milliseconds since the start (the time stamp of input made here).
    fn ms(&self) -> u32 {
        self.start.elapsed().as_millis() as u32
    }

    fn screen(&self, name: &str) -> Option<&Screen> {
        self.screens.iter().find(|s| s.output.name() == name)
    }
}

/// An output's picture: XRGB8888, rows `stride` bytes apart.
fn pixels<'a>(image: &'a Image<'static, 'static>) -> (&'a [u8], usize) {
    let (h, stride) = (image.height(), image.stride());
    // SAFETY: the picture is h rows `stride` bytes apart, alive (and not drawn to) while `image` is borrowed here
    (unsafe { std::slice::from_raw_parts(image.data() as *const u8, stride * h) }, stride)
}

/// Write an output's picture as an 8-bit RGB PNG, the pointer painted over it when given.
fn write_png(image: &Image<'static, 'static>, path: &Path, pointer: Option<(&crate::pointer::Picture, (i32, i32))>) -> Result<(), String> {
    let (w, h) = (image.width(), image.height());
    let (bytes, stride) = pixels(image);
    let mut with_pointer;
    let bytes = match pointer {
        Some((pic, at)) => {
            with_pointer = bytes.to_vec();
            crate::pointer::blend(&mut with_pointer, stride, (w as u32, h as u32), pic, at);
            &with_pointer[..]
        }
        None => bytes,
    };
    let mut rgb = Vec::with_capacity(w * h * 3);
    for row in bytes.chunks(stride).take(h) {
        for px in row[..w * 4].as_chunks::<4>().0 {
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
pub fn run(
    mut event_loop: EventLoop<'static, App>,
    mut app: App,
    ping_source: smithay::reexports::calloop::ping::PingSource,
    runtime: PathBuf,
) -> Result<(), Box<dyn std::error::Error>> {
    let h = Rc::new(RefCell::new(Headless::new(app.heads.list.len() + 1)?));
    let handle = event_loop.handle();
    // a closing window's last picture can be taken from the screens in memory
    app.pictures_of_closing = true;

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
    // the desktop commands Yggdrasil's own tools use (wmctl.rs) come first
    if let Some(r) = crate::wmctl::command(words, app) {
        return r;
    }
    let output = |name: &str, app: &App| app.head(name).map(|hd| hd.output.clone()).ok_or_else(|| format!("no output {name}"));
    match words {
        ["outputs"] => Ok(app
            .heads
            .list
            .iter()
            .map(|hd| {
                let o = &hd.output;
                let m = o.current_mode().map(|m| m.size).unwrap_or_default();
                let l = state::logical_size(o);
                let at = o.current_location();
                format!(
                    "{} {}x{} scale={} at={},{} logical={}x{}{}{}",
                    o.name(),
                    m.w,
                    m.h,
                    o.current_scale().fractional_scale(),
                    at.x,
                    at.y,
                    l.w,
                    l.h,
                    if app.power.is_on(o) { "" } else { " power=off" },
                    if hd.enabled { "" } else { " disabled" }
                )
            })
            .collect()),
        ["resize", name, size] => {
            let o = output(name, app)?;
            let (w, hh) = parse_size(size).ok_or_else(|| format!("{size}: give WIDTHxHEIGHT"))?;
            let mode = Mode { size: (w, hh).into(), refresh: 60_000 };
            // another screen of this size: its own mode is that one, and the old one is gone
            for old in [o.current_mode(), o.preferred_mode()].into_iter().flatten().filter(|m| *m != mode) {
                o.delete_mode(old);
            }
            o.set_preferred(mode);
            let mut want = app.head(name).ok_or("the output went away")?.now();
            want.mode = mode;
            app.configure_outputs(vec![(name.to_string(), want)], false)?;
            Ok(vec![])
        }
        ["scale", name, s] => {
            let v = parse_scale(s).ok_or_else(|| format!("{s}: give a number from 0.25 to 8"))?;
            let mut want = app.head(name).ok_or_else(|| format!("no output {name}"))?.now();
            want.scale = v;
            app.configure_outputs(vec![(name.to_string(), want)], false)?;
            Ok(vec![])
        }
        ["add-output", spec] => {
            let spec = parse_output(spec).ok_or_else(|| format!("{spec}: give WIDTHxHEIGHT or WIDTHxHEIGHT@SCALE"))?;
            if app.heads.list.len() >= 16 {
                return Err("16 outputs at most".into());
            }
            let o = make_output(h.next, &spec);
            h.next += 1;
            let name = o.name();
            app.add_output(o, spec.scale);
            Ok(vec![name])
        }
        ["remove-output", name] => {
            app.remove_output(name)?;
            Ok(vec![])
        }
        ["screenshot", name, path, rest @ ..] if rest.is_empty() || rest == ["pointer"] => {
            if app.head(name).is_none_or(|hd| !hd.enabled) {
                return Err(format!("no output {name} that is on"));
            }
            h.render(app);
            let o = output(name, app)?;
            let pointer = if rest.is_empty() { None } else { app.pointer_picture(&o) };
            let s = h.screen(name).ok_or("the output went away")?;
            write_png(&s.image, Path::new(path), pointer.as_ref().map(|(p, at)| (&**p, *at)))?;
            Ok(vec![])
        }
        ["pointer-look"] => Ok(vec![app.pointer_look()]),
        ["windows"] => Ok(app
            .window_infos()
            .into_iter()
            .map(|i| {
                let (title, app_id) = state::title_app(&i.window);
                let size = i.window.geometry().size;
                let (title, app_id) = (crate::wmctl::field(&title), crate::wmctl::field(&app_id));
                format!("{app_id}\t{title}\t{},{}\t{}x{}\t{}\t{}", i.at.x, i.at.y, size.w, size.h, i.workspace + 1, i.states.join(","))
            })
            .collect()),
        ["focus"] => Ok(vec![app.focus_info()]),
        ["free"] => Ok(app
            .space
            .outputs()
            .map(|o| {
                let r = app.free_area(o);
                format!("{}\t{},{}\t{}x{}", o.name(), r.loc.x, r.loc.y, r.size.w, r.size.h)
            })
            .collect()),
        ["workspace"] => Ok(vec![(app.workspace() + 1).to_string()]),
        ["lock"] => {
            let mut lines = vec![if app.locked { "locked" } else { "unlocked" }.to_string()];
            for (o, size) in app.lock_infos() {
                let (w, hh) = size.map(|s| (s.w, s.h)).unwrap_or((0, 0));
                lines.push(format!("{}\t{w}x{hh}", o.name()));
            }
            Ok(lines)
        }
        ["key", combos @ ..] if !combos.is_empty() => {
            let mut all = vec![];
            for c in combos {
                all.push(combo(c).ok_or_else(|| format!("{c}: unknown key (names: super, ctrl, alt, shift, Return, Escape, Tab, Left ..., a-z, 0-9, F1-F12)"))?);
            }
            for keys in all {
                for k in &keys {
                    press(app, h, *k, true);
                }
                for k in keys.iter().rev() {
                    press(app, h, *k, false);
                }
            }
            Ok(vec![])
        }
        ["key-down" | "key-up", c] => {
            let keys = combo(c).ok_or_else(|| format!("{c}: unknown key"))?;
            let down = words[0] == "key-down";
            if down {
                for k in &keys {
                    press(app, h, *k, true);
                }
            } else {
                for k in keys.iter().rev() {
                    press(app, h, *k, false);
                }
            }
            Ok(vec![])
        }
        ["type", text @ ..] if !text.is_empty() => {
            let text = text.join(" ");
            let mut all = vec![];
            for c in text.chars() {
                all.push(char_key(c).ok_or_else(|| format!("{c:?} cannot be typed here"))?);
            }
            for (code, shift) in all {
                if shift {
                    press(app, h, KEY_LEFTSHIFT, true);
                }
                press(app, h, code, true);
                press(app, h, code, false);
                if shift {
                    press(app, h, KEY_LEFTSHIFT, false);
                }
            }
            Ok(vec![])
        }
        ["pointer", x, y] => {
            let (x, y) = (x.parse::<f64>().map_err(|_| "pointer X Y: numbers")?, y.parse::<f64>().map_err(|_| "pointer X Y: numbers")?);
            let t = h.ms();
            app.pointer_to((x, y).into(), t);
            Ok(vec![])
        }
        ["click", rest @ ..] if rest.len() <= 1 => {
            let button = match rest.first().copied().unwrap_or("left") {
                "left" => 0x110,
                "right" => 0x111,
                "middle" => 0x112,
                b => return Err(format!("{b}: left, right or middle")),
            };
            // a press of the virtual mouse's button: left-handed, its left one is the right button
            let button = crate::input::button(&crate::input::mouse(&app.input, crate::input::HEADLESS_MOUSE), button);
            let t = h.ms();
            app.button(button, smithay::backend::input::ButtonState::Pressed, t);
            app.button(button, smithay::backend::input::ButtonState::Released, t + 1);
            Ok(vec![])
        }
        ["pointer-move", dx, dy] => {
            let num = |v: &str| v.parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 65536.0).ok_or("pointer-move DX DY: numbers");
            let m = crate::input::mouse(&app.input, crate::input::HEADLESS_MOUSE);
            let (mx, my) = crate::input::motion(&m, (num(dx)?, num(dy)?));
            let to = app.pointer_location + smithay::utils::Point::from((mx, my));
            let to = app.nearest_on_screens(to).unwrap_or(to);
            let t = h.ms();
            app.pointer_to(to, t);
            Ok(vec![])
        }
        ["scroll", dy, rest @ ..] if rest.len() <= 1 => {
            use smithay::backend::input::{Axis, AxisSource};
            let num = |v: &str| v.parse::<f64>().ok().filter(|v| v.is_finite() && v.abs() <= 10_000.0).ok_or("scroll DY [DX]: numbers");
            let m = crate::input::mouse(&app.input, crate::input::HEADLESS_MOUSE);
            let (vy, vx) = (crate::input::scroll(&m, num(dy)?), crate::input::scroll(&m, rest.first().map(|v| num(v)).transpose()?.unwrap_or(0.0)));
            let t = h.ms();
            app.wake();
            let pointer = app.pointer.clone();
            let mut frame = smithay::input::pointer::AxisFrame::new(t).source(AxisSource::Wheel);
            for (axis, v) in [(Axis::Vertical, vy), (Axis::Horizontal, vx)] {
                if v != 0.0 {
                    frame = frame.value(axis, v);
                }
            }
            pointer.axis(app, frame);
            pointer.frame(app);
            Ok(vec![])
        }
        ["input"] => {
            let (rate, delay) = crate::input::repeat(&app.input);
            let m = crate::input::mouse(&app.input, crate::input::HEADLESS_MOUSE);
            Ok(vec![
                format!("{}	repeat_rate={rate}	repeat_delay={delay}", crate::input::HEADLESS_KEYBOARD),
                crate::input::describe(crate::input::HEADLESS_MOUSE, &m),
            ])
        }
        ["power", name, on] => {
            let o = output(name, app)?;
            let on = match *on {
                "on" => true,
                "off" => false,
                v => return Err(format!("{v}: on or off")),
            };
            app.set_output_power(&o, on);
            Ok(vec![])
        }
        ["clock"] => Ok(vec![format!("{} {}", if app.clock.running() { "running" } else { "stopped" }, app.clock.now().as_millis())]),
        ["clock", "stop"] => {
            app.clock.stop();
            Ok(vec![])
        }
        ["clock", "run"] => {
            app.clock.run();
            app.wake();
            Ok(vec![])
        }
        ["clock", step] if step.starts_with('+') => {
            let ms: u64 = step[1..].parse().map_err(|_| format!("{step}: +MILLISECONDS"))?;
            if app.clock.running() {
                return Err("the clock runs: clock stop first".into());
            }
            app.clock.advance(Duration::from_millis(ms.min(60_000)));
            app.wake();
            Ok(vec![])
        }
        ["quit"] => {
            app.quit();
            Ok(vec![])
        }
        [] => Err("no command".into()),
        _ => Err(format!(
            "unknown command {:?} (outputs, resize NAME WxH, scale NAME S, add-output WxH[@S], remove-output NAME, screenshot NAME FILE [pointer], pointer-look, windows, layers, free, focus, workspace, lock, key COMBO, key-down COMBO, key-up COMBO, type TEXT, pointer X Y, click [BUTTON], pointer-move DX DY, scroll DY [DX], input, power NAME on|off, clock [stop|run|+MS], quit; for the tools: clients, active, workspaces, workspace N, activate ID, close ID, minimize ID, maximize ID on|off|toggle, send ID N, place ID X Y W H, cursor, screens, pointer-timeout [S], modifiers)",
            words.join(" ")
        )),
    }
}

const KEY_LEFTSHIFT: u32 = 42;

/// A key's kernel code (evdev) by name: modifiers, the named keys, letters, digits and F1 to F12 (US layout).
fn key_code(name: &str) -> Option<u32> {
    let n = name.to_ascii_lowercase();
    let named = match n.as_str() {
        "super" | "meta" | "logo" | "win" => 125,
        "ctrl" | "control" => 29,
        "rctrl" => 97,
        "alt" => 56,
        "shift" => KEY_LEFTSHIFT,
        "altgr" => 100,
        "return" | "enter" => 28,
        "escape" | "esc" => 1,
        "tab" => 15,
        "space" => 57,
        "backspace" => 14,
        "delete" => 111,
        "up" => 103,
        "down" => 108,
        "left" => 105,
        "right" => 106,
        "home" => 102,
        "end" => 107,
        "pageup" => 104,
        "pagedown" => 109,
        "print" => 99,
        "equal" => 13,
        "minus" => 12,
        "slash" => 53,
        "xf86audiopause" => 201,
        "f11" => 87,
        "f12" => 88,
        "xf86audiomute" | "mute" => 113,
        "xf86audiolowervolume" | "volumedown" => 114,
        "xf86audioraisevolume" | "volumeup" => 115,
        "xf86audionext" => 163,
        "xf86audioplay" => 164,
        "xf86audioprev" => 165,
        "xf86audiostop" => 166,
        "xf86monbrightnessdown" => 224,
        "xf86monbrightnessup" => 225,
        "xf86audiomicmute" => 248,
        _ => 0,
    };
    if named != 0 {
        return Some(named);
    }
    if let Some(f) = n.strip_prefix('f').and_then(|d| d.parse::<u32>().ok()).filter(|d| (1..=10).contains(d)) {
        return Some(58 + f);
    }
    let mut chars = n.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => char_key(c).filter(|(_, shift)| !shift).map(|(code, _)| code),
        _ => None,
    }
}

/// The key that types a character (US layout), and whether Shift is held for it.
fn char_key(c: char) -> Option<(u32, bool)> {
    const ROWS: [(&str, u32); 4] = [("1234567890-=", 2), ("qwertyuiop[]", 16), ("asdfghjkl;'`", 30), ("zxcvbnm,./", 44)];
    const SHIFTED: [(&str, u32); 4] = [("!@#$%^&*()_+", 2), ("QWERTYUIOP{}", 16), ("ASDFGHJKL:\"~", 30), ("ZXCVBNM<>?", 44)];
    match c {
        ' ' => return Some((57, false)),
        '\n' => return Some((28, false)),
        '\\' => return Some((43, false)),
        '|' => return Some((43, true)),
        _ => {}
    }
    for (table, shift) in [(ROWS, false), (SHIFTED, true)] {
        for (row, first) in table {
            if let Some(i) = row.chars().position(|x| x == c) {
                return Some((first + i as u32, shift));
            }
        }
    }
    None
}

/// "super+ctrl+2": the keys of a combination, in the order they are pressed.
fn combo(s: &str) -> Option<Vec<u32>> {
    s.split('+').map(key_code).collect()
}

/// One key down or up, as if from a keyboard.
fn press(app: &mut App, h: &Headless, code: u32, down: bool) {
    use smithay::backend::input::KeyState;
    let state = if down { KeyState::Pressed } else { KeyState::Released };
    app.key((code + 8).into(), state, h.ms());
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
        let found: Vec<PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("yggdrasil-compositor-") && n.ends_with(".ctl"))
            })
            .collect();
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
    fn the_desktops_session_bus_is_not_passed_on() {
        assert!(desktop_bus("unix:path=/run/user/1000/bus", 1000));
        assert!(desktop_bus("unix:abstract=/tmp/dbus-x", 1000));
        assert!(!desktop_bus("unix:path=/tmp/ygg-hl-1/bus", 1000), "a test's own bus is");
    }

    #[test]
    fn the_backend_is_chosen() {
        assert!(wanted(&v(&["--headless"])));
        assert!(wanted(&v(&["--backend", "headless"])));
        assert!(wanted(&v(&["--backend=headless"])));
        assert!(!wanted(&v(&["--backend", "winit"])));
    }
}
