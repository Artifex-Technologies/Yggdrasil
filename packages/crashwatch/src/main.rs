//! yggdrasil-crashwatch: follows the folder systemd-coredump stores cores in, and for each new core of yours starts one notice
//! (`crashwatch-notice EXE SIGNAL`, which exits when it is done). At most one notice per program every ten minutes.
//!
//!   yggdrasil-crashwatch              watch (what the user service runs)
//!   yggdrasil-crashwatch --dry-run    print "EXE crashed (SIGNAL)" instead of showing a notice
//!   yggdrasil-crashwatch --dir DIR    watch another folder (tests)
//!
//! `CRASHWATCH_NOTICE` names another notice program (tests). Idle, this is one blocking read on an inotify descriptor: no
//! threads, no timers, no D-Bus connection.

use std::ffi::CString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use yggdrasil_crashwatch::core::{crash_of, parse_core_name, read_xattr, Crash, CORE_DIR};
use yggdrasil_crashwatch::quiet::{Quiet, QUIET_SECS};

const NOTICE: &str = "/usr/lib/yggdrasil/crashwatch-notice";
const MASK: u32 = libc::IN_CREATE | libc::IN_MOVED_TO | libc::IN_ONLYDIR;

fn errno() -> i32 {
    std::io::Error::last_os_error().raw_os_error().unwrap_or(0)
}

fn add_watch(fd: i32, path: &Path) -> i32 {
    let Ok(p) = CString::new(path.as_os_str().as_bytes()) else { return -1 };
    // SAFETY: a valid descriptor and a NUL-terminated path
    unsafe { libc::inotify_add_watch(fd, p.as_ptr(), MASK) }
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
    dry_run: bool,
    notice: PathBuf,
    /// notices still running (each exits within about fifteen minutes); reaped as they end
    running: Vec<Child>,
}

impl Watcher {
    fn core_appeared(&mut self, dir: &Path, name: &str) {
        let Some(core) = parse_core_name(name) else { return };
        let path = dir.join(name);
        let Some(Crash { exe, signal }) = crash_of(&core, self.uid, |k| read_xattr(&path, k)) else { return };
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
    let mut dry_run = false;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dry-run" => dry_run = true,
            "--dir" => dir = args.next().map(PathBuf::from).unwrap_or(dir),
            "-h" | "--help" => {
                println!("usage: yggdrasil-crashwatch [--dry-run] [--dir DIR]");
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
    let mut w = Watcher {
        // SAFETY: getuid cannot fail
        uid: unsafe { libc::getuid() },
        quiet: Quiet::new(QUIET_SECS),
        dry_run,
        notice: std::env::var_os("CRASHWATCH_NOTICE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(NOTICE)),
        running: Vec::new(),
    };
    // inotify events: a 16-byte header (wd, mask, cookie, len) and len bytes of NUL-padded name
    let mut buf = [0u8; 4096];
    loop {
        let mut pfd = libc::pollfd { fd: watch.fd, events: libc::POLLIN, revents: 0 };
        // asleep until something happens; only while a notice is still running it wakes every few seconds to reap it
        let timeout = if w.running.is_empty() { -1 } else { 5000 };
        // SAFETY: one pollfd
        unsafe { libc::poll(&mut pfd, 1, timeout) };
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
        w.reap();
    }
}
