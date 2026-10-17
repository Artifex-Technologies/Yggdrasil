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
    if args.first().map(String::as_str) == Some("--dialog") {
        return dialog();
    }
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
    let uid = unsafe { libc::getuid() };
    let _conn = match agent::register(agent::Agent { uid, child: Mutex::new(None) }, subject) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("labyrinth-polkit: could not register as the authentication agent: {e}");
            std::process::exit(1);
        }
    };
    eprintln!("labyrinth-polkit: registered");
    // no window here: each request opens the password box as a process of its own
    loop {
        std::thread::park();
    }
}

/// The password box for one request: reads what to show from stdin, writes the typed answers to stdout, ends when told.
fn dialog() -> eframe::Result {
    use std::io::{BufRead, Write};
    let (tx, rx) = channel();
    let ctx_slot: Arc<Mutex<Option<egui::Context>>> = Arc::new(Mutex::new(None));
    let wake_slot = ctx_slot.clone();
    std::thread::spawn(move || {
        let wake = || {
            if let Some(c) = wake_slot.lock().unwrap().as_ref() {
                c.request_repaint();
            }
        };
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(&line) else { continue };
            let text = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
            let msg = match v.get("t").and_then(|t| t.as_str()).unwrap_or("") {
                "begin" => agent::UiMsg::Begin { message: text("message"), action_id: text("action_id"), user: text("user") },
                "prompt" => {
                    let (rtx, rrx) = channel::<Option<String>>();
                    std::thread::spawn(move || {
                        let a = rrx.recv().ok().flatten();
                        let v = match a {
                            Some(mut p) => {
                                let v = serde_json::json!({"answer": p});
                                zeroize::Zeroize::zeroize(&mut p);
                                v
                            }
                            None => serde_json::json!({"answer": null}),
                        };
                        let mut out = std::io::stdout().lock();
                        let _ = writeln!(out, "{v}");
                        let _ = out.flush();
                    });
                    agent::UiMsg::Prompt { text: text("text"), echo: v.get("echo").and_then(|e| e.as_bool()).unwrap_or(false), reply: rtx }
                }
                "message" => agent::UiMsg::Message { text: text("text"), error: v.get("error").and_then(|e| e.as_bool()).unwrap_or(false) },
                "done" => agent::UiMsg::Done { ok: true },
                _ => continue,
            };
            let _ = tx.send(msg);
            wake();
        }
        // the agent went away: nothing more can be answered
        std::process::exit(1);
    });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([440.0, 250.0])
            .with_resizable(false)
            .with_title("Authentication required")
            .with_app_id("labyrinth-polkit")
            .with_always_on_top(),
        ..Default::default()
    };
    eframe::run_native(
        "Authentication required",
        options,
        Box::new(move |cc| Ok(Box::new(ui::App::new(rx, ctx_slot, &cc.egui_ctx)))),
    )
}
