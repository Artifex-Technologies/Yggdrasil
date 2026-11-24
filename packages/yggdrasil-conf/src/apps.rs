//! Programs as the menu knows them: a program's name from its menu entry (`Name=` in the `[Desktop Entry]` group of
//! APP_ID.desktop), for the app id a portal request or a window gives. The share picker's question and the screen sharing
//! item in the bar name the program this way; when there is no entry they say "A program" rather than guess.

use std::path::PathBuf;

/// The folders menu entries are in: the person's own first (XDG_DATA_HOME), then the system's (XDG_DATA_DIRS).
pub fn application_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let data_home =
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).or_else(|| home.map(|h| h.join(".local/share")));
    let data_dirs = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    data_home
        .into_iter()
        .chain(data_dirs.split(':').filter(|d| d.starts_with('/')).map(PathBuf::from))
        .map(|d| d.join("applications"))
        .collect()
}

/// A program's name from its menu entry; None when there is none, or `app_id` is not an app id.
pub fn app_name(app_id: &str) -> Option<String> {
    app_name_in(app_id, &application_dirs())
}

/// As `app_name`, from the folders given.
pub fn app_name_in(app_id: &str, dirs: &[PathBuf]) -> Option<String> {
    // an app id is a D-Bus-style name: anything else is not looked up as a file name
    let plain = app_id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'));
    if app_id.is_empty() || !plain || app_id.starts_with('.') {
        return None;
    }
    dirs.iter().find_map(|d| entry_name(&std::fs::read_to_string(d.join(format!("{app_id}.desktop"))).ok()?))
}

fn entry_name(text: &str) -> Option<String> {
    let mut in_entry = false;
    for l in text.lines().map(str::trim) {
        if l.starts_with('[') {
            in_entry = l == "[Desktop Entry]";
        } else if in_entry {
            if let Some(v) = l.strip_prefix("Name=") {
                let v = v.trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_program_is_named_by_its_menu_entry_and_never_guessed() {
        let dir = std::env::temp_dir().join(format!("conf-apps-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("org.example.Call.desktop"),
            "[Desktop Action new]\nName=New call\n[Desktop Entry]\nType=Application\nName=Example Call\n",
        )
        .unwrap();
        let dirs = vec![PathBuf::from("/nonexistent"), dir.clone()];
        assert_eq!(app_name_in("org.example.Call", &dirs).as_deref(), Some("Example Call"));
        // no entry, or an id that is not one: no name
        assert_eq!(app_name_in("org.example.Other", &dirs), None);
        assert_eq!(app_name_in("../org.example.Call", &dirs), None);
        assert_eq!(app_name_in(".hidden", &dirs), None);
        assert_eq!(app_name_in("", &dirs), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
