//! File operations: copy, move, rename, new folder, trash.

use std::collections::VecDeque;
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
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|p| !p.exists()).expect("unbounded range")
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
    /// Every file the worker has reached and what became of it (the transfer panel's list).
    pub log: Mutex<FileLog>,
}

/// What became of one file (or folder) of a transfer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FileState {
    /// Being copied now: `FileEntry::done` bytes of `FileEntry::size`.
    Copying,
    /// Put in place (it was not there before).
    Done,
    /// Put over the one that was there, as asked.
    Replaced,
    /// Put beside the one that was there, under a numbered name.
    KeptBoth,
    /// Left alone because you said so.
    Skipped,
    /// Left alone because the same file was already there.
    Same,
    /// Cancel stopped it halfway (the half-written copy was removed).
    Stopped,
    /// It could not be copied or moved: why.
    Failed(String),
}

impl FileState {
    /// The short reason line under the file's name.
    pub fn reason(&self) -> &str {
        match self {
            FileState::Copying => "Copying",
            FileState::Done => "Done",
            FileState::Replaced => "Replaced the one that was there",
            FileState::KeptBoth => "Kept both: the new one has a number",
            FileState::Skipped => "Skipped",
            FileState::Same => "Already there",
            FileState::Stopped => "Stopped",
            FileState::Failed(why) => why,
        }
    }
}

/// One line of the transfer panel: a file the worker reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileEntry {
    /// Its name, ready to draw.
    pub name: String,
    /// Where it comes from.
    pub src: PathBuf,
    /// Where it was going.
    pub dst: PathBuf,
    pub is_dir: bool,
    pub size: u64,
    /// Bytes written so far (while it is being copied).
    pub done: u64,
    pub state: FileState,
}

/// How many lines of each kind a transfer keeps (the files done before them are only counted).
pub const LOG_CAP: usize = 5000;

/// The files a transfer has reached, newest last. A copy of millions of files keeps only the last [`LOG_CAP`] (and every
/// failure up to [`LOG_CAP`] of them, however long ago it happened), with a count of the rest.
#[derive(Default, Debug)]
pub struct FileLog {
    /// The newest files that did not fail, oldest first.
    pub recent: VecDeque<FileEntry>,
    /// The files that failed, oldest first.
    pub failed: Vec<FileEntry>,
    /// Lines no longer kept (older files that did not fail, and failures past the cap).
    pub dropped: u64,
    /// All failures, also those past the cap.
    pub failures: u64,
    /// How many were skipped and how many were already there (all of them, not only those kept).
    pub skipped: u64,
    pub same: u64,
}

impl FileLog {
    /// Lines kept: failures and the others.
    pub fn len(&self) -> usize {
        self.failed.len() + self.recent.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `i`th line as the panel lists them: the failures first (newest first), then the others, newest first.
    pub fn row(&self, i: usize) -> Option<&FileEntry> {
        let nf = self.failed.len();
        if i < nf {
            return self.failed.get(nf - 1 - i);
        }
        let j = i - nf;
        let nr = self.recent.len();
        if j < nr {
            self.recent.get(nr - 1 - j)
        } else {
            None
        }
    }

    fn push(&mut self, e: FileEntry) {
        if self.recent.len() >= LOG_CAP {
            self.recent.pop_front();
            self.dropped += 1;
        }
        self.recent.push_back(e);
    }

    /// The newest line ends in `state` (a failure moves to the failures).
    fn finish_last(&mut self, src: &Path, state: FileState) {
        match &state {
            FileState::Skipped => self.skipped += 1,
            FileState::Same => self.same += 1,
            _ => {}
        }
        let last_is_it = self.recent.back().is_some_and(|e| e.src == src && e.state == FileState::Copying);
        if let FileState::Failed(_) = state {
            self.failures += 1;
            let entry = if last_is_it { self.recent.pop_back() } else { None };
            if self.failed.len() >= LOG_CAP {
                self.dropped += 1;
                return;
            }
            let mut e = entry.unwrap_or_else(|| FileEntry {
                name: name_of(src),
                src: src.to_path_buf(),
                dst: PathBuf::new(),
                is_dir: false,
                size: 0,
                done: 0,
                state: FileState::Copying,
            });
            e.state = state;
            self.failed.push(e);
        } else if last_is_it {
            if let Some(e) = self.recent.back_mut() {
                if !matches!(state, FileState::Stopped) {
                    e.done = e.size;
                }
                e.state = state;
            }
        }
    }
}

fn name_of(p: &Path) -> String {
    p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string())
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
    /// A file (or folder) reached: its line, as being copied.
    fn begin(&self, src: &Path, dst: &Path, is_dir: bool, size: u64) {
        if let Ok(mut log) = self.log.lock() {
            log.push(FileEntry {
                name: name_of(src),
                src: src.to_path_buf(),
                dst: dst.to_path_buf(),
                is_dir,
                size,
                done: 0,
                state: FileState::Copying,
            });
        }
    }
    /// How far the file being copied is.
    fn copied(&self, src: &Path, bytes: u64) {
        if let Ok(mut log) = self.log.lock() {
            if let Some(e) = log.recent.back_mut().filter(|e| e.src == src) {
                e.done = bytes;
            }
        }
    }
    /// The line for `src` ends: what became of it.
    fn end(&self, src: &Path, state: FileState) {
        if let Ok(mut log) = self.log.lock() {
            log.finish_last(src, state);
        }
    }
    /// The line for `src` ends with `err`: stopped (cancelled) or failed with the reason.
    fn end_err(&self, src: &Path, err: &io::Error) {
        let state = if err.kind() == io::ErrorKind::Interrupted { FileState::Stopped } else { FileState::Failed(reason(err)) };
        self.end(src, state);
    }
}

/// An error as a person reads it: "Permission denied", without "(os error 13)".
pub fn reason(err: &io::Error) -> String {
    let s = err.to_string();
    match s.find(" (os error") {
        Some(i) => s[..i].to_string(),
        None => s,
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
    let mut written = 0u64;
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
        written += n as u64;
        prog.copied(src, written);
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
pub fn transfer(
    src: &Path,
    dest_dir: &Path,
    mv: bool,
    ask: &mut dyn FnMut(&Path, &Path) -> Option<Decision>,
    stats: &mut Stats,
    prog: &Progress,
) -> io::Result<PathBuf> {
    let refused = refuse_root(src).and_then(|()| {
        if src.is_dir() && dest_dir.starts_with(src) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "cannot copy or move a folder into itself"));
        }
        src.file_name().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no file name"))
    });
    let name = match refused {
        Ok(n) => n,
        Err(e) => {
            prog.begin(src, dest_dir, src.is_dir(), 0);
            prog.end_err(src, &e);
            return Err(e);
        }
    };
    if src.parent() == Some(dest_dir) {
        if mv {
            return Ok(src.to_path_buf());
        }
        // copying into the folder it is already in: a numbered copy, as before
        let dst = unique_path(dest_dir, &name.to_string_lossy());
        prog.begin(src, &dst, src.is_dir(), 0);
        if let Err(e) = copy_recursive(src, &dst) {
            prog.end_err(src, &e);
            return Err(e);
        }
        prog.end(src, FileState::Done);
        stats.copied += 1;
        return Ok(dst);
    }
    let target = dest_dir.join(name);
    merge(src, &target, mv, ask, stats, prog)?;
    Ok(target)
}

fn merge(
    src: &Path,
    dst: &Path,
    mv: bool,
    ask: &mut dyn FnMut(&Path, &Path) -> Option<Decision>,
    stats: &mut Stats,
    prog: &Progress,
) -> io::Result<()> {
    let sm = match std::fs::symlink_metadata(src) {
        Ok(m) => m,
        Err(err) => {
            prog.begin(src, dst, false, 0);
            prog.end_err(src, &err);
            return Err(err);
        }
    };
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
                        prog.begin(src, dst, true, 0);
                        prog.end(src, FileState::Skipped);
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
                    prog.begin(src, dst, true, 0);
                    prog.end(src, FileState::Done);
                    return Ok(());
                }
            }
        }
        let opened = std::fs::create_dir_all(dst).and_then(|()| std::fs::read_dir(src));
        let entries = match opened {
            Ok(rd) => rd,
            Err(err) => {
                prog.begin(src, dst, true, 0);
                prog.end_err(src, &err);
                return Err(err);
            }
        };
        for e in entries {
            let e = match e {
                Ok(e) => e,
                Err(err) => {
                    prog.begin(src, dst, true, 0);
                    prog.end_err(src, &err);
                    return Err(err);
                }
            };
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
    prog.begin(src, dst, false, sm.len());
    match put_file(src, dst, &sm, parent, &name, mv, ask, stats, prog) {
        Ok(state) => {
            prog.end(src, state);
            prog.file_done();
            Ok(())
        }
        Err(err) => {
            prog.end_err(src, &err);
            Err(err)
        }
    }
}

/// The file part of [`merge`]: put the file `src` at `dst`, asking when another is there; what became of it.
#[allow(clippy::too_many_arguments)]
fn put_file(
    src: &Path,
    dst: &Path,
    sm: &std::fs::Metadata,
    parent: &Path,
    name: &str,
    mv: bool,
    ask: &mut dyn FnMut(&Path, &Path) -> Option<Decision>,
    stats: &mut Stats,
    prog: &Progress,
) -> io::Result<FileState> {
    Ok(match std::fs::symlink_metadata(dst) {
        Err(_) => {
            put(src, dst, mv, prog)?;
            stats.copied += 1;
            FileState::Done
        }
        Ok(dm) if dm.is_dir() => {
            // a folder is in the way of this file: keep both, never touch the folder
            put(src, &unique_path(parent, name), mv, prog)?;
            stats.kept_both += 1;
            FileState::KeptBoth
        }
        Ok(dm) if identical(src, dst, sm, &dm) => {
            stats.same += 1;
            prog.add(sm.len());
            FileState::Same
        }
        Ok(_) => match ask(src, dst).ok_or_else(cancelled)? {
            Decision::Replace => {
                put_over(src, dst, mv, prog)?;
                stats.replaced += 1;
                FileState::Replaced
            }
            Decision::Skip => {
                stats.skipped += 1;
                prog.add(sm.len());
                FileState::Skipped
            }
            Decision::KeepBoth => {
                put(src, &unique_path(parent, name), mv, prog)?;
                stats.kept_both += 1;
                FileState::KeptBoth
            }
        },
    })
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

    /// A copy or move as the window makes one (nothing is in the way in these tests: Skip is never needed).
    fn copy_to(src: &Path, dest: &Path) -> io::Result<PathBuf> {
        transfer(src, dest, false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &Progress::default())
    }

    fn move_to(src: &Path, dest: &Path) -> io::Result<PathBuf> {
        transfer(src, dest, true, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &Progress::default())
    }

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

        let c = copy_to(&d.join("src"), &d.join("dest")).unwrap();
        assert_eq!(std::fs::read_to_string(c.join("sub/f.txt")).unwrap(), "hi");
        assert!(d.join("src/sub/f.txt").exists());

        // copying into the folder it is already in: a numbered copy
        let c2 = copy_to(&d.join("dest/src"), &d.join("dest")).unwrap();
        assert_eq!(c2, d.join("dest/src (2)"));

        let m = move_to(&d.join("src"), &d.join("dest/src (2)")).unwrap();
        assert!(m.join("sub/f.txt").exists());
        assert!(!d.join("src").exists());
        // moving onto its own folder changes nothing
        assert_eq!(move_to(&m, &d.join("dest/src (2)")).unwrap(), m);

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
        assert!(copy_to(&d.join("a"), &d.join("a/b")).is_err());
        assert!(move_to(&d.join("a"), &d.join("a/b")).is_err());
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
        assert!(move_to(Path::new("/"), &d).is_err());
        assert!(copy_to(Path::new("/"), &d).is_err());
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
        for (dir, files) in [
            ("src/Sarina", vec![("a.txt", "same"), ("b.txt", "NEW b"), ("sub/c.txt", "c")]),
            ("dest/Sarina", vec![("b.txt", "old b"), ("keep.txt", "mine")]),
        ] {
            for (f, body) in files {
                let p = d.join(dir).join(f);
                std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                std::fs::write(p, body).unwrap();
            }
        }
        let mut asked = vec![];
        let mut stats = Stats::default();
        // answer Skip: the existing b.txt must survive
        let out = transfer(
            &d.join("src/Sarina"),
            &d.join("dest"),
            false,
            &mut |s, t| {
                asked.push((s.to_path_buf(), t.to_path_buf()));
                Some(Decision::Skip)
            },
            &mut stats,
            &Progress::default(),
        )
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
        transfer(&d.join("src/Sarina"), &d.join("dest"), false, &mut |_, _| Some(Decision::Replace), &mut stats, &Progress::default())
            .unwrap();
        assert_eq!(std::fs::read_to_string(d.join("dest/Sarina/b.txt")).unwrap(), "NEW b");
        assert_eq!((stats.replaced, stats.same), (1, 2));

        // KeepBoth keeps the old one and numbers the new
        std::fs::write(d.join("src/Sarina/b.txt"), "third b").unwrap();
        let mut stats = Stats::default();
        transfer(&d.join("src/Sarina"), &d.join("dest"), false, &mut |_, _| Some(Decision::KeepBoth), &mut stats, &Progress::default())
            .unwrap();
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

    fn rows(prog: &Progress) -> Vec<(String, FileState)> {
        let log = prog.log.lock().unwrap();
        (0..log.len()).map(|i| log.row(i).unwrap()).map(|e| (e.name.clone(), e.state.clone())).collect()
    }

    #[test]
    fn the_log_lists_each_file_and_what_became_of_it() {
        let d = tmp("log");
        std::fs::create_dir_all(d.join("src/sub")).unwrap();
        std::fs::create_dir_all(d.join("dest/src")).unwrap();
        std::fs::write(d.join("src/new.txt"), "new").unwrap();
        std::fs::write(d.join("src/sub/same.txt"), "same").unwrap();
        std::fs::write(d.join("src/clash.txt"), "mine").unwrap();
        std::fs::write(d.join("dest/src/clash.txt"), "theirs").unwrap();
        // copy sub/same.txt first, so it is already there (same size, time and bytes) the second time
        std::fs::create_dir_all(d.join("dest/src/sub")).unwrap();
        let prog0 = Progress::default();
        transfer(&d.join("src/sub"), &d.join("dest/src"), false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &prog0).unwrap();
        assert_eq!(rows(&prog0), vec![("same.txt".into(), FileState::Done)]);

        let prog = Progress::default();
        transfer(&d.join("src"), &d.join("dest"), false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &prog).unwrap();
        let mut got = rows(&prog);
        got.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(
            got,
            vec![("clash.txt".into(), FileState::Skipped), ("new.txt".into(), FileState::Done), ("same.txt".into(), FileState::Same),]
        );
        let log = prog.log.lock().unwrap();
        assert_eq!((log.skipped, log.same, log.failures), (1, 1, 0));
        let new = log.recent.iter().find(|e| e.name == "new.txt").unwrap();
        assert_eq!((new.size, new.done), (3, 3), "a finished file shows all its bytes");
        assert_eq!(new.dst, d.join("dest/src/new.txt"));
    }

    #[test]
    fn failures_are_kept_listed_first_with_the_reason() {
        let d = tmp("logfail");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir(d.join("dest")).unwrap();
        std::fs::write(d.join("src/ok.txt"), "ok").unwrap();
        // a file in the way of the destination folder's own creation: the folder "src" cannot be made under it
        std::fs::write(d.join("blocker"), "x").unwrap();
        let prog = Progress::default();
        let r = transfer(&d.join("src"), &d.join("blocker"), false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &prog);
        assert!(r.is_err());
        // a source that is gone
        let r = transfer(&d.join("missing.txt"), &d.join("dest"), false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &prog);
        assert!(r.is_err());
        transfer(&d.join("src/ok.txt"), &d.join("dest"), false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), &prog).unwrap();
        let got = rows(&prog);
        assert_eq!(got.len(), 3);
        // failures first (newest first), then the rest
        assert_eq!(got[0].0, "missing.txt");
        assert!(matches!(&got[0].1, FileState::Failed(why) if why.contains("No such file")), "{:?}", got[0]);
        assert!(!got[0].1.reason().contains("os error"));
        assert_eq!(got[1].0, "src");
        assert!(matches!(got[1].1, FileState::Failed(_)));
        assert_eq!(got[2], ("ok.txt".into(), FileState::Done));
        assert_eq!(prog.log.lock().unwrap().failures, 2);
        // a folder into itself is refused and listed
        let prog = Progress::default();
        assert!(copy_to_with(&d.join("src"), &d.join("src"), &prog).is_err());
        assert!(matches!(rows(&prog)[0].1, FileState::Failed(_)));
    }

    fn copy_to_with(src: &Path, dest: &Path, prog: &Progress) -> io::Result<PathBuf> {
        transfer(src, dest, false, &mut |_, _| Some(Decision::Skip), &mut Stats::default(), prog)
    }

    #[test]
    fn the_log_is_capped_and_counts_the_rest() {
        let mut log = FileLog::default();
        let entry = |i: usize, state: FileState| FileEntry {
            name: format!("f{i}"),
            src: PathBuf::from(format!("/s/f{i}")),
            dst: PathBuf::new(),
            is_dir: false,
            size: 1,
            done: 0,
            state,
        };
        for i in 0..LOG_CAP + 10 {
            log.push(entry(i, FileState::Copying));
            let state = if i % 2 == 0 { FileState::Done } else { FileState::Failed("nope".into()) };
            log.finish_last(Path::new(&format!("/s/f{i}")), state);
        }
        // half failed: all of those kept (under the cap), the others too (under the cap)
        assert_eq!(log.failures as usize, (LOG_CAP + 10) / 2);
        assert_eq!(log.failed.len(), (LOG_CAP + 10) / 2);
        assert_eq!(log.recent.len(), (LOG_CAP + 10) / 2);
        for i in 0..LOG_CAP * 2 {
            log.push(entry(i, FileState::Copying));
            log.finish_last(Path::new(&format!("/s/f{i}")), FileState::Done);
        }
        assert_eq!(log.recent.len(), LOG_CAP);
        assert!(log.dropped > 0);
        assert_eq!(log.row(0).unwrap().state, FileState::Failed("nope".into()));
        assert_eq!(log.row(log.failed.len()).unwrap().name, format!("f{}", LOG_CAP * 2 - 1), "the newest first after the failures");
        assert!(log.row(log.len()).is_none());
        // past the cap, failures are counted, not kept
        for i in 0..LOG_CAP {
            log.push(entry(i, FileState::Copying));
            log.finish_last(Path::new(&format!("/s/f{i}")), FileState::Failed("late".into()));
        }
        assert_eq!(log.failed.len(), LOG_CAP);
        assert_eq!(log.failures as usize, (LOG_CAP + 10) / 2 + LOG_CAP);
    }

    #[test]
    fn a_cancelled_file_is_listed_as_stopped() {
        let d = tmp("logstop");
        std::fs::create_dir_all(d.join("src")).unwrap();
        std::fs::create_dir(d.join("dest")).unwrap();
        std::fs::write(d.join("src/big.bin"), vec![0u8; 10]).unwrap();
        let prog = Progress::default();
        prog.cancel.store(true, Ordering::Relaxed);
        assert!(copy_to_with(&d.join("src/big.bin"), &d.join("dest"), &prog).is_err());
        assert_eq!(rows(&prog), vec![("big.bin".into(), FileState::Stopped)]);
    }
}
