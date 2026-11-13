//! A laptop's lid (Settings > Power > When the lid is closed). While the session runs, yggdrasil-idle holds logind's
//! "handle-lid-switch" lock, so logind leaves the lid to it, and then:
//!
//! - closed: the screen locks at once (`lid_lock`, on by default);
//! - still closed after `lid_sleep` seconds (300 by default; 0 = never): the computer sleeps. With `lid_hibernate` set (1800 by
//!   default, counted from closing; 0 = never) it is logind's suspend-then-hibernate, which wakes the computer once to hibernate
//!   when the system's HibernateDelaySec has passed (Settings keeps that at lid_hibernate - lid_sleep, through the battery
//!   helper); a `lid_hibernate` no later than `lid_sleep` hibernates straight away;
//! - opened: whatever was still to come is called off;
//! - docked (an external screen, logind's Docked): only the lock, the computer stays awake.
//!
//! When yggdrasil-idle is not running the lock goes with it and logind's own lid handling (sleep at once) is back, so a closed
//! laptop never stays awake by mistake. A computer without a lid (no /proc/acpi/button/lid) gets none of this.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// What to do with the lid, from idle.conf (seconds).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LidConf {
    pub lock: bool,
    pub sleep: u32,
    pub hibernate: u32,
}

impl Default for LidConf {
    fn default() -> Self {
        LidConf { lock: true, sleep: 300, hibernate: 1800 }
    }
}

impl LidConf {
    pub fn parse(text: &str) -> LidConf {
        let mut c = LidConf::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "lid_lock" => c.lock = !matches!(v, "0" | "no" | "false" | "off"),
                "lid_sleep" => c.sleep = v.parse().unwrap_or(c.sleep),
                "lid_hibernate" => c.hibernate = v.parse().unwrap_or(c.hibernate),
                _ => {}
            }
        }
        c
    }
}

/// What the lid asks for now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Act {
    Nothing,
    Lock,
    Suspend,
    SuspendThenHibernate,
    Hibernate,
}

/// The lid's state over time: when it closed, and what has been done since.
#[derive(Debug, Default)]
pub struct Lid {
    closed_at: Option<Instant>,
    locked: bool,
    slept: bool,
}

impl Lid {
    /// One reading: the lid closed or not, docked or not, at `now`.
    pub fn step(&mut self, c: &LidConf, closed: bool, docked: bool, now: Instant) -> Act {
        if !closed {
            *self = Lid::default();
            return Act::Nothing;
        }
        let at = *self.closed_at.get_or_insert(now);
        if c.lock && !self.locked {
            self.locked = true;
            return Act::Lock;
        }
        if docked || self.slept || c.sleep == 0 || now.duration_since(at) < Duration::from_secs(c.sleep.into()) {
            return Act::Nothing;
        }
        self.slept = true;
        if c.hibernate == 0 {
            Act::Suspend
        } else if c.hibernate <= c.sleep {
            Act::Hibernate
        } else {
            Act::SuspendThenHibernate
        }
    }

    /// The computer woke up with the lid still closed (a timer, a plugged-in charger): count again from now, so it goes back to
    /// sleep after `lid_sleep` instead of staying awake in a bag.
    pub fn woke(&mut self, now: Instant) {
        if self.closed_at.is_some() {
            self.closed_at = Some(now);
            self.slept = false;
        }
    }
}

/// The lid's state file (`/proc/acpi/button/lid/LID0/state`: "state:      closed").
pub fn lid_file(root: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path().join("state")).filter(|p| p.exists()).collect();
    v.sort();
    v.into_iter().next()
}

pub fn closed(state: &str) -> bool {
    state.contains("closed")
}

fn manager(sys: &zbus::blocking::Connection) -> Option<zbus::blocking::Proxy<'static>> {
    zbus::blocking::Proxy::new(sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").ok()
}

/// Watches the lid on a thread of its own; returns at once. `conf` is idle.conf, read again at each closing.
pub fn spawn(conf: PathBuf) {
    if !yggdrasil_conf::own_session() {
        return;
    }
    let Some(state) = lid_file(Path::new("/proc/acpi/button/lid")) else { return };
    std::thread::spawn(move || {
        let Ok(sys) = zbus::blocking::Connection::system() else { return };
        let Some(mgr) = manager(&sys) else { return };
        // logind leaves the lid to us while we hold this; it goes when we stop, and logind's own handling is back
        let hold: Option<zbus::zvariant::OwnedFd> = mgr
            .call(
                "Inhibit",
                &("handle-lid-switch", "Yggdrasil", "The lid: lock, then sleep and hibernate as Settings > Power says", "block"),
            )
            .ok();
        if hold.is_none() {
            crate::log("lid: logind would not leave the lid to us; it handles the lid itself");
            return;
        }
        crate::log("lid: handled here (lock, sleep, hibernate as idle.conf says)");
        let mut lid = Lid::default();
        let mut c = LidConf::default();
        let mut was_closed = false;
        loop {
            let now = Instant::now();
            let is_closed = std::fs::read_to_string(&state).map(|s| closed(&s)).unwrap_or(false);
            if is_closed && !was_closed {
                c = LidConf::parse(&std::fs::read_to_string(&conf).unwrap_or_default());
            }
            was_closed = is_closed;
            let docked = is_closed && mgr.get_property::<bool>("Docked").unwrap_or(false);
            let act = lid.step(&c, is_closed, docked, now);
            match act {
                Act::Nothing => {}
                Act::Lock => {
                    crate::log("lid: closed, locking");
                    let _ = std::process::Command::new("loginctl").arg("lock-session").status();
                }
                Act::Suspend | Act::SuspendThenHibernate | Act::Hibernate => {
                    let method = match act {
                        Act::Suspend => "Suspend",
                        Act::Hibernate => "Hibernate",
                        _ => "SuspendThenHibernate",
                    };
                    crate::log(&format!("lid: closed {} s, {method}", c.sleep));
                    // returns once the computer is awake again
                    let _ = mgr.call_method(method, &(false,));
                    lid.woke(Instant::now());
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
        #[allow(unreachable_code)]
        drop(hold);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closing_locks_then_sleeps_and_opening_calls_it_off() {
        let c = LidConf::default();
        let t = Instant::now();
        let s = |n: u64| t + Duration::from_secs(n);
        let mut lid = Lid::default();
        assert_eq!(lid.step(&c, true, false, s(0)), Act::Lock);
        assert_eq!(lid.step(&c, true, false, s(299)), Act::Nothing);
        assert_eq!(lid.step(&c, true, false, s(300)), Act::SuspendThenHibernate, "5 min, hibernate at 30");
        assert_eq!(lid.step(&c, true, false, s(301)), Act::Nothing, "once");
        // opened before the 5 minutes: nothing more, and closing again starts over
        let mut lid = Lid::default();
        lid.step(&c, true, false, s(0));
        assert_eq!(lid.step(&c, false, false, s(100)), Act::Nothing);
        assert_eq!(lid.step(&c, true, false, s(101)), Act::Lock);
        assert_eq!(lid.step(&c, true, false, s(300)), Act::Nothing, "counted from the second closing");
        assert_eq!(lid.step(&c, true, false, s(401)), Act::SuspendThenHibernate);
    }

    #[test]
    fn docked_only_locks_and_the_settings_are_followed() {
        let t = Instant::now();
        let mut lid = Lid::default();
        let c = LidConf::default();
        assert_eq!(lid.step(&c, true, true, t), Act::Lock);
        assert_eq!(lid.step(&c, true, true, t + Duration::from_secs(4000)), Act::Nothing, "an external screen: stays awake");
        let only_sleep = LidConf { lock: false, sleep: 1, hibernate: 0 };
        let mut lid = Lid::default();
        assert_eq!(lid.step(&only_sleep, true, false, t), Act::Nothing);
        assert_eq!(lid.step(&only_sleep, true, false, t + Duration::from_secs(1)), Act::Suspend);
        let straight = LidConf { lock: true, sleep: 600, hibernate: 600 };
        let mut lid = Lid::default();
        lid.step(&straight, true, false, t);
        assert_eq!(lid.step(&straight, true, false, t + Duration::from_secs(600)), Act::Hibernate);
        let never = LidConf { lock: true, sleep: 0, hibernate: 1800 };
        let mut lid = Lid::default();
        lid.step(&never, true, false, t);
        assert_eq!(lid.step(&never, true, false, t + Duration::from_secs(9999)), Act::Nothing, "sleep never: stays awake");
    }

    #[test]
    fn waking_with_the_lid_closed_sleeps_again_later() {
        let t = Instant::now();
        let c = LidConf::default();
        let mut lid = Lid::default();
        lid.step(&c, true, false, t);
        lid.step(&c, true, false, t + Duration::from_secs(300));
        lid.woke(t + Duration::from_secs(1000));
        assert_eq!(lid.step(&c, true, false, t + Duration::from_secs(1200)), Act::Nothing);
        assert_eq!(lid.step(&c, true, false, t + Duration::from_secs(1300)), Act::SuspendThenHibernate);
    }

    #[test]
    fn the_conf_and_the_lid_file() {
        assert_eq!(LidConf::parse(""), LidConf { lock: true, sleep: 300, hibernate: 1800 });
        assert_eq!(
            LidConf::parse("mode = act\nlid_lock = 0\nlid_sleep = 60\nlid_hibernate = 0\n"),
            LidConf { lock: false, sleep: 60, hibernate: 0 }
        );
        assert!(closed("state:      closed\n") && !closed("state:      open\n"));
    }
}
