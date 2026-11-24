//! yggdrasil-compositor-session with stand-ins for everything it starts: the compositor (it runs its `--run` program with a
//! display, leaves an after-session note when told to, and ends with the exit code it is told), systemctl, systemd-run,
//! systemd-cat, dbus-update-activation-environment, the sign-in animation, the Hyprland session (the way back) and the
//! system's reboot. Each notes how it was called; nothing of the real session or system is touched.

use std::path::{Path, PathBuf};
use std::process::Command;

struct World {
    dir: PathBuf,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl World {
    fn new(name: &str) -> World {
        let dir = std::env::temp_dir().join(format!("own-session-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for d in ["bin", "log", "run", "home"] {
            std::fs::create_dir_all(dir.join(d)).unwrap();
        }
        let w = World { dir };
        let log = w.dir.join("log");
        let l = log.display();
        let note = |name: &str| format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {l}/{name}\n");
        w.tool("systemctl", &note("systemctl"));
        w.tool("systemd-run", &note("systemd-run"));
        w.tool("dbus-update-activation-environment", &note("dbus"));
        w.tool("yggdrasil-splash", &note("splash"));
        w.tool("fake-hyprland-session", &note("fallback"));
        w.tool("fake-power", &note("power"));
        w.tool("fake-panel", "#!/bin/sh\n");
        // systemd-cat -t TAG [PROGRAM ARGS...]: noted, then the program runs (its exit status is systemd-cat's)
        w.tool(
            "systemd-cat",
            &format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> {l}/systemd-cat\nshift 2\n[ $# -gt 0 ] && exec \"$@\"\ncat >/dev/null\n"),
        );
        // the compositor: noted; runs the --run program with a display; leaves the note in log/plan-after; ends with log/plan-code
        w.tool(
            "fake-compositor",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" >> {l}/compositor\nwhile [ $# -gt 0 ] && [ \"$1\" != --run ]; do shift; done\nshift\n\
                 WAYLAND_DISPLAY=wayland-test DISPLAY=:9 \"$@\"\n\
                 [ -f {l}/plan-after ] && cat {l}/plan-after > \"$XDG_RUNTIME_DIR/yggdrasil-after-session\"\n\
                 exit $(cat {l}/plan-code 2>/dev/null || echo 0)\n"
            ),
        );
        w
    }

    fn tool(&self, name: &str, text: &str) {
        let p = self.dir.join("bin").join(name);
        std::fs::write(&p, text).unwrap();
        Command::new("chmod").arg("+x").arg(&p).status().unwrap();
    }

    fn plan(&self, code: i32, after: Option<&str>) {
        std::fs::write(self.dir.join("log/plan-code"), code.to_string()).unwrap();
        if let Some(a) = after {
            std::fs::write(self.dir.join("log/plan-after"), a).unwrap();
        }
    }

    /// The session, run to its end: its exit code.
    fn run(&self, extra: &[(&str, &Path)]) -> i32 {
        let b = |n: &str| self.dir.join("bin").join(n);
        let mut c = Command::new(env!("CARGO_BIN_EXE_yggdrasil-compositor-session"));
        c.env_clear()
            .env("PATH", format!("{}:/usr/bin:/bin", self.dir.join("bin").display()))
            .env("HOME", self.dir.join("home"))
            .env("XDG_RUNTIME_DIR", self.dir.join("run"))
            .env("YGGDRASIL_COMPOSITOR", b("fake-compositor"))
            .env("YGGDRASIL_SESSION_FALLBACK", b("fake-hyprland-session"))
            .env("YGGDRASIL_SYSTEMCTL", b("fake-power"))
            .env("YGGDRASIL_OWN_PANEL", self.dir.join("no-own-panel"));
        for (k, v) in extra {
            c.env(k, v);
        }
        c.status().expect("the session runs").code().unwrap_or(-1)
    }

    fn log(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.join("log").join(name)).unwrap_or_default().lines().map(str::to_string).collect()
    }
}

#[test]
fn a_session_starts_the_compositor_with_its_autostart_and_cleans_up_after() {
    let w = World::new("signout");
    w.plan(0, None);
    assert_eq!(w.run(&[]), 0);
    let comp = w.log("compositor");
    let me = env!("CARGO_BIN_EXE_yggdrasil-compositor-session");
    assert_eq!(comp, vec![format!("--drm --run {me} --autostart")], "the compositor on the hardware, the autostart its first program");
    // the autostart, in order: what is left stopped first, the environment handed on, then the session's programs
    let sc = w.log("systemctl");
    assert_eq!(sc[0], "--user stop graphical-session.target");
    assert_eq!(sc[1], "--user reset-failed");
    assert_eq!(sc[2], "--user unset-environment HYPRLAND_INSTANCE_SIGNATURE");
    assert!(sc[3].starts_with("--user import-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP XDG_SESSION_DESKTOP"), "{}", sc[3]);
    assert_eq!(sc[4], "--user start yggdrasil-session.target");
    assert_eq!(w.log("dbus"), ["--systemd --all"]);
    assert_eq!(w.log("splash").len(), 1, "the sign-in animation");
    // after the compositor: the session's programs stopped, its display taken back
    assert_eq!(sc[5], "--user stop yggdrasil-own-panel.service yggdrasil-session.target graphical-session.target");
    assert_eq!(sc[6], "--user unset-environment WAYLAND_DISPLAY DISPLAY");
    assert_eq!(sc.len(), 7);
    assert!(w.log("systemd-run").is_empty(), "no own panel installed: none started");
    assert!(w.log("fallback").is_empty() && w.log("power").is_empty(), "a sign-out is only a sign-out");
    let journal = w.log("systemd-cat").join("\n");
    assert!(journal.contains("autostart started for Yggdrasil-Compositor:Yggdrasil on wayland-test"), "{journal}");
}

#[test]
fn a_compositor_that_fails_at_once_gives_way_to_the_hyprland_session() {
    let w = World::new("fallback");
    w.plan(1, None);
    w.run(&[]);
    assert_eq!(w.log("fallback").len(), 1, "the Hyprland session started instead");
    let journal = w.log("systemd-cat").join("\n");
    assert!(journal.contains("starting the Hyprland session instead"), "{journal}");
}

#[test]
fn restart_from_the_session_happens_once_it_has_ended_and_an_old_note_is_no_order() {
    let w = World::new("reboot");
    w.plan(0, Some("reboot\n"));
    assert_eq!(w.run(&[]), 0);
    assert_eq!(w.log("power"), ["reboot"]);
    assert!(w.log("fallback").is_empty());
    assert!(!w.dir.join("run/yggdrasil-after-session").exists(), "the note is used up");
    // a note left from a session that never read it: removed at the start, so this one only signs out
    let w = World::new("stale");
    w.plan(0, None);
    std::fs::write(w.dir.join("run/yggdrasil-after-session"), "poweroff").unwrap();
    assert_eq!(w.run(&[]), 0);
    assert!(w.log("power").is_empty(), "{:?}", w.log("power"));
}

#[test]
fn the_own_panel_build_takes_the_packaged_panels_place_and_a_test_leaves_the_user_manager_alone() {
    let w = World::new("panel");
    w.plan(0, None);
    let panel = w.dir.join("bin/fake-panel");
    w.run(&[("YGGDRASIL_OWN_PANEL", &panel), ("YGGDRASIL_SESSION_TEST", Path::new("1"))]);
    let sc = w.log("systemctl");
    assert!(sc.iter().any(|l| l == "--user stop app-yggdrasil*panel@autostart.service"), "{sc:?}");
    let run = w.log("systemd-run");
    assert_eq!(run.len(), 1);
    assert!(run[0].starts_with("--user --unit=yggdrasil-own-panel --collect -p PartOf=graphical-session.target"), "{}", run[0]);
    assert!(run[0].ends_with(&panel.display().to_string()));
    // YGGDRASIL_SESSION_TEST: nothing stopped at the end
    assert!(!sc.iter().any(|l| l.contains("yggdrasil-own-panel.service") || l.contains("unset-environment WAYLAND_DISPLAY")), "{sc:?}");
}
