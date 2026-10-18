//! labyrinth-idle: how long has the user been away, and what should happen about it.
//!
//! It asks the compositor (Wayland `ext-idle-notify-v1`) to tell it when the user has been idle for each of a list of times, and
//! writes every change to ~/.local/state/labyrinth/idle.log together with what else was going on (is the screen locked, which
//! programs inhibit idling). By default it only OBSERVES: it changes nothing, so it can run beside PowerDevil to find out what
//! blanks and locks the screen. With `mode = act` in ~/.config/labyrinth/idle.conf it also performs the configured steps
//! (lock, display off, suspend; dimming is not built yet), holding back locking and sleeping while something inhibits idling.
//!
//!   labyrinth-idle            run in the foreground (autostart it)
//!
//! idle.conf (seconds of inactivity, 0 = never):
//!   mode = observe            # or: act
//!   lock = 600
//!   display_off = 900
//!   suspend = 0

mod dim;
mod policy;

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
    let base = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))).unwrap_or_else(|| "/tmp".into());
    base.join("labyrinth")
}

fn config_path() -> std::path::PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))).unwrap_or_else(|| "/tmp".into());
    base.join("labyrinth").join("idle.conf")
}

fn now() -> String {
    let out = Command::new("date").arg("+%Y-%m-%d %H:%M:%S").output().ok();
    out.map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
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
    run("busctl", &["--user", "call", "org.freedesktop.ScreenSaver", "/ScreenSaver", "org.freedesktop.ScreenSaver", "GetActive"]).ends_with("true")
}

/// logind's inhibitors as (who, what) pairs, e.g. ("Firefox", "idle").
fn inhibitors() -> Vec<(String, String)> {
    let out = run("systemd-inhibit", &["--list", "--no-pager", "--no-legend"]);
    out.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            // WHO UID USER PID COMM WHAT WHY MODE
            (f.len() >= 6).then(|| (f[0].to_string(), f[5].to_string()))
        })
        .filter(|(_, what)| what.contains("idle") || what.contains("sleep"))
        .collect()
}

fn context() -> String {
    let inh = inhibitors();
    let who: Vec<String> = inh.iter().map(|(w, what)| format!("{w}({what})")).collect();
    format!("locked={} inhibitors=[{}]", screensaver_active(), who.join(", "))
}

pub struct App {
    conf: Config,
    dimmer: Option<dim::Dimmer>,
    globals: GlobalList,
    qh: QueueHandle<App>,
    /// The largest mark that has fired since the user was last active, and when.
    longest: Option<(u32, Instant)>,
    done: HashMap<Step, bool>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for App {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}
delegate_noop!(App: ignore wl_seat::WlSeat);
delegate_noop!(App: ignore ExtIdleNotifierV1);

impl Dispatch<ExtIdleNotificationV1, u32> for App {
    fn event(app: &mut Self, _: &ExtIdleNotificationV1, event: ext_idle_notification_v1::Event, mark: &u32, _: &Connection, _: &QueueHandle<Self>) {
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
        match step {
            Step::Lock => {
                run("loginctl", &["lock-session"]);
            }
            Step::DisplayOff => {
                run("kscreen-doctor", &["--dpms", "off"]);
            }
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
        }
    }
}

fn read_conf() -> (Config, Option<std::time::SystemTime>) {
    let p = config_path();
    let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
    (std::fs::read_to_string(&p).map(|t| Config::parse(&t)).unwrap_or_default(), m)
}

fn main() {
    let (conf, mut conf_time) = read_conf();
    let conn = match Connection::connect_to_env() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("labyrinth-idle: no Wayland display: {e}");
            std::process::exit(1);
        }
    };
    let (globals, mut queue) = match registry_queue_init::<App>(&conn) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("labyrinth-idle: {e}");
            std::process::exit(1);
        }
    };
    let qh = queue.handle();
    let (Ok(notifier), Ok(seat)) = (globals.bind::<ExtIdleNotifierV1, _, _>(&qh, 1..=2, ()), globals.bind::<wl_seat::WlSeat, _, _>(&qh, 1..=9, ())) else {
        eprintln!("labyrinth-idle: the compositor does not offer ext-idle-notify-v1");
        std::process::exit(1);
    };
    let all = marks(&conf);
    let mut notes: Vec<ExtIdleNotificationV1> = all.iter().map(|m| notifier.get_input_idle_notification(m * 1000, &seat, &qh, *m)).collect();
    log(&format!("started: {} (marks: {:?}s; steps: {:?})", if conf.act { "ACTING" } else { "observing only" }, all, conf.steps().iter().map(|(s, st)| format!("{}@{}", st.name(), s)).collect::<Vec<_>>()));
    let dimmer = dim::Dimmer::new(&globals, &qh);
    if dimmer.is_none() {
        log("dimming is not available here (needs layer-shell, wp-viewporter, wl_shm)");
    }
    let mut app = App { conf, dimmer, globals, qh: qh.clone(), longest: None, done: HashMap::new() };
    let mut last_check = Instant::now();
    loop {
        if queue.dispatch_pending(&mut app).is_err() || conn.flush().is_err() {
            eprintln!("labyrinth-idle: the Wayland connection ended");
            std::process::exit(1);
        }
        let animating = app.dimmer.as_ref().map(|d| d.animating() || d.alpha > 0.0 && !d.is_shown()).unwrap_or(false);
        if let Some(guard) = conn.prepare_read() {
            use std::os::fd::AsRawFd;
            let mut pfd = libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 };
            let timeout = if animating { 16 } else { 1000 };
            let n = unsafe { libc::poll(&mut pfd, 1, timeout) };
            if n > 0 && guard.read().is_err() {
                eprintln!("labyrinth-idle: the Wayland connection ended");
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
                    log(&format!("settings changed: {} (steps: {:?})", if new.act { "ACTING" } else { "observing only" }, new.steps().iter().map(|(s, st)| format!("{}@{}", st.name(), s)).collect::<Vec<_>>()));
                    app.conf = new;
                    app.done.clear();
                    app.longest = None;
                }
            }
        }
    }
}
