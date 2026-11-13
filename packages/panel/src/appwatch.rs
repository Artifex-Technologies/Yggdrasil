//! The app folders and the icon packs' folders, watched (inotify) instead of looked at every 4 s: an app installed or removed, or
//! an icon file added or replaced, is taken up once the burst of files a package install writes has settled (`QUIET`).
//!
//! A folder that is not there yet (Flatpak's exports before the first Flatpak app, an icon pack not installed) is waited for: the
//! nearest folder above it that is there is watched for new folders, and the watch moves down as they appear. When the inotify
//! watch limit is reached, the watch gives up and says so once (`start`'s `on_limit`); the panel then looks every 4 s, as before.

use std::cell::RefCell;
use std::collections::HashMap;
use std::os::fd::OwnedFd;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use calloop::generic::Generic;
use calloop::timer::{TimeoutAction, Timer};
use calloop::{Interest, LoopHandle, Mode, PostAction, RegistrationToken};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};

use crate::paint::ICON_CONTEXTS;

/// A burst is over when no file has changed for this long.
pub const QUIET: Duration = Duration::from_millis(500);
/// A burst that goes on is taken up at least this often.
pub const LONGEST: Duration = Duration::from_secs(5);

/// What is watched.
#[derive(Clone, Debug, Default)]
pub struct Places {
    /// Folders of desktop entries (their files only).
    pub apps: Vec<PathBuf>,
    /// The icon packs' folders: each with its folders below, two deep where one of them is an icon context ("48x48/apps").
    pub themes: Vec<PathBuf>,
    /// Folders of loose icon files (their files only).
    pub icons: Vec<PathBuf>,
}

/// What changed in a burst.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Changed {
    pub apps: bool,
    pub icons: bool,
}

impl Changed {
    fn any(self) -> bool {
        self.apps || self.icons
    }
    fn add(&mut self, o: Changed) {
        self.apps |= o.apps;
        self.icons |= o.icons;
    }
}

// what a watched folder is for (one folder can be several)
const APPS: u8 = 1;
const ICONS: u8 = 2;
/// above a folder that is not there yet: only new folders matter
const ABOVE: u8 = 4;

fn kinds_changed(k: u8) -> Changed {
    Changed { apps: k & APPS != 0, icons: k & ICONS != 0 }
}

/// The watch's errors that mean "no more watches" rather than "not this folder".
fn is_limit(e: rustix::io::Errno) -> bool {
    e == rustix::io::Errno::NOSPC || e == rustix::io::Errno::MFILE || e == rustix::io::Errno::NFILE || e == rustix::io::Errno::NOMEM
}

/// The folders to watch now, each once (by where it really is), with what each is for.
fn wanted(p: &Places) -> Vec<(PathBuf, u8)> {
    let mut v: Vec<(PathBuf, PathBuf, u8)> = vec![];
    let mut add = |d: &Path, k: u8| {
        let real = std::fs::canonicalize(d).unwrap_or_else(|_| d.to_path_buf());
        match v.iter_mut().find(|e| e.1 == real) {
            Some(e) => e.2 |= k,
            None => v.push((d.to_path_buf(), real, k)),
        }
    };
    let above = |d: &Path, add: &mut dyn FnMut(&Path, u8)| {
        if let Some(a) = d.ancestors().skip(1).find(|a| a.is_dir()) {
            add(a, ABOVE);
        }
    };
    let subdirs = |d: &Path| -> Vec<PathBuf> {
        let mut s: Vec<PathBuf> =
            std::fs::read_dir(d).map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        s.sort();
        s
    };
    let context = |p: &Path| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| ICON_CONTEXTS.contains(&n));
    for (dirs, k) in [(&p.apps, APPS), (&p.icons, ICONS)] {
        for d in dirs {
            if d.is_dir() {
                add(d, k);
            } else {
                above(d, &mut add);
            }
        }
    }
    for t in &p.themes {
        if !t.is_dir() {
            above(t, &mut add);
            continue;
        }
        add(t, ICONS);
        for a in subdirs(t) {
            add(&a, ICONS);
            for b in subdirs(&a) {
                if context(&a) || context(&b) {
                    add(&b, ICONS);
                }
            }
        }
    }
    v.into_iter().map(|(d, _, k)| (d, k)).collect()
}

/// When a burst of changes is taken up: `QUIET` after its last change, or `LONGEST` after its first.
#[derive(Default)]
struct Burst {
    first: Option<Instant>,
    last: Option<Instant>,
}

impl Burst {
    fn note(&mut self, now: Instant) {
        self.first.get_or_insert(now);
        self.last = Some(now);
    }
    fn due(&self) -> Option<Instant> {
        Some((self.last? + QUIET).min(self.first? + LONGEST))
    }
}

type OnChange<S> = Box<dyn FnMut(&mut S, Changed)>;
type OnLimit = Box<dyn FnOnce(std::io::Error)>;

struct Inner<S> {
    fd: Rc<OwnedFd>,
    places: Places,
    /// watch descriptor -> (folder, what it is for)
    wds: HashMap<i32, (PathBuf, u8)>,
    pending: Changed,
    burst: Burst,
    /// a timer is waiting for the burst to settle
    armed: bool,
    on_change: Option<OnChange<S>>,
    on_limit: Option<OnLimit>,
    /// gave up (the limit): no watching any more
    stopped: bool,
}

impl<S> Inner<S> {
    /// Watch what is to be watched now and stop watching the rest. What a folder newly watched is for counts as changed (its files
    /// may have come before the watch did), except on the first look.
    fn resync(&mut self, first: bool) -> Result<Changed, std::io::Error> {
        let mut now: HashMap<i32, (PathBuf, u8)> = HashMap::new();
        let mut fresh = Changed::default();
        for (dir, k) in wanted(&self.places) {
            let mask = if k & (APPS | ICONS) != 0 {
                WatchFlags::CLOSE_WRITE
                    | WatchFlags::MOVED_TO
                    | WatchFlags::MOVED_FROM
                    | WatchFlags::CREATE
                    | WatchFlags::DELETE
                    | WatchFlags::DELETE_SELF
                    | WatchFlags::MOVE_SELF
                    | WatchFlags::ONLYDIR
            } else {
                WatchFlags::CREATE | WatchFlags::MOVED_TO | WatchFlags::DELETE_SELF | WatchFlags::MOVE_SELF | WatchFlags::ONLYDIR
            };
            match inotify::add_watch(&*self.fd, &dir, mask) {
                Ok(wd) => {
                    if !first && self.wds.get(&wd).is_none_or(|(_, old)| old & (APPS | ICONS) != k & (APPS | ICONS)) {
                        fresh.add(kinds_changed(k));
                    }
                    now.insert(wd, (dir, k));
                }
                Err(e) if is_limit(e) => return Err(e.into()),
                // gone again, or not readable: not this one
                Err(_) => {}
            }
        }
        for wd in self.wds.keys().filter(|wd| !now.contains_key(wd)) {
            let _ = inotify::remove_watch(&*self.fd, *wd);
        }
        self.wds = now;
        Ok(fresh)
    }

    /// Read what happened: what changed, and whether folders came or went (look again at what to watch).
    fn read(&mut self) -> (Changed, bool) {
        let mut buf = [std::mem::MaybeUninit::<u8>::uninit(); 4096];
        let mut reader = inotify::Reader::new(&*self.fd, &mut buf);
        let (mut changed, mut again) = (Changed::default(), false);
        // until the queue is empty (EAGAIN: the watch does not block)
        while let Ok(ev) = reader.next() {
            let f = ev.events();
            if f.contains(ReadFlags::QUEUE_OVERFLOW) {
                // events were lost: anything may have changed
                changed = Changed { apps: true, icons: true };
                again = true;
                continue;
            }
            let Some((_, k)) = self.wds.get(&ev.wd()) else { continue };
            let k = *k;
            if f.intersects(ReadFlags::DELETE_SELF | ReadFlags::MOVE_SELF | ReadFlags::IGNORED) {
                changed.add(kinds_changed(k));
                again = true;
            } else if f.contains(ReadFlags::ISDIR) {
                // a folder came or went: below a pack, or on the way to a folder not there yet
                changed.add(kinds_changed(k));
                again = true;
            } else if let Some(name) = ev.file_name().and_then(|n| n.to_str().ok()) {
                // the files that matter: desktop entries, and icons (not the caches made after an install)
                if k & APPS != 0 && name.ends_with(".desktop") {
                    changed.apps = true;
                }
                if k & ICONS != 0 && [".svg", ".svgz", ".png"].iter().any(|e| name.ends_with(e)) {
                    changed.icons = true;
                }
            }
        }
        (changed, again)
    }
}

/// The watch, kept to change the icon packs watched (`set_icon_themes`).
pub struct Watch<S: 'static> {
    inner: Rc<RefCell<Inner<S>>>,
    handle: LoopHandle<'static, S>,
    token: RegistrationToken,
}

/// Watch `places` from `handle`'s loop: `on_change` gets each settled burst (what changed); `on_limit` is called once if the
/// inotify limit is reached, now (then there is no watch) or later.
pub fn start<S: 'static>(
    handle: &LoopHandle<'static, S>,
    places: Places,
    on_change: impl FnMut(&mut S, Changed) + 'static,
    on_limit: impl FnOnce(std::io::Error) + 'static,
) -> Option<Watch<S>> {
    let fd = match inotify::init(CreateFlags::CLOEXEC | CreateFlags::NONBLOCK) {
        Ok(fd) => Rc::new(fd),
        Err(e) => {
            on_limit(e.into());
            return None;
        }
    };
    let mut inner = Inner {
        fd: fd.clone(),
        places,
        wds: HashMap::new(),
        pending: Changed::default(),
        burst: Burst::default(),
        armed: false,
        on_change: Some(Box::new(on_change)),
        on_limit: None,
        stopped: false,
    };
    if let Err(e) = inner.resync(true) {
        on_limit(e);
        return None;
    }
    inner.on_limit = Some(Box::new(on_limit));
    let inner = Rc::new(RefCell::new(inner));
    let i2 = inner.clone();
    let h2 = handle.clone();
    let token = handle
        .insert_source(Generic::new(fd, Interest::READ, Mode::Level), move |_, _, _| {
            let mut i = i2.borrow_mut();
            let (mut changed, again) = i.read();
            if again {
                match i.resync(false) {
                    Ok(fresh) => changed.add(fresh),
                    Err(e) => {
                        i.give_up(e);
                        return Ok(PostAction::Remove);
                    }
                }
            }
            if changed.any() {
                i.pending.add(changed);
                i.burst.note(Instant::now());
                if !i.armed {
                    i.armed = true;
                    arm(&h2, &i2);
                }
            }
            Ok(PostAction::Continue)
        })
        .ok()?;
    Some(Watch { inner, handle: handle.clone(), token })
}

/// A timer that hands the burst over once it has settled.
fn arm<S: 'static>(handle: &LoopHandle<'static, S>, inner: &Rc<RefCell<Inner<S>>>) {
    let inner = inner.clone();
    let _ = handle.insert_source(Timer::from_duration(QUIET), move |_, _, st| {
        let (changed, mut cb) = {
            let mut i = inner.borrow_mut();
            match i.burst.due() {
                Some(at) if at > Instant::now() => return TimeoutAction::ToInstant(at),
                _ => {}
            }
            i.burst = Burst::default();
            i.armed = false;
            (std::mem::take(&mut i.pending), i.on_change.take())
        };
        // called with nothing borrowed: it may change the watch (another icon pack)
        if let Some(f) = cb.as_mut() {
            if changed.any() {
                f(st, changed);
            }
        }
        inner.borrow_mut().on_change = cb;
        TimeoutAction::Drop
    });
}

impl<S> Inner<S> {
    fn give_up(&mut self, e: std::io::Error) {
        self.stopped = true;
        for wd in self.wds.keys() {
            let _ = inotify::remove_watch(&*self.fd, *wd);
        }
        self.wds.clear();
        if let Some(f) = self.on_limit.take() {
            f(e);
        }
    }
}

impl<S: 'static> Watch<S> {
    /// Another icon pack is in use: watch its folders instead.
    pub fn set_icon_themes(&self, themes: Vec<PathBuf>) {
        let mut i = self.inner.borrow_mut();
        if i.stopped {
            return;
        }
        i.places.themes = themes;
        if let Err(e) = i.resync(true) {
            i.give_up(e);
            self.handle.remove(self.token);
        }
    }

    /// The folders watched now.
    #[cfg(test)]
    fn watched(&self) -> Vec<PathBuf> {
        let mut v: Vec<PathBuf> = self.inner.borrow().wds.values().map(|(p, _)| p.clone()).collect();
        v.sort();
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use calloop::EventLoop;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// A scratch folder of this test's own (never shared with another test), removed when dropped.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(name: &str) -> Scratch {
            static N: AtomicU32 = AtomicU32::new(0);
            let p = std::env::temp_dir().join(format!(
                "ygg-panel-appwatch-{name}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Scratch(p)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// What the panel would do: each burst handed over, and the apps read again from the folders.
    #[derive(Default)]
    struct Seen {
        bursts: Vec<Changed>,
        apps: Vec<String>,
    }

    fn desktop(name: &str) -> String {
        format!("[Desktop Entry]\nType=Application\nName={name}\nExec={name}\nIcon={name}\n")
    }

    /// The panel's loop, cut down to the watch: no timer of its own, only what the watch puts in it.
    fn watching(places: Places) -> (EventLoop<'static, Seen>, Watch<Seen>, Seen) {
        let ev: EventLoop<Seen> = EventLoop::try_new().unwrap();
        let dirs = places.apps.clone();
        let w = start(
            &ev.handle(),
            places,
            move |st: &mut Seen, c| {
                st.bursts.push(c);
                if c.apps {
                    st.apps = crate::apps::load_from(&dirs).into_iter().map(|a| a.name).collect();
                }
            },
            |e| panic!("the watch limit, in a test: {e}"),
        )
        .expect("watching");
        (ev, w, Seen::default())
    }

    /// Run the loop until `done`, at most `secs`.
    fn run_until(ev: &mut EventLoop<Seen>, st: &mut Seen, secs: u64, done: impl Fn(&Seen) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(secs);
        while !done(st) {
            if Instant::now() >= until {
                return false;
            }
            ev.dispatch(Some(Duration::from_millis(50)), st).unwrap();
        }
        true
    }

    /// Run the loop for `d` whatever happens.
    fn run_for(ev: &mut EventLoop<Seen>, st: &mut Seen, d: Duration) {
        let until = Instant::now() + d;
        while Instant::now() < until {
            ev.dispatch(Some(Duration::from_millis(50)), st).unwrap();
        }
    }

    #[test]
    fn a_new_app_shows_up_without_a_timer() {
        let s = Scratch::new("new");
        let apps = s.0.join("applications");
        std::fs::create_dir_all(&apps).unwrap();
        let (mut ev, _w, mut st) = watching(Places { apps: vec![apps.clone()], ..Default::default() });
        run_for(&mut ev, &mut st, Duration::from_millis(200));
        assert!(st.bursts.is_empty(), "nothing yet: {:?}", st.bursts);
        // the loop has no timer of its own: only the watch can bring the app in
        std::fs::write(apps.join("ygg-test-new.desktop"), desktop("NewApp")).unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.apps.contains(&"NewApp".to_string())), "the new app shows up");
        assert_eq!(st.bursts, [Changed { apps: true, icons: false }]);
        // removed, as a package removal does
        std::fs::remove_file(apps.join("ygg-test-new.desktop")).unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.apps.is_empty()), "the removed app goes");
    }

    #[test]
    fn a_burst_is_one_reload() {
        let s = Scratch::new("burst");
        let apps = s.0.join("applications");
        let pack = s.0.join("icons/Pack");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::create_dir_all(pack.join("48x48/apps")).unwrap();
        let (mut ev, _w, mut st) = watching(Places { apps: vec![apps.clone()], themes: vec![pack.clone()], ..Default::default() });
        // a package install: many files, written beside and renamed as pacman does, then the caches its hooks make (these alone
        // change nothing). How changes spread over time are gathered is `a_long_burst_is_taken_up_on_the_way` (no clock here: a
        // slow machine could not split this burst).
        for n in 0..60 {
            let tmp = apps.join(format!(".ygg-test-{n}.desktop.part"));
            std::fs::write(&tmp, desktop(&format!("App{n}"))).unwrap();
            std::fs::rename(&tmp, apps.join(format!("ygg-test-{n}.desktop"))).unwrap();
            std::fs::write(pack.join(format!("48x48/apps/ygg-test-{n}.png")), b"png").unwrap();
        }
        std::fs::write(apps.join("mimeinfo.cache"), "[MIME Cache]\n").unwrap();
        std::fs::write(pack.join("icon-theme.cache"), b"cache").unwrap();
        run_for(&mut ev, &mut st, QUIET * 3);
        assert_eq!(st.bursts, [Changed { apps: true, icons: true }], "one reload for the whole burst");
        assert_eq!(st.apps.len(), 60, "and every app is in it");
        // the caches alone, later: no reload
        std::fs::write(apps.join("mimeinfo.cache"), "[MIME Cache]\nx=y\n").unwrap();
        std::fs::write(pack.join("icon-theme.cache"), b"cache 2").unwrap();
        run_for(&mut ev, &mut st, QUIET * 2);
        assert_eq!(st.bursts.len(), 1, "caches are not apps or icons: {:?}", st.bursts);
    }

    #[test]
    fn a_folder_made_later_is_watched() {
        let s = Scratch::new("later");
        // Flatpak not installed yet, the icon pack not either
        let flatpak = s.0.join("var/lib/flatpak/exports/share/applications");
        let pack = s.0.join("icons/Pack");
        std::fs::create_dir_all(s.0.join("var/lib")).unwrap();
        std::fs::create_dir_all(s.0.join("icons")).unwrap();
        let (mut ev, w, mut st) = watching(Places { apps: vec![flatpak.clone()], themes: vec![pack.clone()], ..Default::default() });
        assert_eq!(w.watched(), [s.0.join("icons"), s.0.join("var/lib")], "the nearest folders that are there");
        // the first Flatpak app: its folders, and its entry at once (before the watch can have moved down)
        std::fs::create_dir_all(&flatpak).unwrap();
        std::fs::write(flatpak.join("org.example.First.desktop"), desktop("First")).unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.apps.contains(&"First".to_string())), "the first app shows up");
        assert!(w.watched().contains(&flatpak), "the new folder is watched: {:?}", w.watched());
        // the next one comes through the folder's own watch
        std::fs::write(flatpak.join("org.example.Second.desktop"), desktop("Second")).unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.apps.contains(&"Second".to_string())), "the second app shows up");
        // the icon pack installed: a size folder, a context folder below it, an icon
        let n = st.bursts.len();
        std::fs::create_dir_all(pack.join("64x64/apps")).unwrap();
        std::fs::write(pack.join("64x64/apps/org.example.First.png"), b"png").unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.bursts[n..].iter().any(|c| c.icons)), "the new pack's icons are taken up");
        assert!(w.watched().contains(&pack.join("64x64/apps")), "{:?}", w.watched());
        // an icon replaced in it later: its folder's own watch
        run_for(&mut ev, &mut st, QUIET * 2);
        let n = st.bursts.len();
        std::fs::write(pack.join("64x64/apps/org.example.First.png"), b"png 2").unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.bursts[n..].iter().any(|c| c.icons)), "a replaced icon is taken up");
        // Flatpak removed entirely, then installed again: waited for again
        std::fs::remove_dir_all(s.0.join("var/lib/flatpak")).unwrap();
        run_for(&mut ev, &mut st, QUIET * 2);
        assert!(w.watched().contains(&s.0.join("var/lib")), "back to waiting: {:?}", w.watched());
        std::fs::create_dir_all(&flatpak).unwrap();
        std::fs::write(flatpak.join("org.example.Third.desktop"), desktop("Third")).unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.apps == ["Third"]), "{:?}", st.apps);
    }

    #[test]
    fn another_icon_pack_is_watched_instead() {
        let s = Scratch::new("pack");
        let (a, b) = (s.0.join("icons/A"), s.0.join("icons/B"));
        std::fs::create_dir_all(a.join("apps/scalable")).unwrap();
        std::fs::create_dir_all(b.join("scalable/apps")).unwrap();
        // a folder of no icon context two deep is not watched (no icons are looked for there)
        std::fs::create_dir_all(b.join("scalable/mimetypes")).unwrap();
        let (mut ev, w, mut st) = watching(Places { themes: vec![a.clone()], ..Default::default() });
        assert_eq!(w.watched(), [a.clone(), a.join("apps"), a.join("apps/scalable")]);
        w.set_icon_themes(vec![b.clone()]);
        assert_eq!(w.watched(), [b.clone(), b.join("scalable"), b.join("scalable/apps")]);
        std::fs::write(a.join("apps/scalable/x.svg"), "<svg/>").unwrap();
        run_for(&mut ev, &mut st, QUIET * 2);
        assert!(st.bursts.is_empty(), "the old pack is no longer watched");
        std::fs::write(b.join("scalable/apps/x.svg"), "<svg/>").unwrap();
        assert!(run_until(&mut ev, &mut st, 10, |st| st.bursts == [Changed { apps: false, icons: true }]));
    }

    #[test]
    fn a_long_burst_is_taken_up_on_the_way() {
        let mut b = Burst::default();
        let t = Instant::now();
        assert_eq!(b.due(), None);
        b.note(t);
        assert_eq!(b.due(), Some(t + QUIET));
        b.note(t + Duration::from_millis(300));
        assert_eq!(b.due(), Some(t + Duration::from_millis(300) + QUIET), "each change waits for quiet again");
        b.note(t + LONGEST - Duration::from_millis(100));
        assert_eq!(b.due(), Some(t + LONGEST), "but not for ever");
    }

    #[test]
    fn only_running_out_is_the_limit() {
        use rustix::io::Errno;
        assert!(is_limit(Errno::NOSPC) && is_limit(Errno::MFILE));
        assert!(!is_limit(Errno::NOENT) && !is_limit(Errno::ACCESS) && !is_limit(Errno::NOTDIR));
    }
}
