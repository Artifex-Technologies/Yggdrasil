//! The computer's shared word list (/var/lib/yggdrasil/spell/global.txt): words every account on this computer gets.
//!
//! How a word gets there (the owner's rule, 2026-11-03): it is offered by **two accounts**, or **an administrator approves it**.
//!
//! - Each account that has "offer my words" on (spell.conf `share = yes`, the default) writes the words it added itself to
//!   `offered/<uid>.txt`. The folder is like /tmp (mode 1733): anyone may create a file in it, nobody but root can list it or read
//!   another's file, and a file only counts for the account that owns it (the name is checked against the owner).
//! - Only words that are safe to share are offered (`shareable`): letters with an apostrophe or a hyphen inside, 2 to 40 of them.
//!   Never anything with a digit, an @, a slash or a dot (codes, keys, addresses, links), and never something that looks typed by
//!   accident into a password box.
//! - `yggdrasil-spell global merge` (root; the system unit yggdrasil-spell-global.path runs it when a file in `offered` changes)
//!   counts the accounts per word and writes `global.txt` (readable by all) and `pending.txt` (words one account offered, without
//!   saying which: readable by root and the `wheel` group, the administrators, who approve them in Settings).
//! - `approved.txt` and `denied.txt` hold the administrators' choices (`yggdrasil-spell global approve|deny WORD`, through pkexec).
//!   A denied word never reaches the shared list, however many accounts offer it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

pub const DIR: &str = "/var/lib/yggdrasil/spell";
/// The most bytes of one account's offer that are read (a word list, not a dump).
const MAX_OFFER: u64 = 512 * 1024;

pub fn offered_dir(root: &Path) -> PathBuf {
    root.join("offered")
}

/// A word that may be offered to the other accounts: stricter than `words::valid`, since other people will see it.
pub fn shareable(w: &str) -> bool {
    let n = w.chars().count();
    if !(2..=40).contains(&n) || !crate::words::valid(w) {
        return false;
    }
    let chars: Vec<char> = w.chars().collect();
    // letters only, with ' or - (or a typographic apostrophe) between letters
    let ok = chars.iter().enumerate().all(|(i, c)| {
        c.is_alphabetic()
            || ("'-\u{2019}".contains(*c) && i > 0 && i + 1 < chars.len() && chars[i - 1].is_alphabetic() && chars[i + 1].is_alphabetic())
    });
    if !ok {
        return false;
    }
    // a long run with no vowel at all looks like a password or a key typed into the wrong box, not a word
    let vowels = chars.iter().filter(|c| "aeiouyAEIOUYàáâäæãåāèéêëēėęîïíīįìôöòóœøōõûüùúūÿ".contains(**c)).count();
    !(n >= 7 && vowels == 0)
}

/// The words this account offers: its own list's shareable words.
pub fn offer_text(list: &crate::words::WordList) -> String {
    let mut s = String::from("# Words this account offers to the computer's shared list (Settings > Keyboard > Spelling and languages)\n");
    for w in list.words().filter(|w| shareable(w)) {
        s.push_str(w);
        s.push('\n');
    }
    s
}

/// Writes this account's offer (or takes it back when `share` is off). Quietly does nothing when the shared folder is missing
/// (yggdrasil-spell is installed, but not its system part yet).
pub fn offer(root: &Path, uid: u32, list: &crate::words::WordList, share: bool) -> std::io::Result<()> {
    let dir = offered_dir(root);
    if !dir.is_dir() {
        return Ok(());
    }
    let path = dir.join(format!("{uid}.txt"));
    if !share {
        return match std::fs::remove_file(&path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        };
    }
    let text = offer_text(list);
    if std::fs::read_to_string(&path).ok().as_deref() == Some(text.as_str()) {
        return Ok(()); // unchanged: no write, so the merge is not woken for nothing
    }
    // written in place (a temporary file in a sticky folder would be one more name to clean up), readable by root alone
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
    f.write_all(text.as_bytes())
}

/// Words from a list file: one per line, # comments, only shareable ones.
fn read_words(path: &Path) -> BTreeSet<String> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|l| !l.starts_with('#') && shareable(l))
        .map(str::to_string)
        .collect()
}

/// What a merge decides.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Merged {
    pub global: BTreeSet<String>,
    /// Offered by one account only, waiting for an administrator.
    pub pending: BTreeSet<String>,
}

/// The rule itself: two accounts, or approved; never denied. Words are compared without regard to case for counting, and kept as
/// the most common spelling.
pub fn decide(offers: &[BTreeSet<String>], approved: &BTreeSet<String>, denied: &BTreeSet<String>) -> Merged {
    let denied_low: BTreeSet<String> = denied.iter().map(|w| w.to_lowercase()).collect();
    // lowercase -> (accounts, spelling -> count)
    let mut seen: BTreeMap<String, (usize, BTreeMap<String, usize>)> = BTreeMap::new();
    for words in offers {
        let mut mine: BTreeSet<String> = BTreeSet::new();
        for w in words {
            let low = w.to_lowercase();
            let e = seen.entry(low.clone()).or_default();
            *e.1.entry(w.clone()).or_default() += 1;
            if mine.insert(low) {
                e.0 += 1;
            }
        }
    }
    let mut m = Merged::default();
    for (low, (accounts, spellings)) in seen {
        if denied_low.contains(&low) {
            continue;
        }
        let best = spellings.into_iter().max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(w, _)| w).unwrap_or(low.clone());
        if accounts >= 2 {
            m.global.insert(best);
        } else {
            m.pending.insert(best);
        }
    }
    for w in approved {
        if !denied_low.contains(&w.to_lowercase()) {
            m.pending.retain(|p| p.to_lowercase() != w.to_lowercase());
            m.global.insert(w.clone());
        }
    }
    m
}

/// Every account's offer, each counted only when the file's owner is the account its name says.
pub fn read_offers(root: &Path) -> Vec<BTreeSet<String>> {
    use std::os::unix::fs::MetadataExt;
    let mut out = vec![];
    let Ok(rd) = std::fs::read_dir(offered_dir(root)) else { return out };
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        let Some(uid) = name.strip_suffix(".txt").and_then(|u| u.parse::<u32>().ok()) else { continue };
        // symlink_metadata: a link planted in the folder is not followed
        let Ok(md) = std::fs::symlink_metadata(e.path()) else { continue };
        if !md.is_file() || md.uid() != uid || md.len() > MAX_OFFER {
            continue;
        }
        out.push(read_words(&e.path()));
    }
    out
}

/// The merge (root): reads the offers and the administrators' choices, writes global.txt and pending.txt.
pub fn merge(root: &Path) -> std::io::Result<Merged> {
    let m = decide(&read_offers(root), &read_words(&root.join("approved.txt")), &read_words(&root.join("denied.txt")));
    let mut g = String::from("# Words every account on this computer gets (offered by two accounts, or approved by an administrator)\n");
    g.extend(m.global.iter().map(|w| format!("{w}\n")));
    write_mode(&root.join("global.txt"), &g, 0o644, None)?;
    let mut p = String::from("# Words one account offered, waiting for an administrator (Settings > Keyboard > Spelling and languages)\n");
    p.extend(m.pending.iter().map(|w| format!("{w}\n")));
    write_mode(&root.join("pending.txt"), &p, 0o640, wheel_gid())?;
    Ok(m)
}

/// An administrator's choice (root): approve (it goes on the shared list now) or deny (it never does). Undoes the opposite.
pub fn choose(root: &Path, approve: bool, words: &[String]) -> std::io::Result<()> {
    let (yes, no) = if approve { ("approved.txt", "denied.txt") } else { ("denied.txt", "approved.txt") };
    let mut add = read_words(&root.join(yes));
    let mut take = read_words(&root.join(no));
    for w in words.iter().filter(|w| shareable(w)) {
        add.insert(w.clone());
        take.retain(|t| t.to_lowercase() != w.to_lowercase());
    }
    let render = |s: &BTreeSet<String>| s.iter().map(|w| format!("{w}\n")).collect::<String>();
    write_mode(&root.join(yes), &render(&add), 0o600, None)?;
    write_mode(&root.join(no), &render(&take), 0o600, None)?;
    merge(root).map(|_| ())
}

fn wheel_gid() -> Option<u32> {
    std::fs::read_to_string("/etc/group").ok()?.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.first() == Some(&"wheel")).then(|| f.get(2)?.parse().ok()).flatten()
    })
}

fn write_mode(path: &Path, text: &str, mode: u32, gid: Option<u32>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let tmp = path.with_file_name(format!(".{}.tmp", path.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::write(&tmp, text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode))?;
    if let Some(g) = gid {
        std::os::unix::fs::chown(&tmp, None, Some(g))?;
    }
    std::fs::rename(tmp, path)
}

/// The words waiting for an administrator (pending.txt; readable by the administrators only). None when it cannot be read.
pub fn pending(root: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(root.join("pending.txt")).ok()?;
    Some(text.lines().filter(|l| !l.starts_with('#') && shareable(l)).map(str::to_string).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ws: &[&str]) -> BTreeSet<String> {
        ws.iter().map(|w| w.to_string()).collect()
    }

    #[test]
    fn what_may_be_shared() {
        for w in ["Yggdrasil", "don't", "Heimdall's", "e-mail", "café", "Sarina"] {
            assert!(shareable(w), "{w}");
        }
        for w in [
            "x86_64",
            "hunter2",
            "a",
            "ada@example.com",
            "https://x.org",
            "-dash",
            "end-",
            "www.example",
            "a.b",
            "Pa55word",
            "qzxkwrtpl",
            "two words",
        ] {
            assert!(!shareable(w), "{w}");
        }
    }

    #[test]
    fn two_accounts_or_an_administrator() {
        let offers = vec![set(&["Yggdrasil", "Heimdall", "Mothra"]), set(&["yggdrasil", "Limbo"]), set(&["Yggdrasil"])];
        let m = decide(&offers, &set(&["Limbo"]), &set(&[]));
        assert_eq!(m.global, set(&["Limbo", "Yggdrasil"]), "Yggdrasil by three accounts (its usual spelling); Limbo approved");
        assert_eq!(m.pending, set(&["Heimdall", "Mothra"]));
        let denied = decide(&offers, &set(&[]), &set(&["YGGDRASIL", "Mothra"]));
        assert!(denied.global.is_empty(), "a denied word never goes, whatever the case");
        assert_eq!(denied.pending, set(&["Heimdall", "Limbo"]));
    }

    #[test]
    fn one_account_twice_is_one_account() {
        let m = decide(&[set(&["Word", "word"])], &set(&[]), &set(&[]));
        assert!(m.global.is_empty());
    }

    #[test]
    fn offers_count_only_for_their_owner() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ygg-spell-global-{}", std::process::id()));
        let off = offered_dir(&root);
        std::fs::create_dir_all(&off).unwrap();
        let me = unsafe { libc::getuid() };
        std::fs::write(off.join(format!("{me}.txt")), "Yggdrasil\nhunter2\n").unwrap();
        // a file named for another account (made by this one) does not count as that account's
        std::fs::write(off.join(format!("{}.txt", me + 1)), "Yggdrasil\n").unwrap();
        std::os::unix::fs::symlink(off.join(format!("{me}.txt")), off.join(format!("{}.txt", me + 2))).unwrap();
        let offers = read_offers(&root);
        assert_eq!(offers, vec![set(&["Yggdrasil"])], "only the owner's file, only shareable words, no links");
        let m = merge(&root).unwrap();
        assert!(m.global.is_empty(), "one real account: not shared yet");
        assert_eq!(pending(&root).unwrap(), vec!["Yggdrasil".to_string()]);
        let mode = std::fs::metadata(root.join("pending.txt")).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o640, "the administrators read it, nobody else");
        choose(&root, true, &["Yggdrasil".to_string()]).unwrap();
        assert_eq!(crate::words::read_global(&root.join("global.txt")), vec!["Yggdrasil".to_string()]);
        choose(&root, false, &["Yggdrasil".to_string()]).unwrap();
        assert!(crate::words::read_global(&root.join("global.txt")).is_empty(), "denied after all: taken off");
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn an_offer_follows_the_switch() {
        let root = std::env::temp_dir().join(format!("ygg-spell-offer-{}", std::process::id()));
        let mut list = crate::words::WordList::default();
        list.add("Yggdrasil", "");
        list.add("x86_64", "");
        // no shared folder: nothing written, no error
        offer(&root, 1000, &list, true).unwrap();
        assert!(!root.exists());
        std::fs::create_dir_all(offered_dir(&root)).unwrap();
        offer(&root, 1000, &list, true).unwrap();
        let text = std::fs::read_to_string(offered_dir(&root).join("1000.txt")).unwrap();
        assert!(text.contains("Yggdrasil\n") && !text.contains("x86_64"));
        offer(&root, 1000, &list, false).unwrap();
        assert!(!offered_dir(&root).join("1000.txt").exists(), "switched off: the offer is taken back");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
