//! yggdrasil-close: closes the focused window (Super+Q, Alt+F4, Super+W), except for programs that should keep running in the
//! background: their window is minimized instead (as Super+Down does), so music keeps playing and Fylgja keeps showing what plays.
//! Click the program in the dock to bring it back; quit it from its own menu.
//!
//! The list: ~/.config/yggdrasil/keep-running, one window class per line (# comments). Without the file: Spotify.
//! On whichever compositor runs (yggdrasil-wm).

use yggdrasil_wm::Desk;

/// The classes that keep running: the file's lines without comments and blanks (spaces inside a line are dropped too), or
/// Spotify when there is no file.
fn keep_list(file: Option<&str>) -> Vec<String> {
    match file {
        Some(text) => text
            .lines()
            .map(|l| l.split('#').next().unwrap_or("").chars().filter(|c| *c != ' ' && *c != '\t').collect::<String>())
            .filter(|l| !l.is_empty())
            .collect(),
        None => vec!["spotify".into()],
    }
}

/// Whether a window of this class keeps running (the class as a whole line of the list, in any case).
fn keeps(list: &[String], class: &str) -> bool {
    !class.is_empty() && list.iter().any(|k| k.to_lowercase() == class.to_lowercase())
}

fn main() {
    let Some(desk) = Desk::detect() else { std::process::exit(1) };
    let class = desk.active_window(false).map(|w| w.class).unwrap_or_default();
    let file = std::fs::read_to_string(yggdrasil_conf::dirs::config().join("keep-running")).ok();
    let done = if keeps(&keep_list(file.as_deref()), &class) { desk.minimize_active() } else { desk.close_active() };
    std::process::exit(if done { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keep_running_list() {
        assert_eq!(keep_list(None), ["spotify"]);
        let list = keep_list(Some("# programs that keep playing\nSpotify\n  org.example.Player  # mine\n\n"));
        assert_eq!(list, ["Spotify", "org.example.Player"]);
        assert!(keeps(&list, "spotify") && keeps(&list, "ORG.EXAMPLE.PLAYER"));
        assert!(!keeps(&list, "org.example") && !keeps(&list, ""));
        assert!(keep_list(Some("")).is_empty(), "an empty file keeps nothing running");
    }
}
