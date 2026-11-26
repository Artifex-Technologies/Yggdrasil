use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};
use yggdrasil_ui::dnd;

mod cloudside;
mod icons;
mod settings_win;
mod statusbar;
mod topbar;
mod transfers;

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense, UiBuilder};
use egui_extras::{Column, TableBuilder};
use icons::Icon;
use minos_core::archive::{self, Format};
use minos_core::cloud::{self, Badge, Link, Report};
use minos_core::columns::{self, ListColumn};
use minos_core::commandbar;
use minos_core::devices;
use minos_core::grouping::{self, GroupBy};
use minos_core::listing::{self, Entry, Sort, SortKey};
use minos_core::places::{self, CloudPlace, CloudState};
use minos_core::preview::{self, Body, PreviewData};
use minos_core::privacy;
use minos_core::run;
use minos_core::search::{display_relative, Search};
use minos_core::state::{SavedTab, State, ViewMode, GALLERY_SIZES, PREVIEW_MAX, PREVIEW_MIN};
use minos_core::{expand_address, fileops, History, Tab};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use theme::{palette_for, Palette};
use yggdrasil_ui::sysicon;
use yggdrasil_ui::{chrome, theme};

/// Right-click > Scrub location sounds, running on its own thread (see App::start_scrub): a small window with its progress, then
/// whether it worked, until closed.
struct ScrubJob {
    id: u64,
    /// The original's name, for the window's title.
    name: String,
    /// The copy being written (it did not exist before: removed again if the scrub is stopped or fails).
    out: PathBuf,
    /// How far, in thousandths.
    frac: Arc<std::sync::atomic::AtomicU32>,
    pid: u32,
    rx: Receiver<Result<PathBuf, String>>,
    result: Option<Result<PathBuf, String>>,
    stopped: bool,
}

/// One archive to extract: everything (`inner` empty) or one entry, into `dest`; `here` for Extract Here's wording.
struct ExtractJob {
    archive: PathBuf,
    dest: PathBuf,
    inner: String,
    here: bool,
}

/// Right-click > Remove hidden info, running on its own thread (see App::start_strip): a small window while it works, then
/// what was removed (or that nothing was hidden), until closed.
struct StripJob {
    id: u64,
    name: String,
    /// The copy asked for (it did not exist before: removed again if the strip is stopped or fails).
    out: PathBuf,
    pid: u32,
    rx: Receiver<Result<privacy::Stripped, String>>,
    result: Option<Result<privacy::Stripped, String>>,
    stopped: bool,
}

/// What the menu learnt about a file (privacy::has_audio, privacy::can_strip: on a thread when the menu first opens on it): the
/// file's size and change time it was read for, and the answer (None while it is being read).
type Probes = Arc<std::sync::Mutex<HashMap<PathBuf, ((u64, Option<std::time::SystemTime>), Option<bool>)>>>;
const AUDIO_PROBES_ID: &str = "minos-audio-probes";
const STRIP_PROBES_ID: &str = "minos-strip-probes";

/// The answer `ask` gave for this file as it is now, or None while it is asked (on a thread, the first time).
fn probe(ui: &egui::Ui, id: &str, path: &Path, stamp: (u64, Option<std::time::SystemTime>), ask: fn(&Path) -> bool) -> Option<bool> {
    let probes = ui.ctx().data_mut(|d| d.get_temp_mut_or_default::<Probes>(egui::Id::new(id)).clone());
    let known = probes.lock().ok().and_then(|m| m.get(path).filter(|(s, _)| *s == stamp).map(|(_, a)| *a));
    match known {
        Some(a) => a,
        None => {
            if let Ok(mut m) = probes.lock() {
                m.insert(path.to_path_buf(), (stamp, None));
            }
            let (path, ctx) = (path.to_path_buf(), ui.ctx().clone());
            std::thread::spawn(move || {
                let a = ask(&path);
                if let Ok(mut m) = probes.lock() {
                    m.insert(path, (stamp, Some(a)));
                }
                ctx.request_repaint();
            });
            None
        }
    }
}

/// The Privacy part of a file's menu (privacy.rs): nothing is read until the menu is open on the file; then its first bytes, and
/// for a video ffprobe (3 s at most), and heimdall-strip --can, on threads. While those run the entries show greyed.
fn privacy_menu(ui: &mut egui::Ui, e: &Entry, actions: &mut Vec<Action>) {
    let scrub = privacy::on_path(privacy::SCRUB_TOOL);
    let strip = privacy::on_path(privacy::STRIP_TOOL) && yggdrasil_ui::privacy_prefs::get(yggdrasil_ui::privacy_prefs::REMOVE_HIDDEN_INFO);
    if !scrub && !strip {
        return;
    }
    let Ok(meta) = std::fs::metadata(&e.path) else { return };
    let stamp = (meta.len(), meta.modified().ok());
    let has_audio =
        if scrub { probe(ui, AUDIO_PROBES_ID, &e.path, stamp, |p| privacy::has_audio(p, Duration::from_secs(3))) } else { Some(false) };
    let strippable =
        if strip { probe(ui, STRIP_PROBES_ID, &e.path, stamp, |p| privacy::can_strip(p, Duration::from_secs(3))) } else { Some(false) };
    let facts = privacy::Facts {
        scrub_installed: scrub,
        has_audio: has_audio.unwrap_or(false),
        strip_on: strip,
        strippable: strippable.unwrap_or(false),
    };
    let list = privacy::entries(facts);
    let waiting: Vec<(privacy::Entry, &str)> = [
        (has_audio.is_none(), privacy::Entry::ScrubSounds, "Looking for sound in it..."),
        (strippable.is_none(), privacy::Entry::RemoveHidden, "Looking at what kind of file it is..."),
    ]
    .into_iter()
    .filter_map(|(w, en, why)| w.then_some((en, why)))
    .collect();
    if list.is_empty() && waiting.is_empty() {
        return;
    }
    ui.separator();
    for entry in [privacy::Entry::ScrubSounds, privacy::Entry::RemoveHidden] {
        if list.contains(&entry) {
            if ui.button(entry.label()).on_hover_text(entry.hint()).clicked() {
                actions.push(Action::Privacy(entry, e.path.clone()));
                ui.close();
            }
        } else if let Some((_, why)) = waiting.iter().find(|(w, _)| *w == entry) {
            ui.add_enabled(false, egui::Button::new(entry.label())).on_disabled_hover_text(*why);
        }
    }
}

/// Whether `dest` is on the same drive as `src` (a move there is just a rename, not a copy and a delete).
/// Clipboard text as compared: line endings and trailing blank lines do not count.
fn same_text(t: &str) -> String {
    t.replace("\r\n", "\n").trim_end().to_string()
}

/// Files as the system clipboard carries them between file managers: one file:// address per line (text/uri-list form).
fn uri_list(paths: &[PathBuf]) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut out = String::new();
    for p in paths {
        out.push_str("file://");
        for &b in p.as_os_str().as_bytes() {
            if b.is_ascii_alphanumeric() || b"/-._~".contains(&b) {
                out.push(b as char);
            } else {
                out.push_str(&format!("%{b:02X}"));
            }
        }
        out.push_str("\r\n");
    }
    out
}

/// The files a clipboard's text names (file:// addresses, or plain absolute paths, one per line; GNOME's "copy"/"cut" first
/// line is skipped), those that exist. Empty when the text is not a list of files.
fn paths_from_clipboard(text: &str) -> Vec<PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut out = Vec::new();
    for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && *l != "copy" && *l != "cut" && !l.starts_with('#')) {
        let path = if let Some(rest) = line.strip_prefix("file://") {
            let rest = rest.strip_prefix("localhost").unwrap_or(rest);
            let b = rest.as_bytes();
            let mut bytes = Vec::with_capacity(b.len());
            let mut i = 0;
            while i < b.len() {
                let hex = |c: u8| (c as char).to_digit(16);
                if b[i] == b'%' && i + 2 < b.len() {
                    if let (Some(h), Some(l)) = (hex(b[i + 1]), hex(b[i + 2])) {
                        bytes.push((h * 16 + l) as u8);
                        i += 3;
                        continue;
                    }
                }
                bytes.push(b[i]);
                i += 1;
            }
            PathBuf::from(std::ffi::OsString::from_vec(bytes))
        } else if line.starts_with('/') {
            PathBuf::from(line)
        } else {
            return Vec::new(); // text that is not a list of files
        };
        if path.exists() {
            out.push(path);
        }
    }
    out
}

fn same_drive(src: Option<&PathBuf>, dest: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (src.and_then(|s| s.parent()).and_then(|p| std::fs::metadata(p).ok()), std::fs::metadata(dest).ok()) {
        (Some(a), Some(b)) => a.dev() == b.dev(),
        _ => false,
    }
}

/// Drag-and-drop payload: the files being dragged.
struct DragPaths(Vec<PathBuf>);

/// A favorite being dragged in the sidebar to a new place: its index.
#[derive(Clone, Copy, Debug)]
struct DragFav(usize);

/// A tab being dragged: its pane and index. Let go outside the window, it opens in a new Minos window; on another tab,
/// it moves there.
#[derive(Clone, Copy, Debug)]
struct DragTab(usize, usize);

#[derive(Clone, Copy, Debug, PartialEq)]
enum Nav {
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
}

enum Action {
    Focus(usize),
    Up(usize),
    TogglePreview,
    ExtractHere(Vec<PathBuf>),
    ExtractTo(Vec<PathBuf>),
    Compress(Vec<PathBuf>, Format),
    NewFile(usize),
    EditAddress(usize),
    Go(usize, PathBuf),
    OpenTab(usize, PathBuf),
    CloseTab(usize, usize),
    ReopenTab,
    /// The + button and Ctrl+T: Home or the current folder, as set in Settings.
    NewTab(usize),
    /// Arrow, Home, End and Page keys: move the selection (the bool extends it with Shift).
    KeyNav(usize, Nav, bool),
    /// Enter: open what is selected.
    KeyOpen(usize),
    /// Typing a name: jump to the first entry that starts with it.
    TypeFind(usize, String),
    Back(usize),
    Forward(usize),
    FocusOtherPane,
    CloseWindow,
    SelectTab(usize, usize),
    PinTab(usize, usize),
    Reload,
    OpenFile(PathBuf),
    /// Right-click > Run in terminal, for a program or script marked as runnable.
    RunInTerminal(PathBuf),
    /// Right-click > a Privacy entry on a file (privacy.rs).
    Privacy(privacy::Entry, PathBuf),
    Cut(Vec<PathBuf>),
    Copy(Vec<PathBuf>),
    Paste(usize),
    Rename(PathBuf),
    Trash(Vec<PathBuf>),
    NewFolder(usize),
    ToggleTag(PathBuf, String),
    SetColor(PathBuf, Option<[u8; 3]>),
    ToggleFav(PathBuf),
    MoveFav(PathBuf, i32),
    /// A favorite dragged from one place in the list to the gap before another (or the end).
    ReorderFav(usize, usize),
    /// A tab moved to another place in its pane's tab strip (the gap before index `to`).
    MoveTab(usize, usize, usize),
    /// A tab dragged out of the window: it opens in a new Minos window and leaves this one.
    DetachTab(usize, usize),
    ResetFavs,
    Move(Vec<PathBuf>, PathBuf),
    CopyPath(PathBuf),
    Verify(String),
    ToggleSplit,
    SetView(ViewMode),
    ProtonConnect,
    RetrySync(PathBuf),
    /// Proton Drive: download files and folders to this computer.
    Download(Vec<PathBuf>),
    /// A drive's partition that is not mounted: mount it and open it in the pane.
    Mount(usize, String),
    /// Unmount and power off a removable drive (its device name).
    Eject(String),
    /// Ask how to format a removable drive.
    FormatAsk(String),
    /// Start a tool for a drive (program, package); a missing one is offered for install.
    Launch(String, String),
    /// Install a package in a terminal (it asks for the password there).
    Install(String),
    /// The command bar's Properties (Alt+Enter): what is selected in the pane, or its folder.
    Properties(usize),
    /// The command bar's Group menu.
    SetGroup(GroupBy),
    /// A column of the details list shown or hidden (the layout menu's Columns, a right-click on the header).
    ToggleColumn(ListColumn),
    /// The navigation row's sidebar button.
    ToggleSidebar,
    /// A cloud drive under Cloud Storage: mount it (and open it once mounted, when the bool says so), or unmount it.
    CloudMount(String, bool),
    CloudUnmount(String),
    /// Settings > Cloud Storage.
    ManageCloud,
    /// Menu > Settings...: Minos's own settings window (settings_win.rs).
    OpenSettings,
}

/// What the drive watcher and drive jobs report back.
enum DevEvent {
    /// A drive came, went, or was mounted or unmounted.
    Changed,
    /// A job finished: what to say, and a folder to open.
    Done(String, Option<(usize, PathBuf)>),
}

/// The Format dialog: the drive, the chosen file system, the label, and whether the erase was confirmed.
struct FormatForm {
    disk: String,
    fs: usize,
    label: String,
    sure: bool,
}

/// Whether a folder watch's event is a change to the folder's entries (made, changed, removed, renamed), not a read of it.
fn folder_changed(kind: &notify::EventKind) -> bool {
    !matches!(kind, notify::EventKind::Access(_) | notify::EventKind::Other)
}

struct Watch {
    path: PathBuf,
    _watcher: RecommendedWatcher,
    rx: Receiver<()>,
}

struct TabView {
    tab: Tab,
    entries: Vec<Entry>,
    error: Option<String>,
    query: String,
    address: String,
    selected: BTreeSet<PathBuf>,
    anchor: Option<usize>,
    /// The keyboard's position in the list (a path, so it survives re-sorting).
    cursor: Option<PathBuf>,
    recursive: bool,
    /// Where the running search started (the folder, or / for the whole computer), for the names shown.
    search_root: PathBuf,
    /// The search box under the magnifier is showing, and should take the keyboard when it opens.
    search_open: bool,
    search_focus: bool,
    /// The command bar's quick filter: only names with these letters in them are shown (cleared on leaving the folder).
    filter: String,
    filter_open: bool,
    filter_focus: bool,
    search: Option<Search>,
    results: Option<Vec<Entry>>,
    /// The address bar is showing an editable path instead of clickable segments.
    editing: bool,
    edit_fresh: bool,
    /// The folder's listing while it is still on its way (a drive slow to answer): the list waits, the window does not.
    pending: Option<Pending>,
    /// The folder `entries` were listed from (None: This PC, a disk or an archive).
    listed: Option<PathBuf>,
}

/// A folder listing read on its own thread.
struct Pending {
    dir: PathBuf,
    rx: Receiver<std::io::Result<Vec<Entry>>>,
}

/// How long the window waits for a folder's listing before it draws "waiting" and lets the listing come in later. A folder on a
/// local disk is listed well within it, so it shows at once, as before.
const LISTING_WAIT: Duration = Duration::from_millis(150);

impl TabView {
    fn new(path: PathBuf, pinned: bool, sort: Sort, hidden: bool) -> Self {
        let mut t = Self {
            tab: Tab { history: History::new(path), pinned },
            entries: vec![],
            error: None,
            query: String::new(),
            address: String::new(),
            selected: BTreeSet::new(),
            anchor: None,
            cursor: None,
            recursive: true,
            search_root: PathBuf::new(),
            search_open: false,
            search_focus: false,
            filter: String::new(),
            filter_open: false,
            filter_focus: false,
            search: None,
            results: None,
            editing: false,
            edit_fresh: false,
            pending: None,
            listed: None,
        };
        t.reload(sort, hidden);
        t
    }

    /// What the list shows: the search results, or the folder's entries.
    fn shown(&self) -> &[Entry] {
        self.results.as_deref().unwrap_or(&self.entries)
    }

    /// Indices into `shown()` in the order they are drawn: what the search words, the quick filter and the hidden-items setting
    /// let through, in their groups when the list is grouped (Group by). This PC and a disk's partitions are shown as they are.
    fn arranged(&self, hidden: bool, group: GroupBy) -> grouping::Arranged {
        let shown = self.shown();
        if self.results.is_none() && places::is_virtual(&self.dir()) {
            return grouping::Arranged { order: (0..shown.len()).collect(), heads: vec![] };
        }
        let filter = self.filter.to_lowercase();
        let keep = |i: &usize| commandbar::filter_matches(&shown[*i].name, &filter);
        let base: Vec<usize> = if self.results.is_some() {
            (0..shown.len()).filter(keep).collect()
        } else {
            listing::visible(shown, &self.query, hidden).into_iter().filter(|&i| !pick_hides(&shown[i])).filter(keep).collect()
        };
        let offset = if group == GroupBy::Modified { grouping::utc_offset() } else { 0 };
        grouping::arrange(shown, &base, group, std::time::SystemTime::now(), offset)
    }

    fn order(&self, hidden: bool, group: GroupBy) -> Vec<usize> {
        self.arranged(hidden, group).order
    }

    fn dir(&self) -> PathBuf {
        self.tab.history.current().to_path_buf()
    }

    fn reload(&mut self, sort: Sort, _hidden: bool) {
        let dir = self.dir();
        self.address = dir.to_string_lossy().into_owned();
        self.pending = None;
        let before = self.listed.take();
        if places::is_this_pc(&dir) {
            // The physical disks; with no lsblk, the mounted drives as before.
            let disks = places::disks();
            self.entries = if disks.is_empty() { listing::drive_entries(&places::drives()) } else { listing::disk_entries(&disks) };
            self.error = None;
            self.selected.clear();
            return;
        }
        if let Some(name) = places::disk_name(&dir) {
            // One disk: its partitions.
            self.entries = places::disks().iter().find(|d| d.name == name).map(listing::partition_entries).unwrap_or_default();
            self.error = None;
            self.selected.clear();
            return;
        }
        if let Some((arc, inner)) = archive::split(&dir) {
            match archive::list_cached(&arc) {
                Ok(list) => {
                    let mut e = archive::children(&arc, &list, &inner);
                    listing::sort_entries(&mut e, sort);
                    self.entries = e;
                    self.error = None;
                }
                Err(msg) => {
                    self.entries.clear();
                    self.error = Some(format!("{}: {msg}", arc.display()));
                }
            }
            let still: BTreeSet<PathBuf> = self.entries.iter().map(|e| e.path.clone()).collect();
            self.selected.retain(|p| still.contains(p));
            return;
        }
        // listed on its own thread: a folder on a drive that is slow to answer (Proton Drive over a slow line took minutes on
        // 2026-11-22) leaves the window working, and the list fills in when the drive answers (poll_listing)
        let (tx, rx) = channel();
        let d = dir.clone();
        std::thread::spawn(move || {
            let _ = tx.send(listing::read_dir(&d));
        });
        match rx.recv_timeout(LISTING_WAIT) {
            Ok(listed) => self.take_listing(listed, dir, sort),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // the same folder again (a refresh) keeps what it showed until the new listing is in
                if before.as_ref() == Some(&dir) {
                    self.listed = before;
                } else {
                    self.entries.clear();
                }
                self.error = None;
                self.pending = Some(Pending { dir, rx });
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.take_listing(Err(std::io::Error::other("the listing stopped")), dir, sort)
            }
        }
    }

    fn take_listing(&mut self, listed: std::io::Result<Vec<Entry>>, dir: PathBuf, sort: Sort) {
        match listed {
            Ok(mut e) => {
                listing::sort_entries(&mut e, sort);
                self.entries = e;
                self.error = None;
            }
            Err(err) => {
                self.entries.clear();
                self.error = Some(format!("{}: {err}", dir.display()));
            }
        }
        self.listed = Some(dir);
        let still: BTreeSet<PathBuf> = self.entries.iter().map(|e| e.path.clone()).collect();
        self.selected.retain(|p| still.contains(p));
    }

    /// Takes a listing that came in late. True while one is still on its way (the window then looks again shortly).
    fn poll_listing(&mut self, sort: Sort) -> bool {
        let Some(p) = &self.pending else { return false };
        let got = match p.rx.try_recv() {
            Ok(listed) => listed,
            Err(std::sync::mpsc::TryRecvError::Empty) => return true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => Err(std::io::Error::other("the listing stopped")),
        };
        let Some(p) = self.pending.take() else { return false };
        if p.dir == self.dir() {
            self.take_listing(got, p.dir, sort);
        }
        false
    }

    fn navigate(&mut self, path: PathBuf, sort: Sort, hidden: bool) {
        self.tab.history.visit(path);
        self.query.clear();
        self.filter.clear();
        self.filter_open = false;
        self.selected.clear();
        self.anchor = None;
        self.search = None;
        self.results = None;
        self.reload(sort, hidden);
    }

    fn restart_search(&mut self, ctx: &egui::Context, hidden: bool) {
        self.search_from(ctx, hidden, self.dir());
    }

    /// Search from `root`: this folder, or `/` for the whole computer. Whether it goes into the
    /// folders below is the Settings choice (Apps, Minos).
    fn search_from(&mut self, ctx: &egui::Context, hidden: bool, root: PathBuf) {
        self.search = None;
        self.results = None;
        self.recursive = root == Path::new("/") || yggdrasil_conf::PanelConf::load().search_subfolders;
        if self.recursive && !self.query.trim().is_empty() {
            let ctx = ctx.clone();
            self.results = Some(vec![]);
            self.search_root = root.clone();
            self.search = Some(Search::start(root, self.query.clone(), hidden, 5000, move || ctx.request_repaint()));
        }
    }

    fn poll_search(&mut self) {
        let (Some(s), Some(res)) = (&self.search, &mut self.results) else { return };
        let root = self.search_root.clone();
        while let Ok(p) = s.rx.try_recv() {
            if let Ok(m) = std::fs::metadata(&p) {
                res.push(Entry {
                    name: display_relative(&root, &p),
                    hidden: false,
                    is_dir: m.is_dir(),
                    size: if m.is_dir() { 0 } else { m.len() },
                    modified: m.modified().ok(),
                    path: p,
                    usage: None,
                });
            }
        }
    }
}

struct Pane {
    tabs: Vec<TabView>,
    active: usize,
    sort: Sort,
    watch: Option<Watch>,
    /// The folder left without a watch: it is on a drive that may stall (places::may_stall).
    unwatched: Option<PathBuf>,
}

impl Pane {
    fn from_saved(saved: &[SavedTab], active: usize, fallback: PathBuf, hidden: bool) -> Self {
        let sort = Sort::default();
        let mut tabs: Vec<TabView> = saved
            .iter()
            // a tab on a drive that may stall is kept unasked: looking whether it is still a folder waited on the drive
            .filter(|t| places::may_stall(&t.path) || t.path.is_dir() || places::is_virtual(&t.path))
            .map(|t| TabView::new(t.path.clone(), t.pinned, sort, hidden))
            .collect();
        if tabs.is_empty() {
            tabs.push(TabView::new(fallback, false, sort, hidden));
        }
        let active = active.min(tabs.len() - 1);
        Self { tabs, active, sort, watch: None, unwatched: None }
    }

    fn cur(&mut self) -> &mut TabView {
        &mut self.tabs[self.active]
    }

    fn ensure_watch(&mut self, ctx: &egui::Context) {
        let dir = self.tabs[self.active].dir();
        if self.watch.as_ref().is_some_and(|w| w.path == dir) || self.unwatched.as_ref() == Some(&dir) {
            return;
        }
        self.watch = None;
        if places::may_stall(&dir) {
            // setting the watch waits on the drive, and FUSE and network drives do not report others' changes to it anyway
            self.unwatched = Some(dir);
            return;
        }
        self.unwatched = None;
        let (tx, rx) = channel();
        let c = ctx.clone();
        let Ok(mut w) = notify::recommended_watcher(move |r: notify::Result<notify::Event>| {
            // a change only: reading the folder (which every reload does) reports an access, and reacting to that read it
            // again and again (28 frames a second for as long as Minos was open)
            if r.is_ok_and(|e| folder_changed(&e.kind)) {
                let _ = tx.send(());
                c.request_repaint();
            }
        }) else {
            return;
        };
        if w.watch(&dir, RecursiveMode::NonRecursive).is_ok() {
            self.watch = Some(Watch { path: dir, _watcher: w, rx });
        }
    }

    /// Reload when the filesystem changed under us, and take listings that came in late. True while a listing is still on
    /// its way.
    fn poll_watch(&mut self, hidden: bool) -> bool {
        let mut dirty = false;
        if let Some(w) = &self.watch {
            while w.rx.try_recv().is_ok() {
                dirty = true;
            }
        }
        let sort = self.sort;
        let t = &mut self.tabs[self.active];
        t.poll_search();
        if dirty && t.results.is_none() {
            t.reload(sort, hidden);
        }
        let mut waiting = false;
        for t in &mut self.tabs {
            waiting |= t.poll_listing(sort);
        }
        waiting
    }

    fn saved(&self) -> Vec<SavedTab> {
        self.tabs.iter().map(|t| SavedTab { path: t.dir(), pinned: t.tab.pinned }).collect()
    }
}

fn is_image(p: &Path) -> bool {
    matches!(p.extension().map(|e| e.to_string_lossy().to_lowercase()).as_deref(), Some("png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"))
}

fn file_uri(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
    }
}

/// Opens a file with its default app, through the operating system's app launcher where it has one (opener.rs).
fn open_with_system(path: &Path) {
    minos_core::opener::open(path);
}

fn fmt_time(t: std::time::SystemTime) -> String {
    // Civil-from-days conversion on UTC; avoids a date crate for one column.
    let secs = t.duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

struct Ctx<'a> {
    st: &'a State,
    links: &'a [Link],
    reports: &'a BTreeMap<String, Report>,
    clip_has: bool,
    pi: usize,
}

const RENAME_ID: &str = "minos-rename";
const CUT_ID: &str = "minos-cut";
/// How opaque a cut item's icon is drawn.
const CUT_OPACITY: f32 = 0.4;

/// Whether `p` is cut (waiting to be pasted somewhere).
fn is_cut(ctx: &egui::Context, p: &Path) -> bool {
    ctx.data(|d| d.get_temp::<Arc<std::collections::HashSet<PathBuf>>>(egui::Id::new(CUT_ID))).is_some_and(|s| s.contains(p))
}
const RENAME_RESULT_ID: &str = "minos-rename-result";

/// If `e` is being renamed, shows the editor in place of its name and returns true. Enter keeps
/// the new name, Esc, a click elsewhere or another window drops it (the outcome goes to the App through the egui data, see `ui`).
fn inline_rename(ui: &mut egui::Ui, pal: &Palette, e: &Entry) -> bool {
    let ctx = ui.ctx().clone();
    let Some(mut d) = ctx.data(|d| d.get_temp::<Option<RenameDlg>>(egui::Id::new(RENAME_ID))).flatten().filter(|d| d.path == e.path) else {
        return false;
    };
    ctx.data_mut(|d| d.insert_temp(egui::Id::new("minos-rename-seen"), true));
    let id = egui::Id::new(("minos-rename-edit", e.path.clone()));
    if d.fresh {
        // select the name without its extension, like other file managers
        let stem = if e.is_dir {
            d.text.chars().count()
        } else {
            std::path::Path::new(&d.text).file_stem().map(|s| s.to_string_lossy().chars().count()).unwrap_or(0).max(1)
        };
        let mut state = egui::TextEdit::load_state(&ctx, id).unwrap_or_default();
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(stem))));
        state.store(&ctx, id);
        // Focus before the field is drawn: egui collapses a selection in a field that does not have focus (it owns the
        // keyboard's text input only while focused), so focusing it afterwards left the cursor at the end of "New Folder" and
        // typing was added to the name instead of replacing it (2026-11-08).
        ctx.memory_mut(|m| m.request_focus(id));
    }
    let r = ui.add(egui::TextEdit::singleline(&mut d.text).id(id).desired_width(ui.available_width().max(80.0)).text_color(pal.text));
    if d.fresh {
        d.fresh = false;
    }
    let (esc, enter, window_unfocused) =
        ui.input(|i| (i.key_pressed(egui::Key::Escape), i.key_pressed(egui::Key::Enter), i.viewport().focused == Some(false)));
    // Only Enter keeps the new name. Esc, a click anywhere else, or switching to another window drops it,
    // so a stray keypress can never rename something by accident.
    let result = if enter && r.lost_focus() {
        Some(Some((d.path.clone(), d.text.clone())))
    } else if esc || r.lost_focus() || window_unfocused {
        Some(None)
    } else {
        None
    };
    if let Some(res) = result {
        ctx.data_mut(|data| data.insert_temp(egui::Id::new(RENAME_RESULT_ID), res));
    }
    ctx.data_mut(|data| data.insert_temp(egui::Id::new(RENAME_ID), Some(d)));
    true
}

/// Selection, drag source, drop target and context menu shared by both views.
#[allow(clippy::too_many_arguments)]
fn entry_interact(
    ctx: &egui::Context,
    r: &egui::Response,
    e: &Entry,
    pos: usize,
    order: &[usize],
    shown: &[Entry],
    selected: &mut BTreeSet<PathBuf>,
    anchor: &mut Option<usize>,
    cx: &Ctx,
    actions: &mut Vec<Action>,
) {
    if r.clicked() || r.secondary_clicked() {
        actions.push(Action::Focus(cx.pi));
        let m = ctx.input(|i| i.modifiers);
        // A click on the name of an item that was already selected renames it, as in Windows and macOS, unless
        // a second click follows quickly (that is a double click, and opens it). So the rename waits a moment.
        let was_only_selected = selected.len() == 1 && selected.contains(&e.path);
        let now = ctx.input(|i| i.time);
        let pending_id = egui::Id::new("minos-rename-pending");
        if r.clicked() {
            ctx.data_mut(|d| d.remove::<(PathBuf, f64)>(pending_id));
            if !m.shift && !m.command && was_only_selected && e.usage.is_none() {
                let on_name = ctx
                    .data(|d| d.get_temp::<Rect>(egui::Id::new(("minos-name-rect", e.path.clone()))))
                    .zip(r.interact_pointer_pos())
                    .map(|(rect, p)| rect.contains(p))
                    .unwrap_or(false);
                if on_name {
                    ctx.data_mut(|d| d.insert_temp(pending_id, (e.path.clone(), now)));
                }
            }
        }
        if r.secondary_clicked() && selected.contains(&e.path) {
            // keep the existing multi-selection for the context menu
        } else if m.shift && anchor.is_some() {
            let a = anchor.unwrap().min(order.len().saturating_sub(1));
            let (lo, hi) = (a.min(pos), a.max(pos));
            selected.clear();
            for &i in &order[lo..=hi] {
                selected.insert(shown[i].path.clone());
            }
        } else if m.command && r.clicked() {
            if !selected.remove(&e.path) {
                selected.insert(e.path.clone());
            }
            *anchor = Some(pos);
        } else {
            selected.clear();
            selected.insert(e.path.clone());
            *anchor = Some(pos);
        }
    }
    if r.double_clicked() {
        ctx.data_mut(|d| d.remove::<(PathBuf, f64)>(egui::Id::new("minos-rename-pending")));
        if e.is_dir || archive::is_archive_file(&e.path) {
            actions.push(Action::Go(cx.pi, e.path.clone()));
        } else {
            actions.push(Action::OpenFile(e.path.clone()));
        }
    }
    if r.middle_clicked() && e.is_dir {
        actions.push(Action::OpenTab(cx.pi, e.path.clone()));
    }
    if r.drag_started() && e.usage.is_none() {
        if !selected.contains(&e.path) {
            selected.clear();
            selected.insert(e.path.clone());
        }
        egui::DragAndDrop::set_payload(ctx, DragPaths(selected.iter().cloned().collect()));
    }
    if e.is_dir {
        // folders on screen this frame, for drops that arrive from other windows
        ctx.data_mut(|d| {
            d.get_temp_mut_or_default::<Vec<(Rect, PathBuf)>>(egui::Id::new("minos-drop-rows")).push((r.interact_rect, e.path.clone()))
        });
        if egui::DragAndDrop::has_payload_of_type::<DragPaths>(ctx) && r.contains_pointer() {
            ctx.layer_painter(r.layer_id).rect_stroke(
                r.interact_rect,
                6,
                egui::Stroke::new(2.0, accent_colour()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(p) = r.dnd_release_payload::<DragPaths>() {
            actions.push(Action::Move(p.0.clone(), e.path.clone()));
        }
    }

    let targets: Vec<PathBuf> = if selected.contains(&e.path) { selected.iter().cloned().collect() } else { vec![e.path.clone()] };
    r.context_menu(|ui| {
        if e.is_dir {
            if ui.button("Open").clicked() {
                actions.push(Action::Go(cx.pi, e.path.clone()));
                ui.close();
            }
            if ui.button("Open in new tab").clicked() {
                actions.push(Action::OpenTab(cx.pi, e.path.clone()));
                ui.close();
            }
        } else {
            if ui.button("Open").clicked() {
                actions.push(Action::OpenFile(e.path.clone()));
                ui.close();
            }
            // only for a file you have marked as runnable (double-click never runs anything)
            if e.usage.is_none() && !archive::inside(&e.path) && run::runnable(&e.path) && ui.button("Run in terminal").clicked() {
                actions.push(Action::RunInTerminal(e.path.clone()));
                ui.close();
            }
            // Privacy: one file at a time, on this computer (not inside an archive, not a cloud file not downloaded yet)
            let local = !matches!(drive_avail(cx.st, e), Some(cloud::Avail::Cloud | cloud::Avail::Partial));
            if targets.len() == 1 && e.usage.is_none() && !archive::inside(&e.path) && local {
                privacy_menu(ui, e, actions);
            }
        }
        ui.separator();
        if ui.button("Cut").clicked() {
            actions.push(Action::Cut(targets.clone()));
            ui.close();
        }
        if ui.button("Copy").clicked() {
            actions.push(Action::Copy(targets.clone()));
            ui.close();
        }
        if ui.add_enabled(cx.clip_has, egui::Button::new("Paste")).clicked() {
            actions.push(Action::Paste(cx.pi));
            ui.close();
        }
        if ui.add_enabled(targets.len() == 1, egui::Button::new("Rename")).clicked() {
            actions.push(Action::Rename(e.path.clone()));
            ui.close();
        }
        if ui.button("Move to trash").clicked() {
            actions.push(Action::Trash(targets.clone()));
            ui.close();
        }
        if ui.button("Copy path").clicked() {
            actions.push(Action::CopyPath(e.path.clone()));
            ui.close();
        }
        if e.usage.is_none() && !archive::inside(&e.path) && ui.button("Properties").clicked() {
            actions.push(Action::Properties(cx.pi));
            ui.close();
        }
        let avail = drive_avail(cx.st, e);
        if matches!(avail, Some(cloud::Avail::Failed)) && ui.button("Retry sync").clicked() {
            actions.push(Action::RetrySync(e.path.clone()));
            ui.close();
        }
        if matches!(avail, Some(cloud::Avail::Cloud | cloud::Avail::Partial))
            && ui.button("Download to this computer").on_hover_text("Keep a copy here, so it opens at once and works offline").clicked()
        {
            actions.push(Action::Download(targets.clone()));
            ui.close();
        }
        // Archives (7-Zip): extract from one, or compress the selection into one.
        if e.usage.is_none() && !places::is_this_pc(&e.path) {
            let inside = archive::inside(&e.path);
            if archive::is_archive_file(&e.path) {
                ui.separator();
                if ui.button("Extract here").clicked() {
                    actions.push(Action::ExtractHere(targets.clone()));
                    ui.close();
                }
                if ui.button("Extract to a new folder").clicked() {
                    actions.push(Action::ExtractTo(targets.clone()));
                    ui.close();
                }
            } else if inside {
                ui.separator();
                if ui.button("Extract to a new folder").clicked() {
                    actions.push(Action::ExtractTo(targets.clone()));
                    ui.close();
                }
            } else {
                ui.separator();
                ui.menu_button("Compress to", |ui| {
                    if ui.button("ZIP").clicked() {
                        actions.push(Action::Compress(targets.clone(), Format::Zip));
                        ui.close();
                    }
                    if ui.button("7z").clicked() {
                        actions.push(Action::Compress(targets.clone(), Format::SevenZ));
                        ui.close();
                    }
                });
            }
        }
        ui.separator();
        ui.menu_button("Tags", |ui| {
            for t in &cx.st.tag_defs {
                let on = cx.st.tags.get(&e.path).is_some_and(|v| v.contains(&t.name));
                let label = RichText::new(format!("{} {}", if on { "\u{2714}" } else { "   " }, t.name)).color(rgb(t.color));
                if ui.button(label).clicked() {
                    for p in &targets {
                        actions.push(Action::ToggleTag(p.clone(), t.name.clone()));
                    }
                    ui.close();
                }
            }
        });
        if e.is_dir {
            ui.menu_button("Folder color", |ui| {
                for t in &cx.st.tag_defs {
                    if ui.button(RichText::new(&t.name).color(rgb(t.color))).clicked() {
                        actions.push(Action::SetColor(e.path.clone(), Some(t.color)));
                        ui.close();
                    }
                }
                if ui.button("Clear").clicked() {
                    actions.push(Action::SetColor(e.path.clone(), None));
                    ui.close();
                }
            });
            let fav = cx.st.favorites.contains(&e.path);
            if ui.button(if fav { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                actions.push(Action::ToggleFav(e.path.clone()));
                ui.close();
            }
        }
    });
}

// ---------------------------------------------------------------------------------------------
// Look and feel, after EdenExplorer: icon toolbar, breadcrumb address bar, purple tabs,
// a sidebar of Places / Favorites / Storage (with usage bars), and a "This PC" view.
// ---------------------------------------------------------------------------------------------

fn icon_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, tip: &str, enabled: bool, active: bool) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), sense);
    let r = CornerRadius::same(8);
    if active {
        ui.painter().rect_filled(rect, r, pal.select);
    } else if enabled && resp.hovered() {
        ui.painter().rect_filled(rect, r, pal.hover);
    }
    let col = if enabled { pal.text } else { pal.dim.gamma_multiply(0.45) };
    icons::paint(ui.painter(), rect.shrink(7.0), icon, col);
    resp.on_hover_text(tip)
}

fn entry_icon(ui: &mut egui::Ui, pal: &Palette, st: &State, e: &Entry, size: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint_entry_icon(ui.painter(), pal, st, e, rect);
}

/// A listing entry's icon. Only folders take a colour (the accent, or their colour tag); a file always shows its own type's icon,
/// and a colour tag on a file is a small dot in the corner. Large icons carry the file's extension.
fn paint_entry_icon(p: &egui::Painter, pal: &Palette, st: &State, e: &Entry, rect: Rect) {
    let tint = st.color_for(&e.path).map(rgb);
    if e.usage.is_some() {
        icons::paint(p, rect, Icon::Drive, pal.drive);
        return;
    }
    if e.is_dir {
        if !(tint.is_none() && sys_entry_icon(p, rect, e)) {
            icons::paint(p, rect, Icon::Folder, tint.unwrap_or(pal.folder));
        }
        return;
    }
    if !sys_entry_icon(p, rect, e) {
        let icon = if archive::is_archive_name(&e.path) { Icon::Archive } else { Icon::File };
        icons::paint(p, rect, icon, if icon == Icon::Archive { pal.amber } else { pal.file });
    }
    if rect.width() >= 40.0 {
        if let Some(ext) =
            e.path.extension().map(|x| x.to_string_lossy().to_uppercase()).filter(|x| !x.is_empty() && x.chars().count() <= 5)
        {
            let font = egui::FontId::proportional((rect.width() * 0.16).clamp(8.0, 13.0));
            let g = p.layout_no_wrap(ext, font, Color32::WHITE);
            let pad = vec2(4.0, 1.5);
            let at = pos2(rect.center().x, rect.top() + rect.height() * 0.3);
            let bg = Rect::from_center_size(at, g.size() + pad * 2.0);
            p.rect_filled(bg, CornerRadius::same(4), Color32::from_black_alpha(150));
            p.galley(bg.min + pad, g, Color32::WHITE);
        }
    }
    if let Some(c) = tint {
        let r = (rect.width() * 0.12).clamp(3.0, 7.0);
        let at = rect.right_bottom() - vec2(r, r);
        p.circle_filled(at, r + 1.0, pal.bg);
        p.circle_filled(at, r, c);
    }
}

/// The system theme's icon for a listing entry (a tinted folder keeps the drawn one, so colour tags still show).
fn sys_entry_icon(p: &egui::Painter, rect: Rect, e: &Entry) -> bool {
    let name = e.path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    if e.usage.is_some() {
        false // drives keep the drawn drive icon
    } else if e.is_dir {
        sysicon::paint(p, rect, &sysicon::folder_names(&name))
    } else if archive::is_archive_name(&e.path) {
        sysicon::paint(p, rect, &["application-zip", "package-x-generic"])
    } else {
        sysicon::paint(p, rect, &sysicon::file_names(&name))
    }
}

/// Sync-status badge, drawn as shapes (no font needed).
fn paint_badge(p: &egui::Painter, center: egui::Pos2, b: Badge, pal: &Palette) {
    let center = egui::pos2(p.round_to_pixel_center(center.x), p.round_to_pixel_center(center.y));
    let (fill, kind) = match b {
        Badge::Synced => (pal.green, 0),
        Badge::SizeOnly => (pal.amber, 1),
        Badge::Problem => (pal.red, 2),
        Badge::Unverified => (pal.dim, 3),
        Badge::ContainsLink => (Color32::from_rgb(110, 150, 220), 3),
    };
    p.circle_filled(center, 6.5, fill);
    let w = egui::Stroke::new(1.6, Color32::WHITE);
    match kind {
        0 => {
            p.add(egui::Shape::line(vec![center + vec2(-3.0, 0.2), center + vec2(-0.8, 2.6), center + vec2(3.2, -2.4)], w));
        }
        1 => {
            p.line_segment([center + vec2(0.0, -3.4), center + vec2(0.0, 0.6)], w);
            p.circle_filled(center + vec2(0.0, 3.0), 1.0, Color32::WHITE);
        }
        2 => {
            p.line_segment([center + vec2(-2.6, -2.6), center + vec2(2.6, 2.6)], w);
            p.line_segment([center + vec2(2.6, -2.6), center + vec2(-2.6, 2.6)], w);
        }
        _ => {
            p.circle_filled(center, 1.6, Color32::WHITE);
        }
    }
}

/// Proton Drive file status: only for files under the mount, and only when Settings has it on.
fn drive_avail(st: &State, e: &Entry) -> Option<cloud::Avail> {
    if !st.drive_status || e.usage.is_some() {
        return None;
    }
    let root = places::proton_drive_candidates().into_iter().find(|r| e.path.starts_with(r))?;
    if e.is_dir {
        cloud::folder_availability(&root, &e.path)
    } else {
        cloud::availability(&root, &e.path, e.size)
    }
}

/// The Status column's marks, like Windows: a green ring with a check = on this computer; a blue cloud outline = in Proton Drive
/// (a partly downloaded file counts as in the cloud until it is whole).
fn paint_avail(p: &egui::Painter, center: egui::Pos2, a: cloud::Avail, pal: &Palette) {
    // on a pixel's centre, so the marks inside a circle sit in its middle (a centre between pixels shifted them half a pixel)
    let center = egui::pos2(p.round_to_pixel_center(center.x), p.round_to_pixel_center(center.y));
    let w = |c: Color32| egui::Stroke::new(1.6, c);
    match a {
        cloud::Avail::Local => {
            p.circle_stroke(center, 7.0, w(pal.green));
            p.add(egui::Shape::line(vec![center + vec2(-3.2, 0.2), center + vec2(-0.9, 2.8), center + vec2(3.4, -2.6)], w(pal.green)));
        }
        cloud::Avail::Syncing => {
            // two half circles chasing each other, each ending in an arrow head
            let blue = Color32::from_rgb(70, 130, 220);
            for sign in [1.0f32, -1.0] {
                let pt = |deg: f32| {
                    let t = deg.to_radians();
                    center + vec2(t.cos(), -t.sin()) * 5.6 * sign
                };
                let arc: Vec<egui::Pos2> = (0..=12).map(|k| pt(160.0 - 125.0 * k as f32 / 12.0)).collect();
                let tip = pt(28.0);
                let back = pt(46.0);
                let dir = (tip - back).normalized();
                let side = vec2(-dir.y, dir.x);
                p.add(egui::Shape::line(arc, w(blue)));
                p.add(egui::Shape::line(vec![tip - dir * 3.2 + side * 2.6, tip + dir * 0.6, tip - dir * 3.2 - side * 2.6], w(blue)));
            }
        }
        cloud::Avail::Failed => {
            let red = Color32::from_rgb(205, 60, 50);
            p.circle_filled(center, 7.0, red);
            p.line_segment([center + vec2(-2.8, -2.8), center + vec2(2.8, 2.8)], w(Color32::WHITE));
            p.line_segment([center + vec2(2.8, -2.8), center + vec2(-2.8, 2.8)], w(Color32::WHITE));
        }
        cloud::Avail::Partial | cloud::Avail::Cloud => {
            let blue = Color32::from_rgb(70, 130, 220);
            const CLOUD: &[(f32, f32)] = &[
                (6.8, 1.0),
                (6.9, 1.3),
                (6.9, 1.6),
                (6.9, 2.0),
                (6.8, 2.3),
                (6.7, 2.6),
                (6.6, 2.9),
                (6.5, 3.1),
                (6.3, 3.4),
                (6.1, 3.6),
                (5.9, 3.9),
                (5.6, 4.1),
                (5.3, 4.2),
                (5.1, 4.4),
                (4.8, 4.5),
                (4.5, 4.5),
                (4.2, 4.6),
                (-3.8, 4.6),
                (-4.1, 4.6),
                (-4.4, 4.5),
                (-4.7, 4.5),
                (-5.0, 4.3),
                (-5.3, 4.2),
                (-5.6, 4.0),
                (-5.8, 3.8),
                (-6.0, 3.6),
                (-6.2, 3.4),
                (-6.4, 3.1),
                (-6.5, 2.8),
                (-6.7, 2.5),
                (-6.7, 2.2),
                (-6.8, 1.9),
                (-6.8, 1.6),
                (-6.8, 1.3),
                (-6.7, 1.0),
                (-6.7, 0.7),
                (-6.5, 0.4),
                (-6.4, 0.1),
                (-6.2, -0.2),
                (-6.0, -0.4),
                (-5.8, -0.6),
                (-5.6, -0.8),
                (-5.3, -1.0),
                (-5.0, -1.1),
                (-4.7, -1.2),
                (-4.4, -1.3),
                (-4.3, -1.7),
                (-4.2, -2.1),
                (-4.0, -2.5),
                (-3.9, -2.9),
                (-3.6, -3.2),
                (-3.4, -3.6),
                (-3.1, -3.9),
                (-2.8, -4.1),
                (-2.4, -4.4),
                (-2.0, -4.5),
                (-1.6, -4.7),
                (-1.2, -4.8),
                (-0.8, -4.9),
                (-0.4, -4.9),
                (0.0, -4.9),
                (0.4, -4.8),
                (0.8, -4.7),
                (1.2, -4.5),
                (1.6, -4.4),
                (1.9, -4.1),
                (2.3, -3.9),
                (2.6, -3.6),
                (2.8, -3.2),
                (3.1, -2.9),
                (3.2, -2.5),
                (3.4, -2.1),
                (3.5, -1.7),
                (3.9, -1.4),
                (4.2, -1.4),
                (4.5, -1.3),
                (4.8, -1.2),
                (5.1, -1.1),
                (5.4, -1.0),
                (5.7, -0.8),
                (5.9, -0.6),
                (6.1, -0.4),
                (6.3, -0.2),
                (6.5, 0.1),
                (6.6, 0.4),
                (6.8, 0.7),
                (6.8, 1.0),
            ];
            let path: Vec<egui::Pos2> = CLOUD.iter().map(|&(x, y)| center + vec2(x, y) * 0.95 - vec2(0.0, 1.2)).collect();
            p.add(egui::Shape::closed_line(path, w(blue)));
        }
    }
}

fn avail_tip(a: cloud::Avail) -> &'static str {
    match a {
        cloud::Avail::Local => "On this computer (also in Proton Drive)",
        cloud::Avail::Partial => "Partly downloaded; the rest comes from Proton Drive when opened",
        cloud::Avail::Cloud => "Only in Proton Drive; opening it downloads it",
        cloud::Avail::Syncing => "Syncing to Proton Drive",
        cloud::Avail::Failed => "Could not sync to Proton Drive (for a folder: something inside it). Right-click it and choose Retry sync",
    }
}

fn badge_tip(b: Badge) -> &'static str {
    match b {
        Badge::Synced => "Synced with Proton Drive (verified by hash)",
        Badge::SizeOnly => "Same size as Proton Drive, but contents could not be hash-checked",
        Badge::Problem => "Differs from Proton Drive (see Verify now)",
        Badge::Unverified => "Linked to Proton Drive; not verified yet",
        Badge::ContainsLink => "Contains a folder linked to Proton Drive",
    }
}

fn name_text(e: &Entry, st: &State, pal: &Palette) -> RichText {
    let mut t = RichText::new(&e.name).color(pal.text);
    if let Some(c) = st.color_for(&e.path) {
        t = t.color(rgb(c));
    }
    t
}

fn tab_title(dir: &Path) -> String {
    if places::is_this_pc(dir) {
        return places::pc_name();
    }
    if let Some(name) = places::disk_name(dir) {
        return name;
    }
    // The dock's Trash opens this folder.
    if dir.ends_with("Trash/files") {
        return "Trash".into();
    }
    dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| dir.to_string_lossy().into_owned())
}

fn tab_strip(ui: &mut egui::Ui, pane: &Pane, pi: usize, pal: &Palette, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 0.0);
        for i in 0..pane.tabs.len() {
            let t = &pane.tabs[i];
            let dir = t.dir();
            let title = tab_title(&dir);
            let galley = ui.painter().layout_no_wrap(title, FontId::proportional(13.5), pal.text);
            let pin_w = if t.tab.pinned { 14.0 } else { 0.0 };
            let w = (10.0 + 18.0 + 6.0 + pin_w + galley.size().x + 8.0 + 20.0 + 6.0).min(230.0);
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click_and_drag());
            let active = i == pane.active;
            // tabs can be dragged: along the strip to a new place, or out of the window into a window of their own
            resp.dnd_set_drag_payload(DragTab(pi, i));
            let after = ui.input(|inp| inp.pointer.hover_pos()).is_some_and(|p| p.x > rect.center().x);
            if resp.dnd_hover_payload::<DragTab>().is_some_and(|d| d.0 == pi && d.1 != i) {
                let x = if after { rect.right() + 1.0 } else { rect.left() - 2.0 };
                ui.painter().line_segment([pos2(x, rect.top() + 4.0), pos2(x, rect.bottom() - 4.0)], egui::Stroke::new(2.0, pal.accent));
            }
            if let Some(d) = resp.dnd_release_payload::<DragTab>() {
                if d.0 == pi {
                    actions.push(Action::MoveTab(pi, d.1, if after { i + 1 } else { i }));
                }
            }
            let fill = if active {
                pal.tab_active
            } else if resp.hovered() {
                pal.hover
            } else {
                Color32::TRANSPARENT
            };
            ui.painter().rect_filled(rect, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, fill);
            let icon = if places::is_this_pc(&dir) {
                Icon::Computer
            } else if places::is_virtual(&dir) {
                Icon::Drive
            } else {
                Icon::Folder
            };
            let icon_col = if active {
                pal.text
            } else if places::is_virtual(&dir) {
                pal.dim
            } else {
                pal.folder
            };
            let tab_rect = Rect::from_min_size(rect.min + vec2(10.0, 7.0), vec2(18.0, 18.0));
            let dir_name = dir.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
            let sys_names = if places::is_virtual(&dir) { vec![] } else { sysicon::folder_names(&dir_name) };
            if sys_names.is_empty() || !sysicon::paint(ui.painter(), tab_rect, &sys_names) {
                icons::paint(ui.painter(), tab_rect, icon, icon_col);
            }
            let mut x = rect.left() + 10.0 + 18.0 + 6.0;
            if t.tab.pinned {
                icons::paint(ui.painter(), Rect::from_min_size(pos2(x - 2.0, rect.top() + 9.0), vec2(14.0, 14.0)), Icon::Pin, pal.accent);
                x += 14.0;
            }
            let clip = Rect::from_min_max(pos2(x, rect.top()), pos2(rect.right() - 26.0, rect.bottom()));
            ui.painter().with_clip_rect(clip).galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, pal.text);
            // close button
            let cr = Rect::from_center_size(pos2(rect.right() - 15.0, rect.center().y), vec2(18.0, 18.0));
            let cresp = ui.interact(cr, resp.id.with("close"), Sense::click());
            if cresp.hovered() {
                ui.painter().circle_filled(cr.center(), 9.0, pal.hover.gamma_multiply(1.6));
            }
            icons::paint(ui.painter(), cr.shrink(3.0), Icon::Close, if active { pal.text } else { pal.dim });

            if cresp.clicked() || resp.middle_clicked() {
                actions.push(Action::CloseTab(pi, i));
            } else if resp.clicked() {
                actions.push(Action::SelectTab(pi, i));
            }
            if let Some(p) = resp.dnd_release_payload::<DragPaths>() {
                actions.push(Action::Move(p.0.clone(), dir.clone()));
            }
            // Holding files over another tab for a moment opens it, so they can go into one of its folders.
            if !active && egui::DragAndDrop::has_payload_of_type::<DragPaths>(ui.ctx()) {
                let key = egui::Id::new(("minos-tab-hover", pi, i));
                if resp.contains_pointer() {
                    let now = ui.input(|inp| inp.time);
                    let since: f64 = ui.ctx().data_mut(|d| *d.get_temp_mut_or_insert_with(key, || now));
                    if now - since > 0.5 {
                        actions.push(Action::SelectTab(pi, i));
                        ui.ctx().data_mut(|d| d.remove::<f64>(key));
                    } else {
                        ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    }
                } else {
                    ui.ctx().data_mut(|d| d.remove::<f64>(key));
                }
            }
            let pinned = t.tab.pinned;
            resp.context_menu(|ui| {
                if ui.button(if pinned { "Unpin tab" } else { "Pin tab" }).clicked() {
                    actions.push(Action::PinTab(pi, i));
                    ui.close();
                }
                if ui.button("Close tab").clicked() {
                    actions.push(Action::CloseTab(pi, i));
                    ui.close();
                }
            });
        }
        if icon_button(ui, pal, Icon::Plus, "New tab (Ctrl+T)", true, false).clicked() {
            actions.push(Action::NewTab(pi));
        }
    });
    // a tab being dragged from this strip: its name follows the pointer, and let go outside the window it opens in a
    // window of its own (as browsers do)
    let ctx = ui.ctx().clone();
    if let Some(d) = egui::DragAndDrop::payload::<DragTab>(&ctx).filter(|d| d.0 == pi) {
        let window = ctx.viewport_rect();
        let at = ctx.input(|inp| inp.pointer.latest_pos());
        let outside = at.is_none_or(|p| !window.shrink(2.0).contains(p));
        if let (Some(at), Some(t)) = (at, pane.tabs.get(d.1)) {
            let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("minos-tab-drag")));
            let text = if outside && pane.tabs.len() > 1 { format!("{}  (new window)", tab_title(&t.dir())) } else { tab_title(&t.dir()) };
            let galley = painter.layout_no_wrap(text, FontId::proportional(13.5), pal.text);
            let r = Rect::from_min_size(at + vec2(12.0, 8.0), galley.size() + vec2(16.0, 8.0));
            painter.rect_filled(r, CornerRadius::same(6), pal.tab_active);
            painter.galley(r.min + vec2(8.0, 4.0), galley, pal.text);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if ctx.input(|inp| inp.pointer.any_released()) && outside {
            actions.push(Action::DetachTab(pi, d.1));
            egui::DragAndDrop::clear_payload(&ctx);
        }
    }
}

fn crumbs(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut v = vec![(places::pc_name(), places::this_pc())];
    if places::is_this_pc(dir) {
        return v;
    }
    if let Some(name) = places::disk_name(dir) {
        v.push((name, dir.to_path_buf()));
        return v;
    }
    let mut chain: Vec<PathBuf> = dir.ancestors().map(Path::to_path_buf).collect();
    chain.reverse();
    for p in chain {
        if p.as_os_str().is_empty() {
            continue;
        }
        let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.to_string_lossy().into_owned());
        v.push((name, p));
    }
    v
}

/// The address bar: clickable path segments (also drop targets); click the empty part to type a path.
fn breadcrumb(ui: &mut egui::Ui, pal: &Palette, t: &mut TabView, pi: usize, width: f32, actions: &mut Vec<Action>) {
    let (rect, bg) = ui.allocate_exact_size(vec2(width.max(60.0), 30.0), Sense::click());
    ui.painter().rect_filled(rect, CornerRadius::same(9), pal.field);
    ui.painter().rect_stroke(rect, CornerRadius::same(9), egui::Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);

    if t.editing {
        let inner = rect.shrink2(vec2(8.0, 3.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(Layout::left_to_right(Align::Center)));
        let id = egui::Id::new(("minos-address", pi));
        if t.edit_fresh {
            // the whole path selected, ready to copy or type over; focus first, or egui drops the selection (see inline_rename)
            let mut state = egui::TextEdit::load_state(child.ctx(), id).unwrap_or_default();
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(t.address.chars().count()));
            state.cursor.set_char_range(Some(all));
            state.store(child.ctx(), id);
            child.ctx().memory_mut(|m| m.request_focus(id));
            t.edit_fresh = false;
        }
        let r = child.add(egui::TextEdit::singleline(&mut t.address).id(id).frame(egui::Frame::NONE).desired_width(inner.width()));
        let (enter, esc) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if r.lost_focus() || esc {
            if enter && !esc {
                let expanded = expand_address(&t.address, |k| std::env::var(k).ok());
                actions.push(Action::Go(pi, PathBuf::from(expanded)));
            }
            t.editing = false;
        }
        return;
    }

    let dir = t.dir();
    let segs = crumbs(&dir);
    let font = FontId::proportional(14.0);
    let galleys: Vec<_> = segs.iter().map(|(n, _)| ui.painter().layout_no_wrap(n.clone(), font.clone(), pal.text)).collect();
    let seg_w = |i: usize| galleys[i].size().x + 14.0 + if i == 0 { 22.0 } else { 0.0 };
    let avail = rect.width() - 20.0;
    let total = |from: usize| -> f32 { (from..segs.len()).map(|i| seg_w(i) + 14.0).sum::<f32>() + if from > 0 { 20.0 } else { 0.0 } };
    let mut start = 0;
    while start + 1 < segs.len() && total(start) > avail {
        start += 1;
    }

    let mut x = rect.left() + 10.0;
    let cy = rect.center().y;
    if start > 0 {
        ui.painter().text(pos2(x, cy), Align2::LEFT_CENTER, "...", font.clone(), pal.dim);
        x += 20.0;
    }
    for i in start..segs.len() {
        let w = seg_w(i);
        let srect = Rect::from_min_size(pos2(x, rect.top() + 3.0), vec2(w, 24.0));
        let r = ui.interact(srect, bg.id.with(("crumb", i)), Sense::click());
        if r.hovered() {
            ui.painter().rect_filled(srect, CornerRadius::same(6), pal.hover);
        }
        let mut tx = srect.left() + 7.0;
        if i == 0 {
            icons::paint(ui.painter(), Rect::from_min_size(pos2(tx, cy - 8.0), vec2(16.0, 16.0)), Icon::Computer, pal.dim);
            tx += 22.0;
        }
        ui.painter().galley(pos2(tx, cy - galleys[i].size().y / 2.0), galleys[i].clone(), pal.text);
        if r.clicked() {
            actions.push(Action::Go(pi, segs[i].1.clone()));
        }
        if let Some(d) = r.dnd_release_payload::<DragPaths>() {
            actions.push(Action::Move(d.0.clone(), segs[i].1.clone()));
        }
        x += w;
        if i + 1 < segs.len() {
            icons::paint(ui.painter(), Rect::from_min_size(pos2(x - 2.0, cy - 7.0), vec2(14.0, 14.0)), Icon::Chevron, pal.dim);
            x += 14.0;
        }
    }
    if bg.clicked() {
        t.editing = true;
        t.edit_fresh = true;
        t.address = if places::is_this_pc(&dir) { String::new() } else { dir.to_string_lossy().into_owned() };
    }
}

/// A group's heading in the list (Group by): its name, and a line to the right.
fn group_heading(ui: &mut egui::Ui, pal: &Palette, title: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 26.0), Sense::hover());
    let g = ui.painter().layout_no_wrap(title.to_string(), FontId::proportional(13.0), pal.accent);
    let y = rect.center().y;
    let end = rect.left() + 4.0 + g.size().x;
    ui.painter().galley(pos2(rect.left() + 4.0, y - g.size().y / 2.0), g, pal.accent);
    if end + 10.0 < rect.right() {
        ui.painter().hline(end + 10.0..=rect.right() - 4.0, y, egui::Stroke::new(1.0, pal.border));
    }
}

/// How wide `s` is in `font`.
fn text_width(ui: &egui::Ui, s: &str, font: &FontId) -> f32 {
    ui.painter().layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE).size().x
}

/// `s` with every digit made the font's widest digit: measured, it is as wide as any number of that shape can be.
fn widest_digits(ui: &egui::Ui, s: &str, font: &FontId) -> String {
    let w = |c: char| text_width(ui, &c.to_string(), font);
    let digit = ('0'..='9').max_by(|a, b| w(*a).total_cmp(&w(*b))).unwrap_or('0');
    s.chars().map(|c| if c.is_ascii_digit() { digit } else { c }).collect()
}

/// The preview pane's narrowest: room for the details' usual labels and a date on one line in the font in use (a larger text
/// size widens it), never under PREVIEW_MIN.
fn preview_min(ui: &egui::Ui) -> f32 {
    let font = FontId::proportional(12.5);
    let label = ["Type", "Size", "Modified", "Location", "Contains"].map(|l| text_width(ui, l, &font)).into_iter().fold(0.0, f32::max);
    let date = text_width(ui, &widest_digits(ui, "2026-01-01 00:00", &font), &font);
    // the frame's margins (14 a side), the grid's gap (10), and a few points so nothing touches the edge
    (28.0 + label + 10.0 + date + 6.0).ceil().clamp(PREVIEW_MIN, PREVIEW_MAX)
}

/// The widths Date modified, Type and Size need to show the listed entries (`order` into `shown`) in full, their headings (with
/// the sort arrow) included. Measured
/// once per listing for pane `pi` and kept in egui's memory: only the distinct shapes are measured (a digit counts as the widest
/// digit, so a later "8" never gets cut), never every row on every frame.
fn fitted_widths(ui: &egui::Ui, pi: usize, shown: &[Entry], order: &[usize]) -> [f32; 3] {
    let listed = || order.iter().filter_map(|&i| shown.get(i));
    // what the listing is, cheaply: a change in it (another folder, a file added or grown, hidden items shown) measures again
    let print = listed().fold(order.len() as u64, |h, e| {
        h.rotate_left(7) ^ e.size ^ ((e.name.len() as u64) << 32) ^ u64::from(e.is_dir) ^ u64::from(e.usage.is_some()) << 1
    });
    let id = egui::Id::new(("minos-fit", pi));
    if let Some((p, w)) = ui.ctx().data(|d| d.get_temp::<(u64, [f32; 3])>(id)) {
        if p == print {
            return w;
        }
    }
    let font = egui::TextStyle::Body.resolve(ui.style());
    let width = |s: &str| text_width(ui, s, &font);
    let shape = |s: &str| widest_digits(ui, s, &font);
    // a heading: its words, and room for the sort arrow (12 px and a gap) where it sorts
    let head = |s: &str, sorts: bool| width(s) + if sorts { 12.0 + ui.spacing().item_spacing.x } else { 0.0 };
    let mut types = BTreeSet::new();
    let mut sizes = BTreeSet::new();
    for e in listed() {
        types.insert(listing::type_name(e));
        if !e.is_dir {
            sizes.insert(shape(&listing::human_size(e.size)));
        }
    }
    let date = width(&shape("2026-01-01 00:00")).max(head("Date modified", true));
    let kind = types.iter().map(|t| width(t)).fold(head("Type", false), f32::max).min(320.0);
    let size = sizes.iter().map(|s| width(s)).fold(head("Size", true), f32::max);
    // a little air after the longest text, so it never touches the next column
    let w = [date, kind, size].map(|x| (x + 12.0).ceil());
    ui.ctx().data_mut(|d| d.insert_temp(id, (print, w)));
    w
}

/// A heading of the list: a click sorts by it (where it sorts); `menu`, when given, opens on a right-click (the columns).
fn header_cell(
    ui: &mut egui::Ui,
    pal: &Palette,
    label: &str,
    key: Option<SortKey>,
    sort: Sort,
    new_sort: &mut Sort,
    menu: Option<&dyn Fn(&mut egui::Ui)>,
) {
    ui.horizontal(|ui| {
        let arrow = key.is_some_and(|k| sort.key == k);
        // the words give way to the sort arrow in a narrow column, shortened with "..."
        let room = (ui.available_width() - if arrow { 12.0 + ui.spacing().item_spacing.x } else { 0.0 }).max(0.0);
        let r = ui
            .allocate_ui(vec2(room, ui.available_height()), |ui| {
                ui.add(egui::Label::new(RichText::new(label).strong().color(pal.text)).truncate().sense(Sense::click()))
            })
            .inner;
        if let Some(k) = key {
            if arrow {
                let (rect, _) = ui.allocate_exact_size(vec2(12.0, 12.0), Sense::hover());
                icons::paint(ui.painter(), rect, if sort.descending { Icon::SortDown } else { Icon::SortUp }, pal.text);
            }
            if r.clicked() {
                *new_sort = Sort { key: k, descending: sort.key == k && !sort.descending };
            }
        }
        if let Some(menu) = menu {
            r.context_menu(|ui| menu(ui));
        }
    });
}

/// The details list's columns, ticked where shown: the layout menu's Columns and a right-click on the list's header. Name is
/// always there; Status only while Settings shows Proton Drive's marks.
pub(crate) fn columns_menu(ui: &mut egui::Ui, st: &State, toggle: &mut dyn FnMut(ListColumn)) {
    ui.label(RichText::new("Columns").small());
    ui.add_enabled(false, egui::Checkbox::new(&mut true, "Name"));
    for c in ListColumn::ALL {
        if c == ListColumn::Status && !st.drive_status {
            continue;
        }
        let mut on = st.columns.shows(c);
        let label = if c == ListColumn::Status { "Status (Proton Drive folders)" } else { c.label() };
        if ui.checkbox(&mut on, label).clicked() {
            toggle(c);
        }
    }
}

fn usage_bar(ui: &mut egui::Ui, pal: &Palette, total: u64, free: u64, width: f32) {
    let frac = listing::used_fraction(total, free);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 16.0), Sense::hover());
    ui.painter().rect_filled(rect, CornerRadius::same(3), pal.border.gamma_multiply(0.5));
    let mut fill = rect;
    fill.set_width(rect.width() * frac);
    ui.painter().rect_filled(fill, CornerRadius::same(3), theme::usage_color(pal, frac));
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, format!("{:.0}%", frac * 100.0), FontId::proportional(11.0), Color32::WHITE);
}

#[allow(clippy::too_many_arguments)]
fn draw_pane(
    ui: &mut egui::Ui,
    pane: &mut Pane,
    pi: usize,
    focused: bool,
    st: &State,
    cloud_ctx: (&[Link], &BTreeMap<String, Report>),
    clip_has: bool,
    actions: &mut Vec<Action>,
) {
    // Whatever does not fit stays inside this pane instead of painting over its neighbour.
    ui.shrink_clip_rect(ui.max_rect());
    if ui.input(|i| i.pointer.any_pressed()) && ui.rect_contains_pointer(ui.max_rect()) {
        actions.push(Action::Focus(pi));
    }
    let hidden = st.show_hidden;
    let pal = palette_for(st.dark, st.accent()).translucent(st.opacity);
    let cx = Ctx { st, links: cloud_ctx.0, reports: cloud_ctx.1, clip_has, pi };

    // with one pane the tabs are in the title strip and the navigation row runs across the window (App::ui); in split view
    // each pane has its own
    if st.split {
        tab_strip(ui, pane, pi, &pal, actions);
        ui.add_space(2.0);
        topbar::nav_bar(ui, pane, pi, &pal, st, pi == 0, actions);
        ui.add_space(4.0);
    }
    topbar::command_bar(ui, pane, pi, &pal, st, clip_has, actions);
    ui.add_space(4.0);

    let sort = pane.sort;
    let active = pane.active;
    let t = &mut pane.tabs[active];
    if let Some(err) = &t.error {
        ui.colored_label(pal.red, err);
        return;
    }
    if t.pending.is_some() && t.entries.is_empty() {
        ui.colored_label(pal.dim, format!("Waiting for the drive to answer: {}", t.dir().display()));
        return;
    }

    let searching = t.results.is_some();
    let in_pc = places::is_virtual(&t.dir());
    let in_archive = archive::split(&t.dir()).is_some();
    let dir = t.dir();
    let grouping::Arranged { order, heads } = t.arranged(hidden, st.group);
    let TabView { entries, results, selected, anchor, .. } = t;
    let shown: &[Entry] = results.as_deref().unwrap_or(entries);
    let mut new_sort = sort;
    // a keyboard move asks for its row to be scrolled into view
    let want_row: Option<usize> = ui.ctx().data_mut(|d| {
        let id = egui::Id::new("minos-scroll-row");
        match d.get_temp::<(usize, usize)>(id) {
            Some((p, r)) if p == cx.pi => {
                d.remove::<(usize, usize)>(id);
                Some(r)
            }
            _ => None,
        }
    });

    // The list's empty space (all of it in an empty folder): registered before the rows, so a row takes its own clicks and
    // this only the clicks between and below them. Right-click opens the folder's menu (2026-11-08: there was none, so an
    // empty folder could not be pasted into with the mouse).
    let mut bg_area = ui.available_rect_before_wrap();
    bg_area.max.y -= 26.0; // the item count below the list
                           // a drag on it draws a box that selects what it covers (YGG-77)
    let bg = ui.interact(bg_area, ui.id().with(("minos-list-bg", cx.pi)), Sense::click_and_drag());
    let drag_scroll = drag_scrolls(ui.ctx());
    ui.ctx().data_mut(|d| d.remove::<Vec<(Rect, PathBuf)>>(egui::Id::new(("minos-item-rects", cx.pi))));
    let read_only = in_pc || in_archive;
    bg.context_menu(|ui| {
        if ui.add_enabled(clip_has && !read_only, egui::Button::new("Paste")).clicked() {
            actions.push(Action::Paste(cx.pi));
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(!read_only, egui::Button::new("New folder")).clicked() {
            actions.push(Action::NewFolder(cx.pi));
            ui.close();
        }
        if ui.add_enabled(!read_only, egui::Button::new("New file")).clicked() {
            actions.push(Action::NewFile(cx.pi));
            ui.close();
        }
        ui.separator();
        if ui.selectable_label(st.view == ViewMode::Details, "Details view").clicked() {
            actions.push(Action::SetView(ViewMode::Details));
            ui.close();
        }
        if ui.selectable_label(st.view == ViewMode::Gallery, "Gallery view").clicked() {
            actions.push(Action::SetView(ViewMode::Gallery));
            ui.close();
        }
        if ui.button("Refresh (F5)").clicked() {
            actions.push(Action::Reload);
            ui.close();
        }
    });

    match st.view {
        ViewMode::Details => {
            // The row, not the name, takes the clicks and drags (see entry_interact below).
            let ectx = ui.ctx().clone();
            // A Status column (like Windows) in Proton Drive folders, while it is chosen (the columns menu).
            let status_col = st.drive_status
                && st.columns.status
                && !in_pc
                && !in_archive
                && places::proton_drive_candidates().iter().any(|r| dir.starts_with(r));
            let name_cell = |ui: &mut egui::Ui, e: &Entry| {
                ui.horizontal(|ui| {
                    let cut = is_cut(ui.ctx(), &e.path);
                    ui.scope(|ui| {
                        if cut {
                            ui.multiply_opacity(CUT_OPACITY);
                        }
                        entry_icon(ui, &pal, st, e, 18.0);
                    });
                    if let Some(b) = cloud::badge_for(&e.path, cx.links, cx.reports) {
                        let (rect, r) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                        paint_badge(ui.painter(), rect.center(), b, &pal);
                        r.on_hover_text(badge_tip(b));
                    }
                    if !status_col {
                        if let Some(a) = drive_avail(cx.st, e) {
                            let (rect, r) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            paint_avail(ui.painter(), rect.center(), a, &pal);
                            r.on_hover_text(avail_tip(a));
                        }
                    }
                    if !inline_rename(ui, &pal, e) {
                        let lr = ui.add(egui::Label::new(name_text(e, st, &pal)).selectable(false));
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(egui::Id::new(("minos-name-rect", e.path.clone())), lr.rect.expand2(vec2(6.0, 4.0)))
                        });
                    }
                });
            };
            if in_pc {
                let mut tb = TableBuilder::new(ui);
                if let Some(r) = want_row {
                    tb = tb.scroll_to_row(r, None);
                }
                tb.striped(true)
                    .resizable(true)
                    .sense(Sense::click_and_drag())
                    .column(Column::initial(340.0).at_least(160.0).clip(true))
                    .column(Column::initial(90.0).at_least(60.0))
                    .column(Column::initial(210.0).at_least(120.0))
                    .column(Column::remainder().at_least(110.0))
                    .header(26.0, |mut h| {
                        h.col(|ui| header_cell(ui, &pal, "Name", None, sort, &mut new_sort, None));
                        h.col(|ui| header_cell(ui, &pal, "Type", None, sort, &mut new_sort, None));
                        h.col(|ui| header_cell(ui, &pal, "Size", None, sort, &mut new_sort, None));
                        h.col(|ui| header_cell(ui, &pal, "Usage", None, sort, &mut new_sort, None));
                    })
                    .body(|body| {
                        body.rows(30.0, order.len(), |mut row| {
                            let pos = row.index();
                            let e = &shown[order[pos]];
                            row.set_selected(selected.contains(&e.path));
                            row.col(|ui| name_cell(ui, e));
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.label(RichText::new("Folder").color(pal.dim));
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    if let Some((total, free)) = e.usage {
                                        ui.add(
                                            egui::Label::new(
                                                RichText::new(format!("{} / {}", listing::human_size(free), listing::human_size(total)))
                                                    .color(pal.dim),
                                            )
                                            .truncate(),
                                        );
                                    }
                                });
                            });
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    if let Some((total, free)) = e.usage {
                                        usage_bar(ui, &pal, total, free, (ui.available_width() - 14.0).clamp(80.0, 220.0));
                                    }
                                });
                            });
                            let r = row.response();
                            record_item(&ectx, cx.pi, Rect::from_min_size(r.rect.min, vec2(r.rect.width(), 30.0)), &e.path);
                            entry_interact(&ectx, &r, e, pos, &order, shown, selected, anchor, &cx, actions);
                        });
                    });
            } else {
                // The chosen columns, all of them and in their order after Name, at any width: in a narrow pane (the preview
                // pane open, split view) they all give up room alike rather than some going. Until 2026-11-26 a pane under
                // 560 points kept only Name and Size, and a wider one could push Size past its right edge.
                let cols = st.columns.shown(status_col);
                // the table's rows: each item (its place in `order`), with a group's heading before its first item (Group by)
                let rows: Vec<(usize, bool)> = (0..order.len())
                    .flat_map(|pos| heads.iter().any(|(p, _)| *p == pos).then_some((pos, true)).into_iter().chain([(pos, false)]))
                    .collect();
                // Date modified, Type and Size want just the width of what this folder shows in full; Name takes the rest
                let [date_w, type_w, size_w] = fitted_widths(ui, pi, shown, &order);
                let wants: Vec<(f32, f32)> = cols
                    .iter()
                    .map(|c| match c {
                        ListColumn::Status => (80.0, 56.0),
                        ListColumn::Modified => (date_w, 64.0),
                        ListColumn::Type => (type_w, 56.0),
                        ListColumn::Size => (size_w, 56.0),
                    })
                    .collect();
                let spacing = ui.spacing().item_spacing.x;
                let avail_w = ui.available_width() - ui.spacing().scroll.allocated_width();
                let widths = columns::fit(avail_w, spacing, &wants);
                let name_min = columns::name_floor(avail_w, spacing, &widths);
                // A dragged edge stays for the session; other columns, a folder whose widest date, type or size differs, or
                // a pane too narrow for them start fitted again (the widths are the table's id).
                let salt: Vec<u32> = cols.iter().map(|c| *c as u32).chain(widths.iter().map(|w| *w as u32)).collect();
                let mut tb = TableBuilder::new(ui)
                    .id_salt(("details", salt))
                    .striped(true)
                    .resizable(true)
                    .sense(Sense::click_and_drag())
                    .drag_to_scroll(drag_scroll);
                if let Some(r) = want_row.and_then(|w| rows.iter().position(|&(p, head)| p == w && !head)) {
                    tb = tb.scroll_to_row(r, None);
                }
                tb = tb.column(Column::remainder().at_least(name_min).clip(true).resizable(false));
                for w in &widths {
                    tb = tb.column(Column::initial(*w).at_least(w.min(40.0)).clip(true));
                }
                // a right-click on the header: which columns are shown
                let toggled = std::cell::Cell::new(None);
                let menu = |ui: &mut egui::Ui| columns_menu(ui, st, &mut |c| toggled.set(Some(c)));
                let dim = |ui: &mut egui::Ui, text: String| {
                    ui.add(egui::Label::new(RichText::new(text).color(pal.dim)).truncate());
                };
                tb.header(26.0, |mut h| {
                    let (_, r) = h.col(|ui| header_cell(ui, &pal, "Name", Some(SortKey::Name), sort, &mut new_sort, Some(&menu)));
                    r.context_menu(|ui| menu(ui));
                    for c in &cols {
                        let key = match c {
                            ListColumn::Modified => Some(SortKey::Modified),
                            ListColumn::Size => Some(SortKey::Size),
                            ListColumn::Status | ListColumn::Type => None,
                        };
                        let (_, r) = h.col(|ui| header_cell(ui, &pal, c.label(), key, sort, &mut new_sort, Some(&menu)));
                        r.context_menu(|ui| menu(ui));
                    }
                })
                .body(|body| {
                    body.rows(28.0, rows.len(), |mut row| {
                        let (pos, head) = rows[row.index()];
                        if head {
                            let title = heads.iter().find(|(p, _)| *p == pos).map(|(_, h)| h.as_str()).unwrap_or_default();
                            row.col(|ui| group_heading(ui, &pal, title));
                            for _ in &cols {
                                row.col(|_| {});
                            }
                            return;
                        }
                        let e = &shown[order[pos]];
                        row.set_selected(selected.contains(&e.path));
                        row.col(|ui| name_cell(ui, e));
                        for c in &cols {
                            row.col(|ui| {
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| match c {
                                    ListColumn::Status => {
                                        if let Some(a) = drive_avail(cx.st, e) {
                                            let (rect, r) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
                                            paint_avail(ui.painter(), rect.center(), a, &pal);
                                            r.on_hover_text(avail_tip(a));
                                        }
                                    }
                                    ListColumn::Modified => dim(ui, e.modified.map(fmt_time).unwrap_or_default()),
                                    ListColumn::Type => dim(ui, listing::type_name(e).to_string()),
                                    ListColumn::Size => dim(ui, if e.is_dir { String::new() } else { listing::human_size(e.size) }),
                                });
                            });
                        }
                        let r = row.response();
                        record_item(&ectx, cx.pi, Rect::from_min_size(r.rect.min, vec2(r.rect.width(), 28.0)), &e.path);
                        entry_interact(&ectx, &r, e, pos, &order, shown, selected, anchor, &cx, actions);
                    });
                });
                if let Some(c) = toggled.get() {
                    actions.push(Action::ToggleColumn(c));
                }
            }
        }
        ViewMode::Gallery => {
            let size = GALLERY_SIZES[st.gallery_size.min(GALLERY_SIZES.len() - 1)];
            let per_row = (ui.available_width() / (size + 12.0 + ui.spacing().item_spacing.x)).floor().max(1.0) as usize;
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("minos-cols"), per_row));
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .scroll_source(egui::containers::scroll_area::ScrollSource { drag: drag_scroll, ..Default::default() })
                .show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        for (pos, &i) in order.iter().enumerate() {
                            if let Some((_, title)) = heads.iter().find(|(p, _)| *p == pos) {
                                // a group starts on a line of its own, under its heading
                                if pos > 0 {
                                    ui.end_row();
                                }
                                ui.allocate_ui(vec2(ui.max_rect().width(), 28.0), |ui| group_heading(ui, &pal, title));
                                ui.end_row();
                            }
                            let e = &shown[i];
                            let cell = vec2(size + 12.0, size + 34.0);
                            let (rect, _) = ui.allocate_exact_size(cell, Sense::hover());
                            let r = ui.interact(rect, ui.id().with(("cell", pos)), Sense::click_and_drag());
                            if selected.contains(&e.path) {
                                ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                            } else if r.hovered() {
                                ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                            }
                            let img_rect = Rect::from_min_size(rect.min + vec2(6.0, 4.0), vec2(size, size));
                            let tint = st.color_for(&e.path).map(rgb);
                            let cut = is_cut(ui.ctx(), &e.path);
                            if !e.is_dir && !in_archive && is_image(&e.path) && ui.is_rect_visible(rect) {
                                let alpha = if cut { (255.0 * CUT_OPACITY) as u8 } else { 255 };
                                egui::Image::new(file_uri(&e.path))
                                    .fit_to_exact_size(img_rect.size())
                                    .corner_radius(6)
                                    .tint(Color32::from_white_alpha(alpha))
                                    .paint_at(ui, img_rect);
                            } else {
                                let mut painter = ui.painter().clone();
                                if cut {
                                    painter.multiply_opacity(CUT_OPACITY);
                                }
                                paint_entry_icon(&painter, &pal, st, e, img_rect);
                            }
                            let mut badge_x = -10.0;
                            if let Some(b) = cloud::badge_for(&e.path, cx.links, cx.reports) {
                                paint_badge(ui.painter(), rect.right_top() + vec2(badge_x, 12.0), b, &pal);
                                badge_x -= 16.0;
                            }
                            if let Some(a) = drive_avail(cx.st, e) {
                                paint_avail(ui.painter(), rect.right_top() + vec2(badge_x, 12.0), a, &pal);
                            }
                            let mut shown_name = e.name.clone();
                            let max_chars = ((size + 12.0) / 7.0) as usize;
                            if shown_name.chars().count() > max_chars {
                                shown_name = shown_name.chars().take(max_chars.saturating_sub(1)).collect::<String>() + "...";
                            }
                            let name_rect = Rect::from_min_max(
                                pos2(rect.left() + 2.0, rect.bottom() - 28.0),
                                pos2(rect.right() - 2.0, rect.bottom() - 4.0),
                            );
                            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(("minos-name-rect", e.path.clone())), name_rect));
                            // a child that takes no room in the wrapping row: a scope would, and push the next cell down
                            let mut name_ui = ui.new_child(UiBuilder::new().max_rect(name_rect));
                            let editing = inline_rename(&mut name_ui, &pal, e);
                            if !editing {
                                ui.painter().text(
                                    pos2(rect.center().x, rect.bottom() - 16.0),
                                    Align2::CENTER_CENTER,
                                    shown_name,
                                    FontId::proportional(13.0),
                                    tint.unwrap_or(pal.text),
                                );
                            }
                            let r = r.on_hover_text(&e.name);
                            if want_row == Some(pos) {
                                r.scroll_to_me(None);
                            }
                            record_item(ui.ctx(), cx.pi, rect, &e.path);
                            entry_interact(ui.ctx(), &r, e, pos, &order, shown, selected, anchor, &cx, actions);
                        }
                    });
                });
        }
    }
    // the box (YGG-77): from where the drag on empty space began to the pointer, selecting what it covers as it moves
    {
        let ctx = ui.ctx().clone();
        let box_id = egui::Id::new(("minos-box", cx.pi));
        // a finger draws a box only after a double tap (one finger drags to scroll); the mouse always does
        let finger_ok =
            !ctx.input(|i| i.any_touches()) || ctx.data(|d| d.get_temp::<Taps>(egui::Id::new("minos-taps"))).is_some_and(|t| t.armed);
        if bg.drag_started() && finger_ok {
            if let Some(start) = ctx.input(|i| i.pointer.press_origin()) {
                let base = if ctx.input(|i| i.modifiers.command) { selected.clone() } else { BTreeSet::new() };
                ctx.data_mut(|d| d.insert_temp(box_id, BoxDrag { start, base }));
            }
        }
        if let Some(b) = ctx.data(|d| d.get_temp::<BoxDrag>(box_id)) {
            if bg.dragged() {
                if let Some(now) = ctx.input(|i| i.pointer.interact_pos()) {
                    let area = Rect::from_two_pos(b.start, now);
                    let items: Vec<(Rect, PathBuf)> =
                        ctx.data(|d| d.get_temp(egui::Id::new(("minos-item-rects", cx.pi)))).unwrap_or_default();
                    *selected = box_covers(&items, area, &b.base);
                    *anchor = None;
                    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, box_id));
                    let a = accent_colour();
                    painter.rect_filled(area, 2, Color32::from_rgba_unmultiplied(a.r(), a.g(), a.b(), 40));
                    painter.rect_stroke(area, 2, egui::Stroke::new(1.0, a), egui::StrokeKind::Inside);
                    ctx.request_repaint();
                }
            } else {
                ctx.data_mut(|d| d.remove::<BoxDrag>(box_id));
            }
        }
    }
    if new_sort.key != sort.key || new_sort.descending != sort.descending {
        actions.push(Action::Reload);
        pane.sort = new_sort;
    }
    ui.add_space(4.0);
    // how many items and selected: the window's bottom bar (statusbar.rs)
    ui.horizontal(|ui| {
        if searching {
            ui.label(RichText::new("search results").color(pal.dim));
        }
        let t = &mut pane.tabs[pane.active];
        let idle = t.search.as_ref().map(|s| s.finished()).unwrap_or(true);
        if order.is_empty() && !t.query.trim().is_empty() && idle && t.search_root != Path::new("/") {
            ui.label(RichText::new("Nothing found here.").color(pal.dim));
            if ui.button("Search the whole computer").clicked() {
                let c = ui.ctx().clone();
                t.search_from(&c, hidden, PathBuf::from("/"));
            }
        }
        let _ = focused;
    });
}

/// What the preview pane is showing, and the worker that fills it in.
struct PreviewState {
    /// The selection (or folder) the pane is for.
    key: Option<Vec<PathBuf>>,
    data: Option<Arc<PreviewData>>,
    texture: Option<egui::TextureHandle>,
    loading: bool,
    tx: Sender<(Vec<PathBuf>, PreviewData)>,
    rx: Receiver<(Vec<PathBuf>, PreviewData)>,
    cache: HashMap<(PathBuf, u64), Arc<PreviewData>>,
}

impl PreviewState {
    fn new() -> Self {
        let (tx, rx) = channel();
        Self { key: None, data: None, texture: None, loading: false, tx, rx, cache: HashMap::new() }
    }

    fn set(&mut self, ctx: &egui::Context, d: Arc<PreviewData>) {
        self.texture = match &d.body {
            Body::Image { width, height, rgba } => {
                let img = egui::ColorImage::from_rgba_unmultiplied([*width as usize, *height as usize], rgba);
                Some(ctx.load_texture("minos-preview", img, egui::TextureOptions::LINEAR))
            }
            _ => None,
        };
        self.data = Some(d);
        self.loading = false;
    }

    fn mtime_key(p: &Path) -> u64 {
        std::fs::metadata(p)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }

    /// Start (or satisfy from cache) the preview for `want`.
    fn request(&mut self, ctx: &egui::Context, want: Vec<PathBuf>, drive: Option<PreviewData>) {
        if let Some(d) = drive {
            self.set(ctx, Arc::new(d));
            return;
        }
        if want.len() > 1 {
            self.set(ctx, Arc::new(preview::summarize(&want)));
            return;
        }
        let Some(path) = want.first().cloned() else { return };
        let ck = (path.clone(), Self::mtime_key(&path));
        if let Some(hit) = self.cache.get(&ck).cloned() {
            self.set(ctx, hit);
            return;
        }
        // a Proton Drive file that is not on this computer would be downloaded just to show it: say so instead
        if let Some(root) = places::proton_drive_candidates().into_iter().find(|r| path.starts_with(r)) {
            if let Ok(m) = std::fs::metadata(&path) {
                if m.is_file() && matches!(cloud::availability(&root, &path, m.len()), Some(cloud::Avail::Cloud | cloud::Avail::Partial)) {
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.set(
                        ctx,
                        Arc::new(preview::message(
                            &name,
                            "Only in Proton Drive",
                            "Open it, or right-click > Download to this computer, to see it here",
                        )),
                    );
                    return;
                }
            }
        }
        self.loading = true;
        let (tx, c) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let d = preview::build(&path, 640);
            let _ = tx.send((want, d));
            c.request_repaint();
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok((key, data)) = self.rx.try_recv() {
            if self.key.as_ref() == Some(&key) {
                let d = Arc::new(data);
                if let Some(p) = key.first() {
                    if self.cache.len() > 24 {
                        self.cache.clear();
                    }
                    self.cache.insert((p.clone(), Self::mtime_key(p)), d.clone());
                }
                self.set(ctx, d);
            }
        }
    }
}

fn draw_preview(ui: &mut egui::Ui, pal: &Palette, ps: &PreviewState) {
    ui.set_clip_rect(ui.max_rect());
    // the whole pane scrolls with the wheel, so a long text or the details under a picture are never cut off
    egui::ScrollArea::vertical().id_salt("preview-scroll").auto_shrink([false, false]).show(ui, |ui| {
        draw_preview_body(ui, pal, ps);
    });
}

fn draw_preview_body(ui: &mut egui::Ui, pal: &Palette, ps: &PreviewState) {
    let Some(d) = &ps.data else {
        ui.add_space(30.0);
        ui.vertical_centered(|ui| ui.label(RichText::new("Select a file to preview it").color(pal.dim)));
        return;
    };
    let w = ui.available_width();
    ui.add(egui::Label::new(RichText::new(&d.title).strong().size(15.0).color(pal.text)).truncate());
    ui.add(egui::Label::new(RichText::new(&d.subtitle).size(12.5).color(pal.dim)).truncate());
    if ps.loading {
        ui.label(RichText::new("Loading...").color(pal.dim));
    }
    ui.add_space(8.0);

    // A picture is fitted into the pane; a text excerpt runs as long as it is and the pane scrolls.
    let box_h = 280.0_f32;
    match (&d.body, &ps.texture) {
        (Body::Image { width, height, .. }, Some(tex)) => {
            let scale = (w / *width as f32).min(box_h / *height as f32).min(2.0);
            let size = vec2(*width as f32 * scale, *height as f32 * scale);
            ui.vertical_centered(|ui| {
                ui.add(egui::Image::new((tex.id(), size)).corner_radius(8));
            });
        }
        (Body::Text { text, truncated }, _) => {
            egui::Frame::new().fill(pal.field).corner_radius(8).inner_margin(egui::Margin::same(8)).show(ui, |ui| {
                ui.set_width(w - 16.0);
                let mut shown = text.clone();
                if *truncated {
                    shown.push_str("\n...");
                }
                ui.add(egui::Label::new(RichText::new(shown).monospace().size(11.5).color(pal.text)).wrap_mode(egui::TextWrapMode::Wrap));
            });
        }
        (Body::Lines(lines), _) => {
            for l in lines {
                ui.add(egui::Label::new(RichText::new(l).size(13.0).color(pal.text)).truncate());
            }
        }
        (Body::Message(m), _) => {
            ui.label(RichText::new(m).italics().color(pal.dim));
        }
        (Body::Drive { total, free }, _) => {
            usage_bar(ui, pal, *total, *free, (w - 4.0).max(60.0));
            ui.label(RichText::new(format!("{} free of {}", listing::human_size(*free), listing::human_size(*total))).color(pal.dim));
        }
        _ => {}
    }

    if !d.details.is_empty() {
        ui.add_space(10.0);
        ui.label(RichText::new("Details").strong().size(12.5).color(pal.dim));
        ui.add_space(2.0);
        egui::Grid::new("preview-details").num_columns(2).spacing(vec2(10.0, 4.0)).show(ui, |ui| {
            for (k, v) in &d.details {
                ui.label(RichText::new(k).size(12.5).color(pal.dim));
                ui.add(egui::Label::new(RichText::new(v).size(12.5).color(pal.text)).wrap());
                ui.end_row();
            }
        });
    }
}

#[derive(Clone)]
struct RenameDlg {
    path: PathBuf,
    text: String,
    /// True until the first frame has taken keyboard focus. Requesting focus every frame
    /// stops egui ever reporting the field as having lost focus, so Enter did nothing.
    fresh: bool,
}

struct App {
    panes: [Pane; 2],
    focus: usize,
    st: State,
    st_path: Option<PathBuf>,
    /// A window made from a torn-off tab: it saves favorites and settings, but leaves the main window's saved tabs alone.
    detached: bool,
    last_saved: Instant,
    dirty: bool,
    clipboard: Option<(Vec<PathBuf>, bool)>,
    /// What Minos last put on the system clipboard for its own copy or cut (the files as file:// addresses), to tell its own
    /// list from one another program copied since.
    clip_text: Option<String>,
    rename: Option<RenameDlg>,
    rename_unseen: u8,
    /// Copies and moves, one at a time, the rest waiting their turn (transfers.rs: the panel and the bottom bar's part).
    transfers: transfers::Transfers,
    /// The bottom bar (statusbar.rs).
    bar: statusbar::StatusBar,
    /// Right-click > Scrub location sounds, each running or showing how it ended.
    scrubs: Vec<ScrubJob>,
    scrub_next: u64,
    /// Right-click > Remove hidden info, the same way.
    strips: Vec<StripJob>,
    /// Extract Here / Extract To running on threads of their own (Heimdall's file guard checks each file, which takes a
    /// few seconds): each sends what happened when it is done.
    extracts: Vec<Receiver<String>>,
    status: String,
    drives: Vec<places::Drive>,
    /// Recently closed tabs (pane, folder), newest last, for Ctrl+Shift+T.
    closed_tabs: Vec<(usize, PathBuf)>,
    /// The folder last published for other windows (see `session`).
    published: PathBuf,
    picker: Option<Picker>,
    drives_rx: Receiver<Vec<places::Drive>>,
    disks: Vec<places::Disk>,
    cloud: Option<CloudPlace>,
    links: Vec<Link>,
    reports: BTreeMap<String, Report>,
    verifying: Vec<String>,
    verify_tx: Sender<(String, Result<Report, String>)>,
    /// What a Proton Drive download running in the background has to say (taken into the status line).
    download_status: std::sync::Arc<std::sync::Mutex<Option<String>>>,
    verify_rx: Receiver<(String, Result<Report, String>)>,
    /// Proton Drive sign-in form: email, password, mailbox password, 2FA code.
    proton_form: Option<[String; 4]>,
    proton_busy: bool,
    /// Menu > Settings...: Minos's own settings, while the window is open.
    settings_win: Option<settings_win::SettingsWin>,
    /// Whether Proton Drive was added (cloud::proton_added): read at start, after a sign-in and when Settings changed something.
    proton_added: bool,
    /// Until when the mount is looked for after a sign-in (every 2 s); never longer, so an added Proton Drive that is simply not
    /// mounted costs nothing while Minos is open.
    proton_wait: Option<Instant>,
    dev_tx: Sender<DevEvent>,
    dev_rx: Receiver<DevEvent>,
    /// The watcher's way to wake the window, set on the first frame.
    dev_ctx: Arc<std::sync::Mutex<Option<egui::Context>>>,
    /// Removable drives already seen (opened once when they first appear, never again after an eject).
    seen_disks: BTreeSet<String>,
    format_form: Option<FormatForm>,
    /// A drive job is running (what it is), so another is not started on top of it.
    dev_busy: Option<String>,
    proton_tx: Sender<Result<(), String>>,
    proton_rx: Receiver<Result<(), String>>,
    applied_theme: Option<(bool, Option<[u8; 3]>, u32)>,
    pv: PreviewState,
    dnd: Option<dnd::Dnd>,
    dnd_tried: bool,
    /// Where a drag from another window is hovering (logical pixels).
    ext_hover: Option<(f32, f32)>,
    /// The Properties dialog (YGG-169).
    props: Option<topbar::PropsDlg>,
    /// Cloud Storage's drives from Settings (YGG-170).
    cloud_drives: cloudside::CloudDrives,
}

impl App {
    fn new() -> Self {
        let t0 = Instant::now();
        let lap = |what: &str| {
            if std::env::var_os("MINOS_TIMING").is_some() {
                eprintln!("minos: {:>6.0} ms  {}", t0.elapsed().as_secs_f64() * 1000.0, what)
            }
        };
        let st_path = State::config_path();
        let mut st = st_path.as_deref().map(State::load_from).unwrap_or_default();
        st.show_hidden = yggdrasil_conf::PanelConf::load().show_hidden;
        st.drive_status = yggdrasil_conf::PanelConf::load().drive_status;
        {
            let c = yggdrasil_conf::PanelConf::load();
            st.opacity = if c.minos_transparent { c.minos_opacity } else { 100 };
        }
        st.follow_accent = true;
        let defaults: Vec<PathBuf> = places::favorites().into_iter().map(|p| p.path).collect();
        if st.favorites.is_empty() {
            st.favorites = defaults;
        } else if let Some(f) = places::without_old_home(&st.favorites, dirs::home_dir().as_deref(), &defaults) {
            st.favorites = f;
        }
        lap("state loaded");
        // Where this window starts, from Settings: the first window, or one opened while another is open.
        {
            let conf = yggdrasil_conf::PanelConf::load();
            let others = session::others();
            let (mode, here) =
                if others.is_empty() { (conf.minos_first_window, None) } else { (conf.minos_more_windows, others.first().cloned()) };
            match (mode, here) {
                (yggdrasil_conf::StartIn::Last, _) => {}
                (yggdrasil_conf::StartIn::Here, Some(dir)) if dir.is_dir() || places::is_virtual(&dir) => {
                    st.tabs = vec![SavedTab { path: dir, pinned: false }];
                    st.active = 0;
                    st.right_tabs.clear();
                    st.split = false;
                }
                _ => {
                    st.tabs.clear();
                    st.right_tabs.clear();
                    st.active = 0;
                }
            }
        }
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
        let links = cloud::links_path().map(|p| cloud::load_links(&p)).unwrap_or_default();
        let reports = links.iter().filter_map(|l| cloud::last_report(&l.name).map(|r| (l.name.clone(), r))).collect();
        let (verify_tx, verify_rx) = channel();
        let (proton_tx, proton_rx) = channel();
        lap("links loaded");
        let pane_l = Pane::from_saved(&st.tabs, st.active, home.clone(), st.show_hidden);
        lap("left pane (tabs listed)");
        let pane_r = Pane::from_saved(&st.right_tabs, st.right_active, home, st.show_hidden);
        lap("right pane");
        // Mounted drives can stall for seconds (statvfs on a cold mount), so the sidebar fills in after the window is up.
        let (drives_tx, drives_rx) = channel();
        std::thread::spawn(move || {
            let _ = drives_tx.send(places::drives());
        });
        let drives = Vec::new();
        lap("drives (background)");
        let disks = places::disks();
        lap("disks (lsblk)");
        // Drives plugged in or pulled out while Minos is open: the block devices and mounts are looked at every two seconds.
        let (dev_tx, dev_rx) = channel();
        let dev_ctx: Arc<std::sync::Mutex<Option<egui::Context>>> = Default::default();
        {
            let (tx, c) = (dev_tx.clone(), dev_ctx.clone());
            std::thread::spawn(move || {
                let mut last = devices::signature();
                loop {
                    std::thread::sleep(Duration::from_secs(2));
                    let now = devices::signature();
                    if now != last {
                        last = now;
                        if tx.send(DevEvent::Changed).is_err() {
                            return;
                        }
                        if let Some(c) = c.lock().ok().and_then(|g| g.clone()) {
                            c.request_repaint();
                        }
                    }
                }
            });
        }
        let proton_added = cloud::proton_added();
        let cloud_place = places::proton_drive(proton_added);
        lap("proton drive place");
        Self {
            panes: [pane_l, pane_r],
            focus: 0,
            st,
            st_path,
            detached: false,
            last_saved: Instant::now(),
            dirty: false,
            clipboard: None,
            clip_text: None,
            rename: None,
            rename_unseen: 0,
            transfers: Default::default(),
            bar: Default::default(),
            scrubs: Vec::new(),
            scrub_next: 0,
            strips: Vec::new(),
            extracts: Vec::new(),
            status: String::new(),
            drives,
            closed_tabs: Vec::new(),
            published: PathBuf::new(),
            picker: None,
            drives_rx,
            disks,
            cloud: cloud_place,
            links,
            reports,
            verifying: vec![],
            verify_tx,
            download_status: Default::default(),
            verify_rx,
            proton_form: None,
            proton_busy: false,
            settings_win: None,
            proton_added,
            proton_wait: None,
            dev_tx,
            dev_rx,
            dev_ctx,
            seen_disks: BTreeSet::new(),
            format_form: None,
            dev_busy: None,
            proton_tx,
            proton_rx,
            applied_theme: None,
            pv: PreviewState::new(),
            dnd: None,
            dnd_tried: false,
            ext_hover: None,
            props: None,
            cloud_drives: cloudside::CloudDrives::new(),
        }
    }

    /// Drag and drop with other windows (see dnd.rs): start a drag out when the pointer leaves with files,
    /// and take what is dropped on this window.
    fn pump_dnd(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        let id = egui::Id::new("minos-drop-rows");
        let rows: Vec<(Rect, PathBuf)> = ctx.data_mut(|d| d.get_temp::<Vec<(Rect, PathBuf)>>(id)).unwrap_or_default();
        ctx.data_mut(|d| d.insert_temp(id, Vec::<(Rect, PathBuf)>::new()));
        if !self.dnd_tried {
            self.dnd_tried = true;
            self.dnd = dnd::init(frame, ctx);
            if std::env::var_os("MINOS_DEBUG").is_some() {
                eprintln!("minos: window-to-window drag and drop {}", if self.dnd.is_some() { "ready" } else { "not available" });
            }
        }
        let split = self.st.split;
        let focus = self.focus;
        let mut dropped: Vec<(Vec<PathBuf>, (f32, f32), bool)> = vec![];
        let Some(dnd) = &mut self.dnd else { return };
        while let Ok(ev) = dnd.events.try_recv() {
            match ev {
                dnd::DndEvent::Hover(p) => self.ext_hover = Some(p),
                dnd::DndEvent::Leave => self.ext_hover = None,
                dnd::DndEvent::Drop { paths, at, moved } => {
                    if std::env::var_os("MINOS_DEBUG").is_some() {
                        eprintln!("minos: {} item(s) dropped here ({})", paths.len(), if moved { "move" } else { "copy" });
                    }
                    self.ext_hover = None;
                    dropped.push((paths, at, moved));
                }
                dnd::DndEvent::Finished => dnd.finished(),
            }
        }
        // Files in hand reaching the window's edge: hand them to the compositor, so they can go to another window (another
        // Minos too) or program. While still inside: a drag may only start while this window holds the pointer, and on
        // Hyprland that ends the moment the pointer leaves it (KWin, where this was first built, still allowed it after;
        // 2026-11-08 dragging between Minos windows did nothing). A drop back on this window arrives as a drop like any other.
        if !dnd.dragging_out() && egui::DragAndDrop::has_payload_of_type::<DragPaths>(ctx) && dnd.can_start() {
            const EDGE: f32 = 14.0;
            let at_edge = ctx.input(|i| i.pointer.latest_pos()).map(|p| !ctx.content_rect().shrink(EDGE).contains(p)).unwrap_or(true);
            if at_edge {
                if let Some(p) = egui::DragAndDrop::payload::<DragPaths>(ctx) {
                    if std::env::var_os("MINOS_DEBUG").is_some() {
                        eprintln!("minos: dragging {} item(s) out of the window", p.0.len());
                    }
                    dnd.start_drag(p.0.clone());
                    egui::DragAndDrop::clear_payload(ctx);
                }
            }
        }
        if let Some(at) = self.ext_hover {
            let pt = pos2(at.0, at.1);
            if let Some((rect, _)) = rows.iter().find(|(r, _)| r.contains(pt)) {
                ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("minos-ext-hover"))).rect_stroke(
                    *rect,
                    6,
                    egui::Stroke::new(2.0, accent_colour()),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let width = ctx.content_rect().width();
        for (paths, at, moved) in dropped {
            let pt = pos2(at.0, at.1);
            let pane = if split && at.0 > width / 2.0 { 1 } else { focus };
            let dest = rows
                .iter()
                .find(|(r, _)| r.contains(pt))
                .map(|(_, p)| p.clone())
                .unwrap_or_else(|| self.panes[pane].tabs[self.panes[pane].active].dir());
            if places::is_virtual(&dest) || archive::split(&dest).is_some() {
                self.status = "Drop on a drive or a folder".into();
                continue;
            }
            // dropped back where they came from (a drag can leave this window and return): nothing to do
            let paths: Vec<PathBuf> = paths.into_iter().filter(|p| p.parent() != Some(dest.as_path())).collect();
            if paths.is_empty() {
                continue;
            }
            if paths.iter().any(|p| dest.starts_with(p)) {
                self.status = "A folder cannot go into itself".into();
                continue;
            }
            let mv = moved && same_drive(paths.first(), &dest);
            self.start_transfer(ctx, paths, dest, mv);
        }
    }

    /// Copy or move these into `dest` on a worker thread, merging with what is there and asking about every
    /// file that already exists (see fileops::transfer and the "already exists" dialog).
    fn start_transfer(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>, dest: PathBuf, mv: bool) {
        let n = paths.len();
        let ahead = self.transfers.push(ctx, paths, dest, mv);
        let verb = if mv { "Moving" } else { "Copying" };
        self.status =
            if ahead == 0 { format!("{verb} {n} item(s)...") } else { format!("{verb} {n} item(s) after the transfers before it") };
    }

    /// Right-click > Scrub location sounds: heimdall-scrub writes "name (scrubbed).ext" beside the file (a new name, never over
    /// one) on a thread; its window shows how far it is.
    fn start_scrub(&mut self, ctx: &egui::Context, input: PathBuf) {
        let out = privacy::scrubbed_path(&input, &|p| p.symlink_metadata().is_ok());
        let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let running = match privacy::Running::start(&input, &out) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Could not scrub {name}: {e}");
                return;
            }
        };
        let pid = running.id();
        let frac = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let (tx, rx) = channel();
        let (f, c) = (frac.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = running.finish(&mut |x| {
                f.store((x * 1000.0) as u32, std::sync::atomic::Ordering::Relaxed);
                c.request_repaint();
            });
            let _ = tx.send(r);
            c.request_repaint();
        });
        self.scrub_next += 1;
        self.status = format!("Scrubbing {name}...");
        self.scrubs.push(ScrubJob { id: self.scrub_next, name, out, frac, pid, rx, result: None, stopped: false });
    }

    /// The scrubs' windows: progress and Stop while one runs, then Done (with Show) or Failed, until closed.
    fn poll_scrubs(&mut self, ctx: &egui::Context) {
        let red = palette_for(self.st.dark, self.st.accent()).red;
        let mut close = vec![];
        let mut show: Option<PathBuf> = None;
        let mut status: Option<String> = None;
        for (i, job) in self.scrubs.iter_mut().enumerate() {
            if job.result.is_none() {
                if let Ok(r) = job.rx.try_recv() {
                    if r.is_err() || job.stopped {
                        // a partial copy is not left behind (the name was free when the scrub started, so it is ours)
                        let _ = std::fs::remove_file(&job.out);
                    }
                    let r = if job.stopped { Err("Stopped".to_string()) } else { r };
                    status = Some(match &r {
                        Ok(p) => format!("Scrubbed copy saved: {}", p.file_name().unwrap_or_default().to_string_lossy()),
                        Err(e) => format!("Scrubbing {} failed: {e}", job.name),
                    });
                    job.result = Some(r);
                }
            }
            let mut open = true;
            egui::Window::new(format!("Scrub location sounds: {}", job.name))
                .id(egui::Id::new(("minos-scrub", job.id)))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0 - 120.0 * i as f32))
                .default_width(380.0)
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    match &job.result {
                        None => {
                            let f = job.frac.load(std::sync::atomic::Ordering::Relaxed) as f32 / 1000.0;
                            ui.add(egui::ProgressBar::new(f).text(format!("{:.0}%", f * 100.0)).animate(f == 0.0));
                            ui.add_space(4.0);
                            ui.label(RichText::new("Removing aircraft sounds and mains hum; the original stays as it is").small());
                            if ui.button("Stop").clicked() && !job.stopped {
                                job.stopped = true;
                                // SAFETY: kill(2) on the child we started and have not waited for yet
                                unsafe { libc::kill(job.pid as libc::pid_t, libc::SIGTERM) };
                            }
                        }
                        Some(Ok(p)) => {
                            ui.label(RichText::new("Done").strong());
                            ui.label(format!("Saved as {}", p.file_name().unwrap_or_default().to_string_lossy()));
                            ui.horizontal(|ui| {
                                if ui.button("Show").clicked() {
                                    show = Some(p.clone());
                                    open = false;
                                }
                                if ui.button("Close").clicked() {
                                    open = false;
                                }
                            });
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(if job.stopped { "Stopped" } else { "Failed" }).strong().color(red));
                            if !job.stopped {
                                ui.label(e);
                            }
                            ui.label(RichText::new("No copy was kept; the original is untouched").small());
                            if ui.button("Close").clicked() {
                                open = false;
                            }
                        }
                    }
                });
            if !open {
                close.push(job.id);
            }
        }
        if let Some(s) = status {
            self.status = s;
            self.reload_all();
        }
        self.scrubs.retain(|j| !close.contains(&j.id));
        if let Some(p) = show {
            if let Some(dir) = p.parent().map(Path::to_path_buf) {
                let (pi, hidden) = (self.focus, self.st.show_hidden);
                let sort = self.panes[pi].sort;
                let t = self.panes[pi].cur();
                t.navigate(dir, sort, hidden);
                t.selected.clear();
                t.selected.insert(p);
            }
        }
    }

    /// Right-click > Remove hidden info: heimdall-strip writes "name (clean).ext" beside the file (a new name, never over one) on
    /// a thread; its window shows what it removed.
    fn start_strip(&mut self, ctx: &egui::Context, input: PathBuf) {
        let out = privacy::clean_path(&input, &|p| p.symlink_metadata().is_ok());
        let name = input.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let running = match privacy::Stripping::start(&input, &out) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Could not clean {name}: {e}");
                return;
            }
        };
        let pid = running.id();
        let (tx, rx) = channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(running.finish());
            c.request_repaint();
        });
        // one counter for both kinds of job: their windows' ids stay apart
        self.scrub_next += 1;
        self.status = format!("Removing hidden info from {name}...");
        self.strips.push(StripJob { id: self.scrub_next, name, out, pid, rx, result: None, stopped: false });
    }

    /// Extracts on a thread of its own; the status says what happened when it is done (`poll_extracts`).
    fn start_extract(&mut self, ctx: &egui::Context, jobs: Vec<ExtractJob>) {
        if jobs.is_empty() {
            return;
        }
        let (tx, rx) = channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let msgs: Vec<String> = jobs
                .iter()
                .map(|j| {
                    let r = if j.inner.is_empty() {
                        archive::extract_all(&j.archive, &j.dest)
                    } else {
                        archive::extract_entry(&j.archive, &j.inner, &j.dest)
                    };
                    match r {
                        Ok(()) if j.here => format!("Extracted {}", j.archive.file_name().unwrap_or_default().to_string_lossy()),
                        Ok(()) => format!("Extracted to {}", j.dest.display()),
                        Err(e) => format!("{}: {e}", j.archive.display()),
                    }
                })
                .collect();
            let _ = tx.send(msgs.join("; "));
            c.request_repaint();
        });
        self.extracts.push(rx);
        self.status =
            if archive::fileguard().is_some() { "Extracting (Heimdall checks each file)...".into() } else { "Extracting...".into() };
    }

    fn poll_extracts(&mut self) {
        let mut done = vec![];
        self.extracts.retain(|rx| match rx.try_recv() {
            Ok(m) => {
                done.push(m);
                false
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => true,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        });
        if !done.is_empty() {
            self.status = done.join("; ");
            self.reload_all();
        }
    }

    /// The Remove hidden info windows: Stop while one runs, then what was removed (with Show), nothing found, or Failed.
    fn poll_strips(&mut self, ctx: &egui::Context) {
        let red = palette_for(self.st.dark, self.st.accent()).red;
        let mut close = vec![];
        let mut show: Option<PathBuf> = None;
        let mut status: Option<String> = None;
        let below = self.scrubs.len();
        for (i, job) in self.strips.iter_mut().enumerate() {
            if job.result.is_none() {
                if let Ok(r) = job.rx.try_recv() {
                    if r.is_err() || job.stopped {
                        // a partial copy is not left behind (the name was free when it started, so it is ours)
                        let _ = std::fs::remove_file(&job.out);
                    }
                    let r = if job.stopped { Err("Stopped".to_string()) } else { r };
                    status = Some(match &r {
                        Ok(privacy::Stripped { wrote: Some(p), .. }) => {
                            format!("Clean copy saved: {}", p.file_name().unwrap_or_default().to_string_lossy())
                        }
                        Ok(_) => format!("Nothing hidden was found in {}", job.name),
                        Err(e) => format!("Removing hidden info from {} failed: {e}", job.name),
                    });
                    job.result = Some(r);
                }
            }
            let mut open = true;
            egui::Window::new(format!("Remove hidden info: {}", job.name))
                .id(egui::Id::new(("minos-strip", job.id)))
                .collapsible(false)
                .resizable(false)
                .anchor(Align2::RIGHT_BOTTOM, vec2(-16.0, -16.0 - 120.0 * (below + i) as f32))
                .default_width(380.0)
                .show(ctx, |ui| {
                    ui.set_min_width(360.0);
                    match &job.result {
                        None => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label("Writing a clean copy; the original stays as it is");
                            });
                            if ui.button("Stop").clicked() && !job.stopped {
                                job.stopped = true;
                                // SAFETY: kill(2) on the child we started and have not waited for yet
                                unsafe { libc::kill(job.pid as libc::pid_t, libc::SIGTERM) };
                            }
                        }
                        Some(Ok(privacy::Stripped { wrote: Some(p), removed })) => {
                            ui.label(RichText::new("Done").strong());
                            ui.label(format!("Saved as {}", p.file_name().unwrap_or_default().to_string_lossy()));
                            if !removed.is_empty() {
                                ui.add_space(4.0);
                                ui.label(RichText::new("Removed:").small());
                                for r in removed.iter().take(40) {
                                    ui.label(RichText::new(format!("• {r}")).small());
                                }
                            }
                            ui.horizontal(|ui| {
                                if ui.button("Show").clicked() {
                                    show = Some(p.clone());
                                    open = false;
                                }
                                if ui.button("Close").clicked() {
                                    open = false;
                                }
                            });
                        }
                        Some(Ok(_)) => {
                            ui.label(RichText::new("Nothing hidden was found").strong());
                            ui.label(RichText::new("No copy was made: the file has nothing to remove").small());
                            if ui.button("Close").clicked() {
                                open = false;
                            }
                        }
                        Some(Err(e)) => {
                            ui.label(RichText::new(if job.stopped { "Stopped" } else { "Failed" }).strong().color(red));
                            if !job.stopped {
                                ui.label(e);
                            }
                            ui.label(RichText::new("No copy was kept; the original is untouched").small());
                            if ui.button("Close").clicked() {
                                open = false;
                            }
                        }
                    }
                });
            if !open {
                close.push(job.id);
            }
        }
        if let Some(s) = status {
            self.status = s;
            self.reload_all();
        }
        self.strips.retain(|j| !close.contains(&j.id));
        if let Some(p) = show {
            if let Some(dir) = p.parent().map(Path::to_path_buf) {
                let (pi, hidden) = (self.focus, self.st.show_hidden);
                let sort = self.panes[pi].sort;
                let t = self.panes[pi].cur();
                t.navigate(dir, sort, hidden);
                t.selected.clear();
                t.selected.insert(p);
            }
        }
    }

    /// Take what the transfers report (the "already exists" dialog shows while one waits for an answer); when one has ended,
    /// moved items' tags follow them and the folders are read again.
    fn poll_transfer(&mut self, ctx: &egui::Context) {
        if let Some(transfers::Finished { stats, done, mv, stopped, dest }) = self.transfers.poll(ctx) {
            if mv {
                for (src, new) in &done {
                    if !src.exists() {
                        self.st.rekey(src, new);
                    }
                }
            }
            let mut parts = vec![];
            if stats.copied > 0 {
                parts.push(format!("{} {}", if mv { "moved" } else { "copied" }, stats.copied));
            }
            if stats.replaced > 0 {
                parts.push(format!("replaced {}", stats.replaced));
            }
            if stats.kept_both > 0 {
                parts.push(format!("kept both {}", stats.kept_both));
            }
            if stats.skipped > 0 {
                parts.push(format!("skipped {}", stats.skipped));
            }
            if stats.same > 0 {
                parts.push(format!("{} already there", stats.same));
            }
            if !stats.errors.is_empty() {
                parts.push(format!("{} failed: {}", stats.errors.len(), stats.errors[0]));
            }
            let head = if stopped { "Stopped" } else { "Done" };
            self.status = if parts.is_empty() {
                format!("{head}: nothing to do in {}", dest.display())
            } else {
                format!("{head}: {} ({})", parts.join(", "), dest.display())
            };
            self.dirty = true;
            self.reload_all();
        }
    }

    /// Tabs showing This PC or a disk list drives: shown again when drives change. A tab in a folder of a drive that is gone goes
    /// to This PC.
    fn reload_virtual(&mut self) {
        let hidden = self.st.show_hidden;
        for p in &mut self.panes {
            let sort = p.sort;
            for t in &mut p.tabs {
                let dir = t.dir();
                if places::is_virtual(&dir) {
                    t.reload(sort, hidden);
                } else if dir.starts_with("/run/media") && !dir.exists() {
                    t.navigate(places::this_pc(), sort, hidden);
                }
            }
        }
    }

    /// A removable drive seen for the first time is opened (mounted), as Windows does; one ejected and plugged in again is new
    /// again, since it was gone in between.
    fn open_new_drives(&mut self, ctx: &egui::Context) {
        let present: BTreeSet<String> = self.disks.iter().map(|d| d.name.clone()).collect();
        self.seen_disks.retain(|n| present.contains(n));
        for d in self.disks.iter().filter(|d| d.removable && !d.holds_system()) {
            if !self.seen_disks.insert(d.name.clone()) {
                continue;
            }
            let parts = devices::unmounted_parts(d);
            if parts.is_empty() {
                continue;
            }
            let (tx, c, name) = (self.dev_tx.clone(), ctx.clone(), d.display_name());
            std::thread::spawn(move || {
                let errs: Vec<String> = parts.iter().filter_map(|p| devices::mount(p).err()).collect();
                let msg = if errs.is_empty() { format!("{name} is ready") } else { format!("{name} could not be opened: {}", errs[0]) };
                let _ = tx.send(DevEvent::Done(msg, None));
                c.request_repaint();
            });
        }
    }

    /// Runs a drive job in the background; its result comes back as a status line (and maybe a folder to open).
    fn dev_job(&mut self, ctx: &egui::Context, what: String, job: impl FnOnce() -> DevEvent + Send + 'static) {
        if let Some(b) = &self.dev_busy {
            self.status = format!("Still busy: {b}");
            return;
        }
        self.status = format!("{what}...");
        self.dev_busy = Some(what);
        let (tx, c) = (self.dev_tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(job());
            c.request_repaint();
        });
    }

    /// The Format dialog: file system, name, and a confirmation that everything on the drive is erased.
    fn format_dialog(&mut self, ctx: &egui::Context) {
        let Some(f) = &mut self.format_form else { return };
        let Some(d) = self.disks.iter().find(|d| d.name == f.disk).cloned() else {
            self.format_form = None;
            return;
        };
        let (mut go, mut cancel) = (false, false);
        egui::Window::new(format!("Format {}", d.display_name())).id(egui::Id::new("minos-format")).collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(ctx, |ui| {
            ui.label(format!("{}{} ({})", if d.model.is_empty() { String::new() } else { format!("{} ", d.model) }, listing::human_size(d.size), d.name));
            ui.add_space(6.0);
            ui.label("File system");
            for (i, (name, _, _, _, about)) in devices::FORMATS.iter().enumerate() {
                ui.radio_value(&mut f.fs, i, *name).on_hover_text(*about);
            }
            ui.label(RichText::new(devices::FORMATS[f.fs].4).small());
            ui.add_space(6.0);
            ui.label("Name");
            ui.text_edit_singleline(&mut f.label);
            let shown = devices::clean_label(f.fs, &f.label);
            if shown != f.label {
                ui.label(RichText::new(format!("Will be named: {shown}")).small());
            }
            ui.add_space(6.0);
            ui.label(RichText::new(format!("The whole drive is erased and made one {} partition again, its full size, even if an ISO or other partitions were put on it.", listing::human_size(d.size))).small());
            ui.checkbox(&mut f.sure, format!("Erase everything on {}", d.display_name()));
            ui.horizontal(|ui| {
                if ui.add_enabled(f.sure, egui::Button::new("Format")).clicked() {
                    go = true;
                }
                if ui.button("Cancel").clicked() {
                    cancel = true;
                }
            });
        });
        if cancel {
            self.format_form = None;
        }
        if go {
            let f = self.format_form.take().unwrap();
            let fsname = devices::FORMATS[f.fs].0;
            self.dev_job(ctx, format!("Formatting {} as {fsname}", d.display_name()), move || match devices::format(&d, f.fs, &f.label) {
                Ok(()) => DevEvent::Done(format!("Formatted as {fsname}"), None),
                Err(e) => DevEvent::Done(format!("Could not format: {e}"), None),
            });
        }
    }

    fn reload_all(&mut self) {
        self.drives = places::drives();
        self.disks = places::disks();
        let hidden = self.st.show_hidden;
        for p in &mut self.panes {
            let sort = p.sort;
            for t in &mut p.tabs {
                t.reload(sort, hidden);
            }
        }
    }

    fn sync_state(&mut self) {
        let (l, r) = (self.panes[0].saved(), self.panes[1].saved());
        if l != self.st.tabs
            || r != self.st.right_tabs
            || self.panes[0].active != self.st.active
            || self.panes[1].active != self.st.right_active
        {
            self.st.tabs = l;
            self.st.right_tabs = r;
            self.st.active = self.panes[0].active;
            self.st.right_active = self.panes[1].active;
            self.dirty = true;
        }
    }

    fn save(&mut self) {
        if let Some(p) = &self.st_path {
            let mut st = self.st.clone();
            if self.detached {
                // the tabs saved for next time stay the main window's
                let saved = State::load_from(p);
                (st.tabs, st.active, st.split, st.right_tabs, st.right_active) =
                    (saved.tabs, saved.active, saved.split, saved.right_tabs, saved.right_active);
            }
            if let Err(e) = st.save_to(p) {
                self.status = format!("Could not save settings: {e}");
            }
        }
        self.dirty = false;
        self.last_saved = Instant::now();
    }

    fn selected_in_focus(&self) -> Vec<PathBuf> {
        let p = &self.panes[self.focus];
        p.tabs[p.active].selected.iter().cloned().collect()
    }

    fn apply(&mut self, ctx: &egui::Context, actions: Vec<Action>) {
        let hidden = self.st.show_hidden;
        let group = self.st.group;
        let mut queue: std::collections::VecDeque<Action> = actions.into();
        while let Some(a) = queue.pop_front() {
            match a {
                Action::Focus(p) => self.focus = p,
                Action::TogglePreview => {
                    self.st.preview = !self.st.preview;
                    self.dirty = true;
                }
                Action::Up(p) => {
                    let cur = self.panes[p].tabs[self.panes[p].active].dir();
                    if !places::is_this_pc(&cur) {
                        let target = match cur.parent() {
                            Some(par) if !par.as_os_str().is_empty() => par.to_path_buf(),
                            _ => places::this_pc(), // a drive root: up is "This PC"
                        };
                        self.focus = p;
                        let sort = self.panes[p].sort;
                        self.panes[p].cur().navigate(target, sort, hidden);
                    }
                }
                Action::EditAddress(p) => {
                    let t = self.panes[p].cur();
                    t.editing = true;
                    t.edit_fresh = true;
                    t.address = if places::is_virtual(&t.dir()) { String::new() } else { t.dir().to_string_lossy().into_owned() };
                }
                Action::NewFile(p) => {
                    let dir = self.panes[p].tabs[self.panes[p].active].dir();
                    if places::is_virtual(&dir) || archive::split(&dir).is_some() {
                        self.status = "Open a normal folder first (archives are read-only here)".into();
                        continue;
                    }
                    match fileops::new_file(&dir) {
                        Ok(new) => {
                            self.status = "File created".into();
                            self.reload_all();
                            self.rename =
                                Some(RenameDlg { text: new.file_name().unwrap().to_string_lossy().into_owned(), path: new, fresh: true });
                        }
                        Err(e) => self.status = format!("New file failed: {e}"),
                    }
                }
                Action::Go(p, path) => {
                    self.focus = p;
                    let sort = self.panes[p].sort;
                    let t = self.panes[p].cur();
                    if path.is_dir() || places::is_virtual(&path) || archive::split(&path).is_some() {
                        t.navigate(path, sort, hidden);
                    } else if let Some(part) = path.to_string_lossy().strip_prefix(places::PART_PREFIX) {
                        queue.push_back(Action::Mount(p, part.to_string()));
                    } else {
                        t.error = Some(format!("{}: not a folder", path.display()));
                    }
                }
                Action::OpenTab(p, path) => {
                    let sort = self.panes[p].sort;
                    self.panes[p].tabs.push(TabView::new(path, false, sort, hidden));
                    self.panes[p].active = self.panes[p].tabs.len() - 1;
                }
                Action::CloseTab(p, i) => {
                    let pane = &mut self.panes[p];
                    if pane.tabs.len() > 1 && !pane.tabs[i].tab.pinned {
                        let gone = pane.tabs.remove(i);
                        self.closed_tabs.push((p, gone.dir()));
                        self.closed_tabs.drain(..self.closed_tabs.len().saturating_sub(20));
                        pane.active = pane.active.min(pane.tabs.len() - 1);
                    }
                }
                Action::NewTab(p) => {
                    let path = if yggdrasil_conf::PanelConf::load().minos_new_tab == yggdrasil_conf::StartIn::Home {
                        dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"))
                    } else {
                        self.panes[p].tabs[self.panes[p].active].dir()
                    };
                    queue.push_front(Action::OpenTab(p, path));
                }
                Action::ReopenTab => {
                    if let Some((p, path)) = self.closed_tabs.pop() {
                        let sort = self.panes[p].sort;
                        self.panes[p].tabs.push(TabView::new(path, false, sort, hidden));
                        self.panes[p].active = self.panes[p].tabs.len() - 1;
                        self.focus = p;
                    }
                }
                Action::Back(p) => {
                    let sort = self.panes[p].sort;
                    let t = self.panes[p].cur();
                    if t.tab.history.back().is_some() {
                        t.search = None;
                        t.results = None;
                        t.reload(sort, hidden);
                    }
                }
                Action::Forward(p) => {
                    let sort = self.panes[p].sort;
                    let t = self.panes[p].cur();
                    if t.tab.history.forward().is_some() {
                        t.search = None;
                        t.results = None;
                        t.reload(sort, hidden);
                    }
                }
                Action::FocusOtherPane => {
                    if self.st.split {
                        self.focus = 1 - self.focus;
                    }
                }
                Action::KeyOpen(p) => {
                    let t = self.panes[p].cur();
                    let sel: Vec<PathBuf> = t.selected.iter().cloned().collect();
                    let dirs: Vec<&PathBuf> =
                        sel.iter().filter(|s| s.is_dir() || places::is_virtual(s) || archive::split(s).is_some()).collect();
                    if sel.len() == 1 && !dirs.is_empty() {
                        let sort = self.panes[p].sort;
                        self.panes[p].cur().navigate(sel[0].clone(), sort, hidden);
                    } else {
                        for f in sel {
                            if archive::is_archive_file(&f) && f.is_file() {
                                let sort = self.panes[p].sort;
                                self.panes[p].cur().navigate(f, sort, hidden);
                            } else if f.is_dir() {
                                self.panes[p].tabs.push(TabView::new(f, false, self.panes[p].sort, hidden));
                            } else {
                                queue.push_back(Action::OpenFile(f));
                            }
                        }
                    }
                }
                Action::KeyNav(p, nav, extend) => {
                    self.focus = p;
                    let gallery = self.st.view == ViewMode::Gallery;
                    let cols = if gallery { ctx.data(|d| d.get_temp::<usize>(egui::Id::new("minos-cols"))).unwrap_or(1).max(1) } else { 1 };
                    // in the details list Left goes to the parent folder and Right into the selected folder
                    if !gallery && matches!(nav, Nav::Left) {
                        queue.push_front(Action::Up(p));
                        continue;
                    }
                    if !gallery && matches!(nav, Nav::Right) {
                        queue.push_front(Action::KeyOpen(p));
                        continue;
                    }
                    let t = self.panes[p].cur();
                    let order = t.order(hidden, group);
                    if order.is_empty() {
                        continue;
                    }
                    let shown = t.shown();
                    let last = order.len() - 1;
                    let cur = t
                        .cursor
                        .as_ref()
                        .filter(|c| t.selected.contains(*c))
                        .or_else(|| t.selected.iter().next())
                        .and_then(|c| order.iter().position(|&i| shown[i].path == *c));
                    let page = 10 * cols;
                    let to = match (cur, nav) {
                        (None, Nav::Up | Nav::End | Nav::PageUp | Nav::Left) => last,
                        (None, _) => 0,
                        (Some(c), Nav::Up) => c.saturating_sub(cols),
                        (Some(c), Nav::Down) => (c + cols).min(last),
                        (Some(c), Nav::Left) => c.saturating_sub(1),
                        (Some(c), Nav::Right) => (c + 1).min(last),
                        (Some(c), Nav::PageUp) => c.saturating_sub(page),
                        (Some(c), Nav::PageDown) => (c + page).min(last),
                        (Some(_), Nav::Home) => 0,
                        (Some(_), Nav::End) => last,
                    };
                    let from = t.anchor.filter(|_| extend).or(cur).unwrap_or(to).min(last);
                    t.selected.clear();
                    if extend {
                        for &i in &order[from.min(to)..=from.max(to)] {
                            t.selected.insert(t.shown()[i].path.clone());
                        }
                    } else {
                        t.selected.insert(t.shown()[order[to]].path.clone());
                        t.anchor = Some(to);
                    }
                    t.cursor = Some(t.shown()[order[to]].path.clone());
                    ctx.data_mut(|d| d.insert_temp(egui::Id::new("minos-scroll-row"), (p, to)));
                }
                Action::TypeFind(p, typed) => {
                    let now = ctx.input(|i| i.time);
                    let id = egui::Id::new("minos-typefind");
                    let (mut buf, at) = ctx.data(|d| d.get_temp::<(String, f64)>(id)).unwrap_or_default();
                    if now - at > 1.0 {
                        buf.clear();
                    }
                    buf.push_str(&typed);
                    ctx.data_mut(|d| d.insert_temp(id, (buf.clone(), now)));
                    let t = self.panes[p].cur();
                    let order = t.order(hidden, group);
                    let lower = buf.to_lowercase();
                    // typing the same letter again cycles through the names that start with it
                    let start = t
                        .cursor
                        .as_ref()
                        .and_then(|c| order.iter().position(|&i| t.shown()[i].path == *c))
                        .map(|c| if buf.chars().count() == 1 { c + 1 } else { c })
                        .unwrap_or(0);
                    let hit = (0..order.len())
                        .map(|k| (start + k) % order.len())
                        .find(|&k| t.shown()[order[k]].name.to_lowercase().starts_with(&lower));
                    if let Some(k) = hit {
                        let path = t.shown()[order[k]].path.clone();
                        t.selected.clear();
                        t.selected.insert(path.clone());
                        t.anchor = Some(k);
                        t.cursor = Some(path);
                        ctx.data_mut(|d| d.insert_temp(egui::Id::new("minos-scroll-row"), (p, k)));
                    }
                }
                Action::CloseWindow => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                Action::SelectTab(p, i) => {
                    self.panes[p].active = i;
                    let sort = self.panes[p].sort;
                    self.panes[p].cur().reload(sort, hidden);
                }
                Action::PinTab(p, i) => self.panes[p].tabs[i].tab.pinned ^= true,
                Action::Reload => self.reload_all(),
                Action::Mount(p, part) => self.dev_job(ctx, format!("Opening {part}"), move || match devices::mount(&part) {
                    Ok(at) => DevEvent::Done(format!("Opened at {}", at.display()), Some((p, at))),
                    Err(e) => DevEvent::Done(format!("Could not open {part}: {e}"), None),
                }),
                Action::Eject(name) => {
                    if let Some(d) = self.disks.iter().find(|d| d.name == name).cloned() {
                        let shown = d.display_name();
                        // step out of the drive first, or its own tab keeps it busy
                        let sort_hidden = self.st.show_hidden;
                        for pane in &mut self.panes {
                            let sort = pane.sort;
                            for t in &mut pane.tabs {
                                if d.parts.iter().flat_map(|p| &p.mounts).any(|m| t.dir().starts_with(m)) {
                                    t.navigate(places::this_pc(), sort, sort_hidden);
                                }
                            }
                        }
                        self.dev_job(ctx, format!("Ejecting {shown}"), move || match devices::eject(&d) {
                            Ok(()) => DevEvent::Done(format!("{shown} can be removed safely"), None),
                            Err(e) => DevEvent::Done(format!("Could not eject {shown}: {e}"), None),
                        });
                    }
                }
                Action::FormatAsk(name) => {
                    let label = self
                        .disks
                        .iter()
                        .find(|d| d.name == name)
                        .map(|d| d.display_name())
                        .filter(|l| l != "USB Drive")
                        .unwrap_or_else(|| "USB".into());
                    self.format_form = Some(FormatForm { disk: name, fs: 0, label, sure: false });
                }
                Action::Launch(program, package) => match devices::launch(&program, &package) {
                    Ok(()) => self.status = format!("Started {program}"),
                    Err(pkg) if pkg == package => {
                        self.status = format!("{program} is not installed: opening a terminal to install {package}");
                        queue.push_back(Action::Install(package));
                    }
                    Err(e) => self.status = e,
                },
                Action::Properties(p) => {
                    let t = &self.panes[p].tabs[self.panes[p].active];
                    let dir = t.dir();
                    if places::is_virtual(&dir) || archive::split(&dir).is_some() {
                        continue;
                    }
                    let paths = if t.selected.is_empty() { vec![dir] } else { t.selected.iter().cloned().collect() };
                    self.props = Some(topbar::PropsDlg::open(ctx, paths));
                }
                Action::SetGroup(g) => {
                    self.st.group = g;
                    self.dirty = true;
                }
                Action::ToggleColumn(c) => {
                    self.st.columns.toggle(c);
                    self.dirty = true;
                }
                Action::ToggleSidebar => {
                    self.st.sidebar_hidden = !self.st.sidebar_hidden;
                    self.dirty = true;
                }
                Action::CloudMount(name, open) => self.cloud_drives.mount(ctx, &name, true, open),
                Action::CloudUnmount(name) => {
                    // tabs in the drive step out first (to home), or they would show an empty folder
                    if let Some(at) = self.cloud_drives.list.iter().find(|d| d.name == name).map(|d| d.path().to_path_buf()) {
                        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/"));
                        for pane in &mut self.panes {
                            let sort = pane.sort;
                            for t in &mut pane.tabs {
                                if t.dir().starts_with(&at) {
                                    t.navigate(home.clone(), sort, hidden);
                                }
                            }
                        }
                    }
                    self.cloud_drives.mount(ctx, &name, false, false);
                }
                Action::ManageCloud => {
                    if let Err(e) = cloudside::open_settings() {
                        self.status = e;
                    }
                }
                Action::OpenSettings => self.settings_win = Some(settings_win::SettingsWin::open()),
                Action::Install(pkg) => {
                    if let Err(e) = run::install_in_terminal(&pkg) {
                        self.status = format!("Could not install {pkg}: {e}");
                    }
                }
                Action::OpenFile(p) if self.picker.is_some() => {
                    let pk = self.picker.as_mut().unwrap();
                    if pk.save {
                        pk.name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    } else if !pk.dir {
                        finish_pick(&[p]);
                    }
                }
                Action::Privacy(privacy::Entry::ScrubSounds, p) => self.start_scrub(ctx, p),
                Action::Privacy(privacy::Entry::RemoveHidden, p) => self.start_strip(ctx, p),
                Action::RunInTerminal(p) => {
                    if let Err(e) = run::run_in_terminal(&p) {
                        self.status =
                            format!("Could not run {}: {e}", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
                    }
                }
                Action::OpenFile(p) => match archive::split(&p) {
                    None => open_with_system(&p),
                    Some((arc, inner)) => {
                        // on disk, not in /tmp (memory here): what is opened may be large
                        let base = yggdrasil_conf::dirs::cache().join("minos-open");
                        archive::clear_stale_open_dirs(&base);
                        let tmp = base.join(std::process::id().to_string());
                        match archive::open_inner(&arc, &inner, &tmp) {
                            Ok(file) => open_with_system(&file),
                            Err(e) => self.status = format!("Could not open {inner}: {e}"),
                        }
                    }
                },
                Action::ExtractHere(archives) => {
                    let jobs = archives
                        .iter()
                        .map(|a| ExtractJob {
                            archive: a.clone(),
                            dest: a.parent().map(Path::to_path_buf).unwrap_or_default(),
                            inner: String::new(),
                            here: true,
                        })
                        .collect();
                    self.start_extract(ctx, jobs);
                }
                Action::ExtractTo(paths) => {
                    let mut jobs = vec![];
                    for p in &paths {
                        let (arc, inner) = match archive::split(p) {
                            Some((a, i)) => (a, i),
                            None => (p.clone(), String::new()),
                        };
                        let stem = arc.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "extracted".into());
                        let parent = arc.parent().map(Path::to_path_buf).unwrap_or_default();
                        // An archive that already has ONE top-level folder is extracted straight into this
                        // folder (no `name/name` doubling); anything else gets a folder named after it.
                        let single_top = inner.is_empty()
                            && archive::list_cached(&arc).map(|l| archive::top_level_names(&l).len() == 1).unwrap_or(false);
                        let dest = if single_top { parent.clone() } else { fileops::unique_path(&parent, &stem) };
                        jobs.push(ExtractJob { archive: arc, dest, inner, here: false });
                    }
                    self.start_extract(ctx, jobs);
                }
                Action::Compress(paths, format) => {
                    let Some(first) = paths.first() else { continue };
                    let parent = first.parent().map(Path::to_path_buf).unwrap_or_default();
                    let base = if paths.len() == 1 {
                        first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
                    } else {
                        "Archive".into()
                    };
                    let dest = fileops::unique_path(&parent, &format!("{base}.{}", format.ext()));
                    self.status = match archive::create(&dest, &paths, format) {
                        Ok(()) => format!("Created {}", dest.file_name().unwrap_or_default().to_string_lossy()),
                        Err(e) => format!("Compress failed: {e}"),
                    };
                    self.reload_all();
                }
                Action::Cut(v) if v.iter().any(|p| archive::inside(p)) => {
                    self.status = "Items inside an archive cannot be cut; copy them out instead".into();
                }
                Action::Cut(v) => {
                    self.status = format!("{} item(s) cut", v.len());
                    let text = uri_list(&v);
                    ctx.copy_text(text.clone());
                    self.clip_text = Some(text);
                    self.clipboard = Some((v, true));
                }
                Action::Copy(v) => {
                    self.status = format!("{} item(s) copied", v.len());
                    let text = uri_list(&v);
                    ctx.copy_text(text.clone());
                    self.clip_text = Some(text);
                    self.clipboard = Some((v, false));
                }
                Action::Paste(p) => {
                    let Some((paths, cut)) = self.clipboard.clone() else { continue };
                    let dest = self.panes[p].tabs[self.panes[p].active].dir();
                    if places::is_virtual(&dest) {
                        self.status = "Open a drive or folder to paste into".into();
                        continue;
                    }
                    if archive::split(&dest).is_some() {
                        self.status = "Archives are read-only here: extract first, then add files".into();
                        continue;
                    }
                    let mut errs = vec![];
                    let mut files = vec![];
                    for src in &paths {
                        if let Some((arc, inner)) = archive::split(src).filter(|(_, inner)| !inner.is_empty()) {
                            // Copying out of an archive extracts (an archive file itself is copied as a file). (Cut is blocked above.)
                            let is_dir = archive::list_cached(&arc)
                                .ok()
                                .and_then(|l| l.into_iter().find(|e| e.path == inner))
                                .map(|e| e.is_dir)
                                .unwrap_or(false);
                            let r = if is_dir {
                                archive::extract_entry(&arc, &inner, &dest)
                            } else {
                                archive::extract_file_flat(&arc, &inner, &dest).map(|_| ())
                            };
                            if let Err(e) = r {
                                errs.push(format!("{inner}: {e}"));
                            }
                        } else {
                            files.push(src.clone());
                        }
                    }
                    if cut {
                        self.clipboard = None;
                    }
                    if !errs.is_empty() {
                        self.status = errs.join("; ");
                    }
                    if !files.is_empty() {
                        self.start_transfer(ctx, files, dest, cut);
                    } else {
                        self.reload_all();
                    }
                }
                Action::Move(paths, dest) => {
                    if places::is_virtual(&dest) {
                        self.status = "Drop on a drive or a folder".into();
                        continue;
                    }
                    if archive::split(&dest).is_some() || paths.iter().any(|p| archive::inside(p)) {
                        self.status = "Archives are read-only here: use Extract, or copy and paste".into();
                        continue;
                    }
                    // Like Windows: the same drive moves, another drive copies; Shift forces a move, Ctrl a copy.
                    let m = ctx.input(|i| i.modifiers);
                    let mv = if m.shift {
                        true
                    } else if m.command {
                        false
                    } else {
                        same_drive(paths.first(), &dest)
                    };
                    self.start_transfer(ctx, paths, dest, mv);
                }
                Action::Rename(path) if archive::inside(&path) => {
                    self.status = "Items inside an archive cannot be renamed".into();
                }
                Action::Rename(path) => {
                    let text = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    self.rename = Some(RenameDlg { path, text, fresh: true });
                }
                Action::Trash(paths) if paths.iter().any(|p| archive::inside(p)) => {
                    self.status = "Items inside an archive cannot be deleted from here".into();
                }
                Action::Trash(paths) => {
                    let mut errs = vec![];
                    for p in &paths {
                        if let Err(e) = fileops::trash(p) {
                            errs.push(format!("{}: {e}", p.display()));
                        }
                    }
                    self.status = if errs.is_empty() { format!("Moved {} item(s) to trash", paths.len()) } else { errs.join("; ") };
                    self.reload_all();
                }
                Action::NewFolder(p) => {
                    let dir = self.panes[p].tabs[self.panes[p].active].dir();
                    if places::is_virtual(&dir) || archive::split(&dir).is_some() {
                        self.status = "Open a normal folder first (archives are read-only here)".into();
                        continue;
                    }
                    match fileops::new_folder(&dir) {
                        Ok(new) => {
                            self.status = "Folder created".into();
                            self.reload_all();
                            self.rename =
                                Some(RenameDlg { text: new.file_name().unwrap().to_string_lossy().into_owned(), path: new, fresh: true });
                        }
                        Err(e) => self.status = format!("New folder failed: {e}"),
                    }
                }
                Action::ToggleTag(p, t) => {
                    self.st.toggle_tag(&p, &t);
                    self.dirty = true;
                }
                Action::SetColor(p, c) => {
                    match c {
                        Some(c) => self.st.folder_colors.insert(p, c),
                        None => self.st.folder_colors.remove(&p),
                    };
                    self.dirty = true;
                }
                Action::ToggleFav(p) => {
                    self.st.toggle_favorite(&p);
                    self.dirty = true;
                }
                Action::MoveFav(p, d) => {
                    if let Some(i) = self.st.favorites.iter().position(|f| *f == p) {
                        let j = (i as i32 + d).clamp(0, self.st.favorites.len() as i32 - 1) as usize;
                        self.st.favorites.swap(i, j);
                        self.dirty = true;
                    }
                }
                Action::ReorderFav(from, to) => {
                    if self.st.move_favorite(from, to) {
                        self.dirty = true;
                    }
                }
                Action::MoveTab(p, from, to) => {
                    let pane = &mut self.panes[p];
                    if from < pane.tabs.len() && to <= pane.tabs.len() && to != from && to != from + 1 {
                        let active = pane.tabs[pane.active].dir();
                        let t = pane.tabs.remove(from);
                        let to = if to > from { to - 1 } else { to };
                        pane.tabs.insert(to, t);
                        pane.active = pane.tabs.iter().position(|t| t.dir() == active).unwrap_or(to);
                        self.dirty = true;
                    }
                }
                Action::DetachTab(p, i) => {
                    // a new window with only that folder; the tab leaves this one (a lone tab stays: nothing to tear off)
                    let pane = &self.panes[p];
                    if pane.tabs.len() > 1 && i < pane.tabs.len() {
                        let dir = pane.tabs[i].dir();
                        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("minos"));
                        match std::process::Command::new(exe).arg("--window").arg(&dir).spawn() {
                            Ok(_) => {
                                let pinned = pane.tabs[i].tab.pinned;
                                if pinned {
                                    self.panes[p].tabs[i].tab.pinned = false;
                                }
                                queue.push_front(Action::CloseTab(p, i));
                            }
                            Err(e) => self.status = format!("Could not open a new window: {e}"),
                        }
                    }
                }
                Action::ResetFavs => {
                    self.st.favorites = places::favorites().into_iter().map(|p| p.path).collect();
                    self.dirty = true;
                }
                Action::CopyPath(p) => ctx.copy_text(p.to_string_lossy().into_owned()),
                Action::RetrySync(p) => {
                    if let Some(root) = places::proton_drive_candidates().into_iter().find(|r| p.starts_with(r)) {
                        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                        self.status = match cloud::retry_sync(&root, &p) {
                            Ok(1) => format!("Uploading {name} to Proton Drive again..."),
                            Ok(n) => format!("Uploading {n} files in {name} to Proton Drive again..."),
                            Err(e) => format!("Retry sync: {e}"),
                        };
                    }
                }
                Action::Download(paths) => {
                    let (shared, c) = (self.download_status.clone(), ctx.clone());
                    *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some("Downloading from Proton Drive...".into());
                    std::thread::spawn(move || {
                        let say = |t: String| {
                            *shared.lock().unwrap_or_else(|e| e.into_inner()) = Some(t);
                            c.request_repaint();
                        };
                        let failed = cloud::download(&paths, |done, total| {
                            if done < total {
                                say(format!("Downloading from Proton Drive: {done} of {total} files..."));
                            }
                        });
                        say(if failed == 0 {
                            "Downloaded: it is on this computer now".into()
                        } else {
                            format!("Downloaded, but {failed} files could not be read (see Proton Drive's connection)")
                        });
                    });
                }
                Action::Verify(name) => {
                    if let Some(link) = self.links.iter().find(|l| l.name == name).cloned() {
                        if !self.verifying.contains(&name) {
                            self.verifying.push(name.clone());
                            let (tx, c) = (self.verify_tx.clone(), ctx.clone());
                            std::thread::spawn(move || {
                                let r = cloud::verify_link(&link);
                                let _ = tx.send((link.name, r));
                                c.request_repaint();
                            });
                        }
                    }
                }
                Action::ToggleSplit => {
                    self.st.split = !self.st.split;
                    self.focus = 0;
                    self.dirty = true;
                }
                Action::SetView(v) => {
                    self.st.view = v;
                    self.dirty = true;
                }
                Action::ProtonConnect => {
                    if cloud::proton_configured() {
                        // signed in already: the mount is just not running
                        self.proton_busy = true;
                        let (tx, c) = (self.proton_tx.clone(), ctx.clone());
                        std::thread::spawn(move || {
                            // the saved sign-in may have lapsed (a two-factor code or password no longer accepted): then ask again
                            // any failure here (the service refused to start, or never mounted) means: stop it, and ask for the sign-in
                            let r = cloud::start_proton_mount()
                                .and_then(|_| cloud::wait_mounted(15).then_some(()).ok_or_else(String::new))
                                .map_err(|_| {
                                    cloud::stop_proton_mount();
                                    format!(
                                        "{}Proton Drive did not start with the saved sign-in. Please sign in again.",
                                        cloud::SIGN_IN_AGAIN
                                    )
                                });
                            let _ = tx.send(r);
                            c.request_repaint();
                        });
                    } else {
                        self.proton_form = Some(Default::default());
                    }
                }
            }
        }
    }
}

fn section_heading(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(8.0);
    ui.label(RichText::new(text).strong().size(12.5).color(pal.dim));
    ui.add_space(2.0);
}

/// One row of the sidebar: icon, label, and optionally a usage bar underneath (drives).
#[allow(clippy::too_many_arguments)]
fn side_row(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: Icon,
    icon_color: Color32,
    sys: &[&str],
    label: &str,
    selected: bool,
    bar: Option<(f32, Color32)>,
) -> egui::Response {
    side_row_with(ui, pal, icon, icon_color, sys, None, label, selected, bar, false)
}

/// A sidebar row. `special` draws a well-known folder's own picture; `drag` lets the row be dragged (favorites).
#[allow(clippy::too_many_arguments)]
fn side_row_with(
    ui: &mut egui::Ui,
    pal: &Palette,
    icon: Icon,
    icon_color: Color32,
    sys: &[&str],
    special: Option<places::Special>,
    label: &str,
    selected: bool,
    bar: Option<(f32, Color32)>,
    drag: bool,
) -> egui::Response {
    let h = if bar.is_some() { 40.0 } else { 28.0 };
    let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), h), if drag { Sense::click_and_drag() } else { Sense::click() });
    let row = Rect::from_min_size(rect.min, vec2(rect.width(), 28.0));
    if selected {
        ui.painter().rect_filled(row, CornerRadius::same(8), pal.select);
    } else if r.hovered() {
        ui.painter().rect_filled(row, CornerRadius::same(8), pal.hover);
    }
    let icon_rect = Rect::from_min_size(row.min + vec2(8.0, 4.0), vec2(20.0, 20.0));
    if let Some(sp) = special {
        icons::paint_special(ui.painter(), icon_rect, sp);
    } else if sys.is_empty() || !sysicon::paint(ui.painter(), icon_rect, sys) {
        icons::paint(ui.painter(), Rect::from_min_size(row.min + vec2(8.0, 5.0), vec2(18.0, 18.0)), icon, icon_color);
    }
    ui.painter().with_clip_rect(row.shrink2(vec2(4.0, 0.0))).text(
        pos2(row.left() + 34.0, row.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.5),
        pal.text,
    );
    if let Some((frac, color)) = bar {
        let track = Rect::from_min_size(pos2(row.left() + 8.0, row.bottom() + 1.0), vec2(rect.width() - 16.0, 5.0));
        ui.painter().rect_filled(track, CornerRadius::same(3), pal.border.gamma_multiply(0.7));
        let mut fill = track;
        fill.set_width(track.width() * frac);
        ui.painter().rect_filled(fill, CornerRadius::same(3), color);
    }
    r
}

impl App {
    #[allow(clippy::too_many_arguments)]
    /// `fav`: the row is favorite number `fav` (drawn with its folder's own picture, draggable to a new place).
    fn nav_row(
        &self,
        ui: &mut egui::Ui,
        pal: &Palette,
        icon: Icon,
        icon_color: Color32,
        label: &str,
        path: &Path,
        bar: Option<(f32, Color32)>,
        fav: Option<usize>,
        actions: &mut Vec<Action>,
    ) {
        let selected = {
            let p = &self.panes[self.focus];
            p.tabs[p.active].dir() == path
        };
        // A folder that has a colour tag keeps its drawn, tinted icon; the rest use the system theme.
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let sys: Vec<&str> = match icon {
            Icon::Cloud => vec!["proton-drive", "folder-cloud", "folder"],
            Icon::Folder if icon_color == pal.folder => sysicon::folder_names(&name),
            _ => vec![],
        };
        // a well-known folder (Desktop, Documents, ...) has its own picture, unless it was given a colour
        let special = fav.and_then(|_| places::special(path)).filter(|_| icon_color == pal.folder);
        let r = side_row_with(ui, pal, icon, icon_color, &sys, special, label, selected, bar, fav.is_some());
        if r.clicked() {
            actions.push(Action::Go(self.focus, path.to_path_buf()));
        }
        // dragging a favorite to a new place: a line shows the gap it would go into
        if let Some(i) = fav {
            r.dnd_set_drag_payload(DragFav(i));
            let after = |r: &egui::Response| ui.input(|inp| inp.pointer.hover_pos()).is_some_and(|p| p.y > r.rect.center().y);
            if r.dnd_hover_payload::<DragFav>().is_some() {
                let y = if after(&r) { r.rect.bottom() } else { r.rect.top() };
                ui.painter()
                    .line_segment([pos2(r.rect.left() + 6.0, y), pos2(r.rect.right() - 6.0, y)], egui::Stroke::new(2.0, pal.accent));
            }
            if let Some(d) = r.dnd_release_payload::<DragFav>() {
                actions.push(Action::ReorderFav(d.0, if after(&r) { i + 1 } else { i }));
            }
        }
        if r.middle_clicked() {
            actions.push(Action::OpenTab(self.focus, path.to_path_buf()));
        }
        if !places::is_virtual(path) {
            if let Some(d) = r.dnd_release_payload::<DragPaths>() {
                actions.push(Action::Move(d.0.clone(), path.to_path_buf()));
            }
        }
        if fav.is_some() {
            r.context_menu(|ui| {
                if ui.button("Move up").clicked() {
                    actions.push(Action::MoveFav(path.to_path_buf(), -1));
                    ui.close();
                }
                if ui.button("Move down").clicked() {
                    actions.push(Action::MoveFav(path.to_path_buf(), 1));
                    ui.close();
                }
                if ui.button("Remove").clicked() {
                    actions.push(Action::ToggleFav(path.to_path_buf()));
                    ui.close();
                }
            });
        }
    }

    /// A drive under Storage, as Windows shows it: its name, a bar of how full it is, and "7.9 GB free of 106 GB". A removable
    /// drive has an eject button at the right and a right-click menu (open, eject, format, partition, make bootable).
    fn storage_row(&self, ui: &mut egui::Ui, pal: &Palette, d: &places::Disk, actions: &mut Vec<Action>) {
        let target = d.open_path();
        let selected = {
            let p = &self.panes[self.focus];
            let dir = p.tabs[p.active].dir();
            dir == target || dir == places::disk_path(&d.name)
        };
        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 58.0), Sense::click());
        let busy = self.dev_busy.is_some();
        let eject_rect = Rect::from_center_size(pos2(rect.right() - 16.0, rect.top() + 16.0), vec2(24.0, 22.0));
        let over_eject = d.removable && r.hover_pos().is_some_and(|p| eject_rect.contains(p));
        if selected {
            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
        } else if r.hovered() {
            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
        }
        icons::paint(ui.painter(), Rect::from_min_size(rect.min + vec2(8.0, 7.0), vec2(18.0, 18.0)), Icon::Drive, pal.drive);
        let name_clip = Rect::from_min_max(rect.min, pos2(rect.right() - if d.removable { 32.0 } else { 6.0 }, rect.bottom()));
        ui.painter().with_clip_rect(name_clip).text(
            pos2(rect.left() + 34.0, rect.top() + 16.0),
            Align2::LEFT_CENTER,
            d.display_name(),
            FontId::proportional(13.5),
            pal.text,
        );
        let usage = d.mounted_usage();
        let frac = usage.map(|(t, f)| listing::used_fraction(t, f)).unwrap_or(0.0);
        let track = Rect::from_min_size(pos2(rect.left() + 8.0, rect.top() + 30.0), vec2(rect.width() - 16.0, 6.0));
        ui.painter().rect_filled(track, CornerRadius::same(3), pal.border.gamma_multiply(0.7));
        let mut fill = track;
        fill.set_width(track.width() * frac);
        ui.painter().rect_filled(fill, CornerRadius::same(3), theme::usage_color(pal, frac));
        let line = match usage {
            Some((t, f)) => format!("{} free of {}", listing::human_size(f), listing::human_size(t)),
            None if d.removable => format!("{}, click to open", listing::human_size(d.size)),
            None => listing::human_size(d.size),
        };
        ui.painter().with_clip_rect(rect).text(
            pos2(rect.left() + 8.0, rect.top() + 45.0),
            Align2::LEFT_CENTER,
            line,
            FontId::proportional(11.5),
            pal.dim,
        );
        if d.removable {
            if over_eject {
                ui.painter().rect_filled(eject_rect, CornerRadius::same(6), pal.hover.gamma_multiply(2.0));
            }
            // the eject symbol: a triangle over a bar
            let c = eject_rect.center();
            let col = if over_eject { pal.text } else { pal.dim };
            ui.painter().add(egui::Shape::convex_polygon(
                vec![c + vec2(0.0, -7.0), c + vec2(6.0, 0.5), c + vec2(-6.0, 0.5)],
                col,
                egui::Stroke::NONE,
            ));
            ui.painter().rect_filled(Rect::from_min_max(c + vec2(-6.0, 3.0), c + vec2(6.0, 5.5)), CornerRadius::same(1), col);
        }
        let r = r.on_hover_text(
            format!(
                "{}\n{}",
                if d.model.is_empty() { d.name.clone() } else { format!("{} ({})", d.model, d.name) },
                if d.removable { "Right-click for eject, format, partition and bootable drive" } else { "" }
            )
            .trim()
            .to_string(),
        );
        if r.clicked() {
            if over_eject {
                actions.push(Action::Eject(d.name.clone()));
            } else if d.removable && usage.is_none() {
                if let Some(part) = devices::unmounted_parts(d).into_iter().next() {
                    actions.push(Action::Mount(self.focus, part));
                } else {
                    actions.push(Action::Go(self.focus, places::disk_path(&d.name)));
                }
            } else {
                actions.push(Action::Go(self.focus, target.clone()));
            }
        }
        if r.middle_clicked() {
            actions.push(Action::OpenTab(self.focus, target.clone()));
        }
        if !d.removable {
            return;
        }
        r.context_menu(|ui| {
            if ui.button("Open").clicked() {
                if usage.is_none() {
                    if let Some(part) = devices::unmounted_parts(d).into_iter().next() {
                        actions.push(Action::Mount(self.focus, part));
                    }
                } else {
                    actions.push(Action::Go(self.focus, target.clone()));
                }
                ui.close();
            }
            if ui.add_enabled(!busy, egui::Button::new("Eject")).clicked() {
                actions.push(Action::Eject(d.name.clone()));
                ui.close();
            }
            ui.separator();
            if ui.add_enabled(!busy, egui::Button::new("Format...")).clicked() {
                actions.push(Action::FormatAsk(d.name.clone()));
                ui.close();
            }
            let (prog, pkg) = devices::PARTITIONER;
            if ui
                .button(if devices::installed(prog) {
                    "Partition... (GParted)".to_string()
                } else {
                    "Partition... (installs GParted)".to_string()
                })
                .clicked()
            {
                actions.push(Action::Launch(prog.into(), pkg.into()));
                ui.close();
            }
            ui.menu_button("Make bootable", |ui| {
                for (title, prog, pkg, about) in devices::BOOTABLE {
                    let shown = if devices::installed(prog) { title.to_string() } else { format!("{title} (installs it)") };
                    if ui.button(shown).on_hover_text(about).clicked() {
                        actions.push(Action::Launch(prog.into(), pkg.into()));
                        ui.close();
                    }
                }
            });
        });
    }

    fn sidebar(&self, ui: &mut egui::Ui, pal: &Palette, actions: &mut Vec<Action>) {
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            section_heading(ui, pal, "Places");
            self.nav_row(ui, pal, Icon::Computer, pal.drive, &places::pc_name(), &places::this_pc(), None, None, actions);
            if let Some(home) = dirs::home_dir() {
                self.nav_row(ui, pal, Icon::Home, pal.drive, "My User Home", &home, None, None, actions);
            }

            section_heading(ui, pal, "Favorites");
            for (i, path) in self.st.favorites.iter().enumerate() {
                let tint = self.st.color_for(path).map(rgb).unwrap_or(pal.folder);
                self.nav_row(ui, pal, Icon::Folder, tint, &places::label_for(path), path, None, Some(i), actions);
            }
            // the favorite being dragged follows the pointer
            if let (Some(d), Some(at)) = (egui::DragAndDrop::payload::<DragFav>(ui.ctx()), ui.ctx().pointer_interact_pos()) {
                if let Some(path) = self.st.favorites.get(d.0) {
                    let layer = egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("minos-fav-drag"));
                    let painter = ui.ctx().layer_painter(layer);
                    let text = places::label_for(path);
                    let galley = painter.layout_no_wrap(text, FontId::proportional(13.5), pal.text);
                    let r = Rect::from_min_size(at + vec2(12.0, -10.0), galley.size() + vec2(16.0, 8.0));
                    painter.rect_filled(r, CornerRadius::same(6), pal.tab_active);
                    painter.galley(r.min + vec2(8.0, 4.0), galley, pal.text);
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
            }
            if ui.small_button("Reset favorites").clicked() {
                actions.push(Action::ResetFavs);
            }

            section_heading(ui, pal, "Storage");
            if self.disks.is_empty() {
                for d in &self.drives {
                    let frac = listing::used_fraction(d.total, d.free);
                    self.nav_row(
                        ui,
                        pal,
                        Icon::Drive,
                        pal.drive,
                        &d.label,
                        &d.path,
                        Some((frac, theme::usage_color(pal, frac))),
                        None,
                        actions,
                    );
                }
            }
            for d in self.disks.iter().filter(|d| !d.removable) {
                self.storage_row(ui, pal, d, actions);
            }
            if self.disks.iter().any(|d| d.removable) {
                section_heading(ui, pal, "Removable");
                for d in self.disks.iter().filter(|d| d.removable) {
                    self.storage_row(ui, pal, d, actions);
                }
            }

            // Cloud Storage only with something added under it (no heading, no "Manage" link before that: adding starts in Settings)
            let any_cloud = self.cloud.is_some() || !self.links.is_empty() || !self.cloud_drives.list.is_empty();
            if any_cloud {
                section_heading(ui, pal, "Cloud Storage");
            }
            if let Some(c) = &self.cloud {
                match c.state {
                    CloudState::Ready => self.nav_row(ui, pal, Icon::Cloud, pal.dim, &c.label, &c.path, None, None, actions),
                    CloudState::NotMounted => {
                        let (rect, r) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
                        if r.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                        }
                        icons::paint(
                            ui.painter(),
                            Rect::from_min_size(rect.min + vec2(8.0, 5.0), vec2(18.0, 18.0)),
                            Icon::Cloud,
                            pal.dim.gamma_multiply(0.5),
                        );
                        let label = if self.proton_busy {
                            format!("{} (connecting...)", c.label)
                        } else {
                            format!("{} (click to connect)", c.label)
                        };
                        ui.painter().text(
                            pos2(rect.left() + 34.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            label,
                            FontId::proportional(13.5),
                            pal.dim.gamma_multiply(0.7),
                        );
                        if r.clicked() && !self.proton_busy {
                            actions.push(Action::ProtonConnect);
                        }
                        r.on_hover_text(format!("Not connected. Click to sign in; it will appear at {}", c.path.display()));
                    }
                }
            }
            self.cloud_drive_rows(ui, pal, actions);
            for l in &self.links {
                let badge = if self.verifying.contains(&l.name) {
                    None
                } else {
                    cloud::badge_for(&l.local, std::slice::from_ref(l), &self.reports)
                };
                let tip = if self.verifying.contains(&l.name) {
                    "Verifying...".to_string()
                } else {
                    badge.map(badge_tip).unwrap_or("Not verified yet").to_string()
                };
                let selected = {
                    let p = &self.panes[self.focus];
                    p.tabs[p.active].dir() == l.local
                };
                let r = side_row(ui, pal, Icon::Folder, pal.folder, &["folder-cloud", "folder"], &l.name, selected, None);
                if let Some(b) = badge {
                    paint_badge(ui.painter(), pos2(r.rect.right() - 16.0, r.rect.top() + 14.0), b, pal);
                }
                let r = r.on_hover_text(format!("{} <-> {}\n{}", l.local.display(), l.remote, tip));
                if r.clicked() {
                    actions.push(Action::Go(self.focus, l.local.clone()));
                }
                r.context_menu(|ui| {
                    if ui.button("Verify now").clicked() {
                        actions.push(Action::Verify(l.name.clone()));
                        ui.close();
                    }
                });
            }
            if any_cloud && ui.small_button("Manage cloud drives").on_hover_text("Settings > Cloud Storage").clicked() {
                actions.push(Action::ManageCloud);
            }
        });
    }
}

impl Drop for App {
    fn drop(&mut self) {
        session::leave();
    }
}

impl eframe::App for App {
    /// Fully see-through behind everything, so the translucent panels show the desktop (the default is a dark grey that would tint it).
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.0, 0.0, 0.0, 0.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        static FIRST: std::sync::Once = std::sync::Once::new();
        FIRST.call_once(|| since_start("first frame"));
        let here = self.panes[self.focus].tabs[self.panes[self.focus].active].dir();
        if here != self.published && self.picker.is_none() {
            session::publish(&here);
            self.published = here;
        }
        // light or dark and the accent follow the system's setting (Settings, Appearance); hidden items, drive status and
        // see-through follow Settings, Apps. Read at the start and when the settings folder changes (the watch wakes Minos
        // then), never on a timer.
        if theme::settings_changed(&ctx, "minos") {
            let look = yggdrasil_conf::appearance::load();
            self.st.dark = look.dark;
            self.st.system_accent = look.accent;
            let conf = yggdrasil_conf::PanelConf::load();
            self.st.drive_status = conf.drive_status;
            self.st.opacity = if conf.minos_transparent { conf.minos_opacity } else { 100 };
            let show = conf.show_hidden;
            if show != self.st.show_hidden {
                self.st.show_hidden = show;
                self.reload_all();
            }
            // Settings > Cloud Storage writes its list into the same folder (Proton Drive added or removed there too)
            self.cloud_drives.reload_list();
            self.proton_added = cloud::proton_added();
            self.cloud = places::proton_drive(self.proton_added);
        }
        let want = (self.st.dark, self.st.accent(), self.st.opacity);
        if self.applied_theme != Some(want) {
            theme::apply_opacity(&ctx, want.0, want.1, want.2);
            self.applied_theme = Some(want);
        }
        let pal = palette_for(self.st.dark, self.st.accent()).translucent(self.st.opacity);
        let hidden = self.st.show_hidden;
        let split = self.st.split;
        for (i, p) in self.panes.iter_mut().enumerate() {
            if i == 0 || split {
                p.ensure_watch(&ctx);
                if p.poll_watch(hidden) {
                    ctx.request_repaint_after(Duration::from_millis(100));
                }
            }
        }
        while let Ok(d) = self.drives_rx.try_recv() {
            self.drives = d;
        }
        let first = self.dev_ctx.lock().map(|mut g| g.replace(ctx.clone()).is_none()).unwrap_or(false);
        if first {
            self.open_new_drives(&ctx);
        }
        let mut changed = false;
        while let Ok(ev) = self.dev_rx.try_recv() {
            match ev {
                DevEvent::Changed => changed = true,
                DevEvent::Done(msg, go) => {
                    self.dev_busy = None;
                    self.status = msg;
                    changed = true;
                    if let Some((p, path)) = go {
                        let sort = self.panes[p].sort;
                        self.panes[p].cur().navigate(path, sort, self.st.show_hidden);
                    }
                }
            }
        }
        if changed {
            self.disks = places::disks();
            self.drives = places::drives();
            self.reload_virtual();
            self.open_new_drives(&ctx);
        }
        if self.drives.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
        while let Ok(res) = self.proton_rx.try_recv() {
            self.proton_busy = false;
            match res {
                Ok(()) => {
                    self.status = "Proton Drive is connecting; its files appear in a few seconds".into();
                    self.proton_form = None;
                    self.proton_wait = Some(Instant::now() + Duration::from_secs(30));
                }
                Err(e) => match e.strip_prefix(cloud::SIGN_IN_AGAIN) {
                    Some(msg) => {
                        self.status = msg.to_string();
                        let mut f: [String; 4] = Default::default();
                        f[0] = cloud::proton_username();
                        self.proton_form = Some(f);
                    }
                    None => self.status = e,
                },
            }
            self.proton_added = cloud::proton_added();
            self.cloud = places::proton_drive(self.proton_added);
        }
        let waiting =
            self.proton_wait.is_some_and(|t| Instant::now() < t) && self.cloud.as_ref().is_some_and(|c| c.state == CloudState::NotMounted);
        if !waiting {
            self.proton_wait = None;
        }
        if self.proton_busy || waiting {
            // keep looking while the mount comes up after a sign-in (a file check every 2 s, for 30 s at most)
            self.cloud = places::proton_drive(self.proton_added);
            ctx.request_repaint_after(Duration::from_secs(2));
        }
        if let Some(t) = self.download_status.lock().ok().and_then(|mut g| g.take()) {
            self.status = t;
        }
        let (open, say) = self.cloud_drives.poll(&ctx);
        if let Some(path) = open {
            let (f, sort) = (self.focus, self.panes[self.focus].sort);
            self.panes[f].cur().navigate(path, sort, self.st.show_hidden);
        }
        if let Some(s) = say {
            self.status = s;
        }
        while let Ok((name, res)) = self.verify_rx.try_recv() {
            self.verifying.retain(|n| *n != name);
            match res {
                Ok(r) => {
                    self.status = format!("{name}: {}", r.summary());
                    self.reports.insert(name, r);
                }
                Err(e) => self.status = format!("{name}: verification failed: {e}"),
            }
        }

        let mut actions: Vec<Action> = vec![];
        chrome::resize_edges(&ctx);

        // Tab and window shortcuts work everywhere, even in a text field.
        {
            let f = self.focus;
            let n = self.panes[f].tabs.len();
            let cur = self.panes[f].active;
            ctx.input_mut(|i| {
                use egui::{Key, Modifiers};
                if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Tab) {
                    actions.push(Action::SelectTab(f, (cur + n - 1) % n));
                } else if i.consume_key(Modifiers::COMMAND, Key::Tab) {
                    actions.push(Action::SelectTab(f, (cur + 1) % n));
                }
                if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::T) {
                    actions.push(Action::ReopenTab);
                } else if i.consume_key(Modifiers::COMMAND, Key::T) {
                    actions.push(Action::NewTab(f));
                }
                if i.modifiers.alt && !i.modifiers.command {
                    if i.consume_key(Modifiers::ALT, Key::ArrowLeft) {
                        actions.push(Action::Back(f));
                    }
                    if i.consume_key(Modifiers::ALT, Key::ArrowRight) {
                        actions.push(Action::Forward(f));
                    }
                }
                for (k, key) in [Key::Num1, Key::Num2, Key::Num3, Key::Num4, Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9]
                    .into_iter()
                    .enumerate()
                {
                    // Ctrl+1..8 pick that tab, Ctrl+9 the last, as in browsers
                    if i.consume_key(Modifiers::COMMAND, key) {
                        let to = if k == 8 { n - 1 } else { k };
                        if to < n {
                            actions.push(Action::SelectTab(f, to));
                        }
                    }
                }
                if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::W) {
                    actions.push(Action::CloseWindow);
                } else if i.consume_key(Modifiers::COMMAND, Key::W) {
                    actions.push(Action::CloseTab(f, cur));
                }
            });
        }

        // Keyboard shortcuts, only when no text field has focus.
        if !ctx.egui_wants_keyboard_input() {
            let sel = self.selected_in_focus();
            let f = self.focus;
            let mut open_search = false;
            let mut paste_req: Option<Option<String>> = None;
            ctx.input_mut(|i| {
                use egui::{Key, Modifiers};
                for (key, nav) in [
                    (Key::ArrowUp, Nav::Up),
                    (Key::ArrowDown, Nav::Down),
                    (Key::ArrowLeft, Nav::Left),
                    (Key::ArrowRight, Nav::Right),
                    (Key::Home, Nav::Home),
                    (Key::End, Nav::End),
                    (Key::PageUp, Nav::PageUp),
                    (Key::PageDown, Nav::PageDown),
                ] {
                    if i.consume_key(Modifiers::NONE, key) {
                        actions.push(Action::KeyNav(f, nav, false));
                    } else if i.consume_key(Modifiers::SHIFT, key) {
                        actions.push(Action::KeyNav(f, nav, true));
                    }
                }
                // Alt+Enter first: the plain Enter below would take it too (egui matches Enter with Alt held as Enter)
                if i.consume_key(Modifiers::ALT, Key::Enter) {
                    actions.push(Action::Properties(f));
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) {
                    actions.push(Action::KeyOpen(f));
                }
                if i.consume_key(Modifiers::NONE, Key::Tab) {
                    actions.push(Action::FocusOtherPane);
                }
                // plain typing finds an entry by its first letters (not with Ctrl or Alt held)
                if !i.modifiers.command && !i.modifiers.alt {
                    let typed: String =
                        i.events.iter().filter_map(|e| if let egui::Event::Text(t) = e { Some(t.as_str()) } else { None }).collect();
                    let typed: String = typed.chars().filter(|c| !c.is_control()).collect();
                    if !typed.is_empty() {
                        actions.push(Action::TypeFind(f, typed));
                    }
                }
            });
            ctx.input(|i| {
                let cmd = i.modifiers.command;
                // egui turns Ctrl+C / Ctrl+X / Ctrl+V into dedicated events and does NOT also
                // deliver them as key presses, so key_pressed(C) alone never fires.
                let copy_ev = i.events.iter().any(|e| matches!(e, egui::Event::Copy));
                let cut_ev = i.events.iter().any(|e| matches!(e, egui::Event::Cut));
                let paste_text = i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None });
                if (i.modifiers.alt && i.key_pressed(egui::Key::ArrowUp)) || i.key_pressed(egui::Key::Backspace) {
                    actions.push(Action::Up(f));
                }
                if i.key_pressed(egui::Key::F3) {
                    actions.push(Action::TogglePreview);
                }
                if cmd && i.key_pressed(egui::Key::F) {
                    open_search = true;
                }
                if cmd && i.key_pressed(egui::Key::L) {
                    actions.push(Action::EditAddress(f));
                }
                if i.key_pressed(egui::Key::F5) {
                    actions.push(Action::Reload);
                }
                if (cut_ev || (cmd && i.key_pressed(egui::Key::X))) && !sel.is_empty() {
                    actions.push(Action::Cut(sel.clone()));
                }
                if (copy_ev || (cmd && i.key_pressed(egui::Key::C))) && !sel.is_empty() {
                    actions.push(Action::Copy(sel.clone()));
                }
                // Ctrl+V comes only as a paste event, and only when the system clipboard holds text: Minos's copy and cut put the
                // files there as file:// addresses (below), so it does; with nothing there, the key itself (rarely seen) still pastes
                if let Some(t) = paste_text {
                    paste_req = Some(Some(t));
                } else if cmd && i.key_pressed(egui::Key::V) {
                    paste_req = Some(None);
                }
                if i.key_pressed(egui::Key::F2) && sel.len() == 1 {
                    actions.push(Action::Rename(sel[0].clone()));
                }
                if i.key_pressed(egui::Key::Delete) && !sel.is_empty() {
                    actions.push(Action::Trash(sel.clone()));
                }
                if cmd && i.modifiers.shift && i.key_pressed(egui::Key::N) {
                    actions.push(Action::NewFolder(f));
                }
            });
            if let Some(text) = paste_req {
                match text {
                    // another program's files (or text) since Minos's own copy
                    // compared without line endings: Minos writes \r\n (text/uri-list), egui hands \n back, and a cut pasted
                    // with Ctrl+V was taken for another program's files and copied instead of moved (2026-11-08)
                    Some(t) if self.clip_text.as_deref().map(same_text) != Some(same_text(&t)) => {
                        let paths = paths_from_clipboard(&t);
                        if paths.is_empty() {
                            self.status = "The clipboard holds text, not files".into();
                        } else {
                            self.clipboard = Some((paths, false));
                            self.clip_text = Some(t);
                            actions.push(Action::Paste(f));
                        }
                    }
                    _ => actions.push(Action::Paste(f)),
                }
            }
            if open_search {
                let t = self.panes[self.focus].cur();
                t.search_open = true;
                t.search_focus = true;
            }
            // Ctrl+A selects everything visible in the focused tab.
            if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::A)) {
                let t = self.panes[self.focus].cur();
                let all: Vec<PathBuf> = t.order(hidden, GroupBy::None).into_iter().map(|i| t.shown()[i].path.clone()).collect();
                t.selected = all.into_iter().collect();
            }
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            let full = ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(!full));
        }

        // Above the sidebar too, so it runs the full width like a title bar.
        egui::Panel::top("strip").frame(egui::Frame::new().fill(pal.strip).inner_margin(egui::Margin::symmetric(8, 4))).show(ui, |ui| {
            ui.horizontal(|ui| {
                let menu = icon_button(ui, &pal, Icon::Menu, "Menu", true, false);
                egui::Popup::menu(&menu).show(|ui| {
                    ui.separator();
                    if ui.selectable_label(self.st.view == ViewMode::Details, "Details view").clicked() {
                        actions.push(Action::SetView(ViewMode::Details));
                    }
                    if ui.selectable_label(self.st.view == ViewMode::Gallery, "Gallery view").clicked() {
                        actions.push(Action::SetView(ViewMode::Gallery));
                    }
                    if self.st.view == ViewMode::Gallery {
                        let mut s = self.st.gallery_size as u8;
                        if ui.add(egui::Slider::new(&mut s, 0..=3).text("Thumbnail size")).changed() {
                            self.st.gallery_size = s as usize;
                            self.dirty = true;
                        }
                    }
                    ui.separator();
                    let mut pv = self.st.preview;
                    if ui.checkbox(&mut pv, "Preview pane (F3)").changed() {
                        actions.push(Action::TogglePreview);
                    }
                    let mut sp = self.st.split;
                    if ui.checkbox(&mut sp, "Split pane").changed() {
                        actions.push(Action::ToggleSplit);
                    }
                    let mut side = !self.st.sidebar_hidden;
                    if ui.checkbox(&mut side, "Sidebar").changed() {
                        actions.push(Action::ToggleSidebar);
                    }
                    ui.separator();
                    if ui.button("Settings...").clicked() {
                        actions.push(Action::OpenSettings);
                    }
                });
                ui.add_space(6.0);
                // the tabs, as the Files app has them in its title bar (in split view each pane keeps its own above it)
                if !split {
                    tab_strip(ui, &self.panes[0], 0, &pal, &mut actions);
                    ui.add_space(6.0);
                }
                // The strip is the title bar: empty space moves the window, buttons at the end.
                chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                chrome::window_controls(ui, &pal);
            });
        });

        // the navigation row across the window, under the tabs, in the title strip's colour: only the current tab is
        // coloured (split view: one in each pane, draw_pane)
        if !split {
            egui::Panel::top("nav").frame(egui::Frame::new().fill(pal.strip).inner_margin(egui::Margin::symmetric(8, 5))).show(ui, |ui| {
                topbar::nav_bar(ui, &mut self.panes[0], 0, &pal, &self.st, true, &mut actions);
            });
        }

        // the bottom bar (statusbar.rs, transfers.rs): across the whole window, under the sidebar too, as in Windows Explorer
        // (in the open and save dialog only while it has a message or a transfer, and without the counts: the dialog's
        // file-type filter hides files the counts would include)
        self.bar.say(std::mem::take(&mut self.status));
        let picking = self.picker.is_some();
        if !picking || self.bar.has_message() || !self.transfers.is_empty() {
            let (bar, tr, pane) = (&mut self.bar, &mut self.transfers, &self.panes[self.focus]);
            let t = &pane.tabs[pane.active];
            let view = statusbar::View {
                dir: t.tab.history.current(),
                entries: &t.entries,
                results: t.results.as_deref(),
                // the command bar's quick filter counts as the filter while it is set (YGG-169)
                query: if t.filter.is_empty() { &t.query } else { &t.filter },
                hidden: self.st.show_hidden,
                selected: &t.selected,
                loading: t.pending.is_some(),
                counts: !picking,
            };
            egui::Panel::bottom("statusbar")
                .exact_size(statusbar::HEIGHT)
                .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(8, 0)))
                .show(ui, |ui| {
                    ui.horizontal_centered(|ui| bar.show(ui, &pal, &view, |ui| tr.bar(ui, &pal)));
                });
            if let Some(dir) = self.transfers.panel(&ctx, &pal, statusbar::HEIGHT) {
                actions.push(Action::Go(self.focus, dir));
            }
        }

        if !self.st.sidebar_hidden {
            egui::Panel::left("sidebar")
                .default_size(230.0)
                .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(8, 6)))
                .show(ui, |ui| {
                    self.sidebar(ui, &pal, &mut actions);
                });
        }

        if self.st.preview {
            let want: Vec<PathBuf> = {
                let p = &self.panes[self.focus];
                let t = &p.tabs[p.active];
                if t.selected.is_empty() {
                    vec![t.dir()]
                } else {
                    t.selected.iter().cloned().collect()
                }
            };
            if self.pv.key.as_ref() != Some(&want) {
                self.pv.key = Some(want.clone());
                // A drive selected in "This PC" gets a usage summary instead of a folder listing.
                let drive = {
                    let p = &self.panes[self.focus];
                    let t = &p.tabs[p.active];
                    if want.len() == 1 && places::is_virtual(&t.dir()) {
                        t.entries
                            .iter()
                            .find(|e| e.path == want[0])
                            .and_then(|e| e.usage.map(|(total, free)| preview::drive(&e.name, total, free)))
                    } else if want.len() == 1 && places::is_this_pc(&want[0]) {
                        Some(preview::message(&places::pc_name(), "Your drives", "Select a drive to see how full it is"))
                    } else {
                        None
                    }
                };
                self.pv.request(&ctx, want, drive);
            }
            self.pv.poll(&ctx);
            // the width the pane was last dragged to (the narrowest at first), kept for next time once the drag ends
            let min = preview_min(ui);
            let shown = egui::Panel::right("preview")
                .default_size(self.st.preview_width.clamp(min, PREVIEW_MAX))
                .max_size(PREVIEW_MAX)
                .min_size(min)
                .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(egui::Margin::symmetric(14, 12)))
                .show(ui, |ui| {
                    draw_preview(ui, &pal, &self.pv);
                });
            // at the narrowest it is kept as "the narrowest" (PREVIEW_MIN), so it follows a later change of font or text size
            let width = shown.response.rect.width().round();
            let keep = if width <= min.round() + 1.0 { PREVIEW_MIN } else { width };
            if (keep - self.st.preview_width).abs() >= 1.0 && !ctx.input(|i| i.pointer.any_down()) {
                self.st.preview_width = keep;
                self.dirty = true;
            }
        }

        ctx.data_mut(|d| {
            d.insert_temp(egui::Id::new(RENAME_ID), self.rename.clone());
            d.remove::<Option<(PathBuf, String)>>(egui::Id::new(RENAME_RESULT_ID));
            d.insert_temp(egui::Id::new("minos-rename-seen"), false);
        });
        let clip_has = self.clipboard.is_some();
        // what is cut and not yet pasted: drawn faded (as on Windows), until it is pasted or another copy or cut replaces it
        let cut: Arc<std::collections::HashSet<PathBuf>> = Arc::new(match &self.clipboard {
            Some((v, true)) => v.iter().cloned().collect(),
            _ => Default::default(),
        });
        ctx.data_mut(|d| d.insert_temp(egui::Id::new(CUT_ID), cut));
        let focus = self.focus;
        if let Some(pk) = self.picker.as_mut() {
            let cur = self.panes[focus].tabs[self.panes[focus].active].dir();
            let sel: Vec<(PathBuf, bool)> =
                self.panes[focus].tabs[self.panes[focus].active].selected.iter().map(|p| (p.clone(), p.is_dir())).collect();
            if let Ok(mut g) = PICK_GLOBS.lock() {
                *g = pk.filters.get(pk.filter).map(|f| f.1.clone()).unwrap_or_default();
            }
            let mut choose: Option<Vec<PathBuf>> = None;
            let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape)) && !ctx.egui_wants_keyboard_input();
            egui::Panel::bottom("picker").frame(egui::Frame::new().fill(pal.strip).inner_margin(egui::Margin::symmetric(14, 12))).show(
                ui,
                |ui| {
                    // like the Windows dialog: the name on top with the file type beside it, the buttons below on the right
                    let label_w = 78.0;
                    let filter_w = if pk.filters.is_empty() { 0.0 } else { 190.0 };
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [label_w, 28.0],
                            egui::Label::new(RichText::new(if pk.dir { "Folder:" } else { "File name:" }).size(14.0)),
                        );
                        let field_w = (ui.available_width() - filter_w - if filter_w > 0.0 { 10.0 } else { 0.0 }).max(120.0);
                        if pk.save {
                            ui.add_sized([field_w, 28.0], egui::TextEdit::singleline(&mut pk.name).vertical_align(Align::Center));
                        } else {
                            let shown = if pk.dir {
                                if sel.len() == 1 && sel[0].1 {
                                    sel[0].0.display().to_string()
                                } else {
                                    cur.display().to_string()
                                }
                            } else {
                                sel.iter()
                                    .filter_map(|s| s.0.file_name())
                                    .map(|n| format!("\"{}\"", n.to_string_lossy()))
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            };
                            let mut text = shown;
                            ui.add_sized(
                                [field_w, 28.0],
                                egui::TextEdit::singleline(&mut text).interactive(false).vertical_align(Align::Center),
                            );
                        }
                        if pk.filters.len() > 1 {
                            egui::ComboBox::from_id_salt("pickfilter")
                                .width(filter_w - 20.0)
                                .selected_text(pk.filters[pk.filter].0.clone())
                                .show_ui(ui, |ui| {
                                    for (k, f) in pk.filters.iter().enumerate() {
                                        ui.selectable_value(&mut pk.filter, k, f.0.clone());
                                    }
                                });
                        } else if let Some(f) = pk.filters.first() {
                            ui.add_sized([filter_w, 28.0], egui::Label::new(RichText::new(f.0.clone()).color(pal.dim)));
                        }
                    });
                    ui.add_space(10.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let ok = if pk.save {
                            !pk.name.trim().is_empty() && !pk.name.contains('/')
                        } else if pk.dir {
                            true
                        } else {
                            !sel.is_empty() && sel.iter().all(|s| !s.1) && (pk.multiple || sel.len() == 1)
                        };
                        let label = if pk.save && cur.join(pk.name.trim()).exists() { "Replace".to_string() } else { pk.button() };
                        let size = vec2(110.0, 34.0);
                        // the main button first (it sits on the far right), filled with the accent so it is easy to see
                        let main = egui::Button::new(RichText::new(label).size(15.0).strong().color(Color32::WHITE))
                            .fill(pal.accent)
                            .min_size(size);
                        if ui.add_enabled(ok, main).clicked() {
                            choose = Some(if pk.save {
                                vec![cur.join(pk.name.trim())]
                            } else if pk.dir {
                                if sel.len() == 1 && sel[0].1 {
                                    vec![sel[0].0.clone()]
                                } else {
                                    vec![cur.clone()]
                                }
                            } else {
                                sel.iter().map(|s| s.0.clone()).collect()
                            });
                        }
                        if ui.add(egui::Button::new(RichText::new("Cancel").size(15.0)).min_size(size)).clicked() {
                            cancel = true;
                        }
                    });
                },
            );
            if let Some(c) = choose {
                finish_pick(&c);
            }
            if cancel {
                std::process::exit(1);
            }
        }
        let st = &self.st;
        let cloud_ctx = (self.links.as_slice(), &self.reports);
        let [left, right] = &mut self.panes;
        egui::CentralPanel::default().show(ui, |ui| {
            if split {
                ui.columns(2, |cols| {
                    ui_pane(&mut cols[0], left, 0, focus == 0, st, cloud_ctx, clip_has, &mut actions);
                    ui_pane(&mut cols[1], right, 1, focus == 1, st, cloud_ctx, clip_has, &mut actions);
                });
            } else {
                ui_pane(ui, left, 0, true, st, cloud_ctx, clip_has, &mut actions);
            }
        });

        // Dialogs.
        // The rename happens in place, in the row (inline_rename); take back what it did.
        let mut rename_done: Option<Option<(PathBuf, String)>> = None;
        if self.rename.is_some() {
            let (cur, seen, res) = ctx.data(|d| {
                (
                    d.get_temp::<Option<RenameDlg>>(egui::Id::new(RENAME_ID)).flatten(),
                    d.get_temp::<bool>(egui::Id::new("minos-rename-seen")).unwrap_or(false),
                    d.get_temp::<Option<(PathBuf, String)>>(egui::Id::new(RENAME_RESULT_ID)),
                )
            });
            if let Some(res) = res {
                rename_done = Some(res);
            } else if !seen {
                // the item is not on screen any more (another folder, another view): drop it
                if self.rename_unseen > 2 {
                    rename_done = Some(None);
                }
                self.rename_unseen += 1;
                ctx.request_repaint();
            } else {
                self.rename_unseen = 0;
                self.rename = cur;
            }
        }
        // a click on a name that was waiting out the double-click time now starts renaming
        if let Some((p, at)) = ctx.data(|d| d.get_temp::<(PathBuf, f64)>(egui::Id::new("minos-rename-pending"))) {
            if ctx.input(|i| i.time) - at > 0.45 {
                ctx.data_mut(|d| d.remove::<(PathBuf, f64)>(egui::Id::new("minos-rename-pending")));
                actions.push(Action::Rename(p));
            } else {
                ctx.request_repaint_after(Duration::from_millis(100));
            }
        }
        if let Some(done) = rename_done {
            self.rename = None;
            if let Some((path, name)) = done {
                match fileops::rename(&path, &name) {
                    Ok(new) => {
                        self.st.rekey(&path, &new);
                        self.dirty = true;
                        self.status = format!("Renamed to {name}");
                    }
                    Err(e) => self.status = format!("Rename failed: {e}"),
                }
                self.reload_all();
            }
        }

        self.format_dialog(&ctx);
        if let Some(d) = &mut self.props {
            if !d.show(&ctx, &pal) {
                self.props = None;
            }
        }

        let mut proton_go = false;
        let mut proton_cancel = false;
        if let Some(w) = &mut self.settings_win {
            let mut manage = false;
            if !w.show(&ctx, &pal, self.proton_added, &mut manage) {
                self.settings_win = None;
            }
            if manage {
                if let Err(e) = cloudside::open_settings() {
                    self.status = e;
                }
            }
        }
        if let Some(f) = &mut self.proton_form {
            let busy = self.proton_busy;
            egui::Window::new("Connect Proton Drive").collapsible(false).show(&ctx, |ui| {
                ui.label("Proton email or username");
                ui.text_edit_singleline(&mut f[0]);
                ui.label("Password");
                ui.add(egui::TextEdit::singleline(&mut f[1]).password(true));
                ui.label("Two-factor code (if you use one; it lasts about 30 seconds)");
                ui.text_edit_singleline(&mut f[3]);
                ui.label(
                    RichText::new("Kept, scrambled, in rclone's own config, which only you can read; never on a command line.").small(),
                );
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(
                            !busy && !f[0].is_empty() && !f[1].is_empty(),
                            egui::Button::new(if busy { "Connecting..." } else { "Connect" }),
                        )
                        .clicked()
                    {
                        proton_go = true;
                    }
                    if ui.add_enabled(!busy, egui::Button::new("Cancel")).clicked() {
                        proton_cancel = true;
                    }
                });
            });
        }
        if proton_go {
            if let Some(f) = self.proton_form.clone() {
                self.proton_busy = true;
                let (tx, c) = (self.proton_tx.clone(), ctx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(cloud::connect_proton(&f[0], &f[1], &f[2], &f[3]));
                    c.request_repaint();
                });
            }
        }
        if proton_cancel {
            self.proton_form = None;
        }

        self.poll_transfer(&ctx);
        self.poll_scrubs(&ctx);
        self.poll_strips(&ctx);
        self.poll_extracts();
        self.pump_dnd(&ctx, _frame);
        if let Some(p) = egui::DragAndDrop::payload::<DragPaths>(&ctx) {
            if let Some(pos) = ctx.pointer_latest_pos() {
                egui::Area::new("minos-drag-ghost".into())
                    .order(egui::Order::Tooltip)
                    .fixed_pos(pos + vec2(14.0, 14.0))
                    .interactable(false)
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.label(if p.0.len() == 1 {
                                format!("Move {}", p.0[0].file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default())
                            } else {
                                format!("Move {} items", p.0.len())
                            });
                        });
                    });
            }
        }

        chrome::outline(&ctx, &pal);
        self.apply(&ctx, actions);
        if !self.status.is_empty() {
            // a message set just now goes to the bottom bar on the next frame
            ctx.request_repaint();
        }
        self.sync_state();
        if self.dirty && self.last_saved.elapsed() > Duration::from_secs(1) {
            self.save();
        }
    }

    fn on_exit(&mut self) {
        self.sync_state();
        self.save();
    }
}

#[allow(clippy::too_many_arguments)]
fn ui_pane(
    ui: &mut egui::Ui,
    pane: &mut Pane,
    pi: usize,
    focused: bool,
    st: &State,
    cloud_ctx: (&[Link], &BTreeMap<String, Report>),
    clip_has: bool,
    actions: &mut Vec<Action>,
) {
    ui.push_id(pi, |ui| draw_pane(ui, pane, pi, focused, st, cloud_ctx, clip_has, actions));
}

/// A folder given on the command line: a path, a file:// URL, or trash:/ (the trash folder).
fn start_path(arg: &str) -> PathBuf {
    if arg.starts_with("trash:") {
        let base = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".local/share")));
        return base.map(|b| b.join("Trash/files")).unwrap_or_default();
    }
    PathBuf::from(arg.strip_prefix("file://").unwrap_or(arg))
}

/// File-chooser mode: `minos --pick open|save|dir ...` lets a program ask for files (the portal backend runs it).
/// The chosen paths are printed one per line and it exits 0; closing it or Cancel exits 1.
struct Picker {
    save: bool,
    dir: bool,
    multiple: bool,
    name: String,
    accept: String,
    /// (label, globs such as `*.png`)
    filters: Vec<(String, Vec<String>)>,
    filter: usize,
}

impl Picker {
    /// Read the command line (everything after `--pick`); None when Minos is not asked to pick.
    fn from_args(args: &[String]) -> Option<(Picker, Option<PathBuf>, String)> {
        let i = args.iter().position(|a| a == "--pick")?;
        let mode = args.get(i + 1).map(String::as_str).unwrap_or("open");
        let mut p = Picker {
            save: mode == "save",
            dir: mode == "dir",
            multiple: false,
            name: String::new(),
            accept: String::new(),
            filters: vec![],
            filter: 0,
        };
        let (mut folder, mut title) = (None, String::new());
        let mut k = i + 2;
        while k < args.len() {
            let val = args.get(k + 1).cloned().unwrap_or_default();
            match args[k].as_str() {
                "--multiple" => p.multiple = true,
                "--title" => {
                    title = val;
                    k += 1
                }
                "--name" => {
                    p.name = val;
                    k += 1
                }
                "--accept" => {
                    p.accept = val;
                    k += 1
                }
                "--folder" => {
                    folder = Some(PathBuf::from(val));
                    k += 1
                }
                "--filter-index" => {
                    p.filter = val.parse().unwrap_or(0);
                    k += 1
                }
                "--filter" => {
                    let (label, globs) = val.split_once('|').unwrap_or((val.as_str(), ""));
                    p.filters.push((label.to_string(), globs.split(';').filter(|g| !g.is_empty()).map(str::to_string).collect()));
                    k += 1
                }
                _ => {}
            }
            k += 1;
        }
        p.filter = p.filter.min(p.filters.len().saturating_sub(1));
        Some((p, folder, title))
    }

    fn button(&self) -> String {
        if !self.accept.is_empty() {
            self.accept.replace('_', "")
        } else if self.save {
            "Save".into()
        } else if self.dir {
            "Select".into()
        } else {
            "Open".into()
        }
    }
}

/// The patterns of the chosen file type while picking, applied to the file list.
static PICK_GLOBS: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());

/// `*.png`-style match, case-insensitive (`*` any run, `?` any one character).
fn glob_match(pat: &str, name: &str) -> bool {
    let (p, n): (Vec<char>, Vec<char>) = (pat.to_lowercase().chars().collect(), name.to_lowercase().chars().collect());
    fn go(p: &[char], n: &[char]) -> bool {
        match p.split_first() {
            None => n.is_empty(),
            Some(('*', rest)) => (0..=n.len()).any(|k| go(rest, &n[k..])),
            Some(('?', rest)) => !n.is_empty() && go(rest, &n[1..]),
            Some((c, rest)) => n.first() == Some(c) && go(rest, &n[1..]),
        }
    }
    go(&p, &n)
}

/// Whether the picking filter hides this entry (folders always show).
fn pick_hides(e: &Entry) -> bool {
    if e.is_dir {
        return false;
    }
    let g = PICK_GLOBS.lock().map(|g| g.clone()).unwrap_or_default();
    !g.is_empty() && !g.iter().any(|p| glob_match(p, &e.name))
}

/// Print the chosen paths and leave, so the portal that started Minos can read them.
fn finish_pick(paths: &[PathBuf]) -> ! {
    use std::io::Write;
    let mut out = std::io::stdout().lock();
    for p in paths {
        let _ = writeln!(out, "{}", p.display());
    }
    let _ = out.flush();
    std::process::exit(0)
}

static STARTED: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// With MINOS_TIMING set, print how long after launch `what` happened.
fn since_start(what: &str) {
    if std::env::var_os("MINOS_TIMING").is_some() {
        eprintln!("minos: {} ms since launch: {what}", STARTED.get().map(|t| t.elapsed().as_millis()).unwrap_or(0));
    }
}

/// `--window FOLDER`: the folder of a tab dragged out of another Minos window.
fn window_arg(argv: &[String]) -> Option<PathBuf> {
    let i = argv.iter().position(|a| a == "--window")?;
    argv.get(i + 1).map(PathBuf::from).filter(|p| p.is_dir())
}

fn main() -> eframe::Result {
    STARTED.get_or_init(Instant::now);
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let pick = Picker::from_args(&argv);
    let title = pick.as_ref().map(|p| p.2.clone()).filter(|t| !t.is_empty()).unwrap_or_else(|| "Minos".into());
    let picking = pick.is_some();
    let mut pick = Some(pick);
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1200.0, 760.0])
            .with_min_inner_size([640.0, 400.0])
            .with_title(title)
            // Minos draws its own title bar (chrome.rs): no grey system title bar or frame.
            .with_decorations(false)
            // see-through when Settings asks for it (it can only be chosen when the window is made)
            .with_transparent(true)
            // Wayland/X11 application id: matches minos.desktop so docks and the
            // bar can find the icon. Must be a plain name; "Minos" was not.
            .with_app_id("minos"),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Minos",
        options,
        Box::new(|cc| {
            yggdrasil_ui::fonts::install(&cc.egui_ctx);
            since_start("window and renderer ready");
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let mut st = State::config_path().as_deref().map(State::load_from).unwrap_or_default();
            st.dark = theme::system_dark();
            st.follow_accent = true;
            theme::apply(&cc.egui_ctx, st.dark, if st.follow_accent { theme::system_accent() } else { None });
            let mut app = App::new();
            // made-up transfers for the screenshots (scripts/screenshots.sh); a debug build only
            #[cfg(debug_assertions)]
            if let Some(a) = argv.iter().find(|a| a.starts_with("--demo-transfers")) {
                app.transfers.demo(a.strip_prefix("--demo-transfers=").unwrap_or(""));
            }
            // `minos FOLDER` (the dock's Trash, "open with"): that folder in a new tab.
            if let Some((pk, folder, _)) = pick.take().flatten() {
                // choosing a file: no saved tabs, no saved state, starts in the folder asked for
                let start = folder.filter(|f| f.is_dir()).or_else(dirs::home_dir).unwrap_or_else(|| PathBuf::from("/"));
                let (sort, hidden) = (app.panes[0].sort, app.st.show_hidden);
                app.panes[0].tabs = vec![TabView::new(start, false, sort, hidden)];
                app.panes[0].active = 0;
                app.st.split = false;
                app.st.preview = false;
                app.st_path = None;
                PICK_GLOBS.lock().unwrap().clear();
                app.picker = Some(pk);
            } else if let Some(p) = window_arg(&argv) {
                // a tab torn off another window: only that folder, and this window does not keep its tabs for next time
                let (sort, hidden) = (app.panes[0].sort, app.st.show_hidden);
                app.panes[0].tabs = vec![TabView::new(p, false, sort, hidden)];
                app.panes[0].active = 0;
                app.st.split = false;
                app.detached = true;
            } else if let Some(p) = std::env::args().nth(1).filter(|a| !a.starts_with("--")).map(|a| start_path(&a)).filter(|p| p.is_dir())
            {
                let (sort, hidden) = (app.panes[0].sort, app.st.show_hidden);
                app.panes[0].tabs.push(TabView::new(p, false, sort, hidden));
                app.panes[0].active = app.panes[0].tabs.len() - 1;
            }
            since_start("app built");
            Ok(Box::new(app))
        }),
    );
    if picking {
        // the window was closed without choosing
        std::process::exit(1);
    }
    result
}

/// Which Minos windows are open and the folder each is in, so a new window can start where another is.
mod session {
    use std::path::PathBuf;

    fn dir() -> Option<PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR").map(|d| PathBuf::from(d).join("minos"))
    }

    fn alive(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/comm")).map(|c| c.trim() == "minos").unwrap_or(false)
    }

    /// Folders of the other open windows, most recently active first.
    pub fn others() -> Vec<PathBuf> {
        let Some(d) = dir() else { return vec![] };
        let me = std::process::id();
        let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(d)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| {
                let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                if pid == me || !alive(pid) {
                    let _ = if pid != me { std::fs::remove_file(e.path()) } else { Ok(()) };
                    return None;
                }
                Some((e.metadata().ok()?.modified().ok()?, PathBuf::from(std::fs::read_to_string(e.path()).ok()?.trim_end_matches('\n'))))
            })
            .collect();
        v.sort_by_key(|a| std::cmp::Reverse(a.0));
        v.into_iter().map(|x| x.1).collect()
    }

    pub fn publish(folder: &std::path::Path) {
        if let Some(d) = dir() {
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join(std::process::id().to_string()), folder.to_string_lossy().as_bytes());
        }
    }

    pub fn leave() {
        if let Some(d) = dir() {
            let _ = std::fs::remove_file(d.join(std::process::id().to_string()));
        }
    }
}

/// The accent for drop targets: the operating system's default (its branding), else Yggdrasil's.
/// Where an item is this frame, for a box being drawn (YGG-77). A table row's response reaches below the row (egui_extras),
/// so rows are given as their top and the table's row height.
fn record_item(ctx: &egui::Context, pi: usize, rect: Rect, path: &Path) {
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(Rect, PathBuf)>>(egui::Id::new(("minos-item-rects", pi))).push((rect, path.to_path_buf()))
    });
}

// ---- box selection (YGG-77) ----

/// A box being drawn in a pane: where the press began, and the selection it adds to (Ctrl held at the start), or none.
#[derive(Clone, Debug)]
struct BoxDrag {
    start: egui::Pos2,
    base: BTreeSet<PathBuf>,
}

/// What a box covers: every item whose rectangle it touches, added to `base`.
fn box_covers(items: &[(Rect, PathBuf)], area: Rect, base: &BTreeSet<PathBuf>) -> BTreeSet<PathBuf> {
    let mut out = base.clone();
    out.extend(items.iter().filter(|(r, _)| r.intersects(area)).map(|(_, p)| p.clone()));
    out
}

/// A second tap this soon after a tap, this near it, starts a box when the finger then drags (a double tap and drag);
/// a tap is a touch shorter and stiller than these.
const DOUBLE_TAP_S: f64 = 0.4;
const DOUBLE_TAP_NEAR: f32 = 40.0;
const TAP_MAX_S: f64 = 0.35;
const TAP_STILL: f32 = 12.0;

/// The finger's taps, for the double tap and drag. `armed`: the finger now down is a second tap's, so a drag from empty
/// space draws a box instead of scrolling.
#[derive(Clone, Debug, Default)]
struct Taps {
    last_tap: Option<(f64, egui::Pos2)>,
    press: Option<(f64, egui::Pos2)>,
    armed: bool,
}

impl Taps {
    fn start(&mut self, now: f64, at: egui::Pos2) {
        self.armed = self.last_tap.is_some_and(|(t, p)| now - t <= DOUBLE_TAP_S && p.distance(at) <= DOUBLE_TAP_NEAR);
        self.press = Some((now, at));
    }
    fn end(&mut self, now: f64, at: egui::Pos2) {
        let tap = self.press.is_some_and(|(t, p)| now - t <= TAP_MAX_S && p.distance(at) <= TAP_STILL);
        // the second tap of a double tap is not the first of another
        self.last_tap = if tap && !self.armed { Some((now, at)) } else { None };
        self.press = None;
        self.armed = false;
    }
}

/// Reads this frame's touches into the tap state; whether dragging the list should scroll it: only with a finger that is
/// not drawing a box (a mouse drag on empty space draws one, even on a computer with a touchscreen).
fn drag_scrolls(ctx: &egui::Context) -> egui::containers::scroll_area::DragScroll {
    use egui::containers::scroll_area::DragScroll;
    let id = egui::Id::new("minos-taps");
    let mut taps: Taps = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    let touching = ctx.input(|i| {
        for e in &i.events {
            if let egui::Event::Touch { phase, pos, .. } = e {
                match phase {
                    egui::TouchPhase::Start => taps.start(i.time, *pos),
                    egui::TouchPhase::End | egui::TouchPhase::Cancel => taps.end(i.time, *pos),
                    egui::TouchPhase::Move => {}
                }
            }
        }
        i.any_touches()
    });
    let armed = taps.armed;
    ctx.data_mut(|d| d.insert_temp(id, taps));
    if touching && !armed {
        DragScroll::Always
    } else {
        DragScroll::Never
    }
}

fn accent_colour() -> Color32 {
    let a = yggdrasil_conf::integrations::default_accent();
    Color32::from_rgb(a[0], a[1], a[2])
}

#[cfg(test)]
mod listing_tests {
    #[test]
    fn reading_a_folder_is_no_change_to_it() {
        use notify::event::{AccessKind, AccessMode, CreateKind, ModifyKind, RemoveKind, RenameMode};
        use notify::EventKind;
        assert!(!super::folder_changed(&EventKind::Access(AccessKind::Open(AccessMode::Any))));
        assert!(!super::folder_changed(&EventKind::Access(AccessKind::Close(AccessMode::Read))));
        assert!(super::folder_changed(&EventKind::Create(CreateKind::File)));
        assert!(super::folder_changed(&EventKind::Modify(ModifyKind::Name(RenameMode::Both))));
        assert!(super::folder_changed(&EventKind::Remove(RemoveKind::Any)));
    }

    use super::*;

    #[test]
    fn a_listing_that_comes_late_fills_the_tab_and_a_quick_one_shows_at_once() {
        let d = std::env::temp_dir().join(format!("minos-late-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("a.txt"), "x").unwrap();
        let sort = Sort::default();
        let mut t = TabView::new(d.clone(), false, sort, false);
        assert!(t.pending.is_none(), "a folder on a local disk is listed at once");
        assert_eq!(t.entries.len(), 1);
        // a drive that has not answered yet: the tab waits, and takes the listing when it comes
        let (tx, rx) = channel();
        t.entries.clear();
        t.pending = Some(Pending { dir: d.clone(), rx });
        assert!(t.poll_listing(sort), "still on its way");
        tx.send(listing::read_dir(&d)).unwrap();
        assert!(!t.poll_listing(sort));
        assert!(t.pending.is_none());
        assert_eq!(t.entries.len(), 1);
        // a listing for a folder the tab has left is dropped
        let (tx, rx) = channel();
        t.pending = Some(Pending { dir: d.join("elsewhere"), rx });
        tx.send(Ok(vec![])).unwrap();
        assert!(!t.poll_listing(sort));
        assert_eq!(t.entries.len(), 1);
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod clipboard_tests {
    use super::*;

    #[test]
    fn files_go_onto_the_clipboard_and_come_back() {
        let d = std::env::temp_dir().join(format!("minos-clip-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let a = d.join("HPT FTDI Drivers 2.08.zip");
        let b = d.join("ünïcode & more.txt");
        std::fs::write(&a, b"x").unwrap();
        std::fs::write(&b, b"y").unwrap();
        let text = uri_list(&[a.clone(), b.clone()]);
        assert!(text.contains("HPT%20FTDI%20Drivers%202.08.zip"), "{text}");
        assert_eq!(paths_from_clipboard(&text), vec![a.clone(), b.clone()]);
        // GNOME's form, and plain paths
        assert_eq!(paths_from_clipboard(&format!("copy\nfile://{}", a.display().to_string().replace(' ', "%20"))), vec![a.clone()]);
        assert_eq!(paths_from_clipboard(&format!("{}\n", b.display())), vec![b.clone()]);
        assert_eq!(same_text(&text), same_text(&text.replace("\r\n", "\n")), "Minos's own list comes back with \\n only");
        // ordinary text is not a list of files
        assert!(paths_from_clipboard("hello world").is_empty());
        assert!(paths_from_clipboard("file:///no/such/file").is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod box_tests {
    use super::*;

    fn at(x: f32, y: f32) -> egui::Pos2 {
        pos2(x, y)
    }

    /// YGG-77: a box selects the items it touches; with Ctrl it adds to what was selected; the other way round (drawn up
    /// and to the left) covers the same.
    #[test]
    fn a_box_selects_what_it_covers() {
        let items: Vec<(Rect, PathBuf)> = (0..6)
            .map(|i| (Rect::from_min_size(at(10.0, 10.0 + 30.0 * i as f32), vec2(300.0, 28.0)), PathBuf::from(format!("/d/f{i}"))))
            .collect();
        let names = |s: &BTreeSet<PathBuf>| s.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let area = Rect::from_two_pos(at(400.0, 75.0), at(200.0, 130.0));
        assert_eq!(names(&box_covers(&items, area, &BTreeSet::new())), ["f2", "f3", "f4"]);
        let base: BTreeSet<PathBuf> = [PathBuf::from("/d/f0")].into();
        assert_eq!(names(&box_covers(&items, area, &base)), ["f0", "f2", "f3", "f4"]);
        assert!(box_covers(&items, Rect::from_two_pos(at(320.0, 0.0), at(400.0, 500.0)), &BTreeSet::new()).is_empty());
    }

    /// A double tap then drag draws a box: the second touch is armed only when it comes soon after a tap, near it; a long
    /// or moving touch is no tap, and a third tap starts afresh.
    #[test]
    fn a_double_tap_arms_the_box() {
        let mut t = Taps::default();
        t.start(1.0, at(100.0, 100.0));
        assert!(!t.armed);
        t.end(1.1, at(102.0, 101.0));
        t.start(1.3, at(110.0, 105.0));
        assert!(t.armed, "a second tap soon and near");
        t.end(2.0, at(300.0, 400.0));
        assert!(!t.armed);
        // too late, too far, or after a drag: not armed
        t.start(3.0, at(100.0, 100.0));
        t.end(3.1, at(100.0, 100.0));
        t.start(3.6, at(100.0, 100.0));
        assert!(!t.armed, "too late");
        t.end(3.7, at(100.0, 100.0));
        t.start(3.8, at(200.0, 100.0));
        assert!(!t.armed, "too far");
        t.end(3.85, at(200.0, 100.0));
        t.start(4.5, at(100.0, 100.0));
        t.end(4.6, at(150.0, 100.0));
        t.start(4.7, at(150.0, 100.0));
        assert!(!t.armed, "the first touch moved: no tap");
        // the touch that armed does not count as the first tap of another double tap
        let mut t = Taps::default();
        t.start(5.0, at(10.0, 10.0));
        t.end(5.1, at(10.0, 10.0));
        t.start(5.2, at(10.0, 10.0));
        t.end(5.3, at(10.0, 10.0));
        t.start(5.4, at(10.0, 10.0));
        assert!(!t.armed);
    }
}
