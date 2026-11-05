//! yggdrasil-compositor: Yggdrasil's own Wayland compositor, on Smithay, in development (the session runs on Hyprland meanwhile).
//!
//! Today it runs nested in a window (winit backend) while it is being built, so it cannot touch the real session:
//!
//!   yggdrasil-compositor [--run COMMAND ARGS... [--run COMMAND ...]]
//!       opens a window and serves a Wayland socket (its name is printed as WAYLAND_DISPLAY=...);
//!       programs given with --run are started inside it
//!
//! X11 programs run through XWayland, started with the compositor (its display is printed as DISPLAY=...);
//! YGGDRASIL_COMPOSITOR_XWAYLAND=0 leaves it out.
//!
//! See docs/COMPOSITOR.md for what is built and what is not.

mod deco;
mod draw;
mod grabs;
mod shortcuts;
mod switcher;
mod tiling;
mod state;
mod toplevels;
mod xwayland;
mod x11asked;

use std::sync::Arc;

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent};
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use state::{App, ClientState};

/// linux-dmabuf: programs that draw with the GPU (games, browsers, anything on Vulkan or OpenGL) hand their pictures over as GPU
/// buffers. The formats offered are the ones this renderer can read; with the GPU's render node known, programs are also told which
/// GPU to render on (default feedback), which they need to pick the right one on a machine with two.
fn gpu_buffers(app: &mut App, dh: &smithay::reexports::wayland_server::DisplayHandle, backend: &std::rc::Rc<std::cell::RefCell<winit::WinitGraphicsBackend<GlesRenderer>>>) {
    use smithay::backend::egl::EGLDevice;
    use smithay::backend::renderer::{ImportDma, ImportEgl};
    use smithay::wayland::dmabuf::DmabufFeedbackBuilder;
    let mut b = backend.borrow_mut();
    let formats: Vec<_> = b.renderer().dmabuf_formats().into_iter().collect();
    if formats.is_empty() {
        eprintln!("yggdrasil-compositor: the renderer reads no GPU buffer formats; programs will use shared memory");
        return;
    }
    let node = EGLDevice::device_for_display(b.renderer().egl_context().display()).ok().and_then(|d| d.try_get_render_node().ok().flatten());
    let feedback = node.and_then(|n| DmabufFeedbackBuilder::new(n.dev_id(), formats.iter().copied()).build().ok());
    match &feedback {
        Some(f) => {
            app.dmabuf_state.create_global_with_default_feedback::<App>(dh, f);
        }
        None => {
            app.dmabuf_state.create_global::<App>(dh, formats.iter().copied());
        }
    }
    // older Mesa programs still look for wl_drm (EGL's own way); harmless when the driver has no use for it
    let _ = b.renderer().bind_wl_display(dh);
    if std::env::var_os("YGGDRASIL_COMPOSITOR_DEBUG").is_some() {
        eprintln!("dmabuf: {} formats, render node {:?}", formats.len(), node.map(|n| n.dev_path()));
    }
    drop(b);
    let r = backend.clone();
    app.dmabuf_import = Some(Box::new(move |d| r.borrow_mut().renderer().import_dmabuf(d, None).is_ok()));
}

/// Start the --run programs: on this compositor's Wayland socket, and X11 ones on its XWayland (never the host's X server).
fn start_programs(run: &[Vec<String>], wayland: &str, x11: Option<u32>) {
    for cmd in run {
        let mut c = std::process::Command::new(&cmd[0]);
        c.args(&cmd[1..]).env("WAYLAND_DISPLAY", wayland).env_remove("DISPLAY");
        if let Some(n) = x11 {
            c.env("DISPLAY", format!(":{n}"));
        }
        if let Err(e) = c.spawn() {
            eprintln!("yggdrasil-compositor: cannot start {}: {e}", cmd[0]);
        }
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Ok(f) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(f).init();
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut run: Vec<Vec<String>> = vec![];
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--run" {
            // everything up to the next --run is one command
            let mut cmd = vec![];
            i += 1;
            while i < args.len() && args[i] != "--run" {
                cmd.push(args[i].clone());
                i += 1;
            }
            if !cmd.is_empty() {
                run.push(cmd);
            }
        } else {
            i += 1;
        }
    }

    let mut event_loop: EventLoop<App> = EventLoop::try_new()?;
    let display: Display<App> = Display::new()?;
    let dh = display.handle();
    let (backend, winit) = winit::init::<GlesRenderer>().map_err(|e| format!("cannot open a window: {e}"))?;
    backend.window().set_title("Yggdrasil compositor (nested)");
    let backend = std::rc::Rc::new(std::cell::RefCell::new(backend));
    let (ping, ping_source) = smithay::reexports::calloop::ping::make_ping()?;

    let mode = Mode { size: backend.borrow().window_size(), refresh: 60_000 };
    let output = Output::new("yggdrasil-1".into(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Yggdrasil".into(), model: "nested window".into() });
    let _global = output.create_global::<App>(&dh);
    toplevels::create_global(&dh);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let mut app = App::new(&dh, output.clone(), event_loop.handle(), event_loop.get_signal(), ping.clone());
    gpu_buffers(&mut app, &dh, &backend);
    // the pointer's look follows what the program under it asks for (nested: the host window's cursor)
    let for_cursor = backend.clone();
    app.cursor_hook = Some(Box::new(move |image| {
        use smithay::input::pointer::CursorImageStatus;
        let b = for_cursor.borrow();
        match image {
            CursorImageStatus::Hidden => b.window().set_cursor_visible(false),
            CursorImageStatus::Named(icon) => {
                b.window().set_cursor_visible(true);
                b.window().set_cursor(*icon);
            }
            // a picture of the program's own: shown as the default arrow until the compositor draws pointers itself
            CursorImageStatus::Surface(_) => {
                b.window().set_cursor_visible(true);
                b.window().set_cursor(smithay::input::pointer::CursorIcon::Default);
            }
        }
    }));
    // a wake-up asks the window for a new frame; nothing is drawn while nothing changes
    let for_ping = backend.clone();
    event_loop.handle().insert_source(ping_source, move |_, _, _| for_ping.borrow().window().request_redraw())?;
    ping.ping();
    app.space.map_output(&output, (0, 0));

    // Wayland clients: the display itself, and the socket they connect to.
    event_loop.handle().insert_source(Generic::new(display, Interest::READ, LoopMode::Level), |_, display, app| {
        // SAFETY: the display is not dropped while it is registered here.
        unsafe { display.get_mut().dispatch_clients(app).unwrap() };
        Ok(PostAction::Continue)
    })?;
    let socket = ListeningSocketSource::new_auto()?;
    let name = socket.socket_name().to_string_lossy().into_owned();
    println!("WAYLAND_DISPLAY={name}");
    app.socket = name.clone();
    event_loop.handle().insert_source(socket, |stream, _, app| {
        let _ = app.display_handle.insert_client(stream, Arc::new(ClientState::default()));
    })?;
    // X11 programs (XWayland). The --run programs wait until it has answered (a fraction of a second), so an X11 program among
    // them finds its window manager there; without XWayland they start at once.
    let xwayland = std::env::var("YGGDRASIL_COMPOSITOR_XWAYLAND").map(|v| v != "0" && v != "off").unwrap_or(true);
    let start_run = {
        let name = name.clone();
        move |app: &mut App| start_programs(&run, &name, app.x11_display)
    };
    if !xwayland {
        start_run(&mut app);
    } else if let Err(e) = xwayland::start(&mut app, event_loop.handle(), Box::new(start_run.clone())) {
        eprintln!("yggdrasil-compositor: XWayland could not start ({e}); X11 programs will not run");
        start_run(&mut app);
    }

    // workspaces.conf and shortcuts.conf are read again when they change (Settings writes them)
    event_loop.handle().insert_source(smithay::reexports::calloop::timer::Timer::from_duration(std::time::Duration::from_secs(2)), |_, _, app| {
        app.reload_config();
        smithay::reexports::calloop::timer::TimeoutAction::ToDuration(std::time::Duration::from_secs(2))
    })?;
    let mut damage_tracker = OutputDamageTracker::from_output(&output);
    let start = std::time::Instant::now();
    let mut dh2 = dh.clone();
    event_loop.handle().insert_source(winit, move |event, _, app| match event {
        WinitEvent::Resized { size, .. } => {
            output.change_current_state(Some(Mode { size, refresh: 60_000 }), None, None, None);
            app.output_resized();
        }
        WinitEvent::Input(event) => app.on_input(event),
        WinitEvent::Redraw => {
            let mut backend = backend.borrow_mut();
            let mut damage: Option<Vec<Rectangle<i32, smithay::utils::Physical>>> = None;
            {
                let age = backend.buffer_age().unwrap_or(0);
                let Ok((renderer, mut fb)) = backend.bind() else { return };
                if app.locked {
                    // While the session is locked only the locker's own picture is drawn: nothing of the session shows.
                    let elements = app.lock_elements(renderer);
                    damage = damage_tracker.render_output(renderer, &mut fb, age, &elements, [0.0f32, 0.0, 0.0, 1.0]).ok().and_then(|r| r.damage.cloned());
                } else {
                    let elements = app.scene(renderer);
                    damage = damage_tracker.render_output(renderer, &mut fb, age, &elements, [0.05f32, 0.05, 0.08, 1.0]).ok().and_then(|r| r.damage.cloned());
                }
            }
            // only what changed is sent on; a frame with no change at all is not even submitted
            if let Some(d) = &damage {
                let _ = backend.submit(Some(d));
            }
            app.send_frames(start.elapsed());
            app.space.refresh();
            app.foreign_refresh();
            app.popups.cleanup();
            let _ = dh2.flush_clients();
        }
        WinitEvent::CloseRequested => app.quit(),
        _ => {}
    })?;

    // Replies go out after every pass of the loop, not only after a frame: while nothing is drawn (the idle case) a program
    // waiting on a round trip would otherwise wait for the next 2-second config check (vkcube never got its window up).
    event_loop.run(None, &mut app, |app| {
        let _ = app.display_handle.flush_clients();
    })?;
    Ok(())
}
