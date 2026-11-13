//! The nested session: yggdrasil-session started with Yggdrasil's own compositor in its headless test mode as the compositor (a
//! stand-in "Hyprland" on the PATH that runs `yggdrasil-compositor --headless`), and stand-ins for everything else (systemctl,
//! systemd-cat, dbus-update-activation-environment, udevadm, fc-match, notify-send), in a runtime folder, home and session bus
//! socket of its own. The compositor runs the session's autostart line, taken from yggdrasil.lua itself, the way Hyprland runs
//! it at start; the test checks the session comes up (the environment reaches the systemd and D-Bus stand-ins with this
//! compositor's display, the session target is started, the compositor answers on its control socket) and ends cleanly (exit
//! 0, the session target stopped, the monitor log closed, no process left), and that a compositor killed under it is started
//! again and the notice about it arrives.
//!
//! And with systemd itself (a_crash_brings_the_session_programs_back): a user manager of the test's own (its own runtime folder,
//! only the units given, only the XDG autostart generator, in a scope of its own that is stopped with all it ran) as the one the
//! session's real `systemctl --user` talks to, with stand-ins for the panel and idle watch (in the autostart folder, so systemd
//! makes their units as it does the real ones, the panel's drop-in from packages/panel applied), the clipboard and two portals.
//! The panel ending with a stray child must end its unit; after the compositor is killed every one of them must come back on the
//! next one, also those that ended with exit 0 and the portal that kept running. Skipped where no such manager can be had (a
//! container without systemd), unless YGGDRASIL_TEST_SYSTEMD_REQUIRED=1.
//!
//! Nothing here reaches the desktop (but for the systemd test's one request to the desktop's user manager: its scope): the PATH holds only the stand-ins and a few plain tools, the environment is cleared, and
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

/// A world of the test's own in a temporary folder: a runtime folder, home and PATH of stand-ins (nothing of the system but
/// the plain TOOLS), the compositor as "Hyprland" running the autostart script, which runs yggdrasil.lua's own autostart line.
fn world(name: &str) -> PathBuf {
    // short: the compositor's socket path must fit in 108 bytes
    let dir = std::env::temp_dir().join(format!("ygg-ns-{name}-{}", std::process::id()));
    remove(&dir);
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
    dir
}

/// Runs the launcher in DIR's world (in namespaces of its own when NAMESPACES and they can be had) and waits for it (at most a
/// minute): its exit code and what it wrote to standard error before its output went to its log.
fn launch(dir: &Path, comp: &Path, marker: &str, namespaces_too: bool, env: &[(&str, &str)]) -> (Option<i32>, String) {
    let mut c = if namespaces_too && namespaces() {
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
        .env("T", dir)
        .env("YGG_TEST_COMPOSITOR", comp)
        .env("YGGDRASIL_COMPOSITOR_ETC", dir.join("etc"))
        .env("YGGDRASIL_COMPOSITOR_XWAYLAND", "0")
        .env("YGG_NESTED_TEST", marker)
        .envs(env.iter().copied())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
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
    (code, early)
}

/// Whatever is left of this session (without namespaces) after a moment: none may be.
fn no_leftovers(marker: &str) {
    std::thread::sleep(Duration::from_millis(500));
    let left = leftovers(marker);
    for pid in &left {
        // SAFETY: kill with a pid found by its environment marker
        unsafe { libc::kill(*pid, libc::SIGKILL) };
    }
    assert!(left.is_empty(), "processes left after the session ended: {left:?}");
}

fn finish(dir: PathBuf, code: Option<i32>, early: String) -> Session {
    let read = |p: &str| std::fs::read_to_string(dir.join(p)).unwrap_or_default();
    let s = Session {
        code,
        calls: read("calls"),
        output: format!("{early}{}", read("home/.local/state/yggdrasil/session-output.log")),
        events: read("home/.local/state/yggdrasil/monitor-events.log"),
        dir,
    };
    eprintln!("--- calls\n{}--- output\n{}--- events\n{}", s.calls, s.output, s.events);
    s
}

/// A folder removed, also one a systemd manager left its unreadable files in.
fn remove(dir: &Path) {
    if std::fs::remove_dir_all(dir).is_err() && dir.exists() {
        let _ = Command::new("chmod").args(["-R", "u+rwx"]).arg(dir).status();
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// Runs the launcher in a world of its own and waits for it (at most a minute).
fn run_session(name: &str, crash_first: bool) -> Option<Session> {
    let Some(comp) = compositor() else {
        assert!(std::env::var_os("YGGDRASIL_TEST_NESTED_REQUIRED").is_none(), "no yggdrasil-compositor (YGGDRASIL_COMPOSITOR)");
        eprintln!("nested session: skipped, no yggdrasil-compositor built (cargo build in packages/compositor, or YGGDRASIL_COMPOSITOR)");
        return None;
    };
    let dir = world(name);
    // the session bus: a socket of the test's own, never the desktop's
    let _bus = std::os::unix::net::UnixListener::bind(dir.join("run/bus")).unwrap();
    let marker = format!("{}-{name}", std::process::id());
    let (code, early) = launch(&dir, &comp, &marker, true, if crash_first { &[("CRASH_FIRST", "1")] } else { &[] });
    no_leftovers(&marker);
    Some(finish(dir, code, early))
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
    remove(&s.dir);
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
            // the session's programs stopped, so the next autostart starts them again (an empty desktop otherwise)
            "systemctl --user stop yggdrasil-session.target graphical-session.target",
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
    remove(&s.dir);
}

// ---------------------------------------------------------------- the session's programs after a crash, with systemd itself

/// The standard user units the session's targets need, from the system's systemd.
const SYSTEMD_UNITS: &[&str] = &[
    "basic.target",
    "default.target",
    "exit.target",
    "shutdown.target",
    "sockets.target",
    "paths.target",
    "timers.target",
    "graphical-session.target",
    "graphical-session-pre.target",
    "xdg-desktop-autostart.target",
    "app.slice",
    "app-graphical.slice",
    "background.slice",
    "session.slice",
    "systemd-exit.service",
];
const SYSTEMD_UNIT_DIR: &str = "/usr/lib/systemd/user";
const SYSTEMD: &str = "/usr/lib/systemd/systemd";
const XDG_AUTOSTART_GENERATOR: &str = "/usr/lib/systemd/user-generators/systemd-xdg-autostart-generator";
const SESSION_TARGET: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/systemd/yggdrasil-session.target");
const PANEL_DROP_IN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../panel/yggdrasil-panel-autostart.conf");
/// The session's programs the stand-ins play: the panel and the idle watch (the autostart folder), the clipboard (a unit of the
/// graphical session), two portals (started on demand, as D-Bus does): one that ends with its display, one that stays.
const PROGRAMS: &[&str] = &["panel", "idle", "clipboard", "portal-gtk", "portal-stays"];

/// A systemd user manager of the test's own: its own runtime folder (so `systemctl --user` there reaches it and nothing else),
/// only the units given here (SYSTEMD_UNIT_PATH) and only the XDG autostart generator, reading only the test's autostart folder,
/// in a scope of its own (Delegate=yes, 1 GB at most) that is stopped, with everything in it, when this is dropped. The desktop's
/// own user manager only makes that scope.
struct Manager {
    scope: String,
    runtime: PathBuf,
}

impl Manager {
    /// None (the test is skipped) where no such manager can be had: no systemd, no user manager to make the scope (a container).
    fn start(dir: &Path, marker: &str) -> Option<Manager> {
        let required = std::env::var_os("YGGDRASIL_TEST_SYSTEMD_REQUIRED").is_some();
        let skip = |why: &str| {
            assert!(!required, "no private systemd user manager: {why}");
            eprintln!("session programs test: skipped, {why}");
            None
        };
        let missing: Vec<String> = SYSTEMD_UNITS
            .iter()
            .map(|u| Path::new(SYSTEMD_UNIT_DIR).join(u))
            .chain([PathBuf::from(SYSTEMD), PathBuf::from(XDG_AUTOSTART_GENERATOR)])
            .filter(|p| !p.exists())
            .map(|p| p.display().to_string())
            .collect();
        if !missing.is_empty() {
            return skip(&format!("not here: {missing:?}"));
        }
        let units = dir.join("units");
        let gen = dir.join("gen");
        std::fs::create_dir_all(&units).unwrap();
        std::fs::create_dir_all(dir.join("envgen")).unwrap();
        std::fs::create_dir_all(&gen).unwrap();
        for u in SYSTEMD_UNITS {
            std::fs::copy(Path::new(SYSTEMD_UNIT_DIR).join(u), units.join(u)).unwrap();
        }
        std::os::unix::fs::symlink(XDG_AUTOSTART_GENERATOR, gen.join("systemd-xdg-autostart-generator")).unwrap();
        let runtime = dir.join("run");
        let scope = format!("ygg-test-session-{}.scope", std::process::id());
        let log = std::fs::File::create(dir.join("manager.log")).unwrap();
        let mut c = Command::new("systemd-run");
        c.args(["--user", "--scope", "--quiet", "--unit", &scope, "-p", "Delegate=yes", "-p", "MemoryMax=1G", "-p", "MemorySwapMax=0"])
            .args(["--", "/usr/bin/env", "-i"])
            .arg("PATH=/usr/bin")
            .arg(format!("HOME={}", dir.join("home").display()))
            .arg(format!("XDG_RUNTIME_DIR={}", runtime.display()))
            .arg(format!("XDG_CONFIG_DIRS={}", dir.join("xdg").display()))
            .arg(format!("XDG_CONFIG_HOME={}", dir.join("home/.config").display()))
            .arg(format!("XDG_DATA_HOME={}", dir.join("home/.local/share").display()))
            .arg(format!("SYSTEMD_UNIT_PATH={}:{}", units.display(), runtime.join("systemd/generator.late").display()))
            .arg(format!("SYSTEMD_GENERATOR_PATH={}", gen.display()))
            .arg(format!("SYSTEMD_ENVIRONMENT_GENERATOR_PATH={}", dir.join("envgen").display()))
            .arg(format!("YGG_NESTED_TEST={marker}"))
            .arg(SYSTEMD)
            .args(["--user", "--unit=basic.target", "--log-target=console", "--log-level=notice"])
            .args(["--default-standard-output=null", "--default-standard-error=null"])
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        let Ok(mut child) = c.spawn() else { return skip("no systemd-run") };
        let m = Manager { scope, runtime };
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if m.systemctl(&["show", "-P", "Version"]).0 {
                // systemd-run is the manager itself now (exec'd in the scope); it is reaped when it ends
                std::thread::spawn(move || child.wait());
                return Some(m);
            }
            if let Ok(Some(s)) = child.try_wait() {
                drop(m);
                let log = std::fs::read_to_string(dir.join("manager.log")).unwrap_or_default();
                return skip(&format!("the manager did not start ({s}): {log}"));
            }
            if Instant::now() > deadline {
                drop(m);
                return skip("the manager did not answer in 15 s");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// `systemctl --user ARGS` against this manager only: whether it succeeded, and its output.
    fn systemctl(&self, args: &[&str]) -> (bool, String) {
        let out = Command::new("/usr/bin/systemctl")
            .env_clear()
            .env("PATH", "/usr/bin")
            .env("XDG_RUNTIME_DIR", &self.runtime)
            .arg("--user")
            .args(args)
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output();
        match out {
            Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).into_owned()),
            Err(_) => (false, String::new()),
        }
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        // the test's scope, by its own name, on the desktop's user manager: stopping it ends the private manager and all it ran
        let _ = Command::new("systemctl").args(["--user", "stop", &self.scope]).stderr(Stdio::null()).status();
    }
}

/// One of the session's programs, played by this test's own binary (started by the private manager with YGG_STAND_IN set):
/// connects to the compositor on $WAYLAND_DISPLAY, notes "KIND up in run N", and when the display goes, ends as that program
/// does: the panel and the clipboard quietly (exit 0, the "Broken pipe" of 2026-11-13), the idle watch and the GTK portal with
/// an error, the other portal not at all. The panel also starts a child that is not tied to it (as its `pactl subscribe` was)
/// and ends at once, exit 0, on SIGUSR1. Without YGG_STAND_IN this test does nothing.
#[test]
fn stand_in_program() {
    use std::io::Write;
    let Ok(kind) = std::env::var("YGG_STAND_IN") else { return };
    let t = PathBuf::from(std::env::var_os("T").expect("T"));
    let note = |s: &str| {
        let mut f = std::fs::OpenOptions::new().append(true).open(t.join("calls")).expect("calls");
        let _ = f.write_all(format!("{s}\n").as_bytes());
    };
    let run = std::fs::read_to_string(t.join("runs")).unwrap_or_default().trim().to_string();
    let display = std::env::var("WAYLAND_DISPLAY").unwrap_or_default();
    let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap_or_default()).join(&display);
    let mut conn = match std::os::unix::net::UnixStream::connect(&socket) {
        Ok(c) => c,
        Err(e) => {
            note(&format!("{kind} cannot connect to {display:?} in run {run}: {e}"));
            std::process::exit(1)
        }
    };
    if kind == "panel" {
        extern "C" fn quit(_: libc::c_int) {
            // SAFETY: _exit is async-signal-safe
            unsafe { libc::_exit(0) }
        }
        // SAFETY: a handler that only calls _exit
        unsafe { libc::signal(libc::SIGUSR1, quit as *const () as libc::sighandler_t) };
        // left to run on purpose: the stray child the panel's unit must not count as the panel
        #[allow(clippy::zombie_processes)]
        let child = Command::new("sleep").arg("600").stdin(Stdio::null()).spawn().expect("sleep");
        std::fs::write(t.join(format!("panel-child.{run}")), child.id().to_string()).unwrap();
    }
    note(&format!("{kind} up in run {run}"));
    let mut b = [0u8; 256];
    while matches!(conn.read(&mut b), Ok(n) if n > 0) {}
    note(&format!("{kind} lost its display in run {run}"));
    match kind.as_str() {
        "panel" | "clipboard" => std::process::exit(0),
        "portal-stays" => loop {
            std::thread::sleep(Duration::from_secs(3600));
        },
        _ => std::process::exit(1),
    }
}

/// The autostart of this test, run by each compositor: yggdrasil.lua's own line (the environment to the private manager, the
/// session target started), the portals asked for, then a wait until every program is up on this compositor. In the first run
/// the panel then ends on its own (its stray child still there) and the unit's state is noted; then the compositor is killed.
/// In the second the crash notice is awaited and the session signed out of.
const SYSTEMD_AUTOSTART: &str = r#"r=$(cat "$T/runs")
echo "autostart on WAYLAND_DISPLAY=$WAYLAND_DISPLAY (run $r)" >> "$CALLS"
sh "$T/autostart-line"
# the portals start when a program first asks for one, through D-Bus: here as such a request would
systemctl --user start test-portal-gtk.service test-portal-stays.service
all_up() {
  i=0
  while [ $i -lt 200 ]; do
    n=0
    for k in $PROGRAMS; do grep -qx "$k up in run $r" "$CALLS" && n=$((n + 1)); done
    [ $n = $COUNT ] && return 0
    sleep 0.1; i=$((i + 1))
  done
  return 1
}
if all_up; then echo "all up in run $r" >> "$CALLS"; else echo "not all up in run $r" >> "$CALLS"; fi
if [ "$r" = 1 ]; then
  panel='app-yggdrasil\x2dpanel@autostart.service'
  systemctl --user kill --kill-whom=main -s USR1 "$panel"
  i=0; while [ $i -lt 50 ] && [ "$(systemctl --user is-active "$panel")" = active ]; do sleep 0.1; i=$((i + 1)); done
  echo "the panel's unit after the panel ended: $(systemctl --user is-active "$panel")" >> "$CALLS"
  if kill -0 "$(cat "$T/panel-child.1")" 2>/dev/null; then echo "the panel's child: still there" >> "$CALLS"; else echo "the panel's child: gone" >> "$CALLS"; fi
  systemctl --user start "$panel"
  i=0; while [ $i -lt 50 ] && [ "$(grep -cx "panel up in run 1" "$CALLS")" != 2 ]; do sleep 0.1; i=$((i + 1)); done
  echo "killing the compositor" >> "$CALLS"
  kill -KILL $PPID
  exit 0
fi
i=0; while [ $i -lt 150 ] && ! grep -q '^notify-send' "$CALLS"; do sleep 0.1; i=$((i + 1)); done
echo "asking the compositor to quit" >> "$CALLS"
"$YGG_TEST_COMPOSITOR" ctl quit >> "$CALLS" 2>&1
"#;

#[test]
fn a_crash_brings_the_session_programs_back() {
    let Some(comp) = compositor() else {
        assert!(std::env::var_os("YGGDRASIL_TEST_NESTED_REQUIRED").is_none(), "no yggdrasil-compositor (YGGDRASIL_COMPOSITOR)");
        eprintln!("session programs test: skipped, no yggdrasil-compositor built");
        return;
    };
    let name = "sd";
    let dir = world(name);
    let marker = format!("{}-{name}", std::process::id());
    let Some(manager) = Manager::start(&dir, &marker) else {
        remove(&dir);
        return;
    };
    let me = std::env::current_exe().unwrap();
    let program = |kind: &str| {
        format!("/usr/bin/env YGG_STAND_IN={kind} T={} {} stand_in_program --exact --quiet --test-threads=1", dir.display(), me.display())
    };
    // the autostart folder's entries, under the names the real ones have (so the panel's own drop-in applies)
    for kind in ["panel", "idle"] {
        write(
            &dir.join(format!("xdg/autostart/yggdrasil-{kind}.desktop")),
            &format!("[Desktop Entry]\nType=Application\nName=Yggdrasil {kind} (stand-in)\nExec={}\nNoDisplay=true\n", program(kind)),
            0o644,
        );
    }
    let units = dir.join("units");
    std::fs::copy(SESSION_TARGET, units.join("yggdrasil-session.target")).unwrap();
    write(
        &units.join(r"app-yggdrasil\x2dpanel@autostart.service.d/yggdrasil.conf"),
        &std::fs::read_to_string(PANEL_DROP_IN).unwrap(),
        0o644,
    );
    // the clipboard as its unit is (packages/clipboard), the portals as xdg-desktop-portal-gtk's and -hyprland's are
    let unit = |name: &str, kind: &str, service: &str, install: &str| {
        write(
            &units.join(name),
            &format!(
                "[Unit]\nPartOf=graphical-session.target\nAfter=graphical-session.target\n\n[Service]\nExecStart={}\n{service}\n{install}",
                program(kind)
            ),
            0o644,
        );
    };
    unit("test-clipboard.service", "clipboard", "Restart=on-failure\nRestartSec=3", "\n[Install]\nWantedBy=graphical-session.target\n");
    unit("test-portal-gtk.service", "portal-gtk", "", "");
    unit("test-portal-stays.service", "portal-stays", "Restart=on-failure", "");
    std::fs::create_dir_all(units.join("graphical-session.target.wants")).unwrap();
    std::os::unix::fs::symlink("../test-clipboard.service", units.join("graphical-session.target.wants/test-clipboard.service")).unwrap();
    let (ok, _) = manager.systemctl(&["daemon-reload"]);
    assert!(ok, "daemon-reload");

    // the launcher's systemctl is the real one, on the private manager only (its runtime folder; --user or nothing)
    let real = Path::new("/usr/bin/systemctl");
    stand_in(
        &dir,
        "systemctl",
        &format!("[ \"$1\" = --user ] || {{ echo refused >> \"$CALLS\"; exit 1; }}\nexec {} \"$@\"\n", real.display()),
    );
    write(&dir.join("autostart"), SYSTEMD_AUTOSTART, 0o644);
    let programs = PROGRAMS.join(" ");
    let count = PROGRAMS.len().to_string();
    // without namespaces: the private manager's programs (outside them) connect to the compositor and are seen by `kill -0`
    let (code, early) = launch(&dir, &comp, &marker, false, &[("PROGRAMS", &programs), ("COUNT", &count)]);
    let (_, active) = manager.systemctl(&["list-units", "--state=active", "--no-legend", "--plain", "*.service"]);
    drop(manager);
    no_leftovers(&marker);
    let s = finish(dir, code, early);
    eprintln!("--- active after the session\n{active}");
    assert_eq!(s.code, Some(0), "exit");
    assert!(!s.calls.contains("refused"), "only systemctl --user");
    order(
        &s.calls,
        &[
            "Hyprland --config ",
            "systemctl --user start yggdrasil-session.target",
            "all up in run 1",
            // the panel ended (exit 0) with a child still running: its unit ends with it, and the child too
            "the panel's unit after the panel ended: inactive",
            "the panel's child: gone",
            "killing the compositor",
            "systemd-cat -t yggdrasil-session echo Hyprland crashed (exit 137); starting it again with Yggdrasil's settings.",
            "systemctl --user stop yggdrasil-session.target graphical-session.target",
            "Hyprland --config ",
            "systemctl --user start yggdrasil-session.target",
            // every program back on the new compositor: also those that ended quietly (exit 0) and the portal that stayed
            "all up in run 2",
            "asking the compositor to quit",
            "systemctl --user stop yggdrasil-session.target graphical-session.target",
        ],
    )
    .unwrap();
    for kind in PROGRAMS {
        assert!(s.calls.lines().any(|l| l == format!("{kind} up in run 2")), "{kind} came back");
        assert!(!s.calls.contains(&format!("{kind} cannot connect")), "{kind} started without a display");
    }
    // the one that stays was started afresh, not left on the old display: it noted losing it, then came up again
    order(&s.calls, &["portal-stays lost its display in run 1", "portal-stays up in run 2"]).unwrap();
    // signed out: none of the session's programs still running
    for kind in ["panel", "idle", "clipboard", "portal"] {
        assert!(!active.contains(kind), "{kind} still active after the session:\n{active}");
    }
    remove(&s.dir);
}
