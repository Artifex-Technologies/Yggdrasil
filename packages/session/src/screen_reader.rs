//! yggdrasil-screen-reader: Super+Alt+S starts the Orca screen reader, or stops it when it runs. Orca, Speech Dispatcher and
//! eSpeak NG come with the accessibility component (Settings > Apps > Optional software). docs/ACCESSIBILITY.md.
//!
//! As the script: a running Orca is a process named exactly "orca" (what `pgrep -x orca` matched; read from /proc here) and is
//! sent SIGTERM (`pkill -x orca`); a new one is started in a session of its own (`setsid -f orca --replace`), its output to
//! /dev/null. The exit code is the notification's (notify-send's), 1 when notify-send is not installed, and 1 when there is no
//! Orca to start.

use crate::run;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

const ME: &str = "yggdrasil-screen-reader";

pub fn main() {
    let running = orca_processes();
    let code = if !running.is_empty() {
        for pid in running {
            // SAFETY: kill with a pid read from /proc and a fixed signal
            unsafe { libc::kill(pid, libc::SIGTERM) };
        }
        say("Screen reader off", "")
    } else if let Some(orca) = run::which("orca") {
        let mut c = Command::new(orca);
        c.arg("--replace").stdout(Stdio::null()).stderr(Stdio::null());
        // SAFETY: setsid is async-signal-safe; it only fails when the child already leads a process group, which a new child
        // never does
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let _ = c.spawn();
        say("Screen reader on", "Super+Alt+S turns it off")
    } else {
        say("No screen reader installed", "Settings > Apps > Optional software: Accessibility (Orca)");
        1
    };
    std::process::exit(code)
}

/// notify-send -a Accessibility TITLE BODY when it is installed (its exit code), else 1.
fn say(title: &str, body: &str) -> i32 {
    if run::which("notify-send").is_none() {
        return 1;
    }
    run::status(ME, Command::new("notify-send").args(["-a", "Accessibility", title, body]))
}

/// The processes named exactly "orca" (their comm, as pgrep -x reads it), this one never among them.
pub fn orca_processes() -> Vec<i32> {
    let me = std::process::id() as i32;
    let Ok(d) = std::fs::read_dir("/proc") else { return vec![] };
    d.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&pid| pid != me && std::fs::read(format!("/proc/{pid}/comm")).is_ok_and(|c| c == b"orca\n"))
        .collect()
}
