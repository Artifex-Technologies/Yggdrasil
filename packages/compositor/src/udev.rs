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
//!   "non-desktop") gets a CRTC, the laptop's own (eDP, LVDS, DSI) first, and becomes an `Output` named as the kernel names it
//!   (DP-1, HDMI-A-1, eDP-1, Virtual-1) with all its modes, its size in millimetres and the make and model from its EDID;
//!   Settings > Displays' monitors.lua then sets its mode, scale and place (outputs.rs, the same code as headless; a screen it
//!   does not name runs at its own size at the fastest rate it has for it, and one it gives no scale gets one from its size,
//!   laptop.rs). A laptop's panel mounted turned in its case (a handheld's, a tablet's: the connector's "panel orientation")
//!   starts turned upright unless monitors.lua turns it. A mode changed later (wlr-output-management) is set on the CRTC at
//!   the next frame (a mode within half a hertz of the one asked: 60 for a panel's 60.003 Hz); one the screen cannot have (its
//!   GPU refuses it, at the first modeset or later, or it is not one of its modes) leaves the CRTC as it is, and the screen is
//!   put back at that mode, so programs and Settings never see a mode it does not run. A screen unplugged gives its CRTC
//!   back (its DrmCompositor goes, which switches the CRTC off) but stays a screen for 2 s (outputs.rs, UNPLUG_GRACE): a monitor
//!   whose link bounces comes back with its windows and panels, only its CRTC set up again; one plugged in is set up once its
//!   link has stayed for 0.7 s (SETTLE). A connector
//!   that appears later (a DisplayPort hub or dock, MST, makes new connectors) has the device reset first: Smithay learns a
//!   connector's properties only then (every screen is drawn whole again).
//! - **The lid** (laptop.rs): closed while another screen is on, the laptop's screen leaves its CRTC as if unplugged; opened, it
//!   comes back as monitors.lua says. Closed with no other screen: it stays, dark and switched off, for logind to suspend.
//!   The tablet-mode switch is recorded (`switches`); both are known from the start (open and off until libinput says otherwise).
//! - **Screens off**: one switched off by wlr-output-power (yggdrasil-idle) or in Settings has its CRTC switched off (the
//!   DrmCompositor's `clear`: no picture, the panel's backlight off); on again, the mode is set again with a whole frame.
//! - **Suspend and resume** (laptop.rs' SleepWatch: the kernel's word that the system woke): every DrmCompositor's state is read
//!   again, every screen drawn whole, the input devices set up again and the connectors looked at again (a screen unplugged
//!   while it slept). A switch to another terminal and back is the seat's pause and activate, as before.
//! - **Screen copies** (readback.rs): wlr-screencopy and ext-image-copy-capture, drawn again on the GPU into shared memory or
//!   the program's GPU buffer: grim, yggdrasil-shot and the panel's previews.
//! - **Drawing**: on a GPU with hardware EGL, GBM buffers drawn with OpenGL ES (GlesRenderer); otherwise (no 3D: a plain
//!   virtual GPU, a simple framebuffer, YGGDRASIL_COMPOSITOR_RENDERER=pixman) dumb buffers drawn in software (pixman), so a
//!   machine with no working GPU driver still gets a picture. The GL context is asked for at high priority (the default one
//!   when the driver refuses it); each GPU's line at the start says what EGL granted. Each screen has its own Smithay
//!   `DrmCompositor`, which puts the pointer on the cursor plane when it can (YGGDRASIL_COMPOSITOR_CURSOR_PLANE=0: never, the
//!   pointer drawn into each frame).
//! - **Frames**: drawn only when something changed (`App::wake`), then not again before that frame was shown (the page flip's
//!   vblank event); a frame with no change is not even sent, and a run of frames with no change waits a refresh between them.
//!   Under variable refresh with a full screen window (a game) only its frames start a refresh: the pointer, the panels and
//!   the rest go out with its next frame, at most 1/24 s after its last one (`vrr_flip`).
//!   Nothing is drawn and no timer runs while nothing changes.
//! - **Input**: libinput (keyboards, mice, touchpads, TrackPoints, the touchscreen and pen, a virtual machine's tablet), named
//!   as Hyprland names them; Settings > Peripherals' input.conf sets libinput's speed, profile, natural scrolling, left-handed
//!   buttons and a touchpad's taps, click method, scrolling and the rest per device and per kind (input.rs), and its scroll
//!   factor is applied here. The touchscreen and the pen are the laptop screen's (touch.rs).
//! - **The control socket** (control.rs) is there too, for Yggdrasil's own tools (yggdrasil-wm); it makes no input.
//! - **Ending**: `ctl quit`, SIGTERM, SIGINT or SIGHUP end it cleanly: the CRTCs it tinted (Night Light) get a straight gamma
//!   table back so the next session is not tinted, the session is closed and the console comes back.
//!
//! - **Waiting for programs' GPUs** (sync.rs): a commit with a GPU buffer is applied once the buffer is drawn, through
//!   explicit sync (linux-drm-syncobj-v1, which NVIDIA needs) where the GPU can, the buffer's own fences otherwise.
//!
//! - **Direct scanout**: a full screen window drawn exactly as the program made it offers its buffer (look::Clipped,
//!   xwayland::OpaqueElement), and Smithay's DrmCompositor puts it on the primary plane when it covers the screen, nothing is
//!   drawn over it and the GPU takes it; while a full screen window covers a screen, a buffer of any format is tried there
//!   (`frame_flags`; `YGGDRASIL_COMPOSITOR_SCANOUT=0` keeps every picture composited). The log says when a screen's picture
//!   starts going out directly and why it is composited again (`why_composited`).
//!
//! Known limitations (docs/COMPOSITOR.md): each GPU draws only its own screens (none draws for another, as a laptop's NVIDIA
//! GPU behind the Intel one would need); only the primary plane takes a window, the overlay planes only the pointer; no
//! tearing control (Smithay's DRM compositor has no asynchronous page flips).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use smithay::backend::allocator::dumb::DumbAllocator;
use smithay::backend::allocator::format::FormatSet;
use smithay::backend::allocator::gbm::{GbmAllocator, GbmBufferFlags, GbmDevice};
use smithay::backend::allocator::{Format, Fourcc, Modifier};
use smithay::backend::drm::compositor::FrameFlags;
use smithay::backend::drm::compositor::PrimaryPlaneElement;
use smithay::backend::drm::exporter::gbm::GbmFramebufferExporter;
use smithay::backend::drm::output::{DrmOutput, DrmOutputManager, DrmOutputRenderElements};
use smithay::backend::drm::VrrSupport;
use smithay::backend::drm::{DrmDevice, DrmDeviceFd, DrmError, DrmEvent, DrmNode, NodeType};
use smithay::backend::egl::context::ContextPriority;
use smithay::backend::egl::{EGLContext, EGLDevice, EGLDisplay};
use smithay::backend::input::InputEvent;
use smithay::backend::libinput::{LibinputInputBackend, LibinputSessionInterface};
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::{Element, Id, Kind, RenderElementPresentationState, RenderElementState, RenderingReason};
use smithay::backend::renderer::gles::{GlesRenderbuffer, GlesRenderer};
use smithay::backend::renderer::pixman::PixmanRenderer;
use smithay::backend::renderer::{ImportDma, ImportEgl, ImportMemWl};
use smithay::backend::session::libseat::LibSeatSession;
use smithay::backend::session::{Event as SessionEvent, Session};
use smithay::backend::udev::{self, UdevBackend, UdevEvent};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{EventLoop, Interest, LoopHandle, Mode as LoopMode, PostAction, RegistrationToken};
use smithay::reexports::drm::control::{connector, crtc, property, Device as _, ModeTypeFlags, ResourceHandle};
use smithay::reexports::input::{AccelProfile, DeviceCapability, Libinput};
use smithay::wayland::dmabuf::DmabufFeedback;

use crate::input::{DeviceInfo, Kind as InputKind};
use crate::laptop::Switches;
use smithay::reexports::rustix::fs::OFlags;
use smithay::reexports::wayland_server::Display;
use smithay::utils::{DeviceFd, Logical, Physical, Rectangle, Size, Transform};
use smithay::wayland::dmabuf::DmabufFeedbackBuilder;

use crate::state::{App, Elem, GameCommits, PixElem};

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
    /// the laptop's own (eDP, LVDS, DSI)
    internal: bool,
    /// its CRTC is switched off (the screen is off, switched off in Settings, or dark behind the closed lid)
    off: bool,
    drm: Out,
    /// the connector's modes (a mode chosen in Settings is looked up here)
    modes: Vec<smithay::reexports::drm::control::Mode>,
    /// the mode the CRTC is set to
    mode: Mode,
    /// the mode its GPU refused when it was set up (`scan`), which its first frame does not ask again (`follow_mode`)
    refused: Option<Mode>,
    /// a frame was queued and its page flip has not happened yet
    queued: bool,
    /// something changed since it was last drawn
    dirty: bool,
    /// a frame with no change was drawn: the next one waits until then (a program that commits without changing anything
    /// does not spin the compositor)
    not_before: Instant,
    /// the tint its CRTC's gamma table has (red, green, blue scale; ctm.rs); None: not set yet, or to be set again (after a
    /// terminal had the screen, which may have reset it)
    gamma: Option<[f64; 3]>,
    /// variable refresh can be switched on this screen without a modeset (the monitor and the GPU say so)
    vrr_capable: bool,
    /// what programs on this screen are told to allocate their GPU buffers as (linux-dmabuf feedback): the renderer's
    /// formats, and for a window the screen tried to show directly the formats its planes scan out first
    /// (`App::send_dmabuf_feedback`)
    feedback: Option<Feedback>,
    /// the element whose buffer went to the screen as it is in the last frame, on the primary plane (direct scanout)
    direct: Option<Id>,
    /// its frames paced by its full screen window's under variable refresh (`vrr_flip`)
    pacer: Pacer,
}

/// A screen's two dmabuf feedbacks: the renderer's (the default), and the one with the planes' scanout formats first.
struct Feedback {
    render: DmabufFeedback,
    scanout: DmabufFeedback,
}

/// The feedbacks for a screen drawn by a GBM compositor: its primary and overlay planes' formats that the renderer can also
/// read (so a buffer that cannot be scanned out can still be drawn), as a scanout tranche on the screen's device, then the
/// renderer's formats on the render node.
fn screen_feedback(out: &GbmOut, render_node: DrmNode, render_formats: &FormatSet) -> Option<Feedback> {
    use smithay::reexports::wayland_protocols::wp::linux_dmabuf::zv1::server::zwp_linux_dmabuf_feedback_v1::TrancheFlags;
    out.with_compositor(|c| {
        let surface = c.surface();
        let planes = surface.planes();
        let scanout: FormatSet = std::iter::once(surface.plane_info())
            .chain(planes.overlay.iter())
            .flat_map(|p| p.formats.iter().copied())
            .filter(|f| render_formats.contains(f))
            .collect();
        let device = surface.device_fd().dev_id().ok()?;
        let builder = DmabufFeedbackBuilder::new(render_node.dev_id(), render_formats.iter().copied());
        let render = builder.clone().build().ok()?;
        let scanout = builder.add_preference_tranche(device, TrancheFlags::Scanout, scanout, 4u32..=6).build().ok()?;
        Some(Feedback { render, scanout })
    })
}

struct Device {
    node: DrmNode,
    gpu: Gpu,
    screens: HashMap<crtc::Handle, Screen>,
    token: RegistrationToken,
    /// the connectors the device had when it was opened or last reset (Smithay knows the properties of these only)
    known: Vec<connector::Handle>,
    /// the connectors connected at the last look (a screen kept off by the lid too)
    was_connected: Vec<connector::Handle>,
    /// connectors plugged in a moment ago, set up once their link has stayed for SETTLE
    settling: Vec<(connector::Handle, Instant)>,
    /// a look at the connectors is due (a timer runs for the settling ones)
    rescan_pending: bool,
    /// the CRTCs whose gamma table this session tinted (Night Light) and has not set straight since: straightened before the
    /// session lets the GPU go (`untint`), a CRTC whose screen was unplugged too
    tinted: HashSet<crtc::Handle>,
}

/// How long a screen just plugged in waits before its first modeset: a monitor that is hunting between its inputs (or a cable
/// pushed in) drops its link again within this, and is then never set up and torn down for nothing.
const SETTLE: Duration = Duration::from_millis(700);

pub struct Udev {
    session: LibSeatSession,
    /// the GPU that renders for programs (dmabuf): the boot one
    primary: DrmNode,
    devices: HashMap<DrmNode, Device>,
    libinput: Libinput,
    /// keyboards, mice, touchpads and the rest, set up as input.conf says (again when it changes or a device comes)
    inputs: Vec<(smithay::reexports::input::Device, DeviceInfo)>,
    /// the lid and the tablet-mode switch
    switches: Switches,
    /// the screens whose frame had damage since their last copy that waited for one (readback.rs)
    copy_changed: HashSet<String>,
    input_seen: Option<yggdrasil_wm::input::InputConf>,
    /// a draw is already asked for (the wake-ups of one pass are folded into it), and the timer for screens that wait
    idle_pending: bool,
    timer_pending: bool,
    /// the timer for screens whose frame waits for their full screen game's next one (`vrr_flip`): when it fires, and its source
    vrr_timer: Option<(Instant, RegistrationToken)>,
    /// the pointer's last picture and its buffer (uploaded again only when the picture changes)
    pointer: Option<(Arc<crate::pointer::Picture>, MemoryRenderBuffer)>,
    start: Instant,
}

pub const BACKGROUND: [f32; 4] = [0.05, 0.05, 0.08, 1.0];
pub const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// Run the hardware backend until `quit`, a signal, or the loop is stopped.
pub fn run(run: Vec<Vec<String>>) -> Result<(), Box<dyn std::error::Error>> {
    check_env()?;
    // the signals that end it are taken through a signalfd (blocked here, before any thread or child exists; the programs it
    // starts get them unblocked again: main.rs plain_signals. XWayland, which Smithay starts, keeps them blocked; it ends with
    // the compositor)
    let signals = crate::end_signals()?;
    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let display: Display<App> = Display::new()?;
    let dh = display.handle();
    let handle = event_loop.handle();
    let (ping, ping_source) = smithay::reexports::calloop::ping::make_ping()?;

    let (session, notifier) = LibSeatSession::new().map_err(|e| format!("no seat (logind or seatd): {e}"))?;
    let seat = session.seat();
    eprintln!("yggdrasil-compositor: seat {seat}");
    let boot = udev::primary_gpu(&seat).ok().flatten();
    let all = udev::all_gpus(&seat).unwrap_or_default();
    let wanted = std::env::var("YGGDRASIL_COMPOSITOR_GPU").ok();
    let (chosen, why) =
        render_gpu(boot.as_deref(), &all, wanted.as_deref(), |p| has_screen(p, Path::new("/sys/class/drm"))).ok_or("no GPU found")?;
    eprintln!("yggdrasil-compositor: drawing on {} ({why})", chosen.display());
    if !cursor_plane() {
        eprintln!(
            "yggdrasil-compositor: the pointer is drawn into each frame, never on a cursor plane (YGGDRASIL_COMPOSITOR_CURSOR_PLANE)"
        );
    }
    let primary = DrmNode::from_path(&chosen).map_err(|e| format!("{}: {e}", chosen.display()))?;
    let udev_backend = UdevBackend::new(&seat)?;

    let mut libinput = Libinput::new_with_udev::<LibinputSessionInterface<LibSeatSession>>(session.clone().into());
    libinput.udev_assign_seat(&seat).map_err(|_| "libinput could not take the seat's input devices")?;

    let udev = Rc::new(RefCell::new(Udev {
        session: session.clone(),
        primary,
        devices: HashMap::new(),
        libinput: libinput.clone(),
        inputs: vec![],
        switches: Switches::default(),
        copy_changed: HashSet::new(),
        input_seen: None,
        idle_pending: false,
        timer_pending: false,
        vrr_timer: None,
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
    // the workspaces for other programs (ext-workspace: a bar, a pager)
    crate::workspaces::create_global(&dh);
    // a screen's colour matrix (hyprsunset: Settings' Night Light), put into its CRTC's gamma table (`draw`)
    crate::ctm::create_global(&dh);
    // screens off and on (yggdrasil-idle): the CRTC switched off
    crate::outputpower::create_global(&dh);
    // screenshots and the panel's previews, drawn again on the GPU (readback.rs)
    crate::screencopy::create_global(&dh);
    crate::capture::create_globals(&dh);
    let mut app = App::new(&dh, first, handle.clone(), event_loop.get_signal(), ping);
    for o in outputs {
        let scale = default_scale_of(&o);
        app.add_output(o, scale);
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
                        Gpu::Gles { manager, .. } => manager.lock().activate(false),
                        Gpu::Pixman { manager, .. } => manager.lock().activate(false),
                    };
                    if let Err(e) = active {
                        eprintln!("yggdrasil-compositor: the GPU did not come back: {e}");
                    }
                    for s in d.screens.values_mut() {
                        // the frame on its way was lost with the switch; the terminal drew over the screen: all of it again
                        // (and a screen that is off is switched off again: the terminal may have lit it)
                        s.queued = false;
                        s.dirty = true;
                        s.off = false;
                        s.gamma = None;
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
                        let scale = default_scale_of(&o);
                        app.add_output(o, scale);
                    }
                    // the lid may want the laptop's screen off now
                    device_changed(&u, app, node);
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

    // keyboards, mice, touchpads, the touchscreen and pen, the lid and the tablet-mode switch
    let u = udev.clone();
    handle.insert_source(LibinputInputBackend::new(libinput), move |event, _, app| {
        match &event {
            InputEvent::DeviceAdded { device } => input_added(&u, app, device.clone()),
            InputEvent::DeviceRemoved { device } => {
                u.borrow_mut().inputs.retain(|(d, _)| d != device);
                app.input_devices = u.borrow().inputs.iter().map(|(_, i)| i.clone()).collect();
            }
            InputEvent::SwitchToggle { event } => {
                use smithay::backend::input::{Switch, SwitchState, SwitchToggleEvent};
                // (smithay's view of the event: libinput's own has methods of the same names)
                let on = SwitchToggleEvent::<LibinputInputBackend>::state(event) == SwitchState::On;
                match SwitchToggleEvent::<LibinputInputBackend>::switch(event) {
                    Some(Switch::Lid) => lid(&u, app, on),
                    Some(Switch::TabletMode) => {
                        u.borrow_mut().switches.tablet_mode = Some(on);
                        eprintln!("yggdrasil-compositor: tablet mode {}", if on { "on" } else { "off" });
                        app.tablet_mode(on);
                    }
                    _ => {}
                }
                return;
            }
            _ => {}
        }
        app.on_input(event, true);
    })?;

    // waking from a suspend (laptop.rs): the kernel says so through a timer it cancels
    match crate::laptop::SleepWatch::new() {
        Ok(watch) => {
            let u = udev.clone();
            handle.insert_source(Generic::new(watch, Interest::READ, LoopMode::Level), move |_, w, app| {
                // SAFETY: the watch is not dropped while it is registered here
                if let Some(slept) = unsafe { w.get_mut() }.woke() {
                    eprintln!("yggdrasil-compositor: woke after {} s asleep", slept.as_secs());
                    resumed(&u, app);
                }
                Ok(PostAction::Continue)
            })?;
        }
        Err(e) => {
            eprintln!("yggdrasil-compositor: no watch for waking from a suspend ({e}); after one, `ctl resumed` draws the screens again")
        }
    }

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
    let u = udev.clone();
    handle.insert_source(Generic::new(listener, Interest::READ, LoopMode::Level), move |_, l, app| {
        while let Ok((stream, _)) = l.accept() {
            c.borrow_mut().serve(stream, app, &mut |words, app| command(&u, words, app), USAGE);
        }
        Ok(PostAction::Continue)
    })?;

    let u = udev.clone();
    event_loop.run(None, &mut app, |app| {
        let _ = app.display_handle.flush_clients();
        control.borrow_mut().tell_watchers(app);
        // input.conf changed (read again when the config files' watch saw it change, or on Settings' `reload`): the devices are
        // set up again
        let mut b = u.borrow_mut();
        if b.input_seen.as_ref() != Some(&app.input) {
            b.input_seen = Some(app.input.clone());
            for (d, info) in b.inputs.iter_mut() {
                set_up_input(d, info, &app.input);
            }
        }
    })?;

    // the screens are let go before the session is closed, and the session before the console is given back
    crate::xwayland::remove_display_files(&app);
    let _ = std::fs::remove_file(&ctl);
    let _ = std::fs::remove_file(runtime.join(&app.socket));
    let _ = std::fs::remove_file(runtime.join(format!("{}.lock", app.socket)));
    let mut b = udev.borrow_mut();
    // before the GPUs are let go: no tint of ours stays for the session after this one
    if b.session.is_active() {
        for d in b.devices.values() {
            untint(d);
        }
    } else if b.devices.values().any(|d| !d.tinted.is_empty()) {
        eprintln!("yggdrasil-compositor: another terminal has the screens: their tint stays until the next session sets its own");
    }
    for (_, d) in b.devices.drain() {
        event_loop.handle().remove(d.token);
    }
    eprintln!("yggdrasil-compositor: ended");
    Ok(())
}

/// Before the session lets a GPU go: every CRTC it tinted (Night Light) gets a straight gamma table back, and one of its CRTCs
/// without a gamma table has a colour matrix (CTM) or degamma table someone left on it cleared, so the session after this one
/// (Hyprland, the console) is not tinted: the kernel keeps a CRTC's colour tables for whoever takes the GPU next. Gamescope
/// clears them on exit the same way. (On an atomic driver the kernel's legacy gamma call also clears the CTM and degamma table.)
fn untint(dev: &Device) {
    let drm = gpu_drm(&dev.gpu);
    let crtcs: HashSet<crtc::Handle> = dev.screens.keys().chain(&dev.tinted).copied().collect();
    for crtc in crtcs {
        let what = dev.screens.get(&crtc).map(|s| s.output.name()).unwrap_or_else(|| format!("{crtc:?}"));
        let len = drm.get_crtc(crtc).map(|c| c.gamma_length() as usize).unwrap_or(0);
        if len >= 2 {
            if dev.tinted.contains(&crtc) {
                let straight = crate::ctm::ramp(1.0, len);
                match drm.set_gamma(crtc, &straight, &straight, &straight) {
                    Ok(()) => eprintln!("yggdrasil-compositor: {what}: tint taken off"),
                    Err(e) => eprintln!("yggdrasil-compositor: {what}: the tint was not taken off: {e}"),
                }
            }
            continue;
        }
        for name in ["CTM", "DEGAMMA_LUT"] {
            if let Some((p, _)) = property(drm, crtc, name).filter(|(_, v)| *v != 0) {
                match drm.set_property(crtc, p.handle(), 0) {
                    Ok(()) => eprintln!("yggdrasil-compositor: {what}: {name} cleared"),
                    Err(e) => eprintln!("yggdrasil-compositor: {what}: {name} not cleared: {e}"),
                }
            }
        }
    }
}

/// The commands, for an unknown one's answer.
const USAGE: &str = "outputs, scale NAME S, power NAME on|off, switches, lid open|closed, tablet-mode on|off, resumed, input, pointer-look, windows, focus, free, workspace, lock, watch, quit; for the tools: clients, active, workspaces, workspace N, activate ID, close ID, minimize ID, maximize ID on|off|toggle, send ID N, place ID X Y W H, cursor, screens, pointer-timeout [S], modifiers";

/// Two nodes of the same GPU (its card and its render node).
fn same_gpu(a: DrmNode, b: DrmNode) -> bool {
    a.dev_id() == b.dev_id()
        || a.node_with_type(NodeType::Render).and_then(|n| n.ok()) == b.node_with_type(NodeType::Render).and_then(|n| n.ok())
}

/// When variable refresh (VRR) is on: YGGDRASIL_COMPOSITOR_VRR says 0 (never), 1 (always) or 2 (while a full screen window
/// covers the screen: the default, as the Hyprland session's `vrr = 2`). A desktop at a varying rate makes its animations
/// uneven and some monitors flicker in their brightness; a game in full screen is what it is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VrrPolicy {
    Never,
    Always,
    FullScreen,
}

fn parse_vrr(v: Option<&str>) -> VrrPolicy {
    match v.map(|v| v.trim().to_ascii_lowercase()).as_deref() {
        Some("0" | "off" | "no" | "never") => VrrPolicy::Never,
        Some("1" | "on" | "yes" | "always") => VrrPolicy::Always,
        _ => VrrPolicy::FullScreen,
    }
}

fn vrr_policy() -> VrrPolicy {
    thread_local!(static P: VrrPolicy = parse_vrr(std::env::var("YGGDRASIL_COMPOSITOR_VRR").ok().as_deref()));
    P.with(|p| *p)
}

/// What the pointer's element is to Smithay's DrmCompositor: a cursor, which it puts on the GPU's cursor plane (or an overlay
/// plane) when it can; or, when YGGDRASIL_COMPOSITOR_CURSOR_PLANE says 0 (no, off, false), an element like any other, which no
/// plane takes, so the pointer is drawn into each frame with the rest. One restart with it tells a driver's cursor-plane bug
/// from ours (a pointer that flickers, lags, shows twice or leaves a trail): gamescope keeps the cursor plane off on AMD by
/// default for known driver issues. A screen with a visible pointer then never shows a full screen picture directly (scanout
/// needs the pointer on a plane of its own).
fn pointer_kind(cursor_plane: bool) -> Kind {
    match cursor_plane {
        true => Kind::Cursor,
        false => Kind::Unspecified,
    }
}

fn cursor_plane() -> bool {
    thread_local!(static C: bool = crate::switched_on(std::env::var("YGGDRASIL_COMPOSITOR_CURSOR_PLANE").ok().as_deref()));
    C.with(|c| *c)
}

fn vrr_wanted(policy: VrrPolicy, fullscreen: bool) -> bool {
    match policy {
        VrrPolicy::Never => false,
        VrrPolicy::Always => true,
        VrrPolicy::FullScreen => fullscreen,
    }
}

/// How long a change that is not the game's own frame waits for that frame under variable refresh: 1/24 s, a fixed bound
/// rather than one from the screen's lowest VRR rate. KMS tells only that a screen can vary its rate, not its range (that is
/// in the EDID's range descriptor, which Smithay does not read), and the range is not what this bound is for: with no flip
/// the screen shows its last frame again at its lowest rate by itself (some drivers double frames below it: low framerate
/// compensation). The bound is how long the pointer and the panels may lag behind a game that slows or stops: a game at 24
/// frames a second or more carries them with its own frames, a slower or paused one (a menu that does not redraw) lets them
/// refresh the screen themselves. It is a frame at 30 a second (33.3 ms) and a quarter more, so that a game capped at 30, the
/// most common cap below 60, stays paced: its frames come a millisecond or more late now and then, and with the bound at
/// exactly 1/30 s (Valve's gamescope, its adaptive_sync_cursor_min_fps) such a frame would come just after the pointer's own
/// flip and wait for the refresh that flip started (up to 7 ms at 144 Hz), the judder this is for. Hyprland's
/// cursor:min_refresh_rate is 24 too. A bound from the lowest rate (48 Hz on many monitors) would let the pointer break the
/// pace of every game running between 30 and 48 frames a second.
const GAME_WAIT: Duration = Duration::from_nanos(1_000_000_000 / 24);

/// Whether a screen's frame goes out now or waits for its game's next one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flip {
    Now,
    /// until then at the latest: the screen's frame is drawn then if the game has not drawn one
    Wait(Instant),
}

/// Under variable refresh a screen refreshes when a frame is flipped, so every flip that is not the game's starts a refresh
/// early: the game's frames judder, and some monitors flicker in their brightness (what Hyprland's cursor:no_break_fs_vrr is
/// for). So while it is `paced` (variable refresh is on and a full screen window, the game, covers the screen and shows) only
/// the game's commits flip at once: the pointer moving or changing its picture, a notification, the panels, a window over the
/// game and the compositor's own animations wait for the game's next frame and go out with it, and at the latest
/// `GAME_WAIT` after the game's last commit (`game_last`). A game that has not committed for that long (paused, or under 24
/// frames a second) paces nothing: every change flips at once, so the pointer moves at the screen's own rate over its menu.
/// `game_committed`: the game committed since the screen was last drawn. Off this path (no variable refresh, nothing full
/// screen, the lock on) every change flips at once, as it always has.
/// Smithay's own flag for this (FrameFlags::SKIP_CURSOR_ONLY_UPDATES) is not used: it skips only a pointer moved on the
/// cursor plane (not one drawn into the picture, nor a notification or a panel), the frame is still worked out first, and its
/// empty frame would start the hold after a frame with no change (`not_before`), keeping the game's next frame back a refresh.
fn vrr_flip(paced: bool, game_committed: bool, game_last: Option<Instant>, now: Instant) -> Flip {
    if !paced || game_committed {
        return Flip::Now;
    }
    match game_last.map(|t| t + GAME_WAIT) {
        Some(at) if at > now => Flip::Wait(at),
        _ => Flip::Now,
    }
}

/// Whether a screen's frames are paced by its game (`vrr_flip`): variable refresh was on before this frame and stays on (not
/// being switched either way: the frame that switches it goes out at once), a full screen window covers the screen (variable
/// refresh always on, with no game, paces nothing), and the lock is off (its field and pointer show at once).
fn vrr_paced(vrr_on: bool, vrr_wanted: bool, fullscreen: bool, locked: bool) -> bool {
    vrr_on && vrr_wanted && fullscreen && !locked
}

/// A screen's pacing by its game under variable refresh: how many of the game's commits its last frame had
/// (`App::game_commits`). draw keeps one for each screen and asks it about every frame.
#[derive(Debug, Default)]
struct Pacer {
    game_drawn: u64,
}

impl Pacer {
    /// `vrr_flip` for the screen's frame `now`; a frame that goes out has all of the game's commits so far.
    fn flip(&mut self, paced: bool, game: GameCommits, now: Instant) -> Flip {
        let flip = vrr_flip(paced, game.count != self.game_drawn, game.last, now);
        if flip == Flip::Now {
            self.game_drawn = game.count;
        }
        flip
    }
}

/// The GPU that draws, and that programs render on: the one YGGDRASIL_COMPOSITOR_GPU names (`/dev/dri/card1`, or `card1`);
/// otherwise the boot GPU when a screen is connected to it, else the first GPU with a screen, else the boot GPU (else the
/// first). A desktop with two cards may boot from the one with no screen (the firmware goes by slot); drawing there would
/// need every frame copied to the other card, which this compositor does not do yet. Gives the path and why it was chosen.
fn render_gpu(
    boot: Option<&Path>,
    all: &[std::path::PathBuf],
    wanted: Option<&str>,
    has_screen: impl Fn(&Path) -> bool,
) -> Option<(std::path::PathBuf, &'static str)> {
    let same = |a: &Path, b: &Path| a == b || matches!((a.canonicalize(), b.canonicalize()), (Ok(x), Ok(y)) if x == y);
    if let Some(w) = wanted.map(str::trim).filter(|w| !w.is_empty()) {
        let w = if w.contains('/') { std::path::PathBuf::from(w) } else { Path::new("/dev/dri").join(w) };
        if let Some(p) = all.iter().find(|p| same(p, &w)) {
            return Some((p.clone(), "YGGDRASIL_COMPOSITOR_GPU"));
        }
        eprintln!("yggdrasil-compositor: YGGDRASIL_COMPOSITOR_GPU={} is not one of the GPUs; choosing as usual", w.display());
    }
    let mut gpus: Vec<std::path::PathBuf> = Vec::new();
    if let Some(b) = boot {
        gpus.push(b.to_path_buf());
    }
    gpus.extend(all.iter().filter(|p| !boot.is_some_and(|b| same(p, b))).cloned());
    if let Some(b) = boot.filter(|b| has_screen(b)) {
        return Some((b.to_path_buf(), if gpus.len() > 1 { "the boot GPU, which has the screens" } else { "the only GPU" }));
    }
    if let Some(p) = gpus.iter().find(|p| has_screen(p)) {
        return Some((p.clone(), "the GPU the screens are connected to (not the boot one)"));
    }
    gpus.into_iter().next().map(|p| (p, if boot.is_some() { "the boot GPU (no screen connected yet)" } else { "the first GPU" }))
}

/// Whether a screen is connected to the GPU `/dev/dri/cardN`: one of its connectors in sysfs (`cardN-DP-1/status`) says
/// "connected".
fn has_screen(gpu: &Path, sysfs: &Path) -> bool {
    let Some(name) = gpu.file_name().map(|n| n.to_string_lossy().into_owned()) else { return false };
    let prefix = format!("{name}-");
    std::fs::read_dir(sysfs).into_iter().flatten().flatten().any(|e| {
        e.file_name().to_string_lossy().starts_with(&prefix)
            && std::fs::read_to_string(e.path().join("status")).is_ok_and(|s| s.trim() == "connected")
    })
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
        Gpu::Gles { renderer, .. } => format!("OpenGL ES ({})", priority_words(renderer.egl_context().get_context_priority())),
        Gpu::Pixman { .. } => "software (pixman)".into(),
    };
    eprintln!("yggdrasil-compositor: {} draws with {kind}", path.display());
    let known = gpu_drm(&gpu).resource_handles().map(|r| r.connectors().to_vec()).unwrap_or_default();
    let device = Device {
        node,
        gpu,
        screens: HashMap::new(),
        token,
        known,
        was_connected: vec![],
        settling: vec![],
        rescan_pending: false,
        tinted: HashSet::new(),
    };
    b.devices.insert(node, device);
    drop(b);
    Ok(scan(udev, node, &[], &[], false).0)
}

/// A GPU's DRM device.
fn gpu_drm(gpu: &Gpu) -> &DrmDevice {
    match gpu {
        Gpu::Gles { manager, .. } => manager.device(),
        Gpu::Pixman { manager, .. } => manager.device(),
    }
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
                let exporter = GbmFramebufferExporter::new(gbm.clone(), render_node.into());
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
    let context = with_fallback(|| EGLContext::new_with_priority(&display, ContextPriority::High), || EGLContext::new(&display))
        .map_err(|e| format!("EGL context: {e}"))?;
    // SAFETY: the context is current only on this thread
    let renderer = unsafe { GlesRenderer::new(context) }.map_err(|e| format!("OpenGL ES: {e}"))?;
    Ok((renderer, gbm))
}

/// The compositor's GL context at high priority, so the GPU runs its frames ahead of a game's that keeps it busy (gamescope
/// asks for the same): `high` first, and `plain` (the default priority) when the driver refuses it (amdgpu lets only the DRM
/// master or a process with CAP_SYS_NICE have it; Mesa then fails to make the context). EGL may also grant less than asked
/// without failing: `priority_words` says what it reports.
fn with_fallback<C, E: std::fmt::Display>(high: impl FnOnce() -> Result<C, E>, plain: impl FnOnce() -> Result<C, E>) -> Result<C, E> {
    high().or_else(|e| {
        eprintln!("yggdrasil-compositor: a high-priority GL context was refused ({e}): the default priority");
        plain()
    })
}

/// A GL context's priority as EGL reports it, for the line each GPU prints at the start.
fn priority_words(p: Option<ContextPriority>) -> &'static str {
    match p {
        Some(ContextPriority::RealTime) => "GPU priority realtime",
        Some(ContextPriority::High) => "GPU priority high",
        Some(ContextPriority::Medium) => "GPU priority default: high not granted",
        Some(ContextPriority::Low) => "GPU priority low",
        None => "GPU priority not reported: the driver has no EGL_IMG_context_priority",
    }
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
            // explicit sync (NVIDIA needs it): on the node programs render on
            if let Some(path) = render.dev_path() {
                crate::sync::offer(app, dh, &path);
            }
            // copies may go straight into programs' GPU buffers (readback.rs): the formats the GPU draws into
            let render_formats = renderer.egl_context().dmabuf_render_formats().clone();
            let copy_formats: Vec<(Fourcc, Vec<u64>)> = COLOR_FORMATS
                .iter()
                .map(|f| (*f, render_formats.iter().filter(|x| x.code == *f).map(|x| u64::from(x.modifier)).collect::<Vec<u64>>()))
                .filter(|(_, m)| !m.is_empty())
                .collect();
            app.screencopy.dmabuf = copy_formats.iter().map(|(f, _)| *f).collect();
            app.capture.dmabuf = Some(crate::capture::DmabufOffer { device: render.dev_id(), formats: copy_formats });
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

/// The connected connectors of a GPU now, the laptop's own first (so it always finds a CRTC).
fn connected(dev: &Device) -> Vec<connector::Info> {
    let drm = gpu_drm(&dev.gpu);
    let Ok(res) = drm.resource_handles() else { return vec![] };
    let mut v: Vec<connector::Info> = res
        .connectors()
        .iter()
        .filter_map(|c| drm.get_connector(*c, true).ok())
        .filter(|c| c.state() == connector::State::Connected && !c.modes().is_empty())
        .filter(|c| !non_desktop(drm, c.handle()))
        .collect();
    v.sort_by_key(|c| !is_internal(c));
    v
}

/// A laptop's own panel, by its connector's kind.
fn is_internal(c: &connector::Info) -> bool {
    matches!(c.interface(), connector::Interface::EmbeddedDisplayPort | connector::Interface::LVDS | connector::Interface::DSI)
}

/// A DRM object's property by its name (a connector's "EDID", a CRTC's "CTM"): what the property is, and its value now.
fn property<T: ResourceHandle>(drm: &DrmDevice, handle: T, name: &str) -> Option<(property::Info, property::RawValue)> {
    let props = drm.get_properties(handle).ok()?;
    let (ids, values) = props.as_props_and_values();
    ids.iter().zip(values).find_map(|(id, v)| drm.get_property(*id).ok().filter(|p| p.name().to_str() == Ok(name)).map(|p| (p, *v)))
}

/// A VR headset says it is not a desktop screen (it is leased to the program that drives it; leasing comes later).
fn non_desktop(drm: &DrmDevice, conn: connector::Handle) -> bool {
    property(drm, conn, "non-desktop").is_some_and(|(_, v)| v != 0)
}

/// How an internal panel sits in its case, from its connector's "panel orientation" (the kernel's, from the firmware or its
/// quirks table for tablets and handhelds whose panel is mounted turned), as the turn that shows the picture upright. None when
/// the connector has no such property or says something unknown.
fn panel_orientation(drm: &DrmDevice, conn: connector::Handle) -> Option<(String, Transform)> {
    let (info, v) = property(drm, conn, "panel orientation")?;
    let property::ValueType::Enum(values) = info.value_type() else { return None };
    let word = values.get_value_from_raw_value(v)?.name().to_str().ok()?.to_string();
    panel_turn(&word).map(|t| (word, t))
}

/// The kernel's words for a panel's orientation as a turn (wl_output's, as monitors.lua's `transform`): a panel whose left side
/// is up shows the picture upright turned 90° (1), one whose right side is up turned 270° (3); KWin and Mutter turn them
/// the same way.
fn panel_turn(word: &str) -> Option<Transform> {
    match word {
        "Normal" => Some(Transform::Normal),
        "Upside Down" => Some(Transform::_180),
        "Left Side Up" => Some(Transform::_90),
        "Right Side Up" => Some(Transform::_270),
        _ => None,
    }
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
/// are given up, and so are the ones named in `off` (the lid's), their CRTC switched off (their names are returned). A screen
/// in `kept` (its link went a moment ago, outputs.rs' UNPLUG_GRACE) that is connected again gets its CRTC with the same Output.
/// With `settle`, a connector just plugged in is set up only once its link stayed for SETTLE; the third value is then when to
/// look again.
fn scan(
    udev: &Rc<RefCell<Udev>>,
    node: DrmNode,
    off: &[String],
    kept: &[Output],
    settle: bool,
) -> (Vec<Output>, Vec<String>, Option<Duration>) {
    let mut b = udev.borrow_mut();
    let Some(dev) = b.devices.get_mut(&node) else { return (vec![], vec![], None) };
    // a connector the device did not have (a DisplayPort hub or dock: MST): Smithay maps a connector's properties only when the
    // device is reset, so it is reset (every screen goes dark for a moment and is drawn whole again)
    let handles = gpu_drm(&dev.gpu).resource_handles().map(|r| r.connectors().to_vec()).unwrap_or_default();
    if handles.iter().any(|h| !dev.known.contains(h)) {
        eprintln!("yggdrasil-compositor: new connectors (a hub or dock): the GPU's screens are set up again");
        // (not while another terminal shows: again when it comes back)
        if reset_device(dev) {
            dev.known = handles;
        }
    } else {
        dev.known = handles;
    }
    let all = connected(dev);
    let handles: Vec<connector::Handle> = all.iter().map(|c| c.handle()).collect();
    let was = std::mem::replace(&mut dev.was_connected, handles.clone());
    dev.settling.retain(|(h, _)| handles.contains(h));
    let mut again: Option<Duration> = None;
    let now: Vec<connector::Info> = all.into_iter().filter(|c| !off.contains(&connector_name(c))).collect();
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
        let back = kept.iter().find(|o| o.name() == name).cloned();
        if settle && back.is_none() {
            let since = match dev.settling.iter().find(|(h, _)| *h == conn.handle()) {
                Some((_, t)) => Some(*t),
                None if !was.contains(&conn.handle()) => {
                    eprintln!("yggdrasil-compositor: {name} plugged in: set up once its link has settled");
                    dev.settling.push((conn.handle(), Instant::now()));
                    Some(Instant::now())
                }
                None => None,
            };
            if let Some(t) = since {
                let left = SETTLE.saturating_sub(t.elapsed());
                if !left.is_zero() {
                    again = Some(again.map_or(left, |a| a.min(left)));
                    continue;
                }
                dev.settling.retain(|(h, _)| *h != conn.handle());
            }
        }
        let used: Vec<crtc::Handle> = dev.screens.keys().copied().collect();
        let drm = gpu_drm(&dev.gpu);
        let Some(crtc) = free_crtc(drm, &conn, &used) else {
            eprintln!("yggdrasil-compositor: {name}: no free CRTC for it");
            continue;
        };
        let modes = conn.modes().to_vec();
        let preferred = modes.iter().find(|m| m.mode_type().contains(ModeTypeFlags::PREFERRED)).copied().unwrap_or(modes[0]);
        let output = match back {
            // back within the grace: the same output (its windows, panels and wl_output), at the mode it had where it still can
            Some(o) => {
                for m in &modes {
                    if !o.modes().contains(&Mode::from(*m)) {
                        o.add_mode(Mode::from(*m));
                    }
                }
                o
            }
            None => {
                let (make, model) = edid_names(drm, conn.handle());
                let (w, h) = conn.size().unwrap_or((0, 0));
                let o = Output::new(
                    name.clone(),
                    // no serial number: programs would get one more way to tell this machine apart
                    PhysicalProperties {
                        size: (w as i32, h as i32).into(),
                        subpixel: Subpixel::Unknown,
                        make,
                        model,
                        serial_number: String::new(),
                    },
                );
                for m in &modes {
                    o.add_mode(Mode::from(*m));
                }
                o.set_preferred(Mode::from(preferred));
                // a panel mounted turned in its case (a handheld's, a tablet's) starts upright: its turn until monitors.lua
                // gives one (outputs.rs keeps a screen's turn while nothing sets another; the tablet watcher's still wins)
                if is_internal(&conn) {
                    if let Some((word, t)) = panel_orientation(drm, conn.handle()).filter(|(_, t)| *t != Transform::Normal) {
                        eprintln!(
                            "yggdrasil-compositor: {name}: its panel sits turned ('panel orientation' {word}): turned {} unless monitors.lua turns it",
                            crate::outputs::transform_number(t)
                        );
                        o.change_current_state(None, Some(t), None, None);
                    }
                }
                o
            }
        };
        let offered: Vec<Mode> = modes.iter().map(|m| Mode::from(*m)).collect();
        let had = output.current_mode().and_then(|m| crate::outputs::closest_mode(&offered, m));
        // a new screen starts in the mode Settings' file gives it, so it is not set twice (its own preferred first, then
        // the file's as soon as the rules are read)
        let ruled = || {
            let rules =
                std::fs::read_to_string(crate::outputs::monitors_path()).map(|t| crate::outputs::parse_rules(&t)).unwrap_or_default();
            let want = crate::outputs::want_from_rules(&rules, &name, Mode::from(preferred), &offered, 1.0);
            want.enabled.then(|| crate::outputs::closest_mode(&offered, want.mode)).flatten()
        };
        let start = had.or_else(ruled).and_then(|m| modes.iter().find(|x| Mode::from(**x) == m).copied()).unwrap_or(preferred);
        if output.current_mode() != Some(Mode::from(start)) {
            // the mode only: a screen keeps its turn (outputs.rs)
            output.change_current_state(Some(Mode::from(start)), None, None, None);
        }
        let planes = drm.planes(&crtc).ok();
        let render_formats = match &dev.gpu {
            Gpu::Gles { renderer, .. } => Some(renderer.dmabuf_formats()),
            Gpu::Pixman { .. } => None,
        };
        let mut made = set_up(&mut dev.gpu, crtc, start, conn.handle(), &output, planes.clone());
        // a fast mode the GPU refuses at this first modeset (two fast monitors can need more than it has together): the
        // screen's own mode on the CRTC, so it still gets a picture. The output keeps asking the refused one until its first
        // frame, which puts the screen back at the CRTC's mode through the App (follow_mode, App::mode_kept: programs,
        // Settings and the layout told) without asking the GPU for it again; add_output's rules would ask it again otherwise
        let mut mode = Mode::from(start);
        let mut refused = None;
        if let (Err(e), true) = (&made, start != preferred) {
            eprintln!(
                "yggdrasil-compositor: {name}: {}x{} at {} Hz did not set ({e}): its own mode instead",
                mode.size.w,
                mode.size.h,
                mode.refresh as f64 / 1000.0
            );
            refused = Some(mode);
            mode = Mode::from(preferred);
            made = set_up(&mut dev.gpu, crtc, preferred, conn.handle(), &output, planes);
        }
        match made {
            Ok(drm) => {
                let vrr = match &drm {
                    Out::Gles(o) => o.with_compositor(|c| c.vrr_supported(conn.handle()).ok()),
                    Out::Pixman(o) => o.with_compositor(|c| c.vrr_supported(conn.handle()).ok()),
                };
                let vrr_capable = matches!(vrr, Some(VrrSupport::Supported));
                let render_node = dev.node.node_with_type(NodeType::Render).and_then(|n| n.ok()).unwrap_or(dev.node);
                let feedback = match (&drm, &render_formats) {
                    (Out::Gles(o), Some(f)) => screen_feedback(o, render_node, f),
                    _ => None,
                };
                match vrr {
                    Some(VrrSupport::Supported) => eprintln!("yggdrasil-compositor: {name} can use variable refresh"),
                    Some(VrrSupport::RequiresModeset) => {
                        eprintln!(
                            "yggdrasil-compositor: {name}: variable refresh needs a modeset here: not used (the screen would go dark)"
                        )
                    }
                    _ => {}
                }
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
                        internal: is_internal(&conn),
                        off: false,
                        drm,
                        modes,
                        mode,
                        refused,
                        queued: false,
                        dirty: true,
                        not_before: Instant::now(),
                        gamma: None,
                        vrr_capable,
                        feedback,
                        direct: None,
                        pacer: Pacer::default(),
                    },
                );
                added.push(output);
            }
            Err(e) => eprintln!("yggdrasil-compositor: {name}: {e}"),
        }
    }
    (added, gone, again)
}

/// A screen's DrmCompositor on its CRTC at `mode` (Smithay's test commits find whether the GPU takes it).
fn set_up(
    gpu: &mut Gpu,
    crtc: crtc::Handle,
    mode: smithay::reexports::drm::control::Mode,
    conn: connector::Handle,
    output: &Output,
    planes: Option<smithay::backend::drm::Planes>,
) -> Result<Out, String> {
    match gpu {
        Gpu::Gles { renderer, manager } => manager
            .lock()
            .initialize_output::<_, Elem>(crtc, mode, &[conn], output, planes, &mut **renderer, &DrmOutputRenderElements::default())
            .map(Out::Gles)
            .map_err(|e| e.to_string()),
        Gpu::Pixman { renderer, manager } => manager
            .lock()
            .initialize_output::<_, PixElem>(crtc, mode, &[conn], output, planes, &mut **renderer, &DrmOutputRenderElements::default())
            .map(Out::Pixman)
            .map_err(|e| e.to_string()),
    }
}

/// A GPU's connectors changed (a screen plugged in or out, the lid): screens are added and removed to match. The new ones come
/// first, so the last screen is never gone in between (the lid opened as the monitor is unplugged).
/// A screen whose connector went is kept for a moment (outputs.rs: `output_unplugged`) and, back within it, keeps its windows and
/// panels and gets a CRTC again with nothing else rebuilt; one the lid takes off goes at once.
fn device_changed(udev: &Rc<RefCell<Udev>>, app: &mut App, node: DrmNode) {
    let off = lid_plan(udev, app);
    let kept = app.leaving_outputs();
    let (added, gone, again) = scan(udev, node, &off, &kept, true);
    for o in added {
        if app.output_back(&o.name()) {
            eprintln!("yggdrasil-compositor: {} came back: kept as it was", o.name());
            continue;
        }
        let scale = default_scale_of(&o);
        app.add_output(o, scale);
    }
    for name in gone {
        if off.contains(&name) {
            if let Err(e) = app.remove_output(&name) {
                eprintln!("yggdrasil-compositor: {name}: {e}");
            }
        } else {
            app.output_unplugged(&name);
        }
    }
    // a screen settling: looked at again when it has (one timer at a time)
    let pending = udev.borrow().devices.get(&node).is_none_or(|d| d.rescan_pending);
    if let (Some(d), false) = (again, pending) {
        if let Some(dev) = udev.borrow_mut().devices.get_mut(&node) {
            dev.rescan_pending = true;
        }
        let u = udev.clone();
        let _ = app.loop_handle.insert_source(Timer::from_duration(d), move |_, _, app| {
            if let Some(dev) = u.borrow_mut().devices.get_mut(&node) {
                dev.rescan_pending = false;
            }
            device_changed(&u, app, node);
            TimeoutAction::Drop
        });
    }
    app.wake();
}

/// The internal screens the lid keeps off their CRTC now (laptop.rs): with the lid closed, while another screen of any GPU is
/// connected and on (as Settings has it, or would have it when it is not set up yet).
fn lid_plan(udev: &Rc<RefCell<Udev>>, app: &App) -> Vec<String> {
    let b = udev.borrow();
    if !b.switches.lid_closed() {
        return vec![];
    }
    let screens: Vec<(String, bool)> = b
        .devices
        .values()
        .flat_map(connected)
        .map(|c| {
            let name = connector_name(&c);
            let on = match app.head(&name) {
                Some(h) => h.enabled,
                None => {
                    crate::outputs::want_from_rules(&app.heads.rules, &name, Mode { size: (1, 1).into(), refresh: 0 }, &[], 1.0).enabled
                }
            };
            (name, on)
        })
        .collect();
    crate::laptop::lid_takes_off(true, &screens)
}

/// The scale a screen gets when Settings' monitors.lua gives it none: from its size and its own mode (laptop.rs).
fn default_scale_of(o: &Output) -> f64 {
    let size = o.physical_properties().size;
    let mode = o.preferred_mode().or_else(|| o.current_mode()).map(|m| (m.size.w, m.size.h)).unwrap_or((0, 0));
    crate::laptop::default_scale((size.w, size.h), mode)
}

/// The lid was closed or opened (libinput's lid switch, or `ctl lid`): the laptop's screen leaves or comes back, or (closed
/// with no other screen) goes dark.
fn lid(udev: &Rc<RefCell<Udev>>, app: &mut App, closed: bool) {
    let was = udev.borrow().switches.lid_closed;
    udev.borrow_mut().switches.lid_closed = Some(closed);
    if was == Some(closed) {
        return;
    }
    eprintln!("yggdrasil-compositor: the lid is {}", if closed { "closed" } else { "open" });
    if !closed {
        // opening it is the person back: the screens come on (yggdrasil-idle) as at the first input
        app.idle_state.notify_activity(&app.seat);
    }
    let nodes: Vec<DrmNode> = udev.borrow().devices.keys().copied().collect();
    for n in nodes {
        device_changed(udev, app, n);
    }
    mark_all_dirty(udev);
    app.wake();
}

/// Every screen is to be drawn again (all of it when `whole`).
fn mark_all_dirty(udev: &Rc<RefCell<Udev>>) {
    for d in udev.borrow_mut().devices.values_mut() {
        for s in d.screens.values_mut() {
            s.dirty = true;
        }
    }
}

/// Reset a device and each of its screens' state (as after a switch back): its connectors are learnt again, every CRTC is set
/// as the DrmCompositor wants it at its next frame, which is drawn whole.
fn reset_device(dev: &mut Device) -> bool {
    let r = match &mut dev.gpu {
        Gpu::Gles { manager, .. } => manager.device_mut().reset_state(),
        Gpu::Pixman { manager, .. } => manager.device_mut().reset_state(),
    };
    if let Err(e) = r {
        eprintln!("yggdrasil-compositor: the GPU could not be reset: {e}");
        return false;
    }
    reset_screens(dev);
    true
}

/// Each screen's DrmCompositor reads the CRTC's state again and its next frame is whole (after a resume or a reset).
fn reset_screens(dev: &mut Device) {
    for s in dev.screens.values_mut() {
        let r = match &s.drm {
            Out::Gles(o) => o.with_compositor(|c| c.reset_state()),
            Out::Pixman(o) => o.with_compositor(|c| c.reset_state()),
        };
        if let Err(e) = r {
            eprintln!("yggdrasil-compositor: {}: its state was not read again: {e}", s.output.name());
        }
        match &s.drm {
            Out::Gles(o) => o.reset_buffers(),
            Out::Pixman(o) => o.reset_buffers(),
        }
        s.queued = false;
        s.dirty = true;
        s.off = false;
    }
}

/// The system woke from a suspend (or `ctl resumed`): every screen's state is read again and drawn whole, the input devices are
/// set up again, and the connectors are looked at again (a screen unplugged while it slept, or plugged in).
fn resumed(udev: &Rc<RefCell<Udev>>, app: &mut App) {
    {
        let mut b = udev.borrow_mut();
        for d in b.devices.values_mut() {
            reset_screens(d);
        }
        // the input devices are set up again after the loop's pass
        b.input_seen = None;
    }
    let nodes: Vec<DrmNode> = udev.borrow().devices.keys().copied().collect();
    for n in nodes {
        device_changed(udev, app, n);
    }
    app.wake();
}

/// An input device appeared: its name (as Hyprland's) and kind, set up as input.conf says. Every pointer is set up again, as a
/// touchpad that comes after its companion "-mouse" device makes that one a TrackPoint's buttons.
fn input_added(udev: &Rc<RefCell<Udev>>, app: &mut App, device: smithay::reexports::input::Device) {
    let mut b = udev.borrow_mut();
    let taken: Vec<String> = b.inputs.iter().map(|(_, i)| i.name.clone()).collect();
    let name = crate::input::device_name(&device.name(), &taken);
    b.inputs.push((device.clone(), DeviceInfo { raw: device.name().to_string(), name, kind: InputKind::Other }));
    let caps = |d: &smithay::reexports::input::Device| crate::input::Caps {
        pointer: d.has_capability(DeviceCapability::Pointer),
        keyboard: d.has_capability(DeviceCapability::Keyboard),
        touch: d.has_capability(DeviceCapability::Touch),
        tablet_tool: d.has_capability(DeviceCapability::TabletTool),
        switch: d.has_capability(DeviceCapability::Switch),
        tap_fingers: d.config_tap_finger_count(),
    };
    let touchpads: Vec<String> =
        b.inputs.iter().filter(|(d, _)| caps(d).pointer && caps(d).tap_fingers > 0).map(|(_, i)| i.name.clone()).collect();
    for (d, info) in b.inputs.iter_mut() {
        let kind = crate::input::kind_of(&info.name, caps(d), &touchpads);
        if kind != info.kind || d == &device {
            info.kind = kind;
            set_up_input(d, info, &app.input);
        }
    }
    // the lid and tablet-mode switches are known from the start (open and off until libinput's toggle says otherwise)
    if device.has_capability(DeviceCapability::Switch) {
        use smithay::reexports::input::event::switch::Switch;
        let has = |s| device.switch_has_switch(s) == Ok(true);
        b.switches.device_added(has(Switch::Lid), has(Switch::TabletMode));
    }
    let info = b.inputs.last().map(|(_, i)| i.clone());
    app.input_devices = b.inputs.iter().map(|(_, i)| i.clone()).collect();
    if let Some(i) = info {
        eprintln!("yggdrasil-compositor: input {} ({})", i.name, i.kind.word());
        // a touchscreen: programs are told the seat has touch from the start
        if i.kind == InputKind::Touch && app.seat.get_touch().is_none() {
            app.seat.add_touch();
        }
    }
}

/// The hardware backend's own commands (the rest are control.rs's).
fn command(udev: &Rc<RefCell<Udev>>, words: &[&str], app: &mut App) -> Option<Result<Vec<String>, String>> {
    Some(match words {
        ["switches"] => Ok(udev.borrow().switches.describe()),
        // what the lid switch would say, for trying it with the lid open (the screen goes or comes as it would)
        ["lid", "open"] => {
            lid(udev, app, false);
            Ok(vec![])
        }
        ["lid", "closed"] => {
            lid(udev, app, true);
            Ok(vec![])
        }
        ["tablet-mode", v @ ("on" | "off")] => {
            udev.borrow_mut().switches.tablet_mode = Some(*v == "on");
            Ok(vec![])
        }
        // as after waking from a suspend
        ["resumed"] => {
            resumed(udev, app);
            Ok(vec![])
        }
        // every input device: its name, kind and the settings in force for it
        ["input"] => Ok(app.input_devices.iter().map(|d| crate::input::describe_device(d, &app.input)).collect()),
        _ => return None,
    })
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
    let blob = property(drm, conn, "EDID").map(|(_, v)| v).filter(|v| *v != 0);
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

/// A mouse, touchpad or TrackPoint: input.conf's settings for it (input.rs) as libinput's own; what is not set goes back to
/// libinput's default for the device. (A device without a setting refuses it.)
fn set_up_input(device: &mut smithay::reexports::input::Device, info: &DeviceInfo, conf: &yggdrasil_wm::input::InputConf) {
    use smithay::reexports::input::{ClickMethod, ScrollMethod};
    use yggdrasil_wm::input::{ClickMethod as Click, ScrollMethod as Scroll};
    if !info.kind.points() {
        return;
    }
    let p = crate::input::settings(conf, &info.name, info.kind);
    let m = crate::input::mouse_of(&p);
    let _ = device.config_accel_set_speed(m.speed.clamp(-1.0, 1.0));
    let profile = match m.accel {
        Some(yggdrasil_wm::input::Accel::Flat) => Some(AccelProfile::Flat),
        Some(yggdrasil_wm::input::Accel::Adaptive) => Some(AccelProfile::Adaptive),
        None => device.config_accel_default_profile(),
    };
    if let Some(a) = profile {
        let _ = device.config_accel_set_profile(a);
    }
    let _ = device.config_scroll_set_natural_scroll_enabled(m.natural);
    let _ = device.config_left_handed_set(m.left_handed);
    if device.config_tap_finger_count() > 0 {
        let _ = device.config_tap_set_enabled(p.tap_to_click.unwrap_or_else(|| device.config_tap_default_enabled()));
        let _ = device.config_tap_set_drag_enabled(p.tap_and_drag.unwrap_or_else(|| device.config_tap_default_drag_enabled()));
        let _ = device.config_tap_set_drag_lock_enabled(
            p.drag_lock
                .map(|on| {
                    if on {
                        smithay::reexports::input::DragLockState::EnabledTimeout
                    } else {
                        smithay::reexports::input::DragLockState::Disabled
                    }
                })
                .unwrap_or_else(|| device.config_tap_default_drag_lock_enabled()),
        );
    }
    if device.config_dwt_is_available() {
        let _ = device.config_dwt_set_enabled(p.disable_while_typing.unwrap_or_else(|| device.config_dwt_default_enabled()));
    }
    let click = match p.click_method {
        Some(Click::ButtonAreas) => Some(ClickMethod::ButtonAreas),
        Some(Click::Clickfinger) => Some(ClickMethod::Clickfinger),
        None => device.config_click_default_method(),
    };
    if let Some(c) = click.filter(|c| device.config_click_methods().contains(c)) {
        let _ = device.config_click_set_method(c);
    }
    if device.config_middle_emulation_is_available() {
        let on = p.middle_button_emulation.unwrap_or_else(|| device.config_middle_emulation_default_enabled());
        let _ = device.config_middle_emulation_set_enabled(on);
    }
    let scroll = match p.scroll_method {
        Some(Scroll::TwoFinger) => Some(ScrollMethod::TwoFinger),
        Some(Scroll::Edge) => Some(ScrollMethod::Edge),
        Some(Scroll::OnButtonDown) => Some(ScrollMethod::OnButtonDown),
        Some(Scroll::NoScroll) => Some(ScrollMethod::NoScroll),
        None => device.config_scroll_default_method(),
    };
    if let Some(sm) = scroll.filter(|sm| *sm == ScrollMethod::NoScroll || device.config_scroll_methods().contains(sm)) {
        let _ = device.config_scroll_set_method(sm);
    }
    let button = p.scroll_button.unwrap_or_else(|| device.config_scroll_default_button());
    let _ = device.config_scroll_set_button(button);
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
        // another terminal shows: nothing is drawn on the screens, but copies are still made (grim from another session)
        serve_copies(udev, app);
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
    // a screen whose frame waits for its game (`vrr_flip`) is not drawn: nothing follows a frame for it
    let mut drew = false;
    let mut waits: Option<Instant> = None;
    for (n, c) in todo {
        match draw(udev, app, n, c) {
            Some(at) => waits = Some(waits.map_or(at, |w| w.min(at))),
            None => drew = true,
        }
    }
    serve_copies(udev, app);
    if drew {
        after_frame(udev, app);
    }
    vrr_timer(udev, app, waits);
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

/// The timer that draws the screens whose frames wait for their game (`vrr_flip`): set for the earliest of them (`at`), moved
/// when that changes, and taken away when none waits. Nothing is armed off variable refresh.
fn vrr_timer(udev: &Rc<RefCell<Udev>>, app: &App, at: Option<Instant>) {
    let mut b = udev.borrow_mut();
    if b.vrr_timer.map(|(t, _)| t) == at {
        return;
    }
    if let Some((_, token)) = b.vrr_timer.take() {
        app.loop_handle.remove(token);
    }
    let Some(at) = at else { return };
    let u = udev.clone();
    let armed = app.loop_handle.insert_source(Timer::from_deadline(at), move |_, _, app| {
        u.borrow_mut().vrr_timer = None;
        draw_all(&u, app);
        TimeoutAction::Drop
    });
    b.vrr_timer = armed.ok().map(|token| (at, token));
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
    app.workspaces_refresh();
    app.popups.cleanup();
    let _ = app.display_handle.flush_clients();
}

/// The pointer picture's source rectangle (all of it, in its pixels) and the logical size that the screen's scale turns back
/// into those pixels (rounded: at most a pixel off at a fractional scale).
fn pointer_rect(pic: &crate::pointer::Picture, scale: f64) -> (Rectangle<f64, Logical>, Size<i32, Logical>) {
    let logical = |px: u32| ((px as f64 / scale).round() as i32).max(1);
    (Rectangle::from_size((pic.w as f64, pic.h as f64).into()), (logical(pic.w), logical(pic.h)).into())
}

/// The pointer for this screen, as an element on top (the DrmCompositor puts it on the cursor plane when it can, unless
/// YGGDRASIL_COMPOSITOR_CURSOR_PLANE=0: `pointer_kind`).
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
                // the picture is already in the screen's pixels (pointer_picture multiplies by the scale), so it is placed at
                // its own size: Smithay multiplies an element's logical size by the scale once more, which drew the pointer
                // 1.25 x 1.25 too big on a 1.25 screen (YGG-94). The whole picture is the source.
                let (src, size) = pointer_rect(&pic, $output.current_scale().fractional_scale());
                MemoryRenderBufferRenderElement::from_buffer(
                    &mut **$renderer,
                    (x as f64, y as f64),
                    buf,
                    None,
                    Some(src),
                    Some(size),
                    pointer_kind(cursor_plane()),
                )
                .ok()
            }
            None => None,
        }
    }};
}

/// One screen's frame: its mode as Settings set it, its elements, drawn and queued for the page flip. Gives whether a frame was
/// queued, the elements' states, the element that went to the screen directly, and, when the one that did in the last frame
/// no longer does, why.
macro_rules! frame {
    ($cache:expr, $app:expr, $renderer:expr, $out:expr, $screen:expr, $elem:ty, $flags:expr) => {{
        let output = $screen.output.clone();
        let mut elements: Vec<$elem> = vec![];
        if let Some(p) = pointer_element!($cache, $app, $renderer, &output) {
            elements.push(p.into());
        }
        let clear = if $app.locked {
            elements.extend($app.lock_elements(&mut **$renderer, &output).into_iter().map(Into::into));
            BLACK
        } else {
            elements.extend($app.scene(&mut **$renderer, &output));
            BACKGROUND
        };
        match $out.render_frame(&mut **$renderer, &elements, clear, $flags) {
            Ok(r) => {
                let direct = match &r.primary_element {
                    PrimaryPlaneElement::Element(e) => Some(e.id().clone()),
                    PrimaryPlaneElement::Swapchain(_) => None,
                };
                let why = match (&direct, &$screen.direct) {
                    (None, Some(was)) => {
                        let el = elements.iter().find(|e| e.id() == was);
                        let scale = output.current_scale().fractional_scale();
                        Some(why_composited(
                            el.and_then(|e| e.change()),
                            el.is_some_and(|e| covers(e.geometry(scale.into()), &output)),
                            r.states.element_render_state(was.clone()),
                        ))
                    }
                    _ => None,
                };
                let (empty, states) = (r.is_empty, r.states);
                if empty {
                    Ok((false, states, direct, why))
                } else {
                    $out.queue_frame(()).map(|_| (true, states, direct, why)).map_err(|e| e.to_string())
                }
            }
            Err(e) => Err(e.to_string()),
        }
    }};
}

/// The mode a screen's output asks (`asked`: Settings', monitors.lua's) against the one its CRTC has (`now`): None when they
/// are the same, else the mode asked and either where its connector's mode within half a hertz of it is in `offered` (the
/// connector's modes, as Smithay's; outputs.rs matched it so already) or why the screen cannot have it. `refused` is the mode its
/// GPU did not take when the screen was set up (`scan`): not asked of it again.
fn mode_to_set(asked: Option<Mode>, now: Mode, refused: Option<Mode>, offered: &[Mode]) -> Option<(Mode, Result<usize, &'static str>)> {
    let want = asked.filter(|w| *w != now)?;
    if refused == Some(want) {
        return Some((want, Err("its GPU did not take it when the screen was set up")));
    }
    let at = crate::outputs::closest_mode(offered, want).and_then(|c| offered.iter().position(|m| *m == c));
    Some((want, at.ok_or("not one of its modes")))
}

/// Set the mode Settings chose (when it changed) on the CRTC. True when the screen cannot have it (its GPU refuses it, or it is
/// not one of its modes): the CRTC stays at the mode it has, which the screen is to be put back at (`App::mode_kept`), so a
/// refused mode is neither shown to programs nor tried again on every frame. The mode refused at the set-up counts only at the
/// first frame (asked again by add_output's rules); asked later, it is tried as any other.
macro_rules! follow_mode {
    ($renderer:expr, $out:expr, $screen:expr, $elem:ty) => {{
        let offered: Vec<Mode> = $screen.modes.iter().map(|m| Mode::from(*m)).collect();
        match mode_to_set($screen.output.current_mode(), $screen.mode, $screen.refused.take(), &offered) {
            None => false,
            Some((want, at)) => {
                let set = at.map_err(String::from).and_then(|i| {
                    $out.use_mode::<_, $elem>($screen.modes[i], &mut **$renderer, &DrmOutputRenderElements::default())
                        .map_err(|e| e.to_string())
                });
                match set {
                    Ok(()) => {
                        $screen.mode = want;
                        false
                    }
                    Err(e) => {
                        let now = $screen.mode;
                        eprintln!(
                            "yggdrasil-compositor: {}: {}x{} at {} Hz not set ({e}): it stays at {}x{} at {} Hz",
                            $screen.output.name(),
                            want.size.w,
                            want.size.h,
                            want.refresh as f64 / 1000.0,
                            now.size.w,
                            now.size.h,
                            now.refresh as f64 / 1000.0
                        );
                        true
                    }
                }
            }
        }
    }};
}

/// Whether a screen is not lit: switched off (wlr-output-power: yggdrasil-idle), switched off in Settings (out of the space), or
/// the laptop's own behind the closed lid (`lid_dark`).
fn dark(app: &App, o: &Output, lid_dark: bool) -> bool {
    lid_dark || !app.power.is_on(o) || !app.space.outputs().any(|x| x == o)
}

/// The screen copies asked for (readback.rs), drawn with the boot GPU's renderer.
fn serve_copies(udev: &Rc<RefCell<Udev>>, app: &mut App) {
    if app.screencopy.pending.is_empty() && app.capture.pending.is_empty() {
        return;
    }
    let mut guard = udev.borrow_mut();
    let b = &mut *guard;
    let lid_closed = b.switches.lid_closed();
    let primary = b.primary;
    let Some(key) = b.devices.keys().find(|n| same_gpu(**n, primary)).or_else(|| b.devices.keys().next()).copied() else { return };
    let Some(dev) = b.devices.get_mut(&key) else { return };
    let is_dark = move |app: &App, o: &Output| dark(app, o, lid_closed && crate::laptop::is_internal(&o.name()));
    match &mut dev.gpu {
        Gpu::Gles { renderer, .. } => {
            crate::readback::serve::<GlesRenderer, GlesRenderbuffer>(&mut **renderer, Fourcc::Abgr8888, app, &mut b.copy_changed, &is_dark)
        }
        Gpu::Pixman { renderer, .. } => crate::readback::serve::<PixmanRenderer, smithay::reexports::pixman::Image<'static, 'static>>(
            &mut **renderer,
            Fourcc::Argb8888,
            app,
            &mut b.copy_changed,
            &is_dark,
        ),
    }
}

/// Draw one screen; one that is not lit has its CRTC switched off instead (once), and is lit again with a whole frame. Under
/// variable refresh a frame may wait for the full screen game's next one instead (`vrr_flip`): then the screen stays to be
/// drawn, and the time it waits until at the latest is given back.
fn draw(udev: &Rc<RefCell<Udev>>, app: &mut App, node: DrmNode, crtc: crtc::Handle) -> Option<Instant> {
    let mut guard = udev.borrow_mut();
    let b = &mut *guard;
    let lid_closed = b.switches.lid_closed();
    let dev = b.devices.get_mut(&node)?;
    // the screen's tint (Night Light) into its CRTC's gamma table, when it changed
    let want = crate::ctm::channels(&app.ctm_of(&dev.screens.get(&crtc).map(|s| s.output.name()).unwrap_or_default()));
    if dev.screens.get(&crtc).is_some_and(|s| s.gamma != Some(want)) {
        let drm = gpu_drm(&dev.gpu);
        let len = drm.get_crtc(crtc).map(|c| c.gamma_length() as usize).unwrap_or(0);
        let r = if len < 2 {
            Err("the screen has no gamma table".to_string())
        } else {
            let [r, g, bl] = want.map(|f| crate::ctm::ramp(f, len));
            drm.set_gamma(crtc, &r, &g, &bl).map_err(|e| e.to_string())
        };
        if r.is_ok() {
            if want == [1.0; 3] {
                dev.tinted.remove(&crtc);
            } else {
                dev.tinted.insert(crtc);
            }
        }
        if let Some(s) = dev.screens.get_mut(&crtc) {
            if let Err(e) = r {
                // said once: tried again only when the tint changes or the screen comes back from a terminal
                eprintln!("yggdrasil-compositor: {}: no tint: {e}", s.output.name());
            }
            s.gamma = Some(want);
        }
    }
    let screen = dev.screens.get_mut(&crtc)?;
    screen.dirty = false;
    if dark(app, &screen.output, lid_closed && screen.internal) {
        if !screen.off {
            let r = match &screen.drm {
                Out::Gles(o) => o.with_compositor(|c| c.clear()),
                Out::Pixman(o) => o.with_compositor(|c| c.clear()),
            };
            match r {
                Ok(()) => {
                    screen.off = true;
                    eprintln!("yggdrasil-compositor: {} switched off", screen.output.name());
                    b.copy_changed.insert(screen.output.name());
                }
                Err(e) if b.session.is_active() => eprintln!("yggdrasil-compositor: {}: not switched off: {e}", screen.output.name()),
                Err(_) => screen.dirty = true,
            }
        }
        return None;
    }
    if screen.off {
        // lit again: the next frame sets the mode again and is drawn whole
        screen.off = false;
        match &screen.drm {
            Out::Gles(o) => o.reset_buffers(),
            Out::Pixman(o) => o.reset_buffers(),
        }
        eprintln!("yggdrasil-compositor: {} switched on", screen.output.name());
    }
    let full = app.fullscreen_on(&screen.output);
    // variable refresh: on while a full screen window covers the screen (or always, or never: YGGDRASIL_COMPOSITOR_VRR)
    let want_vrr = screen.vrr_capable && vrr_wanted(vrr_policy(), full);
    let (now_vrr, set) = match &mut screen.drm {
        Out::Gles(o) => {
            o.with_compositor(|c| (c.vrr_enabled(), (c.vrr_enabled() != want_vrr).then(|| c.use_vrr(want_vrr).map_err(|e| e.to_string()))))
        }
        Out::Pixman(o) => {
            o.with_compositor(|c| (c.vrr_enabled(), (c.vrr_enabled() != want_vrr).then(|| c.use_vrr(want_vrr).map_err(|e| e.to_string()))))
        }
    };
    match set {
        Some(Ok(())) => {
            eprintln!("yggdrasil-compositor: {}: variable refresh {}", screen.output.name(), if want_vrr { "on" } else { "off" })
        }
        Some(Err(e)) => {
            // told once: not tried again on this screen
            eprintln!(
                "yggdrasil-compositor: {}: variable refresh not switched {} (was {now_vrr}): {e}",
                screen.output.name(),
                if want_vrr { "on" } else { "off" }
            );
            screen.vrr_capable = false;
        }
        None => {}
    }
    // under variable refresh (on already, not just switched) the game's frames pace the screen: anything else waits for its
    // next one (no hold either, so the game's frame is drawn the moment it comes)
    let paced = vrr_paced(now_vrr, want_vrr, full, app.locked);
    if let Flip::Wait(at) = screen.pacer.flip(paced, app.game_commits(&screen.output), Instant::now()) {
        screen.dirty = true;
        // told once for each wait, not at every mouse event during it
        if b.vrr_timer.is_none_or(|(t, _)| t != at) && std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
            eprintln!(
                "frame {} at {:?}: waits for the full screen window's next frame, until {:?} at the latest",
                screen.output.name(),
                b.start.elapsed(),
                at.saturating_duration_since(b.start)
            );
        }
        return Some(at);
    }
    let refused = match (&mut dev.gpu, &mut screen.drm) {
        (Gpu::Gles { renderer, .. }, Out::Gles(out)) => follow_mode!(renderer, out, screen, Elem),
        (Gpu::Pixman { renderer, .. }, Out::Pixman(out)) => follow_mode!(renderer, out, screen, PixElem),
        _ => false,
    };
    if refused {
        // the screen goes back to the mode its CRTC kept, its layout, windows and Settings with it; that wakes it, and it is
        // drawn at that mode on the next pass (the App is told with the backend let go)
        let (name, kept) = (screen.output.name(), screen.mode);
        drop(guard);
        app.mode_kept(&name, kept);
        return None;
    }
    let result = match (&mut dev.gpu, &mut screen.drm) {
        (Gpu::Gles { renderer, .. }, Out::Gles(out)) => {
            frame!(b.pointer, app, renderer, out, screen, Elem, frame_flags(scanout_allowed(), full))
        }
        (Gpu::Pixman { renderer, .. }, Out::Pixman(out)) => {
            frame!(b.pointer, app, renderer, out, screen, PixElem, frame_flags(scanout_allowed(), full))
        }
        _ => Err("the screen and its GPU do not match".into()),
    };
    let dev = b.devices.get_mut(&node)?;
    let screen = dev.screens.get_mut(&crtc)?;
    if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
        eprintln!("frame {} at {:?}: {:?}", screen.output.name(), b.start.elapsed(), result.as_ref().map(|(q, _, d, _)| (q, d.is_some())));
    }
    let result = match result {
        Ok((queued, states, direct, why)) => {
            if let Some(f) = &screen.feedback {
                app.send_dmabuf_feedback(&screen.output, &f.render, &f.scanout, &states);
            }
            if direct.is_some() != screen.direct.is_some() {
                let name = screen.output.name();
                match &why {
                    None => eprintln!("yggdrasil-compositor: {name}: a full screen picture goes to the screen directly (scanout, no copy)"),
                    Some(why) => eprintln!("yggdrasil-compositor: {name}: composited again: {why}"),
                }
            }
            screen.direct = direct;
            Ok(queued)
        }
        Err(e) => Err(e),
    };
    match result {
        Ok(true) => {
            screen.queued = true;
            let o = screen.output.clone();
            b.copy_changed.insert(o.name());
            if !app.capture.sessions.is_empty() {
                app.capture_output_changed(&o, false);
            }
        }
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
    None
}

/// Whether a program's buffer may go to the screen directly: `YGGDRASIL_COMPOSITOR_SCANOUT` 0 (no, off, false) keeps every
/// picture composited, for a driver that shows the switch badly and to measure the difference with one restart.
fn scanout_allowed() -> bool {
    thread_local!(static A: bool = crate::switched_on(std::env::var("YGGDRASIL_COMPOSITOR_SCANOUT").ok().as_deref()));
    A.with(|a| *a)
}

/// What a frame may do with the planes: Smithay's default (the primary plane takes a program's buffer of the screen's own
/// format, the overlay planes scanout candidates, the cursor plane the pointer); while a full screen window covers the
/// screen, the primary plane takes a buffer of any format the plane lists (a game's 10-bit or ABGR one): the GPU's test decides.
/// Without direct scanout only the pointer keeps a plane of its own.
fn frame_flags(scanout: bool, full: bool) -> FrameFlags {
    match (scanout, full) {
        (false, _) => FrameFlags::ALLOW_OVERLAY_PLANE_SCANOUT | FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT,
        (true, true) => FrameFlags::DEFAULT | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY,
        (true, false) => FrameFlags::DEFAULT,
    }
}

/// Whether a picture at `geo` covers all of `output`'s mode, as the one Smithay puts on the primary plane must (with nothing
/// below it showing).
fn covers(geo: Rectangle<i32, Physical>, output: &Output) -> bool {
    output.current_mode().is_some_and(|m| geo.contains_rect(Rectangle::from_size(output.current_transform().transform_size(m.size))))
}

/// Why the picture that went to the screen directly is composited again, from the compositor's change to it (look::Change),
/// whether it still covers the screen, and Smithay's state for it in this frame.
fn why_composited(change: Option<crate::look::Change>, covers: bool, state: Option<RenderElementState>) -> String {
    if let Some(c) = change {
        return c.to_string();
    }
    match state.map(|s| s.presentation_state) {
        None => "it is no longer drawn".into(),
        Some(RenderElementPresentationState::Skipped) => "it is hidden".into(),
        Some(RenderElementPresentationState::ZeroCopy) => "it went to another plane".into(),
        Some(RenderElementPresentationState::Rendering { reason: Some(RenderingReason::FormatUnsupported) }) => {
            "the primary plane does not take its buffer's format".into()
        }
        Some(RenderElementPresentationState::Rendering { reason: Some(RenderingReason::ScanoutFailed) }) => {
            "the GPU refused its buffer on the primary plane".into()
        }
        Some(RenderElementPresentationState::Rendering { reason: None }) if !covers => {
            "it no longer covers the whole screen (moved or drawn smaller)".into()
        }
        Some(RenderElementPresentationState::Rendering { reason: None }) => {
            "something is drawn over it (the pointer without a cursor plane, a menu, a notification), or its new buffer cannot go \
             to a plane (shared memory, another size)"
                .into()
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
    #[test]
    fn variable_refresh_follows_full_screen_unless_told_otherwise() {
        use super::{parse_vrr, vrr_wanted, VrrPolicy};
        assert_eq!(parse_vrr(None), VrrPolicy::FullScreen);
        assert_eq!(parse_vrr(Some("2")), VrrPolicy::FullScreen);
        assert_eq!(parse_vrr(Some("0")), VrrPolicy::Never);
        assert_eq!(parse_vrr(Some(" ON ")), VrrPolicy::Always);
        assert!(vrr_wanted(VrrPolicy::FullScreen, true) && !vrr_wanted(VrrPolicy::FullScreen, false));
        assert!(vrr_wanted(VrrPolicy::Always, false) && !vrr_wanted(VrrPolicy::Never, true));
    }

    #[test]
    fn a_full_screen_window_may_go_out_in_any_format_and_the_log_says_why_it_stops() {
        use super::{covers, frame_flags, why_composited};
        use crate::look::Change;
        use smithay::backend::drm::compositor::FrameFlags;
        use smithay::backend::renderer::element::{RenderElementPresentationState as P, RenderElementState, RenderingReason};
        use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
        use smithay::utils::{Physical, Rectangle, Transform};
        // the primary plane takes a program's buffer in its own format always, in any format while a window is full screen
        assert_eq!(frame_flags(true, false), FrameFlags::DEFAULT);
        assert!(frame_flags(true, true).contains(FrameFlags::DEFAULT | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
        assert!(!frame_flags(true, true).contains(FrameFlags::SKIP_CURSOR_ONLY_UPDATES));
        // YGGDRASIL_COMPOSITOR_SCANOUT=0 (read with crate::switched_on): no primary plane for any picture, the pointer still on
        // its plane
        for full in [false, true] {
            let f = frame_flags(false, full);
            assert!(!f.intersects(FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT | FrameFlags::ALLOW_PRIMARY_PLANE_SCANOUT_ANY));
            assert!(f.contains(FrameFlags::ALLOW_CURSOR_PLANE_SCANOUT));
        }
        let state = |presentation_state| Some(RenderElementState { visible_area: 1, presentation_state, needs_capture: false });
        // the compositor's own reason first, then the frame's
        let drawn = || state(P::Rendering { reason: None });
        assert_eq!(why_composited(Some(Change::Stretched), false, drawn()), Change::Stretched.to_string());
        assert_eq!(why_composited(None, false, None), "it is no longer drawn");
        assert_eq!(why_composited(None, true, state(P::Skipped)), "it is hidden");
        assert!(why_composited(None, true, state(P::Rendering { reason: Some(RenderingReason::FormatUnsupported) })).contains("format"));
        assert!(why_composited(None, true, state(P::Rendering { reason: Some(RenderingReason::ScanoutFailed) })).contains("GPU refused"));
        assert!(why_composited(None, true, drawn()).starts_with("something is drawn over it"));
        assert!(why_composited(None, false, drawn()).starts_with("it no longer covers the whole screen"));
        // covering the screen: all of its mode, turned as the screen is
        let o = Output::new(
            "DP-1".into(),
            PhysicalProperties {
                size: (0, 0).into(),
                subpixel: Subpixel::Unknown,
                make: "test".into(),
                model: "test".into(),
                serial_number: String::new(),
            },
        );
        o.change_current_state(Some(Mode { size: (1920, 1080).into(), refresh: 60_000 }), Some(Transform::Normal), None, None);
        let r = |x, y, w, h| Rectangle::<i32, Physical>::new((x, y).into(), (w, h).into());
        assert!(covers(r(0, 0, 1920, 1080), &o) && covers(r(-10, -10, 1940, 1100), &o));
        assert!(!covers(r(-200, -100, 1920, 1080), &o), "moved: the background shows at the right and the bottom");
        assert!(!covers(r(0, 0, 1920, 1000), &o));
        o.change_current_state(None, Some(Transform::_90), None, None);
        assert!(covers(r(0, 0, 1080, 1920), &o) && !covers(r(0, 0, 1920, 1080), &o));
    }

    #[test]
    fn under_variable_refresh_only_the_games_frames_flip_at_once() {
        use super::{vrr_flip, Flip, GAME_WAIT};
        use std::time::{Duration, Instant};
        let t0 = Instant::now();
        let ms = |n: u64| t0 + Duration::from_millis(n);
        // the game drew 5 ms ago and has not committed since: the pointer, a notification, a panel wait for its next frame,
        // at most until 1/24 s after its last one (so 40 ms after it, a game at 25 frames a second is still waited for)
        assert_eq!(vrr_flip(true, false, Some(ms(0)), ms(5)), Flip::Wait(t0 + GAME_WAIT));
        assert_eq!(vrr_flip(true, false, Some(ms(0)), ms(40)), Flip::Wait(t0 + GAME_WAIT));
        // the game committed: its frame flips at once (and takes whatever waited along)
        assert_eq!(vrr_flip(true, true, Some(ms(5)), ms(5)), Flip::Now);
        // the bound is reached, or the game is paused or under 24 frames a second: the change flips by itself
        assert_eq!(vrr_flip(true, false, Some(ms(0)), t0 + GAME_WAIT), Flip::Now);
        assert_eq!(vrr_flip(true, false, Some(ms(0)), ms(42)), Flip::Now);
        assert_eq!(vrr_flip(true, false, Some(ms(0)), ms(2000)), Flip::Now);
        // no full screen window has committed on this screen yet: nothing to wait for
        assert_eq!(vrr_flip(true, false, None, ms(5)), Flip::Now);
        // not paced (variable refresh off or just being switched, nothing full screen, the lock on): every change at once, as
        // before
        assert_eq!(vrr_flip(false, false, Some(ms(0)), ms(5)), Flip::Now);
        assert_eq!(vrr_flip(false, true, Some(ms(0)), ms(5)), Flip::Now);
        assert_eq!(GAME_WAIT, Duration::from_nanos(41_666_666));
    }

    #[test]
    fn only_a_game_under_variable_refresh_that_was_on_paces_its_screen() {
        use super::vrr_paced;
        // (variable refresh on before this frame, wanted for it, a full screen window on the screen, the lock on)
        assert!(vrr_paced(true, true, true, false), "a game under variable refresh");
        assert!(!vrr_paced(false, true, true, false), "the frame that switches it on goes out at once");
        assert!(!vrr_paced(true, false, false, false), "nor the one that switches it off (the game left full screen)");
        assert!(!vrr_paced(true, true, false, false), "variable refresh always on (YGGDRASIL_COMPOSITOR_VRR=1) with no game");
        assert!(!vrr_paced(true, true, true, true), "the lock over the game");
        assert!(!vrr_paced(false, false, false, false), "off variable refresh");
    }

    /// One screen as draw_all and draw see it, for `flips`: whether its frames are paced, the `Pacer` draw keeps for it,
    /// whether something changed since it was drawn, the game's commits so far, the timer for its wait, and the flips so far.
    struct Sim {
        paced: bool,
        t0: std::time::Instant,
        pacer: super::Pacer,
        dirty: bool,
        game: crate::state::GameCommits,
        timer: Option<std::time::Instant>,
        flips: Vec<(f64, bool)>,
    }

    impl Sim {
        /// draw for the screen at `now`, when it is to be drawn
        fn pass(&mut self, now: std::time::Instant) {
            if !self.dirty {
                return;
            }
            let carries = self.game.count != self.pacer.game_drawn;
            match self.pacer.flip(self.paced, self.game, now) {
                super::Flip::Now => {
                    self.flips.push(((now - self.t0).as_secs_f64() * 1000.0, carries));
                    (self.dirty, self.timer) = (false, None);
                }
                super::Flip::Wait(w) => self.timer = Some(w),
            }
        }
    }

    /// The flips one screen makes for a run of changes, as draw_all and draw decide them: a change (ms after the start; true:
    /// the game's commit, false: anything else) marks the screen to be drawn, and it is drawn when its `Pacer` says so, at once
    /// or when the timer for its wait fires. Gives each flip's time (ms) and whether it carried a game frame.
    fn flips(paced: bool, changes: &[(f64, bool)]) -> Vec<(f64, bool)> {
        let t0 = std::time::Instant::now();
        let at = |ms: f64| t0 + std::time::Duration::from_secs_f64(ms / 1000.0);
        let mut s = Sim { paced, t0, pacer: super::Pacer::default(), dirty: false, game: Default::default(), timer: None, flips: vec![] };
        for &(t, game) in changes {
            if let Some(w) = s.timer.filter(|w| *w <= at(t)) {
                s.pass(w);
            }
            if game {
                s.game = crate::state::GameCommits { count: s.game.count + 1, last: Some(at(t)) };
            }
            s.dirty = true;
            s.pass(at(t));
        }
        if let Some(w) = s.timer {
            s.pass(w);
        }
        s.flips
    }

    /// A game's frames at `fps` for `ms` milliseconds, each late or early by the next of `jitter` (ms, in turn), and the mouse
    /// moving all the while (1000 Hz), in time order.
    fn game_and_mouse(fps: f64, ms: usize, jitter: &[f64]) -> (Vec<f64>, Vec<(f64, bool)>) {
        let frames = (ms as f64 * fps / 1000.0) as usize;
        let game: Vec<f64> = (0..=frames).map(|i| i as f64 * 1000.0 / fps + jitter[i % jitter.len()]).collect();
        let mut changes: Vec<(f64, bool)> = game.iter().map(|t| (*t, true)).chain((0..ms).map(|i| (i as f64 + 0.5, false))).collect();
        changes.sort_by(|a, b| a.0.total_cmp(&b.0));
        (game, changes)
    }

    #[test]
    fn a_game_paces_the_screen_and_the_pointer_goes_out_with_its_frames() {
        let round = |v: Vec<(f64, bool)>| v.into_iter().map(|(t, g)| ((t * 10.0).round() / 10.0, g)).collect::<Vec<_>>();
        // a game at 60 frames a second for 100 ms and the mouse moving all the while: under variable refresh the screen flips
        // with the game's frames only, every one carrying a game frame; without, at every mouse event
        let (game, changes) = game_and_mouse(60.0, 100, &[0.0]);
        let paced = flips(true, &changes);
        assert_eq!(paced.len(), game.len(), "{paced:?}");
        assert!(paced.iter().zip(&game).all(|((t, g), want)| *g && (t - want).abs() < 0.01), "{paced:?}");
        assert_eq!(flips(false, &changes).len(), changes.len());
        // a game capped at 30 frames a second whose frames come up to 3 ms late or early (frame limiters do), for half a
        // second: still every flip is one of its frames, none the pointer's own (with the bound at exactly 1/30 s every late
        // frame would follow a flip of the pointer's)
        let (game, changes) = game_and_mouse(30.0, 500, &[0.0, 3.0, -3.0, 1.0, 3.0, -2.0, 0.5, 3.0]);
        let paced = flips(true, &changes);
        assert_eq!(paced.len(), game.len(), "{paced:?}");
        assert!(paced.iter().zip(&game).all(|((t, g), want)| *g && (t - want).abs() < 0.01), "{paced:?}");
        // a notification (or a panel) 10 ms after a frame of a game at 40 frames a second goes out with its next frame
        assert_eq!(round(flips(true, &[(0.0, true), (10.0, false), (25.0, true)])), [(0.0, true), (25.0, true)]);
        // the game stops right after the pointer moved: the pointer is shown 1/24 s after the game's last frame
        assert_eq!(round(flips(true, &[(0.0, true), (5.0, false)])), [(0.0, true), (41.7, false)]);
        // a paused game (its menu does not redraw): every move flips at once, the pointer at the screen's own rate
        assert_eq!(round(flips(true, &[(0.0, true), (500.0, false), (501.0, false), (502.0, false)])).len(), 4);
        // a game under 24 frames a second (20): the pointer waits for the bound, then moves by itself until the next frame
        assert_eq!(
            round(flips(true, &[(0.0, true), (10.0, false), (45.0, false), (50.0, true)])),
            [(0.0, true), (41.7, false), (45.0, false), (50.0, true)]
        );
    }

    #[test]
    fn the_gpu_with_the_screens_draws_and_the_variable_wins() {
        use std::path::{Path, PathBuf};
        let (c0, c1) = (PathBuf::from("/dev/dri/card0"), PathBuf::from("/dev/dri/card1"));
        let all = vec![c0.clone(), c1.clone()];
        let screens_on = |which: &'static str| move |p: &Path| p.ends_with(which);
        // one GPU, with or without a screen
        assert_eq!(super::render_gpu(Some(&c1), std::slice::from_ref(&c1), None, screens_on("card1")), Some((c1.clone(), "the only GPU")));
        // the boot card has the screens
        assert_eq!(super::render_gpu(Some(&c0), &all, None, screens_on("card0")).unwrap().0, c0);
        // the boot card has none: the one with the screens draws
        let (p, why) = super::render_gpu(Some(&c0), &all, None, screens_on("card1")).unwrap();
        assert_eq!((p, why), (c1.clone(), "the GPU the screens are connected to (not the boot one)"));
        // nothing connected yet: the boot card
        assert_eq!(super::render_gpu(Some(&c1), &all, None, |_| false).unwrap().0, c1);
        assert_eq!(super::render_gpu(None, &all, None, |_| false).unwrap().0, c0);
        // the variable, by path or name; one that names no GPU is ignored
        assert_eq!(
            super::render_gpu(Some(&c0), &all, Some("card1"), screens_on("card0")).unwrap(),
            (c1.clone(), "YGGDRASIL_COMPOSITOR_GPU")
        );
        assert_eq!(super::render_gpu(Some(&c0), &all, Some("/dev/dri/card1"), screens_on("card0")).unwrap().0, c1);
        assert_eq!(super::render_gpu(Some(&c0), &all, Some("card7"), screens_on("card0")).unwrap().0, c0);
        assert_eq!(super::render_gpu(None, &[], None, |_| true), None);
    }

    #[test]
    fn a_screen_counts_when_one_connector_of_that_gpu_says_connected() {
        let t = std::env::temp_dir().join(format!("ygg-sysfs-{}", std::process::id()));
        for (dir, status) in [
            ("card0-DP-1", "disconnected"),
            ("card1-HDMI-A-1", "disconnected"),
            ("card1-DP-2", "connected\n"),
            ("card10-DP-1", "connected"),
        ] {
            std::fs::create_dir_all(t.join(dir)).unwrap();
            std::fs::write(t.join(dir).join("status"), status).unwrap();
        }
        let (zero, one, two) = (
            super::has_screen(std::path::Path::new("/dev/dri/card0"), &t),
            super::has_screen(std::path::Path::new("/dev/dri/card1"), &t),
            super::has_screen(std::path::Path::new("/dev/dri/card2"), &t),
        );
        std::fs::remove_dir_all(&t).unwrap();
        assert!(!zero && one && !two);
    }

    use super::*;

    fn v(s: &[&str]) -> Vec<String> {
        s.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn the_pointer_is_drawn_at_its_own_pixels() {
        // YGG-94: on a scaled screen the picture (already scaled) must come out at its own size, not scaled again
        for (scale, px) in [(1.0, 24u32), (1.25, 30), (1.5, 36), (2.0, 48), (1.25, 24), (1.75, 42)] {
            let pic = crate::pointer::Picture { w: px, h: px, hot: (0, 0), data: vec![] };
            let (src, size) = pointer_rect(&pic, scale);
            assert_eq!(src.size, (px as f64, px as f64).into());
            let shown = size.to_f64().to_physical(scale);
            assert!((shown.w - px as f64).abs() <= 1.0, "scale {scale}: {px} px shown as {}", shown.w);
        }
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
    fn a_refused_high_priority_falls_back_to_the_default() {
        // granted: the high-priority context, the default one never made
        assert_eq!(with_fallback(|| Ok::<_, String>("high"), || panic!("not asked")), Ok("high"));
        // refused (Mesa fails the context when amdgpu says no): the default priority
        assert_eq!(with_fallback(|| Err("EGL_BAD_ALLOC".to_string()), || Ok("default")), Ok("default"));
        // neither: the default's own error
        assert_eq!(
            with_fallback(|| Err::<&str, _>("refused".to_string()), || Err("no context".to_string())),
            Err("no context".to_string())
        );
        // what each GPU's line says, granted or not
        assert_eq!(priority_words(Some(ContextPriority::High)), "GPU priority high");
        assert!(priority_words(Some(ContextPriority::Medium)).contains("high not granted"));
        assert!(priority_words(None).contains("not reported"));
    }

    #[test]
    fn the_pointer_leaves_the_cursor_plane_when_switched_off() {
        // YGGDRASIL_COMPOSITOR_CURSOR_PLANE (read with crate::switched_on): a cursor, which Smithay may put on a plane, or an
        // element like the rest, which no plane takes
        assert_eq!(pointer_kind(crate::switched_on(None)), Kind::Cursor);
        assert_eq!(pointer_kind(crate::switched_on(Some("0"))), Kind::Unspecified);
        assert_eq!(pointer_kind(crate::switched_on(Some("off"))), Kind::Unspecified);
    }

    #[test]
    fn a_panels_orientation_is_its_turn() {
        // the kernel's words (drm_connector.c's panel orientation enum) as wl_output's numbers, as KWin and Mutter turn them
        let n = |w: &str| panel_turn(w).map(crate::outputs::transform_number);
        assert_eq!(n("Normal"), Some(0));
        assert_eq!(n("Left Side Up"), Some(1));
        assert_eq!(n("Upside Down"), Some(2));
        assert_eq!(n("Right Side Up"), Some(3), "a Steam Deck's, which Hyprland users turn with transform 3");
        assert_eq!(n("Sideways"), None);
    }

    #[test]
    fn a_mode_the_screen_cannot_have_leaves_it_as_it_is() {
        use super::mode_to_set;
        let m = |w: i32, h: i32, mhz: i32| Mode { size: (w, h).into(), refresh: mhz };
        // an ultrawide's own size at 60 (its EDID's first), 144 and 180, and a smaller size
        let offered = [m(3440, 1440, 59_973), m(3440, 1440, 144_000), m(3440, 1440, 180_000), m(2560, 1440, 59_951)];
        let now = offered[0];
        // nothing asked, or the mode the CRTC has: nothing to set
        assert_eq!(mode_to_set(None, now, None, &offered), None);
        assert_eq!(mode_to_set(Some(now), now, Some(offered[2]), &offered), None);
        // another of its modes: that one, also when asked within half a hertz of it
        assert_eq!(mode_to_set(Some(offered[1]), now, None, &offered), Some((offered[1], Ok(1))));
        assert_eq!(mode_to_set(Some(m(3440, 1440, 180_400)), now, None, &offered), Some((m(3440, 1440, 180_400), Ok(2))));
        // the mode the GPU refused at the set-up (scan) is not asked of it again: the screen stays as it is
        let refused = mode_to_set(Some(offered[2]), now, Some(offered[2]), &offered);
        assert!(matches!(refused, Some((w, Err(_))) if w == offered[2]), "{refused:?}");
        // that refusal holds for that mode only
        assert_eq!(mode_to_set(Some(offered[1]), now, Some(offered[2]), &offered), Some((offered[1], Ok(1))));
        // a mode the connector does not have (another size; a rate more than half a hertz off)
        assert!(matches!(mode_to_set(Some(m(3840, 1600, 60_000)), now, None, &offered), Some((_, Err(_)))));
        assert!(matches!(mode_to_set(Some(m(3440, 1440, 165_000)), now, None, &offered), Some((_, Err(_)))));
    }

    #[test]
    fn a_refresh_is_one_frame_long() {
        let m = Mode { size: (1920, 1080).into(), refresh: 60_000 };
        assert_eq!(refresh_of(&m), Duration::from_micros(16_666));
    }
}
