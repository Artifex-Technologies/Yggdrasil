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
    pub path: PathBuf,
}

/// Parse the `[Desktop Entry]` group of a desktop file.
pub fn parse_entry(id: &str, text: &str, path: &Path) -> Option<App> {
    let mut app = App { id: id.to_string(), path: path.to_path_buf(), ..Default::default() };
    let mut in_main = false;
    let mut kind_ok = false;
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
            "Hidden" if v == "true" => return None,
            "Type" => kind_ok = v == "Application",
            _ => {}
        }
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
            let v: Vec<String> = t.lines().map(|l| l.trim().trim_end_matches(".desktop").to_string()).filter(|l| !l.is_empty() && !l.starts_with('#')).collect();
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
    fn parses_a_desktop_entry() {
        let t = "[Desktop Entry]\nType=Application\nName=Finder\nIcon=finder\nExec=finder %U\nStartupWMClass=finder\n[Desktop Action x]\nName=Other\n";
        let a = parse_entry("finder", t, Path::new("/x/finder.desktop")).unwrap();
        assert_eq!((a.name.as_str(), a.icon.as_str(), a.exec.as_str()), ("Finder", "finder", "finder %U"));
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
