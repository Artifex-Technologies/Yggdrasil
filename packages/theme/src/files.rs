//! The files of this user that ygg-theme writes, as the Python script wrote them: appearance.conf (the desktop's own record),
//! GTK 3 and 4's settings.ini and gtk.css, xsettingsd's file, the pointer's environment.d file, and only where a program
//! built on KDE's libraries is installed, kdeglobals, kcminputrc and the colour scheme file.

use std::fs;
use std::path::Path;

use crate::colors::{gtk_colors, rgb, scheme, Rgb};

pub const DEFAULT_CURSOR: &str = "Yggdrasil";
pub const CURSOR_SIZE: i32 = 24;

/// Where the script stopped with a traceback.
#[derive(Debug)]
pub struct Crash(pub String);

pub type R<T> = Result<T, Crash>;

fn env(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

/// os.path.join(a, b) (b not absolute)
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

fn dirname(p: &str) -> String {
    match p.rfind('/') {
        None => String::new(),
        Some(i) => {
            let head = &p[..i + 1];
            let t = head.trim_end_matches('/');
            if t.is_empty() {
                head.to_string()
            } else {
                t.to_string()
            }
        }
    }
}

pub fn home() -> String {
    if let Some(h) = env("HOME") {
        return h;
    }
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            return std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        }
    }
    "~".into()
}

pub fn config() -> String {
    env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).unwrap_or_else(|| join(&home(), ".config"))
}

pub fn data() -> String {
    env("XDG_DATA_HOME").filter(|v| !v.is_empty()).unwrap_or_else(|| join(&join(&home(), ".local"), "share"))
}

/// open(path, encoding="utf-8").read() in universal-newline mode: None on an OSError, a Crash when it is not UTF-8.
fn read_text(path: &str) -> R<Option<String>> {
    match fs::read(path) {
        Ok(b) => String::from_utf8(b)
            .map(|t| Some(t.replace("\r\n", "\n").replace('\r', "\n")))
            .map_err(|e| Crash(format!("UnicodeDecodeError: {path}: {e}"))),
        Err(_) => Ok(None),
    }
}

fn write_text(path: &str, text: &str) -> R<()> {
    fs::write(path, text).map_err(|e| Crash(format!("{path}: {e}")))
}

fn makedirs(d: &str) -> R<()> {
    fs::create_dir_all(if d.is_empty() { "." } else { d }).map_err(|e| Crash(format!("{d}: {e}")))
}

/// {group: {key: value}} in order.
pub type Updates = Vec<(String, Vec<(String, String)>)>;

/// Sets many key=value pairs in many [group]s of an INI file in one go, keeping everything else (and the order) as it is.
pub fn ini_update(path: &str, updates: &Updates) -> R<()> {
    let lines: Vec<String> = match read_text(path)? {
        Some(t) => t.split('\n').map(str::to_string).collect(),
        None => vec![],
    };
    // split into (group, [lines]); the part before the first group has group None
    let mut blocks: Vec<(Option<String>, Vec<String>)> = vec![];
    let mut cur: (Option<String>, Vec<String>) = (None, vec![]);
    for line in lines {
        let s = pycompat::strip(&line);
        if s.starts_with('[') && s.ends_with(']') {
            blocks.push(cur);
            let inner: String = {
                let cs: Vec<char> = s.chars().collect();
                if cs.len() >= 2 {
                    cs[1..cs.len() - 1].iter().collect()
                } else {
                    String::new()
                }
            };
            cur = (Some(inner), vec![line.clone()]);
        } else {
            cur.1.push(line);
        }
    }
    blocks.push(cur);
    let mut done: Vec<String> = vec![];
    for blk in blocks.iter_mut() {
        let Some(g) = blk.0.clone() else { continue };
        let Some((_, kv)) = updates.iter().find(|(n, _)| *n == g) else { continue };
        let mut pending: Vec<(String, String)> = kv.clone();
        let mut out: Vec<String> = vec![];
        for line in &blk.1 {
            let k = if line.contains('=') && !pycompat::strip(line).starts_with('#') {
                Some(pycompat::strip(line.split('=').next().unwrap()).to_string())
            } else {
                None
            };
            match k.and_then(|k| pending.iter().position(|(n, _)| *n == k)) {
                Some(i) => {
                    let (k, v) = pending.remove(i);
                    out.push(format!("{k}={v}"));
                }
                None => out.push(line.clone()),
            }
        }
        // new keys go after the last non-empty line of the group
        while out.last().map(|l| pycompat::strip(l).is_empty()).unwrap_or(false) {
            out.pop();
        }
        out.extend(pending.iter().map(|(k, v)| format!("{k}={v}")));
        out.push(String::new());
        blk.1 = out;
        if !done.contains(&g) {
            done.push(g);
        }
    }
    for (g, kv) in updates {
        if !done.contains(g) {
            let last = blocks.last_mut().unwrap();
            if last.1.last().map(|l| !pycompat::strip(l).is_empty()).unwrap_or(false) {
                last.1.push(String::new());
            }
            let mut lines = vec![format!("[{g}]")];
            lines.extend(kv.iter().map(|(k, v)| format!("{k}={v}")));
            lines.push(String::new());
            blocks.push((Some(g.clone()), lines));
        }
    }
    let text = blocks.iter().map(|b| b.1.join("\n")).collect::<Vec<_>>().join("\n");
    let text = text.trim_end_matches('\n').to_string() + "\n";
    makedirs(&dirname(path))?;
    write_text(path, &text)
}

pub fn ini_set(path: &str, group: &str, key: &str, value: &str) -> R<()> {
    ini_update(path, &vec![(group.to_string(), vec![(key.to_string(), value.to_string())])])
}

/// shutil.which(cmd): an executable file of that name in a folder of PATH.
pub fn which(cmd: &str) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let path = env("PATH").unwrap_or_else(|| "/bin:/usr/bin".into());
    path.split(':').any(|d| {
        let p = join(if d.is_empty() { "." } else { d }, cmd);
        let Ok(c) = std::ffi::CString::new(std::ffi::OsStr::new(&p).as_bytes()) else { return false };
        Path::new(&p).is_file() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0
    })
}

/// Runs a program quietly, at most 20 seconds; whatever happens is fine.
pub fn run(cmd: &[&str]) {
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(cmd[0]).args(&cmd[1..]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn()
    else {
        return;
    };
    if !matches!(pycompat::process::wait_timeout(&mut child, std::time::Duration::from_secs(20)), Ok(Some(_))) {
        let _ = child.kill();
        let _ = child.wait();
    }
}

pub fn appearance_path() -> String {
    join(&join(&config(), "yggdrasil"), "appearance.conf")
}

/// appearance.conf as key = value (empty when there is none), in the script's two steps: a dict of the lines' keys as
/// written (a later line takes an earlier one's value, in its place), then the keys stripped (a later entry wins).
pub fn read_appearance() -> R<Vec<(String, String)>> {
    let Some(text) = read_text(&appearance_path())? else { return Ok(vec![]) };
    let mut raw: Vec<(String, String)> = vec![];
    for x in text.split_inclusive('\n') {
        let l = pycompat::strip(x.split('#').next().unwrap_or(""));
        if let Some((k, v)) = l.split_once('=') {
            set_kv(&mut raw, k, v);
        }
    }
    let mut out: Vec<(String, String)> = vec![];
    for (k, v) in raw {
        set_kv(&mut out, pycompat::strip(&k), pycompat::strip(&v));
    }
    Ok(out)
}

fn get<'a>(vals: &'a [(String, String)], k: &str) -> &'a str {
    vals.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str()).unwrap_or("")
}

/// The desktop's own record of the choice (read by the panel, the apps and the portal). The icon pack (Settings writes it)
/// is kept; the pointer is written only when it is not the default.
pub fn write_appearance(dark: bool, accent: Rgb, cursor: Option<&str>) -> R<()> {
    let p = appearance_path();
    makedirs(&dirname(&p))?;
    let old = read_appearance()?;
    let cursor = cursor.filter(|c| !c.is_empty()).unwrap_or(get(&old, "cursor")).to_string();
    let mut text = format!("style = {}\naccent = {}\n", if dark { "dark" } else { "light" }, rgb(accent));
    if !get(&old, "icons").is_empty() {
        text += &format!("icons = {}\n", get(&old, "icons"));
    }
    if !cursor.is_empty() && cursor != DEFAULT_CURSOR {
        text += &format!("cursor = {cursor}\n");
    }
    let tmp = format!("{p}.tmp");
    write_text(&tmp, &text)?;
    fs::rename(&tmp, &p).map_err(|e| Crash(format!("{p}: {e}")))
}

fn cursor_dirs() -> Vec<String> {
    vec![join(&data(), "icons"), join(&home(), ".icons"), "/usr/share/icons".into()]
}

/// An installed theme of that name with a cursors folder (a plain name: no paths).
pub fn is_cursor_theme(name: &str) -> bool {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return false;
    }
    cursor_dirs().iter().any(|d| Path::new(&join(&join(d, name), "cursors")).is_dir())
}

/// The pointer theme chosen, when it is still installed; Yggdrasil's own otherwise.
pub fn cursor_theme() -> R<String> {
    let name = get(&read_appearance()?, "cursor").to_string();
    Ok(if is_cursor_theme(&name) { name } else { DEFAULT_CURSOR.into() })
}

/// The icon pack in use (`icons` in appearance.conf); Liki when none was chosen.
pub fn icon_theme() -> R<String> {
    let name = get(&read_appearance()?, "icons").to_string();
    Ok(if !name.is_empty() && !name.contains('/') && !name.starts_with('.') { name } else { "Liki".into() })
}

/// Whether a program built on KDE's libraries is installed (KDE's settings library, or KDE's Flatpak runtime).
/// YGG_THEME_KDE=1 or 0 decides instead (tests).
pub fn kde_programs() -> bool {
    match env("YGG_THEME_KDE").as_deref() {
        Some("1") => return true,
        Some("0") => return false,
        _ => {}
    }
    let mut paths: Vec<String> = vec![];
    for d in ["/usr/lib", "/usr/lib64"] {
        for n in ["libKF6ConfigCore.so.6", "libKF5ConfigCore.so.5"] {
            paths.push(join(d, n));
        }
    }
    paths.push("/var/lib/flatpak/runtime/org.kde.Platform".into());
    paths.push(join(&join(&join(&data(), "flatpak"), "runtime"), "org.kde.Platform"));
    paths.iter().any(|p| Path::new(p).exists())
}

/// Points every place programs read the pointer from at NAME; with live, the running compositor switches now.
pub fn apply_cursor(name: &str, live: bool) -> R<()> {
    if kde_programs() {
        ini_set(&join(&config(), "kcminputrc"), "Mouse", "cursorTheme", name)?;
    }
    for ver in ["gtk-3.0", "gtk-4.0"] {
        let d = join(&config(), ver);
        makedirs(&d)?;
        ini_set(&join(&d, "settings.ini"), "Settings", "gtk-cursor-theme-name", name)?;
    }
    xsettings(&[("Gtk/CursorThemeName", name)])?;
    // programs started from now on (Wayland programs and the compositor at the next login read these)
    let envd = join(&config(), "environment.d");
    makedirs(&envd)?;
    write_text(&join(&envd, "50-yggdrasil-cursor.conf"), &format!("XCURSOR_THEME={name}\nXCURSOR_SIZE={CURSOR_SIZE}\n"))?;
    if which("gsettings") {
        run(&["gsettings", "set", "org.gnome.desktop.interface", "cursor-theme", name]);
    }
    if live {
        run(&["systemctl", "--user", "set-environment", &format!("XCURSOR_THEME={name}")]);
        // the compositor (yggdrasil-wm: Hyprland's `setcursor` on its socket, or Yggdrasil's own compositor)
        if let Some(d) = yggdrasil_wm::Desk::detect() {
            d.set_cursor(name, CURSOR_SIZE as u32);
        }
    }
    Ok(())
}

/// Sets keys in xsettingsd's file (X11 and XWayland programs read their theme from it) and tells it to reload.
pub fn xsettings(want: &[(&str, &str)]) -> R<()> {
    let xs = join(&join(&config(), "xsettingsd"), "xsettingsd.conf");
    if !Path::new(&xs).is_file() {
        return Ok(());
    }
    let mut want: Vec<(String, String)> = want.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    let text = read_text(&xs)?.ok_or_else(|| Crash(format!("{xs}: cannot read")))?;
    let mut lines: Vec<String> = text.split('\n').map(str::to_string).collect();
    for line in lines.iter_mut() {
        let key = line.split(' ').next().unwrap_or("").to_string();
        if let Some(i) = want.iter().position(|(k, _)| *k == key) {
            let (k, v) = want.remove(i);
            *line = format!("{k} \"{v}\"");
        }
    }
    let mut kept: Vec<String> = lines.into_iter().filter(|l| !pycompat::strip(l).is_empty()).collect();
    kept.extend(want.iter().map(|(k, v)| format!("{k} \"{v}\"")));
    write_text(&xs, &(kept.join("\n") + "\n"))?;
    run(&["pkill", "-HUP", "-x", "xsettingsd"]);
    Ok(())
}

/// A colour scheme file as [(group, [(key, value)])].
pub fn scheme_sections(text: &str) -> Updates {
    let mut out: Updates = vec![];
    let mut cur: Option<usize> = None;
    for line in pycompat::splitlines(text) {
        let line = pycompat::strip(line);
        if line.starts_with('[') && line.ends_with(']') {
            let name: String = {
                let cs: Vec<char> = line.chars().collect();
                if cs.len() >= 2 {
                    cs[1..cs.len() - 1].iter().collect()
                } else {
                    String::new()
                }
            };
            cur = Some(match out.iter().position(|(n, _)| *n == name) {
                Some(i) => i,
                None => {
                    out.push((name, vec![]));
                    out.len() - 1
                }
            });
        } else if line.contains('=') && cur.is_some() && !line.starts_with('#') {
            let (k, v) = line.split_once('=').unwrap();
            set_kv(&mut out[cur.unwrap()].1, k, v);
        }
    }
    out
}

fn set_kv(kv: &mut Vec<(String, String)>, k: &str, v: &str) {
    match kv.iter_mut().find(|(n, _)| n == k) {
        Some(e) => e.1 = v.to_string(),
        None => kv.push((k.to_string(), v.to_string())),
    }
}

fn group<'a>(u: &'a mut Updates, name: &str) -> &'a mut Vec<(String, String)> {
    let i = match u.iter().position(|(n, _)| n == name) {
        Some(i) => i,
        None => {
            u.push((name.to_string(), vec![]));
            u.len() - 1
        }
    };
    &mut u[i].1
}

/// For programs built on KDE's libraries only: the colour scheme in the user's folder, and the same colours, accent and icon
/// pack in kdeglobals.
pub fn write_kde_scheme(dark: bool, accent: Rgb, icons: &str) -> R<()> {
    let name = if dark { "YggdrasilDark" } else { "YggdrasilLight" };
    let sdir = join(&data(), "color-schemes");
    makedirs(&sdir)?;
    write_text(&join(&sdir, &format!("{name}.colors")), &scheme(dark, accent))?;
    let mut updates = scheme_sections(&scheme(dark, accent));
    let g = group(&mut updates, "General");
    set_kv(g, "AccentColor", &rgb(accent));
    set_kv(g, "ColorScheme", name);
    set_kv(group(&mut updates, "KDE"), "widgetStyle", "Fusion");
    *group(&mut updates, "Icons") = vec![("Theme".into(), icons.into())];
    ini_update(&join(&config(), "kdeglobals"), &updates)
}

/// re.sub(r"^@import\s+['\"]colors\.css['\"];\s*$", "", old, flags=re.M)
fn drop_colors_import(old: &str) -> String {
    let s = pycompat::PY_SPACE_CLASS;
    regex::Regex::new(&format!(r#"(?m)^@import{s}+['"]colors\.css['"];{s}*$"#)).unwrap().replace_all(old, "").into_owned()
}

pub fn apply(dark: bool, accent: Rgb) -> R<()> {
    write_appearance(dark, accent, None)?;
    let icons = icon_theme()?;
    if kde_programs() {
        write_kde_scheme(dark, accent, &icons)?;
    }
    // GTK 3 and 4: the theme name, the dark flag, the pointer and the icons; the colours in each user stylesheet. Dark mode is
    // a theme of its own (Yggdrasil-dark), not only the dark flag: Chromium-based browsers load a theme by its name
    let theme = if dark { "Yggdrasil-dark" } else { "Yggdrasil" };
    for ver in ["gtk-3.0", "gtk-4.0"] {
        let d = join(&config(), ver);
        makedirs(&d)?;
        let ini = join(&d, "settings.ini");
        ini_set(&ini, "Settings", "gtk-application-prefer-dark-theme", if dark { "true" } else { "false" })?;
        ini_set(&ini, "Settings", "gtk-icon-theme-name", &icons)?;
        ini_set(&ini, "Settings", "gtk-theme-name", theme)?;
        // libadwaita and GTK 4 read only the user's stylesheet: the colours there, apart from anything the user adds below
        let css_path = join(&d, "gtk.css");
        let old = read_text(&css_path)?.unwrap_or_default();
        let marker = "/* --- yours below this line --- */";
        let mut mine = old.split_once(marker).map(|(_, m)| m.to_string()).unwrap_or_default();
        if !old.is_empty() && !old.contains(marker) {
            // a stylesheet from before (it imports colours.css): keep it below the marker, minus that import
            mine = "\n".to_string() + &drop_colors_import(&old);
        }
        let tail = if mine.is_empty() { "\n".to_string() } else { mine };
        write_text(&css_path, &format!("{}{marker}{tail}", gtk_colors(dark, accent)))?;
    }
    // X11 (and XWayland) programs read their theme from xsettingsd; say the same there
    xsettings(&[("Net/ThemeName", theme), ("Net/IconThemeName", &icons)])?;
    // the pointer (Yggdrasil's own unless another was chosen)
    apply_cursor(&cursor_theme()?, false)?;
    // tell what is running
    if which("gsettings") {
        run(&["gsettings", "set", "org.gnome.desktop.interface", "color-scheme", if dark { "prefer-dark" } else { "default" }]);
        run(&["gsettings", "set", "org.gnome.desktop.interface", "gtk-theme", theme]);
        run(&["gsettings", "set", "org.gnome.desktop.interface", "icon-theme", &icons]);
    }
    Ok(())
}

/// (dark, accent) from the desktop's own file (dark and the default accent when there is none).
pub fn current() -> R<(bool, Rgb)> {
    let vals = read_appearance()?;
    let style = vals.iter().find(|(k, _)| k == "style").map(|(_, v)| v.as_str()).unwrap_or("dark");
    Ok((style != "light", crate::colors::parse_accent(get(&vals, "accent")).unwrap_or(crate::colors::DEFAULT_ACCENT)))
}
