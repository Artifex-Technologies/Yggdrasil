//! The icons the vendors made, for everything in the Marketplace: an app is shown with its own icon (the one in its package, its Flatpak, the
//! AppStream catalog, or Flathub), not with a restyled one from the icon pack. Only an app with no vendor icon anywhere falls back to the
//! pack's (Liki's) icon, and then to its default app icon.
//!
//! Found without network when possible: an index of the icon files on this computer is made once (hicolor, pixmaps, the Flatpak exports, the
//! AppStream icon caches). For an app that is not here but is on Flathub, its icon is fetched in the background (one at a time), only when the
//! "online details" switch is on (the same one that allows screenshots), and kept in the Marketplace's cache.
//!
//! An app that is neither here nor on Flathub can name its icon in the operating system's own catalog (`icon_url`, an https address,
//! ideally pinned to a commit). It is fetched the same way (in the background, with the same switch, once), kept only if its content is
//! a PNG or an SVG within the limits below, and cached under the app's id.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};

use eframe::egui;

static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
static FETCHED: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);
static QUEUED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static TX: OnceLock<mpsc::Sender<Job>> = OnceLock::new();

/// The most an icon from a catalog's `icon_url` may weigh.
pub const URL_ICON_MAX_BYTES: usize = 1024 * 1024;
/// Its longest side in pixels (a PNG's, or the width and height an SVG declares).
pub const URL_ICON_MAX_SIDE: f64 = 1024.0;

/// One icon to look for in the background: Flathub's for `flathub` first, else the catalog's `url` for the app `id`.
struct Job {
    key: String,
    flathub: String,
    id: String,
    url: String,
    ctx: egui::Context,
}

/// What a downloaded icon is, judged by its content.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Png,
    Svg,
}

impl Kind {
    fn ext(self) -> &'static str {
        match self {
            Kind::Png => "png",
            Kind::Svg => "svg",
        }
    }
}

fn subdirs(p: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(p).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default()
}

/// The folders icon files are in, best (biggest, vector) first.
fn icon_dirs() -> Vec<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let mut hicolors = vec![PathBuf::from("/usr/share/icons/hicolor"), PathBuf::from("/var/lib/flatpak/exports/share/icons/hicolor")];
    if let Some(h) = &home {
        hicolors.push(h.join(".local/share/flatpak/exports/share/icons/hicolor"));
        hicolors.push(h.join(".local/share/icons/hicolor"));
    }
    let mut v = vec![];
    for size in ["scalable", "1024x1024", "512x512", "384x384", "256x256", "192x192", "128x128", "96x96", "64x64", "48x48"] {
        for h in &hicolors {
            v.push(h.join(size).join("apps"));
        }
    }
    v.push(PathBuf::from("/usr/share/pixmaps"));
    // the AppStream catalogs' own icon caches (named like "package_icon.png")
    for root in ["/usr/share/swcatalog/icons", "/var/lib/swcatalog/icons", "/var/cache/swcatalog/icons", "/usr/share/app-info/icons"] {
        for origin in subdirs(Path::new(root)) {
            for size in ["128x128", "64x64"] {
                v.push(origin.join(size));
            }
        }
    }
    for root in
        ["/var/lib/flatpak/appstream"].into_iter().map(PathBuf::from).chain(home.iter().map(|h| h.join(".local/share/flatpak/appstream")))
    {
        for remote in subdirs(&root) {
            for arch in subdirs(&remote) {
                for size in ["128x128", "64x64"] {
                    v.push(arch.join("active/icons").join(size));
                }
            }
        }
    }
    v
}

fn build_index() -> HashMap<String, PathBuf> {
    let mut m: HashMap<String, PathBuf> = HashMap::new();
    for d in icon_dirs() {
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
            if ext != "svg" && ext != "png" {
                continue;
            }
            let Some(stem) = p.file_stem().and_then(|s| s.to_str()).map(|s| s.to_lowercase()) else { continue };
            // an earlier folder has the better picture
            m.entry(stem.clone()).or_insert_with(|| p.clone());
            if let Some((_, after)) = stem.split_once('_') {
                m.entry(after.to_string()).or_insert(p.clone());
            }
        }
    }
    m
}

fn cache_file(flathub: &str) -> Option<PathBuf> {
    crate::details::icon_cache_dir().map(|d| d.join(format!("{flathub}.png")))
}

/// The vendor's icon file for an app, if there is one now: installed, else Flathub's (`flathub`), else the one the catalog names
/// (`icon_url`, for the app `id`). Starts a background fetch when none is cached and fetching is allowed; the window is asked to draw
/// again when it arrives.
pub fn find(names: &[&str], flathub: &str, id: &str, icon_url: &str, online: bool, ctx: &egui::Context) -> Option<PathBuf> {
    let index = INDEX.get_or_init(build_index);
    for n in names {
        if let Some(p) = index.get(&n.to_lowercase()) {
            return Some(p.clone());
        }
    }
    let flathub = if crate::details::safe_id(flathub) { flathub } else { "" };
    let url = if fetchable(icon_url) && crate::details::safe_id(id) { icon_url } else { "" };
    if flathub.is_empty() && url.is_empty() {
        return None;
    }
    if let Some(p) = Some(flathub).filter(|f| !f.is_empty()).and_then(cache_file).filter(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(p) = Some(url).filter(|u| !u.is_empty()).and_then(|u| url_cached(id, u)) {
        return Some(p);
    }
    let key = if flathub.is_empty() { format!("app:{id}") } else { flathub.to_string() };
    if let Some(Some(p)) = FETCHED.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(&key).cloned())) {
        return Some(p);
    }
    if online {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<Job>();
            std::thread::spawn(move || {
                for job in rx {
                    let mut got = None;
                    if !job.flathub.is_empty() {
                        got = fetch(&job.flathub);
                    }
                    if got.is_none() && !job.url.is_empty() {
                        got = fetch_url(&job.id, &job.url);
                    }
                    if let Ok(mut g) = FETCHED.lock() {
                        g.get_or_insert_with(HashMap::new).insert(job.key, got.clone());
                    }
                    if got.is_some() {
                        job.ctx.request_repaint();
                    }
                    // one at a time, and not in a hurry
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
            });
            tx
        });
        let fresh = QUEUED.lock().ok().map(|mut g| g.get_or_insert_with(HashSet::new).insert(key.clone())).unwrap_or(false);
        if fresh {
            let _ = tx.send(Job { key, flathub: flathub.to_string(), id: id.to_string(), url: url.to_string(), ctx: ctx.clone() });
        }
    }
    None
}

/// Flathub's icon for an app, kept in the cache.
fn fetch(id: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("curl")
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--max-time",
            "20",
            "--max-filesize",
            "3000000",
            &format!("https://flathub.org/api/v2/appstream/{id}"),
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let url = icon_url(&String::from_utf8_lossy(&out.stdout))?;
    let got = crate::details::fetch_image(&url)?;
    let dest = cache_file(id)?;
    std::fs::create_dir_all(dest.parent()?).ok()?;
    std::fs::copy(&got, &dest).ok()?;
    Some(dest)
}

/// The icon address in Flathub's AppStream JSON (https only).
pub fn icon_url(json_text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json_text).ok()?;
    v.get("icon").and_then(|u| u.as_str()).filter(|u| u.starts_with("https://")).map(String::from)
}

/// Whether an `icon_url` from the catalog may be fetched: https, a host, and nothing that is not a plain address character.
pub fn fetchable(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else { return false };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    !host.is_empty() && !host.contains('@') && url.len() <= 2048 && url.chars().all(|c| c.is_ascii_graphic() && !"\"<>\\^`{|}".contains(c))
}

/// The cache file name of an app's icon from its catalog address: the app's id (plain characters only) and a hash of the address,
/// so a new address (a newer pinned commit) is fetched again. None for an id that could name another place.
pub fn url_cache_name(id: &str, url: &str, kind: Kind) -> Option<String> {
    if !crate::details::safe_id(id) || id.starts_with('.') || id.len() > 128 {
        return None;
    }
    let mut h: u64 = 1469598103934665603;
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    Some(format!("app-{id}-{h:016x}.{}", kind.ext()))
}

fn url_cached(id: &str, url: &str) -> Option<PathBuf> {
    let dir = crate::details::icon_cache_dir()?;
    [Kind::Png, Kind::Svg].into_iter().filter_map(|k| url_cache_name(id, url, k)).map(|n| dir.join(n)).find(|p| p.is_file())
}

/// The icon at a catalog's address, kept in the cache when its content is a PNG or SVG within the limits. https only, redirects
/// included; at most `URL_ICON_MAX_BYTES`.
fn fetch_url(id: &str, url: &str) -> Option<PathBuf> {
    if !fetchable(url) {
        return None;
    }
    let dir = crate::details::icon_cache_dir()?;
    let tmp = dir.join(format!("{}.part", url_cache_name(id, url, Kind::Png)?));
    let ok = std::process::Command::new("curl")
        .args(["-fsSL", "--proto", "=https", "--proto-redir", "=https", "--max-redirs", "3", "--max-time", "20"])
        .args(["--max-filesize", &URL_ICON_MAX_BYTES.to_string(), "-o", &tmp.to_string_lossy(), "--", url])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let kept = ok
        .then(|| std::fs::read(&tmp).ok())
        .flatten()
        .and_then(|b| accept(&b))
        .and_then(|k| url_cache_name(id, url, k))
        .map(|n| dir.join(n))
        .filter(|dest| std::fs::rename(&tmp, dest).is_ok());
    let _ = std::fs::remove_file(&tmp);
    kept
}

/// A downloaded icon's kind, if it may be kept: not empty, not over `URL_ICON_MAX_BYTES`, and a PNG or an SVG by its content.
pub fn accept(bytes: &[u8]) -> Option<Kind> {
    if bytes.is_empty() || bytes.len() > URL_ICON_MAX_BYTES {
        return None;
    }
    if let Some((w, h)) = png_size(bytes) {
        return (w > 0 && h > 0 && f64::from(w.max(h)) <= URL_ICON_MAX_SIDE).then_some(Kind::Png);
    }
    svg_ok(bytes).then_some(Kind::Svg)
}

/// A PNG's width and height from its header (the signature, then the IHDR chunk).
fn png_size(b: &[u8]) -> Option<(u32, u32)> {
    if b.len() < 24 || &b[..8] != b"\x89PNG\r\n\x1a\n" || &b[12..16] != b"IHDR" {
        return None;
    }
    let be = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
    Some((be(16), be(20)))
}

/// An SVG that draws only itself: well-formed XML (no DTD, so no entities) with an `svg` root, no scripts, embedded pages or
/// pictures, links only inside the file, and no declared size over the limit.
fn svg_ok(b: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(b)) else { return false };
    let Ok(doc) = roxmltree::Document::parse(text) else { return false };
    let root = doc.root_element();
    if root.tag_name().name() != "svg" {
        return false;
    }
    let side = |a: &str| {
        root.attribute(a).is_none_or(|v| {
            let n: String = v.trim().chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
            n.parse::<f64>().map_or(true, |n| n <= URL_ICON_MAX_SIDE)
        })
    };
    if !side("width") || !side("height") {
        return false;
    }
    // url(...) in an attribute or a style sheet may point only inside the file (url(#id))
    let outside = |text: &str| {
        let v: String = text.to_ascii_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
        v.match_indices("url(").any(|(i, _)| !v[i + 4..].trim_start_matches(['"', '\'']).starts_with('#')) || v.contains("@import")
    };
    root.descendants().filter(|n| n.is_element()).all(|n| {
        let name = n.tag_name().name();
        !["script", "foreignObject", "image", "feImage"].contains(&name)
            && !(name == "style" && n.descendants().filter_map(|t| t.text()).any(outside))
            && n.attributes().all(|a| {
                let link_out = a.name() == "href" && !a.value().trim().starts_with('#');
                !link_out && !a.name().starts_with("on") && !outside(a.value())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_address_is_read_and_only_https_is_taken() {
        assert_eq!(
            icon_url(r#"{"icon":"https://dl.flathub.org/media/a/icons/128x128/a.png"}"#).as_deref(),
            Some("https://dl.flathub.org/media/a/icons/128x128/a.png")
        );
        assert_eq!(icon_url(r#"{"icon":"http://x/y.png"}"#), None);
        assert_eq!(icon_url(r#"{}"#), None);
        assert_eq!(icon_url("not json"), None);
    }

    #[test]
    fn the_index_finds_the_part_after_the_package_name() {
        let t = std::env::temp_dir().join(format!("vendor-test-{}", std::process::id()));
        std::fs::create_dir_all(&t).unwrap();
        std::fs::write(t.join("firefox_firefox.png"), b"x").unwrap();
        let mut m: HashMap<String, PathBuf> = HashMap::new();
        for e in std::fs::read_dir(&t).unwrap().flatten() {
            let stem = e.path().file_stem().unwrap().to_string_lossy().to_lowercase();
            m.entry(stem.clone()).or_insert(e.path());
            if let Some((_, a)) = stem.split_once('_') {
                m.entry(a.to_string()).or_insert(e.path());
            }
        }
        assert!(m.contains_key("firefox"));
        let _ = std::fs::remove_dir_all(t);
    }

    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut b = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        b.extend_from_slice(&w.to_be_bytes());
        b.extend_from_slice(&h.to_be_bytes());
        b.extend_from_slice(&[8, 6, 0, 0, 0, 0, 0, 0, 0]);
        b
    }

    #[test]
    fn a_catalog_icon_address_must_be_https() {
        let u = "https://raw.githubusercontent.com/storytold/pdfcraft/e191b0ea69b7dcc4e554c1c4e51b6214bec1c590/assets/app-icon/hicolor/128x128/apps/ai.storyteller.pdfcraft.png";
        assert!(fetchable(u));
        for bad in [
            "",
            "http://example.org/a.png",
            "file:///etc/passwd",
            "ftp://example.org/a.png",
            "https://",
            "https:///a.png",
            "https://user@example.org/a.png",
            "https://example.org/a b.png",
            "https://example.org/a.png\n-o/tmp/x",
            "HTTPS://example.org/a.png",
        ] {
            assert!(!fetchable(bad), "{bad:?}");
        }
        assert!(!fetchable(&format!("https://example.org/{}", "a".repeat(2100))));
    }

    #[test]
    fn only_png_and_svg_are_kept_judged_by_their_content() {
        assert_eq!(accept(&png(128, 128)), Some(Kind::Png));
        assert_eq!(accept(br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16"><circle r="8"/></svg>"#), Some(Kind::Svg));
        assert_eq!(
            accept(br##"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="128" height="128"><defs><linearGradient id="g"/></defs><rect fill="url(#g)" width="9" height="9"/><use xlink:href="#g"/></svg>"##),
            Some(Kind::Svg),
            "links inside the file are fine"
        );
        for refused in [
            &b"\xff\xd8\xff\xe0\0\x10JFIF"[..],               // JPEG
            b"GIF89a\x01\0\x01\0",                            // GIF
            b"RIFF\0\0\0\0WEBPVP8 ",                          // WebP
            b"<!doctype html><html><body>404</body></html>", // a web page
            b"Not Found",
            b"",
            &png(128, 128)[..20], // a PNG cut short
            br#"<svg xmlns="http://www.w3.org/2000/svg"><script>alert(1)</script></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" onload="x()"/>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><image xlink:href="file:///etc/passwd"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink"><use xlink:href="https://e.org/a.svg#x"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><rect fill="url(https://e.org/p)"/></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><style>@import "https://e.org/a.css";</style></svg>"#,
            br#"<svg xmlns="http://www.w3.org/2000/svg"><foreignObject><p>hi</p></foreignObject></svg>"#,
            br#"<!DOCTYPE svg [<!ENTITY a "aaaa">]><svg xmlns="http://www.w3.org/2000/svg">&a;</svg>"#,
            br#"<html><svg/></html>"#,
        ] {
            assert_eq!(accept(refused), None, "{:?}", String::from_utf8_lossy(refused));
        }
    }

    #[test]
    fn oversized_icons_are_refused() {
        assert_eq!(accept(&png(1024, 1024)), Some(Kind::Png));
        assert_eq!(accept(&png(4096, 128)), None, "too many pixels");
        assert_eq!(accept(&png(0, 128)), None);
        let mut big = png(128, 128);
        big.resize(URL_ICON_MAX_BYTES + 1, 0);
        assert_eq!(accept(&big), None, "too many bytes");
        assert_eq!(accept(br#"<svg xmlns="http://www.w3.org/2000/svg" width="20000px" height="16"/>"#), None);
        let padded = format!(r#"<svg xmlns="http://www.w3.org/2000/svg"><!--{}--></svg>"#, "x".repeat(URL_ICON_MAX_BYTES));
        assert_eq!(accept(padded.as_bytes()), None);
    }

    /// Needs the internet and writes into $XDG_CACHE_HOME: `XDG_CACHE_HOME=/tmp/x cargo test -- --ignored`.
    #[test]
    #[ignore]
    fn a_pinned_artcraft_icon_is_fetched_and_kept() {
        let u = "https://raw.githubusercontent.com/storytold/pdfcraft/e191b0ea69b7dcc4e554c1c4e51b6214bec1c590/assets/app-icon/hicolor/128x128/apps/ai.storyteller.pdfcraft.png";
        let p = fetch_url("pdfcraft", u).expect("fetched");
        assert_eq!(accept(&std::fs::read(&p).unwrap()), Some(Kind::Png));
        assert_eq!(url_cached("pdfcraft", u), Some(p));
        assert_eq!(fetch_url("notthere", "https://raw.githubusercontent.com/storytold/pdfcraft/main/no-such-file.png"), None);
    }

    #[test]
    fn the_cache_name_is_the_app_id_and_cannot_leave_the_folder() {
        let n = url_cache_name("pdfcraft", "https://e.org/a.png", Kind::Png).unwrap();
        assert!(n.starts_with("app-pdfcraft-") && n.ends_with(".png"), "{n}");
        assert!(n.chars().all(|c| c.is_ascii_alphanumeric() || "-.".contains(c)));
        assert_ne!(n, url_cache_name("pdfcraft", "https://e.org/b.png", Kind::Png).unwrap(), "a new address is fetched again");
        assert!(url_cache_name("pdfcraft", "https://e.org/a.svg", Kind::Svg).unwrap().ends_with(".svg"));
        for bad in ["", "../x", "a/b", "..", ".hidden", "a b", "a\0b", "ä"] {
            assert_eq!(url_cache_name(bad, "https://e.org/a.png", Kind::Png), None, "{bad:?}");
        }
        assert_eq!(url_cache_name(&"a".repeat(200), "https://e.org/a.png", Kind::Png), None);
    }
}
