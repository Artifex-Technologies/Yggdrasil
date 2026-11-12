//! Word packs: lists of words the language dictionaries do not have (software and tech terms, companies and products, gaming,
//! people's names), each a file with a short header, switched on and off in Settings > Keyboard > Spelling and languages.
//!
//! /usr/share/yggdrasil/spell/packs/<id>.txt (from the yggdrasil-spell-packs package) or the user's own in
//! ~/.local/share/yggdrasil/spell/packs/:
//!
//! ```text
//! # name: Software and tech
//! # description: Programming, networking, cloud and security terms
//! # licence: MIT
//! # source: cspell-dicts 0964f52 (dictionaries/software-terms, ...)
//! # default: yes
//! word
//! ```
//!
//! Pack words are not copied into the user's own word list: they are written to the apps' lists while the pack is on, and taken out
//! again when it is switched off (unless the user has the word too).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::words::valid;

pub fn dirs() -> Vec<PathBuf> {
    vec![PathBuf::from("/usr/share/yggdrasil/spell/packs"), crate::words::dir().join("packs")]
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pack {
    pub id: String,
    pub name: String,
    pub description: String,
    pub licence: String,
    pub source: String,
    pub default_on: bool,
    pub path: PathBuf,
    pub count: usize,
}

impl Pack {
    pub fn words(&self) -> Vec<String> {
        read_words(&self.path)
    }
}

fn read_words(p: &Path) -> Vec<String> {
    std::fs::read_to_string(p).unwrap_or_default().lines().filter(|l| !l.starts_with('#') && valid(l)).map(str::to_string).collect()
}

/// The packs that are installed, by name; the first folder wins for an id.
pub fn installed() -> Vec<Pack> {
    installed_in(&dirs())
}

pub fn installed_in(dirs: &[PathBuf]) -> Vec<Pack> {
    let mut out: Vec<Pack> = vec![];
    for d in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().is_none_or(|x| x != "txt") {
                continue;
            }
            let id = p.file_stem().unwrap_or_default().to_string_lossy().to_string();
            if out.iter().any(|x| x.id == id) || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
                continue;
            }
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let head = |k: &str| {
                text.lines().take_while(|l| l.starts_with('#')).find_map(|l| {
                    l.strip_prefix('#')
                        .and_then(|l| l.trim().strip_prefix(k))
                        .and_then(|r| r.strip_prefix(':'))
                        .map(|v| v.trim().to_string())
                })
            };
            let count = text.lines().filter(|l| !l.starts_with('#') && !l.is_empty()).count();
            out.push(Pack {
                name: head("name").unwrap_or_else(|| id.clone()),
                description: head("description").unwrap_or_default(),
                licence: head("licence").unwrap_or_default(),
                source: head("source").unwrap_or_default(),
                default_on: head("default").is_some_and(|v| v == "yes"),
                id,
                path: p,
                count,
            });
        }
    }
    // the ones on by default first, the biggest of them first (Software and tech before Gaming)
    out.sort_by(|a, b| b.default_on.cmp(&a.default_on).then(b.count.cmp(&a.count)).then(a.name.cmp(&b.name)));
    out
}

/// The words of the packs that are on, and of every installed pack (to take out of the apps' lists what a switched-off pack put there).
#[derive(Default, Debug, Clone)]
pub struct PackWords {
    pub on: BTreeSet<String>,
    pub all: BTreeSet<String>,
}

pub fn words_for(on_ids: &[String]) -> PackWords {
    let mut w = PackWords::default();
    for p in installed() {
        // the packs come from case-insensitive lists: "Speedrun" means "speedrun" too. Hunspell (and the apps) accept any
        // capitalisation of a lowercase entry, so the lowercase form goes along; "GitHub" stays as it is written as well.
        let words: Vec<String> = p
            .words()
            .into_iter()
            .flat_map(|x| {
                let l = x.to_lowercase();
                if l == x {
                    vec![x]
                } else {
                    vec![x, l]
                }
            })
            .collect();
        if on_ids.contains(&p.id) {
            w.on.extend(words.iter().cloned());
        }
        w.all.extend(words);
    }
    w
}

/// Turns cspell-style word files into a pack's word list: comments and empty lines dropped, `!word` (a word that is wrong) and
/// lines with a suggestion (`:` or `->`) dropped, the compound and case markers (`*`, `+`, `~`) taken off, phrases split into
/// their words, camelCase code identifiers dropped, and only what `valid` accepts kept, sorted and without repeats.
pub fn import(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for line in text.lines() {
        let l = line.split('#').next().unwrap_or("").trim();
        if l.is_empty() || l.starts_with('!') || l.contains("->") || l.contains(':') {
            continue;
        }
        for tok in l.split_whitespace() {
            let w = tok.trim_matches(|c| matches!(c, '*' | '+' | '~'));
            // code, not prose: camelCase identifiers (podAntiAffinity, hBrush); names like GitHub start with a capital
            let camel = w.chars().next().is_some_and(char::is_lowercase) && w.chars().any(char::is_uppercase);
            if !camel
                && valid(w)
                && w.chars().all(|c| c.is_alphanumeric() || "'-._".contains(c))
                && w.chars().next().is_some_and(char::is_alphanumeric)
            {
                out.insert(w.trim_end_matches('.').to_string());
            }
        }
    }
    out
}

/// Known misspellings, for keeping them out of a pack: every `"wrong->right"` string in a file (cspell's en-common-misspellings,
/// a JSON file of such strings), the wrong side, in lowercase. A plain scan of quoted strings: nothing else in the file matters.
pub fn misspellings(text: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut parts = text.split('"');
    parts.next();
    // every second piece is inside quotes (the files have no escaped quotes in these strings)
    while let (Some(inside), _) = (parts.next(), parts.next()) {
        if let Some((wrong, _)) = inside.split_once("->") {
            out.insert(wrong.trim().to_lowercase());
        }
    }
    out
}

/// Takes known misspellings out of a pack's words: only words in lowercase, because the same letters capitalised are usually a
/// name (Synopsys, Automattic) that a misspellings list matches by accident.
pub fn without_misspellings(words: BTreeSet<String>, bad: &BTreeSet<String>) -> (BTreeSet<String>, Vec<String>) {
    let (drop, keep): (Vec<String>, Vec<String>) = words.into_iter().partition(|w| w.chars().all(|c| !c.is_uppercase()) && bad.contains(w));
    (keep.into_iter().collect(), drop)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misspellings_are_kept_out_but_names_are_not() {
        let bad = misspellings(
            r#"{"dictionaryDefinitions":[{"name":"x","suggestWords":["splitted->split","synopsys->synopsis","sended->sent"]}]}"#,
        );
        assert_eq!(bad.len(), 3);
        let words: BTreeSet<String> = ["splitted", "Synopsys", "Kubernetes", "sended"].iter().map(|s| s.to_string()).collect();
        let (keep, drop) = without_misspellings(words, &bad);
        assert_eq!(keep.into_iter().collect::<Vec<_>>(), ["Kubernetes", "Synopsys"]);
        assert_eq!(drop, ["sended", "splitted"]);
    }

    #[test]
    fn import_cleans_cspell_files() {
        let got = import("# comment\n*Auth*\n+suffix\n~Kubernetes\n!wrongword\nfoo->bar\nkey: value\nGitHub Actions\nx86_64\n(parens)\n.hidden\nNode.js\npodAntiAffinity\nhBrush\n\n");
        let want: Vec<&str> = vec!["Actions", "Auth", "GitHub", "Kubernetes", "Node.js", "suffix", "x86_64"];
        assert_eq!(got.iter().map(String::as_str).collect::<Vec<_>>(), want);
    }

    #[test]
    fn reads_headers_and_counts() {
        let d = std::env::temp_dir().join(format!("ygg-spell-packs-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("tech.txt"), "# name: Software and tech\n# licence: MIT\n# default: yes\nKubernetes\nGitHub\n").unwrap();
        std::fs::write(d.join("names.txt"), "# name: People's names\nAda\n").unwrap();
        std::fs::write(d.join("bad id!.txt"), "x\n").unwrap();
        let p = installed_in(std::slice::from_ref(&d));
        assert_eq!(p.iter().map(|p| (p.id.as_str(), p.default_on, p.count)).collect::<Vec<_>>(), [("tech", true, 2), ("names", false, 1)]);
        assert_eq!(p[0].name, "Software and tech");
        assert_eq!(p[0].words(), ["Kubernetes", "GitHub"]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
