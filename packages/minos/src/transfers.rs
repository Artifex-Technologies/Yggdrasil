//! Copies and moves, shown the way Proton Drive shows its transfers. One runs at a time; the ones asked for meanwhile wait
//! their turn (and can be cancelled before they start). Expanded, a panel above the bottom bar on the right: a title, one
//! progress bar for everything, "477 of 1515 transfers completed", and the files the worker has reached, each with a mark and
//! what became of it (failures first). Collapsed, only a slim bar and a line in the bottom bar's right end. The question about
//! a file that already exists stays a dialog in the middle of the window.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Rect, RichText, Sense};
use minos_core::fileops::{self, FileEntry, FileState, Progress};
use minos_core::listing;
use minos_core::queue::{Phase, Queue, Slot};
use yggdrasil_ui::theme::Palette;

use crate::icons::{self, Icon};

/// What a transfer did: (source, where it went) for each item.
pub type Done = Vec<(PathBuf, PathBuf)>;

enum TEvent {
    Conflict(PathBuf, PathBuf, Sender<Option<fileops::Decision>>),
    Done { stats: fileops::Stats, done: Done, stopped: bool },
}

/// A transfer that has just ended, for the window to act on (moved items' tags follow them, the folders are read again).
pub struct Finished {
    pub stats: fileops::Stats,
    pub done: Done,
    pub mv: bool,
    pub stopped: bool,
    pub dest: PathBuf,
}

/// One copy or move: waiting, running on its own thread, or ended.
pub struct Job {
    what: String,
    mv: bool,
    /// What to bring (taken by the worker when it starts).
    paths: Vec<PathBuf>,
    dest: PathBuf,
    prog: Arc<Progress>,
    rx: Option<Receiver<TEvent>>,
    started: Option<Instant>,
    /// Recent (seconds, bytes done) readings, for the speed.
    samples: VecDeque<(f64, u64)>,
    /// Bytes and files a second, from the readings.
    speed: f64,
    fspeed: f64,
    /// The file the thread is waiting for an answer about: what is coming, what is there, where to send the answer.
    ask: Option<(PathBuf, PathBuf, Sender<Option<fileops::Decision>>)>,
    /// "Do this for all": the answer given to every later question.
    sticky: Option<fileops::Decision>,
    all: bool,
    /// It ended early: Cancel, or Stop in the question.
    stopped: bool,
}

impl Job {
    fn new(paths: Vec<PathBuf>, dest: PathBuf, mv: bool) -> Self {
        let n = paths.len();
        let what = format!(
            "{} {} to {}",
            if mv { "Moving" } else { "Copying" },
            if n == 1 { paths[0].file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default() } else { format!("{n} items") },
            dest.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| dest.display().to_string())
        );
        Self {
            what,
            mv,
            paths,
            dest,
            prog: Arc::new(Progress::default()),
            rx: None,
            started: None,
            samples: VecDeque::new(),
            speed: 0.0,
            fspeed: 0.0,
            ask: None,
            sticky: None,
            all: false,
            stopped: false,
        }
    }

    /// The worker and the counting thread start.
    fn start(&mut self, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let c = ctx.clone();
        let paths = std::mem::take(&mut self.paths);
        {
            let (srcs, p) = (paths.clone(), self.prog.clone());
            std::thread::spawn(move || fileops::scan(&srcs, &p));
        }
        let (wp, dest, mv) = (self.prog.clone(), self.dest.clone(), self.mv);
        std::thread::spawn(move || {
            let mut stats = fileops::Stats::default();
            let mut done = vec![];
            let mut stopped = false;
            for src in &paths {
                let res = fileops::transfer(
                    src,
                    &dest,
                    mv,
                    &mut |a, b| {
                        let (rt, rr) = channel();
                        tx.send(TEvent::Conflict(a.to_path_buf(), b.to_path_buf(), rt)).ok()?;
                        c.request_repaint();
                        rr.recv().ok().flatten()
                    },
                    &mut stats,
                    &wp,
                );
                match res {
                    Ok(new) => done.push((src.clone(), new)),
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {
                        stopped = true;
                        break;
                    }
                    Err(e) => stats.errors.push(format!("{}: {e}", src.display())),
                }
            }
            let _ = tx.send(TEvent::Done { stats, done, stopped });
            c.request_repaint();
        });
        self.rx = Some(rx);
        self.started = Some(Instant::now());
    }

    /// Cancel: the worker stops at the next piece (a half-written file is removed) and a pending question is answered Stop.
    fn cancel(&mut self) {
        self.prog.cancel.store(true, Relaxed);
        if let Some((_, _, reply)) = self.ask.take() {
            let _ = reply.send(None);
        }
    }

    /// The speed from the readings of the last four seconds.
    fn sample(&mut self) {
        let Some(t) = self.started else { return };
        let now = t.elapsed().as_secs_f64();
        let done = self.prog.done_bytes.load(Relaxed);
        self.samples.push_back((now, done));
        while self.samples.len() > 2 && now - self.samples[0].0 > 4.0 {
            self.samples.pop_front();
        }
        let (t0, b0) = self.samples[0];
        self.speed = if now - t0 > 0.4 { done.saturating_sub(b0) as f64 / (now - t0) } else { 0.0 };
        self.fspeed = self.prog.done_files.load(Relaxed) as f64 / now.max(0.5);
    }
}

/// Every transfer's numbers together, for the title, the bar and the bottom bar's line.
#[derive(Default)]
struct Totals {
    files_done: u64,
    files_total: u64,
    bytes_done: u64,
    bytes_total: u64,
    /// Still counting what the running one has to do.
    scanning: bool,
    failures: u64,
    queued: usize,
    /// The running one: a move, its speed, and its own bytes left.
    running: Option<(bool, f64, u64)>,
    stopped: bool,
}

impl Totals {
    /// How much of everything started is done, 0 to 1 (finished ones count as whole).
    fn frac(&self) -> f32 {
        if self.running.is_none() {
            return 1.0;
        }
        if self.bytes_total == 0 {
            return 0.0;
        }
        (self.bytes_done as f64 / self.bytes_total as f64).clamp(0.0, 1.0) as f32
    }

    /// "about 3 min left", once the totals are counted and the speed is known.
    fn left(&self) -> Option<String> {
        let (_, speed, left) = self.running?;
        if self.scanning || speed <= 1.0 || left == 0 {
            return None;
        }
        Some(format!("about {} left", duration((left as f64 / speed) as u64)))
    }
}

/// "3 h 12 min", "4 min", "12 s" (as the copy window always said it).
pub fn duration(secs: u64) -> String {
    if secs >= 3600 {
        format!("{} h {} min", secs / 3600, secs % 3600 / 60)
    } else if secs >= 60 {
        format!("{} min", secs / 60 + 1)
    } else {
        format!("{secs} s")
    }
}

/// How tall one line of the panel's list is.
const ROW_H: f32 = 44.0;
const PANEL_W: f32 = 440.0;

/// The transfers, their panel and their part of the bottom bar.
pub struct Transfers {
    q: Queue<Job>,
    /// The panel is open; collapsed, only the bottom bar's line shows.
    pub expanded: bool,
}

impl Default for Transfers {
    fn default() -> Self {
        // the first transfer opens the panel, as Proton Drive's does; after that it stays as it was left
        Self { q: Queue::default(), expanded: true }
    }
}

impl Transfers {
    /// A copy or move of `paths` into `dest`, merging with what is there. It starts at once when nothing else runs, else it
    /// waits its turn. How many are before it.
    pub fn push(&mut self, ctx: &egui::Context, paths: Vec<PathBuf>, dest: PathBuf, mv: bool) -> usize {
        let ahead = self.q.iter().filter(|s| s.phase != Phase::Finished).count();
        self.q.push(Job::new(paths, dest, mv));
        ctx.request_repaint();
        ahead
    }

    pub fn is_empty(&self) -> bool {
        self.q.is_empty()
    }

    /// Takes what the running transfer reports, shows the "already exists" dialog while it waits for an answer, and starts
    /// the next one when it ends. The one that ended, if one did.
    pub fn poll(&mut self, ctx: &egui::Context) -> Option<Finished> {
        let mut out = None;
        let mut ended = None;
        if let Some(Slot { id, job, .. }) = self.q.running_mut() {
            let mut finished: Option<(fileops::Stats, Done, bool)> = None;
            if let Some(rx) = &job.rx {
                while let Ok(ev) = rx.try_recv() {
                    match ev {
                        TEvent::Conflict(a, b, reply) => match job.sticky {
                            Some(d) => {
                                let _ = reply.send(Some(d));
                            }
                            None => job.ask = Some((a, b, reply)),
                        },
                        TEvent::Done { stats, done, stopped } => finished = Some((stats, done, stopped)),
                    }
                }
            }
            match finished {
                Some((stats, done, stopped)) => {
                    job.stopped = stopped;
                    job.rx = None;
                    job.ask = None;
                    job.speed = 0.0;
                    ended = Some(*id);
                    out = Some(Finished { stats, done, mv: job.mv, stopped, dest: job.dest.clone() });
                }
                None => {
                    ask_dialog(ctx, job);
                    if job.rx.is_some() {
                        job.sample();
                    }
                }
            }
        }
        if let Some(id) = ended {
            self.q.finish(id);
        }
        if let Some(slot) = self.q.start_next() {
            slot.job.start(ctx);
        }
        if self.q.busy() {
            ctx.request_repaint_after(Duration::from_millis(250));
        }
        out
    }

    /// The numbers shown: while one runs, its own (as "Copying 477 of 1515" says); when none runs, all the finished ones
    /// together. The failures are always all of them.
    fn totals(&self) -> Totals {
        let mut t = Totals { queued: self.q.queued(), ..Default::default() };
        let running = self.q.running().map(|s| s.id);
        for s in self.q.iter() {
            if s.phase == Phase::Queued {
                continue;
            }
            let p = &s.job.prog;
            if let Ok(log) = p.log.lock() {
                t.failures += log.failures;
            }
            t.stopped |= s.job.stopped;
            if running.is_some_and(|id| id != s.id) {
                continue;
            }
            let (fd, ft, bd, bt) =
                (p.done_files.load(Relaxed), p.total_files.load(Relaxed), p.done_bytes.load(Relaxed), p.total_bytes.load(Relaxed));
            t.files_done += fd;
            t.files_total += ft;
            t.bytes_total += bt;
            if s.phase == Phase::Running {
                t.bytes_done += bd;
                t.scanning = p.scanning.load(Relaxed);
                t.running = Some((s.job.mv, s.job.speed, bt.saturating_sub(bd)));
            } else {
                t.bytes_done += bt;
            }
        }
        t
    }

    /// The panel's title: what runs, else how it all ended.
    fn title(&self, t: &Totals) -> String {
        if let Some(s) = self.q.running() {
            return s.job.what.clone();
        }
        if t.failures > 0 {
            "Some items failed".into()
        } else if t.stopped {
            "Stopped".into()
        } else if self.q.queued() > 0 {
            "Waiting".into()
        } else {
            "All transfers done".into()
        }
    }

    /// The bottom bar's right end: a slim bar, "Copying 477 of 1515 · 2.1 GB of 3.4 GB · about 3 min left", the failures in
    /// red, and a chevron; a click opens or closes the panel. Drawn right to left (the caller's layout).
    pub fn bar(&mut self, ui: &mut egui::Ui, pal: &Palette) {
        if self.q.is_empty() {
            return;
        }
        let t = self.totals();
        let busy = self.q.busy();
        if !busy && small_button(ui, pal, Icon::Close, "Clear the finished transfers").clicked() {
            self.q.dismiss_finished();
            return;
        }
        let chev = if self.expanded { Icon::ChevronDown } else { Icon::ChevronUp };
        let tip = if self.expanded { "Hide the transfers" } else { "Show the transfers" };
        let mut toggle = small_button(ui, pal, chev, tip).clicked();
        if t.failures > 0 {
            let r = ui.add(egui::Label::new(RichText::new(format!("{} failed", t.failures)).color(pal.red).strong()).sense(Sense::click()));
            toggle |= r.on_hover_text("Show which files failed and why").clicked();
        }
        let line = match t.running {
            Some((mv, ..)) if t.bytes_total == 0 && t.scanning => {
                format!("{} · counting files…", if mv { "Moving" } else { "Copying" })
            }
            Some((mv, ..)) => {
                let mut s = format!(
                    "{} {} of {}{} · {} of {}",
                    if mv { "Moving" } else { "Copying" },
                    t.files_done,
                    t.files_total,
                    if t.scanning { "+" } else { "" },
                    listing::human_size(t.bytes_done),
                    listing::human_size(t.bytes_total),
                );
                if let Some(left) = t.left() {
                    s += " · ";
                    s += &left;
                }
                s
            }
            None => format!("{} · {} of {} completed", self.title(&t), t.files_done, t.files_total),
        };
        let line = if t.queued > 0 { format!("{line} · {} waiting", t.queued) } else { line };
        let r = ui.add(egui::Label::new(RichText::new(line).color(pal.text)).sense(Sense::click()).truncate());
        toggle |= r.clicked();
        let (rect, r) = ui.allocate_exact_size(vec2(120.0, 14.0), Sense::click());
        slim_bar(ui.painter(), Rect::from_center_size(rect.center(), vec2(120.0, 4.0)), t.frac(), bar_colour(pal, &t), pal);
        toggle |= r.clicked();
        if toggle {
            self.expanded = !self.expanded;
        }
    }

    /// The expanded panel, anchored bottom right, `above` points over the window's bottom edge (the bottom bar's height). A
    /// folder to open when a failed file's "Open its folder" was chosen.
    pub fn panel(&mut self, ctx: &egui::Context, pal: &Palette, above: f32) -> Option<PathBuf> {
        if !self.expanded || self.q.is_empty() {
            return None;
        }
        let t = self.totals();
        let title = self.title(&t);
        let busy = self.q.busy();
        let several = self.q.len() > 1;
        let mut open: Option<PathBuf> = None;
        let mut collapse = false;
        let mut clear = false;
        let mut cancel_running = false;
        let mut cancel_queued: Option<u64> = None;
        let mut dismiss: Option<u64> = None;
        egui::Area::new(egui::Id::new("minos-transfers"))
            .order(egui::Order::Foreground)
            .anchor(Align2::RIGHT_BOTTOM, vec2(-12.0, -(above + 8.0)))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(pal.sidebar)
                    .stroke(egui::Stroke::new(1.0, pal.border))
                    .corner_radius(CornerRadius::same(10))
                    .shadow(ui.style().visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.set_width(PANEL_W);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // the head: title, how many done, the bar
                        egui::Frame::new().inner_margin(egui::Margin { left: 16, right: 10, top: 12, bottom: 12 }).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.vertical(|ui| {
                                    ui.add(egui::Label::new(RichText::new(&title).strong().size(15.0).color(pal.text)).truncate());
                                    ui.add_space(3.0);
                                    let mut sub = format!("{} of {} transfers completed", t.files_done, t.files_total);
                                    if t.scanning {
                                        sub += " (still counting)";
                                    }
                                    if t.queued > 0 {
                                        sub += &format!(" · {} waiting", t.queued);
                                    }
                                    ui.label(RichText::new(sub).size(12.0).color(pal.dim));
                                });
                                ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
                                    if !busy && small_button(ui, pal, Icon::Close, "Clear the finished transfers").clicked() {
                                        clear = true;
                                    }
                                    if small_button(ui, pal, Icon::ChevronDown, "Hide (the bottom bar keeps the progress)").clicked() {
                                        collapse = true;
                                    }
                                    if self.q.running().is_some() && ui.small_button("Cancel").clicked() {
                                        cancel_running = true;
                                    }
                                });
                            });
                            if let Some(s) = self.q.running() {
                                ui.add_space(6.0);
                                let mut line = format!(
                                    "{} of {}{}  ·  {}/s",
                                    listing::human_size(t.bytes_done),
                                    listing::human_size(t.bytes_total),
                                    if t.scanning { " so far" } else { "" },
                                    listing::human_size(s.job.speed as u64)
                                );
                                if s.job.fspeed > 0.0 {
                                    line += &format!("  ·  {:.1} files/s", s.job.fspeed);
                                }
                                if let Some(left) = t.left() {
                                    line += "  ·  ";
                                    line += &left;
                                }
                                ui.label(RichText::new(line).size(12.0).color(pal.dim));
                            }
                            ui.add_space(10.0);
                            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width() - 6.0, 6.0), Sense::hover());
                            slim_bar(ui.painter(), rect, t.frac(), bar_colour(pal, &t), pal);
                        });
                        let (line, _) = ui.allocate_exact_size(vec2(PANEL_W, 1.0), Sense::hover());
                        ui.painter().rect_filled(line, 0.0, pal.border);
                        // the list: a line per job when there are several, and each job's files
                        let header = usize::from(several);
                        let total: usize = self.q.listed().map(|s| header + s.job.prog.log.lock().map(|l| l.len()).unwrap_or(0)).sum();
                        if total == 0 {
                            egui::Frame::new().inner_margin(egui::Margin::symmetric(16, 12)).show(ui, |ui| {
                                ui.label(RichText::new("Counting what to copy…").size(12.0).color(pal.dim));
                            });
                            return;
                        }
                        egui::ScrollArea::vertical().max_height(330.0).auto_shrink([false, true]).show_rows(
                            ui,
                            ROW_H,
                            total,
                            |ui, range| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                let mut at = 0usize;
                                for s in self.q.listed() {
                                    let Ok(log) = s.job.prog.log.lock() else { continue };
                                    let n = header + log.len();
                                    if at + n <= range.start {
                                        at += n;
                                        continue;
                                    }
                                    if at >= range.end {
                                        break;
                                    }
                                    for i in range.start.max(at)..range.end.min(at + n) {
                                        let k = i - at;
                                        if several && k == 0 {
                                            match job_row(ui, pal, s, &log) {
                                                Some(RowAct::Cancel) if s.phase == Phase::Running => cancel_running = true,
                                                Some(RowAct::Cancel) => cancel_queued = Some(s.id),
                                                Some(RowAct::Dismiss) => dismiss = Some(s.id),
                                                None => {}
                                            }
                                        } else if let Some(e) = log.row(k - header) {
                                            if let Some(p) = file_row(ui, pal, e) {
                                                open = Some(p);
                                            }
                                        } else {
                                            ui.allocate_exact_size(vec2(PANEL_W, ROW_H), Sense::hover());
                                        }
                                    }
                                    at += n;
                                }
                            },
                        );
                    });
            });
        if cancel_running {
            if let Some(s) = self.q.running_mut() {
                s.job.cancel();
            }
        }
        if let Some(id) = cancel_queued {
            self.q.cancel_queued(id);
        }
        if let Some(id) = dismiss {
            self.q.dismiss(id);
        }
        if clear {
            self.q.dismiss_finished();
        }
        if collapse {
            self.expanded = false;
        }
        open
    }

    /// Made-up transfers for the screenshots (`minos FOLDER --demo-transfers[=collapsed|=failed]`, debug builds only): one
    /// copying, one waiting, one ended with failures (only that one with `failed`), the panel open unless `collapsed`.
    #[cfg(debug_assertions)]
    pub fn demo(&mut self, mode: &str) {
        self.expanded = mode != "collapsed";
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home/alex"));
        let pics = home.join("Pictures/Holiday 2025");
        let cloud = home.join("Proton Drive/Photos");
        let entry = |name: &str, size: u64, done: u64, state: FileState| FileEntry {
            name: name.into(),
            src: pics.join(name),
            dst: cloud.join("Holiday 2025").join(name),
            is_dir: false,
            size,
            done,
            state,
        };
        // ended: two failed
        let id = self.q.push(Job::new(vec![home.join("Documents/Recipes.md")], home.join("Backup"), false));
        {
            let job = &self.q.get(id).expect("just pushed").job;
            let p = &job.prog;
            p.total_files.store(14, Relaxed);
            p.done_files.store(12, Relaxed);
            p.total_bytes.store(48_000_000, Relaxed);
            p.done_bytes.store(41_000_000, Relaxed);
            let mut log = p.log.lock().expect("demo log");
            log.failures = 2;
            log.failed.push(entry("Budget 2026.ods", 52_000, 0, FileState::Failed("Permission denied".into())));
            log.failed.push(entry(".~lock.Budget 2026.ods#", 90, 0, FileState::Failed("File name too long".into())));
            log.recent.push_back(entry("Trip plan.md", 3_100, 3_100, FileState::Done));
        }
        self.q.start_next();
        self.q.finish(id);
        if mode == "failed" {
            return;
        }
        // running
        let id = self.q.push(Job::new(vec![pics.clone()], cloud.clone(), false));
        self.q.start_next();
        if let Some(s) = self.q.running_mut() {
            debug_assert_eq!(s.id, id);
            let job = &mut s.job;
            job.what = "Copying 1515 items to Photos".into();
            job.speed = 7_400_000.0;
            job.fspeed = 3.2;
            job.started = Some(Instant::now());
            let p = &job.prog;
            p.total_files.store(1515, Relaxed);
            p.done_files.store(477, Relaxed);
            p.total_bytes.store(3_400_000_000, Relaxed);
            p.done_bytes.store(2_100_000_000, Relaxed);
            let mut log = p.log.lock().expect("demo log");
            log.failures = 1;
            log.failed.push(entry("IMG_0412.HEIC", 2_900_000, 0, FileState::Failed("No space left on device".into())));
            for i in 0..20u64 {
                let name = format!("IMG_{:04}.jpg", 450 + i);
                let state = if i % 7 == 3 { FileState::Same } else { FileState::Done };
                log.recent.push_back(entry(&name, 4_200_000 + i * 31_000, 4_200_000 + i * 31_000, state));
            }
            log.recent.push_back(entry("VID_0471.mp4", 640_000_000, 410_000_000, FileState::Copying));
        }
        // waiting
        self.q.push(Job::new(vec![home.join("Music")], home.join("Backup"), false));
    }
}

enum RowAct {
    Cancel,
    Dismiss,
}

/// A job's own line (when several are listed): its mark, what it is, how it stands, Cancel or Clear.
fn job_row(ui: &mut egui::Ui, pal: &Palette, s: &Slot<Job>, log: &fileops::FileLog) -> Option<RowAct> {
    let (rect, _) = ui.allocate_exact_size(vec2(PANEL_W, ROW_H), Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 0.0, pal.strip);
    let (icon, col, state) = match s.phase {
        Phase::Queued => (Icon::Waiting, pal.dim, "Waiting its turn".to_string()),
        Phase::Running => (Icon::Waiting, pal.accent, "Running".to_string()),
        Phase::Finished if log.failures > 0 => (Icon::Failed, pal.red, format!("{} failed", log.failures)),
        Phase::Finished if s.job.stopped => (Icon::Skipped, pal.amber, "Stopped".to_string()),
        Phase::Finished => (Icon::Tick, pal.green, "Done".to_string()),
    };
    let ir = Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(18.0, 18.0));
    if s.phase == Phase::Running {
        let pr = &s.job.prog;
        let (d, t) = (pr.done_bytes.load(Relaxed), pr.total_bytes.load(Relaxed));
        icons::paint_ring(&p, ir, if t > 0 { d as f32 / t as f32 } else { 0.0 }, col, pal.border);
    } else {
        icons::paint(&p, ir, icon, col);
    }
    let x = rect.left() + 46.0;
    text_line(&p, pos2(x, rect.top() + 7.0), rect.right() - 90.0 - x, &s.job.what, 13.0, pal.text);
    let failed = s.phase == Phase::Finished && log.failures > 0;
    text_line(&p, pos2(x, rect.top() + 25.0), rect.right() - 90.0 - x, &state, 11.0, if failed { pal.red } else { pal.dim });
    let br = Rect::from_min_size(pos2(rect.right() - 78.0, rect.center().y - 11.0), vec2(66.0, 22.0));
    let mut act = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(br).layout(Layout::right_to_left(Align::Center)));
    match s.phase {
        Phase::Queued | Phase::Running => {
            if child.small_button("Cancel").clicked() {
                act = Some(RowAct::Cancel);
            }
        }
        Phase::Finished => {
            if small_button(&mut child, pal, Icon::Close, "Clear this one").clicked() {
                act = Some(RowAct::Dismiss);
            }
        }
    }
    act
}

/// A file's line: its mark, its name, and what became of it in small text (a failure's reason in red). Right-click on a
/// failure: copy the error, or open its folder. The folder to open, when chosen.
fn file_row(ui: &mut egui::Ui, pal: &Palette, e: &FileEntry) -> Option<PathBuf> {
    let failed = matches!(e.state, FileState::Failed(_));
    let (rect, r) = ui.allocate_exact_size(vec2(PANEL_W, ROW_H), if failed { Sense::click() } else { Sense::hover() });
    let p = ui.painter_at(rect);
    if r.hovered() {
        p.rect_filled(rect, 0.0, pal.hover);
    }
    let ir = Rect::from_center_size(pos2(rect.left() + 26.0, rect.center().y), vec2(18.0, 18.0));
    let reason_col = match &e.state {
        FileState::Copying => {
            let f = if e.size > 0 { e.done as f32 / e.size as f32 } else { 0.0 };
            icons::paint_ring(&p, ir, f, pal.accent, pal.border);
            pal.dim
        }
        FileState::Done | FileState::Replaced | FileState::KeptBoth => {
            icons::paint(&p, ir, Icon::Tick, pal.green);
            pal.dim
        }
        FileState::Skipped | FileState::Same => {
            icons::paint(&p, ir, Icon::Skipped, pal.dim);
            pal.dim
        }
        FileState::Stopped => {
            icons::paint(&p, ir, Icon::Skipped, pal.amber);
            pal.amber
        }
        FileState::Failed(_) => {
            icons::paint(&p, ir, Icon::Failed, pal.red);
            pal.red
        }
    };
    let x = rect.left() + 46.0;
    let w = rect.right() - 14.0 - x;
    text_line(&p, pos2(x, rect.top() + 6.0), w, &e.name, 13.0, pal.text);
    if e.state == FileState::Copying {
        let s = format!("{} of {}", listing::human_size(e.done), listing::human_size(e.size));
        text_line(&p, pos2(x, rect.top() + 25.0), w, &s, 11.0, reason_col);
    } else {
        text_line(&p, pos2(x, rect.top() + 25.0), w, e.state.reason(), 11.0, reason_col);
    }
    let mut open = None;
    if failed {
        let r = r.on_hover_text(e.src.display().to_string());
        r.context_menu(|ui| {
            if ui.button("Copy the error").clicked() {
                ui.ctx().copy_text(format!("{}: {}", e.src.display(), e.state.reason()));
                ui.close();
            }
            if let Some(dir) = e.src.parent().filter(|d| d.is_dir()) {
                if ui.button("Open its folder").clicked() {
                    open = Some(dir.to_path_buf());
                    ui.close();
                }
            }
            if let Some(dir) = e.dst.parent().filter(|d| d.is_dir() && Some(*d) != e.src.parent()) {
                if ui.button("Open the folder it was going to").clicked() {
                    open = Some(dir.to_path_buf());
                    ui.close();
                }
            }
        });
    }
    open
}

/// One line of text from `at`, cut with "…" to `width`.
fn text_line(p: &egui::Painter, at: egui::Pos2, width: f32, text: &str, size: f32, color: Color32) {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::text::TextFormat { font_id: FontId::proportional(size), color, ..Default::default() },
    );
    job.wrap = egui::text::TextWrapping { max_width: width.max(10.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    p.galley(at, p.layout_job(job), color);
}

/// The bar's colour: the accent while it runs, red when something failed, green when all went well.
fn bar_colour(pal: &Palette, t: &Totals) -> Color32 {
    if t.running.is_some() {
        pal.accent
    } else if t.failures > 0 {
        pal.red
    } else if t.stopped {
        pal.amber
    } else {
        pal.green
    }
}

fn slim_bar(p: &egui::Painter, rect: Rect, frac: f32, color: Color32, pal: &Palette) {
    let r = CornerRadius::same((rect.height() / 2.0) as u8);
    p.rect_filled(rect, r, pal.border);
    if frac > 0.0 {
        let mut fill = rect;
        fill.set_width((rect.width() * frac).max(rect.height()));
        p.rect_filled(fill, r, color);
    }
}

/// A small square button with one of Minos's icons.
fn small_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter().rect_filled(rect, CornerRadius::same(6), pal.hover);
    }
    icons::paint(ui.painter(), rect.shrink(5.0), icon, pal.text);
    resp.on_hover_text(tip)
}

/// The "already exists" question for the file the worker is waiting on (the dialog as it always was).
fn ask_dialog(ctx: &egui::Context, job: &mut Job) {
    let Some((a, b, _)) = job.ask.clone() else { return };
    let mut answer: Option<Option<fileops::Decision>> = None;
    let info = |p: &Path| match std::fs::metadata(p) {
        Ok(m) => format!("{}, {}", listing::human_size(m.len()), m.modified().ok().map(crate::fmt_time).unwrap_or_default()),
        Err(_) => String::new(),
    };
    let name = b.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let folder = b.parent().map(|p| p.display().to_string()).unwrap_or_default();
    let mut all = job.all;
    egui::Window::new("File already exists").collapsible(false).resizable(false).anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0)).show(
        ctx,
        |ui| {
            ui.label(RichText::new(&name).strong());
            ui.label(format!("is already in {folder}"));
            ui.add_space(6.0);
            ui.label(format!("Already there:  {}", info(&b)));
            ui.label(format!("Coming in:  {}", info(&a)));
            ui.add_space(6.0);
            ui.checkbox(&mut all, "Do this for all remaining files that already exist");
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui.button("Replace").clicked() {
                    answer = Some(Some(fileops::Decision::Replace));
                }
                if ui.button("Skip").clicked() {
                    answer = Some(Some(fileops::Decision::Skip));
                }
                if ui.button("Keep both").clicked() {
                    answer = Some(Some(fileops::Decision::KeepBoth));
                }
                if ui.button("Stop").clicked() {
                    answer = Some(None);
                }
            });
        },
    );
    job.all = all;
    if let Some(ans) = answer {
        if let Some((_, _, reply)) = job.ask.take() {
            let _ = reply.send(ans);
        }
        if job.all {
            job.sticky = ans;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("minos-tq-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// Polls until the queue is idle, collecting what ended.
    fn run_all(t: &mut Transfers, ctx: &egui::Context) -> Vec<Finished> {
        let mut out = vec![];
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(f) = t.poll(ctx) {
                out.push(f);
            }
            if !t.q.busy() {
                return out;
            }
            assert!(Instant::now() < deadline, "the transfers did not end");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_second_transfer_waits_its_turn_instead_of_being_refused() {
        let d = tmp("queue");
        std::fs::create_dir_all(d.join("a")).unwrap();
        std::fs::create_dir_all(d.join("dest")).unwrap();
        std::fs::write(d.join("a/one.txt"), "1").unwrap();
        std::fs::write(d.join("two.txt"), "2").unwrap();
        let ctx = egui::Context::default();
        let mut t = Transfers::default();
        assert_eq!(t.push(&ctx, vec![d.join("a")], d.join("dest"), false), 0);
        assert_eq!(t.push(&ctx, vec![d.join("two.txt")], d.join("dest"), false), 1, "one ahead of it");
        // a third, cancelled while it waits: it never runs
        t.push(&ctx, vec![d.join("two.txt")], d.join("a"), false);
        t.poll(&ctx); // the first starts
        assert_eq!(t.q.queued(), 2);
        let third = t.q.iter().last().unwrap().id;
        assert!(t.q.cancel_queued(third).is_some());
        let ended = run_all(&mut t, &ctx);
        assert_eq!(ended.len(), 2);
        assert!(d.join("dest/a/one.txt").exists() && d.join("dest/two.txt").exists());
        assert!(!d.join("a/two.txt").exists(), "the cancelled one never ran");
        // both stay listed (finished) until cleared
        assert_eq!(t.q.len(), 2);
        let tot = t.totals();
        assert_eq!((tot.files_done, tot.files_total, tot.failures), (2, 2, 0));
        assert_eq!(tot.frac(), 1.0);
        t.q.dismiss_finished();
        assert!(t.q.is_empty());
    }

    #[test]
    fn a_failure_is_counted_and_titles_the_panel() {
        let d = tmp("fail");
        std::fs::create_dir_all(d.join("dest")).unwrap();
        let ctx = egui::Context::default();
        let mut t = Transfers::default();
        t.push(&ctx, vec![d.join("gone.txt")], d.join("dest"), false);
        let ended = run_all(&mut t, &ctx);
        assert_eq!(ended[0].stats.errors.len(), 1);
        let tot = t.totals();
        assert_eq!(tot.failures, 1);
        assert_eq!(t.title(&tot), "Some items failed");
    }

    #[test]
    fn time_left_reads_like_the_copy_window() {
        assert_eq!(duration(12), "12 s");
        assert_eq!(duration(150), "3 min");
        assert_eq!(duration(3 * 3600 + 12 * 60), "3 h 12 min");
        let t = Totals { running: Some((false, 1_000_000.0, 180_000_000)), ..Default::default() };
        assert_eq!(t.left().as_deref(), Some("about 4 min left"));
        let counting = Totals { scanning: true, ..t };
        assert_eq!(counting.left(), None, "no time left while still counting");
    }
}
