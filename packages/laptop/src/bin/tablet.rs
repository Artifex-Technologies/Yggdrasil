//! /usr/lib/yggdrasil/tablet: tablet mode and screen rotation on a 2-in-1 (laptop::tablet).
//!
//!   tablet watch            the session's watcher (autostart); says "off" and stops on a computer without an accelerometer
//!                           or a tablet-mode switch
//!   tablet mode on|off      the switch (run by the session's Hyprland binds)
//!   tablet lock on|off      Settings > Displays > Lock rotation
//!   tablet keyboard on|off  the on-screen keyboard in tablet mode
//!   tablet json             what Settings shows

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use laptop::sysfs;
use laptop::tablet::{self, Accel, Conf, Osk, State, Watcher};

fn runtime() -> Result<PathBuf, String> {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).filter(|p| p.is_absolute()).ok_or_else(|| "XDG_RUNTIME_DIR is not set".into())
}

fn on_off(arg: Option<&String>) -> Result<bool, String> {
    match arg.map(String::as_str) {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err("on or off".into()),
    }
}

fn watch() -> Result<(), String> {
    let root = Path::new("/");
    let (accel_here, switch) = (sysfs::accelerometer(root), sysfs::tablet_switch(root));
    if !accel_here && !switch {
        println!("off: no accelerometer and no tablet-mode switch");
        return Ok(());
    }
    let run = runtime()?;
    let (tx, rx) = mpsc::channel();
    tablet::listen(&tablet::socket_path(&run), tx.clone()).map_err(|e| e.to_string())?;
    let hypr = match yggdrasil_wm::Desk::detect() {
        Some(yggdrasil_wm::Desk::Hyprland(h)) => {
            tablet::watch_hyprland(&h, tx.clone());
            Some(h)
        }
        Some(yggdrasil_wm::Desk::Own(_)) => {
            eprintln!("tablet: Yggdrasil's own compositor cannot turn a screen yet; the on-screen keyboard only");
            None
        }
        None => None,
    };
    let accel =
        if accel_here { zbus::blocking::Connection::system().ok().and_then(|c| Accel::new(&c).ok()).filter(|a| a.present()) } else { None };
    if let Some(a) = &accel {
        a.watch(tx.clone());
    }
    tablet::watch_inputs(Path::new("/dev/input"), tx.clone());
    let conf_path = Conf::path();
    let state = State {
        has_switch: switch,
        tablet: sysfs::thinkpad_tablet_mode(root).unwrap_or(false),
        orientation: None,
        conf: Conf::load(&conf_path),
        plugged_keyboard: sysfs::plugged_keyboard(root),
        rotation: 0,
    };
    let mut w = Watcher::new(state, hypr, accel, Osk::new(vec![tablet::OSK.into()]), root.to_path_buf(), conf_path);
    drop(tx);
    loop {
        w.step();
        let Ok(e) = rx.recv() else { return Ok(()) };
        w.handle(e);
        // a burst (the accelerometer settling, several devices at once) is acted on once
        while let Ok(e) = rx.try_recv() {
            w.handle(e);
        }
    }
}

fn set_conf(f: impl FnOnce(&mut Conf)) -> Result<(), String> {
    let p = Conf::path();
    let mut c = Conf::load(&p);
    f(&mut c);
    c.save(&p)?;
    tablet::tell(&runtime()?, "conf").map(|_| ())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let r = match args.first().map(String::as_str) {
        Some("watch") => watch(),
        Some("mode") => {
            on_off(args.get(1)).and_then(|on| runtime().and_then(|r| tablet::tell(&r, if on { "mode on" } else { "mode off" })).map(|_| ()))
        }
        Some("lock") => on_off(args.get(1)).and_then(|on| set_conf(|c| c.rotation_lock = on)),
        Some("keyboard") => on_off(args.get(1)).and_then(|on| set_conf(|c| c.keyboard = on)),
        Some("json") => {
            let root = Path::new("/");
            let c = Conf::load(&Conf::path());
            println!(
                "{}",
                serde_json::json!({
                    "accelerometer": sysfs::accelerometer(root),
                    "tablet_switch": sysfs::tablet_switch(root),
                    "lock": c.rotation_lock,
                    "keyboard": c.keyboard,
                    "keyboard_installed": Path::new(tablet::OSK).exists(),
                })
            );
            Ok(())
        }
        _ => Err("usage: tablet watch | mode on|off | lock on|off | keyboard on|off | json".into()),
    };
    if let Err(e) = r {
        eprintln!("tablet: {e}");
        std::process::exit(1);
    }
}
