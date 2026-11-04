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
        _ => {
            eprintln!("usage: yggdrasil-spell check WORD... | text TEXT | languages | words | add WORD | remove WORD | sync | watch");
            2
        }
    };
    std::process::exit(code);
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

fn watch() -> i32 {
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
        // wait for a change to one of the lists, or an hour (then look for newly installed apps)
        let mut relevant = false;
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, 3_600_000) } > 0 {
            let mut buf = [0u8; 8192];
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            let mut off = 0usize;
            while n > 0 && off + 16 <= n as usize {
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
        sync(false);
    }
}
