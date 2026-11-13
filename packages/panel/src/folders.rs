//! Folders watched (inotify) so that a thread or the event loop wakes when a file in them comes, goes or is written, and at no
//! other time. A folder that is not there yet is waited for: the nearest folder above it that is there is watched for new
//! folders, and the watch moves down as they appear (and back up when one is removed).

use std::collections::BTreeMap;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::PathBuf;

use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};

pub struct FolderWatch {
    fd: OwnedFd,
    /// What a file in a target folder must do to count (CREATE, CLOSE_WRITE, ...).
    mask: WatchFlags,
    targets: Vec<PathBuf>,
    /// The folders watched now: their watch and its mask.
    watched: BTreeMap<PathBuf, (i32, WatchFlags)>,
}

impl FolderWatch {
    pub fn new(targets: Vec<PathBuf>, mask: WatchFlags) -> std::io::Result<FolderWatch> {
        let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        let mut w = FolderWatch { fd, mask, targets, watched: BTreeMap::new() };
        w.aim()?;
        Ok(w)
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// A second handle to the watch, for an event loop.
    pub fn poll_fd(&self) -> std::io::Result<OwnedFd> {
        self.fd.as_fd().try_clone_to_owned()
    }

    /// Other folders from now on (an add-on installed with a file of its own).
    pub fn set_targets(&mut self, targets: Vec<PathBuf>) {
        if targets != self.targets {
            self.targets = targets;
            let _ = self.aim();
        }
    }

    /// Watch each target folder, or the nearest folder above it that is there.
    fn aim(&mut self) -> std::io::Result<()> {
        let gone = WatchFlags::DELETE_SELF | WatchFlags::MOVE_SELF | WatchFlags::ONLYDIR;
        let mut want: BTreeMap<PathBuf, WatchFlags> = BTreeMap::new();
        for t in &self.targets {
            if t.is_dir() {
                *want.entry(t.clone()).or_insert(gone) |= self.mask;
            } else if let Some(a) = t.ancestors().skip(1).find(|a| a.is_dir()) {
                *want.entry(a.to_path_buf()).or_insert(gone) |= WatchFlags::CREATE | WatchFlags::MOVED_TO;
            }
        }
        let stale: Vec<PathBuf> = self.watched.keys().filter(|d| !want.contains_key(*d)).cloned().collect();
        for d in stale {
            if let Some((wd, _)) = self.watched.remove(&d) {
                // the same folder by another name keeps its watch
                if !self.watched.values().any(|(w, _)| *w == wd) {
                    let _ = inotify::remove_watch(&self.fd, wd);
                }
            }
        }
        let mut first_err = None;
        for (d, m) in want {
            if self.watched.get(&d).is_some_and(|(_, had)| *had == m) {
                continue;
            }
            match inotify::add_watch(&self.fd, &d, m) {
                Ok(wd) => {
                    self.watched.insert(d, (wd, m));
                }
                Err(e) => {
                    first_err.get_or_insert(e);
                }
            }
        }
        match first_err {
            Some(e) if self.watched.is_empty() => Err(e.into()),
            _ => Ok(()),
        }
    }

    /// Takes what the watch saw (it does not block): the watched folders where something happened. The watch follows folders
    /// made or removed meanwhile.
    pub fn changed(&mut self) -> Vec<PathBuf> {
        let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = inotify::Reader::new(&self.fd, &mut buf);
        let mut wds: Vec<i32> = vec![];
        let mut gone: Vec<i32> = vec![];
        while let Ok(ev) = reader.next() {
            let wd = ev.wd();
            if ev.events().intersects(ReadFlags::IGNORED | ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF) {
                gone.push(wd);
            }
            if !wds.contains(&wd) {
                wds.push(wd);
            }
        }
        let dirs: Vec<PathBuf> = self.watched.iter().filter(|(_, (wd, _))| wds.contains(wd)).map(|(d, _)| d.clone()).collect();
        if !gone.is_empty() {
            self.watched.retain(|_, (wd, _)| !gone.contains(wd));
        }
        if !wds.is_empty() {
            let _ = self.aim();
        }
        dirs
    }

    /// Whether `dir` is one of the folders watched for its files (not a folder above one that is missing).
    #[cfg(test)]
    pub fn watches(&self, dir: &std::path::Path) -> bool {
        self.targets.iter().any(|t| t == dir) && self.watched.contains_key(dir)
    }
}

/// Waits until the watch has something or `timeout` passes (None: no limit), and/or another descriptor is ready. Returns which
/// were ready: (the watch, the other).
pub fn wait(watch: Option<BorrowedFd<'_>>, other: Option<BorrowedFd<'_>>, timeout: Option<std::time::Duration>) -> (bool, bool) {
    use rustix::event::{poll, PollFd, PollFlags};
    let mut fds: Vec<PollFd> = vec![];
    for fd in [watch, other].into_iter().flatten() {
        fds.push(PollFd::from_borrowed_fd(fd, PollFlags::IN));
    }
    let ts = timeout.map(|d| rustix::time::Timespec { tv_sec: d.as_secs() as i64, tv_nsec: d.subsec_nanos() as i64 });
    if fds.is_empty() {
        if let Some(d) = timeout {
            std::thread::sleep(d);
        }
        return (false, false);
    }
    if poll(&mut fds, ts.as_ref()).is_err() {
        return (false, false);
    }
    let mut it = fds.iter().map(|f| f.revents().intersects(PollFlags::IN | PollFlags::ERR | PollFlags::HUP));
    let a = if watch.is_some() { it.next().unwrap_or(false) } else { false };
    let b = if other.is_some() { it.next().unwrap_or(false) } else { false };
    (a, b)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ygg-folders-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn ready(w: &FolderWatch) -> bool {
        wait(Some(w.fd()), None, Some(Duration::from_millis(200))).0
    }

    #[test]
    fn a_file_put_in_or_taken_out_wakes_it_and_nothing_else() {
        let d = scratch("files");
        let mut w = FolderWatch::new(vec![d.clone()], WatchFlags::CREATE | WatchFlags::DELETE | WatchFlags::MOVED_TO).unwrap();
        assert!(!ready(&w), "nothing yet");
        std::fs::write(d.join("a"), "x").unwrap();
        assert!(ready(&w));
        assert_eq!(w.changed(), vec![d.clone()]);
        assert!(!ready(&w), "taken");
        std::fs::remove_file(d.join("a")).unwrap();
        assert!(ready(&w));
        w.changed();
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_made_later_is_waited_for_and_one_removed_again() {
        let d = scratch("later");
        let target = d.join("share/Trash/files");
        let mut w = FolderWatch::new(vec![target.clone()], WatchFlags::CREATE | WatchFlags::DELETE).unwrap();
        assert!(!w.watches(&target));
        std::fs::create_dir_all(&target).unwrap();
        // the watch moves down a folder at a time
        for _ in 0..4 {
            if w.watches(&target) {
                break;
            }
            assert!(ready(&w), "the folders being made are seen");
            w.changed();
        }
        assert!(w.watches(&target), "the folder itself is watched once it is there");
        std::fs::write(target.join("old.txt"), "x").unwrap();
        assert!(ready(&w));
        assert_eq!(w.changed(), vec![target.clone()]);
        std::fs::remove_dir_all(d.join("share/Trash")).unwrap();
        assert!(ready(&w));
        w.changed();
        assert!(!w.watches(&target), "gone: waited for again");
        std::fs::create_dir_all(&target).unwrap();
        for _ in 0..4 {
            if w.watches(&target) {
                break;
            }
            if ready(&w) {
                w.changed();
            }
        }
        assert!(w.watches(&target), "made again: watched again");
        let _ = std::fs::remove_dir_all(&d);
    }
}
