//! An app's icon for the window switcher's cards: the icon its desktop entry names (found from the window's app id the way the
//! dock finds it: the entry's file name, then its StartupWMClass), else an icon named after the app id; looked up in the icon
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

/// The icon name for a window's app id: its desktop entry's (by file name, then by StartupWMClass, case not minded), else the
/// app id itself (icon packs name many icons after it).
pub fn icon_name(app_id: &str) -> Option<String> {
    if app_id.is_empty() || app_id.contains('/') {
        return None;
    }
    let dirs = app_dirs();
    let lower = app_id.to_lowercase();
    for name in [app_id.to_string(), lower.clone()] {
        for d in &dirs {
            if let Ok(t) = std::fs::read_to_string(d.join(format!("{name}.desktop"))) {
                if let (Some(i), _) = entry_fields(&t) {
                    return Some(i);
                }
            }
        }
    }
    for d in &dirs {
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
    Some(app_id.to_string())
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

/// An app's icon `size` pixels square, if it has one.
pub fn for_app(app_id: &str, size: u32) -> Option<Pixmap> {
    draw(&find(&icon_name(app_id)?)?, size)
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
