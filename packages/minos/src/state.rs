//! Persistent state: session tabs, favorites, tags, folder colors, view settings.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SavedTab {
    pub path: PathBuf,
    pub pinned: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Details,
    /// Thumbnail gallery; the number is the thumbnail edge in points.
    Gallery,
}

/// Four gallery thumbnail sizes, as in EdenExplorer.
pub const GALLERY_SIZES: [f32; 4] = [64.0, 96.0, 140.0, 200.0];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Tag {
    pub name: String,
    pub color: [u8; 3],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct State {
    pub tabs: Vec<SavedTab>,
    pub active: usize,
    /// Second pane tabs when split view is on.
    pub split: bool,
    pub right_tabs: Vec<SavedTab>,
    pub right_active: usize,
    #[serde(alias = "favourites")]
    pub favorites: Vec<PathBuf>,
    pub tag_defs: Vec<Tag>,
    /// path -> tag names (multi-tagging).
    pub tags: BTreeMap<PathBuf, Vec<String>>,
    /// Custom folder colors, independent of tags.
    pub folder_colors: BTreeMap<PathBuf, [u8; 3]>,
    pub show_hidden: bool,
    pub dark: bool,
    pub view: ViewMode,
    pub gallery_size: usize,
    /// The right-hand preview pane.
    pub preview: bool,
    /// Use the system's accent color (KDE Plasma) for Minos's highlights. On by default.
    pub follow_accent: bool,
    /// The system accent last read; not saved. Set by the app while it runs.
    #[serde(skip)]
    pub system_accent: Option<[u8; 3]>,
}

impl State {
    /// The accent to draw with: the system's, if following it and there is one.
    pub fn accent(&self) -> Option<[u8; 3]> {
        if self.follow_accent { self.system_accent } else { None }
    }
}

impl Default for State {
    fn default() -> Self {
        Self {
            tabs: vec![],
            active: 0,
            split: false,
            right_tabs: vec![],
            right_active: 0,
            favorites: vec![],
            tag_defs: vec![
                Tag { name: "Red".into(), color: [220, 70, 70] },
                Tag { name: "Orange".into(), color: [235, 150, 50] },
                Tag { name: "Green".into(), color: [80, 180, 90] },
                Tag { name: "Blue".into(), color: [80, 140, 230] },
                Tag { name: "Purple".into(), color: [160, 100, 210] },
            ],
            tags: BTreeMap::new(),
            folder_colors: BTreeMap::new(),
            show_hidden: false,
            dark: true,
            view: ViewMode::Details,
            gallery_size: 1,
            preview: true,
            follow_accent: true,
            system_accent: None,
        }
    }
}

impl State {
    pub fn config_path() -> Option<PathBuf> {
        dirs::config_dir().map(|d| d.join("minos").join("state.json"))
    }

    pub fn load_from(path: &Path) -> State {
        crate::compat::read_to_string(path)
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Write atomically (temp file then rename) so a crash cannot leave half a file.
    pub fn save_to(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?)?;
        std::fs::rename(tmp, path)
    }

    pub fn toggle_favorite(&mut self, p: &Path) {
        if let Some(i) = self.favorites.iter().position(|f| f == p) {
            self.favorites.remove(i);
        } else {
            self.favorites.push(p.to_path_buf());
        }
    }

    pub fn toggle_tag(&mut self, p: &Path, tag: &str) {
        let v = self.tags.entry(p.to_path_buf()).or_default();
        if let Some(i) = v.iter().position(|t| t == tag) {
            v.remove(i);
        } else {
            v.push(tag.to_string());
        }
        if v.is_empty() {
            self.tags.remove(p);
        }
    }

    /// Color to draw a path in: its first tag's color, else a custom folder color.
    pub fn color_for(&self, p: &Path) -> Option<[u8; 3]> {
        if let Some(names) = self.tags.get(p) {
            for n in names {
                if let Some(t) = self.tag_defs.iter().find(|t| &t.name == n) {
                    return Some(t.color);
                }
            }
        }
        self.folder_colors.get(p).copied()
    }

    /// Re-key tags, colors and favorites after a rename or move.
    pub fn rekey(&mut self, from: &Path, to: &Path) {
        if let Some(v) = self.tags.remove(from) {
            self.tags.insert(to.to_path_buf(), v);
        }
        if let Some(c) = self.folder_colors.remove(from) {
            self.folder_colors.insert(to.to_path_buf(), c);
        }
        for f in &mut self.favorites {
            if f == from {
                *f = to.to_path_buf();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_missing_fields() {
        let dir = std::env::temp_dir().join(format!("lf-state-{}", std::process::id()));
        let file = dir.join("state.json");
        let mut s = State::default();
        s.tabs.push(SavedTab { path: "/a".into(), pinned: true });
        s.toggle_favorite(Path::new("/fav"));
        s.toggle_tag(Path::new("/a"), "Red");
        s.save_to(&file).unwrap();
        let l = State::load_from(&file);
        assert_eq!(l.tabs, s.tabs);
        assert_eq!(l.favorites, vec![PathBuf::from("/fav")]);
        assert_eq!(l.color_for(Path::new("/a")), Some([220, 70, 70]));

        // Old/partial file still loads, filling defaults.
        std::fs::write(&file, r#"{"dark": false}"#).unwrap();
        let p = State::load_from(&file);
        assert!(!p.dark);
        assert!(!p.tag_defs.is_empty());
        // Corrupt file falls back to defaults rather than panicking.
        std::fs::write(&file, "{{{").unwrap();
        assert!(State::load_from(&file).dark);
    }

    #[test]
    fn tags_toggle_and_rekey() {
        let mut s = State::default();
        let a = Path::new("/a");
        s.toggle_tag(a, "Blue");
        s.toggle_tag(a, "Green");
        assert_eq!(s.tags[a].len(), 2);
        s.toggle_tag(a, "Blue");
        s.toggle_tag(a, "Green");
        assert!(s.tags.is_empty());
        s.toggle_tag(a, "Blue");
        s.folder_colors.insert(a.into(), [1, 2, 3]);
        s.rekey(a, Path::new("/b"));
        assert!(s.tags.contains_key(Path::new("/b")) && !s.tags.contains_key(a));
        assert_eq!(s.folder_colors[Path::new("/b")], [1, 2, 3]);
    }
}
