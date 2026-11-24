//! The searches made in Agora, newest first: shown under the search field while it is empty and in use, and cleared from
//! there. Kept in `~/.local/state/yggdrasil/agora-searches` (one per line), on this computer only.

use std::path::PathBuf;

/// How many are kept.
const KEPT: usize = 8;

pub struct Recent {
    path: PathBuf,
    pub items: Vec<String>,
}

impl Recent {
    pub fn load(path: PathBuf) -> Recent {
        let items = std::fs::read_to_string(&path)
            .map(|t| t.lines().map(str::trim).filter(|l| !l.is_empty()).take(KEPT).map(str::to_string).collect())
            .unwrap_or_default();
        Recent { path, items }
    }

    pub fn path() -> PathBuf {
        yggdrasil_conf::dirs::state().join("agora-searches")
    }

    /// Puts `query` first (once, whatever its case: the newest spelling wins), keeping the newest `KEPT`.
    pub fn remember(&mut self, query: &str) {
        let q = query.trim();
        if q.is_empty() || q.chars().any(char::is_control) {
            return;
        }
        let low = q.to_lowercase();
        self.items.retain(|x| x.to_lowercase() != low);
        self.items.insert(0, q.to_string());
        self.items.truncate(KEPT);
        self.save();
    }

    pub fn clear(&mut self) {
        self.items.clear();
        let _ = std::fs::remove_file(&self.path);
    }

    fn save(&self) {
        if let Some(d) = self.path.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(&self.path, self.items.join("\n") + "\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn searches_are_kept_newest_first_once_each_and_can_be_cleared() {
        let d = std::env::temp_dir().join(format!("agora-recent-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let f = d.join("agora-searches");
        let mut r = Recent::load(f.clone());
        assert!(r.items.is_empty());
        for q in ["krita", "  blender ", "", "KRITA", "line\nbreak"] {
            r.remember(q);
        }
        assert_eq!(r.items, ["KRITA", "blender"], "trimmed, once each, newest first; empty and odd ones left out");
        for i in 0..10 {
            r.remember(&format!("q{i}"));
        }
        assert_eq!(r.items.len(), 8);
        assert_eq!(r.items[0], "q9");
        let again = Recent::load(f.clone());
        assert_eq!(again.items, r.items, "kept on disk");
        r.clear();
        assert!(r.items.is_empty() && !f.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}
