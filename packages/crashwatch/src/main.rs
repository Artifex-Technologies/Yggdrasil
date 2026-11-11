//! yggdrasil-crashwatch: follows the folder systemd-coredump stores cores in, and your own journal for the crashes whose core
//! is not stored, and for each new crash of yours starts one notice (`crashwatch-notice EXE SIGNAL`, which exits when it is
//! done). One notice per crash, and at most one per program every ten minutes.
//!
//!   yggdrasil-crashwatch                      watch (what the user service runs)
//!   yggdrasil-crashwatch --dry-run            print "EXE crashed (SIGNAL)" instead of showing a notice
//!   yggdrasil-crashwatch --dir DIR            watch another folder for cores (tests)
//!   yggdrasil-crashwatch --journal-dir DIR    follow DIR/user-UID.journal instead of your own journal file (tests)
//!
//! `CRASHWATCH_NOTICE` names another notice program (tests). Idle, this is one blocking read on an inotify descriptor (the
//! core folder, the journal folder and the user's journal file): no threads, no timers, no D-Bus connection.

use std::ffi::CString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use yggdrasil_crashwatch::core::{crash_of, crash_of_entry, parse_core_name, read_xattr, Crash, CORE_DIR};
use yggdrasil_crashwatch::journal::{self, Journal};
use yggdrasil_crashwatch::quiet::{Quiet, Seen, QUIET_SECS};

const NOTICE: &str = "/usr/lib/yggdrasil/crashwatch-notice";
const MASK: u32 = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_ONLYDIR;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn add_watch(fd: i32, path: &Path) -> i32 {
    add_watch_for(fd, path, MASK)
}

fn add_watch_for(fd: i32, path: &Path, mask: u32) -> i32 {
    let Ok(p) = CString::new(path.as_os_str().as_bytes()) else { return -1 };
    // SAFETY: a valid descriptor and a NUL-terminated path
    unsafe { libc::inotify_add_watch(fd, p.as_ptr(), mask) }
}

fn rm_watch(fd: i32, wd: &mut i32) {
    if *wd >= 0 {
        // SAFETY: a watch this descriptor made
        unsafe { libc::inotify_rm_watch(fd, *wd) };
        *wd = -1;
    }
}

/// The user's own journal file (journald's user-UID.journal in the journal folder), for the crashes whose core is not stored:
/// a watch on the file wakes the watcher when journald writes to it, and a watch on the folder when journald starts a new one
/// (it renames the full one aside), which is then read from its first entry.
struct Follow {
    dir: PathBuf,
    name: String,
    uid: u32,
    dir_wd: i32,
    file_wd: i32,
    journal: Option<Journal>,
    /// tries left at opening a new file that would not open yet (journald may still be writing its header), one a second
    retries: u32,
}

impl Follow {
    /// Follows `dir`/user-UID.journal from now on; None (said once) when there is no such folder: the journal is kept in memory
    /// only, so there is no file of the user's own.
    fn start(fd: i32, dir: PathBuf, uid: u32) -> Option<Follow> {
        let dir_wd = add_watch(fd, &dir);
        if dir_wd < 0 {
            eprintln!("yggdrasil-crashwatch: no journal of your own in {}; only crashes whose core is stored are seen", dir.display());
            return None;
        }
        let mut f = Follow { name: journal::file_name(uid), dir, uid, dir_wd, file_wd: -1, journal: None, retries: 0 };
        if f.dir.join(&f.name).exists() && !f.open(fd, false) {
            eprintln!("yggdrasil-crashwatch: until journald starts a new one, only crashes whose core is stored are seen");
        }
        Some(f)
    }

    fn open(&mut self, fd: i32, from_start: bool) -> bool {
        let path = self.dir.join(&self.name);
        rm_watch(fd, &mut self.file_wd);
        // the watch first: nothing written between the two is slept through
        self.file_wd = add_watch_for(fd, &path, libc::IN_MODIFY | libc::IN_CLOSE_WRITE);
        match Journal::open(&path, self.uid, from_start) {
            Ok(j) => {
                self.journal = Some(j);
                self.retries = 0;
                true
            }
            Err(e) => {
                rm_watch(fd, &mut self.file_wd);
                if self.retries <= 1 {
                    eprintln!("yggdrasil-crashwatch: {e}");
                }
                self.retries = self.retries.saturating_sub(1);
                false
            }
        }
    }

    /// journald started a new file: what is left in the old one first, then the new one from its start.
    fn new_file(&mut self, fd: i32, w: &mut Watcher) {
        self.read(w);
        self.journal = None;
        self.retries = 10;
        self.open_new(fd, w);
    }

    /// Opens the new file from its start and reads what is in it already.
    fn open_new(&mut self, fd: i32, w: &mut Watcher) {
        if self.open(fd, true) {
            self.read(w);
        }
    }

    fn read(&mut self, w: &mut Watcher) {
        if let Some(j) = &mut self.journal {
            j.new_entries(|field| w.journal_entry(field));
        }
    }

    /// How long the wait may last: a second while a new file is still to be opened, else no limit.
    fn timeout_ms(&self) -> i32 {
        if self.journal.is_none() && self.retries > 0 {
            1000
        } else {
            -1
        }
    }
}

/// The inotify watch on the core folder; while the folder does not exist yet (systemd creates it at boot, or with the first
/// core), a watch on its parent for it to appear.
struct Watch {
    fd: i32,
    dir: PathBuf,
    dir_wd: i32,
    parent_wd: i32,
}

impl Watch {
    fn new(dir: PathBuf) -> Result<Watch, String> {
        // SAFETY: no pointers involved
        let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
        if fd < 0 {
            return Err(format!("inotify: {}", std::io::Error::last_os_error()));
        }
        let mut w = Watch { fd, dir, dir_wd: -1, parent_wd: -1 };
        w.arm()?;
        Ok(w)
    }

    fn arm(&mut self) -> Result<(), String> {
        for _ in 0..2 {
            let wd = add_watch(self.fd, &self.dir);
            if wd >= 0 {
                self.dir_wd = wd;
                if self.parent_wd >= 0 {
                    // SAFETY: a watch this descriptor made
                    unsafe { libc::inotify_rm_watch(self.fd, self.parent_wd) };
                    self.parent_wd = -1;
                }
                return Ok(());
            }
            if errno() != libc::ENOENT {
                return Err(format!("cannot watch {}: {}", self.dir.display(), std::io::Error::last_os_error()));
            }
            if self.parent_wd < 0 {
                let parent = self.dir.parent().unwrap_or(Path::new("/"));
                self.parent_wd = add_watch(self.fd, parent);
                if self.parent_wd < 0 {
                    return Err(format!("cannot watch {}: {}", parent.display(), std::io::Error::last_os_error()));
                }
            }
            // the folder may have appeared between the two calls: once more
        }
        Ok(())
    }
}

struct Watcher {
    uid: u32,
    quiet: Quiet,
    seen: Seen,
    dry_run: bool,
    notice: PathBuf,
    /// notices still running (each exits within about fifteen minutes); reaped as they end
    running: Vec<Child>,
}

impl Watcher {
    fn core_appeared(&mut self, dir: &Path, name: &str) {
        let Some(core) = parse_core_name(name) else { return };
        let path = dir.join(name);
        let Some(crash) = crash_of(&core, self.uid, |k| read_xattr(&path, k)) else { return };
        if self.seen.first(core.pid) {
            self.show(crash);
        }
    }

    /// A journal entry of systemd-coredump: a crash whose core was not stored, or the second word of one whose core was.
    fn journal_entry(&mut self, field: &dyn Fn(&str) -> Option<String>) {
        let Some((pid, crash)) = crash_of_entry(self.uid, field) else { return };
        if pid.is_none_or(|p| self.seen.first(p)) {
            self.show(crash);
        }
    }

    fn show(&mut self, Crash { exe, signal }: Crash) {
        if !self.quiet.allow(&exe, yggdrasil_crashwatch::boot_secs()) {
            return;
        }
        if self.dry_run {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{exe} crashed ({signal})");
            let _ = out.flush();
            return;
        }
        match Command::new(&self.notice).arg(&exe).arg(&signal).stdin(Stdio::null()).spawn() {
            Ok(child) => self.running.push(child),
            Err(e) => eprintln!("yggdrasil-crashwatch: cannot start {}: {e}", self.notice.display()),
        }
    }

    fn reap(&mut self) {
        self.running.retain_mut(|c| matches!(c.try_wait(), Ok(None)));
    }
}

fn main() {
    let mut dir = PathBuf::from(CORE_DIR);
    let mut journal_dir: Option<PathBuf> = None;
    let mut dry_run = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--dir" => dir = args.next().map(PathBuf::from).unwrap_or(dir),
            "--journal-dir" => journal_dir = args.next().map(PathBuf::from),
            "-h" | "--help" => {
                println!("usage: yggdrasil-crashwatch [--dry-run] [--dir DIR] [--journal-dir DIR]");
                return;
            }
            other => {
                eprintln!("yggdrasil-crashwatch: unknown argument {other}");
                std::process::exit(2);
            }
        }
    }
    let mut watch = match Watch::new(dir) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("yggdrasil-crashwatch: {e}");
            std::process::exit(1);
        }
    };
    // SAFETY: getuid cannot fail
    let uid = unsafe { libc::getuid() };
    // without a journal of the user's own (said once) the stored cores are still seen
    let mut follow = journal_dir.or_else(journal::default_dir).and_then(|d| Follow::start(watch.fd, d, uid));
    let mut w = Watcher {
        uid,
        quiet: Quiet::new(QUIET_SECS),
        seen: Seen::default(),
        dry_run,
        notice: std::env::var_os("CRASHWATCH_NOTICE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(NOTICE)),
        running: Vec::new(),
    };
    // inotify events: a 16-byte header (wd, mask, cookie, len) and len bytes of NUL-padded name
    let mut buf = [0u8; 4096];
    loop {
        let mut pfd = libc::pollfd { fd: watch.fd, events: libc::POLLIN, revents: 0 };
        // asleep until something happens; only while a notice is still running it wakes every few seconds to reap it (and once
        // a second while a new journal file is still to be opened)
        let mut timeout = if w.running.is_empty() { -1 } else { 5000 };
        if let Some(t) = follow.as_ref().map(Follow::timeout_ms).filter(|t| *t >= 0) {
            timeout = if timeout < 0 { t } else { timeout.min(t) };
        }
        // SAFETY: one pollfd
        unsafe { libc::poll(&mut pfd, 1, timeout) };
        let (mut journal_written, mut new_journal) = (false, false);
        loop {
            // SAFETY: reads at most buf.len() bytes into buf
            let n = unsafe { libc::read(watch.fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n <= 0 {
                break;
            }
            let n = n as usize;
            let mut off = 0;
            while off + 16 <= n {
                let int = |i: usize| u32::from_ne_bytes(buf[off + i..off + i + 4].try_into().unwrap());
                let (wd, mask, len) = (int(0) as i32, int(4), int(12) as usize);
                let end = (off + 16 + len).min(n);
                let raw = &buf[off + 16..end];
                let name = String::from_utf8_lossy(&raw[..raw.iter().position(|&b| b == 0).unwrap_or(raw.len())]).into_owned();
                off = end;
                if mask & libc::IN_Q_OVERFLOW != 0 {
                    eprintln!("yggdrasil-crashwatch: too many files at once; some crashes may have no notice");
                    journal_written = true;
                } else if let Some(f) = follow.as_mut().filter(|f| wd >= 0 && (wd == f.file_wd || wd == f.dir_wd)) {
                    if wd == f.file_wd && mask & libc::IN_IGNORED != 0 {
                        // the file is gone (journald removed it): the folder's watch sees the next one
                        f.file_wd = -1;
                    } else if wd == f.file_wd {
                        journal_written = true;
                    } else if mask & libc::IN_IGNORED != 0 {
                        eprintln!("yggdrasil-crashwatch: the journal folder is gone; only crashes whose core is stored are seen");
                        follow = None;
                    } else if name == f.name {
                        new_journal = true;
                    }
                } else if wd == watch.dir_wd && mask & libc::IN_IGNORED != 0 {
                    // the folder was removed: wait for it to come back
                    watch.dir_wd = -1;
                    if let Err(e) = watch.arm() {
                        eprintln!("yggdrasil-crashwatch: {e}");
                        std::process::exit(1);
                    }
                } else if wd == watch.dir_wd && mask & libc::IN_ISDIR == 0 && !name.is_empty() {
                    let dir = watch.dir.clone();
                    w.core_appeared(&dir, &name);
                } else if wd == watch.parent_wd && watch.dir.file_name().is_some_and(|f| f.as_bytes() == name.as_bytes()) {
                    if let Err(e) = watch.arm() {
                        eprintln!("yggdrasil-crashwatch: {e}");
                        std::process::exit(1);
                    }
                }
            }
        }
        // after the cores: a stored core's journal entry then finds its crash already handled
        if let Some(f) = &mut follow {
            if new_journal {
                f.new_file(watch.fd, &mut w);
            } else if f.journal.is_none() && f.retries > 0 {
                f.open_new(watch.fd, &mut w);
            } else if journal_written {
                f.read(&mut w);
            }
        }
        w.reap();
    }
}
