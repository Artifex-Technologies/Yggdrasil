//! Icons from the system icon theme (Labyrinth, which is MacTahoe in dark plus per-app
//! overrides), so Minos shows the same folders, files and drives as the dock and the launcher.
//! Rendering is cached as textures in egui's memory; nothing has to be passed around.
//!
//! Callers give a list of icon names to try, best first, and fall back to their own drawing
//! when none is found (`paint` returns false).

use eframe::egui::{self, Color32, ColorImage, Context, Painter, Rect, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Labyrinth's own icons first (it is the icon pack Labyrinth ships: MacTahoe in dark plus its per-app
/// icons), then what they stand on. See docs/ICONS.md.
const BASE: [&str; 7] = ["Labyrinth", "MacTahoe-purple-dark", "MacTahoe-purple", "hicolor", "breeze-dark", "breeze", "Adwaita"];
const CONTEXTS: [&str; 5] = ["places", "mimes", "devices", "apps", "status"];

#[derive(Default)]
struct Cache {
    found: HashMap<String, Option<PathBuf>>,
    tex: HashMap<(String, u32), Option<TextureHandle>>,
}

/// The icon theme chosen in KDE (`[Icons] Theme` in kdeglobals).
pub fn parse_icon_theme(kdeglobals: &str) -> Option<String> {
    let mut in_icons = false;
    for line in kdeglobals.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_icons = line == "[Icons]";
        } else if in_icons {
            if let Some(v) = line.strip_prefix("Theme=") {
                let v = v.trim();
                return (!v.is_empty()).then(|| v.to_string());
            }
        }
    }
    None
}

/// The chain to look icons up in: the pack chosen in KDE first (so another pack changes what Minos
/// shows), then Labyrinth's own, which fills in whatever that pack has no icon for, so nothing is
/// ever missing. Read once, when the first icon is wanted; a changed pack shows at the next start.
fn themes() -> &'static Vec<String> {
    static T: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let chosen = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .and_then(|d| std::fs::read_to_string(d.join("kdeglobals")).ok())
            .and_then(|t| parse_icon_theme(&t));
        let mut v: Vec<String> = vec![];
        if let Some(c) = chosen {
            if c != "Labyrinth" {
                // another pack: then hicolor, where Minos's icon for that pack is put (the pack's own
                // file-manager icon if it has no Minos one), then Labyrinth's to fill the gaps
                v.push(c);
                v.push("hicolor".to_string());
            }
        }
        v.extend(BASE.iter().map(|s| s.to_string()));
        v
    })
}

fn roots() -> Vec<PathBuf> {
    let mut v = vec![];
    if let Some(h) = std::env::var_os("HOME") {
        v.push(PathBuf::from(h).join(".local/share/icons"));
    }
    v.push(PathBuf::from("/usr/share/icons"));
    v
}

/// The icon file for a name in the theme chain, vector first.
pub fn find(name: &str) -> Option<PathBuf> {
    let exts = ["svg", "png"];
    for root in roots() {
        for theme in themes() {
            let t = root.join(theme);
            for ctx in CONTEXTS {
                for sub in ["scalable", "48", "32", "24", "22", "16"] {
                    for e in exts {
                        let p = t.join(ctx).join(sub).join(format!("{name}.{e}"));
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
                // the other layout (hicolor and most app-installed icons): size first, then the context
                for sub in ["scalable", "256x256", "128x128", "64x64", "48x48", "32x32"] {
                    for e in exts {
                        let p = t.join(sub).join(ctx).join(format!("{name}.{e}"));
                        if p.is_file() {
                            return Some(p);
                        }
                    }
                }
            }
        }
    }
    None
}

fn render(path: &Path, px: u32) -> Option<ColorImage> {
    let data = std::fs::read(path).ok()?;
    let mut pm = resvg::tiny_skia::Pixmap::new(px, px)?;
    if path.extension().map(|e| e == "png").unwrap_or(false) {
        let src = resvg::tiny_skia::Pixmap::decode_png(&data).ok()?;
        let s = (px as f32 / src.width() as f32).min(px as f32 / src.height() as f32);
        let tx = (px as f32 - src.width() as f32 * s) / 2.0;
        let ty = (px as f32 - src.height() as f32 * s) / 2.0;
        let paint = resvg::tiny_skia::PixmapPaint { quality: resvg::tiny_skia::FilterQuality::Bicubic, ..Default::default() };
        pm.draw_pixmap(0, 0, src.as_ref(), &paint, resvg::tiny_skia::Transform::from_scale(s, s).post_translate(tx, ty), None);
    } else {
        let tree = resvg::usvg::Tree::from_data(&data, &resvg::usvg::Options::default()).ok()?;
        let sz = tree.size();
        let s = (px as f32 / sz.width()).min(px as f32 / sz.height());
        let tx = (px as f32 - sz.width() * s) / 2.0;
        let ty = (px as f32 - sz.height() * s) / 2.0;
        resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s).post_translate(tx, ty), &mut pm.as_mut());
    }
    // tiny-skia is premultiplied; egui wants straight alpha.
    let mut rgba = Vec::with_capacity((px * px * 4) as usize);
    for p in pm.pixels() {
        let c = p.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Some(ColorImage::from_rgba_unmultiplied([px as usize, px as usize], &rgba))
}

fn cache(ctx: &Context) -> Arc<Mutex<Cache>> {
    ctx.data_mut(|d| d.get_temp_mut_or_insert_with::<Arc<Mutex<Cache>>>(egui::Id::new("labyrinth-sysicons"), Default::default).clone())
}

/// Paint the first icon of `names` that exists in the theme into `rect`. False if none does.
pub fn paint(painter: &Painter, rect: Rect, names: &[&str]) -> bool {
    let ctx = painter.ctx();
    let px = ((rect.width().min(rect.height()) * ctx.pixels_per_point()).ceil() as u32).clamp(16, 512);
    let px = (px + 7) / 8 * 8; // fewer distinct sizes, fewer textures
    let c = cache(ctx);
    let mut c = match c.lock() {
        Ok(c) => c,
        Err(_) => return false,
    };
    for name in names {
        let key = (name.to_string(), px);
        if !c.tex.contains_key(&key) {
            let path = c.found.entry(name.to_string()).or_insert_with(|| find(name)).clone();
            let tex = path.and_then(|p| render(&p, px)).map(|img| ctx.load_texture(format!("sysicon-{name}-{px}"), img, TextureOptions::LINEAR));
            c.tex.insert(key.clone(), tex);
        }
        if let Some(Some(t)) = c.tex.get(&key) {
            painter.image(t.id(), rect, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
            return true;
        }
    }
    false
}

/// Icon names for a folder, by its name ("Documents" gets the documents folder).
pub fn folder_names(dir_name: &str) -> Vec<&'static str> {
    let special = match dir_name.to_lowercase().as_str() {
        "documents" => "folder-documents",
        "downloads" => "folder-download",
        "music" => "folder-music",
        "pictures" | "images" => "folder-pictures",
        "videos" => "folder-videos",
        "desktop" => "user-desktop",
        "public" => "folder-public",
        "templates" => "folder-templates",
        _ => "",
    };
    if special.is_empty() { vec!["folder"] } else { vec![special, "folder"] }
}

/// Icon names for a file, from its extension.
pub fn file_names(file_name: &str) -> Vec<&'static str> {
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let kind = match ext.as_str() {
        "pdf" => "application-pdf",
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tiff" | "heic" | "ico" => "image-x-generic",
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "opus" | "aac" => "audio-x-generic",
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" => "video-x-generic",
        "zip" | "7z" | "rar" | "tar" | "gz" | "xz" | "bz2" | "zst" => "application-zip",
        "deb" | "rpm" | "pkg" | "appimage" | "flatpak" => "package-x-generic",
        "doc" | "docx" | "odt" | "rtf" => "x-office-document",
        "xls" | "xlsx" | "ods" | "csv" => "x-office-spreadsheet",
        "ppt" | "pptx" | "odp" => "x-office-presentation",
        "py" => "text-x-python",
        "rs" => "text-x-rust",
        "html" | "htm" => "text-html",
        "json" => "application-json",
        "sh" | "bash" | "js" | "ts" | "c" | "cpp" | "h" | "java" | "go" | "lua" => "text-x-script",
        "exe" | "msi" | "bin" => "application-x-executable",
        _ => "",
    };
    if kind.is_empty() { vec!["text-x-generic", "application-x-generic"] } else { vec![kind, "text-x-generic"] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_extension_and_folder() {
        assert_eq!(file_names("report.PDF")[0], "application-pdf");
        assert_eq!(file_names("README")[0], "text-x-generic");
        assert_eq!(folder_names("Downloads"), vec!["folder-download", "folder"]);
        assert_eq!(folder_names("src"), vec!["folder"]);
    }
}
