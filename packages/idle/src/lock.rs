//! Locking the screen in Yggdrasil's own session: logind asks the session to lock (Super+L, `loginctl lock-session`, the idle timer)
//! or the machine is about to sleep, and this starts `yggdrasil-lock` (once; it exits when the right password is typed). In another
//! desktop's session that desktop's own lock screen answers those requests, so this stays out of it.

use std::io::BufRead;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// True once the running locker has reported that the compositor shows only the lock (its "locked" line).
static LOCKED: AtomicBool = AtomicBool::new(false);

fn lock_now(running: &Arc<Mutex<Option<Child>>>) {
    let mut r = running.lock().unwrap();
    // already locked: the locker is still running
    if let Some(c) = r.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            return;
        }
    }
    *r = start();
    crate::log(if r.is_some() { "locked the screen" } else { "could not start yggdrasil-lock" });
    if r.is_some() {
        let run = running.clone();
        std::thread::spawn(move || watch(run));
    }
}

fn start() -> Option<Child> {
    LOCKED.store(false, Ordering::SeqCst);
    let mut c = Command::new("yggdrasil-lock").stdin(Stdio::null()).stdout(Stdio::piped()).spawn().ok()?;
    if let Some(out) = c.stdout.take() {
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(out).lines().map_while(Result::ok) {
                if line.trim() == "locked" {
                    LOCKED.store(true, Ordering::SeqCst);
                }
            }
        });
    }
    Some(c)
}

/// Asks logind to hold sleep back until we let go (a "delay" lock: logind waits at most InhibitDelayMaxSec, 5 s by default).
fn inhibit_sleep(mgr: &zbus::blocking::Proxy) -> Option<zbus::zvariant::OwnedFd> {
    mgr.call("Inhibit", &("sleep", "Yggdrasil", "Lock the screen before sleeping", "delay")).ok()
}

/// Locks, then waits (at most 3 s, inside logind's limit) until the locker reports the screen is locked, so the machine never
/// sleeps, and wakes, with the desktop showing.
fn lock_before_sleep(running: &Arc<Mutex<Option<Child>>>) {
    lock_now(running);
    let until = Instant::now() + Duration::from_secs(3);
    while !LOCKED.load(Ordering::SeqCst) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(25));
    }
    if !LOCKED.load(Ordering::SeqCst) {
        crate::log("the lock screen did not confirm in time; letting the machine sleep anyway");
    }
}

/// Waits for the locker. It ends with 0 only after the right password; ending any other way (a crash) would leave Hyprland's
/// "lock screen died" screen up, so it is started again (Hyprland's misc.allow_session_lock_restore lets the new one take over),
/// at most a few times a minute so a locker that cannot start does not loop.
fn watch(running: Arc<Mutex<Option<Child>>>) {
    let mut restarts: Vec<std::time::Instant> = vec![];
    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        let mut r = running.lock().unwrap();
        let Some(c) = r.as_mut() else { return };
        match c.try_wait() {
            Ok(None) => continue,
            Ok(Some(status)) if status.success() => {
                *r = None;
                return; // unlocked
            }
            _ => {}
        }
        restarts.retain(|t| t.elapsed() < std::time::Duration::from_secs(60));
        if restarts.len() >= 5 {
            crate::log("the lock screen keeps ending without a password; not starting it again");
            *r = None;
            return;
        }
        restarts.push(std::time::Instant::now());
        std::thread::sleep(std::time::Duration::from_millis(300));
        *r = start();
        crate::log(if r.is_some() { "the lock screen ended without the password: started it again" } else { "could not start yggdrasil-lock again" });
        if r.is_none() {
            return;
        }
    }
}

/// Listens for logind's Lock signal for this session and for the machine going to sleep. Runs on its own thread; returns at once.
pub fn spawn() {
    if !yggdrasil_conf::own_session() {
        return;
    }
    std::thread::spawn(|| {
        let running: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
        let Ok(sys) = zbus::blocking::Connection::system() else { return };
        let Ok(mgr) = zbus::blocking::Proxy::new(&sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager") else { return };
        let id = std::env::var("XDG_SESSION_ID").unwrap_or_default();
        let session: Option<zbus::zvariant::OwnedObjectPath> = if id.is_empty() { mgr.call("GetSessionByPID", &(std::process::id(),)).ok() } else { mgr.call("GetSession", &(id.as_str(),)).ok() };
        // the machine is about to sleep: lock first, so it wakes up locked
        {
            let (sys2, run2) = (sys.clone(), running.clone());
            std::thread::spawn(move || {
                let Ok(m) = zbus::blocking::Proxy::new(&sys2, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager") else { return };
                let Ok(sigs) = m.receive_signal("PrepareForSleep") else { return };
                let mut hold = inhibit_sleep(&m);
                if hold.is_none() {
                    crate::log("lock: logind would not hold sleep back; the screen may lock only after sleep starts");
                }
                for s in sigs {
                    if s.body().deserialize::<bool>().unwrap_or(false) {
                        lock_before_sleep(&run2);
                        hold = None; // let go: now it may sleep
                    } else if hold.is_none() {
                        hold = inhibit_sleep(&m); // woke up: hold the next sleep back too
                    }
                }
                drop(hold);
            });
        }
        let Some(path) = session else {
            crate::log("lock: cannot tell which login session this is; Super+L and the idle lock will not work");
            return;
        };
        let Ok(sess) = zbus::blocking::Proxy::new(&sys, "org.freedesktop.login1", path.as_str(), "org.freedesktop.login1.Session") else { return };
        let Ok(sigs) = sess.receive_signal("Lock") else { return };
        crate::log("lock: Yggdrasil's own session, yggdrasil-lock answers lock requests");
        for _ in sigs {
            lock_now(&running);
        }
    });
}
