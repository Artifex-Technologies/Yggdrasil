//! What Settings > Apps knows about each installed app, and the changes it can make to one.
//!
//! The apps are the visible desktop entries. When the operating system has an app launcher with per-app rules (its `apps`
//! integration, docs/INTEGRATIONS.md), the choices live in two places: the network rule in its rules file (written by its network
//! command, with a password) and everything else in its settings folder as ID.json (read by its launcher when it starts the app).

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
    /// Every new destination is held until you answer a notification.
    Ask,
}

impl NetMode {
    pub fn word(self) -> &'static str {
        match self {
            NetMode::Allow => "allow",
            NetMode::Block => "block",
            NetMode::Ethernet => "ethernet",
            NetMode::Wifi => "wifi",
            NetMode::Ask => "ask",
        }
    }
    pub fn from_word(w: &str) -> NetMode {
        match w {
            "block" => NetMode::Block,
            "ethernet" => NetMode::Ethernet,
            "wifi" => NetMode::Wifi,
            "ask" => NetMode::Ask,
            _ => NetMode::Allow,
        }
    }
}

/// The sandbox choices for an app (read by the operating system's launcher).
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
    /// How long it has been running in total (the launcher's usage file says).
    pub seconds: u64,
    pub net: NetMode,
    /// A technical tool (settings panels, network helpers) rather than an app people open, hidden unless asked for.
    pub system: bool,
    pub startup: bool,
    pub policy: AppPolicy,
}

/// The operating system's app launcher and app rules, if it has them.
pub fn rules() -> Option<&'static yggdrasil_conf::integrations::Integration> {
    static APPS: std::sync::OnceLock<Option<yggdrasil_conf::integrations::Integration>> = std::sync::OnceLock::new();
    APPS.get_or_init(|| yggdrasil_conf::integrations::first("apps")).as_ref()
}

/// The prefix the launcher gives each app's slice and scopes (`PREFIXid.slice`, `PREFIXid_N.scope`), when it declares one.
fn unit_prefix() -> Option<&'static str> {
    rules()?.get("unit_prefix").filter(|p| p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
}

/// Whether apps can have their own network rules here (the launcher declares a network command).
pub fn has_network_rules() -> bool {
    rules().and_then(|r| r.path("network")).is_some()
}

/// Whether apps can run in the launcher's sandbox (it declares a settings folder).
pub fn has_sandbox() -> bool {
    rules().and_then(|r| r.path("settings")).is_some()
}

/// Starts an app: through the operating system's launcher when there is one (its rules then apply), else as the desktop entry says.
pub fn launch(a: &AppInfo) -> Result<(), String> {
    if let Some(mut cmd) = rules().and_then(|r| r.command("launch")) {
        cmd.push(a.id.clone());
        if Command::new(&cmd[0]).args(&cmd[1..]).status().is_ok_and(|s| s.success()) {
            return Ok(());
        }
    }
    let st = Command::new("gio").arg("launch").arg(&a.desktop).status().map_err(|e| format!("could not start it: {e}"))?;
    if st.success() { Ok(()) } else { Err("could not start it".into()) }
}

/// Changes an app's network rule with the launcher's network command (asks for the password).
pub fn set_network(id: &str, mode: NetMode) -> Result<(), String> {
    let net = rules().and_then(|r| r.path("network")).ok_or("there are no network rules on this system")?;
    if let Some(slice) = slice_name(id) {
        let _ = Command::new("systemctl").args(["--user", "start", &slice]).status();
    }
    crate::sys::act("pkexec", &[&net.to_string_lossy(), "set", id, mode.word()])
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

pub fn desktop_dirs() -> Vec<PathBuf> {
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
    for t in [theme, "Liki", "hicolor"] {
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

/// The slice the launcher gives an app (the same rule as the launcher and its network command).
pub fn slice_name(id: &str) -> Option<String> {
    unit_prefix().map(|p| slice_name_with(p, id))
}

fn slice_name_with(prefix: &str, id: &str) -> String {
    format!("{prefix}{}.slice", id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect::<String>())
}

/// Which of `units` belong to the app `id`: the launcher's scopes (`PREFIX_ID_N.scope`), KDE's (`app-ID-PID.scope` or
/// `app-ID@HASH.scope`), and Flatpak's (`app-flatpak-ID-N.scope`).
pub fn units_of(id: &str, units: &[String]) -> Vec<String> {
    units_of_with(unit_prefix(), id, units)
}

fn units_of_with(prefix: Option<&str>, id: &str, units: &[String]) -> Vec<String> {
    let san: String = id.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    let ours = prefix.map(|p| format!("{p}{san}_"));
    let digits_then_scope = |rest: &str| {
        let r = rest.strip_suffix(".scope").unwrap_or(rest);
        !r.is_empty() && r.chars().all(|c| c.is_ascii_digit())
    };
    units
        .iter()
        .filter(|u| {
            if let Some(rest) = ours.as_deref().and_then(|o| u.strip_prefix(o)) {
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

fn policy_path(dir: &Path, id: &str) -> PathBuf {
    dir.join(format!("{id}.json"))
}

pub fn load_policy(id: &str) -> AppPolicy {
    rules().and_then(|r| r.path("settings")).map(|d| load_policy_in(&d, id)).unwrap_or_default()
}

pub fn save_policy(id: &str, p: &AppPolicy) -> Result<(), String> {
    save_policy_in(&rules().and_then(|r| r.path("settings")).ok_or("there is no sandbox on this system")?, id, p)
}

fn load_policy_in(dir: &Path, id: &str) -> AppPolicy {
    let v = read_json(&policy_path(dir, id));
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

fn save_policy_in(dir: &Path, id: &str, p: &AppPolicy) -> Result<(), String> {
    let path = policy_path(dir, id);
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    let v = json!({"sandbox": p.sandbox, "files": if p.files_all { "all" } else { "folders" }, "folders": p.folders, "see_others": p.see_others});
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}

/// Network modes from the system's rules file, for this user: app id to mode.
pub fn load_net_rules() -> HashMap<String, NetMode> {
    let Some(file) = rules().and_then(|r| r.path("network_rules")) else { return HashMap::new() };
    let v = read_json(&file);
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
    let Some(file) = rules().and_then(|r| r.path("usage")) else { return HashMap::new() };
    let v = read_json(&file);
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

/// Packages Settings will not uninstall, because removing them would break the system: the desktop's own (Yggdrasil, its icons, the
/// Hyprland compositor under it, the greetd login, the portals and sound it runs on), the system's core, and what the operating system lists in
/// /etc/yggdrasil/protected-packages (one name per line; `name*` covers every package starting with `name`).
pub fn protected(pkg: &str) -> bool {
    let os_list = std::fs::read_to_string(yggdrasil_conf::dirs::system_config().join("protected-packages")).unwrap_or_default();
    protected_with(pkg, &os_list)
}

fn protected_with(pkg: &str, os_list: &str) -> bool {
    let listed = os_list.lines().map(|l| l.split('#').next().unwrap_or("").trim()).filter(|l| !l.is_empty()).any(|l| match l.strip_suffix('*') {
        Some(prefix) => pkg.starts_with(prefix),
        None => pkg == l,
    });
    listed
        || pkg.starts_with("yggdrasil-")
        || pkg.starts_with("liki-")
        || pkg.starts_with("hypr")
        || pkg.starts_with("greetd")
        || pkg.starts_with("xdg-desktop-portal")
        || matches!(pkg, "aquamarine" | "pipewire" | "wireplumber" | "pacman" | "systemd" | "networkmanager" | "polkit" | "bash")
}

#[cfg(test)]
mod tests {
    #[test]
    fn ask_decisions_are_read_allowed_first() {
        let v: Value = serde_json::from_str(r#"{"type":"list","apps":{"pdfcraft":{"allow":{"9.9.9.9":"dns.example","1.1.1.1":""},"deny":{"5.5.5.5":""}},"x":{"allow":{},"deny":{}}}}"#).unwrap();
        let d = parse_decisions(&v, "pdfcraft");
        assert_eq!(d.iter().map(|x| (x.ip.as_str(), x.allowed)).collect::<Vec<_>>(), vec![("1.1.1.1", true), ("9.9.9.9", true), ("5.5.5.5", false)]);
        assert_eq!(d[1].name, "dns.example");
        assert!(parse_decisions(&v, "missing").is_empty());
        assert_eq!(NetMode::from_word("ask"), NetMode::Ask);
        assert_eq!(NetMode::Ask.word(), "ask");
    }

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
        let of = |id: &str| units_of_with(Some("lapp_"), id, &units);
        assert_eq!(of("firefox"), vec!["app-firefox-1234.scope"]);
        assert_eq!(of("firefox-nightly"), vec!["app-firefox-nightly-99.scope"]);
        assert_eq!(of("spotify-launcher"), vec!["lapp_spotify_launcher_5.scope"]);
        assert_eq!(of("com.microsoft.VSCode").len(), 1);
        assert_eq!(of("org.gimp.GIMP").len(), 1);
        assert_eq!(of("foo").len(), 1);
        assert!(of("bar").is_empty());
        assert!(units_of_with(None, "spotify-launcher", &units).is_empty(), "without a launcher only the standard scopes count");
    }

    #[test]
    fn slice_names_match_the_launcher() {
        assert_eq!(slice_name_with("lapp_", "com.mitchellh.ghostty"), "lapp_com_mitchellh_ghostty.slice");
        assert_eq!(slice_name_with("lapp_", "spotify-launcher"), "lapp_spotify_launcher.slice");
    }

    #[test]
    fn policy_round_trips_through_the_file_the_launcher_reads() {
        let _g = ENV.lock().unwrap_or_else(|e| e.into_inner());
        let d = std::env::temp_dir().join(format!("lf-appmgr-{}", std::process::id()));
        assert_eq!(load_policy_in(&d, "demo"), AppPolicy::default());
        let p = AppPolicy { sandbox: true, files_all: false, folders: vec!["/home/x/Documents".into()], see_others: false };
        save_policy_in(&d, "demo", &p).unwrap();
        assert_eq!(load_policy_in(&d, "demo"), p);
        let v: Value = serde_json::from_str(&std::fs::read_to_string(policy_path(&d, "demo")).unwrap()).unwrap();
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
        assert!(protected("yggdrasil-minos") && protected("liki-icon-theme") && protected("hyprland") && protected("greetd") && !protected("firefox"));
        assert!(super::protected_with("xdg-desktop-portal-hyprland", "") && super::protected_with("pipewire", ""));
        assert!(!super::protected_with("plasma-desktop", "") && !super::protected_with("kwin", ""), "KDE is gone: its packages are ordinary ones");
        let os = "# the operating system's own\nlabyrinth-*\nsarina-git\n";
        assert!(super::protected_with("labyrinth-ollama-tuning", os) && super::protected_with("sarina-git", os) && !super::protected_with("sarina", os));
        assert!(!super::protected_with("labyrinth-ollama-tuning", ""));
    }
}

/// A destination you decided about for an app in "Ask" mode.
#[derive(Clone, Debug, PartialEq)]
pub struct Decision {
    pub ip: String,
    pub name: String,
    pub allowed: bool,
}

fn ask_call(request: &str) -> Option<Value> {
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::net::UnixStream;
    use std::time::Duration;
    let mut s = UnixStream::connect(rules()?.path("network_ask")?).ok()?;
    s.set_read_timeout(Some(Duration::from_millis(800))).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    s.write_all(b"\n").ok()?;
    let mut line = String::new();
    BufReader::new(s).read_line(&mut line).ok()?;
    serde_json::from_str(&line).ok()
}

/// The text of what the ask service answers to {"op":"list"}, as one app's decisions: allowed ones first, then by address.
pub fn parse_decisions(reply: &Value, app: &str) -> Vec<Decision> {
    let mut out = vec![];
    let Some(a) = reply["apps"][app].as_object() else { return out };
    for (key, allowed) in [("allow", true), ("deny", false)] {
        if let Some(m) = a.get(key).and_then(|m| m.as_object()) {
            for (ip, name) in m {
                out.push(Decision { ip: ip.clone(), name: name.as_str().unwrap_or("").to_string(), allowed });
            }
        }
    }
    out.sort_by(|x, y| y.allowed.cmp(&x.allowed).then(x.ip.cmp(&y.ip)));
    out
}

/// Asks the Ask service (a local socket, no password) what you decided for an app. Empty when the service is not running.
pub fn ask_decisions(app: &str) -> Vec<Decision> {
    ask_call(r#"{"op":"list"}"#).map(|v| parse_decisions(&v, app)).unwrap_or_default()
}

/// Forget one decision: that destination is asked about again next time.
pub fn ask_forget(app: &str, ip: &str) {
    let req = json!({"op": "forget", "app": app, "ip": ip}).to_string();
    let _ = ask_call(&req);
}
