//! Locking the screen in Yggdrasil's own session: logind asks the session to lock (Super+L, `loginctl lock-session`, the idle timer)
//! or the machine is about to sleep, and this starts `yggdrasil-lock` (once; it exits when the right password is typed). In another
//! desktop's session (Plasma) that desktop's own lock screen answers those requests, so this stays out of it.

use std::process::{Child, Command};
use std::sync::{Arc, Mutex};

fn lock_now(running: &Arc<Mutex<Option<Child>>>) {
    let mut r = running.lock().unwrap();
    // already locked: the locker is still running
    if let Some(c) = r.as_mut() {
        if matches!(c.try_wait(), Ok(None)) {
            return;
        }
    }
    *r = Command::new("yggdrasil-lock").stdin(std::process::Stdio::null()).spawn().ok();
    crate::log(if r.is_some() { "locked the screen" } else { "could not start yggdrasil-lock" });
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
                for s in sigs {
                    if s.body().deserialize::<bool>().unwrap_or(false) {
                        lock_now(&run2);
                    }
                }
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
