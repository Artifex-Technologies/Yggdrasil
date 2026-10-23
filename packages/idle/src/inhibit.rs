//! "Please don't sleep": programs (video players, presentation tools, downloads) ask the desktop not to blank the screen or go to
//! sleep while they work, through `org.freedesktop.PowerManagement.Inhibit`. Each request becomes a real logind lock ("idle:sleep",
//! block mode, held by this program in the requester's name), so every part of the system sees it, idle.rs's own checks included.
//! The lock goes when the program says so (UnInhibit) or when it quits.
//!
//! While another program holds the name (KDE's PowerDevil), this waits and takes over the moment it is free.

use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use zbus::object_server::SignalEmitter;

pub const NAME: &str = "org.freedesktop.PowerManagement";
pub const PATH: &str = "/org/freedesktop/PowerManagement/Inhibit";

struct Held {
    /// Who asked (their bus name), so the lock goes when they quit.
    owner: String,
    _lock: OwnedFd,
}

#[derive(Clone, Default)]
struct Inhibit {
    held: Arc<Mutex<HashMap<u32, Held>>>,
    next: Arc<Mutex<u32>>,
}

/// A logind block lock on idling and sleeping, in `who`'s name; dropping the descriptor releases it.
fn take_lock(who: &str, why: &str) -> zbus::Result<OwnedFd> {
    let sys = zbus::blocking::Connection::system()?;
    let login = zbus::blocking::Proxy::new(&sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager")?;
    let fd: zbus::zvariant::OwnedFd = login.call("Inhibit", &("idle:sleep", clean(who, 60).as_str(), clean(why, 120).as_str(), "block"))?;
    Ok(fd.into())
}

/// Program names and reasons come from other programs: plain text, short.
pub fn clean(s: &str, max: usize) -> String {
    let t: String = s.chars().filter(|c| !c.is_control()).take(max).collect();
    if t.trim().is_empty() { "an application".into() } else { t }
}

#[zbus::interface(name = "org.freedesktop.PowerManagement.Inhibit")]
impl Inhibit {
    async fn inhibit(&self, application: String, reason: String, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) -> zbus::fdo::Result<u32> {
        let owner = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        let lock = blocking::unblock(move || take_lock(&application, &reason)).await.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let cookie = {
            let mut n = self.next.lock().unwrap();
            *n = n.wrapping_add(1).max(1);
            *n
        };
        let first = {
            let mut h = self.held.lock().unwrap();
            let first = h.is_empty();
            h.insert(cookie, Held { owner, _lock: lock });
            first
        };
        if first {
            let _ = Self::has_inhibit_changed(&emitter, true).await;
        }
        Ok(cookie)
    }

    async fn un_inhibit(&self, cookie: u32, #[zbus(header)] hdr: zbus::message::Header<'_>, #[zbus(signal_emitter)] emitter: SignalEmitter<'_>) {
        let sender = hdr.sender().map(|s| s.to_string()).unwrap_or_default();
        let now_empty = {
            let mut h = self.held.lock().unwrap();
            // only the program that took a lock may give it back
            if h.get(&cookie).is_some_and(|x| x.owner == sender) {
                h.remove(&cookie);
                h.is_empty()
            } else {
                false
            }
        };
        if now_empty {
            let _ = Self::has_inhibit_changed(&emitter, false).await;
        }
    }

    fn has_inhibit(&self) -> bool {
        !self.held.lock().unwrap().is_empty()
    }

    #[zbus(signal)]
    async fn has_inhibit_changed(emitter: &SignalEmitter<'_>, has_inhibit: bool) -> zbus::Result<()>;
}

impl Inhibit {
    /// A program left the bus: its locks go. Returns whether none are left now (and some were before).
    fn forget(&self, owner: &str) -> bool {
        let mut h = self.held.lock().unwrap();
        let before = h.len();
        h.retain(|_, x| x.owner != owner);
        before > 0 && h.is_empty()
    }
}

/// Serves the interface from its own thread (waiting while PowerDevil holds the name). Returns at once.
pub fn spawn() {
    std::thread::spawn(|| loop {
        let i = Inhibit::default();
        let built = zbus::blocking::connection::Builder::session().and_then(|b| b.name(NAME)).and_then(|b| b.serve_at(PATH, i.clone())).and_then(|b| b.build());
        let Ok(conn) = built else {
            std::thread::sleep(Duration::from_secs(10));
            continue;
        };
        crate::log("serving org.freedesktop.PowerManagement.Inhibit");
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else { return };
        let Ok(changes) = dbus.receive_name_owner_changed() else { return };
        for sig in changes {
            let Ok(args) = sig.args() else { continue };
            if args.new_owner().is_none() && i.forget(args.name().as_str()) {
                if let Ok(iface) = conn.object_server().interface::<_, Inhibit>(PATH) {
                    let _ = zbus::block_on(Inhibit::has_inhibit_changed(iface.signal_emitter(), false));
                }
            }
        }
        return;
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Against the running desktop: `cargo test -- --ignored live_lock` (needs yggdrasil-idle serving the interface).
    #[test]
    #[ignore]
    fn live_lock_is_held_while_asked_and_released_after() {
        let c = zbus::blocking::Connection::session().unwrap();
        let p = zbus::blocking::Proxy::new(&c, NAME, PATH, "org.freedesktop.PowerManagement.Inhibit").unwrap();
        let cookie: u32 = p.call("Inhibit", &("LiveTestPlayer", "Playing a video")).unwrap();
        let list = || -> Vec<(String, String, String, String, u32, u32)> {
            let sys = zbus::blocking::Connection::system().unwrap();
            zbus::blocking::Proxy::new(&sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").unwrap().call("ListInhibitors", &()).unwrap()
        };
        let now = list();
        assert!(now.iter().any(|l| l.1 == "LiveTestPlayer" && l.0.contains("idle") && l.0.contains("sleep") && l.3 == "block"), "a block lock on idle and sleep: {now:?}");
        assert!(p.call::<_, _, bool>("HasInhibit", &()).unwrap());
        let _: () = p.call("UnInhibit", &(cookie,)).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(!list().iter().any(|l| l.1 == "LiveTestPlayer"), "gone after UnInhibit");
    }

    #[test]
    fn names_from_other_programs_are_tamed() {
        assert_eq!(clean("VLC\nmedia\x07 player", 60), "VLCmedia player");
        assert_eq!(clean("   ", 60), "an application");
        assert_eq!(clean(&"x".repeat(500), 60).len(), 60);
    }

    #[test]
    fn a_program_that_quits_loses_its_locks() {
        let i = Inhibit::default();
        let fd = || OwnedFd::from(std::fs::File::open("/dev/null").unwrap());
        i.held.lock().unwrap().insert(1, Held { owner: ":1.5".into(), _lock: fd() });
        i.held.lock().unwrap().insert(2, Held { owner: ":1.6".into(), _lock: fd() });
        assert!(!i.forget(":1.5"), "another program still holds one");
        assert!(i.has_inhibit());
        assert!(i.forget(":1.6"));
        assert!(!i.has_inhibit());
    }
}
