//! Snippets: texts kept on purpose, in `~/.local/share/yggdrasil/clipboard/snippets.json` (only this user can read it). Pinned
//! ones come first; the rest by how often they were used.

use serde_json::{json, Value};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Snippet {
    pub title: String,
    pub text: String,
    pub pinned: bool,
    pub used: u64,
}

pub fn path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::data().join("clipboard").join("snippets.json")
}

pub fn parse(text: &str) -> Vec<Snippet> {
    let Ok(v) = serde_json::from_str::<Value>(text) else { return vec![] };
    v["snippets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|s| {
                    Some(Snippet {
                        title: s["title"].as_str().unwrap_or("").to_string(),
                        text: s["text"].as_str()?.to_string(),
                        pinned: s["pinned"].as_bool().unwrap_or(false),
                        used: s["used"].as_u64().unwrap_or(0),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn to_json(list: &[Snippet]) -> String {
    let a: Vec<Value> = list.iter().map(|s| json!({"title": s.title, "text": s.text, "pinned": s.pinned, "used": s.used})).collect();
    serde_json::to_string_pretty(&json!({"snippets": a})).unwrap_or_default()
}

pub fn load() -> Vec<Snippet> {
    std::fs::read_to_string(path()).map(|t| parse(&t)).unwrap_or_default()
}

/// Writes beside the file and renames, owner-only.
pub fn save(list: &[Snippet]) -> std::io::Result<()> {
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    let p = path();
    let dir = p.parent().unwrap();
    std::fs::create_dir_all(dir)?;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    let tmp = p.with_extension("json.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    std::io::Write::write_all(&mut f, to_json(list).as_bytes())?;
    f.sync_all()?;
    std::fs::rename(&tmp, &p)
}

/// The order they are shown in: pinned first, then the most used.
pub fn sort(list: &mut [Snippet]) {
    list.sort_by(|a, b| b.pinned.cmp(&a.pinned).then(b.used.cmp(&a.used)));
}

/// A title for a snippet made from a copied text: its first line, cut short.
pub fn title_from(text: &str) -> String {
    let first = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    if first.chars().count() > 40 {
        format!("{}…", first.chars().take(39).collect::<String>())
    } else {
        first.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_round_trip_and_sort() {
        let mut l = vec![
            Snippet { title: "a".into(), text: "A".into(), pinned: false, used: 5 },
            Snippet { title: "b".into(), text: "B\nline".into(), pinned: true, used: 0 },
            Snippet { title: "c".into(), text: "C".into(), pinned: false, used: 9 },
        ];
        assert_eq!(parse(&to_json(&l)), l);
        sort(&mut l);
        assert_eq!(l.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(), ["b", "c", "a"]);
        assert!(parse("not json").is_empty());
        assert_eq!(parse(r#"{"snippets":[{"title":"no text"},{"text":"x"}]}"#).len(), 1);
        assert_eq!(title_from("\n  hello world\nmore"), "hello world");
        assert_eq!(title_from(&"x".repeat(50)).chars().count(), 40);
    }
}
