//! labyrinth-compositor: Labyrinth's own Wayland compositor, on Smithay. It is the piece that will replace KWin.
//!
//! Today it runs nested in a window (winit backend) while it is being built, so it cannot touch the real session:
//!
//!   labyrinth-compositor [--run COMMAND ARGS... [--run COMMAND ...]]
//!       opens a window and serves a Wayland socket (its name is printed as WAYLAND_DISPLAY=...);
//!       programs given with --run are started inside it
//!
//! See docs/COMPOSITOR.md for what is built and what is not.

mod grabs;
mod state;

use std::sync::Arc;

use smithay::backend::renderer::damage::OutputDamageTracker;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::backend::winit::{self, WinitEvent};
use smithay::desktop::space::render_output;
use smithay::output::{Mode, Output, PhysicalProperties, Subpixel};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{EventLoop, Interest, Mode as LoopMode, PostAction};
use smithay::reexports::wayland_server::Display;
use smithay::utils::{Rectangle, Transform};
use smithay::wayland::socket::ListeningSocketSource;

use state::{App, ClientState};

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
    let (mut backend, winit) = winit::init::<GlesRenderer>().map_err(|e| format!("cannot open a window: {e}"))?;
    backend.window().set_title("Labyrinth compositor (nested)");

    let mode = Mode { size: backend.window_size(), refresh: 60_000 };
    let output = Output::new("labyrinth-1".into(), PhysicalProperties { size: (0, 0).into(), subpixel: Subpixel::Unknown, make: "Labyrinth".into(), model: "nested window".into() });
    let _global = output.create_global::<App>(&dh);
    output.change_current_state(Some(mode), Some(Transform::Flipped180), None, Some((0, 0).into()));
    output.set_preferred(mode);

    let mut app = App::new(&dh, output.clone(), event_loop.handle(), event_loop.get_signal());
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
    event_loop.handle().insert_source(socket, |stream, _, app| {
        let _ = app.display_handle.insert_client(stream, Arc::new(ClientState::default()));
    })?;
    for cmd in &run {
        let mut c = std::process::Command::new(&cmd[0]);
        c.args(&cmd[1..]).env("WAYLAND_DISPLAY", &name).env_remove("DISPLAY");
        if let Err(e) = c.spawn() {
            eprintln!("labyrinth-compositor: cannot start {}: {e}", cmd[0]);
        }
    }

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
            let size = backend.window_size();
            let damage = Rectangle::from_size(size);
            {
                let age = backend.buffer_age().unwrap_or(0);
                let Ok((renderer, mut fb)) = backend.bind() else { return };
                if app.locked {
                    // While the session is locked only the locker's own picture is drawn: nothing of the session shows.
                    let elements = app.lock_elements(renderer);
                    let _ = damage_tracker.render_output(renderer, &mut fb, age, &elements, [0.0f32, 0.0, 0.0, 1.0]);
                } else {
                    let _ = render_output::<_, WaylandSurfaceRenderElement<GlesRenderer>, _, _>(&output, renderer, &mut fb, 1.0, age, [&app.space], &[], &mut damage_tracker, [0.05f32, 0.05, 0.08, 1.0]);
                }
            }
            let _ = backend.submit(Some(&[damage]));
            app.send_frames(start.elapsed());
            app.space.refresh();
            app.popups.cleanup();
            let _ = dh2.flush_clients();
            backend.window().request_redraw();
        }
        WinitEvent::CloseRequested => app.quit(),
        _ => {}
    })?;

    event_loop.run(None, &mut app, |_| {})?;
    Ok(())
}
