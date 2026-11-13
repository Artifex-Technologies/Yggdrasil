//! ygg-theme as Settings and the panel run it, on a scratch home: the Python script's Settings tests (tests/test_ygg_theme.py)
//! carried over, and the pointer.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_ygg-theme");

struct Home(PathBuf);

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn home() -> Home {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let d =
        std::env::temp_dir().join(format!("ygg-theme-home-{}-{}", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    Home(d)
}

impl Home {
    fn run(&self, args: &[&str], kde: &str) -> std::process::Output {
        Command::new(BIN)
            .args(args)
            .env_clear()
            .env("HOME", &self.0)
            .env("XDG_CONFIG_HOME", self.0.join(".config"))
            .env("XDG_DATA_HOME", self.0.join(".local/share"))
            .env("PATH", "/nonexistent")
            .env("YGG_THEME_KDE", kde)
            .output()
            .unwrap()
    }
    fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.0.join(rel)).unwrap()
    }
    fn write(&self, rel: &str, text: &str) {
        let p = self.0.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }
}

fn section(text: &str, name: &str, key: &str) -> Option<String> {
    let mut cur = String::new();
    let mut found = None;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            cur = l.to_string();
        } else if let Some((k, v)) = l.split_once('=') {
            if cur == name && k == key {
                found = Some(v.to_string());
            }
        }
    }
    found
}

#[test]
fn test_apply_writes_everything_and_keeps_the_users_own_css() {
    let h = home();
    h.write(".config/gtk-4.0/gtk.css", "@import 'colors.css';\nwindow { border-radius: 3px; }\n");
    let r = h.run(&["apply", "--dark", "--accent", "12,34,56"], "1");
    assert_eq!(r.status.code(), Some(0), "{}", String::from_utf8_lossy(&r.stderr));
    let k = h.read(".config/kdeglobals");
    assert_eq!(section(&k, "[General]", "ColorScheme").as_deref(), Some("YggdrasilDark"));
    assert_eq!(section(&k, "[General]", "AccentColor").as_deref(), Some("12,34,56"));
    assert_eq!(section(&k, "[KDE]", "widgetStyle").as_deref(), Some("Fusion"));
    assert_eq!(section(&k, "[Icons]", "Theme").as_deref(), Some("Liki"));
    assert!(h.0.join(".local/share/color-schemes/YggdrasilDark.colors").is_file());
    let g3 = h.read(".config/gtk-3.0/settings.ini");
    let g = |k: &str| section(&g3, "[Settings]", k).unwrap();
    assert_eq!(
        (g("gtk-theme-name"), g("gtk-icon-theme-name"), g("gtk-cursor-theme-name")),
        ("Yggdrasil-dark".into(), "Liki".into(), "Yggdrasil".into()),
        "dark mode is a GTK theme of its own"
    );
    let css = h.read(".config/gtk-4.0/gtk.css");
    assert!(css.contains("rgb(12,34,56)"));
    assert!(css.contains("border-radius: 3px"));
    assert!(!css.contains("colors.css"));
    // applying again does not pile things up
    h.run(&["apply", "--light"], "1");
    let css2 = h.read(".config/gtk-4.0/gtk.css");
    assert_eq!(css2.matches("border-radius: 3px").count(), 1);
    assert_eq!(section(&h.read(".config/kdeglobals"), "[General]", "ColorScheme").as_deref(), Some("YggdrasilLight"));
}

#[test]
fn test_no_kde_files_without_kde_programs() {
    let h = home();
    h.write(".config/yggdrasil/appearance.conf", "style = dark\naccent = 1,2,3\nicons = Papirus\n");
    let r = h.run(&["apply", "--light"], "0");
    assert_eq!(r.status.code(), Some(0), "{}", String::from_utf8_lossy(&r.stderr));
    for kde in [".config/kdeglobals", ".config/kcminputrc", ".local/share/color-schemes"] {
        assert!(!h.0.join(kde).exists(), "{kde}");
    }
    assert_eq!(h.read(".config/yggdrasil/appearance.conf"), "style = light\naccent = 1,2,3\nicons = Papirus\n", "the icon pack is kept");
    assert_eq!(section(&h.read(".config/gtk-3.0/settings.ini"), "[Settings]", "gtk-icon-theme-name").as_deref(), Some("Papirus"));
    assert_eq!(String::from_utf8_lossy(&r.stdout), "Yggdrasil light, accent 1,2,3\n");
}

// ---- the pointer

#[test]
fn a_pointer_theme_is_chosen_only_when_installed() {
    let h = home();
    h.write(".local/share/icons/Mine/cursors/left_ptr", "x");
    h.write(".icons/NoCursors/index.theme", "x");
    for bad in ["NoCursors", "../Mine", ".Mine", "", "Mine/cursors"] {
        let r = h.run(&["cursor", bad], "0");
        assert_eq!(r.status.code(), Some(1), "{bad}");
    }
    assert!(String::from_utf8_lossy(&h.run(&["cursor", "it's"], "0").stderr).contains("no cursor theme called \"it's\" is installed"));
    let r = h.run(&["cursor", "Mine"], "1");
    assert_eq!(String::from_utf8_lossy(&r.stdout), "Pointer: Mine\n");
    assert_eq!(h.read(".config/environment.d/50-yggdrasil-cursor.conf"), "XCURSOR_THEME=Mine\nXCURSOR_SIZE=24\n");
    assert_eq!(section(&h.read(".config/kcminputrc"), "[Mouse]", "cursorTheme").as_deref(), Some("Mine"));
    assert!(h.read(".config/yggdrasil/appearance.conf").contains("cursor = Mine\n"));
    assert_eq!(String::from_utf8_lossy(&h.run(&["cursor"], "0").stdout), "Mine\n");
    // the default is not written down
    h.write(".local/share/icons/Yggdrasil/cursors/left_ptr", "x");
    h.run(&["cursor", "Yggdrasil"], "0");
    assert!(!h.read(".config/yggdrasil/appearance.conf").contains("cursor"));
    // the chosen one went away: Yggdrasil's own
    h.write(".config/yggdrasil/appearance.conf", "cursor = Gone\n");
    assert_eq!(String::from_utf8_lossy(&h.run(&["cursor"], "0").stdout), "Yggdrasil\n");
    assert!(!Path::new("/nonexistent").exists());
}

#[test]
fn the_pointer_size_is_kept_and_given_to_every_program() {
    let h = home();
    h.write(".local/share/icons/Mine/cursors/left_ptr", "x");
    h.run(&["cursor", "Mine"], "0");
    for bad in ["0", "25", "big", "-48"] {
        assert_eq!(h.run(&["cursor-size", bad], "0").status.code(), Some(1), "{bad}");
    }
    let r = h.run(&["cursor-size", "48"], "0");
    assert_eq!(String::from_utf8_lossy(&r.stdout), "Pointer size: 48\n");
    assert_eq!(h.read(".config/environment.d/50-yggdrasil-cursor.conf"), "XCURSOR_THEME=Mine\nXCURSOR_SIZE=48\n");
    assert_eq!(section(&h.read(".config/gtk-3.0/settings.ini"), "[Settings]", "gtk-cursor-theme-size").as_deref(), Some("48"));
    let conf = h.read(".config/yggdrasil/appearance.conf");
    assert!(conf.contains("cursor = Mine\n") && conf.contains("cursor_size = 48\n"), "{conf}");
    assert_eq!(String::from_utf8_lossy(&h.run(&["cursor-size"], "0").stdout), "48\n");
    // a later change of theme or style keeps the size
    h.run(&["apply", "--light"], "0");
    assert!(h.read(".config/yggdrasil/appearance.conf").contains("cursor_size = 48\n"));
    // back to the default size: not written down
    h.run(&["cursor-size", "24"], "0");
    assert!(!h.read(".config/yggdrasil/appearance.conf").contains("cursor_size"));
    assert_eq!(h.read(".config/environment.d/50-yggdrasil-cursor.conf"), "XCURSOR_THEME=Mine\nXCURSOR_SIZE=24\n");
}
