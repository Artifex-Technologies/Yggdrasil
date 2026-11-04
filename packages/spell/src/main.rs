//! yggdrasil-spell: the spelling library from a terminal, and `watch`, the small user service that keeps every app's word list in step.
//!
//!   yggdrasil-spell check WORD...     the words no switched-on language knows, with suggestions (exit 1 if any)
//!   yggdrasil-spell text "TEXT"       the misspelled words of a text
//!   yggdrasil-spell languages         installed dictionaries, the switched-on ones marked
//!   yggdrasil-spell words             your word list and the apps each word came from
//!   yggdrasil-spell add WORD | remove WORD
//!   yggdrasil-spell sync              merge every app's list into yours and back (once)
//!   yggdrasil-spell watch             sync whenever an app's list, your list or the settings change (the user service)

use std::collections::BTreeSet;
use std::path::PathBuf;
use yggdrasil_spell::{apps, conf::SpellConf, dicts, words::WordList, Speller};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = &args[1.min(args.len())..];
    let code = match args.first().map(String::as_str) {
        Some("check") => check(rest),
        Some("text") => text(&rest.join(" ")),
        Some("languages") => languages(),
        Some("words") => list(),
        Some("add") | Some("remove") if !rest.is_empty() => change(args[0] == "add", rest),
        Some("sync") => sync(true),
        Some("watch") => watch(),
        Some("packs") => packs(),
        Some("build-pack") => build_pack(rest),
        _ => {
            eprintln!("usage: yggdrasil-spell check WORD... | text TEXT | languages | packs | words | add WORD | remove WORD | sync | watch");
            eprintln!("       yggdrasil-spell build-pack OUT 'name' 'description' licence 'source' yes|no [--deny MISSPELLINGS] FILE...   (for packaging)");
            2
        }
    };
    std::process::exit(code);
}

fn packs() -> i32 {
    let on = SpellConf::load().active_packs();
    for p in yggdrasil_spell::packs::installed() {
        println!("{} {}\t{} words\t{}\t{}", if on.contains(&p.id) { "*" } else { " " }, p.id, p.count, p.name, p.licence);
    }
    0
}

/// Writes a word pack from cspell-style word files (see packs::import): the header, then the words.
fn build_pack(a: &[String]) -> i32 {
    if a.len() < 7 {
        eprintln!("usage: yggdrasil-spell build-pack OUT 'name' 'description' licence 'source' yes|no [--deny MISSPELLINGS] FILE...");
        return 2;
    }
    let mut files = &a[6..];
    let mut bad = std::collections::BTreeSet::new();
    if files.first().is_some_and(|f| f == "--deny") && files.len() >= 2 {
        match std::fs::read_to_string(&files[1]) {
            Ok(t) => bad = yggdrasil_spell::packs::misspellings(&t),
            Err(e) => {
                eprintln!("yggdrasil-spell: {}: {e}", files[1]);
                return 1;
            }
        }
        files = &files[2..];
    }
    let mut words = std::collections::BTreeSet::new();
    for f in files {
        match std::fs::read_to_string(f) {
            Ok(t) => words.extend(yggdrasil_spell::packs::import(&t)),
            Err(e) => {
                eprintln!("yggdrasil-spell: {f}: {e}");
                return 1;
            }
        }
    }
    let (words, dropped) = yggdrasil_spell::packs::without_misspellings(words, &bad);
    if !dropped.is_empty() {
        println!("left out as known misspellings: {}", dropped.join(" "));
    }
    let mut out = format!("# name: {}\n# description: {}\n# licence: {}\n# source: {}\n# default: {}\n", a[1], a[2], a[3], a[4], a[5]);
    for w in &words {
        out.push_str(w);
        out.push('\n');
    }
    if let Err(e) = std::fs::write(&a[0], out) {
        eprintln!("yggdrasil-spell: {}: {e}", a[0]);
        return 1;
    }
    println!("{}: {} words", a[0], words.len());
    0
}

fn speller() -> Speller {
    let s = Speller::load();
    for (code, why) in &s.missing {
        eprintln!("yggdrasil-spell: {} not loaded: {why}", dicts::name(code));
    }
    s
}

fn check(words: &[String]) -> i32 {
    let s = speller();
    let mut bad = 0;
    for w in words {
        if !s.check(w) {
            bad += 1;
            println!("{w}: {}", s.suggest(w).join(", "));
        }
    }
    i32::from(bad > 0)
}

fn text(t: &str) -> i32 {
    let s = speller();
    let bad = s.misspelled(t);
    for (at, w) in &bad {
        println!("{at}\t{w}\t{}", s.suggest(w).join(", "));
    }
    i32::from(!bad.is_empty())
}

fn languages() -> i32 {
    let active = SpellConf::load().active();
    for i in dicts::installed() {
        println!("{} {}\t{}", if active.contains(&i.code) { "*" } else { " " }, i.code, i.name());
    }
    0
}

fn list() -> i32 {
    for (w, from) in WordList::load().entries() {
        println!("{w}\t{}", from.join(", "));
    }
    0
}

fn change(add: bool, words: &[String]) -> i32 {
    for w in words {
        if add && !yggdrasil_spell::words::valid(w) {
            eprintln!("yggdrasil-spell: \"{w}\" is not kept as a word (spaces, a link, an address or a code)");
        }
    }
    let r = apps::edit_words(|l| {
        for w in words {
            if add { l.add(w, "") } else { l.remove(w) };
        }
    });
    match r {
        Ok((r, _, _)) => {
            for e in &r.errors {
                eprintln!("yggdrasil-spell: {e}");
            }
            i32::from(!r.errors.is_empty())
        }
        Err(e) => {
            eprintln!("yggdrasil-spell: {e}");
            1
        }
    }
}

fn sync(verbose: bool) -> i32 {
    let (r, words, lists) = match apps::sync_all() {
        Ok(x) => x,
        Err(e) => {
            eprintln!("yggdrasil-spell: {e}");
            return 1;
        }
    };
    for (w, app) in &r.added {
        println!("from {app}: {w}");
    }
    if verbose {
        println!("{words} words; {lists} app lists, {} written", r.written.len());
    }
    for e in &r.errors {
        eprintln!("yggdrasil-spell: {e}");
    }
    i32::from(!r.errors.is_empty())
}

/// The folders to watch and the file names in them that matter.
fn targets() -> (Vec<PathBuf>, BTreeSet<String>) {
    let conf = SpellConf::load();
    let mut dirs: BTreeSet<PathBuf> = BTreeSet::new();
    let mut names: BTreeSet<String> = ["words.txt", "removed.txt", "spell.conf"].iter().map(|s| s.to_string()).collect();
    for b in apps::bridges(&conf.active()) {
        if let Some(d) = b.path.parent() {
            let _ = std::fs::create_dir_all(d);
            dirs.insert(d.to_path_buf());
        }
        names.insert(b.path.file_name().unwrap_or_default().to_string_lossy().to_string());
    }
    let data = yggdrasil_spell::words::dir();
    let _ = std::fs::create_dir_all(&data);
    dirs.insert(data);
    dirs.insert(yggdrasil_conf::dirs::config());
    (dirs.into_iter().collect(), names)
}

/// The program file and its inode now: an update replaces the file (a new inode), and a watcher still running the old program
/// would treat the new version's files with old rules (2026-11-03: the first word packs were taken for words added in the apps).
fn own_program() -> Option<(PathBuf, u64)> {
    use std::os::unix::fs::MetadataExt;
    let p = std::env::current_exe().ok()?;
    let p = PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)").to_string());
    Some((p.clone(), std::fs::metadata(&p).ok()?.ino()))
}

fn watch() -> i32 {
    use std::os::unix::fs::MetadataExt;
    let me = own_program();
    // updated: exit with a code that is not 0, so systemd (Restart=on-failure) starts the new program; checked before every sync
    let updated = || me.as_ref().is_some_and(|(path, ino)| std::fs::metadata(path).map(|m| m.ino()).ok() != Some(*ino));
    sync(false);
    loop {
        let (dirs, names) = targets();
        // SAFETY: plain inotify calls on a descriptor this function owns and closes
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
        if fd < 0 {
            eprintln!("yggdrasil-spell: inotify is not available");
            return 1;
        }
        for d in &dirs {
            let c = std::ffi::CString::new(d.as_os_str().as_encoded_bytes()).unwrap_or_default();
            unsafe { libc::inotify_add_watch(fd, c.as_ptr(), libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO) };
        }
        // the program file itself: replacing it changes the old file's link count, which wakes the loop
        let mut program_wd = -1;
        if let Some((path, _)) = &me {
            let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap_or_default();
            program_wd = unsafe { libc::inotify_add_watch(fd, c.as_ptr(), libc::IN_ATTRIB | libc::IN_DELETE_SELF | libc::IN_MOVE_SELF) };
        }
        // wait for a change to one of the lists, or an hour (then look for newly installed apps)
        let mut relevant = false;
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, 3_600_000) } > 0 {
            let mut buf = [0u8; 8192];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            let mut off = 0usize;
            while n > 0 && off + 16 <= n as usize {
                let wd = i32::from_ne_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]]);
                // the program file changed: no sleep, the update check below ends this run
                if wd == program_wd {
                    break;
                }
                let len = u32::from_ne_bytes([buf[off + 12], buf[off + 13], buf[off + 14], buf[off + 15]]) as usize;
                let name_bytes = &buf[off + 16..(off + 16 + len).min(n as usize)];
                let name = String::from_utf8_lossy(name_bytes).trim_end_matches('\0').to_string();
                relevant |= names.contains(&name);
                off += 16 + len;
            }
        }
        unsafe { libc::close(fd) };
        if relevant {
            // an app may write several files in a row; one sync after it is done
            std::thread::sleep(std::time::Duration::from_millis(1500));
        }
        if updated() {
            eprintln!("yggdrasil-spell: updated; starting again with the new version");
            return 75;
        }
        sync(false);
    }
}
