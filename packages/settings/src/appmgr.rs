//! What Settings > Apps knows about each installed app, and the changes it can make to one.
//!
//! The apps are the visible desktop entries. Per-app choices live in two places: the network rule in
//! /etc/labyrinth/app-network.json (written by labyrinth-appnet, with a password) and everything else in
//! ~/.config/labyrinth/apps/ID.json (read by labyrinth-launch when it starts the app). See those scripts.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum NetMode {
    #[default]
    Allow,
    Block,
    Ethernet,
    Wifi,
}

impl NetMode {
    pub fn word(self) -> &'static str {
        match self {
            NetMode::Allow => "allow",
            NetMode::Block => "block",
            NetMode::Ethernet => "ethernet",
            NetMode::Wifi => "wifi",
        }
    }
    pub fn from_word(w: &str) -> NetMode {
        match w {
            "block" => NetMode::Block,
            "ethernet" => NetMode::Ethernet,
            "wifi" => NetMode::Wifi,
            _ => NetMode::Allow,
        }
    }
}

/// The sandbox choices for an app (read by labyrinth-launch).
#[derive(Clone, Debug, PartialEq)]
pub struct AppPolicy {
    pub sandbox: bool,
    /// true: the app sees all your files; false: only `folders` (and its own settings folders).
    pub files_all: bool,
    pub folders: Vec<String>,
    /// false: the app cannot see other programs running on the computer.
    pub see_others: bool,
}

impl Default for AppPolicy {
    fn default() -> Self {
        AppPolicy { sandbox: false, files_all: true, folders: vec![], see_others: true }
    }
}

#[derive(Clone, Debug, Default)]
pub struct AppInfo {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub icon: String,
    pub icon_path: Option<PathBuf>,
    pub desktop: PathBuf,
    pub exec: String,
    pub flatpak: bool,
    /// systemd scopes the app is running in (empty: not running, or not started the way Settings can see).
    pub running: Vec<String>,
    pub launches: u64,
    pub last_used: u64,
    /// How long it has been running in total (labyrinth-appusage counts this).
    pub seconds: u64,
    pub net: NetMode,
    /// A technical tool (settings panels, network helpers) rather than an app people open, hidden unless asked for.
    pub system: bool,
    pub startup: bool,
    pub policy: AppPolicy,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
}

fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".config"))
}

fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).unwrap_or_else(|| home().join(".local/share"))
}

/// The keys of a `[Desktop Entry]` section (first value of a key wins; translated keys are skipped).
pub fn parse_desktop(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut inside = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            inside = l == "[Desktop Entry]";
        } else if inside && !l.starts_with('#') {
            if let Some((k, v)) = l.split_once('=') {
                let k = k.trim();
                if !k.contains('[') {
                    out.entry(k.to_string()).or_insert_with(|| v.trim().to_string());
                }
            }
        }
    }
    out
}

fn desktop_dirs() -> Vec<PathBuf> {
    let mut v = vec![data_home().join("applications")];
    for d in std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into()).split(':') {
        if !d.is_empty() {
            v.push(PathBuf::from(d).join("applications"));
        }
    }
    v.push(data_home().join("flatpak/exports/share/applications"));
    v.push(PathBuf::from("/var/lib/flatpak/exports/share/applications"));
    let mut seen = HashSet::new();
    v.retain(|p| seen.insert(p.clone()));
    v
}

/// Find an icon file by name in the chosen pack and the ones behind it.
pub fn icon_path(name: &str, theme: &str) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    if name.starts_with('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    let mut themes: Vec<String> = vec![];
    for t in [theme, "Labyrinth", "MacTahoe-purple-dark", "hicolor", "breeze"] {
        if !t.is_empty() && !themes.iter().any(|x| x == t) {
            themes.push(t.to_string());
        }
    }
    let roots = [data_home().join("icons"), PathBuf::from("/usr/share/icons")];
    for root in &roots {
        for t in &themes {
            let base = root.join(t);
            for size in ["scalable", "512", "256", "128", "96", "64", "48", "32"] {
                for dir in [base.join("apps").join(size), base.join(size).join("apps"), base.join(format!("{size}x{size}")).join("apps")] {
                    for ext in ["svg", "png"] {
                        let p = dir.join(format!("{name}.{ext}"));
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    for ext in ["svg", "png"] {
        let p = PathBuf::from(format!("/usr/share/pixmaps/{name}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// The running scopes in this user's systemd, unescaped (`\x2d` back to `-`).
pub fn running_units() -> Vec<String> {
    let out = Command::new("systemctl").args(["--user", "list-units", "--type=scope", "--no-legend", "--plain", "--no-pager"]).output();
    let Ok(out) = out else { return vec![] };
    String::from_utf8_lossy(&out.stdout).lines().filter_map(|l| l.split_whitespace().next()).map(|u| u.replace("\\x2d", "-")).collect()
}

/// The slice labyrinth-launch gives an app (same rule as the launcher and labyrinth-appnet).
pub fn slice_name(id: &str) -> String {
    format!("lapp_{}.slice", id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>())
}

/// Which of `units` belong to the app `id`: our own scopes (`lapp_ID_N.scope`), KDE's (`app-ID-PID.scope` or
/// `app-ID@HASH.scope`), and Flatpak's (`app-flatpak-ID-N.scope`).
pub fn units_of(id: &str, units: &[String]) -> Vec<String> {
    let san: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let ours = format!("lapp_{san}_");
    let digits_then_scope = |rest: &str| {
        let r = rest.strip_suffix(".scope").unwrap_or(rest);
        !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())
    };
    units
        .iter()
        .filter(|u| {
            if let Some(rest) = u.strip_prefix(&ours) {
                return digits_then_scope(rest);
            }
            for prefix in [format!("app-{id}"), format!("app-flatpak-{id}")] {
                if let Some(rest) = u.strip_prefix(&prefix) {
                    if let Some(r) = rest.strip_prefix('-') {
                        if digits_then_scope(r) {
                            return true;
                        }
                    }
                    if rest.starts_with('@') && rest.ends_with(".scope") {
                        return true;
                    }
                }
            }
            false
        })
        .cloned()
        .collect()
}

fn read_json(path: &Path) -> Value {
    std::fs::read_to_string(path).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or(Value::Null)
}

pub fn policy_path(id: &str) -> PathBuf {
    config_home().join("labyrinth/apps").join(format!("{id}.json"))
}

pub fn load_policy(id: &str) -> AppPolicy {
    let v = read_json(&policy_path(id));
    let mut p = AppPolicy::default();
    if let Some(b) = v["sandbox"].as_bool() {
        p.sandbox = b;
    }
    p.files_all = v["files"].as_str() != Some("folders");
    if let Some(a) = v["folders"].as_array() {
        p.folders = a.iter().filter_map(|x| x.as_str().map(String::from)).collect();
    }
    if let Some(b) = v["see_others"].as_bool() {
        p.see_others = b;
    }
    p
}

pub fn save_policy(id: &str, p: &AppPolicy) -> Result<(), String> {
    let path = policy_path(id);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let v = json!({"sandbox": p.sandbox, "files": if p.files_all { "all" } else { "folders" }, "folders": p.folders, "see_others": p.see_others});
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Network modes from the system's rules file, for this user: app id to mode.
pub fn load_net_rules() -> HashMap<String, NetMode> {
    let v = read_json(Path::new(&std::env::var("LABYRINTH_APPNET_STATE").unwrap_or_else(|_| "/etc/labyrinth/app-network.json".into())));
    let uid = unsafe { libc_getuid() }.to_string();
    v["rules"][&uid].as_object().map(|m| m.iter().filter_map(|(k, x)| x.as_str().map(|s| (k.clone(), NetMode::from_word(s)))).collect()).unwrap_or_default()
}

// getuid without a libc dependency
unsafe fn libc_getuid() -> u32 {
    extern "C" {
        fn getuid() -> u32;
    }
    getuid()
}

pub fn load_usage() -> HashMap<String, (u64, u64, u64)> {
    let v = read_json(&data_home().join("labyrinth/app-usage.json"));
    v.as_object().map(|m| m.iter().map(|(k, e)| (k.clone(), (e["launches"].as_u64().unwrap_or(0), e["last"].as_u64().unwrap_or(0), e["seconds"].as_u64().unwrap_or(0)))).collect()).unwrap_or_default()
}

/// Whether the app starts when you log in: your own autostart entry decides, else the system's.
pub fn startup_enabled(id: &str) -> bool {
    let own = config_home().join("autostart").join(format!("{id}.desktop"));
    let sys = PathBuf::from("/etc/xdg/autostart").join(format!("{id}.desktop"));
    for p in [own, sys] {
        if let Ok(t) = std::fs::read_to_string(&p) {
            let d = parse_desktop(&t);
            let hidden = d.get("Hidden").map(|h| h.eq_ignore_ascii_case("true")).unwrap_or(false);
            let off = d.get("X-GNOME-Autostart-enabled").map(|h| h.eq_ignore_ascii_case("false")).unwrap_or(false);
            return !(hidden || off);
        }
    }
    false
}

/// Turn "start at login" on or off for an app (a file in ~/.config/autostart).
pub fn set_startup(app: &AppInfo, on: bool) -> Result<(), String> {
    let dir = config_home().join("autostart");
    let path = dir.join(format!("{}.desktop", app.id));
    if on {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let body = std::fs::read_to_string(&app.desktop).map_err(|e| e.to_string())?;
        let mut out: Vec<String> = body.lines().filter(|l| !l.starts_with("Hidden=") && !l.starts_with("X-GNOME-Autostart-enabled=")).map(String::from).collect();
        if let Some(i) = out.iter().position(|l| l.trim() == "[Desktop Entry]") {
            out.insert(i + 1, "X-GNOME-Autostart-enabled=true".into());
        }
        std::fs::write(&path, out.join("\n") + "\n").map_err(|e| e.to_string())
    } else {
        // a file with Hidden=true switches off even a system-wide autostart entry
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        std::fs::write(&path, format!("[Desktop Entry]\nType=Application\nName={}\nHidden=true\n", app.name)).map_err(|e| e.to_string())
    }
}

/// Visible apps from the desktop entries, with what Settings shows about each. Sorted by name.
pub fn list_apps(icon_theme: &str) -> Vec<AppInfo> {
    let units = running_units();
    let usage = load_usage();
    let rules = load_net_rules();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out = vec![];
    for dir in desktop_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        let flat = dir.to_string_lossy().contains("flatpak");
        for e in rd.flatten() {
            let path = e.path();
            if path.extension().map(|x| x != "desktop").unwrap_or(true) {
                continue;
            }
            let id = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            if id.is_empty() || seen.contains(&id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let d = parse_desktop(&text);
            let truthy = |k: &str| d.get(k).map(|v| v.eq_ignore_ascii_case("true")).unwrap_or(false);
            if d.get("Type").map(|t| t != "Application").unwrap_or(true) || truthy("NoDisplay") || truthy("Hidden") || d.get("Name").is_none() {
                continue;
            }
            seen.insert(id.clone());
            let icon = d.get("Icon").cloned().unwrap_or_default();
            let (launches, last_used, seconds) = usage.get(&id).copied().unwrap_or((0, 0, 0));
            out.push(AppInfo {
                name: d.get("Name").cloned().unwrap_or_else(|| id.clone()),
                comment: d.get("Comment").cloned().unwrap_or_default(),
                icon_path: icon_path(&icon, icon_theme),
                icon,
                desktop: path.clone(),
                exec: d.get("Exec").cloned().unwrap_or_default(),
                flatpak: flat || d.contains_key("X-Flatpak"),
                running: units_of(&id, &units),
                launches,
                last_used,
                seconds,
                net: rules.get(&id).copied().unwrap_or_default(),
                system: d.get("Categories").map(|c| c.split(';').any(|x| matches!(x, "Settings" | "DesktopSettings" | "HardwareSettings"))).unwrap_or(false) || id.starts_with("avahi") || id.starts_with("bssh") || id.starts_with("bvnc") || id.starts_with("qv4l2") || id.starts_with("qvidcap"),
                startup: startup_enabled(&id),
                policy: load_policy(&id),
                id,
            });
        }
    }
    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// ~/.cache: safe to clear, the app rebuilds it.
    Cache,
    /// ~/.config, ~/.local/share, ~/.var/app: the app's settings and saved data.
    Data,
}

/// The folders in the home folder that belong to the app, by the usual names (its id, name, program, window class).
pub fn data_dirs(app: &AppInfo) -> Vec<(PathBuf, Kind)> {
    let text = std::fs::read_to_string(&app.desktop).unwrap_or_default();
    let d = parse_desktop(&text);
    let mut names: Vec<String> = vec![app.id.clone(), app.name.clone()];
    if let Some(w) = d.get("StartupWMClass") {
        names.push(w.clone());
    }
    if let Some(first) = app.exec.split_whitespace().next() {
        if let Some(b) = Path::new(first.trim_matches('"')).file_name() {
            names.push(b.to_string_lossy().into_owned());
        }
    }
    let mut all: Vec<String> = vec![];
    for n in names {
        for v in [n.clone(), n.to_lowercase()] {
            if !v.is_empty() && !v.contains('/') && !v.starts_with('.') && !all.contains(&v) {
                all.push(v);
            }
        }
    }
    let mut out: Vec<(PathBuf, Kind)> = vec![];
    for (base, kind) in [(home().join(".cache"), Kind::Cache), (config_home(), Kind::Data), (data_home(), Kind::Data)] {
        for n in &all {
            let p = base.join(n);
            if p.is_dir() && !p.is_symlink() && !out.iter().any(|(q, _)| *q == p) {
                out.push((p, kind));
            }
        }
    }
    let flat = home().join(".var/app").join(&app.id);
    if flat.is_dir() {
        out.push((flat.join("cache"), Kind::Cache));
        out.push((flat.join("config"), Kind::Data));
        out.push((flat.join("data"), Kind::Data));
        out.retain(|(p, _)| p.is_dir());
    }
    out
}

/// Bytes under a folder (links are not followed; it stops counting after a very large amount).
pub fn dir_size(p: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![p.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                stack.push(e.path());
            } else if t.is_file() {
                total += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
            if total > 1 << 42 {
                return total;
            }
        }
    }
    total
}

/// Remove the folders of one kind (only ones that are inside the home folder, and never the home folder itself).
pub fn clear_dirs(app: &AppInfo, kind: Kind) -> Result<usize, String> {
    let h = home();
    let mut n = 0;
    for (p, k) in data_dirs(app) {
        if k != kind {
            continue;
        }
        if !p.starts_with(&h) || p == h || p.components().count() <= h.components().count() + 1 {
            continue;
        }
        std::fs::remove_dir_all(&p).map_err(|e| format!("{}: {e}", p.display()))?;
        n += 1;
    }
    Ok(n)
}

/// The package that owns the app's desktop file (pacman), or the Flatpak id.
pub fn owner(app: &AppInfo) -> Option<(String, bool)> {
    if app.flatpak {
        return Some((app.id.clone(), true));
    }
    let out = Command::new("pacman").args(["-Qqo"]).arg(&app.desktop).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some((String::from_utf8_lossy(&out.stdout).trim().to_string(), false)).filter(|(p, _)| !p.is_empty())
}

/// Packages the desktop is made of: removing them would break the system, so Settings does not offer to.
pub fn protected(pkg: &str) -> bool {
    pkg.starts_with("labyrinth-")
        || pkg.starts_with("plasma-")
        || pkg.starts_with("kde")
        || pkg.starts_with("kwin")
        || pkg.starts_with("systemsettings")
        || matches!(pkg, "minos" | "sddm" | "pacman" | "systemd" | "networkmanager" | "xdg-desktop-portal" | "polkit" | "bash")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ENV_LOCK as ENV;

    #[test]
    fn desktop_entries_parse_and_skip_translations() {
        let d = parse_desktop("[Desktop Entry]\nName=Demo\nName[de]=Beispiel\nExec=demo %U\n# c\n[Desktop Action x]\nName=Other\n");
        assert_eq!(d["Name"], "Demo");
        assert_eq!(d["Exec"], "demo %U");
        assert!(!d.contains_key("Name[de]"));
    }

    #[test]
    fn running_scopes_are_matched_to_the_right_app() {
        let units: Vec<String> = ["app-firefox-1234.scope", "app-firefox-nightly-99.scope", "lapp_spotify_launcher_5.scope", "app-com.microsoft.VSCode-1915.scope", "app-flatpak-org.gimp.GIMP-7.scope", "app-foo@abc123.scope", "session-3.scope"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(units_of("firefox", &units), vec!["app-firefox-1234.scope"]);
        assert_eq!(units_of("firefox-nightly", &units), vec!["app-firefox-nightly-99.scope"]);
        assert_eq!(units_of("spotify-launcher", &units), vec!["lapp_spotify_launcher_5.scope"]);
        assert_eq!(units_of("com.microsoft.VSCode", &units).len(), 1);
        assert_eq!(units_of("org.gimp.GIMP", &units).len(), 1);
        assert_eq!(units_of("foo", &units).len(), 1);
        assert!(units_of("bar", &units).is_empty());
    }

    #[test]
    fn slice_names_match_the_launcher() {
        assert_eq!(slice_name("com.mitchellh.ghostty"), "lapp_com_mitchellh_ghostty.slice");
        assert_eq!(slice_name("spotify-launcher"), "lapp_spotify_launcher.slice");
    }

    #[test]
    fn policy_round_trips_through_the_file_the_launcher_reads() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::env::temp_dir().join(format!("lf-appmgr-{}", std::process::id()));
        std::env::set_var("XDG_CONFIG_HOME", &d);
        assert_eq!(load_policy("demo"), AppPolicy::default());
        let p = AppPolicy { sandbox: true, files_all: false, folders: vec!["/home/x/Documents".into()], see_others: false };
        save_policy("demo", &p).unwrap();
        assert_eq!(load_policy("demo"), p);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(policy_path("demo")).unwrap()).unwrap();
        assert_eq!(v["files"], "folders");
        assert_eq!(v["see_others"], false);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn startup_is_a_file_in_autostart_and_can_be_switched_off() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::env::temp_dir().join(format!("lf-appmgr-su-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", d.join("c"));
        let desktop = d.join("demo.desktop");
        std::fs::write(&desktop, "[Desktop Entry]\nType=Application\nName=Demo\nExec=demo\n").unwrap();
        let app = AppInfo { id: "demo".into(), name: "Demo".into(), desktop, ..Default::default() };
        assert!(!startup_enabled("demo"));
        set_startup(&app, true).unwrap();
        assert!(startup_enabled("demo"));
        set_startup(&app, false).unwrap();
        assert!(!startup_enabled("demo"));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn clearing_only_touches_folders_inside_home_and_of_that_kind() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let h = std::env::temp_dir().join(format!("lf-appmgr-home-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(h.join(".cache/demo")).unwrap();
        std::fs::create_dir_all(h.join(".config/demo")).unwrap();
        std::fs::write(h.join(".cache/demo/a"), "x").unwrap();
        std::fs::write(h.join(".config/demo/b"), "yy").unwrap();
        std::env::set_var("HOME", &h);
        std::env::remove_var("XDG_CONFIG_HOME");
        std::env::remove_var("XDG_DATA_HOME");
        let app = AppInfo { id: "demo".into(), name: "Demo".into(), desktop: h.join("none.desktop"), ..Default::default() };
        let dirs = data_dirs(&app);
        assert!(dirs.iter().any(|(p, k)| p.ends_with(".cache/demo") && *k == Kind::Cache));
        assert_eq!(dir_size(&h.join(".config/demo")), 2);
        assert_eq!(clear_dirs(&app, Kind::Cache).unwrap(), 1);
        assert!(!h.join(".cache/demo").exists() && h.join(".config/demo").exists());
        let _ = std::fs::remove_dir_all(&h);
    }

    #[test]
    fn desktop_packages_are_protected() {
        assert!(protected("labyrinth-minos") && protected("plasma-desktop") && protected("kwin") && !protected("firefox"));
    }
}
