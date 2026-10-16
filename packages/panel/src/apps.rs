//! Installed applications (desktop entries) and the dock's list of pinned apps.

use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default)]
pub struct App {
    /// The desktop file name without ".desktop" (also what KWin reports as a window's app id).
    pub id: String,
    pub name: String,
    pub icon: String,
    pub exec: String,
    pub wm_class: String,
    pub no_display: bool,
    /// The desktop entry's Categories (Utility, Network, ...).
    pub categories: Vec<String>,
    pub path: PathBuf,
}

/// Parse the `[Desktop Entry]` group of a desktop file.
pub fn parse_entry(id: &str, text: &str, path: &Path) -> Option<App> {
    let mut app = App { id: id.to_string(), path: path.to_path_buf(), ..Default::default() };
    let mut in_main = false;
    let mut kind_ok = false;
    let mut only_in = String::new();
    let mut not_in = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_main = line == "[Desktop Entry]";
            continue;
        }
        if !in_main || line.starts_with('#') {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "Name" => app.name = v.to_string(),
            "Icon" => app.icon = v.to_string(),
            "Exec" => app.exec = v.to_string(),
            "StartupWMClass" => app.wm_class = v.to_string(),
            "NoDisplay" => app.no_display = v == "true",
            "Categories" => app.categories = v.split(';').filter(|c| !c.is_empty()).map(str::to_string).collect(),
            "OnlyShowIn" => only_in = v.to_lowercase(),
            "NotShowIn" => not_in = v.to_lowercase(),
            "Hidden" if v == "true" => return None,
            "Type" => kind_ok = v == "Application",
            _ => {}
        }
    }
    // Apps that are for other desktops (GNOME-only tools, Avahi browsers) are not listed, as in KDE's menu.
    let has = |list: &str| list.split(';').any(|d| d == "kde");
    if (!only_in.is_empty() && !has(&only_in)) || has(&not_in) {
        app.no_display = true;
    }
    (kind_ok && !app.name.is_empty()).then_some(app)
}

pub fn load_all() -> Vec<App> {
    let mut dirs = vec![PathBuf::from("/usr/share/applications"), PathBuf::from("/usr/local/share/applications")];
    if let Some(h) = std::env::var_os("HOME") {
        dirs.push(PathBuf::from(h).join(".local/share/applications"));
    }
    let mut apps: Vec<App> = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().map(|x| x != "desktop").unwrap_or(true) {
                continue;
            }
            let id = p.file_stem().unwrap().to_string_lossy().to_string();
            if let Some(a) = std::fs::read_to_string(&p).ok().and_then(|t| parse_entry(&id, &t, &p)) {
                apps.retain(|x| x.id != a.id); // later directories (the user's) win
                apps.push(a);
            }
        }
    }
    apps
}

/// The start menu's folder for an app, from its freedesktop categories (the first kind that fits).
pub fn folder_of(a: &App) -> &'static str {
    const MAP: [(&str, &str); 12] = [
        ("Game", "Games"),
        ("AudioVideo", "Multimedia"),
        ("Audio", "Multimedia"),
        ("Video", "Multimedia"),
        ("Graphics", "Graphics"),
        ("Office", "Office"),
        ("Development", "Development"),
        ("Education", "Education"),
        ("Science", "Science"),
        ("Network", "Internet"),
        ("Settings", "Settings"),
        ("System", "System"),
    ];
    for (cat, folder) in MAP {
        if a.categories.iter().any(|c| c == cat) {
            return folder;
        }
    }
    if a.categories.iter().any(|c| c == "Utility") { "Accessories" } else { "Other" }
}

/// The folders to show: each with its apps A to Z; folders A to Z with "Other" last, empty ones left out.
pub fn folders(apps: &[App]) -> Vec<(&'static str, Vec<&App>)> {
    let mut out: Vec<(&'static str, Vec<&App>)> = vec![];
    let mut shown: Vec<&App> = apps.iter().filter(|a| !a.no_display).collect();
    shown.sort_by_key(|a| a.name.to_lowercase());
    shown.dedup_by(|a, b| a.name == b.name);
    for a in shown {
        let f = folder_of(a);
        match out.iter_mut().find(|(n, _)| *n == f) {
            Some((_, v)) => v.push(a),
            None => out.push((f, vec![a])),
        }
    }
    out.sort_by_key(|(n, _)| (*n == "Other", *n));
    out
}

/// The app that goes with a window's app id.
pub fn for_app_id<'a>(apps: &'a [App], app_id: &str) -> Option<&'a App> {
    if app_id.is_empty() {
        return None;
    }
    let lower = app_id.to_lowercase();
    apps.iter()
        .find(|a| a.id == app_id)
        .or_else(|| apps.iter().find(|a| a.id.to_lowercase() == lower))
        .or_else(|| apps.iter().find(|a| !a.wm_class.is_empty() && a.wm_class.to_lowercase() == lower))
}

/// The command from an Exec line with the field codes (%u, %F ...) removed.
pub fn command(exec: &str) -> Vec<String> {
    let mut out = vec![];
    let mut cur = String::new();
    let mut quote = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => quote = !quote,
            '\\' => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            '%' => {
                if let Some(n) = chars.peek().copied() {
                    chars.next();
                    if n == '%' {
                        cur.push('%');
                    }
                }
            }
            ' ' if !quote => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn launch(app: &App) {
    // Through labyrinth-launch when it is there: the app then runs in its own slice (per-app network rules), is
    // counted for Settings > Apps, and follows its sandbox settings.
    if !app.id.is_empty() {
        let started = std::process::Command::new("labyrinth-launch").arg(&app.id).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false);
        if started {
            return;
        }
    }
    let cmd = command(&app.exec);
    let Some((prog, args)) = cmd.split_first() else { return };
    let _ = std::process::Command::new("setsid").arg("-f").arg(prog).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

/// The dock's pinned apps, in order: ~/.config/labyrinth/dock (one desktop id per line), else the
/// system default.
pub fn pinned() -> Vec<String> {
    let user = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/labyrinth/dock"));
    for p in [user, Some(PathBuf::from("/usr/share/labyrinth/dock"))].into_iter().flatten() {
        if let Ok(t) = std::fs::read_to_string(p) {
            let v: Vec<String> = t.lines().map(|l| l.trim().trim_end_matches(".desktop").replace("finder", "minos")).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
            if !v.is_empty() {
                return v;
            }
        }
    }
    vec![]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apps_are_sorted_into_folders() {
        let mk = |name: &str, cats: &str| App { id: name.into(), name: name.into(), categories: cats.split(';').filter(|c| !c.is_empty()).map(str::to_string).collect(), ..Default::default() };
        let apps = vec![mk("Zed", "Development;TextEditor;"), mk("Browser", "Network;WebBrowser;"), mk("Amp", "AudioVideo;Audio;"), mk("Calc", "Utility;"), mk("Mystery", ""), mk("Chess", "Game;BoardGame;")];
        let f = folders(&apps);
        let names: Vec<&str> = f.iter().map(|(n, _)| *n).collect();
        assert_eq!(names, ["Accessories", "Development", "Games", "Internet", "Multimedia", "Other"]);
        assert_eq!(folder_of(&apps[2]), "Multimedia");
        let mut hidden = mk("Hidden", "Utility;");
        hidden.no_display = true;
        assert!(folders(&[hidden]).is_empty());
    }

    #[test]
    fn parses_a_desktop_entry() {
        let t = "[Desktop Entry]\nType=Application\nName=Minos\nIcon=minos\nExec=minos %U\nStartupWMClass=minos\n[Desktop Action x]\nName=Other\n";
        let a = parse_entry("minos", t, Path::new("/x/minos.desktop")).unwrap();
        assert_eq!((a.name.as_str(), a.icon.as_str(), a.exec.as_str()), ("Minos", "minos", "minos %U"));
        let gnome = "[Desktop Entry]
Type=Application
Name=G
OnlyShowIn=GNOME;
";
        let not_kde = "[Desktop Entry]
Type=Application
Name=N
NotShowIn=KDE;GNOME;
";
        let both = "[Desktop Entry]
Type=Application
Name=B
OnlyShowIn=KDE;GNOME;
";
        assert!(parse_entry("g", gnome, Path::new("/g")).unwrap().no_display);
        assert!(parse_entry("n", not_kde, Path::new("/n")).unwrap().no_display);
        assert!(!parse_entry("b", both, Path::new("/b")).unwrap().no_display);
        assert!(parse_entry("h", "[Desktop Entry]\nType=Application\nName=H\nHidden=true\n", Path::new("/h")).is_none());
        assert!(parse_entry("l", "[Desktop Entry]\nType=Link\nName=L\n", Path::new("/l")).is_none());
    }

    #[test]
    fn exec_field_codes_are_removed() {
        assert_eq!(command("vivaldi-stable %U"), vec!["vivaldi-stable"]);
        assert_eq!(command("env FOO=1 \"/opt/my app/run\" --flag %f"), vec!["env", "FOO=1", "/opt/my app/run", "--flag"]);
        assert_eq!(command("sh -c 'echo 100%%'"), vec!["sh", "-c", "'echo", "100%'"]);
    }

    #[test]
    fn windows_match_apps_by_id_or_class() {
        let apps = vec![
            App { id: "org.kde.konsole".into(), name: "Konsole".into(), ..Default::default() },
            App { id: "vivaldi-stable".into(), name: "Vivaldi".into(), wm_class: "Vivaldi-stable".into(), ..Default::default() },
        ];
        assert_eq!(for_app_id(&apps, "org.kde.konsole").unwrap().name, "Konsole");
        assert_eq!(for_app_id(&apps, "vivaldi-stable").unwrap().name, "Vivaldi");
        assert_eq!(for_app_id(&apps, "VIVALDI-STABLE").unwrap().name, "Vivaldi");
        assert!(for_app_id(&apps, "nope").is_none());
        assert!(for_app_id(&apps, "").is_none());
    }
}
