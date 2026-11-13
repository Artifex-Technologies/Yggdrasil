//! yggdrasil-idle: how long has the user been away, and what should happen about it.
//!
//! It asks the compositor (Wayland `ext-idle-notify-v1`) to tell it when the user has been idle for each of a list of times, and
//! writes every change to ~/.local/state/yggdrasil/idle.log together with what else was going on (is the screen locked, which
//! programs inhibit idling). By default it only OBSERVES: it changes nothing, which shows what would blank and lock the screen. With `mode = act` in ~/.config/yggdrasil/idle.conf it also performs the configured steps
//! (lock, display off, suspend; dimming is not built yet), holding back locking and sleeping while something inhibits idling.
//!
//!   yggdrasil-idle            run in the foreground (autostart it)
//!
//! idle.conf (seconds of inactivity, 0 = never):
//!   mode = observe            # or: act
//!   lock = 600
//!   display_off = 900
//!   suspend = 0

mod dim;
mod inhibit;
mod lights;
mod lock;
mod policy;
mod power;

use std::collections::HashMap;
use std::io::Write;
use std::process::Command;
use std::time::{Duration, Instant};

use policy::{blocked_by_inhibitors, due_at, marks, Config, Step};
use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{delegate_noop, Connection, Dispatch, QueueHandle};
use wayland_protocols::ext::idle_notify::v1::client::{
    ext_idle_notification_v1::{self, ExtIdleNotificationV1},
    ext_idle_notifier_v1::ExtIdleNotifierV1,
};

fn state_dir() -> std::path::PathBuf {
    yggdrasil_conf::dirs::state()
}

fn config_path() -> std::path::PathBuf {
    yggdrasil_conf::dirs::config().join("idle.conf")
}

/// The local time for the log, without starting `date` for every line.
fn now() -> String {
    // SAFETY: time and localtime_r only fill what they are given
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe { libc::localtime_r(&libc::time(std::ptr::null_mut()), &mut tm) };
    format!("{:04}-{:02}-{:02} {:02}:{:02}:{:02}", tm.tm_year + 1900, tm.tm_mon + 1, tm.tm_mday, tm.tm_hour, tm.tm_min, tm.tm_sec)
}

fn log(line: &str) {
    let dir = state_dir();
    let _ = std::fs::create_dir_all(&dir);
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("idle.log")) {
        let _ = writeln!(f, "{} {}", now(), line);
    }
}

fn run(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

/// Is the screen locked right now?
fn screensaver_active() -> bool {
    run("busctl", &["--user", "call", "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver", "GetActive"])
        .ends_with("true")
}

/// logind's blocking inhibitors on idling or sleeping, as (who, what) pairs, e.g. ("Firefox", "idle"). Only "block" locks count:
/// "delay" locks (NetworkManager, UPower and the compositor always hold one) only ask logind to wait a moment before sleeping.
fn inhibitors() -> Vec<(String, String)> {
    /// One ListInhibitors entry: (what, who, why, mode, uid, pid).
    type Inhibitor = (String, String, String, String, u32, u32);
    let list = || -> zbus::Result<Vec<Inhibitor>> {
        let sys = zbus::blocking::Connection::system()?;
        let login =
            zbus::blocking::Proxy::new(&sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")?;
        login.call("ListInhibitors", &())
    };
    policy::blocking_inhibitors(list().unwrap_or_default())
}

fn context() -> String {
    let inh = inhibitors();
    let who: Vec<String> = inh.iter().map(|(w, what)| format!("{w}({what})")).collect();
    format!("locked={} inhibitors=[{}]", screensaver_active(), who.join(", "))
}

pub struct App {
    conf: Config,
    dimmer: Option<dim::Dimmer>,
    /// Display power through the compositor (None: it has no output-power protocol).
    power: Option<power::Power>,
    globals: GlobalList,
    qh: QueueHandle<App>,
    /// The largest mark that has fired since the user was last active, and when.
    longest: Option<(u32, Instant)>,
    done: HashMap<Step, bool>,
    /// The screensaver while it runs (it ends when the user is back).
    screensaver: Option<std::process::Child>,
    /// The lights were told to go off and not yet to come back.
    lights_out: bool,
    /// The pointer is hidden (dim or black screen): the compositor's own idle-pointer timeout as it was before, to put back.
    pointer_was: Option<f32>,
}

/// The value this program sets the compositor's idle-pointer timeout to (Hyprland's `cursor:inactive_timeout`) while it hides the
/// pointer. Seeing it at start-up means an earlier run ended with the pointer hidden, so it is put back to 0 (the default).
const POINTER_HIDDEN: f32 = 0.25;

/// The compositor's idle-pointer timeout now (seconds; 0 = never hide), or None when no compositor answers (yggdrasil-wm: Hyprland
/// or Yggdrasil's own).
fn pointer_timeout() -> Option<f32> {
    yggdrasil_wm::Desk::detect()?.pointer_timeout()
}

fn set_pointer_timeout(secs: f32) {
    if let Some(d) = yggdrasil_wm::Desk::detect() {
        d.set_pointer_timeout(secs);
    }
}

/// Keyboard and mouse lights off (`lights::Op::Off`) or back as they were (`On`): Yggdrasil's own (lights.rs, on its worker
/// thread), then the user's own hooks (~/.config/yggdrasil/idle.d, each an executable file, run with "lights-off" or
/// "lights-on"), without waiting for either.
fn lights(op: lights::Op) {
    lights::request(op);
    let arg = if op == lights::Op::Off { "lights-off" } else { "lights-on" };
    let Ok(rd) = std::fs::read_dir(yggdrasil_conf::dirs::config().join("idle.d")) else { return };
    let mut hooks: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    hooks.sort();
    for h in hooks {
        match Command::new(&h).arg(arg).stdin(std::process::Stdio::null()).spawn() {
            Ok(mut c) => {
                std::thread::spawn(move || {
                    let _ = c.wait();
                });
            }
            Err(e) => log(&format!("lights hook {} failed: {e}", h.display())),
        }
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
delegate_noop!(App: ignore wl_seat::WlSeat);
delegate_noop!(App: ignore ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, u32> for App {
    fn event(
        app: &mut Self,
        _: &ExtIdleNotificationV1,
        event: ext_idle_notification_v1::Event,
        mark: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            ext_idle_notification_v1::Event::Idled => {
                app.longest = Some((app.longest.map(|(m, _)| m.max(*mark)).unwrap_or(*mark), Instant::now()));
                log(&format!("idle {}s  {}", mark, context()));
                for step in due_at(&app.conf, *mark) {
                    app.perform(step);
                }
            }
            ext_idle_notification_v1::Event::Resumed => {
                // every mark reports its own resume; say it once, for the longest one that had fired
                if let Some(d) = app.dimmer.as_mut() {
                    d.hide();
                }
                if let Some(was) = app.pointer_was.take() {
                    set_pointer_timeout(was);
                }
                // the screensaver ends as soon as the user is back (the key or click that woke it went to it, not to a program)
                if let Some(mut c) = app.screensaver.take() {
                    let _ = c.kill();
                    let _ = c.wait();
                }
                if let Some(p) = app.power.as_mut().filter(|p| p.off) {
                    p.set(true);
                }
                if app.lights_out {
                    app.lights_out = false;
                    lights(lights::Op::On);
                }
                if let Some((m, _)) = app.longest.take() {
                    log(&format!("active again (had been idle {}s or more)  {}", m, context()));
                    app.done.clear();
                }
            }
            _ => {}
        }
    }
}

impl App {
    fn perform(&mut self, step: Step) {
        if !self.conf.act {
            log(&format!("would {} (observe mode: nothing done)", step.name()));
            return;
        }
        let inh = inhibitors();
        let what: Vec<&str> = inh.iter().map(|(_, w)| w.as_str()).collect();
        if blocked_by_inhibitors(step, &what) {
            log(&format!("{} held back: {}", step.name(), inh.iter().map(|(w, _)| w.as_str()).collect::<Vec<_>>().join(", ")));
            return;
        }
        if self.done.insert(step, true).is_some() {
            return;
        }
        log(&format!("doing {}", step.name()));
        if policy::hides_pointer(step) && self.pointer_was.is_none() {
            // the pointer is drawn above everything (its own plane), so the dim and black covers never darken it: the compositor hides
            // an unmoved pointer once it has been still this long, and it has been still for the whole idle time already
            if let Some(was) = pointer_timeout() {
                self.pointer_was = Some(if was == POINTER_HIDDEN { 0.0 } else { was });
                set_pointer_timeout(POINTER_HIDDEN);
            }
        }
        if self.conf.lights_off && policy::darkens(step) && !self.lights_out {
            self.lights_out = true;
            lights(lights::Op::Off);
        }
        match step {
            Step::Screensaver => {
                let child = std::process::Command::new("yggdrasil-splash").arg("--screensaver").stdin(std::process::Stdio::null()).spawn();
                match child {
                    Ok(c) => self.screensaver = Some(c),
                    Err(e) => log(&format!("cannot start the screensaver (yggdrasil-splash): {e}")),
                }
            }
            Step::Lock => {
                run("loginctl", &["lock-session"]);
            }
            Step::DisplayOff => match self.power.as_mut() {
                Some(p) => p.set(false),
                None => log("cannot turn the displays off: the compositor lacks output-power management"),
            },
            Step::Suspend => {
                run("systemctl", &["suspend"]);
            }
            Step::Hibernate => {
                run("systemctl", &["hibernate"]);
            }
            Step::Dim => match self.dimmer.as_mut() {
                Some(d) => d.show(&self.globals, &self.qh, dim::alpha_for(self.conf.dim_level)),
                None => log("cannot dim: the compositor lacks layer-shell or viewporter"),
            },
            Step::Blank => match self.dimmer.as_mut() {
                Some(d) => d.show(&self.globals, &self.qh, 1.0),
                None => log("cannot black the screen: the compositor lacks layer-shell or viewporter"),
            },
        }
    }
}

fn read_conf() -> (Config, Option<std::time::SystemTime>) {
    let p = config_path();
    let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
    (std::fs::read_to_string(&p).map(|t| Config::parse(&t)).unwrap_or_default(), m)
}

fn main() {
    // "please don't sleep" requests from programs, as logind locks (waits while another program still answers them)
    inhibit::spawn();
    // in Yggdrasil's own session, the lock requests are ours to answer
    lock::spawn();
    let (conf, mut conf_time) = read_conf();
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yggdrasil-idle: no Wayland display: {e}");
            std::process::exit(1);
        }
    };
    let (globals, mut queue) = match registry_queue_init::<App>(&conn) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("yggdrasil-idle: {e}");
            std::process::exit(1);
        }
    };
    let qh = queue.handle();
    let (Ok(notifier), Ok(seat)) =
        (globals.bind::<ExtIdleNotifierV1, _, _>(&qh, 1..=2, ()), globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=9, ()))
    else {
        eprintln!("yggdrasil-idle: the compositor does not offer ext-idle-notify-v1");
        std::process::exit(1);
    };
    let all = marks(&conf);
    let mut notes: Vec<ExtIdleNotificationV1> =
        all.iter().map(|m| notifier.get_input_idle_notification(m * 1000, &seat, &qh, *m)).collect();
    log(&format!(
        "started: {} (marks: {:?}s; steps: {:?})",
        if conf.act { "ACTING" } else { "observing only" },
        all,
        conf.steps().iter().map(|(s, st)| format!("{}@{}", st.name(), s)).collect::<Vec<_>>()
    ));
    let dimmer = dim::Dimmer::new(&globals, &qh);
    if dimmer.is_none() {
        log("dimming is not available here (needs layer-shell, wp-viewporter, wl_shm)");
    }
    let power = power::Power::new(&globals, &qh);
    let mut app = App {
        conf,
        dimmer,
        power,
        globals,
        qh: qh.clone(),
        longest: None,
        done: HashMap::new(),
        screensaver: None,
        lights_out: false,
        pointer_was: None,
    };
    if pointer_timeout() == Some(POINTER_HIDDEN) {
        set_pointer_timeout(0.0);
    }
    // the same for lights an earlier run (or session) turned off and never put back: with nothing to put back, nothing is done
    lights(lights::Op::On);
    let mut last_check = Instant::now();
    loop {
        if queue.dispatch_pending(&mut app).is_err() || conn.flush().is_err() {
            eprintln!("yggdrasil-idle: the Wayland connection ended");
            std::process::exit(1);
        }
        let animating = app.dimmer.as_ref().map(|d| d.animating() || d.alpha > 0.0 && !d.is_shown()).unwrap_or(false);
        if let Some(guard) = conn.prepare_read() {
            use std::os::fd::AsRawFd;
            let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let timeout = if animating { 16 } else { 1000 };
            let n = unsafe { libc::poll(&mut pfd, 1, timeout) };
            if n > 0 && guard.read().is_err() {
                eprintln!("yggdrasil-idle: the Wayland connection ended");
                std::process::exit(1);
            }
        }
        if animating {
            if let Some(d) = app.dimmer.as_mut() {
                d.tick(&qh);
            }
            let _ = conn.flush();
        }
        // pick up a changed idle.conf (Settings writes it) without a restart
        if last_check.elapsed() >= Duration::from_secs(2) {
            last_check = Instant::now();
            let (new, t) = read_conf();
            if t != conf_time {
                conf_time = t;
                if new != app.conf {
                    for n in notes.drain(..) {
                        n.destroy();
                    }
                    let all = marks(&new);
                    notes = all.iter().map(|m| notifier.get_input_idle_notification(m * 1000, &seat, &qh, *m)).collect();
                    log(&format!(
                        "settings changed: {} (steps: {:?})",
                        if new.act { "ACTING" } else { "observing only" },
                        new.steps().iter().map(|(s, st)| format!("{}@{}", st.name(), s)).collect::<Vec<_>>()
                    ));
                    app.conf = new;
                    app.done.clear();
                    app.longest = None;
                }
            }
        }
    }
}
