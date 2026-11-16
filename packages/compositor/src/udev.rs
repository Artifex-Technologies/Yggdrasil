//! The hardware backend (DRM/KMS): the compositor drives the screens itself, on a virtual terminal, instead of running in a window
//! or headless. This is what will let it replace Hyprland; it is developed in a virtual machine, never on the running desktop.
//!
//!   yggdrasil-compositor --drm [--run COMMAND ...]        (`--backend drm` and `--backend udev` are the same)
//!
//! Started from a text console (a virtual terminal), as the user logged in there:
//! - **Seat**: libseat (logind or seatd) opens the GPUs and the input devices without root and takes them away on a switch to
//!   another virtual terminal (Ctrl+Alt+F1..F12, `App::vt_switch`): the screens stop being drawn and libinput is suspended while
//!   another terminal shows; coming back takes them again and draws every screen whole.
//! - **GPUs and screens**: udev lists the GPUs (the boot one first) and tells when one comes or goes, or when a screen is plugged
//!   in or out (a "change" of the GPU: its connectors are looked at again). Every connected connector (not a VR headset's,
//!   "non-desktop") gets a CRTC and becomes an `Output` named as the kernel names it (DP-1, HDMI-A-1, eDP-1, Virtual-1) with
//!   all its modes, its size in millimetres and the make and model from its EDID; Settings > Displays' monitors.lua then sets
//!   its mode, scale and place (outputs.rs, the same code as headless), and a mode changed later (wlr-output-management) is
//!   set on the CRTC at the next frame.
//! - **Drawing**: on a GPU with hardware EGL, GBM buffers drawn with OpenGL ES (GlesRenderer); otherwise (no 3D: a plain
//!   virtual GPU, a simple framebuffer, YGGDRASIL_COMPOSITOR_RENDERER=pixman) dumb buffers drawn in software (pixman), so a
//!   machine with no working GPU driver still gets a picture. Each screen has its own Smithay `DrmCompositor`, which puts the
//!   pointer on the cursor plane when it can.
//! - **Frames**: drawn only when something changed (`App::wake`), then not again before that frame was shown (the page flip's
//!   vblank event); a frame with no change is not even sent, and a run of frames with no change waits a refresh between them.
//!   Nothing is drawn and no timer runs while nothing changes.
//! - **Input**: libinput (keyboards, mice, touchpads, a virtual machine's tablet); Settings > Peripherals' input.conf sets
//!   libinput's speed, profile, natural scrolling and left-handed buttons per device, and its scroll factor is applied here.
//! - **The control socket** (control.rs) is there too, for Yggdrasil's own tools (yggdrasil-wm); it makes no input.
//! - **Ending**: `ctl quit`, SIGTERM, SIGINT or SIGHUP end it cleanly: the session is closed and the console comes back.
//!
//! Not yet (later milestones, docs/COMPOSITOR.md): several GPUs drawing for each other (a laptop's NVIDIA GPU behind the Intel
//! one), explicit sync (linux-drm-syncobj-v1, which NVIDIA needs), direct scanout of full screen games tuned per GPU, VRR, tearing
//! control, a screen switched off through DPMS (wlr-output-power draws it black for now), screen copies (wlr-screencopy,
//! ext-image-copy-capture) and the lid switch.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::dumb::DumbAllocator;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Format, Fourcc, Modifier};
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmNode, NodeType};
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::Kind;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode as LoopMode, PostAction, RegistrationToken};
use smithay::reexports::drm::control::{connector, crtc, Device as _, ModeTypeFlags};
use smithay::reexports::input::{AccelProfile, DeviceCapability, Libinput};
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{DeviceFd, Transform};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::state::{App, Elem, PixElem};

/// The backend was asked for: `--drm`, `--backend drm` or `--backend udev` (`=` forms too).
pub fn wanted(opts: &[String]) -> bool {
    opts.iter().any(|a| a == "--drm" || a == "--backend=drm" || a == "--backend=udev")
        || opts.windows(2).any(|w| w[0] == "--backend" && (w[1] == "drm" || w[1] == "udev"))
}

/// Refuse to start inside a running desktop (it would fight that desktop for the GPU's screens; libseat would refuse anyway, as
/// the desktop's session already holds them), and clear what leads to another compositor before anything starts.
pub fn check_env() -> Result<(), String> {
    let inside = ["WAYLAND_DISPLAY", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "NIRI_SOCKET"]
        .into_iter()
        .find(|v| std::env::var_os(v).is_some_and(|x| !x.is_empty()));
    if let Some(v) = inside {
        if std::env::var_os("YGGDRASIL_COMPOSITOR_DRM_FORCE").is_none() {
            return Err(format!(
                "--drm drives the screens itself: start it from a text console (a virtual terminal), not inside a desktop ({v} is set; YGGDRASIL_COMPOSITOR_DRM_FORCE=1 starts it anyway)"
            ));
        }
    }
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        return Err("--drm needs XDG_RUNTIME_DIR (a login session's /run/user/UID)".into());
    }
    for v in ["WAYLAND_DISPLAY", "WAYLAND_SOCKET", "DISPLAY", "HYPRLAND_INSTANCE_SIGNATURE", "SWAYSOCK", "I3SOCK", "NIRI_SOCKET"] {
        std::env::remove_var(v);
    }
    Ok(())
}

/// The renderer a GPU draws with: OpenGL ES on hardware EGL, else software (pixman) into dumb buffers.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Renderer {
    Gles,
    Pixman,
}

/// YGGDRASIL_COMPOSITOR_RENDERER: `pixman` forces software drawing, `gles` refuses to fall back to it.
fn renderer_wanted() -> Option<Renderer> {
    match std::env::var("YGGDRASIL_COMPOSITOR_RENDERER").ok()?.as_str() {
        "pixman" | "software" => Some(Renderer::Pixman),
        "gles" | "gl" => Some(Renderer::Gles),
        _ => None,
    }
}

type GbmManager = DrmOutputManager<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type GbmOut = DrmOutput<GbmAllocator<DrmDeviceFd>, GbmFramebufferExporter<DrmDeviceFd>, (), DrmDeviceFd>;
type DumbManager = DrmOutputManager<Rc<RefCell<DumbAllocator>>, DrmDeviceFd, (), DrmDeviceFd>;
type DumbOut = DrmOutput<Rc<RefCell<DumbAllocator>>, DrmDeviceFd, (), DrmDeviceFd>;

/// A GPU's way of drawing and its screens' manager.
enum Gpu {
    Gles { renderer: Box<GlesRenderer>, manager: GbmManager },
    Pixman { renderer: Box<PixmanRenderer>, manager: DumbManager },
}

enum Out {
    Gles(GbmOut),
    Pixman(DumbOut),
}

/// One screen: its output, the CRTC driving it and that CRTC's compositor, and where its frames are.
struct Screen {
    output: Output,
    connector: connector::Handle,
    drm: Out,
    /// the connector's modes (a mode chosen in Settings is looked up here)
    modes: Vec<smithay::reexports::drm::control::Mode>,
    /// the mode the CRTC is set to
    mode: Mode,
    /// a frame was queued and its page flip has not happened yet
    queued: bool,
    /// something changed since it was last drawn
    dirty: bool,
    /// a frame with no change was drawn: the next one waits until then (a program that commits without changing anything
    /// does not spin the compositor)
    not_before: Instant,
}

struct Device {
    node: DrmNode,
    gpu: Gpu,
    screens: HashMap<crtc::Handle, Screen>,
    token: RegistrationToken,
}

pub struct Udev {
    session: LibSeatSession,
    /// the GPU that renders for programs (dmabuf): the boot one
    primary: DrmNode,
    devices: HashMap<DrmNode, Device>,
    libinput: Libinput,
    /// keyboards, mice and touchpads, set up as input.conf says (again when it changes)
    inputs: Vec<smithay::reexports::input::Device>,
    input_seen: Option<yggdrasil_wm::input::InputConf>,
    /// a draw is already asked for (the wake-ups of one pass are folded into it), and the timer for screens that wait
    idle_pending: bool,
    timer_pending: bool,
    /// the pointer's last picture and its buffer (uploaded again only when the picture changes)
    pointer: Option<(Arc<crate::pointer::Picture>, MemoryRenderBuffer)>,
    start: Instant,
}

const BACKGROUND: [f32; 4] = [0.05, 0.05, 0.08, 1.0];
const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Run the hardware backend until `quit`, a signal, or the loop is stopped.
pub fn run(run: Vec<Vec<String>>) -> Result<(), Box<dyn std::error::Error>> {
    check_env()?;
    // the signals that end it are taken through a signalfd (blocked here, before any thread or child exists; the programs it
    // starts get them unblocked again: main.rs plain_signals. XWayland, which Smithay starts, keeps them blocked; it ends with
    // the compositor)
    let signals = block_signals()?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let display: Display<App> = Display::new()?;
    let dh = display.handle();
    let handle = event_loop.handle();
    let (ping, ping_source) = smithay::reexports::calloop::ping::make_ping()?;

    let (session, notifier) = LibSeatSession::new().map_err(|e| format!("no seat (logind or seatd): {e}"))?;
    let seat = session.seat();
    eprintln!("yggdrasil-compositor: seat {seat}");
    let primary = udev::primary_gpu(&seat)
        .ok()
        .flatten()
        .and_then(|p| DrmNode::from_path(p).ok())
        .or_else(|| udev::all_gpus(&seat).ok()?.into_iter().find_map(|p| DrmNode::from_path(p).ok()))
        .ok_or("no GPU found")?;
    let udev_backend = UdevBackend::new(&seat)?;

    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput.udev_assign_seat(&seat).map_err(|_| "libinput could not take the seat's input devices")?;

    let udev = Rc::new(RefCell::new(Udev {
        session: session.clone(),
        primary,
        devices: HashMap::new(),
        libinput: libinput.clone(),
        inputs: vec![],
        input_seen: None,
        idle_pending: false,
        timer_pending: false,
        pointer: None,
        start: Instant::now(),
    }));

    // the GPUs: the boot one first, so it is the one programs render on
    let mut found: Vec<(DrmNode, std::path::PathBuf)> =
        udev_backend.device_list().filter_map(|(id, p)| Some((DrmNode::from_dev_id(id).ok()?, p.to_path_buf()))).collect();
    found.sort_by_key(|(n, _)| !same_gpu(*n, primary));
    let mut outputs = vec![];
    for (node, path) in &found {
        match device_added(&udev, &handle, *node, path) {
            Ok(mut o) => outputs.append(&mut o),
            Err(e) => eprintln!("yggdrasil-compositor: {}: {e}", path.display()),
        }
    }
    let first = outputs.first().cloned().ok_or("no screen is connected")?;

    crate::toplevels::create_global(&dh);
    // screens off and on (yggdrasil-idle): drawn black for now (DPMS comes later)
    crate::outputpower::create_global(&dh);
    let mut app = App::new(&dh, first, handle.clone(), event_loop.get_signal(), ping);
    for o in outputs {
        app.add_output(o, 1.0);
    }
    app.reload_monitors();
    crate::outputmgmt::create_global(&dh);
    gpu_buffers(&udev, &mut app, &dh);

    // Ctrl+Alt+Fn: another virtual terminal
    let mut vt_session = session.clone();
    app.vt_switch = Some(Box::new(move |n| {
        if let Err(e) = vt_session.change_vt(n) {
            eprintln!("yggdrasil-compositor: cannot switch to terminal {n}: {e}");
        }
    }));

    // a wake-up (a commit, input, a setting): draw the screens that changed, once per pass of the loop
    let u = udev.clone();
    handle.insert_source(ping_source, move |_, _, app| {
        for d in u.borrow_mut().devices.values_mut() {
            for s in d.screens.values_mut() {
                s.dirty = true;
            }
        }
        schedule(&u, &app.loop_handle);
    })?;
    app.wake();

    // switching away to another terminal and back
    let u = udev.clone();
    handle.insert_source(notifier, move |event, _, app| match event {
        SessionEvent::PauseSession => {
            let mut b = u.borrow_mut();
            b.libinput.suspend();
            for d in b.devices.values_mut() {
                match &mut d.gpu {
                    Gpu::Gles { manager, .. } => manager.pause(),
                    Gpu::Pixman { manager, .. } => manager.pause(),
                }
            }
        }
        SessionEvent::ActivateSession => {
            {
                let mut b = u.borrow_mut();
                if b.libinput.resume().is_err() {
                    eprintln!("yggdrasil-compositor: libinput did not resume");
                }
                for d in b.devices.values_mut() {
                    let active = match &mut d.gpu {
                        Gpu::Gles { manager, .. } => manager.activate(false),
                        Gpu::Pixman { manager, .. } => manager.activate(false),
                    };
                    if let Err(e) = active {
                        eprintln!("yggdrasil-compositor: the GPU did not come back: {e}");
                    }
                    for s in d.screens.values_mut() {
                        // the frame on its way was lost with the switch; the terminal drew over the screen: all of it again
                        s.queued = false;
                        s.dirty = true;
                        match &s.drm {
                            Out::Gles(o) => o.reset_buffers(),
                            Out::Pixman(o) => o.reset_buffers(),
                        }
                    }
                }
            }
            // devices may have come and gone meanwhile
            let nodes: Vec<DrmNode> = u.borrow().devices.keys().copied().collect();
            for n in nodes {
                device_changed(&u, app, n);
            }
            app.wake();
        }
    })?;

    // GPUs and screens coming and going
    let u = udev.clone();
    handle.insert_source(udev_backend, move |event, _, app| match event {
        UdevEvent::Added { device_id, path } => {
            let Ok(node) = DrmNode::from_dev_id(device_id) else { return };
            let h = app.loop_handle.clone();
            match device_added(&u, &h, node, &path) {
                Ok(outputs) => {
                    for o in outputs {
                        app.add_output(o, 1.0);
                    }
                    app.wake();
                }
                Err(e) => eprintln!("yggdrasil-compositor: {}: {e}", path.display()),
            }
        }
        UdevEvent::Changed { device_id } => {
            if let Ok(node) = DrmNode::from_dev_id(device_id) {
                device_changed(&u, app, node);
            }
        }
        UdevEvent::Removed { device_id } => {
            if let Ok(node) = DrmNode::from_dev_id(device_id) {
                device_removed(&u, app, node);
            }
        }
    })?;

    // keyboards, mice, touchpads
    let u = udev.clone();
    handle.insert_source(LibinputInputBackend::new(libinput), move |mut event, _, app| {
        if let InputEvent::DeviceAdded { device } = &mut event {
            set_up_input(device, &app.input);
            u.borrow_mut().inputs.push(device.clone());
        } else if let InputEvent::DeviceRemoved { device } = &event {
            u.borrow_mut().inputs.retain(|d| d != device);
        }
        app.on_input(event, true);
    })?;

    // SIGTERM, SIGINT, SIGHUP: end cleanly
    handle.insert_source(Generic::new(signals, Interest::READ, LoopMode::Level), |_, fd, app| {
        let mut buf = [0u8; 128];
        // SAFETY: reading into a buffer of our own from the signalfd (one siginfo is 128 bytes)
        let _ = unsafe { libc::read(std::os::fd::AsRawFd::as_raw_fd(&**fd), buf.as_mut_ptr().cast(), buf.len()) };
        app.quit();
        Ok(PostAction::Continue)
    })?;

    crate::serve(&event_loop, display, &mut app, run, true)?;

    // the control socket, for Yggdrasil's own tools
    let runtime = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_default());
    let (listener, ctl) = crate::control::listen(&runtime, &app.socket)?;
    let control = Rc::new(RefCell::new(crate::control::Control::default()));
    let c = control.clone();
    handle.insert_source(Generic::new(listener, Interest::READ, LoopMode::Level), move |_, l, app| {
        while let Ok((stream, _)) = l.accept() {
            c.borrow_mut().serve(stream, app, &mut |_, _| None, USAGE);
        }
        Ok(PostAction::Continue)
    })?;

    let u = udev.clone();
    event_loop.run(None, &mut app, |app| {
        let _ = app.display_handle.flush_clients();
        control.borrow_mut().tell_watchers(app);
        // input.conf changed (Settings saved it, or the 2-second look found it changed): the devices are set up again
        let mut b = u.borrow_mut();
        if b.input_seen.as_ref() != Some(&app.input) {
            b.input_seen = Some(app.input.clone());
            for d in b.inputs.iter_mut() {
                set_up_input(d, &app.input);
            }
        }
    })?;

    // the screens are let go before the session is closed, and the session before the console is given back
    let _ = std::fs::remove_file(&ctl);
    let _ = std::fs::remove_file(runtime.join(&app.socket));
    let _ = std::fs::remove_file(runtime.join(format!("{}.lock", app.socket)));
    let mut b = udev.borrow_mut();
    for (_, d) in b.devices.drain() {
        event_loop.handle().remove(d.token);
    }
    eprintln!("yggdrasil-compositor: ended");
    Ok(())
}

/// The commands, for an unknown one's answer.
const USAGE: &str = "outputs, scale NAME S, power NAME on|off, pointer-look, windows, focus, free, workspace, lock, watch, quit; for the tools: clients, active, workspaces, workspace N, activate ID, close ID, minimize ID, maximize ID on|off|toggle, send ID N, place ID X Y W H, cursor, screens, pointer-timeout [S], modifiers";

/// Two nodes of the same GPU (its card and its render node).
fn same_gpu(a: DrmNode, b: DrmNode) -> bool {
    a.dev_id() == b.dev_id()
        || a.node_with_type(NodeType::Render).and_then(|n| n.ok()) == b.node_with_type(NodeType::Render).and_then(|n| n.ok())
}

/// Block SIGTERM, SIGINT and SIGHUP and give a signalfd that becomes readable when one comes.
fn block_signals() -> Result<std::os::fd::OwnedFd, String> {
    use std::os::fd::FromRawFd;
    // SAFETY: plain libc calls on a set of our own, before any other thread exists
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return Err("cannot block the signals that end it".into());
        }
        let fd = libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK);
        if fd < 0 {
            return Err(format!("signalfd: {}", std::io::Error::last_os_error()));
        }
        Ok(std::os::fd::OwnedFd::from_raw_fd(fd))
    }
}

/// A GPU appeared (at start, or plugged in): open it through the seat, choose how it draws, and set up its connected screens.
/// The new outputs are returned (the caller hands them to the App).
fn device_added(udev: &Rc<RefCell<Udev>>, handle: &LoopHandle<'static, App>, node: DrmNode, path: &Path) -> Result<Vec<Output>, String> {
    let mut b = udev.borrow_mut();
    if b.devices.contains_key(&node) {
        return Ok(vec![]);
    }
    let fd = b
        .session
        .open(path, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK)
        .map_err(|e| format!("the seat would not open it: {e}"))?;
    let fd = DrmDeviceFd::new(DeviceFd::from(fd));
    let (drm, notifier) = DrmDevice::new(fd.clone(), true).map_err(|e| format!("not a display device: {e}"))?;
    let gpu = make_gpu(node, drm, fd)?;
    let u = udev.clone();
    let token = handle
        .insert_source(notifier, move |event, _, app| match event {
            DrmEvent::VBlank(crtc) => frame_shown(&u, app, node, crtc),
            DrmEvent::Error(e) => eprintln!("yggdrasil-compositor: {e}"),
        })
        .map_err(|e| e.to_string())?;
    let kind = match &gpu {
        Gpu::Gles { .. } => "OpenGL ES",
        Gpu::Pixman { .. } => "software (pixman)",
    };
    eprintln!("yggdrasil-compositor: {} draws with {kind}", path.display());
    b.devices.insert(node, Device { node, gpu, screens: HashMap::new(), token });
    drop(b);
    Ok(scan(udev, node).0)
}

/// How a GPU draws: hardware EGL on its GBM device (GLES), else dumb buffers drawn with pixman.
fn make_gpu(node: DrmNode, drm: DrmDevice, fd: DrmDeviceFd) -> Result<Gpu, String> {
    let want = renderer_wanted();
    if want != Some(Renderer::Pixman) {
        match gles(&fd) {
            Ok((renderer, gbm)) => {
                let render_node = EGLDevice::device_for_display(renderer.egl_context().display())
                    .ok()
                    .and_then(|d| d.try_get_render_node().ok().flatten())
                    .unwrap_or(node);
                let allocator = GbmAllocator::new(gbm.clone(), GbmBufferFlags::RENDERING | GbmBufferFlags::SCANOUT);
                let exporter = GbmFramebufferExporter::new(gbm.clone(), Some(render_node));
                let formats: FormatSet = renderer.egl_context().dmabuf_render_formats().iter().copied().collect();
                let manager = DrmOutputManager::new(drm, allocator, exporter, Some(gbm), COLOR_FORMATS.iter().copied(), formats);
                return Ok(Gpu::Gles { renderer: Box::new(renderer), manager });
            }
            Err(e) if want == Some(Renderer::Gles) => return Err(format!("OpenGL ES was asked for and did not start: {e}")),
            Err(e) => eprintln!("yggdrasil-compositor: no hardware OpenGL ES ({e}): drawing in software"),
        }
    }
    let renderer = PixmanRenderer::new().map_err(|e| format!("the software renderer did not start: {e}"))?;
    let allocator = Rc::new(RefCell::new(DumbAllocator::new(fd.clone())));
    let formats = [Fourcc::Xrgb8888, Fourcc::Argb8888].map(|code| Format { code, modifier: Modifier::Linear });
    let manager = DrmOutputManager::new(drm, allocator, fd, None, [Fourcc::Xrgb8888, Fourcc::Argb8888], formats);
    Ok(Gpu::Pixman { renderer: Box::new(renderer), manager })
}

/// 8 bits a colour (10-bit screens later, with colour management).
const COLOR_FORMATS: [Fourcc; 2] = [Fourcc::Xrgb8888, Fourcc::Argb8888];

/// OpenGL ES on the GPU's GBM device, if its EGL is hardware (llvmpipe is left out: pixman is lighter for a picture in software).
fn gles(fd: &DrmDeviceFd) -> Result<(GlesRenderer, GbmDevice<DrmDeviceFd>), String> {
    let gbm = GbmDevice::new(fd.clone()).map_err(|e| format!("GBM: {e}"))?;
    // SAFETY: the GBM device lives as long as the display made from it (both are kept with the GPU)
    let display = unsafe { EGLDisplay::new(gbm.clone()) }.map_err(|e| format!("EGL: {e}"))?;
    let device = EGLDevice::device_for_display(&display).map_err(|e| format!("EGL device: {e}"))?;
    if device.is_software() {
        return Err("EGL is software only".into());
    }
    let context = EGLContext::new(&display).map_err(|e| format!("EGL context: {e}"))?;
    // SAFETY: the context is current only on this thread
    let renderer = unsafe { GlesRenderer::new(context) }.map_err(|e| format!("OpenGL ES: {e}"))?;
    Ok((renderer, gbm))
}

/// Programs' GPU buffers (linux-dmabuf) through the boot GPU's renderer, its render node named in the default feedback; the
/// shared-memory formats it reads; wl_drm for older Mesa programs. Software drawing takes shared memory only.
fn gpu_buffers(udev: &Rc<RefCell<Udev>>, app: &mut App, dh: &smithay::reexports::wayland_server::DisplayHandle) {
    let mut b = udev.borrow_mut();
    let primary = b.primary;
    let Some(dev) = b.devices.values_mut().find(|d| same_gpu(d.node, primary)) else { return };
    let node = dev.node;
    match &mut dev.gpu {
        Gpu::Gles { renderer, .. } => {
            app.shm_state.update_formats(renderer.shm_formats());
            let formats: Vec<_> = renderer.dmabuf_formats().into_iter().collect();
            let render = node.node_with_type(NodeType::Render).and_then(|n| n.ok()).unwrap_or(node);
            match DmabufFeedbackBuilder::new(render.dev_id(), formats.iter().copied()).build() {
                Ok(f) => {
                    app.dmabuf_state.create_global_with_default_feedback::<App>(dh, &f);
                }
                Err(_) => {
                    app.dmabuf_state.create_global::<App>(dh, formats.iter().copied());
                }
            }
            let _ = renderer.bind_wl_display(dh);
            let u = udev.clone();
            app.dmabuf_import = Some(Box::new(move |d| {
                let mut b = u.borrow_mut();
                let primary = b.primary;
                b.devices.values_mut().find(|x| same_gpu(x.node, primary)).is_some_and(|x| match &mut x.gpu {
                    Gpu::Gles { renderer, .. } => renderer.import_dmabuf(d, None).is_ok(),
                    Gpu::Pixman { .. } => false,
                })
            }));
        }
        Gpu::Pixman { renderer, .. } => app.shm_state.update_formats(renderer.shm_formats()),
    }
}

/// The connected connectors of a GPU now, each with the CRTC it can use: (connector, its info).
fn connected(dev: &Device) -> Vec<connector::Info> {
    let drm = match &dev.gpu {
        Gpu::Gles { manager, .. } => manager.device(),
        Gpu::Pixman { manager, .. } => manager.device(),
    };
    let Ok(res) = drm.resource_handles() else { return vec![] };
    res.connectors()
        .iter()
        .filter_map(|c| drm.get_connector(*c, true).ok())
        .filter(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
        .filter(|c| !non_desktop(drm, c.handle()))
        .collect()
}

/// A VR headset says it is not a desktop screen (it is leased to the program that drives it; leasing comes later).
fn non_desktop(drm: &DrmDevice, conn: connector::Handle) -> bool {
    let Ok(props) = drm.get_properties(conn) else { return false };
    let (ids, values) = props.as_props_and_values();
    ids.iter().zip(values).any(|(id, v)| drm.get_property(*id).is_ok_and(|p| p.name().to_str() == Ok("non-desktop") && *v != 0))
}

/// A free CRTC for a connector (one its encoders can drive, not used by another screen of this GPU).
fn free_crtc(drm: &DrmDevice, conn: &connector::Info, used: &[crtc::Handle]) -> Option<crtc::Handle> {
    let res = drm.resource_handles().ok()?;
    conn.encoders()
        .iter()
        .filter_map(|e| drm.get_encoder(*e).ok())
        .find_map(|enc| res.filter_crtcs(enc.possible_crtcs()).into_iter().find(|c| !used.contains(c)))
}

/// The kernel's name for a connector: DP-1, HDMI-A-1, eDP-1, Virtual-1.
fn connector_name(c: &connector::Info) -> String {
    format!("{}-{}", c.interface().as_str(), c.interface_id())
}

/// Look at a GPU's connectors: new connected ones become screens (their outputs are returned), screens whose connector went
/// are given up (their names are returned).
fn scan(udev: &Rc<RefCell<Udev>>, node: DrmNode) -> (Vec<Output>, Vec<String>) {
    let mut b = udev.borrow_mut();
    let Some(dev) = b.devices.get_mut(&node) else { return (vec![], vec![]) };
    let now = connected(dev);
    let mut gone = vec![];
    let lost: Vec<crtc::Handle> =
        dev.screens.iter().filter(|(_, s)| !now.iter().any(|c| c.handle() == s.connector)).map(|(c, _)| *c).collect();
    for c in lost {
        if let Some(s) = dev.screens.remove(&c) {
            eprintln!("yggdrasil-compositor: {} unplugged", s.output.name());
            gone.push(s.output.name());
        }
    }
    let mut added = vec![];
    for conn in now {
        if dev.screens.values().any(|s| s.connector == conn.handle()) {
            continue;
        }
        let name = connector_name(&conn);
        let used: Vec<crtc::Handle> = dev.screens.keys().copied().collect();
        let drm = match &dev.gpu {
            Gpu::Gles { manager, .. } => manager.device(),
            Gpu::Pixman { manager, .. } => manager.device(),
        };
        let Some(crtc) = free_crtc(drm, &conn, &used) else {
            eprintln!("yggdrasil-compositor: {name}: no free CRTC for it");
            continue;
        };
        let (make, model) = edid_names(drm, conn.handle());
        let modes = conn.modes().to_vec();
        let preferred = modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)).copied().unwrap_or(modes[0]);
        let (w, h) = conn.size().unwrap_or((0, 0));
        let output =
            Output::new(name.clone(), PhysicalProperties { size: (w as i32, h as i32).into(), subpixel: Subpixel::Unknown, make, model });
        for m in &modes {
            output.add_mode(Mode::from(*m));
        }
        let mode = Mode::from(preferred);
        output.set_preferred(mode);
        output.change_current_state(Some(mode), Some(Transform::Normal), None, None);
        let planes = drm.planes(&crtc).ok();
        let made = match &mut dev.gpu {
            Gpu::Gles { renderer, manager } => manager
                .initialize_output::<_, Elem>(
                    crtc,
                    preferred,
                    &[conn.handle()],
                    &output,
                    planes,
                    &mut **renderer,
                    &DrmOutputRenderElements::default(),
                )
                .map(Out::Gles)
                .map_err(|e| e.to_string()),
            Gpu::Pixman { renderer, manager } => manager
                .initialize_output::<_, PixElem>(
                    crtc,
                    preferred,
                    &[conn.handle()],
                    &output,
                    planes,
                    &mut **renderer,
                    &DrmOutputRenderElements::default(),
                )
                .map(Out::Pixman)
                .map_err(|e| e.to_string()),
        };
        match made {
            Ok(drm) => {
                eprintln!(
                    "yggdrasil-compositor: {name} ({}x{} at {} Hz) on CRTC {crtc:?}",
                    mode.size.w,
                    mode.size.h,
                    mode.refresh as f64 / 1000.0
                );
                dev.screens.insert(
                    crtc,
                    Screen {
                        output: output.clone(),
                        connector: conn.handle(),
                        drm,
                        modes,
                        mode,
                        queued: false,
                        dirty: true,
                        not_before: Instant::now(),
                    },
                );
                added.push(output);
            }
            Err(e) => eprintln!("yggdrasil-compositor: {name}: {e}"),
        }
    }
    (added, gone)
}

/// A GPU's connectors changed (a screen plugged in or out): screens are added and removed to match.
fn device_changed(udev: &Rc<RefCell<Udev>>, app: &mut App, node: DrmNode) {
    let (added, gone) = scan(udev, node);
    for name in gone {
        if let Err(e) = app.remove_output(&name) {
            eprintln!("yggdrasil-compositor: {name}: {e}");
        }
    }
    for o in added {
        app.add_output(o, 1.0);
    }
    app.wake();
}

/// A GPU went: its screens go with it.
fn device_removed(udev: &Rc<RefCell<Udev>>, app: &mut App, node: DrmNode) {
    let Some(dev) = udev.borrow_mut().devices.remove(&node) else { return };
    app.loop_handle.remove(dev.token);
    for s in dev.screens.values() {
        let _ = app.remove_output(&s.output.name());
    }
    app.wake();
}

/// The screen's make and model from its EDID (the manufacturer's three letters and the monitor name, else the product code).
fn edid_names(drm: &DrmDevice, conn: connector::Handle) -> (String, String) {
    let blob = drm.get_properties(conn).ok().and_then(|props| {
        let (ids, values) = props.as_props_and_values();
        ids.iter().zip(values).find_map(|(id, v)| {
            let p = drm.get_property(*id).ok()?;
            (p.name().to_str() == Ok("EDID") && *v != 0).then_some(*v)
        })
    });
    let data = blob.and_then(|b| drm.get_property_blob(b).ok());
    data.as_deref().and_then(parse_edid).unwrap_or_else(|| ("Unknown".into(), "Unknown".into()))
}

/// Make and model from an EDID block: bytes 8-9 are the manufacturer (three letters of 5 bits), 10-11 the product code; one of
/// the four 18-byte descriptors from byte 54 may be the monitor's name (tag 0xFC).
pub fn parse_edid(e: &[u8]) -> Option<(String, String)> {
    if e.len() < 128 || e[..8] != [0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0] {
        return None;
    }
    let m = u16::from_be_bytes([e[8], e[9]]);
    let letter = |v: u16| char::from(b'A' + ((v & 0x1f) as u8).saturating_sub(1));
    let make: String = [letter(m >> 10), letter(m >> 5), letter(m)].iter().collect();
    let name = (0..4).find_map(|i| {
        let d = &e[54 + i * 18..54 + (i + 1) * 18];
        (d[0] == 0 && d[1] == 0 && d[3] == 0xfc)
            .then(|| String::from_utf8_lossy(&d[5..]).split('\n').next().unwrap_or("").trim().to_string())
    });
    let model = name.filter(|n| !n.is_empty()).unwrap_or_else(|| format!("0x{:04X}", u16::from_le_bytes([e[10], e[11]])));
    Some((make, model))
}

/// keyboard, mouse or touchpad: input.conf's speed, profile, natural scrolling and left-handed buttons, as libinput's own settings.
fn set_up_input(device: &mut smithay::reexports::input::Device, conf: &yggdrasil_wm::input::InputConf) {
    if !device.has_capability(DeviceCapability::Pointer) {
        return;
    }
    let m = crate::input::mouse(conf, device.name());
    let _ = device.config_accel_set_speed(m.speed.clamp(-1.0, 1.0));
    if let Some(a) = m.accel {
        let _ = device.config_accel_set_profile(match a {
            yggdrasil_wm::input::Accel::Flat => AccelProfile::Flat,
            yggdrasil_wm::input::Accel::Adaptive => AccelProfile::Adaptive,
        });
    }
    // (a device without the setting refuses it)
    let _ = device.config_scroll_set_natural_scroll_enabled(m.natural);
    let _ = device.config_left_handed_set(m.left_handed);
}

/// Draw the screens that changed, once this pass of the loop is done (the wake-ups of one pass are one draw).
fn schedule(udev: &Rc<RefCell<Udev>>, handle: &LoopHandle<'static, App>) {
    let mut b = udev.borrow_mut();
    if b.idle_pending {
        return;
    }
    b.idle_pending = true;
    let u = udev.clone();
    handle.insert_idle(move |app| {
        u.borrow_mut().idle_pending = false;
        draw_all(&u, app);
    });
}

/// Draw every screen that changed and is not waiting for its page flip (or for its pause after a frame with no change).
fn draw_all(udev: &Rc<RefCell<Udev>>, app: &mut App) {
    if !udev.borrow().session.is_active() {
        return;
    }
    let now = Instant::now();
    let mut later: Option<Instant> = None;
    let todo: Vec<(DrmNode, crtc::Handle)> = {
        let b = udev.borrow();
        b.devices
            .iter()
            .flat_map(|(n, d)| d.screens.iter().filter(|(_, s)| s.dirty && !s.queued).map(move |(c, s)| (*n, *c, s.not_before)))
            .filter_map(|(n, c, nb)| {
                if nb > now {
                    later = Some(later.map_or(nb, |l: Instant| l.min(nb)));
                    None
                } else {
                    Some((n, c))
                }
            })
            .collect()
    };
    let drew = !todo.is_empty();
    for (n, c) in todo {
        draw(udev, app, n, c);
    }
    if drew {
        after_frame(udev, app);
    }
    if let Some(at) = later {
        let mut b = udev.borrow_mut();
        if !b.timer_pending {
            b.timer_pending = true;
            let u = udev.clone();
            let _ = app.loop_handle.insert_source(Timer::from_deadline(at), move |_, _, app| {
                u.borrow_mut().timer_pending = false;
                draw_all(&u, app);
                TimeoutAction::Drop
            });
        }
    }
}

/// What follows a frame: the programs are told their frame was shown (they draw the next), the window list and the menus are
/// brought up to date, an animation asks for its next frame.
fn after_frame(udev: &Rc<RefCell<Udev>>, app: &mut App) {
    let t = udev.borrow().start.elapsed();
    if app.animating() {
        app.wake();
    }
    app.send_frames(t);
    app.space.refresh();
    app.foreign_refresh();
    app.popups.cleanup();
    let _ = app.display_handle.flush_clients();
}

/// The pointer for this screen, as an element on top (the DrmCompositor puts it on the cursor plane when it can).
macro_rules! pointer_element {
    ($cache:expr, $app:expr, $renderer:expr, $output:expr) => {{
        match $app.pointer_picture($output) {
            Some((pic, (x, y))) => {
                // the same picture keeps its buffer (a program's own pointer is read anew each frame: compared by its pixels),
                // so a pointer that did not change is no damage
                let fresh = $cache.as_ref().is_none_or(|(p, _)| !Arc::ptr_eq(p, &pic) && **p != *pic);
                if fresh {
                    let buf = MemoryRenderBuffer::from_slice(
                        &pic.data,
                        Fourcc::Argb8888,
                        (pic.w as i32, pic.h as i32),
                        1,
                        Transform::Normal,
                        None,
                    );
                    $cache = Some((pic.clone(), buf));
                }
                let buf = &$cache.as_ref().expect("just set").1;
                MemoryRenderBufferRenderElement::from_buffer(&mut **$renderer, (x as f64, y as f64), buf, None, None, None, Kind::Cursor)
                    .ok()
            }
            None => None,
        }
    }};
}

/// One screen's frame: its mode as Settings set it, its elements, drawn and queued for the page flip.
macro_rules! frame {
    ($cache:expr, $app:expr, $renderer:expr, $out:expr, $screen:expr, $elem:ty) => {{
        let output = $screen.output.clone();
        let mut elements: Vec<$elem> = vec![];
        if let Some(p) = pointer_element!($cache, $app, $renderer, &output) {
            elements.push(p.into());
        }
        let clear = if !$app.power.is_on(&output) {
            BLACK
        } else if $app.locked {
            elements.extend($app.lock_elements(&mut **$renderer, &output).into_iter().map(Into::into));
            BLACK
        } else {
            elements.extend($app.scene(&mut **$renderer, &output));
            BACKGROUND
        };
        match $out.render_frame(&mut **$renderer, &elements, clear, FrameFlags::DEFAULT) {
            Ok(r) if r.is_empty => Ok(false),
            Ok(_) => $out.queue_frame(()).map(|_| true).map_err(|e| e.to_string()),
            Err(e) => Err(e.to_string()),
        }
    }};
}

/// Set the mode Settings chose (when it changed) on the CRTC.
macro_rules! follow_mode {
    ($renderer:expr, $out:expr, $screen:expr, $elem:ty) => {{
        let want = $screen.output.current_mode();
        if let Some(want) = want.filter(|w| *w != $screen.mode) {
            match $screen.modes.iter().find(|m| Mode::from(**m) == want) {
                Some(m) => match $out.use_mode::<_, $elem>(*m, &mut **$renderer, &DrmOutputRenderElements::default()) {
                    Ok(()) => $screen.mode = want,
                    Err(e) => eprintln!("yggdrasil-compositor: {}: mode not set: {e}", $screen.output.name()),
                },
                None => eprintln!("yggdrasil-compositor: {}: the screen has no mode {:?}", $screen.output.name(), want),
            }
        }
    }};
}

/// Draw one screen.
fn draw(udev: &Rc<RefCell<Udev>>, app: &mut App, node: DrmNode, crtc: crtc::Handle) {
    let mut guard = udev.borrow_mut();
    let b = &mut *guard;
    let Some(dev) = b.devices.get_mut(&node) else { return };
    let Some(screen) = dev.screens.get_mut(&crtc) else { return };
    // a screen switched off in Settings is out of the space: nothing is drawn on it
    if !app.space.outputs().any(|o| *o == screen.output) {
        screen.dirty = false;
        return;
    }
    screen.dirty = false;
    let result = match (&mut dev.gpu, &mut screen.drm) {
        (Gpu::Gles { renderer, .. }, Out::Gles(out)) => {
            follow_mode!(renderer, out, screen, Elem);
            frame!(b.pointer, app, renderer, out, screen, Elem)
        }
        (Gpu::Pixman { renderer, .. }, Out::Pixman(out)) => {
            follow_mode!(renderer, out, screen, PixElem);
            frame!(b.pointer, app, renderer, out, screen, PixElem)
        }
        _ => Err("the screen and its GPU do not match".into()),
    };
    let Some(dev) = b.devices.get_mut(&node) else { return };
    let Some(screen) = dev.screens.get_mut(&crtc) else { return };
    if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
        eprintln!("frame {} at {:?}: {result:?}", screen.output.name(), b.start.elapsed());
    }
    match result {
        Ok(true) => screen.queued = true,
        Ok(false) => {
            // nothing changed on it: no frame is sent, and the next waits a refresh
            screen.not_before = Instant::now() + refresh_of(&screen.mode);
        }
        // the seat was taken away (a switch to another terminal) before its pause reached us: drawn whole when it comes back
        Err(e) if e.contains("Permission denied") => screen.dirty = true,
        Err(e) => {
            if b.session.is_active() {
                eprintln!("yggdrasil-compositor: {}: {e}", screen.output.name());
            }
            screen.not_before = Instant::now() + refresh_of(&screen.mode);
        }
    }
}

/// One refresh of a mode (16.7 ms at 60 Hz).
fn refresh_of(m: &Mode) -> Duration {
    Duration::from_micros(1_000_000_000 / m.refresh.max(1000) as u64)
}

/// A page flip happened on a CRTC: its frame is on screen, the next may be drawn (at once if something changed meanwhile).
fn frame_shown(udev: &Rc<RefCell<Udev>>, app: &mut App, node: DrmNode, crtc: crtc::Handle) {
    let again = {
        let mut b = udev.borrow_mut();
        let Some(screen) = b.devices.get_mut(&node).and_then(|d| d.screens.get_mut(&crtc)) else { return };
        let done = match &mut screen.drm {
            Out::Gles(o) => o.frame_submitted().map(|_| ()).map_err(|e| e.to_string()),
            Out::Pixman(o) => o.frame_submitted().map(|_| ()).map_err(|e| e.to_string()),
        };
        if let Err(e) = done {
            if !e.contains(&DrmError::DeviceInactive.to_string()) {
                eprintln!("yggdrasil-compositor: {}: {e}", screen.output.name());
            }
        }
        screen.queued = false;
        screen.dirty
    };
    if again {
        schedule(udev, &app.loop_handle.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_backend_is_chosen() {
        assert!(wanted(&v(&["--drm"])));
        assert!(wanted(&v(&["--backend", "drm"])));
        assert!(wanted(&v(&["--backend=udev"])));
        assert!(!wanted(&v(&["--backend", "headless"])));
        assert!(!wanted(&v(&[])));
    }

    /// An EDID made up for the test: manufacturer "YGG", product 0x1234, a monitor name descriptor.
    fn edid(name: Option<&str>) -> Vec<u8> {
        let mut e = vec![0u8; 128];
        e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
        let code = |c: u8| (c - b'A' + 1) as u16;
        let m = (code(b'Y') << 10) | (code(b'G') << 5) | code(b'G');
        e[8..10].copy_from_slice(&m.to_be_bytes());
        e[10..12].copy_from_slice(&0x1234u16.to_le_bytes());
        if let Some(n) = name {
            let d = &mut e[72..90];
            d[3] = 0xfc;
            let mut text = n.as_bytes().to_vec();
            text.push(b'\n');
            text.resize(13, b' ');
            d[5..18].copy_from_slice(&text);
        }
        e
    }

    #[test]
    fn make_and_model_come_from_the_edid() {
        assert_eq!(parse_edid(&edid(Some("Test Screen"))), Some(("YGG".into(), "Test Screen".into())));
        assert_eq!(parse_edid(&edid(None)), Some(("YGG".into(), "0x1234".into())), "no name: the product code");
        assert_eq!(parse_edid(&[0u8; 128]), None, "not an EDID");
        assert_eq!(parse_edid(&edid(None)[..100]), None, "too short");
    }

    #[test]
    fn a_refresh_is_one_frame_long() {
        let m = Mode { size: (1920, 1080).into(), refresh: 60_000 };
        assert_eq!(refresh_of(&m), Duration::from_micros(16_666));
    }
}
