//! yggdrasil-panelctl, yggdrasil-mediakey and yggdrasil-mount-open with stand-ins for busctl, wpctl, findmnt, udisksctl and
//! setsid on a PATH of their own: they must make the right calls (was tests/test_panel_scripts.py, for the shell scripts).

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

struct World {
    dir: PathBuf,
}

impl World {
    fn new(name: &str) -> World {
        let dir = std::env::temp_dir().join(format!("panel-ctl-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        World { dir }
    }

    /// A stand-in NAME: writes "NAME ARGS" to the log, then runs BODY.
    fn stub(&self, name: &str, body: &str) -> &World {
        let p = self.dir.join(name);
        let log = self.dir.join("calls.log");
        std::fs::write(&p, format!("#!/bin/sh\necho \"{name} $*\" >> '{}'\n{body}\n", log.display())).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        self
    }

    fn run(&self, prog: &str, args: &[&str]) -> (i32, Vec<String>, String) {
        // the stand-ins, then the shell they are written in
        let path = format!("{}:/usr/bin:/bin", self.dir.display());
        let o = Command::new(prog).args(args).env_clear().env("PATH", path).output().unwrap();
        let calls = std::fs::read_to_string(self.dir.join("calls.log")).unwrap_or_default().lines().map(str::to_string).collect();
        (o.status.code().unwrap_or(-1), calls, String::from_utf8_lossy(&o.stderr).into_owned())
    }
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const PANELCTL: &str = env!("CARGO_BIN_EXE_yggdrasil-panelctl");
const MEDIAKEY: &str = env!("CARGO_BIN_EXE_yggdrasil-mediakey");
const MOUNT_OPEN: &str = env!("CARGO_BIN_EXE_yggdrasil-mount-open");

#[test]
fn panelctl_commands_are_panel_methods() {
    for (arg, want) in [
        ("start", "ToggleStart"),
        ("calendar", "ToggleCalendar"),
        ("ask", "Ask"),
        ("notifications", "ShowFylgja u 3"),
        ("fylgja 2", "ShowFylgja u 2"),
        ("fylgja", "ShowFylgja u 0"),
        ("shortcuts", "ToggleShortcuts"),
        ("cursor-tap", "CursorTap"),
        ("zones-drop", "ZonesDrop"),
    ] {
        let w = World::new("method");
        w.stub("busctl", "");
        let (rc, calls, _) = w.run(PANELCTL, &arg.split(' ').collect::<Vec<_>>());
        assert_eq!(rc, 0, "{arg}");
        assert_eq!(calls, [format!("busctl --user call org.yggdrasil.Panel /Panel org.yggdrasil.Panel {want}")], "{arg}");
    }
}

#[test]
fn panelctl_passes_on_busctl_failing() {
    let w = World::new("fail");
    w.stub("busctl", "echo 'Call failed' >&2; exit 1");
    let (rc, _, err) = w.run(PANELCTL, &["ask"]);
    assert_eq!((rc, err.as_str()), (1, "Call failed\n"));
}

#[test]
fn panelctl_clipboard_runs_the_window() {
    let w = World::new("clipboard");
    w.stub("yggdrasil-clipboard", "exit 0");
    assert_eq!(w.run(PANELCTL, &["clipboard"]).1, ["yggdrasil-clipboard "]);
}

#[test]
fn panelctl_unknown_command_is_an_error() {
    let w = World::new("unknown");
    w.stub("busctl", "");
    let (rc, calls, err) = w.run(PANELCTL, &["nope"]);
    assert_eq!(rc, 2);
    assert!(calls.is_empty());
    assert!(err.starts_with("# yggdrasil-panelctl COMMAND:") && err.ends_with("yggdrasil-panelctl ask\n"), "{err}");
}

#[test]
fn mediakey_volume_and_mute() {
    for (key, want) in [
        ("volume-up", "wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"),
        ("volume-down", "wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"),
        ("mute", "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"),
        ("mic-mute", "wpctl set-mute @DEFAULT_AUDIO_SOURCE@ toggle"),
    ] {
        let w = World::new("volume");
        w.stub("wpctl", "");
        assert_eq!(w.run(MEDIAKEY, &[key]).1, [want], "{key}");
    }
}

#[test]
fn mediakey_player_keys_go_to_the_running_player() {
    let w = World::new("player");
    w.stub("busctl", r#"[ "$2" = list ] && printf ':1.2 1 x\norg.mpris.MediaPlayer2.spotify 1234 spotify\n'; exit 0"#);
    let (rc, calls, _) = w.run(MEDIAKEY, &["play-pause"]);
    assert_eq!(rc, 0);
    assert_eq!(
        calls,
        [
            "busctl --user list --no-legend",
            "busctl --user call org.mpris.MediaPlayer2.spotify /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Player PlayPause"
        ]
    );
}

#[test]
fn mediakey_no_player_is_not_an_error() {
    let w = World::new("noplayer");
    w.stub("busctl", "exit 0");
    let (rc, calls, _) = w.run(MEDIAKEY, &["next"]);
    assert_eq!(rc, 0);
    assert_eq!(calls, ["busctl --user list --no-legend"]);
}

#[test]
fn mount_open_mounts_and_opens() {
    let w = World::new("mount");
    w.stub("findmnt", "exit 1").stub("udisksctl", "echo 'Mounted /dev/sdb1 at /run/media/me/Photos at home.'").stub("setsid", "");
    let (rc, calls, _) = w.run(MOUNT_OPEN, &["/dev/sdb1"]);
    assert_eq!(rc, 0);
    assert_eq!(calls.last().map(String::as_str), Some("setsid -f minos /run/media/me/Photos at home"));
}

#[test]
fn mount_open_refuses_bad_names() {
    let w = World::new("bad");
    for dev in ["sdb1", "/dev/sdb1;reboot", "/dev/a b", ""] {
        let (rc, calls, _) = w.run(MOUNT_OPEN, &[dev]);
        assert_eq!((rc, calls.len()), (2, 0), "{dev}");
    }
    assert!(Path::new(MOUNT_OPEN).exists());
}
