//! Correcting as you type (the input method `yggdrasil-autocorrect`, off unless spell.conf says `autocorrect = yes`): when a word
//! has just been finished (a space or a punctuation mark typed after it), it is replaced only when the fix is all but certain.
//!
//! The rule is careful on purpose, because a wrong "correction" is worse than a typo left alone:
//! - the word is wrong in every switched-on language and in no word list (yours, the computer's, the packs);
//! - it is 3 or more letters, all lowercase or only its first letter capital (no names in CAPS, no camelCase, no digits);
//! - the dictionary's first suggestion is one edit away (one letter changed, added, dropped, or two neighbours swapped) and keeps
//!   the first letter (unless the first two were swapped); and either it is two neighbouring letters swapped (the usual typing
//!   slip: teh, adn, thier, wrold) or no other suggestion is also one edit away (no guessing between two: "helo" is left alone,
//!   since "hole" comes first and "hello" is only one of several);
//! - the person did not just undo the same correction (`Undo`: a word put back is never corrected again this session).
//!
//! The input method sees the text around the cursor (text-input-v3) and nothing is kept or sent anywhere.

use std::collections::HashSet;

/// A replacement: take `delete` bytes before the cursor and type `insert`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fix {
    pub delete: usize,
    pub insert: String,
    /// The word as it was typed and as it was corrected (for undo).
    pub typed: String,
    pub fixed: String,
}

/// Characters that end a word when typed after it.
pub fn ends_word(c: char) -> bool {
    c == ' ' || ".,!?;:)\"".contains(c)
}

/// The edit distance with neighbour swaps (Damerau-Levenshtein, restricted), on characters.
pub fn distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for j in 0..=b.len() {
        d[0][j] = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

/// Is the word shaped like one autocorrect may touch: letters (an apostrophe inside allowed), 3 or more, all lowercase or only the
/// first letter capital?
fn plain(w: &str) -> bool {
    let chars: Vec<char> = w.chars().collect();
    chars.len() >= 3
        && chars.iter().all(|c| c.is_alphabetic() || *c == '\'')
        && chars.iter().skip(1).all(|c| !c.is_uppercase())
        && chars.first().is_some_and(|c| c.is_alphabetic())
}

/// Are `a` and `b` the same but for two neighbouring characters swapped?
pub fn swapped(a: &str, b: &str) -> bool {
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    if a.len() != b.len() {
        return false;
    }
    let diff: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    matches!(diff.as_slice(), [i, j] if *j == i + 1 && a[*i] == b[*j] && a[*j] == b[*i])
}

/// The suggestion to take, if it is certain enough (see the module's rule). `suggestions` best first, as the dictionary gives them.
pub fn pick(typed: &str, suggestions: &[String]) -> Option<String> {
    if !plain(typed) {
        return None;
    }
    let low = typed.to_lowercase();
    let best = suggestions.first().filter(|s| !s.contains([' ', '-']))?;
    let b = best.to_lowercase();
    if distance(&low, &b) != 1 {
        return None;
    }
    let others_near = suggestions[1..].iter().filter(|s| !s.contains(' ') && distance(&low, &s.to_lowercase()) == 1).count();
    if !swapped(&low, &b) && others_near > 0 {
        return None;
    }
    let (t0, t1) = (low.chars().next(), low.chars().nth(1));
    let (b0, b1) = (b.chars().next(), b.chars().nth(1));
    let swapped_start = t0 == b1 && t1 == b0;
    if t0 != b0 && !swapped_start {
        return None;
    }
    // keep the person's capital first letter; a suggestion that is a name (capital) stays as it is
    let first_upper = typed.chars().next().is_some_and(char::is_uppercase);
    Some(if first_upper && !best.chars().next().is_some_and(char::is_uppercase) {
        let mut c = best.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    } else {
        best.to_string()
    })
}

/// What to do with the text before the cursor, which ends with the character just typed. `check` says whether a word is right,
/// `suggest` gives suggestions best first; `kept` holds words the person put back after a correction.
pub fn fix(before: &str, check: impl Fn(&str) -> bool, suggest: impl Fn(&str) -> Vec<String>, kept: &HashSet<String>) -> Option<Fix> {
    let end = before.chars().last()?;
    if !ends_word(end) {
        return None;
    }
    let body = &before[..before.len() - end.len_utf8()];
    let start = body.char_indices().rev().take_while(|(_, c)| c.is_alphabetic() || *c == '\'').last().map(|(i, _)| i)?;
    let word = &body[start..];
    // the word must stand on its own: not glued to digits, a path, an address or a hyphenated compound
    if body[..start].chars().last().is_some_and(|c| !c.is_whitespace() && !"(\"'".contains(c)) {
        return None;
    }
    if kept.contains(word) || check(word) {
        return None;
    }
    let fixed = pick(word, &suggest(word))?;
    Some(Fix { delete: word.len() + end.len_utf8(), insert: format!("{fixed}{end}"), typed: word.to_string(), fixed })
}

/// After a correction: has the person undone it? True when the text before the cursor is exactly what it was with the correction,
/// minus the character that ended the word (they pressed Backspace straight away).
pub fn undone(before: &str, last: &Fix, before_fix: &str) -> bool {
    let corrected = format!("{}{}", &before_fix[..before_fix.len() - last.delete], last.fixed);
    before == corrected
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> HashSet<&'static str> {
        ["the", "receive", "their", "there", "form", "from", "hello", "world", "Sarina", "cat", "cart", "card"].into_iter().collect()
    }

    fn check(w: &str) -> bool {
        words().contains(w) || words().contains(w.to_lowercase().as_str())
    }

    fn suggest(w: &str) -> Vec<String> {
        let mut s: Vec<String> =
            words().into_iter().filter(|c| distance(&w.to_lowercase(), &c.to_lowercase()) <= 2).map(String::from).collect();
        s.sort_by_key(|c| distance(&w.to_lowercase(), &c.to_lowercase()));
        s
    }

    #[test]
    fn distances() {
        assert_eq!(distance("teh", "the"), 1, "a swap is one edit");
        assert_eq!(distance("recieve", "receive"), 1);
        assert_eq!(distance("helo", "hello"), 1);
        assert_eq!(distance("cat", "dog"), 3);
    }

    #[test]
    fn a_certain_typo_is_fixed_when_the_word_ends() {
        let none = HashSet::new();
        assert_eq!(
            fix("I said teh ", check, suggest, &none).unwrap(),
            Fix { delete: 4, insert: "the ".into(), typed: "teh".into(), fixed: "the".into() }
        );
        assert_eq!(fix("helo,", check, suggest, &none).unwrap().insert, "hello,");
        assert_eq!(fix("Recieve ", check, suggest, &none).unwrap().insert, "Receive ", "the capital is kept");
        assert!(fix("I said teh", check, suggest, &none).is_none(), "nothing until the word is finished");
        assert!(fix("the ", check, suggest, &none).is_none(), "a right word is left alone");
    }

    #[test]
    fn anything_uncertain_is_left_alone() {
        let none = HashSet::new();
        assert!(
            fix("car ", |w| w == "x", |_| vec!["cart".into(), "cat".into(), "card".into()], &none).is_none(),
            "several one edit away, none a swap: no guessing"
        );
        assert!(fix("TEH ", check, suggest, &none).is_none(), "CAPS are left alone");
        assert!(fix("tEh ", check, suggest, &none).is_none(), "odd capitals are left alone");
        assert!(fix("xyzzy ", check, suggest, &none).is_none(), "no near suggestion");
        assert!(fix("ab3teh ", check, suggest, &none).is_none(), "glued to digits");
        assert!(fix("/usr/teh ", check, suggest, &none).is_none(), "part of a path");
        assert!(fix("re-teh ", check, suggest, &none).is_none(), "part of a compound");
        assert!(fix("eh ", check, suggest, &none).is_none(), "too short");
        assert!(fix("wrold ", |w| w == "x", |_| vec!["world".into()], &none).is_some(), "a swap inside is fine");
        assert!(fix("ehllo ", |w| w == "x", |_| vec!["hello".into()], &none).is_some(), "the first two swapped is fine");
        assert!(fix("jello ", |w| w == "x", |_| vec!["hello".into()], &none).is_none(), "another first letter is not");
    }

    /// Suggestions as Hunspell's en_US gave them (yggdrasil-spell check, 2026-11-05).
    #[test]
    fn real_dictionary_suggestions() {
        let none = HashSet::new();
        let real = |list: &'static [&'static str]| move |_: &str| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let wrong = |_: &str| false;
        let cases: [(&str, &[&str], Option<&str>); 9] = [
            ("teh", &["the", "eh", "th", "teth", "tech", "tee", "tea", "ten", "tel", "ted"], Some("the")),
            ("recieve", &["receive", "relieve", "reverie"], Some("receive")),
            ("wierd", &["weird", "wired", "wider", "wield"], Some("weird")),
            ("helo", &["hole", "helot", "hello", "halo", "hero", "hell", "held", "hemo", "helm", "help"], None),
            ("adn", &["and", "an", "ad", "ads", "ain", "ado", "add", "adv", "awn", "adz"], Some("and")),
            ("becuase", &["because", "barbecue"], Some("because")),
            ("definately", &["definitely", "effeminately", "definably", "determinately", "definitively"], Some("definitely")),
            ("thier", &["their", "tier", "shier", "trier", "thief", "th ier", "th-ier", "thither"], Some("their")),
            ("wrold", &["world", "wold", "World"], Some("world")),
        ];
        for (typed, list, want) in cases {
            let got = fix(&format!("{typed} "), wrong, real(list), &none).map(|f| f.fixed);
            assert_eq!(got.as_deref(), want, "{typed}");
        }
    }

    #[test]
    fn a_word_put_back_stays() {
        let mut kept = HashSet::new();
        let before = "teh ";
        let f = fix(before, check, suggest, &kept).unwrap();
        // the person pressed Backspace right after: "the" without the space
        assert!(undone("the", &f, before));
        assert!(!undone("the q", &f, before));
        kept.insert(f.typed.clone());
        assert!(fix("teh ", check, suggest, &kept).is_none());
    }
}
