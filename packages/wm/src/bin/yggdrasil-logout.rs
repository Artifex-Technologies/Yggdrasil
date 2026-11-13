//! yggdrasil-logout logout|reboot|poweroff: ends Yggdrasil's session the careful way (the panel runs it when its countdown ends).
//! Every window is asked to close first, so programs can save; after up to 10 seconds, if all have closed, the session ends (or
//! the computer restarts or shuts down, once the compositor has ended). If one has not (it may be asking whether to save),
//! nothing ends: the screen locks, so a computer left alone is not left open, and a notification says which program is still
//! there. On whichever compositor runs (yggdrasil-wm).

use std::process::{Command, Stdio};
use std::time::Duration;

use yggdrasil_wm::{Desk, Window};

/// The programs still open, for the notification: the first three titles (else the class), "and others" after that.
fn names(windows: &[Window]) -> String {
    let mut seen: Vec<String> = vec![];
    for w in windows {
        let n = [w.title.as_str(), w.class.as_str()].into_iter().find(|s| !s.is_empty()).unwrap_or("a program").to_string();
        if !seen.contains(&n) {
            seen.push(n);
        }
    }
    if seen.is_empty() {
        return String::new();
    }
    let mut s = seen.iter().take(3).cloned().collect::<Vec<_>>().join(", ");
    if seen.len() > 3 {
        s.push_str(" and others");
    }
    s
}

/// A program of the system, run for its effect (its output is not read).
fn run(cmd: &str, args: &[&str]) -> bool {
    Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success())
}

fn runtime_dir() -> std::path::PathBuf {
    use std::os::unix::fs::MetadataExt;
    std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()).map(std::path::PathBuf::from).unwrap_or_else(|| {
        let uid = std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0);
        std::path::PathBuf::from(format!("/run/user/{uid}"))
    })
}

fn main() {
    let what = std::env::args().nth(1).unwrap_or_else(|| "logout".into());
    if !["logout", "reboot", "poweroff"].contains(&what.as_str()) {
        eprintln!("usage: yggdrasil-logout logout|reboot|poweroff");
        std::process::exit(2);
    }
    let desk = Desk::detect();
    let windows = || desk.as_ref().map(|d| d.windows()).unwrap_or_default();
    if let Some(d) = &desk {
        for w in windows() {
            d.ask_to_close(&w.id);
        }
    }
    for _ in 0..40 {
        if windows().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let left = names(&windows());
    if !left.is_empty() {
        run("systemd-cat", &["-t", "yggdrasil-session", "echo", &format!("{what} stopped: still open: {left}")]);
        run(
            "notify-send",
            &[
                "-a",
                "Desktop",
                "-u",
                "critical",
                "-i",
                "dialog-warning",
                "Still signed in",
                &format!(
                    "{left} did not close (it may be asking whether to save). The screen was locked instead; close it, then try again."
                ),
            ],
        );
        run("loginctl", &["lock-session"]);
        std::process::exit(1);
    }
    run("systemd-cat", &["-t", "yggdrasil-session", "echo", &format!("{what}: every window closed")]);
    // the compositor ends first, cleanly; yggdrasil-session then restarts or shuts down (it reads what to do from this file)
    if what != "logout" {
        let _ = std::fs::write(runtime_dir().join("yggdrasil-after-session"), format!("{what}\n"));
    }
    // Hyprland's screen-sharing portal crashes when Hyprland goes away under it: it stops first
    if !matches!(desk, Some(Desk::Own(_))) {
        run("systemctl", &["--user", "stop", "xdg-desktop-portal-hyprland.service"]);
    }
    std::process::exit(if desk.is_some_and(|d| d.exit()) { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_programs_still_open_are_named() {
        let w = |title: &str, class: &str| {
            let mut w = yggdrasil_wm::hyprland::parse_clients(r#"[{"address":"0x1"}]"#).remove(0);
            w.title = title.into();
            w.class = class.into();
            w
        };
        assert_eq!(names(&[]), "");
        assert_eq!(
            names(&[w("notes.txt", "editor"), w("", "player"), w("", ""), w("notes.txt", "editor")]),
            "notes.txt, player, a program"
        );
        assert_eq!(names(&[w("a", ""), w("b", ""), w("c", ""), w("d", "")]), "a, b, c and others");
    }
}
