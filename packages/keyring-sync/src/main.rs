//! labyrinth-keyring-sync: keeps gnome-keyring's login keyring and the login password together.
//!
//! The keyring is locked with the login password and PAM unlocks it at login. If the two have drifted apart (an administrator reset the
//! password, or a tool changed it without telling PAM), the keyring stays locked and the first app that needs a secret asks for a password
//! nobody remembers. This program, run at login, notices that and asks one clear question in a small window: the previous password and the
//! current one. Then it changes the keyring's password to the current one (the call Seahorse uses), or, for someone who forgot the previous
//! one, starts a new keyring and keeps the old file aside.
//!
//!   labyrinth-keyring-sync            (autostart) does nothing unless the login keyring is locked
//!   labyrinth-keyring-sync --check    prints the state: locked / unlocked / no service / no keyring
//!   labyrinth-keyring-sync --cli      for tests: LABYRINTH_KEYRING_OLD and LABYRINTH_KEYRING_CURRENT in the environment, no window and
//!                                     no check of the current password (it only changes the keyring, which the caller could do over D-Bus)

mod auth;
mod secret;
mod ui;

use eframe::egui;
use secret::{Keyring, Status};

fn wait_for_service() -> Result<Keyring, Status> {
    // at login the secret service may still be starting
    let mut last = Status::NoService;
    for _ in 0..20 {
        match Keyring::open() {
            Ok(k) => return Ok(k),
            Err(s) => last = s,
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    Err(last)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--check") {
        let s = match Keyring::open() {
            Ok(k) => format!("{:?}", k.status()),
            Err(s) => format!("{s:?}"),
        };
        println!("{}", s.to_lowercase());
        return Ok(());
    }
    if args.iter().any(|a| a == "--cli") {
        let (old, new) = (std::env::var("LABYRINTH_KEYRING_OLD").unwrap_or_default(), std::env::var("LABYRINTH_KEYRING_CURRENT").unwrap_or_default());
        let kr = match Keyring::open() {
            Ok(k) => k,
            Err(s) => {
                eprintln!("no keyring: {s:?}");
                std::process::exit(2);
            }
        };
        match kr.change_password(&old, &new) {
            Ok(()) => println!("changed; keyring is now {:?}", kr.status()),
            Err(e) => {
                eprintln!("failed: {e}");
                std::process::exit(1);
            }
        }
        return Ok(());
    }
    // the normal, windowed run: only when there is something to fix
    match wait_for_service() {
        Ok(k) if k.status() == Status::Locked => {}
        _ => return Ok(()),
    }
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([460.0, 330.0]).with_resizable(false).with_title("Keyring").with_app_id("labyrinth-keyring-sync"),
        ..Default::default()
    };
    eframe::run_native("Keyring", options, Box::new(|cc| Ok(Box::new(ui::App::new(&cc.egui_ctx)))))
}
