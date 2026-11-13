//! yggdrasil-panelctl COMMAND: asks the running panel to do something, for compositor key bindings and scripts. Each command is
//! one method of the panel's D-Bus interface, called with `busctl --user call` (its output to /dev/null, its errors and exit
//! code passed on), as the shell script of the same name did; `clipboard` becomes yggdrasil-clipboard.

use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};

const USAGE: &str = "\
# yggdrasil-panelctl COMMAND: ask the running panel to do something. For compositor key bindings (Hyprland, Sway, labwc ...) and scripts.
#
#   start            open or close the start menu         (bind to Super)
#   calendar         open or close the calendar (and the notification pane when it is on)
#   notifications    Fylgja on its Notifications section
#   fylgja [N]       Fylgja on section N (0 Controls, 1 Today, 2 Stats, 3 Notifications, 4 Wallpaper, 5 Power)
#   clipboard        open or close the clipboard window (yggdrasil-clipboard)   (bind to Super+V)
#   ask              start or send a spoken question to the assistant   (bind to Super+S)
#   ask-press        the talk key went down: a tap starts a hands-free conversation, a tap ends it (bind the press only)
#   ask-release      the talk key came up (accepted from older bindings; does nothing)
#   zones            the window zones sheet for the window in front, or close it   (bind to Super+Shift+Z)
#   zones-drag       a dragged window reached the notch   } called by the session's Lua while a window is dragged,
#   zones-drop       the drag ended                        } not by hand
#   shortcuts        show or hide the keyboard shortcuts card   (bind to Super+/)
#   cursor-tap       Ctrl was tapped on its own: two taps in a row put a spotlight on the pointer (Find my mouse)
# Hyprland example (hyprland.conf):  bind = SUPER, S, exec, yggdrasil-panelctl ask
";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let a = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
    let call: &[&str] = match a(0) {
        "start" => &["ToggleStart"],
        "calendar" => &["ToggleCalendar"],
        "notifications" => &["ShowFylgja", "u", "3"],
        "fylgja" => &["ShowFylgja", "u", if a(1).is_empty() { "0" } else { a(1) }],
        "clipboard" => {
            let e = Command::new("yggdrasil-clipboard").exec();
            not_started("yggdrasil-clipboard", e)
        }
        "ask" => &["Ask"],
        "ask-press" => &["AskPress"],
        "ask-release" => &["AskRelease"],
        "zones" => &["ZonesPick"],
        "zones-drag" => &["ZonesDrag"],
        "zones-drop" => &["ZonesDrop"],
        "shortcuts" => &["ToggleShortcuts"],
        "cursor-tap" => &["CursorTap"],
        _ => {
            eprint!("{USAGE}");
            std::process::exit(2)
        }
    };
    let mut c = Command::new("busctl");
    c.args(["--user", "call", "org.yggdrasil.Panel", "/Panel", "org.yggdrasil.Panel"]).args(call).stdout(Stdio::null());
    std::process::exit(match c.status() {
        Ok(s) => s.code().unwrap_or_else(|| 128 + std::os::unix::process::ExitStatusExt::signal(&s).unwrap_or(0)),
        Err(e) => {
            eprintln!(
                "yggdrasil-panelctl: busctl: {}",
                if e.kind() == std::io::ErrorKind::NotFound { "not found".into() } else { e.to_string() }
            );
            if e.kind() == std::io::ErrorKind::NotFound {
                127
            } else {
                126
            }
        }
    })
}

/// The shell's answer when exec could not start a program: 127 when it is not there, 126 otherwise.
fn not_started(prog: &str, e: std::io::Error) -> ! {
    if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!("yggdrasil-panelctl: {prog}: not found");
        std::process::exit(127)
    }
    eprintln!("yggdrasil-panelctl: {prog}: {e}");
    std::process::exit(126)
}
