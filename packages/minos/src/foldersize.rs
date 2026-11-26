//! How much a folder holds, for the status bar: the files directly in it are summed from the listing at once; the folders in
//! it are walked on a thread of their own, which stops when the window moves to another folder. A folder on a drive that may
//! stall (network and cloud mounts, `places::may_stall`) is never walked: only its direct files count. A walk never goes into
//! such a drive mounted further down, nor into the kernel's own file systems (/proc, /sys).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use crate::listing::Entry;
use crate::places;

/// What a walk found: each folder's total in the order asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Counted {
    pub bytes: Vec<u64>,
    /// Something inside was not walked (a network or cloud drive mounted there): the totals are less than the truth.
    pub partial: bool,
}

/// The bytes of everything under `dir` (links counted as themselves, never followed), or None when `cancel` is set. Folders
/// at or under one of `skip` are not walked, and `partial` is set when one is met.
pub fn folder_bytes(dir: &Path, skip: &[PathBuf], cancel: &AtomicBool, partial: &mut bool) -> Option<u64> {
    let mut total = 0u64;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        if skip.iter().any(|s| d.starts_with(s)) {
            *partial = true;
            continue;
        }
        let Ok(rd) = std::fs::read_dir(&d) else { continue };
        for e in rd.flatten() {
            if cancel.load(Ordering::Relaxed) {
                return None;
            }
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_dir() {
                stack.push(e.path());
            } else if let Ok(m) = e.metadata() {
                total += m.len();
            }
        }
    }
    Some(total)
}

/// Each of `dirs` walked with [`folder_bytes`]; None when cancelled.
pub fn count(dirs: &[PathBuf], skip: &[PathBuf], cancel: &AtomicBool) -> Option<Counted> {
    let mut partial = false;
    let mut bytes = Vec::with_capacity(dirs.len());
    for d in dirs {
        bytes.push(folder_bytes(d, skip, cancel, &mut partial)?);
    }
    Some(Counted { bytes, partial })
}

/// Whether the folders inside `dir` may be walked: not when `dir` itself is on a drive that may stall.
pub fn may_walk_in(mountinfo: &str, dir: &Path) -> bool {
    !places::may_stall_in(mountinfo, dir)
}

/// The listing's sums, ready for the status bar (no walking: only what the listing already says).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Direct {
    /// Items shown.
    pub items: usize,
    /// Of those, folders.
    pub folders: usize,
    /// The bytes of the files shown.
    pub file_bytes: u64,
}

/// Sums `entries` as `shown` decides (the hidden-items setting, the filter), without allocating.
pub fn direct(entries: &[Entry], shown: impl Fn(&Entry) -> bool) -> Direct {
    let mut d = Direct::default();
    for e in entries.iter().filter(|e| shown(e)) {
        d.items += 1;
        if e.is_dir {
            d.folders += 1;
        } else {
            d.file_bytes += e.size;
        }
    }
    d
}

/// A walk running on its own thread. Dropping it stops the walk.
pub struct Counter {
    cancel: Arc<AtomicBool>,
    rx: Receiver<Counted>,
}

impl Counter {
    /// Walk `dirs` (folders on drives that do not stall; the caller checks with [`may_walk_in`]). `wake` is called once the
    /// totals are in. The thread runs at a low priority so the walk never slows the desktop.
    pub fn start(dirs: Vec<PathBuf>, wake: impl FnOnce() + Send + 'static) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let c = cancel.clone();
        std::thread::spawn(move || {
            lower_priority();
            let skip = std::fs::read_to_string("/proc/self/mountinfo").map(|t| places::count_skips_in(&t)).unwrap_or_default();
            if let Some(counted) = count(&dirs, &skip, &c) {
                if tx.send(counted).is_ok() {
                    wake();
                }
            }
        });
        Self { cancel, rx }
    }

    /// The totals, once the walk is done.
    pub fn poll(&self) -> Option<Counted> {
        self.rx.try_recv().ok()
    }
}

impl Drop for Counter {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// This thread at a lower priority (nice 10), as a background job should be.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: plain system calls on this thread's own id; nothing is shared.
    unsafe {
        let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
        libc::setpriority(libc::PRIO_PROCESS, tid, 10);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("minos-size-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn tree(d: &Path) {
        std::fs::create_dir_all(d.join("a/b/c")).unwrap();
        std::fs::create_dir_all(d.join("e")).unwrap();
        std::fs::write(d.join("a/one"), vec![0u8; 100]).unwrap();
        std::fs::write(d.join("a/b/two"), vec![0u8; 20]).unwrap();
        std::fs::write(d.join("a/b/c/.hidden"), vec![0u8; 3]).unwrap();
        std::fs::write(d.join("top.txt"), vec![0u8; 7]).unwrap();
    }

    #[test]
    fn a_folder_counts_everything_under_it_and_links_are_not_followed() {
        let d = tmp("walk");
        tree(&d);
        std::os::unix::fs::symlink(&d, d.join("a/loop")).unwrap();
        let mut partial = false;
        let never = AtomicBool::new(false);
        assert_eq!(folder_bytes(&d.join("a"), &[], &never, &mut partial), Some(123 + link_len(&d.join("a/loop"))));
        assert_eq!(folder_bytes(&d.join("e"), &[], &never, &mut partial), Some(0));
        assert!(!partial);
        let c = count(&[d.join("a"), d.join("e")], &[], &never).unwrap();
        assert_eq!(c.bytes.len(), 2);
        assert_eq!(c.bytes[1], 0);
    }

    fn link_len(p: &Path) -> u64 {
        std::fs::symlink_metadata(p).unwrap().len()
    }

    #[test]
    fn a_skipped_mount_inside_is_not_walked_and_marks_the_total() {
        let d = tmp("skip");
        tree(&d);
        let mut partial = false;
        let never = AtomicBool::new(false);
        // a/b stands for a cloud drive mounted inside a
        assert_eq!(folder_bytes(&d.join("a"), &[d.join("a/b")], &never, &mut partial), Some(100));
        assert!(partial);
        let c = count(&[d.join("e"), d.join("a")], &[d.join("a/b")], &never).unwrap();
        assert_eq!(c, Counted { bytes: vec![0, 100], partial: true });
    }

    #[test]
    fn a_cancelled_walk_gives_nothing() {
        let d = tmp("cancel");
        tree(&d);
        let stop = AtomicBool::new(true);
        let mut partial = false;
        assert_eq!(folder_bytes(&d, &[], &stop, &mut partial), None);
        assert_eq!(count(&[d.join("a")], &[], &stop), None);
    }

    #[test]
    fn the_counter_reports_and_a_dropped_one_stops() {
        let d = tmp("thread");
        tree(&d);
        let (tx, rx) = channel();
        let c = Counter::start(vec![d.join("a")], move || tx.send(()).unwrap());
        rx.recv_timeout(Duration::from_secs(10)).expect("woken when done");
        let got = c.poll().unwrap();
        assert_eq!(got.bytes, vec![123]);
        // dropped (the window went to another folder): its walk is told to stop (a_cancelled_walk_gives_nothing)
        let c = Counter::start(vec![d.join("a")], || {});
        let cancel = c.cancel.clone();
        drop(c);
        assert!(cancel.load(Ordering::Relaxed));
    }

    const MOUNTINFO: &str = "\
22 1 0:21 / / rw,relatime shared:1 - btrfs /dev/nvme0n1p2 rw\n\
40 22 0:40 / /home/u/ProtonDrive rw,nosuid shared:20 - fuse.rclone proton: rw,user_id=1000\n\
41 22 0:41 / /mnt/share rw shared:21 - cifs //server/x rw\n";

    #[test]
    fn folders_on_a_drive_that_may_stall_are_never_walked() {
        assert!(may_walk_in(MOUNTINFO, Path::new("/home/u/Documents")));
        assert!(!may_walk_in(MOUNTINFO, Path::new("/home/u/ProtonDrive")));
        assert!(!may_walk_in(MOUNTINFO, Path::new("/home/u/ProtonDrive/Photos")));
        assert!(!may_walk_in(MOUNTINFO, Path::new("/mnt/share")));
    }

    #[test]
    fn the_direct_sum_follows_what_is_shown() {
        let e = |name: &str, dir: bool, size: u64| Entry {
            name: name.into(),
            path: PathBuf::from(name),
            is_dir: dir,
            size,
            modified: None,
            created: None,
            hidden: name.starts_with('.'),
            usage: None,
        };
        let list = vec![e("docs", true, 0), e(".cache", true, 0), e("a.txt", false, 10), e(".b", false, 5)];
        assert_eq!(direct(&list, |_| true), Direct { items: 4, folders: 2, file_bytes: 15 });
        assert_eq!(direct(&list, |x| !x.hidden), Direct { items: 2, folders: 1, file_bytes: 10 });
    }
}
