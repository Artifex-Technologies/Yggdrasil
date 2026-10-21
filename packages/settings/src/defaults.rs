//! Settings > Default apps: which program opens each kind of file (text and Markdown, PDFs, pictures, music, video, archives,
//! the web, folders, mail). The choices are the installed programs that say they can open that kind; the answer is stored the
//! standard way (`xdg-mime default`, which writes ~/.config/mimeapps.list), so every program that asks sees it.

use std::collections::BTreeMap;

use crate::appmgr;

/// One row: a kind of file and the types that go with it. The first type is the one asked about.
pub struct Kind {
    pub name: &'static str,
    pub what: &'static str,
    pub mimes: &'static [&'static str],
}

pub const KINDS: &[Kind] = &[
    Kind { name: "Text and Markdown", what: "notes, .md and .txt files", mimes: &["text/markdown", "text/x-markdown", "text/plain"] },
    Kind { name: "Documents", what: "Word, OpenDocument and RTF files", mimes: &["application/vnd.openxmlformats-officedocument.wordprocessingml.document", "application/msword", "application/vnd.oasis.opendocument.text", "application/rtf"] },
    Kind { name: "PDF", what: "PDF files", mimes: &["application/pdf"] },
    Kind { name: "Web browser", what: "links and web pages", mimes: &["x-scheme-handler/http", "x-scheme-handler/https", "text/html"] },
    Kind { name: "Pictures", what: "PNG, JPEG, GIF, WebP and SVG", mimes: &["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml"] },
    Kind { name: "Music", what: "MP3, FLAC, Ogg and WAV", mimes: &["audio/mpeg", "audio/flac", "audio/ogg", "audio/x-wav"] },
    Kind { name: "Video", what: "MP4, Matroska and WebM", mimes: &["video/mp4", "video/x-matroska", "video/webm"] },
    Kind { name: "Archives", what: "ZIP, tar, 7z and gzip", mimes: &["application/zip", "application/x-tar", "application/x-7z-compressed", "application/gzip"] },
    Kind { name: "Folders", what: "opening a folder", mimes: &["inode/directory"] },
    Kind { name: "Email", what: "mailto: links", mimes: &["x-scheme-handler/mailto"] },
];

/// A program that can be chosen: its desktop file name and the name shown.
#[derive(Clone, Debug, PartialEq)]
pub struct Choice {
    pub id: String,
    pub name: String,
}

/// What the page shows for one kind.
#[derive(Clone, Debug)]
pub struct Row {
    pub kind: usize,
    pub current: String,
    pub choices: Vec<Choice>,
}

/// The installed programs with the types they open (desktop file name, shown name, types).
fn programs() -> Vec<(String, String, Vec<String>)> {
    let mut seen = BTreeMap::new();
    for dir in appmgr::desktop_dirs() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let id = e.file_name().to_string_lossy().into_owned();
            if !id.ends_with(".desktop") || seen.contains_key(&id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(e.path()) else { continue };
            let d = appmgr::parse_desktop(&text);
            if d.get("NoDisplay").map(|v| v == "true").unwrap_or(false) || d.get("Hidden").map(|v| v == "true").unwrap_or(false) {
                continue;
            }
            let (Some(name), Some(mt)) = (d.get("Name"), d.get("MimeType")) else { continue };
            seen.insert(id, (name.clone(), mt.split(';').filter(|s| !s.is_empty()).map(String::from).collect::<Vec<_>>()));
        }
    }
    seen.into_iter().map(|(id, (n, m))| (id, n, m)).collect()
}

/// The current answer for a type, as a desktop file name ("" when nothing is set).
fn current(mime: &str) -> String {
    crate::sys::run("xdg-mime", &["query", "default", mime]).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// Reads everything the page needs.
pub fn load() -> Vec<Row> {
    let progs = programs();
    KINDS
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let mut choices: Vec<Choice> = progs.iter().filter(|(_, _, m)| k.mimes.iter().any(|x| m.iter().any(|y| y == x))).map(|(id, n, _)| Choice { id: id.clone(), name: n.clone() }).collect();
            choices.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            Row { kind: i, current: current(k.mimes[0]), choices }
        })
        .collect()
}

/// Makes `app` (a desktop file name) the answer for every type of the kind.
pub fn set(kind: usize, app: &str) -> Result<(), String> {
    let Some(k) = KINDS.get(kind) else { return Err("no such kind".into()) };
    if !app.ends_with(".desktop") || app.contains('/') || app.contains(' ') {
        return Err("that is not a program".into());
    }
    for m in k.mimes {
        crate::sys::act("xdg-mime", &["default", app, m])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_odd_names() {
        assert!(set(0, "../evil.desktop").is_err());
        assert!(set(0, "a b.desktop").is_err());
        assert!(set(0, "notadesktopfile").is_err());
        assert!(set(99, "x.desktop").is_err());
    }

    #[test]
    fn every_kind_has_types() {
        assert!(KINDS.iter().all(|k| !k.mimes.is_empty()));
    }
}

