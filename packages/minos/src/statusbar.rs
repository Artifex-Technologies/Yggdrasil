//! The bottom bar, as Windows Explorer's: on the left "17 items" in the folder (hidden ones only when they are shown) and its
//! size, then "2 selected" and theirs; a status message for a few seconds; on the right the transfers (transfers.rs).
//!
//! The size: the files directly in the folder are summed from the listing at once; the folders in it are walked on a thread
//! of their own (foldersize.rs), "calculating…" until it is done, and the walk is dropped (stopped) when the folder changes.
//! A folder on a drive that may stall (network and cloud mounts) is never walked: its direct files only, said so.
//!
//! Everything is worked out when the listing, the folder or the selection changes, not on every frame.

use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eframe::egui::{self, vec2, Align, Layout, RichText, Sense, UiBuilder};
use minos_core::archive;
use minos_core::foldersize::{self, Counted, Counter, Direct};
use minos_core::listing::{self, Entry};
use minos_core::places;
use yggdrasil_ui::theme::Palette;

/// The bar's height in points.
pub const HEIGHT: f32 = 28.0;
/// How long a status message stays.
const MESSAGE_FOR: Duration = Duration::from_secs(8);
/// A folder whose listing keeps changing (a copy into it) is walked again at most this often.
const REWALK_AFTER: Duration = Duration::from_secs(2);

/// What the focused tab shows, borrowed for one frame.
pub struct View<'a> {
    pub dir: &'a Path,
    pub entries: &'a [Entry],
    /// Search results, when the tab shows those instead of the folder.
    pub results: Option<&'a [Entry]>,
    /// The quick filter typed in the search box.
    pub query: &'a str,
    pub hidden: bool,
    pub selected: &'a BTreeSet<PathBuf>,
    /// The listing is still on its way (a slow drive).
    pub loading: bool,
    /// Show the items and sizes (not in the open and save dialog).
    pub counts: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kind {
    /// A folder on a disk: its folders are walked.
    Folder,
    /// A folder on a drive that may stall: only its direct files are summed.
    Stalls,
    /// This PC, a disk's partitions, inside an archive, search results: what the listing says, nothing walked.
    Listing,
    Results,
    Drives,
}

/// The listing a summary was made from: when any of it changes, the summary is made again.
#[derive(Default, PartialEq, Eq)]
struct ListingKey {
    dir: PathBuf,
    /// Where the entries live and how many: a new listing is a new vector (made before the old one is dropped).
    entries: (usize, usize),
    results: Option<(usize, usize)>,
    hidden: bool,
    query: String,
}

impl ListingKey {
    fn matches(&self, v: &View) -> bool {
        self.dir == v.dir
            && self.entries == (v.entries.as_ptr() as usize, v.entries.len())
            && self.results == v.results.map(|r| (r.as_ptr() as usize, r.len()))
            && self.hidden == v.hidden
            && self.query == v.query
    }
}

/// The selection's sums.
#[derive(Default, Clone, Debug, PartialEq, Eq)]
struct SelSum {
    count: usize,
    bytes: u64,
    /// Folders in the selection whose size is not known yet.
    pending: bool,
    /// Folders in the selection that are not counted (a drive that may stall).
    files_only: bool,
}

#[derive(Default)]
pub struct StatusBar {
    key: ListingKey,
    kind: Option<Kind>,
    /// The listing's sums of what is shown.
    direct: Direct,
    /// The folders shown on the last walk, and whether each passes the filter.
    walk_dirs: Vec<PathBuf>,
    /// The walk for `walk_dirs`, and its result.
    counter: Option<Counter>,
    counted: Option<Counted>,
    /// What the walk was for (the folder and the hidden-items setting), and when it started.
    walk_for: Option<(PathBuf, bool)>,
    walk_started: Option<Instant>,
    /// The listing changed while a walk ran recently: walk again at this time.
    rewalk_at: Option<Instant>,
    /// The folder's total, once the walk is in.
    total: Option<u64>,
    /// The selection's fingerprint (how many, and their paths hashed together) and sums.
    sel_key: (usize, u64, u64),
    sel: SelSum,
    /// Bumped whenever the listing's summary or the walk's result changes, so the selection is summed again.
    generation: u64,
    message: String,
    message_at: Option<Instant>,
}

impl StatusBar {
    /// A status message, shown for a few seconds (an empty one changes nothing).
    pub fn say(&mut self, msg: String) {
        if !msg.is_empty() {
            self.message = msg;
            self.message_at = Some(Instant::now());
        }
    }

    /// A message is showing.
    pub fn has_message(&self) -> bool {
        self.message_at.is_some_and(|at| at.elapsed() < MESSAGE_FOR)
    }

    /// Brings the sums up to date for `v` (cheap when nothing changed).
    fn update(&mut self, ctx: &egui::Context, v: &View) {
        if !self.key.matches(v) {
            self.relist(ctx, v);
        }
        if let Some(c) = self.counter.as_ref().and_then(|c| c.poll()) {
            self.counter = None;
            self.counted = Some(c);
            self.sum_folders(v);
            self.generation += 1;
        }
        if let Some(at) = self.rewalk_at {
            let now = Instant::now();
            if now >= at {
                self.rewalk_at = None;
                self.walk(ctx, v);
            } else {
                ctx.request_repaint_after(at - now);
            }
        }
        let fp = fingerprint(v.selected);
        if (fp.0, fp.1, self.generation) != self.sel_key {
            self.sel_key = (fp.0, fp.1, self.generation);
            self.sel = self.sum_selection(v);
        }
    }

    /// The listing changed (another folder, a reload, the filter, hidden items): sum it again, and walk when needed.
    fn relist(&mut self, ctx: &egui::Context, v: &View) {
        let new_dir = self.key.dir != v.dir;
        if new_dir || self.kind.is_none() {
            self.kind = Some(if places::is_virtual(v.dir) {
                Kind::Drives
            } else if archive::split(v.dir).is_some() {
                Kind::Listing
            } else if places::may_stall(v.dir) {
                Kind::Stalls
            } else {
                Kind::Folder
            });
        }
        let kind = if v.results.is_some() { Kind::Results } else { self.kind.unwrap_or(Kind::Listing) };
        self.key.dir.clear();
        self.key.dir.push(v.dir);
        self.key.entries = (v.entries.as_ptr() as usize, v.entries.len());
        self.key.results = v.results.map(|r| (r.as_ptr() as usize, r.len()));
        self.key.hidden = v.hidden;
        self.key.query.clear();
        self.key.query.push_str(v.query);
        self.generation += 1;
        let q = v.query.to_lowercase();
        self.direct = match v.results {
            Some(r) => foldersize::direct(r, |_| true),
            None if kind == Kind::Drives => foldersize::direct(v.entries, |_| true),
            None => foldersize::direct(v.entries, |e| shown(e, v.hidden, &q)),
        };
        if kind != Kind::Folder {
            self.stop_walk();
            return;
        }
        let walk_for = (v.dir.to_path_buf(), v.hidden);
        if self.walk_for.as_ref() != Some(&walk_for) {
            // another folder, or hidden items shown or hidden: what was counted is for something else
            self.stop_walk();
            self.walk(ctx, v);
        } else if self.walk_started.is_some_and(|t| t.elapsed() < REWALK_AFTER) {
            // the listing keeps changing (a copy into it): keep what is shown, walk again shortly
            self.rewalk_at = Some(self.walk_started.unwrap_or_else(Instant::now) + REWALK_AFTER);
            ctx.request_repaint_after(REWALK_AFTER);
            self.sum_folders(v);
        } else {
            self.walk(ctx, v);
        }
    }

    fn stop_walk(&mut self) {
        self.counter = None;
        self.counted = None;
        self.walk_for = None;
        self.walk_started = None;
        self.rewalk_at = None;
        self.total = None;
        self.walk_dirs.clear();
    }

    /// Walks the folders shown (by the hidden-items setting; the filter only picks which totals are added up, so typing in
    /// it never starts a walk). Until it is in, the last walk's numbers stay (or "calculating…").
    fn walk(&mut self, ctx: &egui::Context, v: &View) {
        self.walk_dirs.clear();
        self.walk_dirs.extend(v.entries.iter().filter(|e| e.is_dir && (v.hidden || !e.hidden)).map(|e| e.path.clone()));
        self.walk_for = Some((v.dir.to_path_buf(), v.hidden));
        self.walk_started = Some(Instant::now());
        if self.walk_dirs.is_empty() {
            self.counter = None;
            self.counted = Some(Counted { bytes: vec![], partial: false });
            self.sum_folders(v);
            return;
        }
        let c = ctx.clone();
        // a new walk replaces (and so stops) the one before
        self.counter = Some(Counter::start(self.walk_dirs.clone(), move || c.request_repaint()));
        // the last numbers stay up only while they are for this same list of folders
        if self.counted.as_ref().is_some_and(|c| c.bytes.len() != self.walk_dirs.len()) {
            self.counted = None;
        }
        self.sum_folders(v);
    }

    /// The folder's total from the walk: the files shown plus the folders shown (the filter applies).
    fn sum_folders(&mut self, v: &View) {
        self.total = None;
        let Some(c) = &self.counted else { return };
        if c.bytes.len() != self.walk_dirs.len() {
            return;
        }
        let q = v.query.to_lowercase();
        let folders: u64 = self
            .walk_dirs
            .iter()
            .zip(&c.bytes)
            .filter(|(p, _)| q.is_empty() || p.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&q)))
            .map(|(_, b)| *b)
            .sum();
        self.total = Some(self.direct.file_bytes + folders);
    }

    fn sum_selection(&self, v: &View) -> SelSum {
        let mut s = SelSum::default();
        if v.selected.is_empty() {
            return s;
        }
        let list = v.results.unwrap_or(v.entries);
        let mut folders = false;
        for e in list.iter().filter(|e| v.selected.contains(&e.path)) {
            s.count += 1;
            if e.is_dir {
                folders = true;
            } else {
                s.bytes += e.size;
            }
        }
        if folders && self.kind == Some(Kind::Folder) && v.results.is_none() {
            match &self.counted {
                Some(c) if c.bytes.len() == self.walk_dirs.len() => {
                    s.bytes += self.walk_dirs.iter().zip(&c.bytes).filter(|(p, _)| v.selected.contains(*p)).map(|(_, b)| *b).sum::<u64>();
                }
                _ => s.pending = true,
            }
        } else if folders && self.kind != Some(Kind::Drives) {
            s.files_only = true;
        }
        s
    }

    /// The bar's left part and the message; `right` draws the right end (the transfers).
    pub fn show(&mut self, ui: &mut egui::Ui, pal: &Palette, v: &View, right: impl FnOnce(&mut egui::Ui)) {
        let ctx = ui.ctx().clone();
        if v.counts {
            self.update(&ctx, v);
        } else {
            self.stop_walk();
            self.kind = None;
            self.key = ListingKey::default();
        }
        let kind = if v.results.is_some() { Kind::Results } else { self.kind.unwrap_or(Kind::Listing) };
        let dim = |t: String| RichText::new(t).size(12.5).color(pal.dim);
        ui.add_space(6.0);
        if v.counts && v.loading {
            ui.label(dim("Reading the folder…".into()));
        } else if v.counts {
            let n = self.direct.items;
            let noun = if kind == Kind::Results { plural(n, "result", "results") } else { plural(n, "item", "items") };
            ui.label(dim(format!("{n} {noun}")));
            if let Some((size, tip)) = self.size_text(kind) {
                sep(ui, pal);
                let r = ui.label(dim(size));
                if let Some(tip) = tip {
                    r.on_hover_text(tip);
                }
            }
        }
        if v.counts && self.sel.count > 0 {
            sep(ui, pal);
            let mut t = format!("{} selected", self.sel.count);
            if self.sel.pending {
                t += " · calculating…";
            } else if self.sel.bytes > 0 || !self.sel.files_only {
                t += &format!(" · {}", listing::human_size(self.sel.bytes));
                if self.sel.files_only {
                    t += " in files";
                }
            }
            ui.label(dim(t));
        }
        let message = match self.message_at {
            Some(at) if at.elapsed() < MESSAGE_FOR => {
                ctx.request_repaint_after(MESSAGE_FOR - at.elapsed());
                Some(self.message.as_str())
            }
            _ => None,
        };
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(4.0);
            right(ui);
            if let Some(m) = message {
                let rest = ui.available_rect_before_wrap();
                ui.scope_builder(UiBuilder::new().max_rect(rest).layout(Layout::left_to_right(Align::Center)), |ui| {
                    sep(ui, pal);
                    ui.add(egui::Label::new(RichText::new(m).size(12.5).color(pal.text)).truncate()).on_hover_text(m);
                });
            }
        });
    }

    /// The folder's size as the bar says it, with a tooltip when it needs explaining.
    fn size_text(&self, kind: Kind) -> Option<(String, Option<&'static str>)> {
        let d = &self.direct;
        match kind {
            Kind::Drives => None,
            Kind::Results | Kind::Listing if d.file_bytes == 0 && d.folders > 0 => None,
            Kind::Results | Kind::Listing => {
                let tip = (d.folders > 0).then_some("Folders are not counted here");
                Some((listing::human_size(d.file_bytes), tip))
            }
            Kind::Stalls => Some((
                format!("{} in files", listing::human_size(d.file_bytes)),
                Some("This folder is on a network or cloud drive: only the files directly in it are counted, so the window never waits on the drive"),
            )),
            Kind::Folder if d.folders == 0 => Some((listing::human_size(d.file_bytes), None)),
            Kind::Folder => match (self.total, &self.counted) {
                (Some(t), Some(c)) if c.partial => Some((
                    format!("{}+", listing::human_size(t)),
                    Some("A network or cloud drive mounted inside is not counted"),
                )),
                (Some(t), _) => Some((listing::human_size(t), None)),
                _ => Some(("calculating…".into(), Some("Adding up what the folders hold"))),
            },
        }
    }
}

/// Whether the listing shows `e` (as the tab's list does: hidden items by the setting, the filter by name). `q` is lowercase.
fn shown(e: &Entry, hidden: bool, q: &str) -> bool {
    (hidden || !e.hidden) && (q.is_empty() || e.name.to_lowercase().contains(q))
}

/// How many are selected and their paths hashed together (order does not matter in a set, and is sorted anyway).
fn fingerprint(sel: &BTreeSet<PathBuf>) -> (usize, u64) {
    if sel.is_empty() {
        return (0, 0);
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in sel {
        p.hash(&mut h);
    }
    (sel.len(), h.finish())
}

fn plural(n: usize, one: &'static str, many: &'static str) -> &'static str {
    if n == 1 {
        one
    } else {
        many
    }
}

/// A thin upright line between the bar's parts.
fn sep(ui: &mut egui::Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(13.0, 14.0), Sense::hover());
    ui.painter().vline(r.center().x, r.y_range(), egui::Stroke::new(1.0, pal.border));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("minos-bar-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn wait_for(bar: &mut StatusBar, ctx: &egui::Context, v: &View) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while bar.counter.is_some() {
            bar.update(ctx, v);
            assert!(Instant::now() < deadline, "the walk did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn items_and_size_follow_hidden_items_and_the_walk() {
        let d = tmp("sum");
        std::fs::create_dir_all(d.join("docs/deep")).unwrap();
        std::fs::create_dir_all(d.join(".cache")).unwrap();
        std::fs::write(d.join("docs/deep/a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(d.join(".cache/c.bin"), vec![0u8; 500]).unwrap();
        std::fs::write(d.join("top.txt"), vec![0u8; 24]).unwrap();
        std::fs::write(d.join(".hidden"), vec![0u8; 6]).unwrap();
        let entries = listing::read_dir(&d).unwrap();
        let ctx = egui::Context::default();
        let none = BTreeSet::new();
        let mut bar = StatusBar::default();
        let v = View { dir: &d, entries: &entries, results: None, query: "", hidden: false, selected: &none, loading: false, counts: true };
        bar.update(&ctx, &v);
        assert_eq!(bar.kind, Some(Kind::Folder));
        assert_eq!(bar.direct, Direct { items: 2, folders: 1, file_bytes: 24 });
        assert_eq!(bar.size_text(Kind::Folder).unwrap().0, "calculating…");
        wait_for(&mut bar, &ctx, &v);
        assert_eq!(bar.total, Some(1024), "the hidden folder is not counted while hidden items are not shown");
        // hidden items shown: counted again, with them
        let v = View { hidden: true, ..v };
        bar.update(&ctx, &v);
        assert_eq!(bar.direct.items, 4);
        wait_for(&mut bar, &ctx, &v);
        assert_eq!(bar.total, Some(1530));
        // the filter only picks which totals add up: no new walk
        let v = View { query: "DOC", ..v };
        bar.update(&ctx, &v);
        assert!(bar.counter.is_none());
        assert_eq!((bar.direct.items, bar.total), (1, Some(1000)));
    }

    #[test]
    fn the_selection_is_summed_with_its_folders() {
        let d = tmp("sel");
        std::fs::create_dir_all(d.join("f")).unwrap();
        std::fs::write(d.join("f/x"), vec![0u8; 300]).unwrap();
        std::fs::write(d.join("a"), vec![0u8; 40]).unwrap();
        std::fs::write(d.join("b"), vec![0u8; 2]).unwrap();
        let entries = listing::read_dir(&d).unwrap();
        let ctx = egui::Context::default();
        let sel: BTreeSet<PathBuf> = [d.join("a")].into();
        let mut bar = StatusBar::default();
        let v = View { dir: &d, entries: &entries, results: None, query: "", hidden: false, selected: &sel, loading: false, counts: true };
        bar.update(&ctx, &v);
        assert_eq!(bar.sel, SelSum { count: 1, bytes: 40, pending: false, files_only: false });
        let sel2: BTreeSet<PathBuf> = [d.join("a"), d.join("f")].into();
        let v = View { selected: &sel2, ..v };
        bar.update(&ctx, &v);
        wait_for(&mut bar, &ctx, &v);
        bar.update(&ctx, &v);
        assert_eq!(bar.sel, SelSum { count: 2, bytes: 340, pending: false, files_only: false });
    }

    #[test]
    fn another_folder_stops_the_walk_of_the_last() {
        let d = tmp("cancel");
        std::fs::create_dir_all(d.join("one/sub")).unwrap();
        std::fs::create_dir_all(d.join("two")).unwrap();
        let one = listing::read_dir(&d.join("one")).unwrap();
        let two = listing::read_dir(&d.join("two")).unwrap();
        let ctx = egui::Context::default();
        let none = BTreeSet::new();
        let mut bar = StatusBar::default();
        let a = d.join("one");
        let v = View { dir: &a, entries: &one, results: None, query: "", hidden: false, selected: &none, loading: false, counts: true };
        bar.update(&ctx, &v);
        assert!(bar.counter.is_some(), "walking one/sub");
        let b = d.join("two");
        let v2 = View { dir: &b, entries: &two, ..v };
        bar.update(&ctx, &v2);
        // two has no folders: nothing walks, and one's walk was dropped (Counter's drop stops its thread)
        assert!(bar.counter.is_none());
        assert_eq!(bar.total, Some(0));
        assert_eq!(bar.walk_for, Some((b.clone(), false)));
    }

    #[test]
    fn listings_that_are_not_walked() {
        let ctx = egui::Context::default();
        let none = BTreeSet::new();
        let e = |name: &str, dir: bool, size: u64| Entry {
            name: name.into(),
            path: PathBuf::from("/x").join(name),
            is_dir: dir,
            size,
            modified: None,
            hidden: false,
            usage: None,
        };
        let results = vec![e("a", false, 5), e("b", true, 0)];
        let mut bar = StatusBar::default();
        let d = tmp("results");
        let v = View {
            dir: &d,
            entries: &[],
            results: Some(&results),
            query: "",
            hidden: false,
            selected: &none,
            loading: false,
            counts: true,
        };
        bar.update(&ctx, &v);
        assert!(bar.counter.is_none(), "search results are not walked");
        assert_eq!(bar.size_text(Kind::Results).unwrap().0, listing::human_size(5));
        // a folder on a drive that may stall: the direct files only, said so (the rule itself: foldersize's tests)
        bar.kind = Some(Kind::Stalls);
        let (text, tip) = bar.size_text(Kind::Stalls).unwrap();
        assert!(text.ends_with("in files") && tip.is_some());
    }

    #[test]
    fn a_message_shows_for_a_while() {
        let mut bar = StatusBar::default();
        bar.say(String::new());
        assert!(bar.message_at.is_none(), "an empty message changes nothing");
        bar.say("Folder created".into());
        assert_eq!(bar.message, "Folder created");
        assert!(bar.message_at.is_some_and(|t| t.elapsed() < MESSAGE_FOR));
    }
}
