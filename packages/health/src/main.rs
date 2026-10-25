//! Health: break reminders. `daemon` is the user service that sends the reminders and keeps the top bar's file current; the
//! settings are on the bar's card, whose controls run `yggdrasil-health press ID`.

use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use yggdrasil_health::*;

const ICON: &str = "/usr/share/yggdrasil/icons/health-symbolic.svg";
const SOUND: &str = "/usr/share/sounds/freedesktop/stereo/message.oga";

/// The session is locked or idle: the screensaver interface (Yggdrasil's lock and most desktops) or logind's hints.
fn away() -> bool {
    let ss = Command::new("busctl")
        .args(["--user", "call", "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver", "GetActive"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().ends_with("true"))
        .unwrap_or(false);
    if ss {
        return true;
    }
    let Ok(id) = std::env::var("XDG_SESSION_ID") else { return false };
    Command::new("loginctl")
        .args(["show-session", &id, "-p", "IdleHint", "-p", "LockedHint", "--value"])
        .stderr(Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().any(|l| l.trim() == "yes"))
        .unwrap_or(false)
}

/// Sends one reminder: a quiet notification (never waiting on it), and a soft sound if that is on.
fn send(name: &str, sound: bool, seed: i64) {
    let msgs = messages(name);
    let Some((head, body)) = msgs.get(seed.rem_euclid(msgs.len().max(1) as i64) as usize) else { return };
    let _ = Command::new("notify-send").args(["-a", "Health", "-u", "low", "-i", ICON, head, body]).stdin(Stdio::null()).spawn().map(|mut c| {
        std::thread::spawn(move || c.wait());
    });
    if sound && std::path::Path::new(SOUND).is_file() {
        let _ = Command::new("paplay").arg(SOUND).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|mut c| {
            std::thread::spawn(move || c.wait());
        });
    }
}

/// The service: wakes when the next reminder is due (at least once a minute, to pick up changed settings and a return from
/// idle), sends what is due and rewrites the bar's file when it changed.
fn daemon() -> ExitCode {
    loop {
        let cfg = load_config();
        let mut st = load_state();
        let before = st.clone();
        let clock = Clock::now();
        for name in tick(&cfg, &mut st, &clock, &mut away) {
            send(&name, cfg.sound, clock.epoch / 60);
        }
        if st != before {
            let _ = save_state(&st);
        }
        write_bar(&cfg, &st, &clock);
        let wait = next_due(&cfg, &st, &clock).map(|(_, t)| t - clock.epoch).unwrap_or(60).clamp(1, 60);
        std::thread::sleep(Duration::from_secs(wait as u64));
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: yggdrasil-health daemon | press ID | summary | fire STREAM | pause MINUTES|tomorrow | resume");
    ExitCode::from(2)
}

/// Saves the settings and updates the bar at once (the service follows within a minute).
fn store(cfg: &Config) {
    let _ = save_config(cfg);
    write_bar(cfg, &load_state(), &Clock::now());
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("daemon") => daemon(),
        // a control on the bar's card (docs/INTEGRATIONS.md): the bar reads its file again at once
        Some("press") => {
            let mut cfg = load_config();
            if !args.get(1).is_some_and(|id| press(&mut cfg, id, &Clock::now())) {
                return usage();
            }
            store(&cfg);
            ExitCode::SUCCESS
        }
        Some("summary") => {
            let (cfg, st, c) = (load_config(), load_state(), Clock::now());
            let next = next_due(&cfg, &st, &c).map(|(n, t)| serde_json::json!({"stream": n, "secs": t - c.epoch}));
            println!("{}", serde_json::json!({"config": cfg, "next": next}));
            ExitCode::SUCCESS
        }
        Some("fire") => match args.get(1).filter(|n| STREAMS.contains(&n.as_str())) {
            Some(n) => {
                send(n, load_config().sound, Clock::now().epoch);
                // let the notification start before exiting
                std::thread::sleep(Duration::from_millis(300));
                ExitCode::SUCCESS
            }
            None => usage(),
        },
        Some("pause") => {
            let mut cfg = load_config();
            if !args.get(1).is_some_and(|h| pause(&mut cfg, h, &Clock::now())) {
                return usage();
            }
            store(&cfg);
            ExitCode::SUCCESS
        }
        Some("resume") => {
            let mut cfg = load_config();
            cfg.paused_until = None;
            store(&cfg);
            ExitCode::SUCCESS
        }
        _ => usage(),
    }
}
