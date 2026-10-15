//! File operations: copy, move, rename, new folder, trash.

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

/// `name`, or `name (2)`, `name (3)` ... if something already exists in `dir`.
pub fn unique_path(dir: &Path, name: &str) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let (stem, ext) = match name.rfind('.') {
        Some(i) if i > 0 => (&name[..i], &name[i..]),
        _ => (name, ""),
    };
    (2..)
        .map(|n| dir.join(format!("{stem} ({n}){ext}")))
        .find(|p| !p.exists())
        .expect("unbounded range")
}

fn copy_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    if src.is_dir() {
        // Refuse to copy a folder into itself: it would recurse forever.
        if dst.starts_with(src) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy a folder into itself"));
        }
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            copy_recursive(&e.path(), &dst.join(e.file_name()))?;
        }
        Ok(())
    } else {
        std::fs::copy(src, dst).map(|_| ())
    }
}

/// A drive or filesystem root ("/", "C:\\") has no parent and must never be moved, copied or renamed:
/// moving one would copy a whole disk and then delete it.
fn refuse_root(p: &Path) -> io::Result<()> {
    if p.parent().is_none() || p.as_os_str().is_empty() {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "refusing to operate on a drive or filesystem root"));
    }
    Ok(())
}

/// Copy `src` into directory `dest_dir`, never overwriting. Returns the new path.
pub fn copy_into(src: &Path, dest_dir: &Path) -> io::Result<PathBuf> {
    refuse_root(src)?;
    let name = src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dst = unique_path(dest_dir, &name.to_string_lossy());
    copy_recursive(src, &dst)?;
    Ok(dst)
}

/// Move `src` into `dest_dir`, never overwriting. Falls back to copy + delete
/// across filesystems. Moving onto its own folder is a no-op returning `src`.
pub fn move_into(src: &Path, dest_dir: &Path) -> io::Result<PathBuf> {
    refuse_root(src)?;
    if src.parent() == Some(dest_dir) {
        return Ok(src.to_path_buf());
    }
    if dest_dir.starts_with(src) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot move a folder into itself"));
    }
    let name = src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    let dst = unique_path(dest_dir, &name.to_string_lossy());
    if std::fs::rename(src, &dst).is_ok() {
        return Ok(dst);
    }
    copy_recursive(src, &dst)?;
    if src.is_dir() { std::fs::remove_dir_all(src)? } else { std::fs::remove_file(src)? }
    Ok(dst)
}

/// How far a transfer has got, written by the worker (and a counting thread) and read by the window that shows it.
#[derive(Default)]
pub struct Progress {
    pub total_bytes: AtomicU64,
    pub total_files: AtomicU64,
    pub done_bytes: AtomicU64,
    pub done_files: AtomicU64,
    /// The counting of everything to copy is still going, so the totals are not final.
    pub scanning: AtomicBool,
    /// Set by the Cancel button; the transfer stops at the next chunk.
    pub cancel: AtomicBool,
    pub current: Mutex<String>,
}

impl Progress {
    fn add(&self, bytes: u64) {
        self.done_bytes.fetch_add(bytes, Ordering::Relaxed);
    }
    fn file_done(&self) {
        self.done_files.fetch_add(1, Ordering::Relaxed);
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    fn at(&self, p: &Path) {
        if let Ok(mut c) = self.current.lock() {
            *c = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        }
    }
}

/// Count the files and bytes under `srcs` into `prog`, so the window can show a percentage.
pub fn scan(srcs: &[PathBuf], prog: &Progress) {
    prog.scanning.store(true, Ordering::Relaxed);
    let mut stack: Vec<PathBuf> = srcs.to_vec();
    while let Some(p) = stack.pop() {
        if prog.cancelled() {
            break;
        }
        let Ok(m) = std::fs::symlink_metadata(&p) else { continue };
        if m.is_dir() {
            if let Ok(rd) = std::fs::read_dir(&p) {
                stack.extend(rd.flatten().map(|e| e.path()));
            }
        } else {
            prog.total_files.fetch_add(1, Ordering::Relaxed);
            prog.total_bytes.fetch_add(m.len(), Ordering::Relaxed);
        }
    }
    prog.scanning.store(false, Ordering::Relaxed);
}

/// What to do with a file that is already at the destination.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Decision {
    /// Put the new one over it.
    Replace,
    /// Leave the one that is there.
    Skip,
    /// Keep both: the new one gets a number, `name (2)`.
    KeepBoth,
}

/// What a transfer did.
#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct Stats {
    /// Files (or whole folders, when they could simply be moved) put in place that were not there before.
    pub copied: usize,
    pub replaced: usize,
    pub kept_both: usize,
    /// Left alone because you said so.
    pub skipped: usize,
    /// Left alone because the same file (size and time) was already there.
    pub same: usize,
    pub errors: Vec<String>,
}

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

/// Same size and (within two seconds) the same modified time. Small files are then compared byte for byte too,
/// so two different edits made close together are never mistaken for one file.
fn identical(src: &Path, dst: &Path, a: &std::fs::Metadata, b: &std::fs::Metadata) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let near = match (a.modified(), b.modified()) {
        (Ok(x), Ok(y)) => x.duration_since(y).unwrap_or_else(|e| e.duration()).as_secs() <= 2,
        _ => false,
    };
    if !near {
        return false;
    }
    if a.len() <= 1 << 20 {
        return matches!((std::fs::read(src), std::fs::read(dst)), (Ok(x), Ok(y)) if x == y);
    }
    true
}

/// Copy one file (or link) in 1 MiB pieces, counting them into `prog` and stopping if it is cancelled (the
/// half-written copy is removed). The modified time is kept so a later transfer can tell it is the same.
fn copy_file(src: &Path, dst: &Path, prog: &Progress) -> io::Result<()> {
    let meta = std::fs::symlink_metadata(src)?;
    if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(std::fs::read_link(src)?, dst)?;
        return Ok(());
    }
    let mut r = std::fs::File::open(src)?;
    let mut w = std::fs::File::create_new(dst)?;
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if prog.cancelled() {
            drop(w);
            let _ = std::fs::remove_file(dst);
            return Err(cancelled());
        }
        let n = match r.read(&mut buf) {
            Ok(n) => n,
            Err(e) => {
                drop(w);
                let _ = std::fs::remove_file(dst);
                return Err(e);
            }
        };
        if n == 0 {
            break;
        }
        if let Err(e) = w.write_all(&buf[..n]) {
            drop(w);
            let _ = std::fs::remove_file(dst);
            return Err(e);
        }
        prog.add(n as u64);
    }
    let _ = std::fs::set_permissions(dst, meta.permissions());
    if let Ok(m) = meta.modified() {
        let _ = w.set_modified(m);
    }
    Ok(())
}

/// Put `src` at `dst` (which must not exist): moved when asked and possible, else copied (and then removed).
fn put(src: &Path, dst: &Path, mv: bool, prog: &Progress) -> io::Result<()> {
    if mv && std::fs::rename(src, dst).is_ok() {
        prog.add(std::fs::symlink_metadata(dst).map(|m| m.len()).unwrap_or(0));
        return Ok(());
    }
    copy_file(src, dst, prog)?;
    if mv {
        std::fs::remove_file(src)?;
    }
    Ok(())
}

/// Put `src` over the existing `dst`: written beside it first, then swapped in, so a failure halfway
/// never leaves a half-written file where a good one was.
fn put_over(src: &Path, dst: &Path, mv: bool, prog: &Progress) -> io::Result<()> {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dst.with_file_name(format!(".{name}.minos-tmp"));
    let _ = std::fs::remove_file(&tmp);
    copy_file(src, &tmp, prog)?;
    if let Err(e) = std::fs::rename(&tmp, dst) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if mv {
        std::fs::remove_file(src)?;
    }
    Ok(())
}

/// Bring `src` into `dest_dir`, merging with what is there: folders of the same name are entered, missing
/// files are added, and a file that is already there (and not the same) is put to `ask`, which answers
/// `None` to stop the whole transfer. Nothing is overwritten without a `Replace`. With `mv`, a source file
/// is removed only after it has been put in place. Returns where the item ended up.
pub fn transfer(src: &Path, dest_dir: &Path, mv: bool, ask: &mut dyn FnMut(&Path, &Path) -> Option<Decision>, stats: &mut Stats, prog: &Progress) -> io::Result<PathBuf> {
    refuse_root(src)?;
    if src.is_dir() && dest_dir.starts_with(src) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy or move a folder into itself"));
    }
    let name = src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))?;
    if src.parent() == Some(dest_dir) {
        if mv {
            return Ok(src.to_path_buf());
        }
        // copying into the folder it is already in: a numbered copy, as before
        let dst = unique_path(dest_dir, &name.to_string_lossy());
        copy_recursive(src, &dst)?;
        stats.copied += 1;
        return Ok(dst);
    }
    let target = dest_dir.join(name);
    merge(src, &target, mv, ask, stats, prog)?;
    Ok(target)
}

fn merge(src: &Path, dst: &Path, mv: bool, ask: &mut dyn FnMut(&Path, &Path) -> Option<Decision>, stats: &mut Stats, prog: &Progress) -> io::Result<()> {
    let sm = std::fs::symlink_metadata(src)?;
    let parent = dst.parent().unwrap_or(Path::new("/"));
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if sm.is_dir() {
        match std::fs::symlink_metadata(dst) {
            Ok(dm) if dm.is_dir() => {} // the same folder is there: merge into it
            Ok(_) => {
                // a file is in the way of this folder
                match ask(src, dst).ok_or_else(cancelled)? {
                    Decision::Skip | Decision::Replace => {
                        // a file is never silently swapped for a folder
                        stats.skipped += 1;
                        return Ok(());
                    }
                    Decision::KeepBoth => {
                        stats.kept_both += 1;
                        return merge(src, &unique_path(parent, &name), mv, ask, stats, prog);
                    }
                }
            }
            Err(_) => {
                if mv && std::fs::rename(src, dst).is_ok() {
                    stats.copied += 1;
                    return Ok(());
                }
            }
        }
        std::fs::create_dir_all(dst)?;
        for e in std::fs::read_dir(src)? {
            let e = e?;
            let (from, to) = (e.path(), dst.join(e.file_name()));
            match merge(&from, &to, mv, ask, stats, prog) {
                Err(err) if err.kind() == io::ErrorKind::Interrupted => return Err(err),
                Err(err) => stats.errors.push(format!("{}: {err}", from.display())),
                Ok(()) => {}
            }
        }
        if mv {
            let _ = std::fs::remove_dir(src); // only works once it is empty
        }
        return Ok(());
    }
    prog.at(src);
    match std::fs::symlink_metadata(dst) {
        Err(_) => {
            put(src, dst, mv, prog)?;
            stats.copied += 1;
        }
        Ok(dm) if dm.is_dir() => {
            // a folder is in the way of this file: keep both, never touch the folder
            put(src, &unique_path(parent, &name), mv, prog)?;
            stats.kept_both += 1;
        }
        Ok(dm) if identical(src, dst, &sm, &dm) => {
            stats.same += 1;
            prog.add(sm.len());
        }
        Ok(_) => match ask(src, dst).ok_or_else(cancelled)? {
            Decision::Replace => {
                put_over(src, dst, mv, prog)?;
                stats.replaced += 1;
            }
            Decision::Skip => {
                stats.skipped += 1;
                prog.add(sm.len());
            }
            Decision::KeepBoth => {
                put(src, &unique_path(parent, &name), mv, prog)?;
                stats.kept_both += 1;
            }
        },
    }
    prog.file_done();
    Ok(())
}

pub fn rename(path: &Path, new_name: &str) -> io::Result<PathBuf> {
    refuse_root(path)?;
    if new_name.is_empty() || new_name.contains('/') || new_name.contains('\\') || new_name == "." || new_name == ".." {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid name"));
    }
    let dst = path.with_file_name(new_name);
    if dst.exists() {
        return Err(io::Error::new(io::ErrorKind::AlreadyExists, "a file with that name exists"));
    }
    std::fs::rename(path, &dst)?;
    Ok(dst)
}

pub fn new_folder(dir: &Path) -> io::Result<PathBuf> {
    let p = unique_path(dir, "New folder");
    std::fs::create_dir(&p)?;
    Ok(p)
}

pub fn new_file(dir: &Path) -> io::Result<PathBuf> {
    let p = unique_path(dir, "New file");
    std::fs::File::create_new(&p)?;
    Ok(p)
}

/// Send to the freedesktop trash (recoverable). Never deletes permanently.
pub fn trash(path: &Path) -> Result<(), String> {
    // A drive or filesystem root ("/", "C:\") has no parent. Never send one to the trash.
    if path.parent().is_none() || path.as_os_str().is_empty() {
        return Err("refusing to trash a drive or filesystem root".into());
    }
    trash::delete(path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("lf-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn unique_names() {
        let d = tmp("unique");
        std::fs::write(d.join("a.txt"), "x").unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a (2).txt"));
        std::fs::write(d.join("a (2).txt"), "x").unwrap();
        assert_eq!(unique_path(&d, "a.txt"), d.join("a (3).txt"));
        assert_eq!(unique_path(&d, "b"), d.join("b"));
    }

    #[test]
    fn copy_move_rename() {
        let d = tmp("ops");
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        std::fs::write(d.join("src/sub/f.txt"), "hi").unwrap();
        std::fs::create_dir(d.join("dest")).unwrap();

        let c = copy_into(&d.join("src"), &d.join("dest")).unwrap();
        assert_eq!(std::fs::read_to_string(c.join("sub/f.txt")).unwrap(), "hi");
        assert!(d.join("src/sub/f.txt").exists());

        let c2 = copy_into(&d.join("src"), &d.join("dest")).unwrap();
        assert_eq!(c2, d.join("dest/src (2)"));

        let m = move_into(&d.join("src"), &d.join("dest/src (2)")).unwrap();
        assert!(m.join("sub/f.txt").exists());
        assert!(!d.join("src").exists());

        let r = rename(&m, "renamed").unwrap();
        assert!(r.ends_with("renamed") && r.exists());
        assert!(rename(&r, "a/b").is_err());
        // "sub" now sits next to nothing, but renaming onto an existing sibling must fail.
        std::fs::create_dir(r.with_file_name("taken")).unwrap();
        assert!(rename(&r, "taken").is_err());
    }

    #[test]
    fn refuses_folder_into_itself() {
        let d = tmp("self");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        assert!(copy_into(&d.join("a"), &d.join("a/b")).is_err());
        assert!(move_into(&d.join("a"), &d.join("a/b")).is_err());
        assert!(d.join("a/b").exists());
    }

    #[test]
    fn roots_are_never_trashed_and_new_file_does_not_overwrite() {
        assert!(trash(Path::new("/")).is_err());
        assert!(trash(Path::new("")).is_err());
        let d = tmp("nfile");
        assert_eq!(new_file(&d).unwrap(), d.join("New file"));
        assert_eq!(new_file(&d).unwrap(), d.join("New file (2)"));
    }

    #[test]
    fn roots_cannot_be_moved_copied_or_renamed() {
        let d = tmp("rootops");
        assert!(move_into(Path::new("/"), &d).is_err());
        assert!(copy_into(Path::new("/"), &d).is_err());
        assert!(rename(Path::new("/"), "x").is_err());
    }

    #[test]
    fn new_folder_does_not_overwrite() {
        let d = tmp("nf");
        assert_eq!(new_folder(&d).unwrap(), d.join("New folder"));
        assert_eq!(new_folder(&d).unwrap(), d.join("New folder (2)"));
    }

    #[test]
    fn transfer_merges_and_asks_before_overwriting() {
        let d = tmp("merge");
        for (dir, files) in [("src/Sarina", vec![("a.txt", "same"), ("b.txt", "NEW b"), ("sub/c.txt", "c")]), ("dest/Sarina", vec![("b.txt", "old b"), ("keep.txt", "mine")])] {
            for (f, body) in files {
                let p = d.join(dir).join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            }
        }
        let mut asked = vec![];
        let mut stats = Stats::default();
        // answer Skip: the existing b.txt must survive
        let out = transfer(&d.join("src/Sarina"), &d.join("dest"), false, &mut |s, t| {
            asked.push((s.to_path_buf(), t.to_path_buf()));
            Some(Decision::Skip)
        }, &mut stats, &Progress::default())
        .unwrap();
        assert_eq!(out, d.join("dest/Sarina"));
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/b.txt")).unwrap(), "old b");
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/a.txt")).unwrap(), "same"); // missing: added
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/sub/c.txt")).unwrap(), "c");
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/keep.txt")).unwrap(), "mine");
        assert_eq!(asked.len(), 1);
        assert_eq!((stats.copied, stats.skipped, stats.replaced), (2, 1, 0));
        assert!(d.join("src/Sarina/a.txt").exists(), "a copy leaves the source alone");

        // the same again: a.txt and c.txt are now identical, so only b.txt is asked about; Replace swaps it
        let mut stats = Stats::default();
        transfer(&d.join("src/Sarina"), &d.join("dest"), false, &mut |_, _| Some(Decision::Replace), &mut stats, &Progress::default()).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/b.txt")).unwrap(), "NEW b");
        assert_eq!((stats.replaced, stats.same), (1, 2));

        // KeepBoth keeps the old one and numbers the new
        std::fs::write(d.join("src/Sarina/b.txt"), "third b").unwrap();
        let mut stats = Stats::default();
        transfer(&d.join("src/Sarina"), &d.join("dest"), false, &mut |_, _| Some(Decision::KeepBoth), &mut stats, &Progress::default()).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/b.txt")).unwrap(), "NEW b");
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/b (2).txt")).unwrap(), "third b");
    }

    #[test]
    fn transfer_cancel_stops_and_move_keeps_what_was_skipped() {
        let d = tmp("mvmerge");
        std::fs::create_dir_all(d.join("src/x")).unwrap();
        std::fs::create_dir_all(d.join("dest/x")).unwrap();
        std::fs::write(d.join("src/x/clash.txt"), "src").unwrap();
        std::fs::write(d.join("src/x/new.txt"), "n").unwrap();
        std::fs::write(d.join("dest/x/clash.txt"), "dest").unwrap();
        let mut stats = Stats::default();
        let r = transfer(&d.join("src/x"), &d.join("dest"), true, &mut |_, _| None, &mut stats, &Progress::default());
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(std::fs::read_to_string(d.join("dest/x/clash.txt")).unwrap(), "dest");
        assert!(d.join("src/x/clash.txt").exists());

        let mut stats = Stats::default();
        transfer(&d.join("src/x"), &d.join("dest"), true, &mut |_, _| Some(Decision::Skip), &mut stats, &Progress::default()).unwrap();
        assert!(d.join("dest/x/new.txt").exists() && !d.join("src/x/new.txt").exists(), "what was added is moved");
        assert!(d.join("src/x/clash.txt").exists(), "what was skipped stays in the source");
        assert_eq!(std::fs::read_to_string(d.join("dest/x/clash.txt")).unwrap(), "dest");
    }

    #[test]
    fn progress_counts_bytes_and_cancel_stops_without_leaving_halves() {
        let d = tmp("prog");
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        std::fs::create_dir(d.join("dest")).unwrap();
        std::fs::write(d.join("src/a.bin"), vec![1u8; 3_000_000]).unwrap();
        std::fs::write(d.join("src/sub/b.bin"), vec![2u8; 500]).unwrap();
        let prog = Progress::default();
        scan(&[d.join("src")], &prog);
        assert_eq!((prog.total_files.load(Ordering::Relaxed), prog.total_bytes.load(Ordering::Relaxed)), (2, 3_000_500));
        let mut stats = Stats::default();
        transfer(&d.join("src"), &d.join("dest"), false, &mut |_, _| Some(Decision::Skip), &mut stats, &prog).unwrap();
        assert_eq!((prog.done_files.load(Ordering::Relaxed), prog.done_bytes.load(Ordering::Relaxed)), (2, 3_000_500));
        assert_eq!(std::fs::metadata(d.join("dest/src/a.bin")).unwrap().len(), 3_000_000);

        // cancelled before it starts: nothing half-written is left behind
        let _ = std::fs::remove_dir_all(d.join("dest/src"));
        let prog = Progress::default();
        prog.cancel.store(true, Ordering::Relaxed);
        let r = transfer(&d.join("src"), &d.join("dest"), false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &prog);
        assert_eq!(r.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert!(!d.join("dest/src/a.bin").exists());
    }
}
