//! The user's word list (~/.local/share/yggdrasil/spell/words.txt: one word per line, then a tab and the apps it came from) and the
//! list of words the user took out (removed.txt), so a removal reaches every app instead of coming back from one of them. The
//! computer's shared list is /var/lib/yggdrasil/spell/global.txt (global.rs: two accounts offer a word, or an administrator approves
//! it). What counts as a word for these lists is `valid`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

pub const GLOBAL: &str = "/var/lib/yggdrasil/spell/global.txt";

pub fn dir() -> PathBuf {
    yggdrasil_conf::dirs::data().join("spell")
}

/// A word that may go on a list: 1 to 99 bytes (Chromium's limit), at least one letter, no spaces or control characters, and not a
/// link, an email address or a long run of digits and letters that looks like a code or a key.
pub fn valid(w: &str) -> bool {
    let n = w.len();
    if n == 0 || n > 99 || !w.chars().any(char::is_alphabetic) || w.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    if w.contains("://") || w.contains('@') || w.starts_with("www.") {
        return false;
    }
    let digits = w.chars().filter(char::is_ascii_digit).count();
    !(n >= 12 && digits >= 3 && w.chars().all(|c| c.is_ascii_alphanumeric() || "_-+/=".contains(c)))
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct WordList {
    /// word -> the apps it came from ("" when added in Yggdrasil itself)
    words: BTreeMap<String, BTreeSet<String>>,
    removed: BTreeSet<String>,
}

impl WordList {
    pub fn load() -> WordList {
        Self::load_from(&dir())
    }

    pub fn load_from(dir: &std::path::Path) -> WordList {
        let mut l = WordList::default();
        for line in std::fs::read_to_string(dir.join("words.txt")).unwrap_or_default().lines() {
            if line.starts_with('#') {
                continue;
            }
            let (w, from) = line.split_once('\t').unwrap_or((line, ""));
            if valid(w) {
                l.words.entry(w.to_string()).or_default().extend(from.split(',').filter(|s| !s.is_empty()).map(str::to_string));
            }
        }
        for line in std::fs::read_to_string(dir.join("removed.txt")).unwrap_or_default().lines() {
            if valid(line) {
                l.removed.insert(line.to_string());
            }
        }
        l
    }

    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(&dir())
    }

    pub fn save_to(&self, dir: &std::path::Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dir)?;
        let mut s = String::from("# Your words (Settings > Keyboard > Spelling and languages): word, a tab, the apps it came from\n");
        for (w, from) in &self.words {
            s.push_str(w);
            if !from.is_empty() {
                s.push('\t');
                s.push_str(&from.iter().cloned().collect::<Vec<_>>().join(","));
            }
            s.push('\n');
        }
        write_atomic(&dir.join("words.txt"), &s)?;
        write_atomic(&dir.join("removed.txt"), &self.removed.iter().map(|w| format!("{w}\n")).collect::<String>())
    }

    pub fn words(&self) -> impl Iterator<Item = &str> {
        self.words.keys().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.words.len()
    }

    pub fn is_empty(&self) -> bool {
        self.words.is_empty()
    }

    /// Words with the apps they came from.
    pub fn entries(&self) -> impl Iterator<Item = (&str, Vec<&str>)> {
        self.words.iter().map(|(w, f)| (w.as_str(), f.iter().map(String::as_str).collect()))
    }

    pub fn contains(&self, w: &str) -> bool {
        self.words.contains_key(w)
    }

    pub fn is_removed(&self, w: &str) -> bool {
        self.removed.contains(w)
    }

    /// Adds a word (from an app, or `""` for the user in Yggdrasil). A word the user removed is not taken back from an app; the user
    /// adding it again in Yggdrasil does take it back. Returns whether the list changed.
    pub fn add(&mut self, w: &str, from: &str) -> bool {
        if !valid(w) || self.removed.contains(w) && !from.is_empty() {
            return false;
        }
        let un_removed = self.removed.remove(w);
        let new_word = !self.words.contains_key(w);
        let e = self.words.entry(w.to_string()).or_default();
        let new_source = !from.is_empty() && e.insert(from.to_string());
        new_word || new_source || un_removed
    }

    pub fn remove(&mut self, w: &str) -> bool {
        let had = self.words.remove(w).is_some();
        self.removed.insert(w.to_string()) || had
    }
}

/// The computer's shared words (global.rs decides which).
pub fn global() -> Vec<String> {
    read_global(std::path::Path::new(GLOBAL))
}

pub fn read_global(path: &std::path::Path) -> Vec<String> {
    std::fs::read_to_string(path).unwrap_or_default().lines().filter(|l| !l.starts_with('#') && valid(l)).map(str::to_string).collect()
}

pub(crate) fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(".{}.{}.tmp", path.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_a_word() {
        for w in ["Yggdrasil", "don't", "café", "x86_64", "Heimdall's", "e-mail"] {
            assert!(valid(w), "{w}");
        }
        for w in ["", "two words", "https://x.org", "me@x.org", "1234", "sk_live_51HxQ2aBcDeFgH12", "ghp_a1b2c3d4e5f6g7h8"] {
            assert!(!valid(w), "{w}");
        }
        assert!(!valid(&"a".repeat(100)));
    }

    #[test]
    fn add_remove_and_a_removal_sticks() {
        let d = std::env::temp_dir().join(format!("ygg-spell-words-{}", std::process::id()));
        let mut l = WordList::default();
        assert!(l.add("Fylgja", "Vivaldi"));
        assert!(l.add("Fylgja", "Obsidian"));
        assert!(!l.add("Fylgja", "Vivaldi"));
        assert!(l.remove("Fylgja"));
        assert!(!l.add("Fylgja", "Firefox"), "an app does not bring back a word the user removed");
        assert!(l.add("Fylgja", ""), "the user can");
        l.save_to(&d).unwrap();
        let again = WordList::load_from(&d);
        assert!(again.contains("Fylgja") && !again.is_removed("Fylgja"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
