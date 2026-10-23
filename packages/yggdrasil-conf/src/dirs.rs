//! Where Yggdrasil keeps its files: `~/.config/yggdrasil`, `~/.local/share/yggdrasil`, `~/.local/state/yggdrasil`, `~/.cache/yggdrasil`
//! for the user (XDG variables respected), `/etc/yggdrasil` and `/usr/share/yggdrasil` for the system. Every program asks here instead
//! of building paths itself, so the desktop has one set of folders whichever operating system it runs on.
//!
//! Before the desktop had its own name its files lived under `labyrinth`. `migrate` moves the desktop's own files from there once (the
//! operating system's files, such as an assistant's settings, stay where they are).

use std::path::{Path, PathBuf};

const NAME: &str = "yggdrasil";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn xdg(var: &str, fallback: &str) -> PathBuf {
    std::env::var_os(var).map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home().join(fallback))
}

static MIGRATED: std::sync::Once = std::sync::Once::new();

/// The first time any folder is asked for, the old files are moved (see `migrate`); no program has to remember to do it.
fn ready() {
    MIGRATED.call_once(migrate);
}

pub fn config() -> PathBuf {
    ready();
    raw_config()
}
pub fn data() -> PathBuf {
    ready();
    xdg("XDG_DATA_HOME", ".local/share").join(NAME)
}
pub fn state() -> PathBuf {
    ready();
    xdg("XDG_STATE_HOME", ".local/state").join(NAME)
}
pub fn cache() -> PathBuf {
    ready();
    xdg("XDG_CACHE_HOME", ".cache").join(NAME)
}

fn raw_config() -> PathBuf {
    xdg("XDG_CONFIG_HOME", ".config").join(NAME)
}
/// System-wide settings an administrator or the operating system sets.
pub fn system_config() -> PathBuf {
    PathBuf::from("/etc").join(NAME)
}
/// Files the packages ship.
pub fn system_data() -> PathBuf {
    PathBuf::from("/usr/share").join(NAME)
}

/// The desktop's own files in the old `labyrinth` folders, by folder: (config, data, state, cache).
const OLD: [(&str, &[&str]); 4] = [
    ("XDG_CONFIG_HOME", &["panel.conf", "idle.conf", "lock.conf", "dock", "wallpaper", "workspaces.conf", "shortcuts.conf", "tools.conf", "weather.json", "market"]),
    ("XDG_DATA_HOME", &["calculator-history.json", "weather", "market", "fylgja", "wallpapers"]),
    ("XDG_STATE_HOME", &["notifications.json", "idle.log", "colors.txt"]),
    ("XDG_CACHE_HOME", &["lock-previews"]),
];

fn old_and_new(var: &str) -> (PathBuf, PathBuf) {
    let base = match var {
        "XDG_CONFIG_HOME" => xdg(var, ".config"),
        "XDG_DATA_HOME" => xdg(var, ".local/share"),
        "XDG_STATE_HOME" => xdg(var, ".local/state"),
        _ => xdg(var, ".cache"),
    };
    (base.join("labyrinth"), base.join(NAME))
}

/// Moves the desktop's own files from the old folders, once (a marker in the new config folder records it). Safe to call from every
/// program at start: a file already in the new place is never overwritten, and a move is a rename (all or nothing).
pub fn migrate() {
    let marker = raw_config().join(".moved-from-labyrinth");
    if marker.exists() {
        return;
    }
    for (var, names) in OLD {
        let (old, new) = old_and_new(var);
        for n in names {
            move_one(&old.join(n), &new.join(n));
        }
    }
    if std::fs::create_dir_all(raw_config()).is_ok() {
        let _ = std::fs::write(&marker, "The desktop's own files were moved here from the old labyrinth folders.\n");
    }
}

fn move_one(from: &Path, to: &Path) {
    if !from.exists() || to.exists() {
        return;
    }
    if let Some(parent) = to.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }
    let _ = std::fs::rename(from, to);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_the_desktops_files_once_and_leaves_the_rest() {
        let d = std::env::temp_dir().join(format!("ygg-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let (cfg, data, state, cache) = (d.join("c"), d.join("d"), d.join("s"), d.join("k"));
        std::env::set_var("XDG_CONFIG_HOME", &cfg);
        std::env::set_var("XDG_DATA_HOME", &data);
        std::env::set_var("XDG_STATE_HOME", &state);
        std::env::set_var("XDG_CACHE_HOME", &cache);
        std::fs::create_dir_all(cfg.join("labyrinth/market")).unwrap();
        std::fs::write(cfg.join("labyrinth/panel.conf"), "dock_icon = 50\n").unwrap();
        std::fs::write(cfg.join("labyrinth/sarina.env"), "OS file\n").unwrap();
        std::fs::write(cfg.join("labyrinth/market/repos.json"), "[]").unwrap();
        std::fs::create_dir_all(state.join("labyrinth")).unwrap();
        std::fs::write(state.join("labyrinth/notifications.json"), "[]").unwrap();
        // a file already in the new place wins
        std::fs::create_dir_all(cfg.join("yggdrasil")).unwrap();
        std::fs::write(cfg.join("labyrinth/idle.conf"), "old").unwrap();
        std::fs::write(cfg.join("yggdrasil/idle.conf"), "new").unwrap();
        migrate();
        assert_eq!(std::fs::read_to_string(config().join("panel.conf")).unwrap(), "dock_icon = 50\n");
        assert!(config().join("market/repos.json").exists());
        assert!(super::state().join("notifications.json").exists());
        assert!(cfg.join("labyrinth/sarina.env").exists(), "the operating system's file stays");
        assert!(!config().join("sarina.env").exists());
        assert_eq!(std::fs::read_to_string(config().join("idle.conf")).unwrap(), "new");
        // once: a file put back in the old place later is left alone
        std::fs::write(cfg.join("labyrinth/dock"), "x").unwrap();
        migrate();
        assert!(cfg.join("labyrinth/dock").exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
