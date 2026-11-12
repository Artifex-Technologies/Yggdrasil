//! Startup apps: what starts when you sign in. Two kinds: the desktop's autostart entries (/etc/xdg/autostart, and yours in
//! ~/.config/autostart, which win over the system's of the same name) and background services you (or an app) switched on in
//! your own account (~/.config/systemd/user/{default,graphical-session}.target.wants).
//!
//! Turning an autostart entry off writes your own copy with Hidden=true, the freedesktop way, so the system's file is never
//! touched and a package update cannot turn it back on; turning it on again sets Hidden=false. A service is switched with
//! systemctl --user enable/disable; services switched off here are remembered so they stay in the list.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Source {
    Autostart,
    Service,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub icon: String,
    pub enabled: bool,
    /// Why it would not start even when on (only for another desktop).
    pub note: String,
    pub source: Option<Source>,
}

/// The main group's keys of a desktop entry.
pub fn parse_desktop(text: &str) -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    let mut in_main = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_main = l == "[Desktop Entry]";
            continue;
        }
        if !in_main || l.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = l.split_once('=') {
            m.entry(k.trim().to_string()).or_insert_with(|| v.trim().to_string());
        }
    }
    m
}

/// Whether an entry starts on this desktop (`desktops`: XDG_CURRENT_DESKTOP's names) and is switched on.
pub fn autostart_state(m: &std::collections::HashMap<String, String>, desktops: &[String]) -> (bool, String) {
    let on = !m.get("Hidden").is_some_and(|v| v == "true") && !m.get("X-GNOME-Autostart-enabled").is_some_and(|v| v == "false");
    let list = |k: &str| m.get(k).map(|v| v.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect::<Vec<_>>());
    let mut note = String::new();
    if let Some(only) = list("OnlyShowIn") {
        if !only.iter().any(|d| desktops.iter().any(|c| c.eq_ignore_ascii_case(d))) {
            note = format!("Only for {}", only.join(", "));
        }
    }
    if let Some(not) = list("NotShowIn") {
        if not.iter().any(|d| desktops.iter().any(|c| c.eq_ignore_ascii_case(d))) {
            note = "Not for this desktop".into();
        }
    }
    (on, note)
}

/// Sets `Hidden=` in the main group of a desktop entry's text (adding it when missing).
pub fn set_hidden(text: &str, hidden: bool) -> String {
    let mut out = vec![];
    let mut in_main = false;
    let mut done = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            if in_main && !done {
                out.push(format!("Hidden={hidden}"));
                done = true;
            }
            in_main = l == "[Desktop Entry]";
        } else if in_main && (l.starts_with("Hidden=") || l.starts_with("Hidden =")) {
            if !done {
                out.push(format!("Hidden={hidden}"));
                done = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        out.push(format!("Hidden={hidden}"));
    }
    out.join("\n") + "\n"
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/"))
}

fn user_autostart() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("autostart")
}

fn user_units() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config")).join("systemd/user")
}

/// Services switched off here (remembered so they stay listed).
fn remembered_file() -> PathBuf {
    yggdrasil_conf::dirs::state().join("taskmanager-startup-off")
}

fn desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().split(':').filter(|s| !s.is_empty()).map(str::to_string).collect()
}

/// The autostart entries in `system` and `user` folders (yours win by file name).
pub fn autostart_in(system: &Path, user: &Path, desktops: &[String]) -> Vec<Item> {
    let mut by_id: std::collections::BTreeMap<String, Item> = Default::default();
    for dir in [system, user] {
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|x| x.to_str()) != Some("desktop") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&p) else { continue };
            let m = parse_desktop(&text);
            let id = p.file_name().unwrap().to_string_lossy().to_string();
            let (enabled, note) = autostart_state(&m, desktops);
            let base = by_id.remove(&id);
            let name = m
                .get("Name")
                .cloned()
                .or_else(|| base.as_ref().map(|b| b.name.clone()))
                .unwrap_or_else(|| id.trim_end_matches(".desktop").to_string());
            by_id.insert(
                id.clone(),
                Item {
                    id,
                    name,
                    comment: m.get("Comment").cloned().or_else(|| base.as_ref().map(|b| b.comment.clone())).unwrap_or_default(),
                    icon: m.get("Icon").cloned().or_else(|| base.as_ref().map(|b| b.icon.clone())).unwrap_or_default(),
                    enabled,
                    note,
                    source: Some(Source::Autostart),
                },
            );
        }
    }
    by_id.into_values().collect()
}

fn unit_description(name: &str) -> String {
    for dir in [user_units(), PathBuf::from("/etc/systemd/user"), PathBuf::from("/usr/lib/systemd/user")] {
        if let Ok(t) = std::fs::read_to_string(dir.join(name)) {
            if let Some(d) = t.lines().find_map(|l| l.trim().strip_prefix("Description=")) {
                return d.trim().to_string();
            }
        }
    }
    String::new()
}

/// Every startup app.
pub fn list() -> Vec<Item> {
    let mut v = autostart_in(Path::new("/etc/xdg/autostart"), &user_autostart(), &desktops());
    let mut on: std::collections::BTreeSet<String> = Default::default();
    for target in ["default.target.wants", "graphical-session.target.wants"] {
        if let Ok(rd) = std::fs::read_dir(user_units().join(target)) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().to_string();
                if n.ends_with(".service") || n.ends_with(".path") {
                    on.insert(n);
                }
            }
        }
    }
    let off: Vec<String> =
        std::fs::read_to_string(remembered_file()).unwrap_or_default().lines().map(str::to_string).filter(|n| !on.contains(n)).collect();
    for (n, enabled) in on.into_iter().map(|n| (n, true)).chain(off.into_iter().map(|n| (n, false))) {
        let d = unit_description(&n);
        v.push(Item {
            name: if d.is_empty() { n.trim_end_matches(".service").to_string() } else { d.clone() },
            comment: n.clone(),
            id: n,
            icon: String::new(),
            enabled,
            note: String::new(),
            source: Some(Source::Service),
        });
    }
    v
}

/// Switches a startup app on or off.
pub fn set(item: &Item, on: bool) -> Result<(), String> {
    match item.source {
        Some(Source::Service) => {
            let out = std::process::Command::new("systemctl")
                .args(["--user", if on { "enable" } else { "disable" }, &item.id])
                .stdin(std::process::Stdio::null())
                .output()
                .map_err(|e| e.to_string())?;
            if !out.status.success() {
                return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
            }
            let f = remembered_file();
            let mut names: Vec<String> =
                std::fs::read_to_string(&f).unwrap_or_default().lines().map(str::to_string).filter(|n| n != &item.id).collect();
            if !on {
                names.push(item.id.clone());
            }
            let _ = std::fs::create_dir_all(f.parent().unwrap());
            std::fs::write(&f, names.join("\n")).map_err(|e| e.to_string())
        }
        _ => set_autostart(&user_autostart(), Path::new("/etc/xdg/autostart"), &item.id, on),
    }
}

/// Writes your own copy of the entry with Hidden= set.
pub fn set_autostart(user: &Path, system: &Path, id: &str, on: bool) -> Result<(), String> {
    if id.contains('/') {
        return Err("bad name".into());
    }
    let mine = user.join(id);
    let text = std::fs::read_to_string(&mine).or_else(|_| std::fs::read_to_string(system.join(id))).map_err(|e| format!("{id}: {e}"))?;
    std::fs::create_dir_all(user).map_err(|e| e.to_string())?;
    let tmp = user.join(format!(".{id}.tmp"));
    std::fs::write(&tmp, set_hidden(&text, !on)).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &mine).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_and_desktops() {
        let m = parse_desktop("[Desktop Entry]\nName=Blueman\nOnlyShowIn=GNOME;Unity;\n[Desktop Action x]\nName=Other\n");
        assert_eq!(m["Name"], "Blueman");
        let (on, note) = autostart_state(&m, &["Yggdrasil".into()]);
        assert!(on);
        assert_eq!(note, "Only for GNOME, Unity");
        let (_, note) = autostart_state(&m, &["Unity".into()]);
        assert!(note.is_empty());
        let off = parse_desktop("[Desktop Entry]\nName=x\nHidden=true\n");
        assert!(!autostart_state(&off, &[]).0);
    }

    #[test]
    fn hidden_is_set_in_the_main_group_only() {
        let t = "[Desktop Entry]\nName=a\nHidden=false\n[Desktop Action b]\nName=b\n";
        let s = set_hidden(t, true);
        assert_eq!(s, "[Desktop Entry]\nName=a\nHidden=true\n[Desktop Action b]\nName=b\n");
        let s = set_hidden("[Desktop Entry]\nName=a\n[Desktop Action b]\nExec=x\n", true);
        assert!(s.starts_with("[Desktop Entry]\nName=a\nHidden=true\n[Desktop Action b]"), "{s}");
        assert_eq!(set_hidden("[Desktop Entry]\nName=a", false), "[Desktop Entry]\nName=a\nHidden=false\n");
    }

    #[test]
    fn your_copy_wins_and_switching_writes_only_yours() {
        let d = std::env::temp_dir().join(format!("tm-startup-{}", std::process::id()));
        let (sys, user) = (d.join("sys"), d.join("user"));
        std::fs::create_dir_all(&sys).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        std::fs::write(sys.join("a.desktop"), "[Desktop Entry]\nName=App A\nComment=does a\nIcon=a\n").unwrap();
        std::fs::write(sys.join("b.desktop"), "[Desktop Entry]\nName=App B\n").unwrap();
        std::fs::write(user.join("b.desktop"), "[Desktop Entry]\nName=My B\nHidden=true\n").unwrap();
        let v = autostart_in(&sys, &user, &[]);
        assert_eq!(v.len(), 2);
        assert!(v[0].enabled && v[0].comment == "does a");
        assert!(!v[1].enabled && v[1].name == "My B");
        set_autostart(&user, &sys, "a.desktop", false).unwrap();
        assert!(std::fs::read_to_string(sys.join("a.desktop")).unwrap().contains("Name=App A"), "the system's file is untouched");
        let v = autostart_in(&sys, &user, &[]);
        assert!(!v[0].enabled && v[0].icon == "a");
        set_autostart(&user, &sys, "a.desktop", true).unwrap();
        assert!(autostart_in(&sys, &user, &[])[0].enabled);
        assert!(set_autostart(&user, &sys, "../x.desktop", true).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}
