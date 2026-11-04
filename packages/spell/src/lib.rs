//! Yggdrasil's spelling library: the Hunspell dictionaries installed on the system (one package per language, added and removed in
//! Settings > Keyboard > Spelling and languages), the languages the user has switched on, and one word list that every app adds to.
//!
//! A word is right when any switched-on language knows it, or it is in the user's word list, or in the computer's shared list.
//! Apps keep their own "add to dictionary" lists (GTK apps through enchant, Firefox, Chromium and every Electron app); `sync` merges
//! all of them into the user's list and writes the result back to each, so a word added in one app is known in all of them.
//!
//! Nothing here goes to the network, and nothing is written outside the user's own folders. The engine is spellbook (pure Rust,
//! Hunspell-compatible, MPL-2.0), so no C or C++ spell library is involved.

pub mod apps;
pub mod conf;
pub mod dicts;
pub mod words;

use std::collections::HashSet;

/// The switched-on languages and the word lists, ready to check words.
pub struct Speller {
    dicts: Vec<(String, spellbook::Dictionary)>,
    words: HashSet<String>,
    /// Languages that are switched on but could not be loaded, with the reason.
    pub missing: Vec<(String, String)>,
}

impl Speller {
    /// The user's languages (spell.conf) and word lists.
    pub fn load() -> Speller {
        let conf = conf::SpellConf::load();
        let mut words: HashSet<String> = words::WordList::load().words().map(str::to_string).collect();
        words.extend(words::global());
        Speller::with(&conf.active(), words)
    }

    /// The given languages (dictionary codes such as `en_US`) and words.
    pub fn with(languages: &[String], words: HashSet<String>) -> Speller {
        let mut dicts = vec![];
        let mut missing = vec![];
        for code in languages {
            match dicts::load(code) {
                Ok(d) => dicts.push((code.clone(), d)),
                Err(e) => missing.push((code.clone(), e)),
            }
        }
        Speller { dicts, words, missing }
    }

    pub fn languages(&self) -> Vec<&str> {
        self.dicts.iter().map(|(c, _)| c.as_str()).collect()
    }

    /// Right in any switched-on language, or in a word list. A word with no letters (a number, a symbol) is always right.
    pub fn check(&self, word: &str) -> bool {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '\'');
        if w.is_empty() || !w.chars().any(char::is_alphabetic) {
            return true;
        }
        // with no language loaded every word would be marked: mark none
        if self.dicts.is_empty() || self.words.contains(w) || self.words.contains(&w.to_lowercase()) {
            return true;
        }
        self.dicts.iter().any(|(_, d)| d.check(w))
    }

    /// Suggestions from every switched-on language, best first per language, without repeats.
    pub fn suggest(&self, word: &str) -> Vec<String> {
        let mut out: Vec<String> = vec![];
        for (_, d) in &self.dicts {
            let mut s = vec![];
            d.suggest(word, &mut s);
            for w in s {
                if !out.contains(&w) {
                    out.push(w);
                }
            }
        }
        out.truncate(10);
        out
    }

    /// The words of a text that no switched-on language and no word list knows, with their byte positions.
    pub fn misspelled<'a>(&self, text: &'a str) -> Vec<(usize, &'a str)> {
        let mut out = vec![];
        let mut start = None;
        for (i, c) in text.char_indices().chain(std::iter::once((text.len(), ' '))) {
            let part = c.is_alphanumeric() || c == '\'' || c == '-' && start.is_some();
            match (part, start) {
                (true, None) => start = Some(i),
                (false, Some(s)) => {
                    let w = text[s..i].trim_end_matches(['\'', '-']);
                    if !w.is_empty() && !self.check(w) {
                        out.push((s, w));
                    }
                    start = None;
                }
                _ => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn english() -> Option<Speller> {
        dicts::find("en_US").map(|_| Speller::with(&["en_US".to_string()], ["Yggdrasil".to_string()].into_iter().collect()))
    }

    #[test]
    fn checks_suggests_and_knows_the_word_list() {
        let Some(s) = english() else { return }; // no English dictionary on this machine
        assert!(s.check("keyboard") && s.check("Keyboard") && s.check("don't"));
        assert!(!s.check("keybaord"));
        assert!(s.suggest("keybaord").iter().any(|w| w == "keyboard"));
        assert!(s.check("Yggdrasil"), "a word from the word list");
        assert!(s.check("1234") && s.check("--") && s.check(""));
        let bad = s.misspelled("The keybaord is on Yggdrasil, teh desk.");
        assert_eq!(bad.iter().map(|(_, w)| *w).collect::<Vec<_>>(), ["keybaord", "teh"]);
        assert_eq!(bad[0].0, 4);
    }

    #[test]
    fn a_language_that_is_not_installed_is_reported_not_fatal() {
        let s = Speller::with(&["xx_NOPE".to_string()], HashSet::new());
        assert!(s.languages().is_empty());
        assert_eq!(s.missing.len(), 1);
        assert!(s.check("anything"), "with no dictionary loaded only the word list can say no, so nothing is marked");
    }
}
