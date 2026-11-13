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
#   display                                    the screens' settings (Settings > Displays)
#   airplane                                   every radio off (Wi-Fi, Bluetooth), or all back on (rfkill), and says which
#   notifications                              the notification list
#   favorite                                   the star key: what ~/.config/yggdrasil/keys.conf says (`favorite = ...`):
#                                              sarina (the default), notifications, screenshot, app:DESKTOP-ID or none
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
        "display" => run(&["yggdrasil-settings", "displays"]),
        "airplane" => airplane(),
        "notifications" => run(&["yggdrasil-panelctl", "notifications"]),
        "favorite" => favorite(),
        _ => {
            eprint!("{USAGE}");
            2
        }
    };
    std::process::exit(code)
}

/// What the star key (F12 on a ThinkPad) is set to do, from keys.conf's `favorite = ...` line.
#[derive(Debug, PartialEq)]
enum Favorite {
    Sarina,
    Notifications,
    Screenshot,
    App(String),
    Nothing,
}

fn parse_favorite(conf: &str) -> Favorite {
    let v = conf
        .lines()
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "favorite")
        .map(|(_, v)| v.trim().to_string())
        .unwrap_or_default();
    match v.as_str() {
        "notifications" => Favorite::Notifications,
        "screenshot" => Favorite::Screenshot,
        "none" => Favorite::Nothing,
        a if a.starts_with("app:") => {
            let id = a[4..].trim().trim_end_matches(".desktop");
            // a desktop file id: letters, digits, dots, dashes and underscores only (it becomes an argument)
            if !id.is_empty() && !id.starts_with('-') && id.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) {
                Favorite::App(id.to_string())
            } else {
                Favorite::Sarina
            }
        }
        _ => Favorite::Sarina,
    }
}

fn favorite() -> i32 {
    let conf = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| std::fs::read_to_string(d.join("yggdrasil/keys.conf")).unwrap_or_default())
        .unwrap_or_default();
    match parse_favorite(&conf) {
        // as a tap of Super+S: a hands-free conversation, and a second press ends it
        Favorite::Sarina => run(&["yggdrasil-panelctl", "ask-press"]),
        Favorite::Notifications => run(&["yggdrasil-panelctl", "notifications"]),
        Favorite::Screenshot => run(&["yggdrasil-shot"]),
        Favorite::App(id) => run(&["gtk-launch", &id]),
        Favorite::Nothing => 0,
    }
}

/// Airplane mode: when any radio is on, every one goes off; when all are off, all come back. `rfkill` (the person at the
/// computer may switch radios), then a notification says which.
fn airplane() -> i32 {
    let list = Command::new("rfkill").args(["-n", "-o", "SOFT"]).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string());
    let any_on = list.map(|l| l.lines().any(|s| s.trim() == "unblocked")).unwrap_or(true);
    let code = run(&["rfkill", if any_on { "block" } else { "unblock" }, "all"]);
    let (title, body) = if any_on {
        ("Airplane mode on", "Wi-Fi and Bluetooth are off. Press the key again to turn them back on.")
    } else {
        ("Airplane mode off", "Wi-Fi and Bluetooth are on again.")
    };
    let icon = if any_on { "airplane-mode-symbolic" } else { "network-wireless-symbolic" };
    let _ = Command::new("notify-send").args(["-a", "Airplane mode", "-i", icon, title, body]).stdout(Stdio::null()).status();
    code
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

    #[test]
    fn the_star_key_does_what_keys_conf_says() {
        assert_eq!(parse_favorite(""), Favorite::Sarina);
        assert_eq!(parse_favorite("# x\nfavorite = notifications\n"), Favorite::Notifications);
        assert_eq!(parse_favorite("favorite=screenshot"), Favorite::Screenshot);
        assert_eq!(parse_favorite("favorite = app:icarus.desktop"), Favorite::App("icarus".into()));
        assert_eq!(parse_favorite("favorite = app:--exec=boom;rm"), Favorite::Sarina, "not a desktop id");
        assert_eq!(parse_favorite("favorite = app:-x"), Favorite::Sarina, "not an option");
        assert_eq!(parse_favorite("favorite = none"), Favorite::Nothing);
    }
}
