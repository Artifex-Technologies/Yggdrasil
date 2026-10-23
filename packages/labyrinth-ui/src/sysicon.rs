//! Icons from the system icon theme (Liki, Yggdrasil's own pack, standing on hicolor only: no KDE or macOS icons), so Minos shows the same folders, files and drives as the dock and the launcher.
//! Rendering is cached as textures in egui's memory; nothing has to be passed around.
//!
//! Callers give a list of icon names to try, best first, and fall back to their own drawing
//! when none is found (`paint` returns false).

use eframe::egui::{self, Color32, ColorImage, Context, Painter, Rect, TextureHandle, TextureOptions};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Liki first (the icon pack Yggdrasil ships and uses by default), then hicolor, where programs put their own icons. See docs/ICONS.md.
const BASE: [&str; 2] = ["Liki", "hicolor"];

/// What an app gets when no pack has an icon for it.
pub const DEFAULT_APP_ICON: &str = "application-default-icon";
const CONTEXTS: [&str; 6] = ["places", "mimetypes", "devices", "apps", "status", "actions"];

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

/// The chain to look icons up in: the pack chosen in KDE first (so another pack changes what is shown),
/// then Labyrinth's own, which fills in whatever that pack has no icon for, so nothing is ever missing.
/// Looked at again when the pack or its files change (see `refresh`), so a new pack or accent shows without a restart.
fn compute_chain() -> Vec<String> {
    let chosen = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .and_then(|d| std::fs::read_to_string(d.join("kdeglobals")).ok())
        .and_then(|t| parse_icon_theme(&t));
    let mut v: Vec<String> = vec![];
    if let Some(c) = chosen {
        if c != "Liki" {
            // another pack: then hicolor, where Minos's icon for that pack is put (the pack's own
            // file-manager icon if it has no Minos one), then Liki's to fill the gaps
            v.push(c);
            v.push("hicolor".to_string());
        }
    }
    v.extend(BASE.iter().map(|s| s.to_string()));
    v
}

static CHAIN: Mutex<Option<Vec<String>>> = Mutex::new(None);

fn themes() -> Vec<String> {
    let mut g = CHAIN.lock().unwrap_or_else(|e| e.into_inner());
    g.get_or_insert_with(compute_chain).clone()
}

/// A number that changes when the icon pack in use, or any file folder of the packs in the chain, changes.
fn fingerprint() -> u64 {
    let mut h: u64 = 1469598103934665603;
    let mut mix = |t: Option<std::time::SystemTime>| {
        let n = t.and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
        h = (h ^ n).wrapping_mul(1099511628211);
    };
    let cfg = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    if let Some(c) = cfg {
        mix(std::fs::metadata(c.join("kdeglobals")).and_then(|m| m.modified()).ok());
    }
    for theme in compute_chain() {
        for byte in theme.bytes() {
            mix(Some(std::time::UNIX_EPOCH + std::time::Duration::from_nanos(byte as u64)));
        }
        for root in roots() {
            for sub in ["", "apps/scalable", "places/scalable", "apps", "256x256/apps", "192x192/apps"] {
                mix(std::fs::metadata(root.join(&theme).join(sub)).and_then(|m| m.modified()).ok());
            }
        }
    }
    h
}

/// Call now and then (every frame is fine: it looks at most once a second): when the icon pack or its files changed, forget
/// every icon found so far, so they are looked up and drawn again. Returns true when that happened.
pub fn refresh(ctx: &Context) -> bool {
    static LAST: Mutex<Option<(std::time::Instant, u64)>> = Mutex::new(None);
    let mut g = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some((at, _)) = *g {
        if at.elapsed() < std::time::Duration::from_secs(1) {
            return false;
        }
    }
    *CHAIN.lock().unwrap_or_else(|e| e.into_inner()) = None;
    let fp = fingerprint();
    let changed = matches!(*g, Some((_, old)) if old != fp);
    *g = Some((std::time::Instant::now(), fp));
    drop(g);
    if changed {
        if let Ok(mut c) = cache(ctx).lock() {
            c.tex.clear();
            c.found.clear();
        }
        ctx.request_repaint();
    }
    changed
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
                for sub in ["scalable", "512x512", "384x384", "256x256", "192x192", "144x144", "128x128", "96x96", "72x72", "64x64", "48x48", "36x36", "32x32"] {
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
    refresh(ctx);
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

/// Like `paint`, for an app: an app that no pack has an icon for gets the default app icon, never a blank.
pub fn paint_app(painter: &Painter, rect: Rect, names: &[&str]) -> bool {
    paint(painter, rect, names) || paint(painter, rect, &[DEFAULT_APP_ICON])
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

/// Icon names for a file, from its extension: the most specific type first, then its family, then plain text. Each type has an
/// icon of its own in Liki, in a colour of its own (only folders follow the accent).
pub fn file_names(file_name: &str) -> Vec<&'static str> {
    let ext = file_name.rsplit_once('.').map(|(_, e)| e.to_lowercase()).unwrap_or_default();
    let (kind, family): (&str, &str) = match ext.as_str() {
        "md" | "markdown" | "mdx" => ("text-markdown", "text-x-generic"),
        "txt" | "text" => ("text-plain", "text-x-generic"),
        "pdf" => ("application-pdf", ""),
        "svg" | "svgz" => ("image-svg+xml", "image-x-generic"),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tiff" | "tif" | "heic" | "heif" | "avif" | "ico" | "raw" | "cr2" | "nef" | "xcf" | "psd" | "kra" => ("image-x-generic", ""),
        "mp3" | "flac" | "wav" | "ogg" | "m4a" | "opus" | "aac" | "wma" | "mid" | "midi" => ("audio-x-generic", ""),
        "mp4" | "mkv" | "mov" | "avi" | "webm" | "m4v" | "wmv" | "flv" | "mpg" | "mpeg" => ("video-x-generic", ""),
        "zip" | "7z" | "rar" => ("application-zip", "application-x-archive"),
        "tar" | "gz" | "tgz" | "xz" | "bz2" | "zst" | "lz4" | "lzma" => ("application-x-compressed-tar", "application-x-archive"),
        "deb" | "rpm" | "pkg" | "appimage" | "flatpak" | "snap" | "whl" | "jar" => ("package-x-generic", ""),
        "doc" | "docx" | "odt" | "rtf" | "pages" => ("x-office-document", ""),
        "xls" | "xlsx" | "ods" | "numbers" => ("x-office-spreadsheet", ""),
        "csv" | "tsv" => ("text-csv", "x-office-spreadsheet"),
        "ppt" | "pptx" | "odp" | "key" => ("x-office-presentation", ""),
        "py" | "pyw" | "ipynb" => ("text-x-python", "text-x-script"),
        "rs" => ("text-x-rust", "text-x-script"),
        "go" => ("text-x-go", "text-x-script"),
        "c" => ("text-x-csrc", "text-x-script"),
        "cpp" | "cc" | "cxx" | "hpp" | "hh" => ("text-x-c++src", "text-x-script"),
        "h" => ("text-x-chdr", "text-x-script"),
        "java" | "kt" | "kts" => ("text-x-java", "text-x-script"),
        "js" | "mjs" | "cjs" | "jsx" => ("application-javascript", "text-x-script"),
        "ts" | "tsx" => ("text-x-typescript", "text-x-script"),
        "lua" => ("text-x-lua", "text-x-script"),
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "bat" | "cmd" => ("application-x-shellscript", "text-x-script"),
        "rb" | "php" | "pl" | "swift" | "cs" | "dart" | "zig" | "nim" | "hs" | "ex" | "exs" | "r" | "scala" | "vue" | "svelte" => ("text-x-script", ""),
        "html" | "htm" | "xhtml" => ("text-html", ""),
        "css" | "scss" | "sass" | "less" => ("text-css", ""),
        "json" | "jsonc" | "json5" | "geojson" => ("application-json", ""),
        "xml" | "plist" | "xsd" | "xsl" => ("application-xml", ""),
        "toml" | "yaml" | "yml" | "ini" | "conf" | "cfg" | "env" | "desktop" | "service" | "properties" => ("text-x-config", "text-x-generic"),
        "log" => ("text-x-log", "text-x-generic"),
        "patch" | "diff" => ("text-x-patch", "text-x-generic"),
        "ttf" | "otf" | "woff" | "woff2" => ("font-x-generic", ""),
        "iso" | "img" | "dmg" | "qcow2" | "vdi" | "vmdk" => ("application-x-cd-image", ""),
        "exe" | "msi" | "bin" | "run" | "elf" | "out" => ("application-x-executable", ""),
        "so" | "dll" | "dylib" | "a" | "o" => ("application-x-sharedlib", "application-x-executable"),
        "sql" => ("application-sql", ""),
        "db" | "sqlite" | "sqlite3" => ("application-x-sqlite3", "application-sql"),
        "epub" | "mobi" | "azw3" => ("application-epub+zip", ""),
        "asc" | "gpg" | "pgp" | "pem" | "crt" | "pub" => ("application-pgp-keys", ""),
        _ => ("", ""),
    };
    let mut v: Vec<&'static str> = [kind, family].into_iter().filter(|s| !s.is_empty()).collect();
    v.extend(["text-x-generic", "application-x-generic"]);
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_changes_when_a_pack_gets_a_new_icon() {
        let tmp = std::env::temp_dir().join(format!("sysicon-fp-{}", std::process::id()));
        let apps = tmp.join(".local/share/icons/Liki/apps/scalable");
        std::fs::create_dir_all(&apps).unwrap();
        std::env::set_var("HOME", &tmp);
        std::env::set_var("XDG_CONFIG_HOME", tmp.join(".config"));
        let before = fingerprint();
        std::thread::sleep(std::time::Duration::from_millis(20));
        std::fs::write(apps.join("minos.svg"), "<svg/>").unwrap();
        assert_ne!(fingerprint(), before);
        let _ = std::fs::remove_dir_all(&tmp);
    }


    #[test]
    fn names_follow_the_extension_and_folder() {
        assert_eq!(file_names("report.PDF")[0], "application-pdf");
        assert_eq!(file_names("README")[0], "text-x-generic");
        assert_eq!(file_names("notes.md"), vec!["text-markdown", "text-x-generic", "text-x-generic", "application-x-generic"]);
        assert_eq!(file_names("Cargo.toml")[0], "text-x-config");
        assert_eq!(file_names("data.csv")[..2], ["text-csv", "x-office-spreadsheet"]);
        assert_eq!(file_names("run.sh")[0], "application-x-shellscript");
        assert_eq!(folder_names("Downloads"), vec!["folder-download", "folder"]);
        assert_eq!(folder_names("src"), vec!["folder"]);
    }
}
