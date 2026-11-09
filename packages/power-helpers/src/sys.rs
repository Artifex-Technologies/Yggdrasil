//! What the helpers do outside themselves, in one place: files (under a root, "/" except in a dry run against a fake tree),
//! commands (systemctl, logger, busctl, nvidia-smi), the stop signal and the clock. A dry run (--dry-run) reads everything as
//! usual and writes nothing: each write is printed as "dry run: ..." and kept in memory, so later reads in the same run see
//! it, as they would have seen the file.
//!
//! The merged service (power.rs) runs each helper's loop on a thread of its own, each with its own Ctx: its own dry-run
//! memory and virtual clock, its own stop (a part switched off) on top of the process's stop signal, and the one graphics
//! card the parts share.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::json::{self, Json};
use crate::py::OsErr;

pub struct Ctx {
    /// Every system path is taken under this folder ("/" normally; a fake tree with --root).
    pub root: PathBuf,
    /// --dry-run: print what would be written, write nothing.
    pub dry: bool,
    /// --steps N (dry run only): N rounds of the service's loop on a virtual clock (no waiting), then stop.
    pub steps: Option<u64>,
    /// Files a dry run would have written (None: removed).
    overlay: RefCell<HashMap<PathBuf, Option<Vec<u8>>>>,
    rounds: Cell<u64>,
    vclock: Cell<f64>,
    /// Tests: the commands that would have run (they never run), and whether the helper counts as root.
    pub recorded: RefCell<Option<Vec<String>>>,
    pub as_root: Cell<Option<bool>>,
    /// --real-card (a dry run with --root): the real graphics card, read only (set calls are printed, as in any dry run).
    pub real_card: bool,
    /// The merged service: this part's own stop (switched off, or the service going down), besides the stop signal.
    pub part: Option<Arc<Stopper>>,
    /// The merged service: the graphics card all its parts share, so libnvidia-ml is initialised once in the process.
    pub cards: Option<Arc<crate::nvml::Slot>>,
    /// A dry run against a fake tree with ROOT/commands.json: the answers of the commands that only ask (loaded once).
    fake_commands: RefCell<Option<Option<FakeCommands>>>,
}

/// Where a dry run's virtual clock starts: like time.monotonic() on a computer that has been on a while.
pub const VCLOCK_START: f64 = 1000.0;

impl Ctx {
    pub fn new(root: PathBuf, dry: bool, steps: Option<u64>) -> Ctx {
        Ctx {
            root,
            dry,
            steps,
            overlay: RefCell::new(HashMap::new()),
            rounds: Cell::new(0),
            vclock: Cell::new(VCLOCK_START),
            recorded: RefCell::new(None),
            as_root: Cell::new(None),
            real_card: false,
            part: None,
            cards: None,
            fake_commands: RefCell::new(None),
        }
    }

    /// Tests: files under `root`, written for real; commands recorded, never run; root.
    pub fn for_tests(root: PathBuf) -> Ctx {
        let c = Ctx::new(root, false, None);
        *c.recorded.borrow_mut() = Some(Vec::new());
        c.as_root.set(Some(true));
        c
    }

    /// The live system, writing for real.
    pub fn system() -> Ctx {
        Ctx::new(PathBuf::from("/"), false, None)
    }

    /// A context like this one (same root, dry run, steps, card choice, part and shared card) with nothing remembered yet:
    /// what a new process of the same helper would start from.
    pub fn fresh(&self) -> Ctx {
        let mut c = Ctx::new(self.root.clone(), self.dry, self.steps);
        c.real_card = self.real_card;
        c.part = self.part.clone();
        c.cards = self.cards.clone();
        if self.recorded.borrow().is_some() {
            *c.recorded.borrow_mut() = Some(Vec::new());
        }
        c.as_root.set(self.as_root.get());
        c
    }

    /// The options every helper takes, taken out of the arguments: --dry-run, --root DIR (dry run only), --steps N (dry run
    /// only) and --real-card (dry run with --root only). Err is the message for an unusable combination.
    pub fn from_args(args: Vec<String>) -> Result<(Ctx, Vec<String>), String> {
        let (mut dry, mut root, mut steps, mut real_card, mut rest) = (false, None, None, false, Vec::new());
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            if a == "--dry-run" {
                dry = true;
            } else if a == "--real-card" {
                real_card = true;
            } else if a == "--root" || a == "--steps" {
                let v = it.next().ok_or(format!("{a} needs a value"))?;
                if a == "--root" {
                    root = Some(PathBuf::from(v));
                } else {
                    steps = Some(v.parse::<u64>().map_err(|_| "--steps needs a number".to_string())?);
                }
            } else if let Some(v) = a.strip_prefix("--root=") {
                root = Some(PathBuf::from(v));
            } else if let Some(v) = a.strip_prefix("--steps=") {
                steps = Some(v.parse::<u64>().map_err(|_| "--steps needs a number".to_string())?);
            } else {
                rest.push(a);
            }
        }
        if !dry && (root.is_some() || steps.is_some()) {
            return Err("--root and --steps are for a dry run (--dry-run)".into());
        }
        if real_card && root.is_none() {
            return Err("--real-card is for a dry run with --root".into());
        }
        let root = match root {
            Some(r) => fs::canonicalize(&r).map_err(|e| format!("--root {}: {e}", r.display()))?,
            None => PathBuf::from("/"),
        };
        let mut c = Ctx::new(root, dry, steps);
        c.real_card = real_card;
        Ok((c, rest))
    }

    /// True with --root: every system path is under a fake tree.
    pub fn fake_root(&self) -> bool {
        self.root != Path::new("/")
    }

    /// True with --root unless --real-card: the graphics card is the fake one in ROOT/nvml.json.
    pub fn fake_card(&self) -> bool {
        self.fake_root() && !self.real_card
    }

    /// A system path ("/etc/yggdrasil/x") under the root.
    pub fn path(&self, p: &str) -> PathBuf {
        self.root.join(p.trim_start_matches('/'))
    }

    /// One line of a dry run's account.
    pub fn say(&self, what: &str) {
        crate::out!("dry run: {what}");
        let _ = std::io::stdout().flush();
    }

    // ------------------------------------------------------------ files

    pub fn read(&self, p: &Path) -> std::io::Result<Vec<u8>> {
        if let Some(v) = self.overlay.borrow().get(p) {
            return v.clone().ok_or_else(|| std::io::Error::from_raw_os_error(libc::ENOENT));
        }
        fs::read(p)
    }

    /// open(p).read(): the text, which must be UTF-8.
    pub fn read_text(&self, p: &Path) -> Result<String, OsErr> {
        let b = self.read(p).map_err(|e| OsErr::at(&e, p))?;
        String::from_utf8(b).map_err(|e| OsErr::Msg(e.to_string()))
    }

    /// int(open(p).read().strip()), None when it cannot be read or is not a number.
    pub fn read_int(&self, p: &Path) -> Option<i64> {
        crate::py::int(&self.read_text(p).ok()?).ok()
    }

    /// json.load(open(p)): Err when the file cannot be read or is not JSON.
    pub fn read_json(&self, p: &Path) -> Result<Json, String> {
        json::parse_bytes(&self.read(p).map_err(|e| e.to_string())?)
    }

    pub fn exists(&self, p: &Path) -> bool {
        match self.overlay.borrow().get(p) {
            Some(v) => v.is_some(),
            None => p.exists(),
        }
    }

    /// The modification time (seconds, nanoseconds), or None.
    pub fn mtime(&self, p: &Path) -> Option<(i64, i64)> {
        use std::os::unix::fs::MetadataExt;
        let m = fs::metadata(p).ok()?;
        Some((m.mtime(), m.mtime_nsec()))
    }

    /// The names in a folder that start with `prefix` and end with `suffix`, sorted (as sorted(glob(...)) lists them).
    pub fn names(&self, dir: &Path, prefix: &str, suffix: &str) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with(prefix) && n.ends_with(suffix) && n.len() >= prefix.len() + suffix.len() && !n.starts_with('.'))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    }

    /// open(p, "w").write(text): a sysfs or powercap file.
    pub fn write_text(&self, p: &Path, text: &str) -> Result<(), OsErr> {
        if self.dry {
            self.say(&format!("write {}: {}", p.display(), text));
            self.overlay.borrow_mut().insert(p.to_path_buf(), Some(text.as_bytes().to_vec()));
            return Ok(());
        }
        fs::write(p, text).map_err(|e| OsErr::at(&e, p))
    }

    /// A settings or state file, as the Python helpers wrote it: the folder made if missing, json.dump(indent=1) into p.tmp,
    /// mode 0644, then renamed over p.
    pub fn write_json(&self, p: &Path, v: &Json) -> Result<(), OsErr> {
        if self.dry {
            self.say(&format!("write {}: {}", p.display(), json::dumps(v)));
            self.overlay.borrow_mut().insert(p.to_path_buf(), Some(json::dump_indent1(v).into_bytes()));
            return Ok(());
        }
        if let Some(dir) = p.parent() {
            fs::create_dir_all(dir).map_err(|e| OsErr::at(&e, dir))?;
        }
        let mut tmp = p.as_os_str().to_owned();
        tmp.push(".tmp");
        let tmp = PathBuf::from(tmp);
        fs::write(&tmp, json::dump_indent1(v)).map_err(|e| OsErr::at(&e, &tmp))?;
        fs::set_permissions(&tmp, fs::Permissions::from_mode(0o644)).map_err(|e| OsErr::at(&e, &tmp))?;
        fs::rename(&tmp, p).map_err(|e| OsErr::between(&e, &tmp, p))
    }

    /// os.remove(p), where a file that is not there is fine.
    pub fn remove(&self, p: &Path) -> Result<(), OsErr> {
        if self.dry {
            if self.exists(p) {
                self.say(&format!("remove {}", p.display()));
            }
            self.overlay.borrow_mut().insert(p.to_path_buf(), None);
            return Ok(());
        }
        match fs::remove_file(p) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(OsErr::at(&e, p)),
            _ => Ok(()),
        }
    }

    // ------------------------------------------------------------ commands

    /// A command that only asks (systemctl is-active): it runs in a dry run too. The exit code, or None when it could not run.
    pub fn ask(&self, cmd: &str, args: &[&str]) -> Option<(i32, String)> {
        self.ask_within(cmd, args, None)
    }

    /// `ask` with a time limit (subprocess.run(timeout=...)): None also when it took longer (the command is killed then).
    /// Against a fake tree that has ROOT/commands.json, the answers come from that file instead and nothing runs.
    pub fn ask_within(&self, cmd: &str, args: &[&str], limit: Option<f64>) -> Option<(i32, String)> {
        let line = format!("{} {}", cmd, args.join(" "));
        if let Some(r) = self.recorded.borrow_mut().as_mut() {
            r.push(line.clone());
        }
        if self.fake_root() {
            let mut fc = self.fake_commands.borrow_mut();
            if let Some(answer) = fc.get_or_insert_with(|| FakeCommands::load(&self.path("/commands.json"))).as_mut().and_then(|fc| fc.answer(&line)) {
                return answer;
            }
        }
        if self.recorded.borrow().is_some() {
            return Some((3, String::new()));
        }
        run_within(cmd, args, limit)
    }

    /// A command that changes something (systemctl restart, logger): a dry run prints it and takes it as done.
    pub fn act(&self, cmd: &str, args: &[&str]) -> i32 {
        if self.dry {
            self.say(&format!("run {} {}", cmd, args.join(" ")));
            return 0;
        }
        if let Some(r) = self.recorded.borrow_mut().as_mut() {
            r.push(format!("{} {}", cmd, args.join(" ")));
            return 0;
        }
        match Command::new(cmd).args(args).stdin(Stdio::null()).output() {
            Ok(o) => o.status.code().unwrap_or(-1),
            Err(_) => -1,
        }
    }

    pub fn is_root(&self) -> bool {
        if let Some(r) = self.as_root.get() {
            return r;
        }
        // a dry run changes nothing, so it shows what root would do
        self.dry || unsafe { libc::geteuid() } == 0
    }

    // ------------------------------------------------------------ clock and stop

    /// time.monotonic(), or the dry run's virtual clock with --steps.
    pub fn monotonic(&self) -> f64 {
        if self.steps.is_some() {
            return self.vclock.get();
        }
        now_monotonic()
    }

    /// time.time()
    pub fn time(&self) -> f64 {
        let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        d.as_nanos() as f64 / 1e9
    }

    /// Waits `s` seconds unless a stop signal comes first. With --steps it waits for nothing: the virtual clock moves on and
    /// one round of the loop is counted.
    pub fn sleep(&self, s: f64) {
        self.wait(s);
        if self.steps.is_some() {
            self.rounds.set(self.rounds.get() + 1);
        }
    }

    /// Waits `s` seconds unless a stop comes first (with --steps: the virtual clock moves on, nothing is counted). A loop
    /// that waits here is alive: the merged service's watchdog sees the beat.
    pub fn wait(&self, s: f64) {
        self.beat();
        if self.steps.is_some() {
            self.vclock.set(self.vclock.get() + s.max(0.0));
            return;
        }
        wait_unless_stopped(s, self.part.as_deref());
    }

    /// Counts a round of a loop that does not sleep through `sleep` (tune watch).
    pub fn round_done(&self) {
        self.beat();
        self.rounds.set(self.rounds.get() + 1);
    }

    /// The merged service: this part's loop is alive (the watchdog reads the time of the last beat).
    pub fn beat(&self) {
        if let Some(p) = &self.part {
            p.beat();
        }
    }

    /// A stop signal came, this part was switched off, or --steps rounds are done.
    pub fn should_stop(&self) -> bool {
        self.stop_requested() || self.steps.is_some_and(|n| self.rounds.get() >= n)
    }

    /// A stop signal came, or this part was switched off (--steps aside).
    pub fn stop_requested(&self) -> bool {
        stopping() || self.part.as_ref().is_some_and(|p| p.why().is_some())
    }

    /// The file descriptors that become readable when a stop comes (the stop signal's pipe, this part's own), for a loop
    /// that waits in poll() on something else too.
    pub fn stop_fds(&self) -> Vec<RawFd> {
        let mut v = Vec::new();
        if let Some(fd) = stop_pipe() {
            v.push(fd);
        }
        if let Some(p) = &self.part {
            v.push(p.fd());
        }
        v
    }
}

// ---------------------------------------------------------------- commands: a time limit, or a fake tree's answers

/// Runs a command with stdin closed and stderr dropped, as subprocess.run(capture_output=True, timeout=limit) did: (exit
/// code, stdout), or None when it cannot run or took longer than `limit` seconds (it is killed then).
pub fn run_within(cmd: &str, args: &[&str], limit: Option<f64>) -> Option<(i32, String)> {
    let mut child = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let Some(limit) = limit else {
        let o = child.wait_with_output().ok()?;
        return Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned()));
    };
    let deadline = now_monotonic() + limit;
    let mut out = child.stdout.take()?;
    let fd = out.as_raw_fd();
    unsafe { libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK) };
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let late = loop {
        let left = deadline - now_monotonic();
        if left <= 0.0 {
            break true;
        }
        let mut pfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        if unsafe { libc::poll(&mut pfd, 1, (left * 1000.0).ceil() as i32) } <= 0 {
            continue; // a signal, or the time is up (seen above)
        }
        match out.read(&mut chunk) {
            Ok(0) => break false,
            Ok(k) => buf.extend_from_slice(&chunk[..k]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted) => {}
            Err(_) => break false,
        }
    };
    drop(out);
    if !late {
        // its output is closed: it is ending; wait for it within what is left of the limit
        loop {
            match child.try_wait() {
                Ok(Some(st)) => return Some((st.code().unwrap_or(-1), String::from_utf8_lossy(&buf).into_owned())),
                Ok(None) if now_monotonic() < deadline => {
                    let ts = libc::timespec { tv_sec: 0, tv_nsec: 5_000_000 };
                    unsafe { libc::nanosleep(&ts, std::ptr::null_mut()) };
                }
                _ => break,
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
    None
}

/// A fake tree's answers for the commands that only ask: ROOT/commands.json maps the start of a command line ("busctl
/// get-property org.freedesktop.UPower.PowerProfiles", "nvidia-smi --query-gpu") to its answer: the text it prints (exit code
/// 0), {"rc": N, "out": "..."}, {"timeout": true}, or null (the command is not there). A list gives one answer per call and
/// its last one after that. The longest key that starts the command line answers; a command line no key starts runs as usual.
struct FakeCommands {
    entries: Vec<(String, Vec<Json>, usize)>,
}

impl FakeCommands {
    fn load(path: &Path) -> Option<FakeCommands> {
        let v = json::parse_bytes(&fs::read(path).ok()?).ok()?;
        let entries = v
            .obj()?
            .iter()
            .map(|(k, v)| {
                let list = match v {
                    Json::Arr(a) if !a.is_empty() => a.clone(),
                    v => vec![v.clone()],
                };
                (k.to_string(), list, 0)
            })
            .collect();
        Some(FakeCommands { entries })
    }

    /// None: no key fits (the command runs as usual); Some(None): not there, or too slow; Some(Some(answer)).
    fn answer(&mut self, line: &str) -> Option<Option<(i32, String)>> {
        let e = self.entries.iter_mut().filter(|(k, _, _)| line.starts_with(k.as_str())).max_by_key(|(k, _, _)| k.len())?;
        let a = e.1[e.2.min(e.1.len() - 1)].clone();
        e.2 += 1;
        Some(match a {
            Json::Str(s) => Some((0, s)),
            Json::Obj(o) if o.at("timeout").truthy() => None,
            Json::Obj(o) => Some((o.at("rc").int().unwrap_or(0) as i32, o.at("out").str().unwrap_or("").to_string())),
            _ => None,
        })
    }
}

/// CLOCK_MONOTONIC, in seconds.
pub fn now_monotonic() -> f64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
}

// ---------------------------------------------------------------- a part's own stop (the merged service)

/// Why a part was asked to stop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Why {
    /// switched off, or the service stopping as asked: the part's clean stop (casefan: fans 60 %, the pump 100 %)
    Clean,
    /// the service going down on a failure (a part panicked, a loop stalled): the part's failure stop (every port 100 %)
    Failure,
}

/// A part's stop in the merged service: set once, seen by its loop at its next turn, and its waits are cut short (an
/// eventfd that stays readable once written). Also the beat of its loop, for the watchdog.
pub struct Stopper {
    why: AtomicU8,
    fd: OwnedFd,
    beat_ms: AtomicU64,
}

impl Stopper {
    pub fn new() -> std::io::Result<Stopper> {
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let s = Stopper { why: AtomicU8::new(0), fd: unsafe { OwnedFd::from_raw_fd(fd) }, beat_ms: AtomicU64::new(0) };
        s.beat();
        Ok(s)
    }

    /// Asks the part to stop (the first reason given stays).
    pub fn stop(&self, why: Why) {
        let v = match why {
            Why::Clean => 1,
            Why::Failure => 2,
        };
        if self.why.compare_exchange(0, v, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
            let one: u64 = 1;
            unsafe { libc::write(self.fd.as_raw_fd(), &one as *const u64 as *const libc::c_void, 8) };
        }
    }

    pub fn why(&self) -> Option<Why> {
        match self.why.load(Ordering::SeqCst) {
            0 => None,
            1 => Some(Why::Clean),
            _ => Some(Why::Failure),
        }
    }

    pub fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    pub fn beat(&self) {
        self.beat_ms.store((now_monotonic() * 1000.0) as u64, Ordering::SeqCst);
    }

    /// Seconds since the loop last showed it is alive.
    pub fn since_beat(&self) -> f64 {
        (now_monotonic() - self.beat_ms.load(Ordering::SeqCst) as f64 / 1000.0).max(0.0)
    }
}

// ---------------------------------------------------------------- output that never takes the helper down

/// eprintln! that cannot panic: a broken stderr (journald restarted) must never stop a helper on its way to a safe speed.
#[macro_export]
macro_rules! warn {
    ($($t:tt)*) => {
        $crate::sys::err_line(format_args!($($t)*))
    };
}

/// println! that cannot panic (a reader that went away).
#[macro_export]
macro_rules! out {
    ($($t:tt)*) => {
        $crate::sys::out_line(format_args!($($t)*))
    };
}

thread_local! {
    static TAG: Cell<Option<&'static str>> = const { Cell::new(None) };
}

/// The merged service's dry run: every line this thread prints starts with "[tag] ", so each part's account can be told
/// from the others' (parity/compare.py compares them part by part with the helpers run one by one).
pub fn set_tag(tag: Option<&'static str>) {
    TAG.with(|t| t.set(tag));
}

fn line(args: fmt::Arguments) -> String {
    match TAG.with(|t| t.get()) {
        Some(t) => format!("[{t}] {args}\n"),
        None => format!("{args}\n"),
    }
}

/// One line on stdout, written at once (the merged service's threads never mix their lines).
pub fn out_line(args: fmt::Arguments) {
    let _ = std::io::stdout().lock().write_all(line(args).as_bytes());
}

/// One line on stderr, written at once.
pub fn err_line(args: fmt::Arguments) {
    let _ = std::io::stderr().lock().write_all(line(args).as_bytes());
}

// ---------------------------------------------------------------- signals

static STOP: AtomicBool = AtomicBool::new(false);
/// The stop pipe: written by the signal handler (async-signal-safe) and never read, so once a stop came it stays readable
/// for every thread that waits on it in poll().
static STOP_R: AtomicI32 = AtomicI32::new(-1);
static STOP_W: AtomicI32 = AtomicI32::new(-1);
static PIPE: std::sync::Once = std::sync::Once::new();

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
    let w = STOP_W.load(Ordering::SeqCst);
    if w >= 0 {
        let b = 1u8;
        unsafe { libc::write(w, &b as *const u8 as *const libc::c_void, 1) };
    }
}

/// The read end of the stop pipe, once catch_stop_signals has made it.
pub fn stop_pipe() -> Option<RawFd> {
    let r = STOP_R.load(Ordering::SeqCst);
    (r >= 0).then_some(r)
}

/// SIGTERM (systemd's stop) and SIGINT end the service's loop at its next turn; a wait in progress is cut short, in any
/// thread (the handler also writes the stop pipe, which every wait polls).
pub fn catch_stop_signals() {
    PIPE.call_once(|| {
        let mut fds = [-1; 2];
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } == 0 {
            STOP_R.store(fds[0], Ordering::SeqCst);
            STOP_W.store(fds[1], Ordering::SeqCst);
        }
    });
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_signal as *const () as usize;
        libc::sigemptyset(&mut sa.sa_mask);
        sa.sa_flags = 0; // no SA_RESTART: a sleep or poll returns at once with EINTR
        libc::sigaction(libc::SIGTERM, &sa, std::ptr::null_mut());
        libc::sigaction(libc::SIGINT, &sa, std::ptr::null_mut());
    }
}

pub fn stopping() -> bool {
    STOP.load(Ordering::SeqCst)
}

/// Runs `f` with SIGTERM and SIGINT blocked in this thread, so threads it starts (libnvidia-ml starts one in nvmlInit)
/// inherit the block and a stop signal always reaches one of ours, cutting its wait short.
pub fn without_stop_signals<T>(f: impl FnOnce() -> T) -> T {
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        let mut old: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
        let r = f();
        libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut());
        r
    }
}

/// Sleeps `s` seconds, returning early when a stop signal arrives.
pub fn sleep_unless_stopped(s: f64) {
    wait_unless_stopped(s, None);
}

/// Sleeps `s` seconds, returning early when a stop signal arrives or `part` is asked to stop.
pub fn wait_unless_stopped(s: f64, part: Option<&Stopper>) {
    let stopped = || stopping() || part.is_some_and(|p| p.why().is_some());
    if s <= 0.0 || stopped() {
        return;
    }
    let deadline = now_monotonic() + s;
    let mut fds: Vec<libc::pollfd> = Vec::with_capacity(2);
    if let Some(fd) = stop_pipe() {
        fds.push(libc::pollfd { fd, events: libc::POLLIN, revents: 0 });
    }
    if let Some(p) = part {
        fds.push(libc::pollfd { fd: p.fd(), events: libc::POLLIN, revents: 0 });
    }
    loop {
        let left = deadline - now_monotonic();
        if left <= 0.0 || stopped() {
            return;
        }
        // with no descriptor, poll() is a plain sleep, cut short by a signal (EINTR)
        let ms = (left * 1000.0).ceil().min(i32::MAX as f64) as i32;
        if unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, ms) } >= 0 {
            return;
        }
    }
}

/// Exits with the code a helper's main returned, after its output is out.
pub fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}
