//! power: the root service yggdrasil-power, which runs the loops of `tune watch`, `casefan run`, `gpufan run` and
//! `powerlimit watch` in one process, each on its own thread, so libnvidia-ml (about 26 MB in every process that initialises
//! it) is loaded and initialised once instead of once per service.
//!
//! The parts and their switches. Each old unit (yggdrasil-tune, -casefan, -gpufan, -powerlimit) stays, as a switch: starting
//! it runs `power on PART` (a file /run/yggdrasil/power/on/PART with a new token; the service starts the part and says so in
//! /run/yggdrasil/power/state.json), stopping it runs `power off PART` (the file goes; the part makes its clean stop, and the
//! command waits for it). So `systemctl restart yggdrasil-casefan` (what `casefan set` runs) is still a clean stop of the
//! fans, then a start that reads the setting afresh; enabling, disabling and `systemctl is-active` work by the old names.
//!
//! Isolation. A part's thread has its own Ctx (its own dry-run memory, clock and stop), and the parts share nothing they
//! change, only the graphics card (nvml::Slot: one nvmlInit, no lock of ours around NVML's thread-safe calls).
//!  - A part's loop that fails is that part's business, as with its own unit: its old ExecStopPost runs in its thread
//!    (casefan: every port 100 %; gpufan: the fans back to the card), and it starts again after its old RestartSec, until its
//!    old start limit (casefan and gpufan: 10 in 120 s; then it stays off at its failure stop, and its unit is stopped). The
//!    other parts go on.
//!  - A part's loop that panics: the panic is caught on its thread, the part's failure stop is made there, every other part
//!    is stopped with its failure stop, and the service ends with exit code 1, so ExecStopPost (`power stopped`) makes the
//!    stop of every part that was on once more from a new process and systemd starts the service again.
//!  - casefan's loop that stops moving (no round for STALL_S seconds: a call into NVML or a driver that never returns): the
//!    supervisor, which never calls NVML, puts every Commander Pro port at 100 % through sysfs and ends the service with
//!    exit code 1 (the process exits, stuck thread and all, and systemd starts it again).
//!  - If NVML cannot be opened or the card is lost, only the parts that read the card see it, each as before: casefan's
//!    temperatures cannot both be read, so every port goes to 100 % (and it tries again every 5 s); gpufan gives the fans
//!    back; tune goes on with the processor and reports the card's error; powerlimit asks nvidia-smi on its own.

use std::collections::VecDeque;
use std::fs;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use crate::json::{self, Obj};
use crate::nvml;
use crate::sys::{self, now_monotonic, Ctx, Stopper, Why};
use crate::{casefan, gpufan, powerlimit, tune};

pub const HELP: &str = r#"power: the root service yggdrasil-power, the loops of tune watch, casefan run, gpufan run and powerlimit watch in one
process (Settings > Power), so the NVIDIA library is loaded once.

Commands:
  power run                 the service: runs each part whose switch is on, on a thread of its own
  power on PART             switch a part on (the unit yggdrasil-PART starts this), and wait until the service has it
  power off PART            switch a part off (the unit's stop) and wait until it has made its clean stop
  power stopped             after the service ended (its ExecStopPost): each part that was on makes its stop again from a new
                            process (casefan: fans 60 %, the pump 100 %, everything 100 % after a failure; gpufan: the fans
                            back to the card)
  power status              the parts' states, as JSON
  power adopt               after an upgrade from the separate services (the package's pacman hook): restarts each old unit
                            that still runs its own helper, so it becomes a switch of this service at once

PART is tune, casefan, gpufan or powerlimit. Switches: /run/yggdrasil/power/on/PART; states: /run/yggdrasil/power/state.json.
A part whose loop fails starts again after 5 s, as its own unit did (casefan and gpufan: at most 10 times in 120 s); a part
that panics, or casefan's loop standing still for 20 s, ends the service with an error after every part made its failure
stop, and systemd starts it again.

Dry run: --dry-run (with --root DIR, --steps N, --real-card as the helpers take them) runs the parts whose switch files are
in the tree, or those named with --parts tune,casefan,gpufan,powerlimit; each line a part prints starts with "[PART] ", and
after a part's loop its old ExecStopPost runs as it did (casefan reset, gpufan reset). A dry run starts nothing again.
"#;

pub const RUN_DIR: &str = "/run/yggdrasil/power";
pub const SWITCHES: &str = "/run/yggdrasil/power/on";
pub const STATE_FILE: &str = "/run/yggdrasil/power/state.json";
pub const SERVICE: &str = "yggdrasil-power.service";
pub const NAMES: [&str; 4] = ["tune", "casefan", "gpufan", "powerlimit"];
/// casefan's loop goes round every 2 s; this long without a round and the watchdog takes over
pub const STALL_S: f64 = 20.0;
/// how long the parts get to make their stops when the service ends
pub const STOP_WAIT_S: f64 = 30.0;
/// the longest the supervisor sleeps between looks when nothing happens (a switch, a part ending and a stop wake it at once)
pub const TICK_S: f64 = 5.0;
/// `power on` waits this long for the service to take a part, `power off` for the part's clean stop
pub const ON_WAIT_S: f64 = 20.0;
pub const OFF_WAIT_S: f64 = 60.0;

/// How a part starts again after its loop failed: its old unit's Restart=on-failure, RestartSec and start limit.
#[derive(Clone, Copy, Debug)]
pub struct Restart {
    pub delay: f64,
    pub burst: usize,
    pub interval: f64,
}

/// One loop of the service.
pub trait Part: Send + Sync {
    fn name(&self) -> &'static str;
    /// The loop, as its helper's command ran it; returns its exit code. It ends when ctx.should_stop().
    fn run(&self, ctx: &Ctx) -> i32;
    /// What its old unit's ExecStopPost ran once the loop ended (`failed`: the loop failed, or the service is going down on
    /// a failure). Given a new Ctx, as a new process would have started from.
    fn after(&self, _ctx: &Ctx, _failed: bool) {}
    fn restart(&self) -> Restart;
    /// How long its loop may go without a round before the watchdog takes over (None: no watchdog).
    fn stall(&self) -> Option<f64> {
        None
    }
    /// The watchdog's safe state, made from the supervisor's thread with no help from the part's (no NVML).
    fn on_stall(&self, _ctx: &Ctx) {}
}

pub struct TunePart;

impl Part for TunePart {
    fn name(&self) -> &'static str {
        "tune"
    }
    fn run(&self, ctx: &Ctx) -> i32 {
        match tune::Tune::new(ctx, &tune::System).cmd_watch() {
            Ok(code) => code,
            Err(tune::Crash(e)) => {
                crate::warn!("tune: {e}");
                1
            }
        }
    }
    fn restart(&self) -> Restart {
        // RestartSec=5 and systemd's default start limit (5 in 10 s), which a 5 s delay never reaches
        Restart { delay: 5.0, burst: 5, interval: 10.0 }
    }
}

pub struct CasefanPart;

impl Part for CasefanPart {
    fn name(&self) -> &'static str {
        "casefan"
    }
    fn run(&self, ctx: &Ctx) -> i32 {
        casefan::cmd_run(ctx)
    }
    fn after(&self, ctx: &Ctx, failed: bool) {
        casefan::cmd_reset(ctx, failed);
    }
    fn restart(&self) -> Restart {
        Restart { delay: 5.0, burst: 10, interval: 120.0 }
    }
    fn stall(&self) -> Option<f64> {
        Some(STALL_S)
    }
    fn on_stall(&self, ctx: &Ctx) {
        casefan::cmd_reset(ctx, true);
    }
}

pub struct GpufanPart;

impl Part for GpufanPart {
    fn name(&self) -> &'static str {
        "gpufan"
    }
    fn run(&self, ctx: &Ctx) -> i32 {
        gpufan::cmd_run(ctx)
    }
    fn after(&self, ctx: &Ctx, _failed: bool) {
        gpufan::cmd_reset(ctx);
    }
    fn restart(&self) -> Restart {
        Restart { delay: 5.0, burst: 10, interval: 120.0 }
    }
}

pub struct PowerlimitPart;

impl Part for PowerlimitPart {
    fn name(&self) -> &'static str {
        "powerlimit"
    }
    fn run(&self, ctx: &Ctx) -> i32 {
        powerlimit::cmd_watch(ctx)
    }
    fn restart(&self) -> Restart {
        // Restart=on-failure with systemd's defaults (100 ms, 5 in 10 s)
        Restart { delay: 0.1, burst: 5, interval: 10.0 }
    }
}

pub fn parts() -> Vec<Arc<dyn Part>> {
    vec![Arc::new(TunePart), Arc::new(CasefanPart), Arc::new(GpufanPart), Arc::new(PowerlimitPart)]
}

// ---------------------------------------------------------------- the supervisor

/// What every part's Ctx is made from.
#[derive(Clone)]
pub struct Base {
    pub root: PathBuf,
    pub dry: bool,
    pub steps: Option<u64>,
    pub real_card: bool,
    pub cards: Option<Arc<nvml::Slot>>,
    /// tests: the parts count as root and run no command
    pub test: bool,
}

impl Base {
    pub fn ctx(&self, part: Option<Arc<Stopper>>) -> Ctx {
        let mut c = if self.test { Ctx::for_tests(self.root.clone()) } else { Ctx::new(self.root.clone(), self.dry, self.steps) };
        (c.dry, c.steps, c.real_card) = (self.dry, self.steps, self.real_card);
        c.part = part;
        c.cards = self.cards.clone();
        c
    }
}

/// Where the switches come from.
pub enum Switches {
    /// the service: one file per part that is on, its text the token of that switching on
    Dir(PathBuf),
    /// a dry run with --parts: these parts, on from the start
    Fixed(Vec<String>),
}

impl Switches {
    fn wanted(&self, name: &str) -> Option<String> {
        match self {
            Switches::Dir(d) => fs::read_to_string(d.join(name)).ok().map(|t| t.trim().to_string()),
            Switches::Fixed(v) => v.iter().any(|n| n == name).then(|| "on".to_string()),
        }
    }
}

struct End {
    code: i32,
    panicked: bool,
}

enum Life {
    Off,
    Running {
        token: String,
        th: JoinHandle<End>,
        stop: Arc<Stopper>,
    },
    /// failed; starts again at `until`
    Waiting {
        token: String,
        until: f64,
    },
    /// its loop ended by itself with exit code 0 (gpufan on "auto"), or (a dry run) it ran its rounds
    Ended {
        token: String,
    },
    /// failed too often (its start limit), or panicked: stays as it is until it is switched on again
    Failed {
        token: String,
    },
}

struct Slot {
    part: Arc<dyn Part>,
    life: Life,
    starts: VecDeque<f64>,
    exit: Option<i32>,
}

/// An eventfd a part's thread writes when it ends, so the supervisor looks at once.
struct Waker(OwnedFd);

impl Waker {
    fn new() -> std::io::Result<Waker> {
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Waker(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    fn wake(&self) {
        let one: u64 = 1;
        unsafe { libc::write(self.0.as_raw_fd(), &one as *const u64 as *const libc::c_void, 8) };
    }
    fn drain(&self) {
        let mut v: u64 = 0;
        unsafe { libc::read(self.0.as_raw_fd(), &mut v as *mut u64 as *mut libc::c_void, 8) };
    }
}

/// inotify on the switches' folder, so a switch is seen at once (None: the supervisor looks every second instead).
fn watch_dir(dir: &Path) -> Option<OwnedFd> {
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC | libc::IN_NONBLOCK) };
    if fd < 0 {
        return None;
    }
    let fd = unsafe { OwnedFd::from_raw_fd(fd) };
    let c = std::ffi::CString::new(dir.as_os_str().as_encoded_bytes()).ok()?;
    let mask = libc::IN_CREATE | libc::IN_DELETE | libc::IN_CLOSE_WRITE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM | libc::IN_ATTRIB;
    (unsafe { libc::inotify_add_watch(fd.as_raw_fd(), c.as_ptr(), mask) } >= 0).then_some(fd)
}

fn drain_fd(fd: &OwnedFd) {
    let mut buf = [0u8; 4096];
    while unsafe { libc::read(fd.as_raw_fd(), buf.as_mut_ptr() as *mut libc::c_void, buf.len()) } > 0 {}
}

pub struct Service {
    slots: Vec<Slot>,
    base: Base,
    switches: Switches,
    /// the service: the states file, and the old units stopped when their part ends by itself or gives up
    state_file: Option<PathBuf>,
    units: bool,
    /// old units to stop once they are active (see stop_unit)
    unit_stops: Vec<String>,
    /// a stop of this service alone (tests); the stop signal works too
    pub quit: Arc<Stopper>,
    waker: Arc<Waker>,
    inotify: Option<OwnedFd>,
    last_state: String,
    pub stop_wait: f64,
    /// the longest the supervisor sleeps between looks (the watchdog looks this often)
    pub tick: f64,
}

impl Service {
    pub fn new(parts: Vec<Arc<dyn Part>>, base: Base, switches: Switches, state_file: Option<PathBuf>) -> std::io::Result<Service> {
        let inotify = match &switches {
            Switches::Dir(d) if state_file.is_some() => watch_dir(d),
            _ => None,
        };
        let units = state_file.is_some() && base.root == Path::new("/") && !base.test;
        Ok(Service {
            slots: parts.into_iter().map(|part| Slot { part, life: Life::Off, starts: VecDeque::new(), exit: None }).collect(),
            base,
            switches,
            state_file,
            units,
            unit_stops: Vec::new(),
            quit: Arc::new(Stopper::new()?),
            waker: Arc::new(Waker::new()?),
            inotify,
            last_state: String::new(),
            stop_wait: STOP_WAIT_S,
            tick: TICK_S,
        })
    }

    fn say(&self, msg: &str) {
        crate::warn!("power: {msg}");
    }

    /// Starts a part's thread (or, past its start limit, leaves it failed).
    fn start(&mut self, i: usize, token: String, now: f64, again: bool) -> Life {
        let slot = &mut self.slots[i];
        let (name, r) = (slot.part.name(), slot.part.restart());
        if !again {
            slot.starts.clear(); // switched on anew: a fresh start limit, as after `systemctl restart`
        }
        while slot.starts.front().is_some_and(|t| now - t > r.interval) {
            slot.starts.pop_front();
        }
        if slot.starts.len() >= r.burst {
            self.say(&format!(
                "{name} failed {} times in {} s: left off, at its failure stop, until it is switched on again",
                r.burst, r.interval
            ));
            self.stop_unit(i);
            return Life::Failed { token };
        }
        let slot = &mut self.slots[i];
        slot.starts.push_back(now);
        let stop = match Stopper::new() {
            Ok(s) => Arc::new(s),
            Err(e) => {
                self.say(&format!("{name} cannot start ({e}); again in {} s", r.delay));
                return Life::Waiting { token, until: now + r.delay };
            }
        };
        let (part, base, waker, s) = (slot.part.clone(), self.base.clone(), self.waker.clone(), stop.clone());
        let spawned = thread::Builder::new().name(name.into()).spawn(move || part_thread(part, base, s, waker));
        match spawned {
            Ok(th) => {
                if !self.base.dry {
                    self.say(&format!("{name} on"));
                }
                Life::Running { token, th, stop }
            }
            Err(e) => {
                self.say(&format!("{name} cannot start ({e}); again in {} s", r.delay));
                Life::Waiting { token, until: now + r.delay }
            }
        }
    }

    /// The service: stops a part's old unit when the part ended by itself or gave up, so `systemctl is-active` tells the
    /// truth about it as it did (the unit's stop then runs `power off`, which finds the part already stopped). Not while the
    /// unit is still starting (its `power on` waiting: a part on "auto" ends at once), which a stop would cancel half way:
    /// it waits in unit_stops until the unit is active.
    fn stop_unit(&mut self, i: usize) {
        if self.units {
            self.unit_stops.push(format!("yggdrasil-{}.service", self.slots[i].part.name()));
            self.stop_units();
        }
    }

    fn stop_units(&mut self) {
        self.unit_stops.retain(|unit| {
            let state =
                Ctx::system().ask("systemctl", &["is-active", unit]).map(|(_, out)| crate::py::strip(&out).to_string()).unwrap_or_default();
            match state.as_str() {
                "activating" | "reloading" => true,
                "active" => {
                    Ctx::system().act("systemctl", &["stop", "--no-block", unit]);
                    false
                }
                _ => false,
            }
        });
    }

    /// One look at a part: its switch, its thread. Some(reason) when the service must end on a failure.
    fn step(&mut self, i: usize, now: f64) -> Option<String> {
        let name = self.slots[i].part.name();
        let want = self.switches.wanted(name);
        let life = std::mem::replace(&mut self.slots[i].life, Life::Off);
        let mut fatal = None;
        let next = match life {
            Life::Off => match want {
                Some(t) => self.start(i, t, now, false),
                None => Life::Off,
            },
            Life::Running { token, th, stop } if !th.is_finished() => {
                if want.as_ref() != Some(&token) && stop.why().is_none() {
                    if !self.base.dry {
                        self.say(&format!("{name} {}", if want.is_some() { "again (switched on anew)" } else { "off" }));
                    }
                    stop.stop(Why::Clean);
                }
                Life::Running { token, th, stop }
            }
            Life::Running { token, th, stop } => {
                let end = th.join().unwrap_or(End { code: 101, panicked: true });
                self.slots[i].exit = Some(end.code);
                if self.base.dry {
                    crate::out!("{name} exit {}", end.code);
                }
                if end.panicked {
                    fatal = Some(format!("{name} panicked"));
                    Life::Failed { token }
                } else if stop.why().is_some() || self.quitting() {
                    // stopped as asked: switched off, or on anew with a new token
                    match want {
                        Some(t) if t != token => self.start(i, t, now, false),
                        _ => Life::Off,
                    }
                } else if end.code == 0 || self.base.dry {
                    if !self.base.dry {
                        self.say(&format!("{name} ended by itself (exit code {})", end.code));
                    }
                    self.stop_unit(i);
                    Life::Ended { token }
                } else {
                    let d = self.slots[i].part.restart().delay;
                    self.say(&format!("{name} failed (exit code {}); again in {d} s", end.code));
                    Life::Waiting { token, until: now + d }
                }
            }
            Life::Waiting { token, until } => match want {
                Some(t) if t != token => self.start(i, t, now, false),
                None => Life::Off,
                Some(_) if now >= until => self.start(i, token, now, true),
                Some(_) => Life::Waiting { token, until },
            },
            // ended or failed: stays so until it is switched off, or on anew
            Life::Ended { token } if want.as_ref() == Some(&token) => Life::Ended { token },
            Life::Failed { token } if want.as_ref() == Some(&token) => Life::Failed { token },
            Life::Ended { .. } | Life::Failed { .. } => match want {
                Some(t) => self.start(i, t, now, false),
                None => Life::Off,
            },
        };
        self.slots[i].life = next;
        fatal
    }

    /// casefan's loop standing still: its safe state from here, and the service ends.
    fn watchdog(&self) -> Option<String> {
        for slot in &self.slots {
            let (Life::Running { stop, .. }, Some(limit)) = (&slot.life, slot.part.stall()) else { continue };
            let still = stop.since_beat();
            if still > limit {
                let name = slot.part.name();
                self.say(&format!("{name}'s loop has not moved for {still:.0} s: its failure stop from the supervisor"));
                let ctx = self.base.ctx(None);
                let _ = panic::catch_unwind(AssertUnwindSafe(|| slot.part.on_stall(&ctx)));
                return Some(format!("{name} stalled"));
            }
        }
        None
    }

    fn state_json(&self) -> String {
        let mut parts = Obj::new();
        for s in &self.slots {
            let (state, token) = match &s.life {
                Life::Off => ("off", None),
                Life::Running { token, stop, .. } => (if stop.why().is_some() { "stopping" } else { "running" }, Some(token)),
                Life::Waiting { token, .. } => ("waiting", Some(token)),
                Life::Ended { token } => ("ended", Some(token)),
                Life::Failed { token } => ("failed", Some(token)),
            };
            let o = Obj::new()
                .with("state", state.into())
                .with("token", token.map(|t| t.as_str()).into())
                .with("exit", s.exit.map(i64::from).into());
            parts.set(s.part.name(), o.with("starts", (s.starts.len() as i64).into()).into());
        }
        json::dumps(&Obj::new().with("pid", (std::process::id() as i64).into()).with("parts", parts.into()).into())
    }

    fn write_state(&mut self) {
        let Some(p) = self.state_file.clone() else { return };
        let s = self.state_json();
        if s == self.last_state {
            return;
        }
        let tmp = p.with_extension("json.tmp");
        if let Some(dir) = p.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if fs::write(&tmp, &s).and_then(|_| fs::rename(&tmp, &p)).is_ok() {
            self.last_state = s;
        }
    }

    fn quitting(&self) -> bool {
        sys::stopping() || self.quit.why().is_some()
    }

    /// Waits until a switch changes, a part ends, a stop comes or `secs` pass.
    fn nap(&self, secs: f64) {
        let mut fds = vec![libc::pollfd { fd: self.waker.0.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        fds.push(libc::pollfd { fd: self.quit.fd(), events: libc::POLLIN, revents: 0 });
        if let Some(fd) = sys::stop_pipe() {
            fds.push(libc::pollfd { fd, events: libc::POLLIN, revents: 0 });
        }
        if let Some(i) = &self.inotify {
            fds.push(libc::pollfd { fd: i.as_raw_fd(), events: libc::POLLIN, revents: 0 });
        }
        let secs = if self.inotify.is_none() { secs.min(1.0) } else { secs };
        unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, (secs.max(0.0) * 1000.0).ceil() as i32) };
        self.waker.drain();
        if let Some(i) = &self.inotify {
            drain_fd(i);
        }
    }

    /// The service's life: returns its exit code (1 when it ends on a failure).
    pub fn run(&mut self) -> i32 {
        let mut fatal = None;
        loop {
            if self.quitting() {
                break;
            }
            let now = now_monotonic();
            for i in 0..self.slots.len() {
                if let Some(f) = self.step(i, now) {
                    fatal.get_or_insert(f);
                }
            }
            if fatal.is_none() {
                fatal = self.watchdog();
            }
            if !self.unit_stops.is_empty() {
                self.stop_units();
            }
            self.write_state();
            if fatal.is_some() || self.quitting() {
                break;
            }
            // a dry run with --steps ends once every part has run its rounds
            if self.base.dry
                && self.base.steps.is_some()
                && !self.slots.iter().any(|s| matches!(s.life, Life::Running { .. } | Life::Waiting { .. }))
            {
                break;
            }
            // an old unit waiting to be stopped is looked at often, until it has started
            let mut secs: f64 = if self.unit_stops.is_empty() { self.tick } else { self.tick.min(0.25) };
            for s in &self.slots {
                if let Life::Waiting { until, .. } = s.life {
                    secs = secs.min(until - now);
                }
            }
            self.nap(secs);
        }
        if let Some(f) = &fatal {
            self.say(&format!("{f}: every part makes its failure stop, and the service ends with an error (systemd starts it again)"));
        }
        let stuck = self.shutdown(if fatal.is_some() { Why::Failure } else { Why::Clean });
        if fatal.is_some() || stuck {
            1
        } else {
            0
        }
    }

    /// Stops every running part and waits for their stops (at most stop_wait). True when one could not be waited for, or
    /// stood still and the watchdog stepped in.
    fn shutdown(&mut self, why: Why) -> bool {
        for s in &self.slots {
            if let Life::Running { stop, .. } = &s.life {
                stop.stop(why);
            }
        }
        let deadline = now_monotonic() + self.stop_wait;
        let mut bad = false;
        loop {
            for i in 0..self.slots.len() {
                if matches!(&self.slots[i].life, Life::Running { th, .. } if th.is_finished()) {
                    let Life::Running { th, .. } = std::mem::replace(&mut self.slots[i].life, Life::Off) else { unreachable!() };
                    let end = th.join().unwrap_or(End { code: 101, panicked: true });
                    self.slots[i].exit = Some(end.code);
                    if self.base.dry {
                        crate::out!("{} exit {}", self.slots[i].part.name(), end.code);
                    }
                }
            }
            let running: Vec<&Slot> = self.slots.iter().filter(|s| matches!(s.life, Life::Running { .. })).collect();
            if running.is_empty() {
                break;
            }
            // the others are waited for while they make their stops; only the ones standing still are given up on
            let still = |s: &Slot| matches!(&s.life, Life::Running { stop, .. } if s.part.stall().is_some_and(|l| stop.since_beat() > l));
            if running.iter().all(|s| still(s)) || now_monotonic() > deadline {
                for s in running {
                    self.say(&format!("{} did not stop: its failure stop from the supervisor", s.part.name()));
                    let ctx = self.base.ctx(None);
                    let _ = panic::catch_unwind(AssertUnwindSafe(|| s.part.on_stall(&ctx)));
                }
                bad = true;
                break;
            }
            self.nap(self.tick.min(0.2));
        }
        for s in &mut self.slots {
            if !matches!(s.life, Life::Running { .. }) {
                s.life = Life::Off;
            }
        }
        self.write_state();
        bad
    }

    /// Whether a part's thread is still running (after `run` returned: one that could not be stopped).
    pub fn stuck(&self) -> bool {
        self.slots.iter().any(|s| matches!(s.life, Life::Running { .. }))
    }
}

/// A part's thread: its loop, then its old ExecStopPost; a panic is caught here and the part's failure stop made.
fn part_thread(part: Arc<dyn Part>, base: Base, stop: Arc<Stopper>, waker: Arc<Waker>) -> End {
    if base.dry {
        sys::set_tag(Some(part.name()));
    }
    let ctx = base.ctx(Some(stop.clone()));
    let end = match panic::catch_unwind(AssertUnwindSafe(|| part.run(&ctx))) {
        Ok(code) => {
            let failed = code != 0 || stop.why() == Some(Why::Failure);
            let fresh = ctx.fresh();
            let _ = panic::catch_unwind(AssertUnwindSafe(|| part.after(&fresh, failed)));
            End { code, panicked: false }
        }
        Err(_) => {
            let fresh = base.ctx(Some(stop.clone()));
            let _ = panic::catch_unwind(AssertUnwindSafe(|| part.after(&fresh, true)));
            End { code: 101, panicked: true }
        }
    };
    waker.wake();
    end
}

// ---------------------------------------------------------------- commands

fn known(name: &str) -> bool {
    NAMES.contains(&name)
}

fn read_state(ctx: &Ctx) -> Option<Obj> {
    ctx.read_json(&ctx.path(STATE_FILE)).ok()?.obj().cloned()
}

fn part_state(ctx: &Ctx, name: &str) -> (Option<String>, Option<String>) {
    let p = read_state(ctx).and_then(|s| s.at("parts").obj().and_then(|p| p.at(name).obj().cloned())).unwrap_or_default();
    (p.at("state").str().map(String::from), p.at("token").str().map(String::from))
}

pub fn cmd_run(ctx: &Ctx, only: Option<Vec<String>>) -> i32 {
    if !ctx.is_root() {
        crate::warn!("power: run is the root service (yggdrasil-power)");
        return 1;
    }
    sys::catch_stop_signals();
    if ctx.dry {
        sys::set_tag(Some("power"));
    }
    // one card for every part (not a fake one: each part reads its own, as the helpers run one by one do)
    let cards = (!ctx.fake_card()).then(|| Arc::new(nvml::Slot::new()));
    let base = Base { root: ctx.root.clone(), dry: ctx.dry, steps: ctx.steps, real_card: ctx.real_card, cards, test: false };
    let switches = match only {
        Some(v) => Switches::Fixed(v),
        None => Switches::Dir(ctx.path(SWITCHES)),
    };
    let state_file = (!ctx.dry).then(|| ctx.path(STATE_FILE));
    if !ctx.dry {
        let _ = fs::create_dir_all(ctx.path(SWITCHES));
    }
    let mut svc = match Service::new(parts(), base, switches, state_file) {
        Ok(s) => s,
        Err(e) => {
            crate::warn!("power: {e}");
            return 1;
        }
    };
    let code = svc.run();
    if svc.stuck() {
        // a thread that never came back (inside NVML or a driver): leave without running anything more in this process
        let _ = std::io::Write::flush(&mut std::io::stdout());
        unsafe { libc::_exit(code) };
    }
    code
}

/// The token of one switching on: when and by whom, so a restart of the unit is a new one.
fn token(ctx: &Ctx) -> String {
    format!("{:.6}-{}", ctx.time(), std::process::id())
}

pub fn cmd_on(ctx: &Ctx, name: &str) -> i32 {
    if !known(name) {
        crate::warn!("power: no part named {} (tune, casefan, gpufan or powerlimit)", crate::py::repr(name));
        return 2;
    }
    if !ctx.is_root() {
        crate::warn!("power: on needs root (the unit yggdrasil-{name} runs it)");
        return 1;
    }
    let t = token(ctx);
    let p = ctx.path(SWITCHES).join(name);
    if !ctx.dry {
        let _ = fs::create_dir_all(ctx.path(SWITCHES));
        let tmp = p.with_extension("tmp");
        if let Err(e) = fs::write(&tmp, format!("{t}\n")).and_then(|_| fs::rename(&tmp, &p)) {
            crate::warn!("power: {}: {e}", p.display());
            return 1;
        }
    } else {
        ctx.say(&format!("write {}: {t}", p.display()));
        return 0;
    }
    let deadline = now_monotonic() + ON_WAIT_S;
    while now_monotonic() < deadline {
        if part_state(ctx, name).1.as_deref() == Some(t.as_str()) {
            return 0;
        }
        sys::sleep_unless_stopped(0.05);
    }
    crate::warn!("power: the power service did not take {name} within {ON_WAIT_S} s (systemctl status {SERVICE})");
    1
}

pub fn cmd_off(ctx: &Ctx, name: &str) -> i32 {
    if !known(name) {
        crate::warn!("power: no part named {} (tune, casefan, gpufan or powerlimit)", crate::py::repr(name));
        return 2;
    }
    if !ctx.is_root() {
        crate::warn!("power: off needs root (the unit yggdrasil-{name} runs it)");
        return 1;
    }
    let p = ctx.path(SWITCHES).join(name);
    if let Err(e) = ctx.remove(&p) {
        crate::warn!("power: {e}");
        return 1;
    }
    if ctx.dry {
        return 0;
    }
    // the service not running (it failed, or waits to start again): its ExecStopPost already made every part's stop
    if ctx.ask("systemctl", &["is-active", "--quiet", SERVICE]).map(|(c, _)| c) != Some(0) {
        return 0;
    }
    let deadline = now_monotonic() + OFF_WAIT_S;
    while now_monotonic() < deadline {
        if matches!(part_state(ctx, name).0.as_deref(), Some("off") | None) {
            return 0;
        }
        sys::sleep_unless_stopped(0.05);
    }
    crate::warn!("power: {name} did not stop within {OFF_WAIT_S} s");
    0
}

/// ExecStopPost: the service ended; each part that is still switched on makes its stop (its old unit's ExecStopPost) from
/// this new process, whatever way the service ended (a crash, a kill, a stall).
pub fn cmd_stopped(ctx: &Ctx) -> i32 {
    if !ctx.is_root() {
        crate::warn!("power: stopped needs root (it is yggdrasil-power's ExecStopPost)");
        return 1;
    }
    stopped(ctx, std::env::var("SERVICE_RESULT").unwrap_or_else(|_| "success".into()) != "success");
    0
}

/// The stops `power stopped` makes: each part still switched on, with its failure stop when the service failed.
pub fn stopped(ctx: &Ctx, failed: bool) {
    for p in parts() {
        if ctx.exists(&ctx.path(SWITCHES).join(p.name())) {
            p.after(ctx, failed);
        }
    }
}

/// After an upgrade from the separate services (the package's pacman hook runs this once the units are reloaded): each old
/// unit still running its own helper process is restarted, so the old helper stops and the unit becomes a switch of
/// yggdrasil-power at once. Until then it would run with its unit's new definition, which has no ExecStopPost and no
/// Restart= for that process. A switch has no process of its own (no main PID), so it is left alone; so is everything in
/// a chroot (systemctl answers nothing there).
pub fn cmd_adopt(ctx: &Ctx) -> i32 {
    if !ctx.is_root() {
        crate::warn!("power: adopt needs root (the package's pacman hook runs it)");
        return 1;
    }
    for name in NAMES {
        let unit = format!("yggdrasil-{name}.service");
        let Some((0, out)) = ctx.ask("systemctl", &["show", "-p", "MainPID", "--value", &unit]) else { continue };
        if old_process(&out) {
            crate::warn!("power: {unit} still runs its own process: restarting it as a part of {SERVICE}");
            ctx.act("systemctl", &["restart", &unit]);
        }
    }
    0
}

/// `systemctl show -p MainPID --value` of a unit that still runs a process of its own.
pub fn old_process(main_pid: &str) -> bool {
    crate::py::int(main_pid).is_ok_and(|pid| pid > 0)
}

pub fn cmd_status(ctx: &Ctx) -> i32 {
    let running = ctx.ask("systemctl", &["is-active", "--quiet", SERVICE]).map(|(c, _)| c) == Some(0);
    let mut out = Obj::new().with("running", running.into());
    if let Some(s) = read_state(ctx) {
        out.set("pid", s.at("pid").clone());
        out.set("parts", s.at("parts").clone());
    }
    crate::out!("{}", json::dumps(&out.into()));
    0
}

pub fn main(args: Vec<String>) -> i32 {
    let mut only: Option<Vec<String>> = None;
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if a == "--parts" {
            let Some(v) = it.next() else {
                crate::warn!("power: --parts needs a value");
                return 2;
            };
            only = Some(v.split(',').filter(|s| !s.is_empty()).map(String::from).collect());
        } else if let Some(v) = a.strip_prefix("--parts=") {
            only = Some(v.split(',').filter(|s| !s.is_empty()).map(String::from).collect());
        } else {
            rest.push(a);
        }
    }
    let (ctx, argv) = match Ctx::from_args(rest) {
        Ok(x) => x,
        Err(e) => {
            crate::warn!("power: {e}");
            return 2;
        }
    };
    if let Some(v) = &only {
        if !ctx.dry {
            crate::warn!("power: --parts is for a dry run (--dry-run)");
            return 2;
        }
        if let Some(bad) = v.iter().find(|n| !known(n)) {
            crate::warn!("power: no part named {} (tune, casefan, gpufan or powerlimit)", crate::py::repr(bad));
            return 2;
        }
    }
    if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
        crate::out!("{HELP}");
        return if argv.len() >= 2 { 0 } else { 2 };
    }
    match (argv[1].as_str(), argv.len()) {
        ("run", 2) => cmd_run(&ctx, only),
        ("on", 3) => cmd_on(&ctx, &argv[2]),
        ("off", 3) => cmd_off(&ctx, &argv[2]),
        ("stopped", 2) => cmd_stopped(&ctx),
        ("adopt", 2) => cmd_adopt(&ctx),
        ("status", 2) => cmd_status(&ctx),
        _ => {
            crate::warn!("power: unknown command (see --help)");
            2
        }
    }
}

#[cfg(test)]
mod tests;
