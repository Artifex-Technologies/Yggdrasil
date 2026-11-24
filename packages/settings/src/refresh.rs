//! Settings draws nothing while nothing changes. egui draws a whole new picture each time the window wakes, and under a
//! compositor on a GPU each picture is a new buffer to compose: before 2026-11-15 Settings woke every few seconds on every page
//! (and every second for the text size) only to look whether something had changed elsewhere.
//!
//! - `Refresher`: the open page is read again now and then on a thread of its own, and the window is woken only when what was
//!   read differs from what it shows (or when it has not read the page yet).
//! - `watch_config`: the settings folder (`~/.config/yggdrasil`) is watched with inotify; a file written there (the text size
//!   and reduced motion in panel.conf, the style and accent, a page's file changed by the panel or a command) wakes the window.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::Msg;
use eframe::egui;

/// What the open page needs read: the messages it is given (sent on `tx` only when they change).
pub type Read = Arc<dyn Fn(&Sender<Msg>) -> Vec<Msg> + Send + Sync>;

#[derive(Default)]
struct Want {
    /// What is read now (a page, or a part of one) and how often; None: nothing
    key: Option<String>,
    every: Duration,
    read: Option<Read>,
    /// Not now: something is being dragged or typed, or the window is in the background
    paused: bool,
    /// What each was last seen as, so the same again wakes nothing
    seen: HashMap<String, u64>,
    last_read: Option<Instant>,
}

#[derive(Clone)]
pub struct Refresher {
    shared: Arc<(Mutex<Want>, Condvar)>,
}

/// What a page's messages say, in short.
pub fn fingerprint(msgs: &[Msg]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    format!("{msgs:?}").hash(&mut h);
    h.finish()
}

impl Refresher {
    pub fn start(tx: Sender<Msg>, ctx: egui::Context) -> Refresher {
        let shared: Arc<(Mutex<Want>, Condvar)> = Arc::default();
        let s = shared.clone();
        std::thread::Builder::new()
            .name("refresh".into())
            .spawn(move || {
                let (lock, cv) = &*s;
                let Ok(mut w) = lock.lock() else { return };
                loop {
                    let ready = !w.paused && w.read.is_some();
                    if !ready {
                        w = match cv.wait(w) {
                            Ok(w) => w,
                            Err(_) => return,
                        };
                        continue;
                    }
                    let due = w.last_read.map_or(Duration::ZERO, |t| w.every.saturating_sub(t.elapsed()));
                    if !due.is_zero() {
                        w = match cv.wait_timeout(w, due) {
                            Ok((w, _)) => w,
                            Err(_) => return,
                        };
                        continue;
                    }
                    let (Some(key), Some(read)) = (w.key.clone(), w.read.clone()) else { continue };
                    w.last_read = Some(Instant::now());
                    drop(w);
                    let msgs = read(&tx);
                    let fp = fingerprint(&msgs);
                    w = match lock.lock() {
                        Ok(w) => w,
                        Err(_) => return,
                    };
                    if w.key.as_deref() == Some(key.as_str()) && w.seen.get(&key) != Some(&fp) {
                        w.seen.insert(key, fp);
                        for m in msgs {
                            let _ = tx.send(m);
                        }
                        ctx.request_repaint();
                    }
                }
            })
            .expect("a thread");
        Refresher { shared }
    }

    /// Read `key` every `every` with `read` (the same key as last time changes nothing); None: read nothing.
    pub fn want(&self, want: Option<(String, Duration, Read)>) {
        let (lock, cv) = &*self.shared;
        let Ok(mut w) = lock.lock() else { return };
        let key = want.as_ref().map(|(k, ..)| k.clone());
        if w.key == key {
            return;
        }
        w.key = key;
        // the page was just read as it opened (`seen`): its next look is a whole period away
        w.last_read = Some(Instant::now());
        (w.every, w.read) = match want {
            Some((_, every, read)) => (every, Some(read)),
            None => (Duration::ZERO, None),
        };
        cv.notify_one();
    }

    /// Hold the looks while something is being dragged or typed, or the window is in the background.
    pub fn pause(&self, paused: bool) {
        let (lock, cv) = &*self.shared;
        let Ok(mut w) = lock.lock() else { return };
        if w.paused != paused {
            w.paused = paused;
            cv.notify_one();
        }
    }

    /// The page `key` was read (as it opened, or after a change made here): what it shows now.
    pub fn seen(&self, key: &str, msgs: &[Msg]) {
        let fp = fingerprint(msgs);
        if let Ok(mut w) = self.shared.0.lock() {
            w.seen.insert(key.to_string(), fp);
        }
    }
}

/// Wake the window when a file in the settings folder is written, made, removed or renamed; `changed` is set then (the
/// window takes it with `swap(false)`). Nothing runs while nothing is written: the thread waits in `read`.
pub fn watch_config(ctx: egui::Context, changed: Arc<AtomicBool>) {
    let dir = yggdrasil_conf::dirs::config();
    let _ = std::fs::create_dir_all(&dir);
    let Ok(path) = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.as_os_str())) else { return };
    // SAFETY: inotify on a descriptor of our own, read into a buffer of our own
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    if fd < 0 {
        return;
    }
    let mask = libc::IN_CLOSE_WRITE | libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM;
    if unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) } < 0 {
        unsafe { libc::close(fd) };
        return;
    }
    let _ = std::thread::Builder::new().name("config-watch".into()).spawn(move || {
        let mut buf = [0u8; 4096];
        loop {
            let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
            if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            if n <= 0 {
                return;
            }
            changed.store(true, Ordering::Release);
            ctx.request_repaint();
        }
    });
}

/// Text size, reduced motion and high contrast from Settings > Accessibility (panel.conf): set at the start and again when the settings folder
/// changes (`watch_config`), instead of the shared look's once-a-second check, which kept the window waking.
pub fn apply_accessibility(ctx: &egui::Context) {
    let c = yggdrasil_conf::PanelConf::load();
    let zoom = f32::from(c.text_scale.clamp(yggdrasil_conf::TEXT_SCALE_MIN, yggdrasil_conf::TEXT_SCALE_MAX)) / 100.0;
    if (ctx.zoom_factor() - zoom).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
    let anim = if c.reduce_motion { 0.0 } else { 1.0 / 12.0 };
    ctx.all_styles_mut(|s| s.animation_time = anim);
    // Settings applies its look every frame, from palettes that follow this
    if yggdrasil_ui::theme::set_high_contrast(c.high_contrast) {
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    fn reader(value: Arc<Mutex<bool>>, reads: Arc<Mutex<u32>>) -> Read {
        Arc::new(move |_| {
            *reads.lock().unwrap() += 1;
            vec![Msg::Firewall(*value.lock().unwrap())]
        })
    }

    #[test]
    fn a_page_read_again_wakes_the_window_only_when_it_changed() {
        let (tx, rx) = channel();
        let r = Refresher::start(tx, egui::Context::default());
        let (value, reads) = (Arc::new(Mutex::new(true)), Arc::new(Mutex::new(0)));
        // as the page opened: read, shown
        r.seen("firewall", &[Msg::Firewall(true)]);
        r.want(Some(("firewall".into(), Duration::from_millis(40), reader(value.clone(), reads.clone()))));
        std::thread::sleep(Duration::from_millis(300));
        assert!(*reads.lock().unwrap() >= 3, "read again now and then");
        assert!(rx.try_recv().is_err(), "the same again: nothing sent, the window not woken");
        // changed elsewhere: sent once
        *value.lock().unwrap() = false;
        let m = rx.recv_timeout(Duration::from_secs(2)).expect("the change is sent");
        assert!(matches!(m, Msg::Firewall(false)));
        std::thread::sleep(Duration::from_millis(200));
        assert!(rx.try_recv().is_err(), "and only once");
        // paused (a drag, the window in the background): not read
        r.pause(true);
        std::thread::sleep(Duration::from_millis(100));
        let n = *reads.lock().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*reads.lock().unwrap(), n, "no looks while paused");
        r.pause(false);
        std::thread::sleep(Duration::from_millis(200));
        assert!(*reads.lock().unwrap() > n, "again after");
        // another page: nothing read for this one any more
        r.want(None);
        std::thread::sleep(Duration::from_millis(100));
        let n = *reads.lock().unwrap();
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(*reads.lock().unwrap(), n);
    }
}
