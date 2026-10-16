//! labyrinth-polkit: Labyrinth's polkit authentication agent. Run in the session (autostart):
//!
//!   labyrinth-polkit                 serve this login session (replaces KDE's polkit agent: only one agent per session can be registered)
//!   labyrinth-polkit --process PID   serve only that process (for testing: `pkexec` run from PID asks this agent)
//!
//! When something asks for administrator rights, polkit calls this program; it shows a password box, hands the password to polkit's
//! own helper (which checks it through PAM and tells polkit the result), and wipes the password from memory. It never decides
//! anything itself.

mod agent;
mod helper;
mod ui;

use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};

use eframe::egui;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let subject = match args.as_slice() {
        [flag, pid] if flag == "--process" => match pid.parse::<u32>() {
            Ok(p) => agent::Subject::Process(p),
            Err(_) => {
                eprintln!("labyrinth-polkit: --process needs a number");
                std::process::exit(2);
            }
        },
        [] => match std::env::var("XDG_SESSION_ID") {
            Ok(id) if !id.is_empty() => agent::Subject::Session(id),
            _ => {
                eprintln!("labyrinth-polkit: no XDG_SESSION_ID (run it inside the login session)");
                std::process::exit(2);
            }
        },
        _ => {
            eprintln!("usage: labyrinth-polkit [--process PID]");
            std::process::exit(2);
        }
    };

    let (tx, rx) = channel();
    let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    let wake_slot = ctx_slot.clone();
    let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(move || {
        if let Some(c) = wake_slot.lock().unwrap().as_ref() {
            c.request_repaint();
        }
    });
    let uid = unsafe { libc::getuid() };
    let _conn = match agent::register(agent::Agent { ui: tx, wake, uid }, subject) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("labyrinth-polkit: could not register as the authentication agent: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("labyrinth-polkit: registered");

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 250.0])
            .with_resizable(false)
            .with_title("Authentication required")
            .with_app_id("labyrinth-polkit")
            .with_visible(false)
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native(
        "Authentication required",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(rx, ctx_slot, &cc.egui_ctx)))),
    )
}
