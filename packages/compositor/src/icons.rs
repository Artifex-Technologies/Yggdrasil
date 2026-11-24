//! An app's icon for the window switcher's cards: the icon its desktop entry names (found from the window's app id the way the
//! dock finds it: the entry's file name, then its StartupWMClass); for a Steam game, the icon of the shortcut Steam made for it
//! (its `Exec` starts `steam://rungameid/N`), else Steam's own `steam_icon_N`, as the dock does (packages/panel, apps.rs): a
//! game under Proton has the window class `steam_app_N`, a native one its own class and `SteamAppId=N` in its environment;
//! else an icon named after the app id; looked up in the icon
//! pack chosen in Settings, then hicolor and Liki (docs/ICONS.md), then the loose pictures in /usr/share/pixmaps. Read only while
//! the switcher is open, once per app (switcher.rs keeps them until it closes).

use std::path::{Path, PathBuf};

use tiny_skia::{Pixmap, PixmapPaint, Transform};

/// The folders desktop entries are in (XDG_DATA_HOME's first: the user's own wins), as the dock lists them.
fn app_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![];
    let home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")));
    dirs.extend(home.map(|h| h.join("applications")));
    let data = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    dirs.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("applications")));
    dirs.push("/var/lib/flatpak/exports/share/applications".into());
    dirs
}

/// The `Icon=` and `StartupWMClass=` of a desktop entry's main group.
fn entry_fields(text: &str) -> (Option<String>, Option<String>) {
    let (mut icon, mut class, mut main) = (None, None, false);
    for line in text.lines().map(str::trim) {
        if line.starts_with('[') {
            main = line == "[Desktop Entry]";
            continue;
        }
        if !main {
            continue;
        }
        match line.split_once('=') {
            Some(("Icon", v)) => icon = Some(v.trim().to_string()),
            Some(("StartupWMClass", v)) => class = Some(v.trim().to_string()),
            _ => {}
        }
    }
    (icon, class)
}

/// The game number of a Steam game's window under Proton (`steam_app_242760`).
pub fn steam_game(app_id: &str) -> Option<&str> {
    let n = app_id.strip_prefix("steam_app_").or_else(|| app_id.strip_prefix("STEAM_APP_"))?;
    (!n.is_empty() && n.bytes().all(|b| b.is_ascii_digit())).then_some(n)
}

/// The Steam game a program belongs to: Steam starts every game, native or not, with `SteamAppId` set to its number. Only
/// that one variable of the program's environment is looked at, and nothing is kept.
pub fn steam_game_of(pid: u32) -> Option<String> {
    steam_game_in(&std::fs::read(format!("/proc/{pid}/environ")).ok()?)
}

fn steam_game_in(environ: &[u8]) -> Option<String> {
    let v = environ.split(|b| *b == 0).find_map(|kv| kv.strip_prefix(b"SteamAppId="))?;
    (!v.is_empty() && v != b"0" && v.iter().all(u8::is_ascii_digit)).then(|| String::from_utf8_lossy(v).into_owned())
}

/// A Steam game's icon name: its shortcut's (the entry whose Exec starts `steam://rungameid/N`), else `steam_icon_N` (Steam
/// saves it in the icon theme folder whether or not it made a shortcut).
fn steam_icon(dirs: &[PathBuf], n: &str) -> String {
    let url = format!("steam://rungameid/{n}");
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Ok(t) = std::fs::read_to_string(&p) else { continue };
            let starts =
                t.lines().any(|l| l.strip_prefix("Exec=").is_some_and(|x| x.split_whitespace().any(|w| w.trim_matches('"') == url)));
            if starts {
                if let (Some(i), _) = entry_fields(&t) {
                    return i;
                }
            }
        }
    }
    format!("steam_icon_{n}")
}

/// The icon a desktop entry for this app id names (by its file name, then by StartupWMClass).
fn entry_icon(dirs: &[PathBuf], app_id: &str) -> Option<String> {
    let lower = app_id.to_lowercase();
    for name in [app_id.to_string(), lower.clone()] {
        for d in dirs {
            if let Ok(t) = std::fs::read_to_string(d.join(format!("{name}.desktop"))) {
                if let (Some(i), _) = entry_fields(&t) {
                    return Some(i);
                }
            }
        }
    }
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "desktop") {
                continue;
            }
            let Ok(t) = std::fs::read_to_string(&p) else { continue };
            if let (Some(i), Some(c)) = entry_fields(&t) {
                if c.to_lowercase() == lower {
                    return Some(i);
                }
            }
        }
    }
    None
}

/// Where icon packs are (the user's, then the system's).
fn roots() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join(".local/share/icons"));
    }
    let data = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    v.extend(data.split(':').filter(|d| !d.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    v
}

/// The packs looked in, in order: the one chosen in Settings, hicolor (where programs put their own icons), Liki.
fn packs() -> Vec<String> {
    let mut v: Vec<String> = yggdrasil_conf::appearance::icon_pack().into_iter().collect();
    for p in ["hicolor", "Liki"] {
        if !v.iter().any(|x| x == p) {
            v.push(p.to_string());
        }
    }
    v
}

/// The file of an icon: an absolute path as it is, else the best match in the packs (scalable first, then the big sizes).
pub fn find(name: &str) -> Option<PathBuf> {
    if name.starts_with('/') {
        return Path::new(name).is_file().then(|| PathBuf::from(name));
    }
    if name.is_empty() || name.contains('/') {
        return None;
    }
    const SIZES: [u32; 8] = [64, 48, 128, 256, 96, 32, 512, 24];
    let mut dirs = vec!["apps/scalable".to_string(), "scalable/apps".to_string()];
    for s in SIZES {
        dirs.push(format!("{s}x{s}/apps"));
        dirs.push(format!("apps/{s}"));
    }
    for root in roots() {
        for pack in packs() {
            let t = root.join(&pack);
            if !t.is_dir() {
                continue;
            }
            for d in &dirs {
                for ext in ["svg", "png"] {
                    let p = t.join(d).join(format!("{name}.{ext}"));
                    if p.is_file() {
                        return Some(p);
                    }
                }
            }
        }
    }
    ["svg", "png"].into_iter().map(|e| PathBuf::from(format!("/usr/share/pixmaps/{name}.{e}"))).find(|p| p.is_file())
}

/// The icon of a file drawn `size` pixels square (an SVG with resvg; a PNG shrunk by halves first, so a big one is not
/// jagged).
pub fn draw(path: &Path, size: u32) -> Option<Pixmap> {
    let data = std::fs::read(path).ok()?;
    let mut pm = Pixmap::new(size, size)?;
    if path.extension().is_some_and(|e| e == "png") {
        let mut src = Pixmap::decode_png(&data).ok()?;
        while src.width() >= size * 2 && src.height() >= size * 2 {
            let mut half = Pixmap::new(src.width() / 2, src.height() / 2)?;
            let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bilinear, ..Default::default() };
            half.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(0.5, 0.5), None);
            src = half;
        }
        let k = (size as f32 / src.width() as f32).min(size as f32 / src.height() as f32);
        let (tx, ty) = ((size as f32 - src.width() as f32 * k) / 2.0, (size as f32 - src.height() as f32 * k) / 2.0);
        let paint = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..Default::default() };
        pm.draw_pixmap(0, 0, src.as_ref(), &paint, Transform::from_scale(k, k).post_translate(tx, ty), None);
        return Some(pm);
    }
    let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
    let sz = tree.size();
    let k = (size as f32 / sz.width()).min(size as f32 / sz.height());
    let (tx, ty) = ((size as f32 - sz.width() * k) / 2.0, (size as f32 - sz.height() * k) / 2.0);
    resvg::render(&tree, Transform::from_scale(k, k).post_translate(tx, ty), &mut pm.as_mut());
    Some(pm)
}

/// An app's icon `size` pixels square, if it has one. `steam`: the Steam game its program belongs to (from its environment),
/// for a native game whose own class no desktop entry knows.
pub fn for_app(app_id: &str, steam: Option<&str>, size: u32) -> Option<Pixmap> {
    if app_id.is_empty() || app_id.contains('/') {
        return None;
    }
    let dirs = app_dirs();
    let game = steam_game(app_id).or(steam);
    let name = match (entry_icon(&dirs, app_id), game) {
        (Some(i), _) => i,
        (None, Some(n)) => steam_icon(&dirs, n),
        (None, None) => app_id.to_string(),
    };
    draw(&find(&name)?, size)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_desktop_entry_names_its_icon_and_window_class() {
        let t = "[Desktop Entry]\nName=Calc\nIcon=accessories-calculator\nStartupWMClass=Calc\n[Desktop Action new]\nIcon=other\n";
        assert_eq!(entry_fields(t), (Some("accessories-calculator".into()), Some("Calc".into())));
        assert_eq!(entry_fields("[Other]\nIcon=x\n"), (None, None));
    }

    #[test]
    fn a_steam_game_takes_its_shortcuts_icon() {
        let dir = std::env::temp_dir().join(format!("ygg-steamicon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("Fallout 4.desktop"),
            "[Desktop Entry]\nName=Fallout 4\nExec=steam steam://rungameid/377160\nIcon=steam_icon_377160\n",
        )
        .unwrap();
        std::fs::write(dir.join("steam.desktop"), "[Desktop Entry]\nName=Steam\nExec=/usr/bin/steam %U\nIcon=steam\n").unwrap();
        let dirs = [dir.clone()];
        let got = (steam_icon(&dirs, "377160"), steam_icon(&dirs, "246620"), entry_icon(&dirs, "steam_app_377160"));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(got, ("steam_icon_377160".into(), "steam_icon_246620".into(), None));
        assert_eq!((steam_game("steam_app_377160"), steam_game("steam_app_"), steam_game("steam")), (Some("377160"), None, None));
        assert_eq!(steam_game_in(b"HOME=/h\0SteamAppId=1332010\0X=1\0"), Some("1332010".into()));
        assert_eq!(steam_game_in(b"SteamAppId=0\0"), None);
        // a program's own environment, as the switcher reads it
        let mut child = std::process::Command::new("sleep").arg("5").env("SteamAppId", "242760").spawn().unwrap();
        // until the program has started (right after the fork its environment is still the test's)
        let start = std::time::Instant::now();
        while std::fs::read_to_string(format!("/proc/{}/cmdline", child.id())).is_ok_and(|c| !c.starts_with("sleep"))
            && start.elapsed() < std::time::Duration::from_secs(5)
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let read = steam_game_of(child.id());
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(read, Some("242760".into()));
    }

    #[test]
    fn an_svg_icon_is_drawn_at_the_size_asked() {
        let dir = std::env::temp_dir().join(format!("ygg-icon-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("i.svg");
        std::fs::write(
            &p,
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="red"/></svg>"#,
        )
        .unwrap();
        let pm = draw(&p, 20).unwrap();
        assert_eq!((pm.width(), pm.height()), (20, 20));
        let c = pm.pixel(10, 10).unwrap();
        assert_eq!((c.red(), c.green(), c.blue(), c.alpha()), (255, 0, 0, 255));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
