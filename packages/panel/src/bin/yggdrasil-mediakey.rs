//! yggdrasil-mediakey KEY: what the multimedia keys do, for the session's key bindings: the sound output and the microphone
//! (wpctl), the music player that is running (its MPRIS interface, through busctl) and the screen brightness (brightnessctl).
//! The same calls, output and exit codes as the shell script of the same name; the player is found in `busctl --user list`
//! here, without awk.

use std::process::{Command, Stdio};

const USAGE: &str = "\
# yggdrasil-mediakey KEY: what the multimedia keys do, for the session's key bindings.
#
#   volume-up | volume-down | mute            the sound output (PipeWire, wpctl); the panel shows its own volume pop-up
#   mic-mute                                   the microphone
#   play-pause | next | previous | stop        the music player that is running (MPRIS)
#   brightness-up | brightness-down            the screen brightness (brightnessctl, when installed)
# Hyprland example:  bindel = , XF86AudioRaiseVolume, exec, yggdrasil-mediakey volume-up
";

fn main() {
    let key = std::env::args().nth(1).unwrap_or_default();
    let code = match key.as_str() {
        "volume-up" => run(&["wpctl", "set-volume", "-l", "1.0", "@DEFAULT_AUDIO_SINK@", "5%+"]),
        "volume-down" => run(&["wpctl", "set-volume", "@DEFAULT_AUDIO_SINK@", "5%-"]),
        "mute" => run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]),
        "mic-mute" => run(&["wpctl", "set-mute", "@DEFAULT_AUDIO_SOURCE@", "toggle"]),
        "play-pause" => mpris("PlayPause"),
        "next" => mpris("Next"),
        "previous" => mpris("Previous"),
        "stop" => mpris("Stop"),
        "brightness-up" => run(&["brightnessctl", "set", "5%+"]),
        "brightness-down" => run(&["brightnessctl", "set", "5%-"]),
        _ => {
            eprint!("{USAGE}");
            2
        }
    };
    std::process::exit(code)
}

/// The running player's method: nothing (exit 0) when no player is running.
fn mpris(method: &str) -> i32 {
    let Some(p) = player() else { return 0 };
    let mut c = Command::new("busctl");
    c.args(["--user", "call", &p, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", method]).stdout(Stdio::null());
    status(&mut c)
}

/// The first bus name in `busctl --user list --no-legend` that starts with org.mpris.MediaPlayer2. (its first field).
fn player() -> Option<String> {
    let o = Command::new("busctl").args(["--user", "list", "--no-legend"]).stderr(Stdio::null()).output().ok()?;
    first_player(&String::from_utf8_lossy(&o.stdout))
}

fn first_player(list: &str) -> Option<String> {
    list.split('\n')
        .find(|l| l.starts_with("org.mpris.MediaPlayer2."))
        .and_then(|l| l.split([' ', '\t']).find(|w| !w.is_empty()))
        .map(str::to_string)
}

fn run(argv: &[&str]) -> i32 {
    status(Command::new(argv[0]).args(&argv[1..]))
}

/// The exit status as the shell gives it: the code, 128 + a signal, 127 when the program is not there.
fn status(c: &mut Command) -> i32 {
    match c.status() {
        Ok(s) => s.code().unwrap_or_else(|| 128 + std::os::unix::process::ExitStatusExt::signal(&s).unwrap_or(0)),
        Err(e) => {
            let prog = c.get_program().to_string_lossy().into_owned();
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("yggdrasil-mediakey: {prog}: not found");
                127
            } else {
                eprintln!("yggdrasil-mediakey: {prog}: {e}");
                126
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_player_on_the_bus() {
        let list =
            ":1.5 123 x\norg.freedesktop.Notifications 9 y\norg.mpris.MediaPlayer2.vlc 77 vlc u\norg.mpris.MediaPlayer2.spotify 8 s\n";
        assert_eq!(first_player(list).as_deref(), Some("org.mpris.MediaPlayer2.vlc"));
        assert_eq!(first_player("org.mpris.MediaPlayer2 1 x\n x org.mpris.MediaPlayer2.a\n"), None);
        assert_eq!(first_player(""), None);
    }
}
