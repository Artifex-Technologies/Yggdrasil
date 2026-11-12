//! The service: which scopes run (the user's cgroup tree, followed with inotify), which app each is, how long each app has run,
//! and when that is written down.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::CString;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{add_seconds, desktop_ids, match_unit, running_scopes, usage_path, INTERVAL};

/// How often the time of apps that keep running is written down (the script wrote every 30 s; Settings reads the file when
/// its Apps page opens). LABYRINTH_APPUSAGE_FLUSH=SECONDS changes it (tests).
pub const FLUSH: Duration = Duration::from_secs(300);
/// The desktop entries are read again when a scope starts and they are older than this (the script: every ten minutes) ...
pub const IDS_MAX_AGE: Duration = Duration::from_secs(600);
/// ... or older than this and the new scope matched none of them (an app installed a moment ago).
pub const IDS_RETRY_AGE: Duration = Duration::from_secs(60);

/// How long each app has run: the scopes seen, the apps with at least one, and the time not yet written down.
pub struct Tracker {
    pub ids: BTreeSet<String>,
    ids_at: Instant,
    /// scope name -> the app it belongs to
    scopes: BTreeMap<String, Option<String>>,
    /// app -> (its running scopes, since when it is counted)
    running: BTreeMap<String, (usize, Instant)>,
    /// app -> seconds not yet written (with the fraction of a second kept)
    pending: BTreeMap<String, f64>,
}

impl Tracker {
    pub fn new(ids: BTreeSet<String>, now: Instant) -> Tracker {
        Tracker { ids, ids_at: now, scopes: BTreeMap::new(), running: BTreeMap::new(), pending: BTreeMap::new() }
    }

    /// Whether any app is running (then the time is written down every FLUSH).
    pub fn any_running(&self) -> bool {
        !self.running.is_empty()
    }

    /// The scopes that run now. Returns whether an app stopped (its time is then written down at once).
    pub fn update(&mut self, now_scopes: &BTreeSet<String>, now: Instant, ids: &mut dyn FnMut() -> BTreeSet<String>) -> bool {
        let gone: Vec<String> = self.scopes.keys().filter(|s| !now_scopes.contains(*s)).cloned().collect();
        let new: Vec<&String> = now_scopes.iter().filter(|s| !self.scopes.contains_key(*s)).collect();
        if !new.is_empty() {
            let age = now.saturating_duration_since(self.ids_at);
            let unmatched = new.iter().any(|s| match_unit(s, &self.ids).is_none());
            if age > IDS_MAX_AGE || (unmatched && age > IDS_RETRY_AGE) {
                self.ids = ids();
                self.ids_at = now;
                // what every scope is may have changed with the entries
                let all: Vec<String> = self.scopes.keys().cloned().collect();
                for s in all {
                    let app = match_unit(&s, &self.ids);
                    if self.scopes.get(&s) != Some(&app) {
                        self.stop(&s, now);
                        self.start(&s, app, now);
                    }
                }
            }
        }
        let mut stopped = false;
        for s in gone {
            stopped |= self.stop(&s, now);
        }
        for s in new {
            let app = match_unit(s, &self.ids);
            self.start(s, app, now);
        }
        stopped
    }

    fn start(&mut self, scope: &str, app: Option<String>, now: Instant) {
        if let Some(a) = &app {
            let e = self.running.entry(a.clone()).or_insert((0, now));
            e.0 += 1;
        }
        self.scopes.insert(scope.to_string(), app);
    }

    /// Returns whether the scope's app stopped running with it.
    fn stop(&mut self, scope: &str, now: Instant) -> bool {
        let Some(Some(app)) = self.scopes.remove(scope) else { return false };
        let Some(e) = self.running.get_mut(&app) else { return false };
        e.0 -= 1;
        if e.0 > 0 {
            return false;
        }
        let since = e.1;
        self.running.remove(&app);
        *self.pending.entry(app).or_insert(0.0) += now.saturating_duration_since(since).as_secs_f64();
        true
    }

    /// The whole seconds to write down now (apps still running are counted up to now); the fractions stay for next time.
    pub fn take(&mut self, now: Instant) -> BTreeMap<String, i64> {
        for (app, (_, since)) in self.running.iter_mut() {
            *self.pending.entry(app.clone()).or_insert(0.0) += now.saturating_duration_since(*since).as_secs_f64();
            *since = now;
        }
        let mut out = BTreeMap::new();
        for (app, secs) in self.pending.iter_mut() {
            let whole = secs.floor();
            if whole >= 1.0 {
                out.insert(app.clone(), whole as i64);
                *secs -= whole;
            }
        }
        out
    }

    /// Puts seconds that could not be written back, to be written next time.
    pub fn give_back(&mut self, secs: BTreeMap<String, i64>) {
        for (app, s) in secs {
            *self.pending.entry(app).or_insert(0.0) += s as f64;
        }
    }
}

/// This user's systemd manager in the cgroup tree: /sys/fs/cgroup/user.slice/user-UID.slice/user@UID.service, taken from this
/// process's own cgroup when it runs under it (the service does), else from the user id.
pub fn cgroup_root() -> Option<PathBuf> {
    let base = Path::new("/sys/fs/cgroup");
    if let Ok(text) = fs::read_to_string("/proc/self/cgroup") {
        for line in text.lines() {
            if let Some(path) = line.strip_prefix("0::") {
                let mut acc = base.to_path_buf();
                for part in path.split('/').filter(|p| !p.is_empty()) {
                    acc.push(part);
                    if part.starts_with("user@") && part.ends_with(".service") {
                        return acc.is_dir().then_some(acc);
                    }
                }
            }
        }
    }
    let uid = unsafe { libc::getuid() };
    let p = base.join(format!("user.slice/user-{uid}.slice/user@{uid}.service"));
    p.is_dir().then_some(p)
}

/// Every scope under the manager (a folder NAME.scope, in the manager's own folder or in a slice, at any depth of slices),
/// watching each slice folder for scopes made and removed.
pub fn scan(root: &Path, mut watch: impl FnMut(&Path)) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        // watched before it is read, so a scope made in between is not missed
        watch(&dir);
        let Ok(rd) = fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name();
            let Some(n) = name.to_str() else { continue };
            if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            if n.ends_with(".scope") {
                out.insert(n.to_string());
            } else if n.ends_with(".slice") {
                todo.push(e.path());
            }
        }
    }
    out
}

/// inotify on the cgroup folders.
pub struct Inotify {
    fd: i32,
}

impl Inotify {
    pub fn new() -> std::io::Result<Inotify> {
        let fd = unsafe { libc::inotify_init1(libc::IN_NONBLOCK | libc::IN_CLOEXEC) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Inotify { fd })
    }

    /// Folders made, removed or moved in `dir` (adding a folder already watched does nothing new).
    pub fn watch(&self, dir: &Path) {
        if let Ok(c) = CString::new(dir.as_os_str().as_bytes()) {
            unsafe {
                libc::inotify_add_watch(
                    self.fd,
                    c.as_ptr(),
                    libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_FROM | libc::IN_MOVED_TO | libc::IN_ONLYDIR,
                )
            };
        }
    }

    /// Reads every event waiting (what they say does not matter: the tree is read again).
    pub fn drain(&self) {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { libc::read(self.fd, buf.as_mut_ptr() as *mut libc::c_void, buf.len()) };
            if n <= 0 {
                break;
            }
        }
    }
}

impl Drop for Inotify {
    fn drop(&mut self) {
        unsafe { libc::close(self.fd) };
    }
}

/// SIGTERM, SIGINT and SIGHUP as a file descriptor, so a stop writes the time down before the process ends.
fn signal_fd() -> i32 {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for s in [libc::SIGTERM, libc::SIGINT, libc::SIGHUP] {
            libc::sigaddset(&mut set, s);
        }
        libc::sigprocmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        libc::signalfd(-1, &set, libc::SFD_CLOEXEC | libc::SFD_NONBLOCK)
    }
}

fn flush_interval() -> Duration {
    std::env::var("LABYRINTH_APPUSAGE_FLUSH")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
        .filter(|s| *s > 0.0)
        .map(Duration::from_secs_f64)
        .unwrap_or(FLUSH)
}

fn write_down(t: &mut Tracker, path: &Path, now: Instant) {
    let secs = t.take(now);
    if secs.is_empty() {
        return;
    }
    if let Err(e) = add_seconds(path, &secs) {
        eprintln!("labyrinth-appusage: cannot write {}: {e}", path.display());
        t.give_back(secs);
    }
}

/// The service: runs until it is stopped.
pub fn run() -> i32 {
    let path = usage_path();
    let flush = flush_interval();
    let sig = signal_fd();
    let root = cgroup_root();
    let ino = root.as_ref().and_then(|_| Inotify::new().ok());
    let mut t = Tracker::new(desktop_ids(), Instant::now());
    let mut last_flush = Instant::now();
    let read_scopes = |ino: &Option<Inotify>| -> BTreeSet<String> {
        match (&root, ino) {
            (Some(r), Some(i)) => scan(r, |d| i.watch(d)),
            _ => running_scopes().into_iter().collect(),
        }
    };
    if ino.is_none() {
        eprintln!("labyrinth-appusage: no cgroup tree of this user's systemd: asking systemctl every {INTERVAL} s");
    }
    t.update(&read_scopes(&ino), Instant::now(), &mut desktop_ids);
    loop {
        let now = Instant::now();
        // the next thing to do without an event: write down (while an app runs), or ask systemctl again (the fallback)
        let mut wait: Option<Duration> = t.any_running().then(|| (last_flush + flush).saturating_duration_since(now));
        if ino.is_none() {
            let poll_in = Duration::from_secs(INTERVAL);
            wait = Some(wait.map_or(poll_in, |w| w.min(poll_in)));
        }
        let timeout = wait.map(|w| w.as_millis().min(i32::MAX as u128) as i32 + 1).unwrap_or(-1);
        let mut fds = [
            libc::pollfd { fd: sig, events: libc::POLLIN, revents: 0 },
            libc::pollfd { fd: ino.as_ref().map(|i| i.fd).unwrap_or(-1), events: libc::POLLIN, revents: 0 },
        ];
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, timeout) };
        let now = Instant::now();
        if n < 0 {
            if std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            eprintln!("labyrinth-appusage: poll: {}", std::io::Error::last_os_error());
            write_down(&mut t, &path, now);
            return 1;
        }
        if fds[0].revents != 0 {
            write_down(&mut t, &path, now);
            return 0;
        }
        let mut changed = false;
        if fds[1].revents != 0 {
            if let Some(i) = &ino {
                i.drain();
            }
            changed = true;
        }
        if ino.is_none() && n == 0 {
            changed = true;
        }
        if changed && t.update(&read_scopes(&ino), now, &mut desktop_ids) {
            write_down(&mut t, &path, now);
            last_flush = now;
        }
        if t.any_running() && now.saturating_duration_since(last_flush) >= flush {
            write_down(&mut t, &path, now);
            last_flush = now;
        }
        if !t.any_running() {
            // the next app's first write is a full interval after it starts
            last_flush = now;
        }
    }
}
