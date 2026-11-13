//! The nested session: yggdrasil-session started with Yggdrasil's own compositor in its headless test mode as the compositor (a
//! stand-in "Hyprland" on the PATH that runs `yggdrasil-compositor --headless`), and stand-ins for everything else (systemctl,
//! systemd-cat, dbus-update-activation-environment, udevadm, fc-match, notify-send), in a runtime folder, home and session bus
//! socket of its own. The compositor runs the session's autostart line, taken from yggdrasil.lua itself, the way Hyprland runs
//! it at start; the test checks the session comes up (the environment reaches the systemd and D-Bus stand-ins with this
//! compositor's display, the session target is started, the compositor answers on its control socket) and ends cleanly (exit
//! 0, the session target stopped, the monitor log closed, no process left), and that a compositor killed under it is started
//! again and the notice about it arrives.
//!
//! Nothing here reaches the desktop: the PATH holds only the stand-ins and a few plain tools, the environment is cleared, and
//! where the kernel allows it (unshare) everything runs in user, mount, process and network namespaces of its own, which also
//! ends whatever is left (YGGDRASIL_TEST_NESTED_PLAIN=1: without them, as in a container where they cannot be had; then
//! every process carrying this run's marker in its environment is looked for afterwards). Needs the compositor: YGGDRASIL_COMPOSITOR, else yggdrasil-compositor beside this test's build (the
//! repository's target folder, built by `cargo build` in packages/compositor); without it the test says so and passes, unless
//! YGGDRASIL_TEST_NESTED_REQUIRED=1.

use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const LAUNCHER: &str = env!("CARGO_BIN_EXE_yggdrasil-session");
const LUA: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/hyprland/yggdrasil.lua");
/// The plain tools the stand-ins use; nothing else of the system is on the PATH.
const TOOLS: &[&str] = &["sh", "cat", "env", "printenv", "grep", "sleep", "kill", "printf", "echo"];

fn compositor() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("YGGDRASIL_COMPOSITOR") {
        return Some(PathBuf::from(p));
    }
    let here = Path::new(LAUNCHER).parent()?;
    let target = here.parent()?;
    [here.to_path_buf(), target.join("debug"), target.join("release")]
        .into_iter()
        .map(|d| d.join("yggdrasil-compositor"))
        .find(|p| p.is_file())
}

/// The session's autostart line as yggdrasil.lua gives it to Hyprland: `hl.exec_cmd("sh -c '...'")`, the one that imports
/// the environment.
fn autostart_line() -> String {
    let lua = std::fs::read_to_string(LUA).expect("yggdrasil.lua");
    let line = lua.lines().find(|l| l.contains("import-environment")).expect("the autostart line in yggdrasil.lua");
    let start = line.find("sh -c '").expect("sh -c '") + "sh -c '".len();
    let end = line.rfind("'\")").expect("'\")");
    line[start..end].to_string()
}

fn write(p: &Path, s: &str, mode: u32) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A stand-in: writes down its name and arguments (one line), then BODY.
fn stand_in(dir: &Path, name: &str, body: &str) {
    write(&dir.join("stubs").join(name), &format!("#!/bin/sh\necho \"{name} $*\" >> \"$CALLS\"\n{body}"), 0o755);
}

/// Whether user, mount, process and network namespaces can be had here (not in every container).
/// YGGDRASIL_TEST_NESTED_PLAIN=1 does without them, as in such a container.
fn namespaces() -> bool {
    std::env::var_os("YGGDRASIL_TEST_NESTED_PLAIN").is_none()
        && Command::new("unshare")
            .args(["-rmpfn", "--mount-proc", "true"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success())
}

struct Session {
    dir: PathBuf,
    code: Option<i32>,
    calls: String,
    output: String,
    events: String,
}

/// Runs the launcher in a world of its own and waits for it (at most a minute).
fn run_session(name: &str, crash_first: bool) -> Option<Session> {
    let Some(comp) = compositor() else {
        assert!(std::env::var_os("YGGDRASIL_TEST_NESTED_REQUIRED").is_none(), "no yggdrasil-compositor (YGGDRASIL_COMPOSITOR)");
        eprintln!("nested session: skipped, no yggdrasil-compositor built (cargo build in packages/compositor, or YGGDRASIL_COMPOSITOR)");
        return None;
    };
    // short: the compositor's socket path must fit in 108 bytes
    let dir = std::env::temp_dir().join(format!("ygg-ns-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for d in ["run", "home", "stubs", "tools"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::set_permissions(dir.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
    for t in TOOLS {
        let real = ["/usr/bin", "/bin"].iter().map(|d| Path::new(d).join(t)).find(|p| p.exists());
        if let Some(real) = real {
            std::os::unix::fs::symlink(real, dir.join("tools").join(t)).unwrap();
        }
    }
    // the session bus: a socket of the test's own, never the desktop's
    let _bus = std::os::unix::net::UnixListener::bind(dir.join("run/bus")).unwrap();

    // "Hyprland": Yggdrasil's compositor, headless, with the autostart as its --run program
    stand_in(
        &dir,
        "Hyprland",
        r#"if [ "$1" = --version ]; then echo "Hyprland 0.0.0-nested (yggdrasil-compositor --headless)"; exit 0; fi
n=$(cat "$T/runs" 2>/dev/null || echo 0); echo $((n + 1)) > "$T/runs"
exec "$YGG_TEST_COMPOSITOR" --headless --size 1280x720 --run sh "$T/autostart"
"#,
    );
    write(&dir.join("autostart-line"), &autostart_line(), 0o644);
    write(
        &dir.join("autostart"),
        r#"echo "autostart on WAYLAND_DISPLAY=$WAYLAND_DISPLAY" >> "$CALLS"
sh "$T/autostart-line"
"$YGG_TEST_COMPOSITOR" ctl outputs >> "$CALLS" 2>&1
if [ "$(cat "$T/runs")" = 1 ] && [ -n "$CRASH_FIRST" ]; then
  echo "killing the compositor" >> "$CALLS"
  kill -KILL $PPID
  exit 0
fi
if [ -n "$CRASH_FIRST" ]; then
  # the notice about the crash comes 10 s after it, while this session runs
  i=0; while [ $i -lt 150 ] && ! grep -q '^notify-send' "$CALLS"; do sleep 0.1; i=$((i + 1)); done
fi
# the sign-out: the compositor ends at once, before it answers
echo "asking the compositor to quit" >> "$CALLS"
"$YGG_TEST_COMPOSITOR" ctl quit >> "$CALLS" 2>&1
"#,
        0o644,
    );
    // import-environment: the values it would hand to systemd
    stand_in(
        &dir,
        "systemctl",
        r#"if [ "$2" = import-environment ]; then
  shift 2
  for v in "$@"; do printf '  %s=%s\n' "$v" "$(printenv "$v")" >> "$CALLS"; done
fi
"#,
    );
    stand_in(&dir, "dbus-update-activation-environment", "echo \"  bus $DBUS_SESSION_BUS_ADDRESS\" >> \"$CALLS\"\n");
    stand_in(&dir, "systemd-cat", "cat >> \"$CALLS\"\n");
    stand_in(&dir, "notify-send", "");
    stand_in(&dir, "fc-match", "printf /usr/share/fonts/TTF/DejaVuSans.ttf\n");
    stand_in(&dir, "udevadm", "echo 'KERNEL[1.0] change /devices/drm/card1 (drm)'\nexec sleep 60\n");
    std::fs::write(dir.join("calls"), "").unwrap();

    let marker = format!("{}-{name}", std::process::id());
    let mut c = if namespaces() {
        // by its full name: the session's PATH holds only the stand-ins
        let unshare = std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
            .map(|d| d.join("unshare"))
            .find(|p| p.is_file())
            .expect("unshare");
        let mut c = Command::new(unshare);
        c.args(["-rmpfn", "--mount-proc", "--"]).arg(LAUNCHER);
        c
    } else {
        eprintln!("nested session: no namespaces here; private folders and a cleared environment only");
        Command::new(LAUNCHER)
    };
    c.env_clear()
        .env("PATH", format!("{}:{}", dir.join("stubs").display(), dir.join("tools").display()))
        .env("HOME", dir.join("home"))
        .env("XDG_RUNTIME_DIR", dir.join("run"))
        .env("LC_ALL", "C")
        .env("CALLS", dir.join("calls"))
        .env("T", &dir)
        .env("YGG_TEST_COMPOSITOR", &comp)
        .env("YGGDRASIL_COMPOSITOR_ETC", dir.join("etc"))
        .env("YGGDRASIL_COMPOSITOR_XWAYLAND", "0")
        .env("YGG_NESTED_TEST", &marker)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if crash_first {
        c.env("CRASH_FIRST", "1");
    }
    let mut child = c.spawn().expect("start yggdrasil-session");
    let deadline = Instant::now() + Duration::from_secs(60);
    let code = loop {
        if let Some(s) = child.try_wait().unwrap() {
            break s.code();
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut early = String::new();
    let _ = child.stderr.take().unwrap().read_to_string(&mut early);
    // whatever is left of this session (without namespaces) after a moment: none may be
    std::thread::sleep(Duration::from_millis(500));
    let left = leftovers(&marker);
    for pid in &left {
        // SAFETY: kill with a pid found by its environment marker
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
    assert!(left.is_empty(), "processes left after the session ended: {left:?}");
    let read = |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_default();
    let s = Session {
        code,
        calls: read("calls"),
        output: format!("{early}{}", read("home/.local/state/yggdrasil/session-output.log")),
        events: read("home/.local/state/yggdrasil/monitor-events.log"),
        dir,
    };
    eprintln!("--- calls\n{}--- output\n{}--- events\n{}", s.calls, s.output, s.events);
    Some(s)
}

/// Processes whose environment carries this run's marker.
fn leftovers(marker: &str) -> Vec<i32> {
    let want = format!("YGG_NESTED_TEST={marker}");
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<i32>().ok())
        .filter(|pid| std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|env| env.split(|&b| b == 0).any(|v| v == want.as_bytes())))
        .collect()
}

/// The lines of `calls` that start with one of PREFIXES, in order.
fn order(calls: &str, prefixes: &[&str]) -> Result<(), String> {
    let mut lines = calls.lines();
    for p in prefixes {
        if !lines.by_ref().any(|l| l.starts_with(p)) {
            return Err(format!("no {p:?} (in this order)"));
        }
    }
    Ok(())
}

#[test]
fn comes_up_and_ends_cleanly() {
    let Some(s) = run_session("up", false) else { return };
    let run = s.dir.join("run");
    let run = run.display();
    assert_eq!(s.code, Some(0), "exit");
    order(
        &s.calls,
        &[
            "fc-match -f %{file} sans-serif",
            "Hyprland --version",
            "systemd-cat -t yggdrasil-session echo starting Hyprland (Hyprland 0.0.0-nested)",
            "Hyprland --config /usr/share/yggdrasil/hyprland/yggdrasil.lua",
            "autostart on WAYLAND_DISPLAY=wayland-",
            "systemctl --user import-environment WAYLAND_DISPLAY",
            "  WAYLAND_DISPLAY=wayland-",
            "  XDG_CURRENT_DESKTOP=Yggdrasil",
            "  XDG_SESSION_TYPE=wayland",
            "  QT_QPA_PLATFORM=wayland;xcb",
            "  QT_WAYLAND_DISABLED_INTERFACES=zxdg_decoration_manager_v1",
            "  QT_QPA_PLATFORMTHEME=xdgdesktopportal",
            &format!("  MANGOHUD_CONFIGFILE={run}/yggdrasil-mangohud.conf"),
            "  GDK_BACKEND=wayland,x11,*",
            "dbus-update-activation-environment --systemd --all",
            &format!("  bus unix:path={run}/bus"),
            "systemctl --user start yggdrasil-session.target",
            "autostart started for Yggdrasil on wayland-",
            "HEADLESS-1",
            "asking the compositor to quit",
            "systemctl --user stop yggdrasil-session.target graphical-session.target",
            "systemd-cat -t yggdrasil-session echo Hyprland ended (exit 0)",
        ],
    )
    .unwrap();
    assert!(s.calls.contains(&format!("bus unix:path={run}/bus")), "the session bus");
    assert!(!s.calls.contains("crashed"), "no crash");
    assert!(s.output.contains("WAYLAND_DISPLAY=wayland-"), "the compositor's output goes to session-output.log");
    let ev: Vec<&str> = s.events.lines().map(|l| l.get(20..).unwrap_or(l)).collect();
    assert_eq!(ev.first(), Some(&"session start"), "{ev:?}");
    assert_eq!(ev.last(), Some(&"session end"), "{ev:?}");
    assert!(std::fs::read_to_string(s.dir.join("run/yggdrasil-mangohud.conf")).unwrap().contains("font_file=/usr/share/fonts"));
    // the compositor took its socket away when it ended
    let sockets: Vec<_> = std::fs::read_dir(s.dir.join("run"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("wayland-") && !n.ends_with(".lock"))
        .collect();
    assert!(sockets.is_empty(), "left in the runtime folder: {sockets:?}");
    let _ = std::fs::remove_dir_all(&s.dir);
}

#[test]
fn a_killed_compositor_is_started_again() {
    let Some(s) = run_session("crash", true) else { return };
    assert_eq!(s.code, Some(0), "exit");
    order(
        &s.calls,
        &[
            "Hyprland --config ",
            "autostart on WAYLAND_DISPLAY=wayland-",
            "systemctl --user start yggdrasil-session.target",
            "killing the compositor",
            "systemd-cat -t yggdrasil-session echo Hyprland crashed (exit 137); starting it again with Yggdrasil's settings.",
            "Hyprland --config ",
            "autostart on WAYLAND_DISPLAY=wayland-",
            "systemctl --user start yggdrasil-session.target",
            "notify-send -a Yggdrasil -i dialog-warning The desktop restarted",
            "asking the compositor to quit",
            "systemctl --user stop yggdrasil-session.target graphical-session.target",
            "systemd-cat -t yggdrasil-session echo Hyprland ended (exit 0)",
        ],
    )
    .unwrap();
    assert!(!s.calls.contains("--safe-mode"), "one crash is no reason for safe mode");
    assert!(s.events.contains("Hyprland ended with exit 137"), "{}", s.events);
    let _ = std::fs::remove_dir_all(&s.dir);
}
