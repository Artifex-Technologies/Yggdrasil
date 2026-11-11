//! GUI-independent core of minos: per-tab history and address-bar
//! expansion. The egui front end and udisks2/inotify layers come later.
//! Feature set modelled on EdenExplorer (MIT, https://github.com/mtucciarone/EdenExplorer).

pub mod archive;
pub mod cloud;
pub mod compat;
pub mod devices;
pub mod fileops;
pub mod fmt;
pub mod listing;
pub mod places;
pub mod preview;
pub mod run;
pub mod search;
pub mod state;

use std::path::{Path, PathBuf};

/// Back/forward history for one tab.
#[derive(Debug, Clone)]
pub struct History {
    entries: Vec<PathBuf>,
    cursor: usize,
}

impl History {
    pub fn new(start: impl Into<PathBuf>) -> Self {
        Self { entries: vec![start.into()], cursor: 0 }
    }

    pub fn current(&self) -> &Path {
        &self.entries[self.cursor]
    }

    /// Navigate to a new location, dropping any forward history.
    pub fn visit(&mut self, path: impl Into<PathBuf>) {
        let path = path.into();
        if path == self.entries[self.cursor] {
            return;
        }
        self.entries.truncate(self.cursor + 1);
        self.entries.push(path);
        self.cursor += 1;
    }

    pub fn can_back(&self) -> bool {
        self.cursor > 0
    }

    pub fn can_forward(&self) -> bool {
        self.cursor + 1 < self.entries.len()
    }

    pub fn back(&mut self) -> Option<&Path> {
        if self.cursor == 0 {
            return None;
        }
        self.cursor -= 1;
        Some(self.current())
    }

    pub fn forward(&mut self) -> Option<&Path> {
        if self.cursor + 1 >= self.entries.len() {
            return None;
        }
        self.cursor += 1;
        Some(self.current())
    }

    /// Parent directory of the current location, if any.
    pub fn up(&self) -> Option<PathBuf> {
        self.current().parent().map(Path::to_path_buf)
    }
}

/// A tab; pinned tabs survive "close others" and are restored at startup.
#[derive(Debug, Clone)]
pub struct Tab {
    pub history: History,
    pub pinned: bool,
}

/// Expand `~` and `$VAR` / `${VAR}` in an address-bar string using `lookup`.
/// Unknown variables are left as written.
pub fn expand_address(input: &str, lookup: impl Fn(&str) -> Option<String>) -> String {
    let mut out = String::new();
    let chars: Vec<char> = input.chars().collect();
    let mut i = 0;
    if chars.first() == Some(&'~') && (chars.len() == 1 || chars[1] == '/') {
        match lookup("HOME") {
            Some(h) => out.push_str(&h),
            None => out.push('~'),
        }
        i = 1;
    }
    while i < chars.len() {
        if chars[i] != '$' {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let braced = chars.get(i + 1) == Some(&'{');
        let start = if braced { i + 2 } else { i + 1 };
        let mut end = start;
        while end < chars.len() && (chars[end].is_alphanumeric() || chars[end] == '_') {
            end += 1;
        }
        let name: String = chars[start..end].iter().collect();
        let close_ok = !braced || chars.get(end) == Some(&'}');
        match (name.is_empty() || !close_ok, lookup(&name)) {
            (false, Some(v)) => {
                out.push_str(&v);
                i = if braced { end + 1 } else { end };
            }
            _ => {
                out.push('$');
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(name: &str) -> Option<String> {
        match name {
            "HOME" => Some("/home/me".into()),
            "MUSIC" => Some("/data/music".into()),
            _ => None,
        }
    }

    #[test]
    fn history_back_forward_and_truncation() {
        let mut h = History::new("/a");
        h.visit("/a/b");
        h.visit("/a/b/c");
        assert!(h.can_back() && !h.can_forward());
        assert_eq!(h.back(), Some(Path::new("/a/b")));
        assert_eq!(h.back(), Some(Path::new("/a")));
        assert!(!h.can_back() && h.can_forward());
        assert_eq!(h.back(), None);
        assert_eq!(h.forward(), Some(Path::new("/a/b")));
        h.visit("/x");
        assert_eq!(h.forward(), None);
        assert_eq!(h.up(), Some(PathBuf::from("/")));
    }

    #[test]
    fn address_expansion() {
        assert_eq!(expand_address("~/Pictures", env), "/home/me/Pictures");
        assert_eq!(expand_address("$MUSIC/live", env), "/data/music/live");
        assert_eq!(expand_address("${MUSIC}x", env), "/data/musicx");
        assert_eq!(expand_address("$NOPE/x", env), "$NOPE/x");
        assert_eq!(expand_address("~other", env), "~other");
    }
}
