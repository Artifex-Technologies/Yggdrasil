//! yggdrasil-compositor-session: Yggdrasil's session on its own compositor, started by the login screen
//! (yggdrasil-compositor.desktop in /usr/share/wayland-sessions). It is the Hyprland session's start without Hyprland: the
//! compositor drives the screens itself (`--drm`, through logind) and runs `yggdrasil-compositor-session --autostart` as its
//! first program, which does what yggdrasil.lua's autostart line does: the environment handed to systemd and D-Bus, and
//! yggdrasil-session.target, which starts the autostart folder (the panel, the idle watch and lock, the password box, the
//! battery and tablet watches). When the compositor ends, the session's units are stopped.
//!
//! A way back: when the compositor ends badly within its first 15 seconds (it could not take the screens, it crashed at
//! start), the Hyprland session starts instead (/usr/bin/yggdrasil-session), so a broken build never leaves the login screen
//! in a loop. Its log: the user journal, `journalctl --user -t yggdrasil-compositor-session -t yggdrasil-compositor`.
//!
//! For a test from inside another desktop: YGGDRASIL_COMPOSITOR (a stand-in for the compositor), YGGDRASIL_SESSION_FALLBACK (a
//! stand-in for the Hyprland session), YGGDRASIL_SYSTEMCTL (a stand-in for the system's `systemctl reboot|poweroff`),
//! YGGDRASIL_OWN_PANEL (this session's own panel build) and YGGDRASIL_SESSION_TEST=1 (the user manager is left alone: the
//! session's units are not stopped at the end).
//!
//! It replaces the shell script of the same name (packages/compositor/session), with the same steps in the same order.

use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

const ME: &str = "yggdrasil-compositor-session";
const NVIDIA: &str = "/proc/driver/nvidia/version";
/// This session's own panel build, when one is installed (install-session.sh puts it here).
const OWN_PANEL: &str = "/usr/local/lib/yggdrasil-own/yggdrasil-panel";
/// A compositor that ends badly sooner than this never got going: the Hyprland session starts instead.
const EARLY_SECS: u64 = 15;

/// The session's environment, in the order the script exported it.
pub fn environment(nvidia: bool) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![
        // Yggdrasil-Compositor first: the portal reads its routing from yggdrasil-compositor-portals.conf (screen sharing is
        // Yggdrasil's own here, Hyprland's on Hyprland); programs still find Yggdrasil in the list
        ("XDG_CURRENT_DESKTOP", "Yggdrasil-Compositor:Yggdrasil"),
        ("XDG_SESSION_DESKTOP", "yggdrasil"),
        ("XDG_SESSION_TYPE", "wayland"),
        ("QT_QPA_PLATFORM", "wayland;xcb"),
        ("GDK_BACKEND", "wayland,x11"),
        ("MOZ_ENABLE_WAYLAND", "1"),
        ("ELECTRON_OZONE_PLATFORM_HINT", "auto"),
    ];
    // NVIDIA's driver: video decoding through it (VA-API's nvidia driver, its direct backend) and GLX through NVIDIA's library,
    // as the Hyprland session sets them (session.rs)
    if nvidia {
        v.extend([("LIBVA_DRIVER_NAME", "nvidia"), ("__GLX_VENDOR_LIBRARY_NAME", "nvidia"), ("NVD_BACKEND", "direct")]);
    }
    v
}

/// The autostart, once the compositor's sockets exist, in order: each a command. A session that ended without its cleanup
/// (killed, or the login screen restarted) can leave graphical-session.target and yggdrasil-session.target active with their
/// programs gone, and starting an active target starts nothing (no panel: a black screen, YGG-91): so whatever is left is
/// stopped first. A Hyprland session that ran before also leaves its signature behind. With this session's own panel build,
/// the autostart folder's panel (the packaged one, which the Hyprland session keeps) is stopped and the own one runs.
pub fn autostart_steps(nvidia: bool, own_panel: Option<&Path>) -> Vec<Vec<OsString>> {
    let s = |v: &[&str]| v.iter().map(OsString::from).collect::<Vec<_>>();
    let mut import = s(&["systemctl", "--user", "import-environment", "WAYLAND_DISPLAY", "DISPLAY"]);
    import.extend(environment(nvidia).into_iter().map(|(k, _)| OsString::from(k)));
    let mut steps = vec![
        s(&["systemctl", "--user", "stop", "graphical-session.target"]),
        s(&["systemctl", "--user", "reset-failed"]),
        s(&["systemctl", "--user", "unset-environment", "HYPRLAND_INSTANCE_SIGNATURE"]),
        import,
        s(&["dbus-update-activation-environment", "--systemd", "--all"]),
        s(&["systemctl", "--user", "start", "yggdrasil-session.target"]),
    ];
    if let Some(p) = own_panel {
        steps.push(s(&["systemctl", "--user", "stop", "app-yggdrasil*panel@autostart.service"]));
        let mut run = s(&[
            "systemd-run",
            "--user",
            "--unit=yggdrasil-own-panel",
            "--collect",
            "-p",
            "PartOf=graphical-session.target",
            "-p",
            "Restart=on-failure",
        ]);
        run.push(p.as_os_str().to_owned());
        steps.push(run);
    }
    steps
}

/// What comes after the compositor ended: a restart or shut-down asked for from the session, the Hyprland session (it ended
/// badly and early: it never got going; a sign-out, status 0, is never sent there), or nothing.
#[derive(Debug, PartialEq)]
pub enum After {
    Systemctl(&'static str),
    Fallback,
    Nothing,
}

pub fn after(asked: Option<&[u8]>, status: i32, ran_secs: u64, fallback_there: bool) -> After {
    let asked: Option<Vec<u8>> = asked.map(|a| a.iter().copied().filter(|&b| b != 0 && b != b'\n').collect());
    match asked.as_deref() {
        Some(b"reboot") => return After::Systemctl("reboot"),
        Some(b"poweroff") => return After::Systemctl("poweroff"),
        _ => {}
    }
    if status != 0 && ran_secs < EARLY_SECS && fallback_there {
        After::Fallback
    } else {
        After::Nothing
    }
}

fn var(name: &str) -> Option<OsString> {
    std::env::var_os(name).filter(|v| !v.is_empty())
}

/// A line in the user journal under this program's tag (on standard error when there is no journal).
fn say(msg: &str) {
    let ok = Command::new("systemd-cat")
        .args(["-t", ME, "echo", msg])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        eprintln!("{ME}: {msg}");
    }
}

/// The runtime folder (where the session's after-session note is left).
fn runtime() -> PathBuf {
    var("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| {
        // SAFETY: getuid cannot fail
        PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() }))
    })
}

/// `yggdrasil-compositor-session --autostart`: run by the compositor as its first program, with WAYLAND_DISPLAY and DISPLAY
/// set. The sign-in animation first, as Hyprland's session starts it (yggdrasil.lua): yggdrasil-splash covers the screens
/// until the top bar is up or twelve seconds have passed; Settings > Appearance > Boot screen chooses what it shows, or off.
pub fn autostart() -> i32 {
    let _ = Command::new("systemd-cat").args(["-t", "yggdrasil-splash", "yggdrasil-splash"]).stdin(Stdio::null()).spawn();
    let own = var("YGGDRASIL_OWN_PANEL").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(OWN_PANEL));
    let own = is_executable(&own).then_some(own.as_path());
    for step in autostart_steps(Path::new(NVIDIA).exists(), own) {
        let _ = Command::new(&step[0]).args(&step[1..]).stdin(Stdio::null()).status();
    }
    let desktop = var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    let display = var("WAYLAND_DISPLAY").unwrap_or_default();
    say(&format!("autostart started for {} on {}", desktop.to_string_lossy(), display.to_string_lossy()));
    0
}

fn is_executable(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// The session: the compositor with its autostart, then what comes after it.
pub fn main() -> ! {
    if std::env::args().nth(1).as_deref() == Some("--autostart") {
        std::process::exit(autostart());
    }
    let nvidia = Path::new(NVIDIA).exists();
    for (k, v) in environment(nvidia) {
        std::env::set_var(k, v);
    }
    let compositor = var("YGGDRASIL_COMPOSITOR").unwrap_or_else(|| "yggdrasil-compositor".into());
    let fallback = var("YGGDRASIL_SESSION_FALLBACK").unwrap_or_else(|| "/usr/bin/yggdrasil-session".into());
    let test = var("YGGDRASIL_SESSION_TEST").is_some();
    // Restart and Shut down: yggdrasil-logout closes every window, writes "reboot" or "poweroff" here and ends the
    // compositor; this does it once the compositor has let go of the screen (as yggdrasil-session does for Hyprland). A note
    // left from before (a session that never read it) is no order for this one.
    let after_file = runtime().join("yggdrasil-after-session");
    let _ = std::fs::remove_file(&after_file);
    let me = std::env::current_exe().unwrap_or_else(|_| PathBuf::from(ME));

    say(&format!("starting {} --drm", compositor.to_string_lossy()));
    let start = Instant::now();
    // systemd-cat runs the compositor with its output in the journal and returns its exit status
    let status = Command::new("systemd-cat")
        .args(["-t", "yggdrasil-compositor"])
        .arg(&compositor)
        .args(["--drm", "--run"])
        .arg(&me)
        .arg("--autostart")
        .status()
        .map(crate::run::code)
        .unwrap_or(127);
    let ran = start.elapsed().as_secs();
    say(&format!("the compositor ended after {ran} s (status {status})"));

    // the session's programs end with it (as yggdrasil-session does for Hyprland)
    if !test {
        let _ = Command::new("systemctl")
            .args(["--user", "stop", "yggdrasil-own-panel.service", "yggdrasil-session.target", "graphical-session.target"])
            .stderr(Stdio::null())
            .status();
        let _ =
            Command::new("systemctl").args(["--user", "unset-environment", "WAYLAND_DISPLAY", "DISPLAY"]).stderr(Stdio::null()).status();
    }

    let asked = std::fs::read(&after_file).ok();
    if asked.is_some() {
        let _ = std::fs::remove_file(&after_file);
    }
    match after(asked.as_deref(), status, ran, is_executable(Path::new(&fallback))) {
        After::Systemctl(what) => {
            say(&format!("{what}, now that the session has ended"));
            let systemctl = var("YGGDRASIL_SYSTEMCTL").unwrap_or_else(|| "systemctl".into());
            let _ = Command::new(systemctl).arg(what).status();
            std::process::exit(0)
        }
        After::Fallback => {
            say(&format!("it ended badly within {ran} s: starting the Hyprland session instead"));
            let e = Command::new(&fallback).exec();
            say(&format!("the Hyprland session could not start: {e}"));
            std::process::exit(1)
        }
        After::Nothing => std::process::exit(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_environment_is_the_scripts() {
        let e = environment(false);
        assert_eq!(e[0], ("XDG_CURRENT_DESKTOP", "Yggdrasil-Compositor:Yggdrasil"));
        assert!(e.iter().all(|(k, _)| !k.contains("NVD")));
        let n = environment(true);
        assert_eq!(n.len(), e.len() + 3);
        assert!(n.contains(&("LIBVA_DRIVER_NAME", "nvidia")));
    }

    #[test]
    fn the_autostart_stops_what_is_left_first_and_hands_the_environment_on() {
        let steps = autostart_steps(true, None);
        let words: Vec<String> = steps.iter().map(|s| s.iter().map(|w| w.to_string_lossy()).collect::<Vec<_>>().join(" ")).collect();
        assert_eq!(words[0], "systemctl --user stop graphical-session.target");
        assert_eq!(words[1], "systemctl --user reset-failed");
        assert_eq!(words[2], "systemctl --user unset-environment HYPRLAND_INSTANCE_SIGNATURE");
        assert!(words[3].starts_with("systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP "), "{}", words[3]);
        assert!(words[3].ends_with(" NVD_BACKEND"), "the NVIDIA variables too: {}", words[3]);
        assert_eq!(words[4], "dbus-update-activation-environment --systemd --all");
        assert_eq!(words[5], "systemctl --user start yggdrasil-session.target");
        assert_eq!(words.len(), 6, "no panel swap without the own panel");
        let with_panel = autostart_steps(false, Some(Path::new("/opt/panel")));
        let last: Vec<String> = with_panel[with_panel.len() - 1].iter().map(|w| w.to_string_lossy().into_owned()).collect();
        assert_eq!(last.first().map(String::as_str), Some("systemd-run"));
        assert_eq!(last.last().map(String::as_str), Some("/opt/panel"));
        assert!(!with_panel[3].iter().any(|w| w == "NVD_BACKEND"));
    }

    #[test]
    fn what_comes_after_the_compositor() {
        assert_eq!(after(Some(b"reboot\n"), 0, 300, true), After::Systemctl("reboot"));
        assert_eq!(after(Some(b"poweroff\0"), 1, 2, true), After::Systemctl("poweroff"), "asked for wins over a bad end");
        assert_eq!(after(Some(b"rm -rf /"), 0, 300, true), After::Nothing, "anything else is no order");
        assert_eq!(after(None, 1, 3, true), After::Fallback, "a bad end in its first seconds");
        assert_eq!(after(None, 1, 3, false), After::Nothing, "no Hyprland session to go to");
        assert_eq!(after(None, 1, 600, true), After::Nothing, "a bad end later is no failed start");
        assert_eq!(after(None, 0, 1, true), After::Nothing, "a quick sign-out is a sign-out");
    }
}
