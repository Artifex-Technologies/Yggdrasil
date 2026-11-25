//! The config files the compositor's settings come from (`App::reload_config`): which they are, what tells that one changed,
//! the watch that wakes the event loop when one does, and at no other time, and the wait between a change and the reading.
//! (Until 2026-11-25 their times were looked at every 2 seconds: a wake-up and a pass over the screens each time, YGG-118.)
//!
//! The watch is inotify on their folders, not on the files: Settings and most editors save a file by writing a new one and
//! renaming it over the old, which a watch on the old file would miss. A folder that is not there yet (no ~/.config/gtk-3.0,
//! no /etc/X11/xorg.conf.d) is waited for: the nearest folder above it that is there is watched, and the watch moves down as
//! the folders are made (and back up when one is removed). A file that is a link (into a dotfiles folder) is also watched
//! where it leads. Only what happens to these files, and to the folders on the way to them, counts: anything else in the same
//! folders (another file in ~/.config/yggdrasil, the rest of /etc) is read from the watch and dropped. inotify hears what this
//! machine does to the files: one changed from another machine in a home on a network file system (NFS) is read at the next
//! `reload`, or with the next change made here.
//!
//! A change is not read the moment it is seen: the files are read once the changes have rested for `SETTLE` (`Wait`,
//! `App::config_changed`), so a save in steps is read once, when it is finished. Vim and Emacs save by renaming the file to a
//! backup, then making, writing and closing a new one; read in between, the file is missing or half written (YGG-138:
//! monitors.lua read while it was missing set the screens to their own size and pulled the pointer onto them, then set them
//! back). A file that had something in it when it was last read and is missing or empty when the wait ends is waited for once
//! more, as a save in steps passes through both: back by then, it is read as it is; still missing or empty, it is read so. So
//! a file removed for good is in force a moment later, as any other change, and not only on `reload`: removing a file to go
//! back to the defaults is something people do. `reload` reads at once. The timer runs only from a change until the reading.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};

use crate::state::App;

/// How long the config files must rest after a change before they are read: a tenth of a second. The steps of a save leave
/// the file missing or half written for well under a millisecond, and for about a millisecond on a busy machine (the stall
/// that showed it, YGG-138); a hundred times that is still as good as at once to someone who saves a file and looks, and
/// Settings does not wait for it: it asks for `reload` once it has written a file.
pub const SETTLE: Duration = Duration::from_millis(100);

/// The wait between the watch seeing a change and the files being read: one timer for a burst of changes, started by the
/// first and ending `SETTLE` after the last.
#[derive(Default)]
pub struct Wait {
    /// when the watch last saw a change; None while no wait runs
    last: Option<Instant>,
    /// the wait was held once for a file that went (`rested`)
    held: bool,
    /// the files that had something in them when they were last read (`read`)
    full: Vec<PathBuf>,
}

impl Wait {
    /// The watch saw a change at `now`: true when no wait runs yet (one is to be started), else the one running ends `SETTLE`
    /// after it.
    pub fn changed(&mut self, now: Instant) -> bool {
        self.held = false;
        self.last.replace(now).is_none()
    }

    /// The wait's time came (at `now`): how much longer it is to run, or None when the files are to be read now (the wait is
    /// over). One more `SETTLE` when a file that had something in it when last read is missing or empty, once.
    pub fn rested(&mut self, now: Instant) -> Option<Duration> {
        let since = self.last.map_or(SETTLE, |t| now.saturating_duration_since(t));
        if since < SETTLE {
            return Some(SETTLE - since);
        }
        if !self.held && self.full.iter().any(|f| !has_something(f)) {
            self.held = true;
            self.last = Some(now);
            return Some(SETTLE);
        }
        self.last = None;
        None
    }

    /// The files are being read (`App::reload_config`): which of them have something in them.
    pub fn read(&mut self, files: Vec<PathBuf>) {
        self.full = files.into_iter().filter(|f| has_something(f)).collect();
    }
}

/// A file with something in it (where it leads, for a link).
fn has_something(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.len() > 0)
}

impl App {
    /// The watch saw one of the config files change: they are read once the changes have rested (`Wait`).
    pub fn config_changed(&mut self) {
        if !self.config_wait.changed(Instant::now()) {
            return;
        }
        let timer =
            self.loop_handle.insert_source(Timer::from_duration(SETTLE), |_, _, app| match app.config_wait.rested(Instant::now()) {
                Some(more) => TimeoutAction::ToDuration(more),
                None => {
                    app.reload_config();
                    TimeoutAction::Drop
                }
            });
        if timer.is_err() {
            // no timer: read at once (reload_config notes the files again)
            self.config_wait = Wait::default();
            self.reload_config();
        }
    }
}

/// Every file `App::reload_config` reads. A file read there and missing here would be read again only on `reload`.
pub fn files() -> Vec<PathBuf> {
    let [x11_keyboard, vconsole] = crate::keymap::files();
    let mut files = vec![
        crate::tiling::config_path(),
        crate::shortcuts::config_path(),
        crate::xwayland::gtk_ini_path(),
        crate::xwayland::opaque_path(),
        crate::xwayland::xwayland_conf_path(),
        x11_keyboard,
        vconsole,
        yggdrasil_wm::input::InputConf::path(),
        yggdrasil_conf::tiling::path(),
        yggdrasil_conf::display::path(),
        crate::outputs::monitors_path(),
    ];
    // appearance.conf and panel.conf
    files.extend(crate::look::Look::files().into_iter().flatten());
    files
}

/// What tells that a file changed since it was read: its time, size and inode; None when there is no such file. The time
/// alone is not enough once files are read the moment they change: a file read while it was being written and the same file
/// finished a moment later can carry the same time (the kernel's clock for files moves in steps of a few milliseconds), but
/// not the same size; a file saved by renaming a new one over it has another inode.
pub type Stamp = Option<(SystemTime, u64, u64)>;

pub fn stamp(path: impl AsRef<Path>) -> Stamp {
    let m = std::fs::metadata(path).ok()?;
    Some((m.modified().ok()?, m.len(), m.ino()))
}

/// What a folder is watched for: a file in it written and closed, renamed in or out, made, removed, or its time set (touch);
/// the folder itself removed or renamed. Only folders are watched.
const MASK: WatchFlags = WatchFlags::CLOSE_WRITE
    .union(WatchFlags::MOVED_TO)
    .union(WatchFlags::MOVED_FROM)
    .union(WatchFlags::CREATE)
    .union(WatchFlags::DELETE)
    .union(WatchFlags::ATTRIB)
    .union(WatchFlags::DELETE_SELF)
    .union(WatchFlags::MOVE_SELF)
    .union(WatchFlags::ONLYDIR);

/// The watch on the config files' folders: an inotify descriptor the event loop waits on (main.rs, `serve`).
pub struct Watch {
    fd: OwnedFd,
    files: Vec<PathBuf>,
    /// The folders watched, by their watch: the folder, and the names in it that count (a file's, or the next folder's on the
    /// way to a folder that is not there yet).
    dirs: BTreeMap<i32, (PathBuf, Vec<OsString>)>,
}

impl Watch {
    pub fn new(files: Vec<PathBuf>) -> std::io::Result<Watch> {
        let fd = inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK)?;
        // a file named from the working folder: from where that is now
        let files = files.into_iter().map(|f| std::path::absolute(&f).unwrap_or(f)).collect();
        let mut w = Watch { fd, files, dirs: BTreeMap::new() };
        w.aim()?;
        Ok(w)
    }

    /// Takes what the watch saw (it does not block): true when one of the files may have changed (written, replaced, made,
    /// removed, or a folder on the way to it made or gone). The watch follows folders made or removed meanwhile, and links
    /// that lead elsewhere now.
    pub fn changed(&mut self) -> bool {
        let mut hit = false;
        {
            let mut buf = [MaybeUninit::<u8>::uninit(); 4096];
            let mut events = inotify::Reader::new(&self.fd, &mut buf);
            loop {
                let ev = match events.next() {
                    Ok(ev) => ev,
                    Err(Errno::INTR) => continue,
                    // nothing more for now
                    Err(_) => break,
                };
                let what = ev.events();
                // the queue was full and events were lost: any of the files may have changed
                if what.contains(ReadFlags::QUEUE_OVERFLOW) {
                    hit = true;
                    continue;
                }
                // (a watch taken off since: what it still had to say is of no interest)
                let Some((dir, names)) = self.dirs.get(&ev.wd()) else { continue };
                // the folder itself went (removed, renamed, its file system unmounted), and its files with it
                if what.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF | ReadFlags::IGNORED | ReadFlags::UNMOUNT) {
                    hit = true;
                    continue;
                }
                let Some(name) = ev.file_name().map(|n| OsStr::from_bytes(n.to_bytes())) else { continue };
                if !names.iter().any(|n| n == name) {
                    continue;
                }
                // a plain file just made and still empty is being written: it counts once it is closed (one that came whole, a
                // hard link or a finished file linked in, counts at once, as a link or a folder does)
                let being_written = || std::fs::symlink_metadata(dir.join(name)).is_ok_and(|m| m.is_file() && m.len() == 0);
                if what.contains(ReadFlags::CREATE) && being_written() {
                    continue;
                }
                hit = true;
            }
        }
        if hit {
            let _ = self.aim();
        }
        hit
    }

    /// Watch the folder of each file and of where a link leads, or, while a folder is not there, the nearest folder above it
    /// for the next folder on the way; take off the watches no longer needed.
    fn aim(&mut self) -> std::io::Result<()> {
        let mut err = None;
        // a few rounds: a folder on the way made while this looked told so before it was watched
        for _ in 0..4 {
            let mut dirs: BTreeMap<i32, (PathBuf, Vec<OsString>)> = BTreeMap::new();
            let mut awaited = vec![];
            for path in self.files.iter().flat_map(|f| with_links(f)) {
                let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else { continue };
                match self.watch_nearest(dir, name) {
                    Ok((wd, at, name)) => {
                        if at != dir {
                            awaited.push(at.join(&name));
                        }
                        let (_, names) = dirs.entry(wd).or_insert_with(|| (at, vec![]));
                        if !names.contains(&name) {
                            names.push(name);
                        }
                    }
                    Err(e) => {
                        err.get_or_insert(e);
                    }
                }
            }
            for wd in self.dirs.keys().filter(|wd| !dirs.contains_key(wd)) {
                let _ = inotify::remove_watch(&self.fd, *wd);
            }
            self.dirs = dirs;
            if !awaited.iter().any(|d| d.is_dir()) {
                break;
            }
        }
        match err {
            Some(e) if self.dirs.is_empty() => Err(e),
            _ => Ok(()),
        }
    }

    /// Watch `dir` for `name`; while `dir` is not there (or is not a folder), the nearest folder above it, for the next folder
    /// on the way. The watch, the folder it is on and the name that counts there.
    fn watch_nearest(&self, dir: &Path, name: &OsStr) -> std::io::Result<(i32, PathBuf, OsString)> {
        let (mut dir, mut name) = (dir, name);
        loop {
            match inotify::add_watch(&self.fd, dir, MASK) {
                Ok(wd) => return Ok((wd, dir.to_path_buf(), name.to_os_string())),
                Err(Errno::NOENT | Errno::NOTDIR) => match (dir.parent(), dir.file_name()) {
                    (Some(up), Some(next)) => (dir, name) = (up, next),
                    _ => return Err(Errno::NOENT.into()),
                },
                Err(e) => return Err(e.into()),
            }
        }
    }
}

impl AsFd for Watch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

/// A file and, when it is a link, where it leads (a link to a link too, a few deep).
fn with_links(file: &Path) -> Vec<PathBuf> {
    let mut out = vec![file.to_path_buf()];
    let mut at = file.to_path_buf();
    for _ in 0..8 {
        let Ok(to) = std::fs::read_link(&at) else { break };
        // a relative link leads from its own folder
        at = at.parent().map(|d| d.join(&to)).unwrap_or(to);
        out.push(at.clone());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ygg-configs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// The folder `dir` is watched for `name` (the kernel queues a watch's events within the call that makes them, so a test
    /// needs no waiting).
    fn watches(w: &Watch, dir: &Path, name: &str) -> bool {
        w.dirs.values().any(|(d, names)| d == dir && names.iter().any(|n| n == name))
    }

    #[test]
    fn a_stamp_tells_a_file_finished_or_replaced_within_the_same_moment() {
        let d = scratch("stamp");
        let f = d.join("a.conf");
        assert_eq!(stamp(&f), None, "no file");
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_800_000_000);
        let write = |path: &Path, text: &str| {
            std::fs::write(path, text).unwrap();
            std::fs::File::options().write(true).open(path).unwrap().set_modified(when).unwrap();
        };
        write(&f, "");
        let empty = stamp(&f);
        write(&f, "style = dark\n");
        let full = stamp(&f);
        assert!(empty.is_some() && full != empty, "read before it was written, then finished: the same time, another size");
        write(&d.join("a.conf.new"), "style = dark\n");
        std::fs::rename(d.join("a.conf.new"), &f).unwrap();
        assert_ne!(stamp(&f), full, "renamed over it: the same time and size, another file");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_burst_of_changes_is_read_once_it_has_rested() {
        let ms = Duration::from_millis;
        let t = Instant::now();
        let mut w = Wait::default();
        assert!(w.changed(t), "the first change starts the wait");
        assert!(!w.changed(t + ms(30)), "the next ones move its end");
        assert_eq!(w.rested(t + ms(100)), Some(ms(30)), "SETTLE after the last change");
        assert_eq!(w.rested(t + ms(130)), None, "then the files are read");
        assert!(w.changed(t + ms(500)), "the wait is over: the next change starts another");
    }

    #[test]
    fn a_file_missing_or_empty_when_the_wait_ends_is_waited_for_once_more() {
        let ms = Duration::from_millis;
        let d = scratch("wait");
        let (f, backup, absent) = (d.join("monitors.lua"), d.join("monitors.lua~"), d.join("input.conf"));
        let text = "hl.monitor({ output = \"DP-1\", scale = 2 })\n";
        std::fs::write(&f, text).unwrap();
        let mut w = Wait::default();
        let files = || vec![f.clone(), absent.clone()];
        w.read(files());
        let t = Instant::now();
        // a save in steps whose wait ends while the file is renamed to its backup: once more, and read once it is back
        std::fs::rename(&f, &backup).unwrap();
        w.changed(t);
        assert_eq!(w.rested(t + ms(100)), Some(SETTLE), "missing");
        std::fs::write(&f, text).unwrap();
        w.changed(t + ms(150));
        assert_eq!(w.rested(t + ms(200)), Some(ms(50)), "the new file closed: SETTLE after that");
        assert_eq!(w.rested(t + ms(250)), None, "back: read");
        w.read(files());
        // ... or while the new file is made and still empty
        std::fs::rename(&f, &backup).unwrap();
        std::fs::File::create(&f).unwrap();
        w.changed(t + ms(300));
        assert_eq!(w.rested(t + ms(400)), Some(SETTLE), "empty");
        std::fs::write(&f, text).unwrap();
        w.changed(t + ms(410));
        assert_eq!(w.rested(t + ms(510)), None, "written: read");
        w.read(files());
        // removed for good: read as missing at the end of the second wait, once
        std::fs::remove_file(&f).unwrap();
        w.changed(t + ms(600));
        assert_eq!(w.rested(t + ms(700)), Some(SETTLE));
        assert_eq!(w.rested(t + ms(800)), None, "still missing: read so");
        w.read(files());
        // files that had nothing in them when last read and still have not: no reason to wait longer (another file changed)
        w.changed(t + ms(900));
        assert_eq!(w.rested(t + ms(1000)), None);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_written_renamed_over_or_removed_is_seen_and_nothing_else() {
        let d = scratch("files");
        let f = d.join("a.conf");
        let mut w = Watch::new(vec![f.clone()]).unwrap();
        assert!(!w.changed(), "nothing yet");
        // made and written in place: once it is closed (half written, it is not read)
        let mut file = std::fs::File::create(&f).unwrap();
        assert!(!w.changed(), "made, not yet written");
        std::io::Write::write_all(&mut file, b"style = dark\n").unwrap();
        drop(file);
        assert!(w.changed(), "written and closed");
        assert!(!w.changed(), "taken");
        // another file in the same folder: dropped
        std::fs::write(d.join("weather.json"), "{}").unwrap();
        assert!(!w.changed(), "another file");
        // saved as Settings and editors save: a new file renamed over it (the new file itself is of no interest)
        std::fs::write(d.join(".a.conf.tmp"), "style = light\n").unwrap();
        assert!(!w.changed(), "the new file, not yet renamed");
        std::fs::rename(d.join(".a.conf.tmp"), &f).unwrap();
        assert!(w.changed(), "renamed over it");
        // touched, then removed
        std::fs::File::options().write(true).open(&f).unwrap().set_modified(SystemTime::now()).unwrap();
        assert!(w.changed(), "its time set");
        std::fs::remove_file(&f).unwrap();
        assert!(w.changed(), "removed");
        // made whole at once (a hard link; a finished temporary file linked in is the same): no close follows
        std::fs::write(d.join("whole"), "style = dark\n").unwrap();
        assert!(!w.changed());
        std::fs::hard_link(d.join("whole"), &f).unwrap();
        assert!(w.changed(), "linked in whole");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn folders_not_there_yet_are_waited_for_and_again_once_removed() {
        let d = scratch("later");
        let folder = d.join("etc/X11/xorg.conf.d");
        let f = folder.join("00-keyboard.conf");
        let mut w = Watch::new(vec![f.clone()]).unwrap();
        assert!(watches(&w, &d, "etc"), "the nearest folder that is there, for the next one on the way");
        assert!(!w.changed());
        // the folders made at once (as create_dir_all, mkdir -p): the watch goes down to the file's folder
        std::fs::create_dir_all(&folder).unwrap();
        assert!(w.changed(), "a folder on the way made (a file may be in it already)");
        assert!(watches(&w, &folder, "00-keyboard.conf"), "{:?}", w.dirs);
        assert_eq!(w.dirs.len(), 1, "the folders above are no longer watched: {:?}", w.dirs);
        std::fs::write(&f, "Section \"InputClass\"\nEndSection\n").unwrap();
        assert!(w.changed(), "the file written");
        // one folder at a time (as an installer making them one by one)
        std::fs::remove_dir_all(d.join("etc")).unwrap();
        assert!(w.changed(), "the folder gone, and the file with it");
        assert!(watches(&w, &d, "etc"), "waited for again: {:?}", w.dirs);
        std::fs::create_dir(d.join("etc")).unwrap();
        assert!(w.changed());
        assert!(watches(&w, &d.join("etc"), "X11"), "{:?}", w.dirs);
        std::fs::create_dir(d.join("etc/X11")).unwrap();
        assert!(w.changed());
        std::fs::create_dir(&folder).unwrap();
        assert!(w.changed());
        assert!(watches(&w, &folder, "00-keyboard.conf"), "{:?}", w.dirs);
        // a folder made by renaming one in
        std::fs::rename(&folder, d.join("etc/X11/elsewhere")).unwrap();
        assert!(w.changed(), "renamed away");
        assert!(watches(&w, &d.join("etc/X11"), "xorg.conf.d"), "{:?}", w.dirs);
        std::fs::write(d.join("etc/X11/elsewhere/00-keyboard.conf"), "").unwrap();
        assert!(!w.changed(), "the folder renamed away is no longer watched");
        std::fs::rename(d.join("etc/X11/elsewhere"), &folder).unwrap();
        assert!(w.changed(), "renamed back in, its file with it");
        assert!(watches(&w, &folder, "00-keyboard.conf"), "{:?}", w.dirs);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_link_is_followed_to_the_file_it_leads_to() {
        let d = scratch("link");
        let (conf, dots, other) = (d.join("conf"), d.join("dots"), d.join("other"));
        for f in [&conf, &dots, &other] {
            std::fs::create_dir_all(f).unwrap();
        }
        std::fs::write(dots.join("shortcuts.conf"), "").unwrap();
        std::fs::write(other.join("shortcuts.conf"), "").unwrap();
        // a dotfiles folder's file, linked in by a relative link
        std::os::unix::fs::symlink("../dots/shortcuts.conf", conf.join("shortcuts.conf")).unwrap();
        let mut w = Watch::new(vec![conf.join("shortcuts.conf")]).unwrap();
        std::fs::write(dots.join("shortcuts.conf"), "Super+E = exec minos\n").unwrap();
        assert!(w.changed(), "the file the link leads to, written");
        std::fs::write(dots.join("README"), "").unwrap();
        assert!(!w.changed(), "another file there");
        // the link made to lead elsewhere (ln -sf: a new link renamed over it)
        std::os::unix::fs::symlink(other.join("shortcuts.conf"), conf.join(".shortcuts.conf.new")).unwrap();
        std::fs::rename(conf.join(".shortcuts.conf.new"), conf.join("shortcuts.conf")).unwrap();
        assert!(w.changed(), "the link replaced");
        std::fs::write(dots.join("shortcuts.conf"), "").unwrap();
        assert!(!w.changed(), "the file it led to before");
        std::fs::write(other.join("shortcuts.conf"), "Super+E = exec minos\n").unwrap();
        assert!(w.changed(), "the file it leads to now");
        // a link made where there was no file: whole at once
        std::fs::remove_file(conf.join("shortcuts.conf")).unwrap();
        assert!(w.changed());
        std::os::unix::fs::symlink(dots.join("shortcuts.conf"), conf.join("shortcuts.conf")).unwrap();
        assert!(w.changed(), "a link made");
        let _ = std::fs::remove_dir_all(&d);
    }
}
