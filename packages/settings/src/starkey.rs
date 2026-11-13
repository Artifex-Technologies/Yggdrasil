//! Peripherals > Keyboard > Star key: what the star key does (a ThinkPad's F12, "favorites"), on a computer with such a key.
//! The choice is kept in ~/.config/yggdrasil/keys.conf (`favorite = ...`), which `yggdrasil-mediakey favorite` reads when the
//! key is pressed: sarina (the default), notifications, screenshot, app:DESKTOP-ID or none.

use eframe::egui;
use yggdrasil_ui::theme::Palette;

use crate::ui;

/// The kernel's codes for a star key: KEY_FAVORITES, and KEY_BOOKMARKS, which ThinkPads send for theirs (the X1 Yoga Gen 8's F12
/// is KEY_BOOKMARKS). Both are XF86Favorites in the keyboard map, the keysym the session binds.
const KEY_FAVORITES: usize = 364;
const KEY_BOOKMARKS: usize = 156;

/// Whether a keyboard here has a star key: /proc/bus/input/devices lists each device's keys as a bitmap ("B: KEY=..."), in
/// hexadecimal words with the highest first.
pub fn has_key(devices: &str, code: usize) -> bool {
    devices.lines().filter_map(|l| l.strip_prefix("B: KEY=")).any(|bits| {
        let words: Vec<&str> = bits.split_whitespace().collect();
        // 64-bit words, as a 64-bit kernel writes them
        let (word, bit) = (code / 64, code % 64);
        words.len() > word && u64::from_str_radix(words[words.len() - 1 - word], 16).map(|w| w >> bit & 1 == 1).unwrap_or(false)
    })
}

fn conf_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("keys.conf")
}

/// `favorite = ...` from keys.conf ("sarina" when not set).
pub fn read_choice(conf: &str) -> String {
    conf.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .find(|(k, _)| k.trim() == "favorite")
        .map(|(_, v)| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| "sarina".into())
}

/// keys.conf with its `favorite` line set to `choice` (other lines kept; the line added when there was none).
pub fn with_choice(conf: &str, choice: &str) -> String {
    let mut out = String::new();
    let mut done = false;
    for l in conf.lines() {
        if !done && !l.trim_start().starts_with('#') && l.split_once('=').is_some_and(|(k, _)| k.trim() == "favorite") {
            out.push_str(&format!("favorite = {choice}\n"));
            done = true;
        } else {
            out.push_str(l);
            out.push('\n');
        }
    }
    if !done {
        if out.is_empty() {
            out.push_str("# Written by Settings > Peripherals > Keyboard (the star key).\n");
        }
        out.push_str(&format!("favorite = {choice}\n"));
    }
    out
}

/// What the page knows: whether there is a star key, the choice, and the apps to pick from (read once, when first shown).
pub struct State {
    pub present: bool,
    pub choice: String,
    apps: Option<Vec<(String, String)>>,
}

impl State {
    pub fn read() -> State {
        let devices = std::fs::read_to_string("/proc/bus/input/devices").unwrap_or_default();
        let conf = std::fs::read_to_string(conf_path()).unwrap_or_default();
        State { present: has_key(&devices, KEY_FAVORITES) || has_key(&devices, KEY_BOOKMARKS), choice: read_choice(&conf), apps: None }
    }
}

fn label(choice: &str, apps: &[(String, String)]) -> String {
    match choice {
        "sarina" => "Talk to Sarina".into(),
        "notifications" => "Show notifications".into(),
        "screenshot" => "Take a screenshot".into(),
        "none" => "Nothing".into(),
        a => {
            let id = a.strip_prefix("app:").unwrap_or(a);
            let name = apps.iter().find(|(i, _)| i == id).map(|(_, n)| n.clone()).unwrap_or_else(|| id.to_string());
            format!("Open {name}")
        }
    }
}

/// The Star key group; nothing on a computer without the key.
pub fn group(ui: &mut egui::Ui, pal: &Palette, st: &mut State) {
    if !st.present {
        return;
    }
    let apps = st.apps.get_or_insert_with(|| {
        crate::appmgr::list_apps("").into_iter().map(|a| (a.id.trim_end_matches(".desktop").to_string(), a.name)).collect()
    });
    let mut chosen: Option<String> = None;
    ui::group(ui, pal, Some("Star key"), |ui| {
        ui::row(ui, pal, true, "When the star key is pressed", Some("The key with a star (F12 on a ThinkPad)"), |ui| {
            egui::ComboBox::from_id_salt("star-key").selected_text(label(&st.choice, apps)).width(240.0).height(360.0).show_ui(ui, |ui| {
                for c in ["sarina", "notifications", "screenshot", "none"] {
                    if ui.selectable_label(st.choice == c, label(c, apps)).clicked() {
                        chosen = Some(c.to_string());
                    }
                }
                ui.separator();
                for (id, name) in apps.iter() {
                    let c = format!("app:{id}");
                    if ui.selectable_label(st.choice == c, format!("Open {name}")).clicked() {
                        chosen = Some(c);
                    }
                }
            });
        });
    });
    if let Some(c) = chosen {
        let path = conf_path();
        let conf = std::fs::read_to_string(&path).unwrap_or_default();
        if let Some(d) = path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        if std::fs::write(&path, with_choice(&conf, &c)).is_ok() {
            st.choice = c;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_key_is_found_in_the_key_bitmap() {
        // bit 364 is word 5 from the right (364 / 64), bit 44 in it
        let with = format!("N: Name=\"ThinkPad Extra Buttons\"\nB: KEY={:x} 0 0 0 0 0\n", 1u64 << 44);
        assert!(has_key(&with, KEY_FAVORITES));
        // KEY_BOOKMARKS (156): word 2 from the right, bit 28 (a ThinkPad's star key)
        assert!(has_key(&format!("B: KEY={:x} 0 0\n", 1u64 << 28), KEY_BOOKMARKS));
        assert!(!has_key("N: Name=\"AT Keyboard\"\nB: KEY=402000000 3803078f800d001 feffffdfffefffff fffffffffffffffe\n", 364));
        assert!(!has_key("", 364));
    }

    #[test]
    fn the_choice_is_kept_in_keys_conf() {
        assert_eq!(read_choice(""), "sarina");
        assert_eq!(read_choice("# x\nfavorite = app:icarus\n"), "app:icarus");
        assert_eq!(
            with_choice("", "screenshot"),
            "# Written by Settings > Peripherals > Keyboard (the star key).\nfavorite = screenshot\n"
        );
        assert_eq!(with_choice("other = 1\nfavorite = sarina\n", "none"), "other = 1\nfavorite = none\n");
    }
}
