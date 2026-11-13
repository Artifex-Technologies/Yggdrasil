//! Programs that keep running in the background: closing their window (Super+Q, Alt+F4, Super+W) minimizes it instead, so music
//! keeps playing and Fylgja keeps showing what plays. `yggdrasil-close` on Hyprland and Yggdrasil's own compositor both ask here.
//!
//! The list: `~/.config/yggdrasil/keep-running`, one window class per line (# comments). Without the file: Spotify.

/// The classes that keep running: the file's lines without comments and blanks (spaces inside a line are dropped too), or
/// Spotify when there is no file.
pub fn parse(file: Option<&str>) -> Vec<String> {
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
pub fn keeps(list: &[String], class: &str) -> bool {
    !class.is_empty() && list.iter().any(|k| k.to_lowercase() == class.to_lowercase())
}

/// The list now (one small file read).
pub fn load() -> Vec<String> {
    parse(std::fs::read_to_string(crate::dirs::config().join("keep-running")).ok().as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_keep_running_list() {
        assert_eq!(parse(None), ["spotify"]);
        let list = parse(Some("# programs that keep playing\nSpotify\n  org.example.Player  # mine\n\n"));
        assert_eq!(list, ["Spotify", "org.example.Player"]);
        assert!(keeps(&list, "spotify") && keeps(&list, "ORG.EXAMPLE.PLAYER"));
        assert!(!keeps(&list, "org.example") && !keeps(&list, ""));
        assert!(parse(Some("")).is_empty(), "an empty file keeps nothing running");
    }
}
