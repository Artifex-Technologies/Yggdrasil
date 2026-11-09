//! What the helpers do outside themselves, in one place: files (under a root, "/" except in a dry run against a fake tree),
//! commands (systemctl, logger), the stop signal and the clock. A dry run (--dry-run) reads everything as usual and writes
//! nothing: each write is printed as "dry run: ..." and kept in memory, so later reads in the same run see it, as they would
//! have seen the file.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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

    /// The options every helper takes, taken out of the arguments: --dry-run, --root DIR (dry run only) and --steps N (dry run
    /// only). Err is the message for an unusable combination.
    pub fn from_args(args: Vec<String>) -> Result<(Ctx, Vec<String>), String> {
        let (mut dry, mut root, mut steps, mut rest) = (false, None, None, Vec::new());
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            if a == "--dry-run" {
                dry = true;
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
        let root = match root {
            Some(r) => fs::canonicalize(&r).map_err(|e| format!("--root {}: {e}", r.display()))?,
            None => PathBuf::from("/"),
        };
        Ok((Ctx::new(root, dry, steps), rest))
    }

    /// True with --root: the graphics card is the fake one in ROOT/nvml.json.
    pub fn fake_root(&self) -> bool {
        self.root != Path::new("/")
    }

    /// A system path ("/etc/yggdrasil/x") under the root.
    pub fn path(&self, p: &str) -> PathBuf {
        self.root.join(p.trim_start_matches('/'))
    }

    /// One line of a dry run's account.
    pub fn say(&self, what: &str) {
        println!("dry run: {what}");
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
        if let Some(r) = self.recorded.borrow_mut().as_mut() {
            r.push(format!("{} {}", cmd, args.join(" ")));
            return Some((3, String::new()));
        }
        let o =Command::new(cmd).args(args).stdin(Stdio::null()).output().ok()?;
        Some((o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned()))
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
        let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
        ts.tv_sec as f64 + ts.tv_nsec as f64 * 1e-9
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

    /// Waits `s` seconds unless a stop signal comes first (with --steps: the virtual clock moves on, nothing is counted).
    pub fn wait(&self, s: f64) {
        if self.steps.is_some() {
            self.vclock.set(self.vclock.get() + s.max(0.0));
            return;
        }
        sleep_unless_stopped(s);
    }

    /// Counts a round of a loop that does not sleep through `sleep` (tune watch).
    pub fn round_done(&self) {
        self.rounds.set(self.rounds.get() + 1);
    }

    /// A stop signal came, or --steps rounds are done.
    pub fn should_stop(&self) -> bool {
        stopping() || self.steps.is_some_and(|n| self.rounds.get() >= n)
    }
}

// ---------------------------------------------------------------- signals

static STOP: AtomicBool = AtomicBool::new(false);

extern "C" fn on_signal(_: libc::c_int) {
    STOP.store(true, Ordering::SeqCst);
}

/// SIGTERM (systemd's stop) and SIGINT end the service's loop at its next turn; a wait in progress is cut short.
pub fn catch_stop_signals() {
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

/// Sleeps `s` seconds, returning early when a stop signal arrives.
pub fn sleep_unless_stopped(s: f64) {
    if s <= 0.0 || stopping() {
        return;
    }
    let mut req = libc::timespec { tv_sec: s.trunc() as libc::time_t, tv_nsec: (s.fract() * 1e9) as libc::c_long };
    let mut rem = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    while unsafe { libc::nanosleep(&req, &mut rem) } == -1 {
        if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) || stopping() {
            return;
        }
        req = rem;
    }
}

/// Exits with the code a helper's main returned, after its output is out.
pub fn exit(code: i32) -> ! {
    let _ = std::io::stdout().flush();
    std::process::exit(code)
}
