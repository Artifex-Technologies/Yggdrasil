//! The icons the vendors made, for everything in the Marketplace: an app is shown with its own icon (the one in its package, its Flatpak, the
//! AppStream catalog, or Flathub), not with a restyled one from the icon pack. Only an app with no vendor icon anywhere falls back to the
//! pack's (Liki's) icon, and then to its default app icon.
//!
//! Found without network when possible: an index of the icon files on this computer is made once (hicolor, pixmaps, the Flatpak exports, the
//! AppStream icon caches). For an app that is not here but is on Flathub, its icon is fetched in the background (one at a time), only when the
//! "online details" switch is on (the same one that allows screenshots), and kept in the Marketplace's cache.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Mutex, OnceLock};

use eframe::egui;

static INDEX: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
static FETCHED: Mutex<Option<HashMap<String, Option<PathBuf>>>> = Mutex::new(None);
static QUEUED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
static TX: OnceLock<mpsc::Sender<(String, egui::Context)>> = OnceLock::new();

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
    for root in ["/var/lib/flatpak/appstream"].into_iter().map(PathBuf::from).chain(home.iter().map(|h| h.join(".local/share/flatpak/appstream"))) {
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

/// The vendor's icon file for an app, if there is one now. Starts a background fetch from Flathub when there is none and fetching is allowed;
/// the window is asked to draw again when it arrives.
pub fn find(names: &[&str], flathub: &str, online: bool, ctx: &egui::Context) -> Option<PathBuf> {
    let index = INDEX.get_or_init(build_index);
    for n in names {
        if let Some(p) = index.get(&n.to_lowercase()) {
            return Some(p.clone());
        }
    }
    if flathub.is_empty() || !crate::details::safe_id(flathub) {
        return None;
    }
    if let Some(p) = cache_file(flathub).filter(|p| p.is_file()) {
        return Some(p);
    }
    if let Some(Some(p)) = FETCHED.lock().ok().and_then(|g| g.as_ref().and_then(|m| m.get(flathub).cloned())) {
        return Some(p);
    }
    if online {
        let tx = TX.get_or_init(|| {
            let (tx, rx) = mpsc::channel::<(String, egui::Context)>();
            std::thread::spawn(move || {
                for (id, ctx) in rx {
                    let got = fetch(&id);
                    if let Ok(mut g) = FETCHED.lock() {
                        g.get_or_insert_with(HashMap::new).insert(id, got.clone());
                    }
                    if got.is_some() {
                        ctx.request_repaint();
                    }
                    // one at a time, and not in a hurry
                    std::thread::sleep(std::time::Duration::from_millis(120));
                }
            });
            tx
        });
        let fresh = QUEUED.lock().ok().map(|mut g| g.get_or_insert_with(HashSet::new).insert(flathub.to_string())).unwrap_or(false);
        if fresh {
            let _ = tx.send((flathub.to_string(), ctx.clone()));
        }
    }
    None
}

/// Flathub's icon for an app, kept in the cache.
fn fetch(id: &str) -> Option<PathBuf> {
    let out = std::process::Command::new("curl").args(["-fsSL", "--proto", "=https", "--max-time", "20", "--max-filesize", "3000000", &format!("https://flathub.org/api/v2/appstream/{id}")]).output().ok()?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_icon_address_is_read_and_only_https_is_taken() {
        assert_eq!(icon_url(r#"{"icon":"https://dl.flathub.org/media/a/icons/128x128/a.png"}"#).as_deref(), Some("https://dl.flathub.org/media/a/icons/128x128/a.png"));
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
}
