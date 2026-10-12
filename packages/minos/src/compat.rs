//! Reading data saved before the app got its name. It was "Labyrinth Files", then "Finder", and is
//! now Minos.
//!
//! Settings, Proton Drive links and verification reports used to live in a `labyrinth-files` or
//! `finder` folder under the config directory. Writes go to the new `minos` folder; reads fall back
//! to the old folders when the new file does not exist, so nothing the user set up is lost.

use std::path::{Path, PathBuf};

const OLD: [&str; 2] = ["finder", "labyrinth-files"];
const NEW: &str = "minos";

/// The paths the same file had under the old folder names, if `p` is inside the new folder.
pub fn legacy_paths(p: &Path) -> Vec<PathBuf> {
    let (Some(parent), Some(name)) = (p.parent(), p.file_name()) else { return vec![] };
    if parent.file_name() != Some(NEW.as_ref()) {
        return vec![];
    }
    OLD.iter().map(|old| parent.with_file_name(old).join(name)).collect()
}

/// Read `p`, or its legacy twin when `p` is missing.
pub fn read_to_string(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().or_else(|| legacy_paths(p).into_iter().find_map(|l| std::fs::read_to_string(l).ok()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_new_folder_to_old() {
        assert_eq!(legacy_paths(Path::new("/c/minos/state.json")), vec![PathBuf::from("/c/finder/state.json"), PathBuf::from("/c/labyrinth-files/state.json")]);
        assert!(legacy_paths(Path::new("/c/other/state.json")).is_empty());
        assert!(legacy_paths(Path::new("state.json")).is_empty());
    }

    #[test]
    fn falls_back_only_when_new_is_missing() {
        let d = std::env::temp_dir().join(format!("compat-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("labyrinth-files")).unwrap();
        std::fs::write(d.join("labyrinth-files/a.json"), "old").unwrap();
        let new = d.join("minos/a.json");
        assert_eq!(read_to_string(&new).as_deref(), Some("old"));
        std::fs::create_dir_all(d.join("finder")).unwrap();
        std::fs::write(d.join("finder/a.json"), "finder").unwrap();
        assert_eq!(read_to_string(&new).as_deref(), Some("finder"), "the newer old name wins");
        std::fs::create_dir_all(d.join("minos")).unwrap();
        std::fs::write(&new, "new").unwrap();
        assert_eq!(read_to_string(&new).as_deref(), Some("new"));
        assert!(read_to_string(&d.join("minos/missing.json")).is_none());
    }
}
