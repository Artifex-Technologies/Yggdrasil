//! Wi-Fi & Network keeps looking for networks while its list (or a Wi-Fi network's own page, with its "last seen") is on
//! screen, as a phone's Wi-Fi settings do; there is no Scan button. Light on purpose:
//!
//! - `Scanner`: a thread that asks NetworkManager for a scan every `EVERY` (NetworkManager itself allows one about every
//!   10 s), reads the lists `READ_AFTER` later (sys::wifi_air: nmcli's list without a scan of its own, the kernel's scan list)
//!   and wakes the window with what it read. It sleeps on a condition variable in between, and does nothing at all while the
//!   page is closed, the window is in the background or minimised, a network is being joined, or Settings has quit.
//! - `Schedule`: when to scan and when to read, as a pure function of the time (tested without NetworkManager).
//! - `Shown`: the lists as shown, in a steady order: a network keeps its place when its signal changes, a new one is added at
//!   the end, one not heard in the last scan is faded, and one not heard for `FORGET_AFTER` goes. The row under the pointer
//!   stays where it is.

use std::collections::HashSet;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::sys::{self, Wifi};
use crate::Msg;
use eframe::egui;
use yggdrasil_wifiscan as scan;

/// How often a scan is asked for while the page is on screen (NetworkManager refuses one sooner than about 10 s after the last).
pub const EVERY: Duration = Duration::from_secs(12);
/// How long after asking the lists are read (a scan of every channel takes a few seconds).
pub const READ_AFTER: Duration = Duration::from_secs(5);
/// A network not heard for this long leaves the list.
pub const FORGET_AFTER: Duration = Duration::from_secs(120);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    Scan,
    Read,
}

/// When to ask for a scan and when to read what it found. Kept while the page is closed, so opening it again soon does not
/// ask sooner than `EVERY` after the last scan.
#[derive(Debug, Default)]
pub struct Schedule {
    last_scan: Option<Instant>,
    read_at: Option<Instant>,
}

impl Schedule {
    /// What to do at `now` (at most one step), and how long to wait before asking again.
    pub fn step(&mut self, now: Instant) -> (Option<Step>, Duration) {
        if self.last_scan.is_none_or(|t| now.saturating_duration_since(t) >= EVERY) {
            // a read still waiting is for an older scan: this one's read replaces it
            self.last_scan = Some(now);
            self.read_at = Some(now + READ_AFTER);
            return (Some(Step::Scan), READ_AFTER);
        }
        if self.read_at.is_some_and(|t| now >= t) {
            self.read_at = None;
            return (Some(Step::Read), self.wait(now));
        }
        (None, self.wait(now))
    }

    fn wait(&self, now: Instant) -> Duration {
        let scan = self.last_scan.map_or(Duration::ZERO, |t| (t + EVERY).saturating_duration_since(now));
        self.read_at.map_or(scan, |r| scan.min(r.saturating_duration_since(now)))
    }
}

#[derive(Default)]
struct Want {
    /// Looking (the page on screen, the window in front, nothing being joined), on these Wi-Fi adapters
    on: bool,
    devices: Vec<String>,
    quit: bool,
}

/// The thread that scans while the page is on screen; dropped when Settings quits.
pub struct Scanner {
    shared: Arc<(Mutex<Want>, Condvar)>,
}

impl Scanner {
    pub fn start(tx: Sender<Msg>, ctx: egui::Context) -> Scanner {
        let shared: Arc<(Mutex<Want>, Condvar)> = Arc::default();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("wifi-scan".into())
            .spawn(move || {
                let (lock, cv) = &*s;
                let mut plan = Schedule::default();
                let Ok(mut w) = lock.lock() else { return };
                loop {
                    if w.quit {
                        return;
                    }
                    if !w.on {
                        w = match cv.wait(w) {
                            Ok(w) => w,
                            Err(_) => return,
                        };
                        continue;
                    }
                    let (step, wait) = plan.step(Instant::now());
                    let Some(step) = step else {
                        // woken early only by a change of `Want`
                        w = match cv.wait_timeout(w, wait) {
                            Ok((w, _)) => w,
                            Err(_) => return,
                        };
                        continue;
                    };
                    let devices = w.devices.clone();
                    drop(w);
                    // a made-up system (a fixture) is never scanned, and its lists do not change
                    if crate::fixture::get().is_none() {
                        match step {
                            Step::Scan => {
                                let _ = yggdrasil_netjoin::scan::request();
                            }
                            Step::Read => {
                                let _ = tx.send(Msg::WifiAir(sys::wifi_air(&devices)));
                                ctx.request_repaint();
                            }
                        }
                    }
                    w = match lock.lock() {
                        Ok(w) => w,
                        Err(_) => return,
                    };
                }
            })
            .expect("a thread");
        Scanner { shared }
    }

    /// Look on these adapters while `on`; off, nothing runs (the thread waits). Only a change wakes the thread.
    pub fn set(&self, on: bool, devices: Vec<String>) {
        let (lock, cv) = &*self.shared;
        let Ok(mut w) = lock.lock() else { return };
        if w.on != on || (on && w.devices != devices) {
            w.on = on;
            if on {
                w.devices = devices;
            }
            cv.notify_one();
        }
    }

    /// Looking now (the page shows "Searching…").
    pub fn on(&self) -> bool {
        self.shared.0.lock().is_ok_and(|w| w.on)
    }
}

impl Drop for Scanner {
    fn drop(&mut self) {
        let (lock, cv) = &*self.shared;
        if let Ok(mut w) = lock.lock() {
            w.quit = true;
            cv.notify_one();
        }
    }
}

struct Row<T> {
    key: String,
    item: T,
    /// When it was last heard (or first listed, when NetworkManager listed it before the radio heard it here)
    heard: Instant,
    /// Heard in the last read
    fresh: bool,
}

/// One list in a steady order: each row keeps its place, new rows go at the end, a row not heard in the last read is faded
/// and one not heard for `FORGET_AFTER` goes (and stays gone while it is only remembered by NetworkManager, not heard).
pub struct Roster<T> {
    rows: Vec<Row<T>>,
    gone: HashSet<String>,
}

impl<T> Default for Roster<T> {
    fn default() -> Self {
        Roster { rows: vec![], gone: HashSet::new() }
    }
}

impl<T: Clone> Roster<T> {
    /// A new read: each item by its key, in the order to add new ones in, and whether the radio heard it in this read.
    pub fn update(&mut self, now: Instant, items: impl IntoIterator<Item = (String, T, bool)>) {
        for r in &mut self.rows {
            r.fresh = false;
        }
        let mut listed = HashSet::new();
        for (key, item, heard) in items {
            listed.insert(key.clone());
            match self.rows.iter_mut().find(|r| r.key == key) {
                Some(r) => {
                    r.item = item;
                    if heard {
                        r.heard = now;
                        r.fresh = true;
                    }
                }
                None if heard || !self.gone.contains(&key) => {
                    self.gone.remove(&key);
                    self.rows.push(Row { key, item, heard: now, fresh: heard });
                }
                None => {}
            }
        }
        let (keep, drop): (Vec<Row<T>>, Vec<Row<T>>) =
            std::mem::take(&mut self.rows).into_iter().partition(|r| now.saturating_duration_since(r.heard) < FORGET_AFTER);
        self.rows = keep;
        self.gone.extend(drop.into_iter().map(|r| r.key));
        // what is no longer listed at all can come back as new
        self.gone.retain(|k| listed.contains(k));
    }

    /// The rows in their order, each with whether it is faded (not heard in the last read).
    pub fn rows(&self) -> impl Iterator<Item = (&T, bool)> {
        self.rows.iter().map(|r| (&r.item, !r.fresh))
    }
}

/// The Wi-Fi page's lists as shown: the named networks (NetworkManager's list) and the access points that hide their name.
#[derive(Default)]
pub struct Shown {
    pub named: Roster<Wifi>,
    pub hidden: Roster<scan::Network>,
}

impl Shown {
    /// Takes a new read of the page. A named network counts as heard when the radio heard it in its last scan (NetworkManager
    /// keeps listing a network for minutes after it was last heard), or always when the radio's list could not be read.
    pub fn update(&mut self, now: Instant, n: &sys::Network) {
        let radio = n.air_error.is_empty() && !n.air.is_empty();
        self.named.update(now, n.networks.iter().map(|w| (w.ssid.clone(), w.clone(), !radio || w.in_use || n.air_for(&w.ssid).is_some())));
        self.hidden.update(now, crate::wifi_ui::hidden_heard(n).map(|a| (scan::mac_text(&a.best().bss.bssid), a.clone(), true)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(secs: u64) -> Duration {
        Duration::from_secs(secs)
    }

    #[test]
    fn a_scan_now_its_read_after_and_never_more_often_than_every() {
        let t0 = Instant::now();
        let mut p = Schedule::default();
        assert_eq!(p.step(t0), (Some(Step::Scan), READ_AFTER), "the page opened: look at once");
        assert_eq!(p.step(t0 + s(1)), (None, READ_AFTER - s(1)), "nothing until the read");
        assert_eq!(p.step(t0 + READ_AFTER), (Some(Step::Read), EVERY - READ_AFTER), "then read, and wait for the next scan");
        assert_eq!(p.step(t0 + s(6)), (None, EVERY - s(6)), "read once only");
        assert_eq!(p.step(t0 + EVERY), (Some(Step::Scan), READ_AFTER));
        assert!(EVERY >= s(10), "never sooner than NetworkManager allows");
        assert!(READ_AFTER < EVERY);
    }

    #[test]
    fn closing_and_opening_again_soon_does_not_scan_sooner() {
        let t0 = Instant::now();
        let mut p = Schedule::default();
        p.step(t0);
        // closed at 2 s (the thread stops asking), opened again at 4 s: no new scan before 12 s
        assert_eq!(p.step(t0 + s(4)), (None, s(1)), "the read of the scan already asked for is still due");
        assert_eq!(p.step(t0 + s(9)).0, Some(Step::Read), "back in front after the read was due: read at once");
        assert_eq!(p.step(t0 + s(9)), (None, s(3)));
        // in the background for a minute: back in front, the old read is not wanted; a new scan is, at once
        let mut p = Schedule::default();
        p.step(t0);
        assert_eq!(p.step(t0 + s(60)), (Some(Step::Scan), READ_AFTER));
        assert_eq!(p.step(t0 + s(61)), (None, s(4)));
    }

    fn wifi(ssid: &str, signal: u8) -> (String, Wifi, bool) {
        (ssid.into(), Wifi { in_use: false, ssid: ssid.into(), signal, secured: true }, true)
    }

    fn shown(r: &Roster<Wifi>) -> Vec<(String, u8, bool)> {
        r.rows().map(|(w, faded)| (w.ssid.clone(), w.signal, faded)).collect()
    }

    #[test]
    fn rows_keep_their_place_new_ones_come_last_and_the_unheard_fade_then_go() {
        let t0 = Instant::now();
        let mut r: Roster<Wifi> = Roster::default();
        r.update(t0, [wifi("Home", 80), wifi("Cafe", 60)]);
        assert_eq!(shown(&r), [("Home".into(), 80, false), ("Cafe".into(), 60, false)]);
        // the cafe is stronger now, and a new network is heard: nothing moves, the new one is added at the end
        r.update(t0 + s(12), [wifi("Cafe", 90), wifi("Library", 95), wifi("Home", 70)]);
        assert_eq!(shown(&r), [("Home".into(), 70, false), ("Cafe".into(), 90, false), ("Library".into(), 95, false)]);
        // the cafe not heard: faded in its place, with what was last heard of it
        r.update(t0 + s(24), [wifi("Home", 70), wifi("Library", 95)]);
        assert_eq!(shown(&r), [("Home".into(), 70, false), ("Cafe".into(), 90, true), ("Library".into(), 95, false)]);
        // heard again: back as it was, in the same place
        r.update(t0 + s(36), [wifi("Home", 70), wifi("Cafe", 50), wifi("Library", 95)]);
        assert_eq!(shown(&r)[1], ("Cafe".into(), 50, false));
        // not heard for two minutes: gone
        r.update(t0 + s(36) + FORGET_AFTER - s(1), [wifi("Home", 70), wifi("Library", 95)]);
        assert_eq!(shown(&r).len(), 3, "faded until two minutes");
        r.update(t0 + s(36) + FORGET_AFTER, [wifi("Home", 70), wifi("Library", 95)]);
        assert_eq!(shown(&r).iter().map(|x| x.0.as_str()).collect::<Vec<_>>(), ["Home", "Library"]);
    }

    #[test]
    fn listed_but_not_heard_is_faded_and_once_gone_stays_gone_until_heard() {
        let t0 = Instant::now();
        let mut r: Roster<Wifi> = Roster::default();
        let stale = |ssid: &str| (ssid.to_string(), Wifi { in_use: false, ssid: ssid.into(), signal: 30, secured: false }, false);
        // NetworkManager still lists one the radio has not heard: shown, faded
        r.update(t0, [wifi("Home", 80), stale("Old")]);
        assert_eq!(shown(&r)[1], ("Old".into(), 30, true));
        // still only listed after two minutes: gone, and it does not come back while it is only listed
        r.update(t0 + FORGET_AFTER, [wifi("Home", 80), stale("Old")]);
        r.update(t0 + FORGET_AFTER + s(12), [wifi("Home", 80), stale("Old")]);
        assert_eq!(shown(&r).len(), 1);
        // heard again: back, at the end
        r.update(t0 + FORGET_AFTER + s(24), [wifi("Home", 80), wifi("Old", 40)]);
        assert_eq!(shown(&r)[1], ("Old".into(), 40, false));
        // gone from NetworkManager's list too, then listed again later: a new row, faded until heard
        r.update(t0 + 3 * FORGET_AFTER, [wifi("Home", 80)]);
        r.update(t0 + 3 * FORGET_AFTER + s(12), [wifi("Home", 80), stale("Old")]);
        assert_eq!(shown(&r)[1], ("Old".into(), 30, true));
    }
}
