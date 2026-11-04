//! The apps' own "add to dictionary" lists, and `sync`, which merges them all into the user's word list and writes the result back
//! to each. Found by what is on disk, not by a list of app names, so a newly installed Electron app joins by itself:
//!
//! - GTK apps (through enchant): ~/.config/enchant/<language>.dic, one per switched-on language; one word per line.
//! - Firefox: persdict.dat in each profile (~/.mozilla/firefox/* and ~/.config/mozilla/firefox/*); one word per line.
//! - Chromium browsers and every Electron app: "Custom Dictionary.txt" in each profile folder (a folder under ~/.config with a
//!   `Preferences` file, where it or its parent has `Local State`): one word per line, then `checksum_v1 = ` and the MD5 of the
//!   words part; Chromium restores its backup copy when that checksum is wrong, so it is always written right.
//!
//! A running browser reads its list when it starts, so a word added elsewhere reaches it at its next start. A word is only taken
//! out everywhere when the user removes it in Yggdrasil (an app writing its list without a word is not a removal: Chromium rewrites
//! its whole list from memory).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::words::{valid, WordList};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Lines,
    Chromium,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bridge {
    /// What the user sees: "Vivaldi", "VS Code", "Firefox", "GTK apps (en_US)".
    pub app: String,
    pub path: PathBuf,
    pub format: Format,
}

impl Bridge {
    pub fn read(&self) -> BTreeSet<String> {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        let body = match self.format {
            Format::Chromium => text.rfind(CHECKSUM).map(|i| &text[..i]).unwrap_or(&text),
            Format::Lines => &text,
        };
        body.lines().map(str::trim).filter(|w| !w.starts_with('#') && valid(w)).map(str::to_string).collect()
    }

    /// Writes the words (only when they differ from what is there). Returns whether the file was written.
    pub fn write(&self, words: &BTreeSet<String>) -> std::io::Result<bool> {
        if self.path.exists() && &self.read() == words {
            return Ok(false);
        }
        if let Some(d) = self.path.parent() {
            std::fs::create_dir_all(d)?;
        }
        crate::words::write_atomic(&self.path, &render(self.format, words))?;
        Ok(true)
    }
}

const CHECKSUM: &str = "checksum_v1 = ";

/// The file's text for these words (sorted, one per line; Chromium's with its checksum line).
pub fn render(format: Format, words: &BTreeSet<String>) -> String {
    let body: String = words.iter().map(|w| format!("{w}\n")).collect();
    match format {
        Format::Lines => body,
        Format::Chromium => format!("{body}{CHECKSUM}{}", md5_hex(body.as_bytes())),
    }
}

/// Every app list on this account, for the given languages.
pub fn bridges(languages: &[String]) -> Vec<Bridge> {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
    let config = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).unwrap_or_else(|| home.join(".config"));
    bridges_in(&home, &config, languages)
}

pub fn bridges_in(home: &Path, config: &Path, languages: &[String]) -> Vec<Bridge> {
    let mut out = vec![];
    for code in languages {
        out.push(Bridge { app: format!("GTK apps ({code})"), path: config.join("enchant").join(format!("{code}.dic")), format: Format::Lines });
    }
    for root in [home.join(".mozilla/firefox"), config.join("mozilla/firefox")] {
        for p in subdirs(&root) {
            if p.join("prefs.js").is_file() || p.join("times.json").is_file() {
                out.push(Bridge { app: "Firefox".into(), path: p.join("persdict.dat"), format: Format::Lines });
            }
        }
    }
    // Chromium-family profiles: three levels under ~/.config cover ~/.config/<app>, <app>/<profile> and <vendor>/<app>/<profile>
    let mut found = vec![];
    collect_chromium(config, config, 0, &mut found);
    found.sort();
    for p in found {
        out.push(Bridge { app: chromium_app_name(config, &p), path: p.join("Custom Dictionary.txt"), format: Format::Chromium });
    }
    out
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir() && !p.is_symlink()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn collect_chromium(config: &Path, dir: &Path, depth: usize, out: &mut Vec<PathBuf>) {
    if depth > 3 {
        return;
    }
    for p in subdirs(dir) {
        let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
        // big folders that are never a browser profile
        if depth == 0 && matches!(name.as_str(), "yggdrasil" | "enchant" | "mozilla" | "systemd" | "pulse" | "dconf" | "gtk-3.0" | "gtk-4.0") {
            continue;
        }
        let local_state = p.join("Local State").is_file() || p.parent().is_some_and(|q| q.join("Local State").is_file());
        if p.join("Preferences").is_file() && local_state {
            out.push(p.clone());
        }
        if !name.starts_with('.') && !matches!(name.as_str(), "Cache" | "Code Cache" | "GPUCache" | "Service Worker" | "IndexedDB" | "Local Storage" | "blob_storage" | "extensions" | "Extensions") {
            collect_chromium(config, &p, depth + 1, out);
        }
    }
}

/// "Vivaldi", "VS Code", "Obsidian", "Chromium (Profile 1)": the folder under ~/.config, with the profile when it is not the default one.
fn chromium_app_name(config: &Path, profile: &Path) -> String {
    let rel = profile.strip_prefix(config).unwrap_or(profile);
    let parts: Vec<String> = rel.components().map(|c| c.as_os_str().to_string_lossy().to_string()).collect();
    let app = parts.iter().find(|p| !matches!(p.as_str(), "BraveSoftware" | "google-chrome-for-testing")).cloned().unwrap_or_default();
    let app = match app.as_str() {
        "Code" => "VS Code".to_string(),
        "google-chrome" => "Chrome".to_string(),
        "Brave-Browser" => "Brave".to_string(),
        a => {
            let mut c = a.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    };
    match parts.last().map(String::as_str) {
        Some(last) if parts.len() > 1 && last != "Default" && !app.eq_ignore_ascii_case(last) => format!("{app} ({last})"),
        _ => app,
    }
}

/// What a sync did.
#[derive(Debug, Default)]
pub struct Report {
    /// (word, app) for every word an app brought in
    pub added: Vec<(String, String)>,
    /// the lists that were written
    pub written: Vec<PathBuf>,
    pub errors: Vec<String>,
}

/// Merges every app list into the user's word list, then writes the user's words (and only those removed in Yggdrasil taken out)
/// back to every app list.
pub fn sync(list: &mut WordList, bridges: &[Bridge]) -> Report {
    let mut r = Report::default();
    let mut seen: Vec<(usize, BTreeSet<String>)> = vec![];
    for (i, b) in bridges.iter().enumerate() {
        let words = b.read();
        for w in &words {
            let new = !list.contains(w);
            if list.add(w, &b.app) && new {
                r.added.push((w.clone(), b.app.clone()));
            }
        }
        seen.push((i, words));
    }
    let all: BTreeSet<String> = list.words().map(str::to_string).collect();
    for (i, words) in seen {
        let b = &bridges[i];
        // keep what the app has (unless removed in Yggdrasil), add everything the user has
        let want: BTreeSet<String> = words.into_iter().filter(|w| !list.is_removed(w)).chain(all.iter().cloned()).collect();
        // no empty files where an app had none
        if !b.path.exists() && want.is_empty() {
            continue;
        }
        match b.write(&want) {
            Ok(true) => r.written.push(b.path.clone()),
            Ok(false) => {}
            Err(e) => r.errors.push(format!("{}: {e}", b.path.display())),
        }
    }
    r
}

/// The whole round for this account: the user's list and every app list for the switched-on languages, one sync at a time (the
/// watcher, Settings and the command can all start one; a lock file in the word-list folder makes them take turns).
pub fn sync_all() -> Result<(Report, usize, usize), String> {
    let _lock = lock()?;
    sync_locked()
}

/// Changes the user's word list (adds or removes words in Yggdrasil itself), then syncs, all under the lock, so a sync that runs at
/// the same moment cannot undo the change.
pub fn edit_words(change: impl FnOnce(&mut WordList)) -> Result<(Report, usize, usize), String> {
    let _lock = lock()?;
    let mut list = WordList::load();
    change(&mut list);
    list.save().map_err(|e| e.to_string())?;
    sync_locked()
}

/// Held while the word lists are read and written; dropping it releases the lock.
pub struct Lock(#[allow(dead_code)] std::fs::File);

pub fn lock() -> Result<Lock, String> {
    let dir = crate::words::dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(".sync.lock")).map_err(|e| e.to_string())?;
    use std::os::fd::AsRawFd;
    // SAFETY: flock on a descriptor owned by the returned guard; closing it releases the lock
    unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) };
    Ok(Lock(f))
}

fn sync_locked() -> Result<(Report, usize, usize), String> {
    let mut list = WordList::load();
    let bridges = bridges(&crate::conf::SpellConf::load().active());
    let r = sync(&mut list, &bridges);
    list.save().map_err(|e| e.to_string())?;
    Ok((r, list.len(), bridges.len()))
}

/// MD5 (RFC 1321), only for Chromium's checksum line; nothing here relies on it for security.
pub fn md5_hex(data: &[u8]) -> String {
    const S: [u32; 64] = [7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
        4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4294967296.0) as u32).collect();
    let mut msg = data.to_vec();
    let bits = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bits.to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) = (0x67452301u32, 0xefcdab89u32, 0x98badcfeu32, 0x10325476u32);
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    [a0, b0, c0, d0].iter().flat_map(|v| v.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ygg-spell-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn md5_known_answers() {
        assert_eq!(md5_hex(b""), "d41d8cd98f00b204e9800998ecf8427e");
        assert_eq!(md5_hex(b"abc"), "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(md5_hex(b"The quick brown fox jumps over the lazy dog"), "9e107d9d372bb6826bd81d3542a419d6");
        assert_eq!(md5_hex(&[b'a'; 64]), "014842d480b571495a4a0363793f7367");
    }

    #[test]
    fn chromium_file_has_the_checksum_chromium_expects() {
        let words: BTreeSet<String> = ["Heimdall", "Yggdrasil"].iter().map(|s| s.to_string()).collect();
        let text = render(Format::Chromium, &words);
        assert_eq!(text, format!("Heimdall\nYggdrasil\nchecksum_v1 = {}", md5_hex(b"Heimdall\nYggdrasil\n")));
        let d = tmp("chromium");
        let b = Bridge { app: "Vivaldi".into(), path: d.join("Custom Dictionary.txt"), format: Format::Chromium };
        std::fs::write(&b.path, &text).unwrap();
        assert_eq!(b.read(), words, "the checksum line is not a word");
        assert!(!b.write(&words).unwrap(), "unchanged words are not written again");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn finds_browsers_electron_apps_firefox_and_gtk() {
        let home = tmp("home");
        let cfg = home.join(".config");
        for (dir, local_state_in) in [("vivaldi/Default", "vivaldi"), ("vivaldi/Profile 1", "vivaldi"), ("Code", "Code"), ("obsidian", "obsidian"), ("BraveSoftware/Brave-Browser/Default", "BraveSoftware/Brave-Browser")] {
            std::fs::create_dir_all(cfg.join(dir)).unwrap();
            std::fs::write(cfg.join(dir).join("Preferences"), "{}").unwrap();
            std::fs::write(cfg.join(local_state_in).join("Local State"), "{}").unwrap();
        }
        std::fs::create_dir_all(cfg.join("notabrowser")).unwrap();
        std::fs::write(cfg.join("notabrowser/Preferences"), "x").unwrap(); // no Local State: not Chromium
        std::fs::create_dir_all(home.join(".mozilla/firefox/abc.default")).unwrap();
        std::fs::write(home.join(".mozilla/firefox/abc.default/prefs.js"), "").unwrap();
        let apps: Vec<String> = bridges_in(&home, &cfg, &["en_US".to_string()]).into_iter().map(|b| b.app).collect();
        assert_eq!(apps, ["GTK apps (en_US)", "Firefox", "Brave", "VS Code", "Obsidian", "Vivaldi", "Vivaldi (Profile 1)"]);
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn a_word_added_in_one_app_reaches_the_others_and_a_removal_reaches_all() {
        let d = tmp("sync");
        let gtk = Bridge { app: "GTK apps (en_US)".into(), path: d.join("en_US.dic"), format: Format::Lines };
        let viv = Bridge { app: "Vivaldi".into(), path: d.join("Custom Dictionary.txt"), format: Format::Chromium };
        std::fs::write(&viv.path, render(Format::Chromium, &["Fylgja".to_string()].into_iter().collect())).unwrap();
        let bridges = vec![gtk.clone(), viv.clone()];
        let mut list = WordList::default();
        let r = sync(&mut list, &bridges);
        assert_eq!(r.added, [("Fylgja".to_string(), "Vivaldi".to_string())]);
        assert!(gtk.read().contains("Fylgja"), "Vivaldi's word reached the GTK apps");
        assert!(sync(&mut list, &bridges).written.is_empty(), "a second sync changes nothing (no loop with the watcher)");
        list.remove("Fylgja");
        sync(&mut list, &bridges);
        assert!(!gtk.read().contains("Fylgja") && !viv.read().contains("Fylgja"), "removed everywhere");
        assert!(!list.contains("Fylgja"), "and the app did not bring it back");
        let _ = std::fs::remove_dir_all(&d);
    }
}
