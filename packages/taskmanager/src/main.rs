//! Task Manager: what runs on the computer and what it uses, laid out like Windows' Task Manager (Processes, Performance,
//! App history, Startup apps, Users, Details, Services) in the Yggdrasil look, light or dark like the rest of the desktop.
//!
//! The numbers are read in a background thread (once a second by default; the speed can be changed or paused), so the window
//! stays smooth; the graphics card is asked only while the Performance tab shows, and services only while their tab does.
//! Ctrl+Shift+Esc opens it, or brings an open one to the front.

mod desk;
mod procs;
mod services;
mod startup;
mod sys;

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Margin, Rect, RichText, Sense, Stroke, StrokeKind};
use procs::{Group, Kind, Proc};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use yggdrasil_ui::theme::{self, Palette};
use yggdrasil_ui::{chrome, sysicon};

const APP_ID: &str = "yggdrasil-taskmanager";
const HISTORY: usize = 60;
const ROW_H: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tab {
    Processes,
    Performance,
    History,
    Startup,
    Users,
    Details,
    Services,
}

const TABS: [(Tab, &str); 7] = [
    (Tab::Processes, "Processes"),
    (Tab::Performance, "Performance"),
    (Tab::History, "App history"),
    (Tab::Startup, "Startup apps"),
    (Tab::Users, "Users"),
    (Tab::Details, "Details"),
    (Tab::Services, "Services"),
];

impl Tab {
    fn code(self) -> u8 {
        TABS.iter().position(|(t, _)| *t == self).unwrap_or(0) as u8
    }
    fn from_code(c: u8) -> Tab {
        TABS.get(c as usize).map(|t| t.0).unwrap_or(Tab::Processes)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Speed {
    High,
    Normal,
    Low,
    Paused,
}

impl Speed {
    fn ms(self) -> u64 {
        match self {
            Speed::High => 500,
            Speed::Normal => 1000,
            Speed::Low => 4000,
            Speed::Paused => 0,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Speed::High => "High (twice a second)",
            Speed::Normal => "Normal (every second)",
            Speed::Low => "Low (every 4 seconds)",
            Speed::Paused => "Paused",
        }
    }
}

// ---------------------------------------------------------------- what the background thread keeps

/// A disk's active time, read rate and write rate over the last minute.
type DiskHistory = (VecDeque<f32>, VecDeque<f32>, VecDeque<f32>);
/// One entry of the Performance tab's list: which, title, the line under it, colour, its history and the graph's top.
type PerfEntry<'a> = (PerfItem, String, String, Color32, &'a VecDeque<f32>, f32);

#[derive(Default)]
struct History {
    cpu: VecDeque<f32>,
    cores: Vec<VecDeque<f32>>,
    mem: VecDeque<f32>,
    disks: HashMap<String, DiskHistory>,
    nets: HashMap<String, (VecDeque<f32>, VecDeque<f32>)>,
    gpu: VecDeque<f32>,
}

fn push(q: &mut VecDeque<f32>, v: f32) {
    if q.len() >= HISTORY {
        q.pop_front();
    }
    q.push_back(v);
}

#[derive(Clone, Debug, Default)]
struct Session {
    uid: u32,
    user: String,
    state: String,
}

#[derive(Default)]
struct Data {
    procs: Vec<Proc>,
    groups: Vec<Group>,
    sys: sys::Snapshot,
    hist: History,
    services: Vec<services::Service>,
    services_at: Option<Instant>,
    startup: Vec<startup::Item>,
    startup_at: Option<Instant>,
    sessions: Vec<Session>,
    sessions_at: Option<Instant>,
    threads: u64,
    generation: u64,
}

struct Shared {
    data: Mutex<Data>,
    tab: AtomicU8,
    interval_ms: AtomicU64,
    refresh_now: AtomicBool,
}

fn parse_sessions(json: &str) -> Vec<Session> {
    let Ok(serde_json::Value::Array(v)) = serde_json::from_str::<serde_json::Value>(json) else { return vec![] };
    let mut out: Vec<Session> = vec![];
    for s in v {
        let Some(uid) = s.get("uid").and_then(|x| x.as_u64()) else { continue };
        if out.iter().any(|o| o.uid == uid as u32) {
            continue;
        }
        out.push(Session {
            uid: uid as u32,
            user: s.get("user").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            state: s.get("state").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        });
    }
    out
}

fn sampler(shared: Arc<Shared>, ctx: egui::Context) {
    let mut ps = procs::Sampler::new();
    let mut ss = sys::SysSampler::default();
    let apps = desk::Apps::load();
    let me = unsafe { libc::getuid() };
    loop {
        let tab = Tab::from_code(shared.tab.load(Ordering::Relaxed));
        let procs = ps.sample();
        let wins = if matches!(tab, Tab::Processes | Tab::History | Tab::Users) { desk::windows() } else { vec![] };
        let groups = procs::group(&procs, &wins, me, &|c, t, e| apps.name(c, t, e));
        let snap = ss.sample(tab == Tab::Performance);
        let now = Instant::now();
        let stale = |at: Option<Instant>, secs: u64| at.map(|t| now.duration_since(t) >= Duration::from_secs(secs)).unwrap_or(true);
        let forced = shared.refresh_now.swap(false, Ordering::Relaxed);
        let (svc_due, st_due, ses_due) = {
            let d = shared.data.lock().unwrap();
            (
                tab == Tab::Services && (forced || stale(d.services_at, 5)),
                tab == Tab::Startup && (forced || stale(d.startup_at, 5)),
                tab == Tab::Users && (forced || stale(d.sessions_at, 5)),
            )
        };
        let svcs = svc_due.then(services::list);
        let st = st_due.then(startup::list);
        let ses = ses_due.then(|| {
            std::process::Command::new("loginctl")
                .args(["list-sessions", "--output=json", "--no-pager"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .map(|o| parse_sessions(&String::from_utf8_lossy(&o.stdout)))
                .unwrap_or_default()
        });
        {
            let mut d = shared.data.lock().unwrap();
            let h = &mut d.hist;
            push(&mut h.cpu, snap.cpu);
            if h.cores.len() != snap.cores.len() {
                h.cores = vec![VecDeque::new(); snap.cores.len()];
            }
            for (q, v) in h.cores.iter_mut().zip(&snap.cores) {
                push(q, *v);
            }
            push(&mut h.mem, if snap.mem.total > 0 { snap.mem.used() as f32 / snap.mem.total as f32 * 100.0 } else { 0.0 });
            for disk in &snap.disks {
                let e = h.disks.entry(disk.name.clone()).or_default();
                push(&mut e.0, disk.active);
                push(&mut e.1, disk.read_rate as f32);
                push(&mut e.2, disk.write_rate as f32);
            }
            for a in &snap.adapters {
                let e = h.nets.entry(a.name.clone()).or_default();
                push(&mut e.0, a.send_rate as f32);
                push(&mut e.1, a.recv_rate as f32);
            }
            if let Some(g) = &snap.gpu {
                push(&mut h.gpu, g.util);
            }
            d.threads = procs.iter().filter(|p| !p.kernel).map(|p| p.threads as u64).sum();
            d.procs = procs;
            d.groups = groups;
            d.sys = snap;
            if let Some(s) = svcs {
                d.services = s;
                d.services_at = Some(now);
            }
            if let Some(s) = st {
                d.startup = s;
                d.startup_at = Some(now);
            }
            if let Some(s) = ses {
                d.sessions = s;
                d.sessions_at = Some(now);
            }
            d.generation += 1;
        }
        ctx.request_repaint();
        // wait for the next look; a tab change or a "refresh now" cuts the wait short
        let start = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(100));
            let ms = shared.interval_ms.load(Ordering::Relaxed);
            if shared.refresh_now.load(Ordering::Relaxed) || shared.tab.load(Ordering::Relaxed) != tab.code() {
                break;
            }
            if ms > 0 && start.elapsed() >= Duration::from_millis(ms) {
                break;
            }
        }
    }
}

// ---------------------------------------------------------------- formatting

fn mb(bytes: u64) -> String {
    let m = bytes as f64 / 1_048_576.0;
    if m >= 1024.0 * 10.0 {
        format!("{:.1} GB", m / 1024.0)
    } else {
        format!("{m:.1} MB")
    }
}

fn size(bytes: u64) -> String {
    let b = bytes as f64;
    if b >= 1e12 {
        format!("{:.1} TB", b / 1e12)
    } else if b >= 1e9 {
        format!("{:.1} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b:.0} B")
    }
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
}

fn rate(bps: f64) -> String {
    let m = bps / 1_048_576.0;
    if m >= 0.05 || bps == 0.0 {
        format!("{m:.1} MB/s")
    } else {
        format!("{:.0} KB/s", bps / 1024.0)
    }
}

fn bits(bytes_per_s: f64) -> String {
    let b = bytes_per_s * 8.0;
    if b >= 1e9 {
        format!("{:.1} Gbps", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.1} Mbps", b / 1e6)
    } else {
        format!("{:.0} Kbps", b / 1e3)
    }
}

fn duration(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    let (d, h, m, s) = (s / 86400, s / 3600 % 24, s / 60 % 60, s % 60);
    if d > 0 {
        format!("{d}:{h:02}:{m:02}:{s:02}")
    } else {
        format!("{h}:{m:02}:{s:02}")
    }
}

fn priority(nice: i64) -> &'static str {
    match nice {
        i64::MIN..=-10 => "Realtime-like",
        -9..=-5 => "High",
        -4..=-1 => "Above normal",
        0 => "Normal",
        1..=9 => "Below normal",
        _ => "Low",
    }
}

/// The heat behind a busy cell, as on Windows: pale yellow when a little, deep orange-red when a lot (0-1).
fn heat(pal: &Palette, dark: bool, x: f32) -> Color32 {
    let x = x.clamp(0.0, 1.0);
    if x < 0.005 {
        return Color32::TRANSPARENT;
    }
    let (lo, hi) = if dark { ([120u8, 96u8, 24u8], [176u8, 64u8, 24u8]) } else { ([255u8, 236u8, 160u8], [255u8, 160u8, 90u8]) };
    let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * x) as u8;
    let a = if dark { (40.0 + 150.0 * x) as u8 } else { (110.0 + 145.0 * x) as u8 };
    let _ = pal;
    Color32::from_rgba_unmultiplied(mix(lo[0], hi[0]), mix(lo[1], hi[1]), mix(lo[2], hi[2]), a)
}

// ---------------------------------------------------------------- tables

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Col {
    Name,
    Status,
    Cpu,
    Mem,
    Disk,
    Pid,
    User,
    Threads,
    Priority,
    Cmd,
    Started,
    CpuTime,
    Read,
    Written,
    Desc,
    Startup,
    Kind,
}

#[derive(Clone, Copy)]
struct Column {
    col: Col,
    title: &'static str,
    /// 0 = takes what is left.
    width: f32,
    right: bool,
}

const fn c(col: Col, title: &'static str, width: f32, right: bool) -> Column {
    Column { col, title, width, right }
}

fn widths(cols: &[Column], total: f32) -> Vec<f32> {
    let fixed: f32 = cols.iter().map(|c| c.width).sum();
    let flex = cols.iter().filter(|c| c.width == 0.0).count().max(1) as f32;
    cols.iter().map(|c| if c.width == 0.0 { ((total - fixed) / flex).max(120.0) } else { c.width }).collect()
}

/// The header row: titles (with a total above, when given) that sort when clicked.
fn header(ui: &mut egui::Ui, pal: &Palette, cols: &[Column], totals: &HashMap<Col, String>, sort: &mut (Col, bool)) {
    let h = if totals.is_empty() { 30.0 } else { 44.0 };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), h), Sense::hover());
    let w = widths(cols, rect.width());
    let mut x = rect.left();
    for (col, cw) in cols.iter().zip(w) {
        let cell = Rect::from_min_size(pos2(x, rect.top()), vec2(cw, h));
        let resp = ui.interact(cell, ui.id().with(("hdr", col.title)), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(cell, CornerRadius::ZERO, pal.hover);
        }
        if resp.clicked() {
            if sort.0 == col.col {
                sort.1 = !sort.1;
            } else {
                *sort =
                    (col.col, !matches!(col.col, Col::Name | Col::User | Col::Status | Col::Cmd | Col::Desc | Col::Kind | Col::Startup));
            }
        }
        let align = if col.right { Align2::RIGHT_BOTTOM } else { Align2::LEFT_BOTTOM };
        let tx = if col.right { cell.right() - 8.0 } else { cell.left() + 8.0 };
        let p = ui.painter().with_clip_rect(cell);
        if let Some(t) = totals.get(&col.col) {
            let ta = if col.right { Align2::RIGHT_TOP } else { Align2::LEFT_TOP };
            p.text(pos2(tx, cell.top() + 4.0), ta, t, FontId::proportional(15.0), pal.text);
        }
        p.text(
            pos2(tx, cell.bottom() - 6.0),
            align,
            col.title,
            FontId::proportional(12.0),
            if sort.0 == col.col { pal.accent } else { pal.dim },
        );
        if sort.0 == col.col {
            // a small triangle: up when ascending
            let cx = if col.right { cell.left() + 10.0 } else { cell.right() - 10.0 };
            let cy = cell.bottom() - 12.0;
            let pts = if sort.1 {
                vec![pos2(cx - 4.0, cy - 2.0), pos2(cx + 4.0, cy - 2.0), pos2(cx, cy + 3.0)]
            } else {
                vec![pos2(cx - 4.0, cy + 3.0), pos2(cx + 4.0, cy + 3.0), pos2(cx, cy - 2.0)]
            };
            ui.painter().add(egui::Shape::convex_polygon(pts, pal.accent, Stroke::NONE));
        }
        ui.painter().vline(cell.right(), cell.y_range(), Stroke::new(1.0, pal.border));
        x += cw;
    }
    ui.painter().hline(rect.x_range(), rect.bottom(), Stroke::new(1.0, pal.border));
}

/// One cell of text, cut at the cell's edge.
fn cell_text(ui: &egui::Ui, pal: &Palette, rect: Rect, text: &str, right: bool, color: Option<Color32>) {
    let p = ui.painter().with_clip_rect(rect.shrink2(vec2(4.0, 0.0)));
    let (pos, al) = if right {
        (pos2(rect.right() - 8.0, rect.center().y), Align2::RIGHT_CENTER)
    } else {
        (pos2(rect.left() + 8.0, rect.center().y), Align2::LEFT_CENTER)
    };
    p.text(pos, al, text, FontId::proportional(13.0), color.unwrap_or(pal.text));
}

fn triangle(ui: &egui::Ui, pal: &Palette, center: egui::Pos2, open: bool) {
    let pts = if open {
        vec![pos2(center.x - 4.0, center.y - 2.0), pos2(center.x + 4.0, center.y - 2.0), pos2(center.x, center.y + 3.0)]
    } else {
        vec![pos2(center.x - 2.0, center.y - 4.0), pos2(center.x - 2.0, center.y + 4.0), pos2(center.x + 3.0, center.y)]
    };
    ui.painter().add(egui::Shape::convex_polygon(pts, pal.dim, Stroke::NONE));
}

// ---------------------------------------------------------------- what the user asked for

#[derive(Clone, Debug, PartialEq)]
enum Act {
    /// End (SIGTERM) or force (SIGKILL) these processes; `tree`: they are a whole tree.
    Signal {
        pids: Vec<u32>,
        sig: i32,
        what: String,
    },
    Renice {
        pid: u32,
        nice: i64,
    },
    Service(services::Service, services::Action),
    Startup(startup::Item, bool),
    Focus(String),
    Open(String),
}

struct Confirm {
    title: String,
    text: String,
    act: Act,
}

struct RunDialog {
    cmd: String,
    admin: bool,
    error: String,
    focus: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PerfItem {
    Cpu,
    Mem,
    Disk(usize),
    Net(usize),
    Gpu,
}

struct App {
    shared: Arc<Shared>,
    tab: Tab,
    speed: Speed,
    search: String,
    proc_sort: (Col, bool),
    detail_sort: (Col, bool),
    hist_sort: (Col, bool),
    svc_sort: (Col, bool),
    startup_sort: (Col, bool),
    expanded: HashSet<u32>,
    sel_pid: Option<u32>,
    sel_service: Option<String>,
    sel_startup: Option<String>,
    perf: PerfItem,
    per_core: bool,
    confirm: Option<Confirm>,
    run: Option<RunDialog>,
    props: Option<u32>,
    note: Option<(String, Instant)>,
    apps: desk::Apps,
    cpu_info: sys::CpuInfo,
    me: u32,
    results: Arc<Mutex<Vec<String>>>,
}

impl App {
    fn new(cc: &eframe::CreationContext, tab: Tab) -> App {
        let shared = Arc::new(Shared {
            data: Mutex::new(Data::default()),
            tab: AtomicU8::new(tab.code()),
            interval_ms: AtomicU64::new(Speed::Normal.ms()),
            refresh_now: AtomicBool::new(false),
        });
        let s = shared.clone();
        let ctx = cc.egui_ctx.clone();
        std::thread::Builder::new().name("sampler".into()).spawn(move || sampler(s, ctx)).expect("sampler thread");
        App {
            shared,
            tab,
            speed: Speed::Normal,
            search: String::new(),
            proc_sort: (Col::Name, false),
            detail_sort: (Col::Name, false),
            hist_sort: (Col::CpuTime, true),
            svc_sort: (Col::Name, false),
            startup_sort: (Col::Name, false),
            expanded: HashSet::new(),
            sel_pid: None,
            sel_service: None,
            sel_startup: None,
            perf: PerfItem::Cpu,
            per_core: false,
            confirm: None,
            run: None,
            props: None,
            note: None,
            apps: desk::Apps::load(),
            cpu_info: sys::cpu_info(),
            me: unsafe { libc::getuid() },
            results: Arc::new(Mutex::new(vec![])),
        }
    }

    fn say(&mut self, msg: impl Into<String>) {
        self.note = Some((msg.into(), Instant::now()));
    }

    fn refresh(&self) {
        self.shared.refresh_now.store(true, Ordering::Relaxed);
    }

    /// Asks first when ending something would hurt (another account's process, the session itself, a forced end, a whole tree).
    fn request(&mut self, act: Act, procs: &[Proc]) {
        if let Act::Signal { pids, sig, what } = &act {
            let by: HashMap<u32, &Proc> = procs.iter().map(|p| (p.pid, p)).collect();
            let foreign = pids.iter().any(|p| by.get(p).is_some_and(|p| p.uid != self.me));
            let critical: Vec<String> =
                pids.iter().filter_map(|p| by.get(p)).filter(|p| procs::is_critical(p)).map(|p| p.name.clone()).collect();
            if foreign || !critical.is_empty() || *sig == libc::SIGKILL || pids.len() > 1 && what.contains("tree") {
                let mut text = String::new();
                if !critical.is_empty() {
                    text += &format!("{} is part of the desktop or the system itself: ending it can sign you out, stop sound or input, or stop the computer.\n\n", critical.join(", "));
                }
                if foreign {
                    text += "It belongs to the system (another account), so it asks for your password.\n\n";
                }
                if *sig == libc::SIGKILL {
                    text += "Forcing it to end gives it no chance to save anything.\n\n";
                }
                text += "Do you want to go ahead?";
                self.confirm = Some(Confirm { title: format!("{what}?"), text, act });
                return;
            }
        }
        self.apply(act);
    }

    fn apply(&mut self, act: Act) {
        match act {
            Act::Signal { pids, sig, what } => {
                let failed = procs::signal(&pids, sig);
                if !failed.is_empty() {
                    // another account's: through polkit (pkexec asks for the password)
                    let sig_name = if sig == libc::SIGKILL {
                        "KILL"
                    } else if sig == libc::SIGSTOP {
                        "STOP"
                    } else if sig == libc::SIGCONT {
                        "CONT"
                    } else {
                        "TERM"
                    };
                    let mut args = vec!["kill".to_string(), "-s".into(), sig_name.into()];
                    args.extend(failed.iter().map(|p| p.to_string()));
                    self.spawn_checked("pkexec", args, format!("{what}: not allowed"));
                } else {
                    self.say(format!("{what}: done"));
                }
                self.refresh();
            }
            Act::Renice { pid, nice } => {
                let r = unsafe { libc::setpriority(libc::PRIO_PROCESS, pid as libc::id_t, nice as libc::c_int) };
                if r != 0 {
                    self.spawn_checked(
                        "pkexec",
                        vec!["renice".into(), "-n".into(), nice.to_string(), "-p".into(), pid.to_string()],
                        "Priority: not allowed".into(),
                    );
                } else {
                    self.say(format!("Priority set to {}", priority(nice)));
                }
                self.refresh();
            }
            Act::Service(s, a) => {
                let results = self.results.clone();
                let shared = self.shared.clone();
                self.say(format!(
                    "{} {}...",
                    match a {
                        services::Action::Start => "Starting",
                        services::Action::Stop => "Stopping",
                        services::Action::Restart => "Restarting",
                        services::Action::Enable => "Switching on",
                        services::Action::Disable => "Switching off",
                    },
                    s.name
                ));
                std::thread::spawn(move || {
                    if let Err(e) = services::run(&s, a) {
                        results.lock().unwrap().push(e);
                    }
                    shared.refresh_now.store(true, Ordering::Relaxed);
                });
            }
            Act::Startup(item, on) => {
                match startup::set(&item, on) {
                    Ok(()) => self.say(format!(
                        "{} {}",
                        item.name,
                        if on { "starts when you sign in" } else { "no longer starts when you sign in" }
                    )),
                    Err(e) => self.say(e),
                }
                self.refresh();
            }
            Act::Focus(addr) => {
                desk::focus_window(&addr);
            }
            Act::Open(target) => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(target)
                    .stdin(std::process::Stdio::null())
                    .spawn()
                    .map(|mut c| std::thread::spawn(move || c.wait()));
            }
        }
    }

    /// Runs a helper in the background; a failure shows as a note.
    fn spawn_checked(&self, prog: &str, args: Vec<String>, fail: String) {
        let results = self.results.clone();
        let shared = self.shared.clone();
        let prog = prog.to_string();
        std::thread::spawn(move || {
            let ok = std::process::Command::new(&prog)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false);
            if !ok {
                results.lock().unwrap().push(fail);
            }
            shared.refresh_now.store(true, Ordering::Relaxed);
        });
    }

    fn matches(&self, text: &str) -> bool {
        self.search.is_empty() || text.to_lowercase().contains(&self.search.to_lowercase())
    }
}

/// Starts a program as Windows' "Run new task" does: a folder or web address opens, anything else runs as a command line.
fn run_task(cmd: &str, admin: bool) -> Result<(), String> {
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return Err("Type a program, folder or address.".into());
    }
    let expanded = if let Some(rest) = cmd.strip_prefix('~') {
        format!("{}{rest}", std::env::var("HOME").unwrap_or_default())
    } else {
        cmd.to_string()
    };
    let is_place = std::path::Path::new(&expanded).exists() && !std::path::Path::new(&expanded).is_file()
        || cmd.starts_with("http://")
        || cmd.starts_with("https://");
    let mut c = if is_place && !admin {
        let mut c = std::process::Command::new("xdg-open");
        c.arg(&expanded);
        c
    } else {
        let first = cmd.split_whitespace().next().unwrap_or("");
        let found = first.contains('/') && std::path::Path::new(first).exists()
            || std::env::var("PATH").unwrap_or_default().split(':').any(|d| std::path::Path::new(d).join(first).is_file());
        if !found {
            return Err(format!("Cannot find \"{first}\". Check the name and try again."));
        }
        let mut c = std::process::Command::new(if admin { "pkexec" } else { "sh" });
        if admin {
            c.args(["sh", "-c", cmd]);
        } else {
            c.args(["-c", cmd]);
        }
        c
    };
    use std::os::unix::process::CommandExt;
    c.stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).process_group(0);
    let mut child = c.spawn().map_err(|e| e.to_string())?;
    std::thread::spawn(move || child.wait());
    Ok(())
}

// ---------------------------------------------------------------- the window

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        chrome::resize_edges(&ctx);
        let accent = theme::system_accent();
        let dark = theme::system_dark();
        theme::apply(&ctx, dark, accent);
        let pal = theme::palette_for(dark, accent);
        sysicon::refresh(&ctx);
        let finished = std::mem::take(&mut *self.results.lock().unwrap());
        for e in finished {
            self.say(e);
        }
        if ctx.input(|i| i.key_pressed(egui::Key::F5)) {
            self.refresh();
        }

        let shared = self.shared.clone();
        let data = shared.data.lock().unwrap();
        let mut acts: Vec<Act> = vec![];

        egui::Panel::left("tabs")
            .default_size(200.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(pal.sidebar).inner_margin(Margin::symmetric(10, 12)))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add_space(4.0);
                    ui.label(RichText::new("Task Manager").size(17.0).strong());
                });
                ui.add_space(14.0);
                for (t, name) in TABS {
                    let sel = self.tab == t;
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    if sel {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.select);
                        ui.painter().rect_filled(
                            Rect::from_min_size(rect.left_top() + vec2(0.0, 9.0), vec2(3.0, 16.0)),
                            CornerRadius::same(2),
                            pal.accent,
                        );
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(8), pal.hover);
                    }
                    ui.painter().text(
                        rect.left_center() + vec2(14.0, 0.0),
                        Align2::LEFT_CENTER,
                        name,
                        FontId::proportional(14.0),
                        pal.text,
                    );
                    if resp.clicked() {
                        self.tab = t;
                        self.shared.tab.store(t.code(), Ordering::Relaxed);
                    }
                }
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    let before = self.speed;
                    egui::ComboBox::from_id_salt("speed").width(ui.available_width()).selected_text(self.speed.label()).show_ui(ui, |ui| {
                        for s in [Speed::High, Speed::Normal, Speed::Low, Speed::Paused] {
                            ui.selectable_value(&mut self.speed, s, s.label());
                        }
                    });
                    if self.speed != before {
                        self.shared.interval_ms.store(self.speed.ms(), Ordering::Relaxed);
                        self.refresh();
                    }
                    ui.label(RichText::new("Update speed").small().color(pal.dim));
                });
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(pal.bg).inner_margin(Margin { left: 12, right: 8, top: 6, bottom: 8 }))
            .show(ui, |ui| {
                // title row: search in the middle, window buttons at the right
                ui.horizontal(|ui| {
                    let title = TABS.iter().find(|t| t.0 == self.tab).map(|t| t.1).unwrap_or("");
                    ui.label(RichText::new(title).size(18.0).strong());
                    ui.add_space(16.0);
                    if !matches!(self.tab, Tab::Performance) {
                        ui.add(
                            egui::TextEdit::singleline(&mut self.search)
                                .hint_text("Type a name, publisher or PID to search")
                                .desired_width(320.0),
                        );
                    }
                    chrome::drag_area(ui, ui.available_width() - chrome::controls_width());
                    chrome::window_controls(ui, &pal);
                });
                ui.add_space(4.0);
                match self.tab {
                    Tab::Processes => self.processes(ui, &pal, dark, &data, &mut acts),
                    Tab::Performance => self.performance(ui, &pal, &data, &mut acts),
                    Tab::History => self.history(ui, &pal, &data),
                    Tab::Startup => self.startup(ui, &pal, &data, &mut acts),
                    Tab::Users => self.users(ui, &pal, dark, &data),
                    Tab::Details => self.details(ui, &pal, dark, &data, &mut acts),
                    Tab::Services => self.services(ui, &pal, &data, &mut acts),
                }
            });

        // the properties of a process
        if let Some(pid) = self.props {
            let p = data.procs.iter().find(|p| p.pid == pid).cloned();
            let resp = egui::Modal::new(egui::Id::new("props")).show(&ctx, |ui| {
                ui.set_width(560.0);
                let Some(p) = p else {
                    ui.label("That process has ended.");
                    return false;
                };
                ui.label(RichText::new(format!("{} (PID {})", p.name, p.pid)).size(17.0).strong());
                ui.add_space(8.0);
                let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))
                    .map(|x| x.to_string_lossy().to_string())
                    .unwrap_or_else(|_| "(not shown for another account's process)".into());
                let started = data.sys.uptime - p.started;
                egui::Grid::new("props-grid").num_columns(2).spacing([16.0, 6.0]).show(ui, |ui| {
                    for (k, v) in [
                        ("Program", p.exe.clone()),
                        ("Command line", p.cmd.clone()),
                        ("Working folder", cwd),
                        ("Account", p.user.clone()),
                        ("Parent", format!("{}", p.ppid)),
                        ("Status", p.state_text().to_string()),
                        ("Started", format!("{} ago", duration(started))),
                        ("Processor time", duration(p.cpu_time)),
                        ("Memory (its own)", mb(p.mem)),
                        ("Threads", p.threads.to_string()),
                        ("Priority", format!("{} (nice {})", priority(p.nice), p.nice)),
                        ("Read from disk", size(p.read)),
                        ("Written to disk", size(p.written)),
                    ] {
                        ui.label(RichText::new(k).color(pal.dim));
                        ui.add(egui::Label::new(v).wrap());
                        ui.end_row();
                    }
                });
                ui.add_space(10.0);
                let mut close = false;
                ui.horizontal(|ui| {
                    if ui.button("Copy command line").clicked() {
                        ui.ctx().copy_text(p.cmd.clone());
                    }
                    if !p.exe.is_empty() && ui.button("Open file location").clicked() {
                        if let Some(dir) = std::path::Path::new(&p.exe).parent() {
                            acts.push(Act::Open(dir.to_string_lossy().to_string()));
                        }
                    }
                    if ui.button("Close").clicked() {
                        close = true;
                    }
                });
                close
            });
            if resp.inner || resp.should_close() {
                self.props = None;
            }
        }

        // asking first
        if let Some(cf) = &self.confirm {
            let (title, text) = (cf.title.clone(), cf.text.clone());
            let mut decided: Option<bool> = None;
            let resp = egui::Modal::new(egui::Id::new("confirm")).show(&ctx, |ui| {
                ui.set_width(440.0);
                ui.label(RichText::new(&title).size(17.0).strong());
                ui.add_space(8.0);
                ui.label(&text);
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.add(egui::Button::new(RichText::new("Go ahead").color(Color32::WHITE)).fill(pal.red)).clicked() {
                        decided = Some(true);
                    }
                    if ui.button("Cancel").clicked() {
                        decided = Some(false);
                    }
                });
            });
            if resp.should_close() && decided.is_none() {
                decided = Some(false);
            }
            if let Some(go) = decided {
                let cf = self.confirm.take().unwrap();
                if go {
                    acts.push(cf.act);
                }
            }
        }

        // Run new task
        let mut close_run = false;
        if let Some(run) = &mut self.run {
            let resp = egui::Modal::new(egui::Id::new("run")).show(&ctx, |ui| {
                ui.set_width(460.0);
                ui.label(RichText::new("Run new task").size(17.0).strong());
                ui.add_space(6.0);
                ui.label("Type the name of a program, folder or internet address, and it opens.");
                ui.add_space(6.0);
                let te = ui.add(
                    egui::TextEdit::singleline(&mut run.cmd)
                        .desired_width(f32::INFINITY)
                        .hint_text("e.g. ghostty, ~/Documents or https://..."),
                );
                if run.focus {
                    te.request_focus();
                    run.focus = false;
                }
                ui.checkbox(&mut run.admin, "Create this task with administrator rights (asks for your password)");
                if !run.error.is_empty() {
                    ui.colored_label(pal.red, &run.error);
                }
                ui.add_space(8.0);
                let enter = te.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                ui.horizontal(|ui| {
                    if ui.button("OK").clicked() || enter {
                        match run_task(&run.cmd, run.admin) {
                            Ok(()) => close_run = true,
                            Err(e) => run.error = e,
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        close_run = true;
                    }
                });
            });
            if resp.should_close() {
                close_run = true;
            }
        }
        if close_run {
            self.run = None;
            self.refresh();
        }

        // a note at the bottom for a few seconds
        if let Some((msg, at)) = &self.note {
            if at.elapsed() < Duration::from_secs(5) {
                let r = ctx.content_rect();
                let galley = ctx.fonts_mut(|f| f.layout_no_wrap(msg.clone(), FontId::proportional(13.0), pal.text));
                let w = galley.size().x + 28.0;
                let rect = Rect::from_center_size(pos2(r.center().x + 100.0, r.bottom() - 30.0), vec2(w, 30.0));
                let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("note")));
                painter.rect(rect, CornerRadius::same(8), pal.field, Stroke::new(1.0, pal.border), StrokeKind::Inside);
                painter.galley(rect.center() - galley.size() / 2.0, galley, pal.text);
                ctx.request_repaint_after(Duration::from_millis(500));
            } else {
                self.note = None;
            }
        }
        let procs = data.procs.clone();
        drop(data);
        for a in acts {
            self.request(a, &procs);
        }
        chrome::outline(&ctx, &pal);
    }
}

// ---------------------------------------------------------------- tabs

/// The command bar under the title (Run new task and the tab's own buttons).
fn command_bar(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        add(ui);
    });
    ui.add_space(6.0);
}

fn sort_groups(v: &mut [&Group], sort: (Col, bool)) {
    v.sort_by(|a, b| {
        let o = match sort.0 {
            Col::Cpu => a.cpu.partial_cmp(&b.cpu).unwrap_or(std::cmp::Ordering::Equal),
            Col::Mem => a.mem.cmp(&b.mem),
            Col::Disk => a.disk.partial_cmp(&b.disk).unwrap_or(std::cmp::Ordering::Equal),
            Col::Status => a.suspended.cmp(&b.suspended),
            Col::CpuTime => a.cpu_time.partial_cmp(&b.cpu_time).unwrap_or(std::cmp::Ordering::Equal),
            Col::Read => a.read.cmp(&b.read),
            Col::Written => a.written.cmp(&b.written),
            Col::Started => b.started.partial_cmp(&a.started).unwrap_or(std::cmp::Ordering::Equal),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        };
        if sort.1 {
            o.reverse()
        } else {
            o
        }
    });
}

fn sort_procs(v: &mut [&Proc], sort: (Col, bool)) {
    v.sort_by(|a, b| {
        let o = match sort.0 {
            Col::Cpu => a.cpu.partial_cmp(&b.cpu).unwrap_or(std::cmp::Ordering::Equal),
            Col::Mem => a.mem.cmp(&b.mem),
            Col::Disk => a.disk.partial_cmp(&b.disk).unwrap_or(std::cmp::Ordering::Equal),
            Col::Pid => a.pid.cmp(&b.pid),
            Col::User => a.user.cmp(&b.user),
            Col::Threads => a.threads.cmp(&b.threads),
            Col::Priority => a.nice.cmp(&b.nice),
            Col::Status => a.state.cmp(&b.state),
            Col::Cmd => a.cmd.cmp(&b.cmd),
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.pid.cmp(&b.pid)),
        };
        if sort.1 {
            o.reverse()
        } else {
            o
        }
    });
}

/// A row in the Processes tab.
enum Row<'a> {
    Section(&'static str, usize),
    Group(&'a Group),
    Child(&'a Proc),
}

impl App {
    fn group_menu(&mut self, ui: &mut egui::Ui, g: &Group, acts: &mut Vec<Act>) {
        let end_what = format!("End {}", g.name);
        if ui.button("End task").clicked() {
            // children first, so a parent cannot start them again
            let mut pids = g.pids.clone();
            pids.reverse();
            acts.push(Act::Signal { pids, sig: libc::SIGTERM, what: end_what.clone() });
            ui.close();
        }
        if ui.button("Force it to end").clicked() {
            acts.push(Act::Signal { pids: g.pids.clone(), sig: libc::SIGKILL, what: format!("Force {} to end", g.name) });
            ui.close();
        }
        if g.suspended {
            if ui.button("Resume").clicked() {
                acts.push(Act::Signal { pids: g.pids.clone(), sig: libc::SIGCONT, what: format!("Resume {}", g.name) });
                ui.close();
            }
        } else if ui.button("Suspend (pause)").clicked() {
            acts.push(Act::Signal { pids: g.pids.clone(), sig: libc::SIGSTOP, what: format!("Suspend {}", g.name) });
            ui.close();
        }
        ui.separator();
        if let Some(w) = g.windows.first() {
            if ui.button("Bring to front").clicked() {
                acts.push(Act::Focus(w.address.clone()));
                ui.close();
            }
            if ui.button("Close its windows (it may ask to save)").clicked() {
                for w in &g.windows {
                    desk::close_window(&w.address);
                }
                ui.close();
            }
        }
        if ui.button("Go to details").clicked() {
            self.tab = Tab::Details;
            self.shared.tab.store(Tab::Details.code(), Ordering::Relaxed);
            self.sel_pid = Some(g.root);
            ui.close();
        }
        if ui.button("Properties").clicked() {
            self.props = Some(g.root);
            ui.close();
        }
        if ui.button("Search online").clicked() {
            acts.push(Act::Open(format!("https://duckduckgo.com/?q={}", g.name.replace(' ', "+").replace('&', "%26"))));
            ui.close();
        }
    }

    fn proc_menu(&mut self, ui: &mut egui::Ui, p: &Proc, kids: &HashMap<u32, Vec<u32>>, acts: &mut Vec<Act>) {
        if ui.button("End task").clicked() {
            acts.push(Act::Signal { pids: vec![p.pid], sig: libc::SIGTERM, what: format!("End {}", p.name) });
            ui.close();
        }
        if ui.button("End process tree").clicked() {
            let mut pids = procs::descendants(p.pid, kids);
            pids.reverse();
            pids.push(p.pid);
            acts.push(Act::Signal { pids, sig: libc::SIGTERM, what: format!("End the {} process tree", p.name) });
            ui.close();
        }
        if ui.button("Force it to end").clicked() {
            acts.push(Act::Signal { pids: vec![p.pid], sig: libc::SIGKILL, what: format!("Force {} to end", p.name) });
            ui.close();
        }
        if matches!(p.state, 'T' | 't') {
            if ui.button("Resume").clicked() {
                acts.push(Act::Signal { pids: vec![p.pid], sig: libc::SIGCONT, what: format!("Resume {}", p.name) });
                ui.close();
            }
        } else if ui.button("Suspend (pause)").clicked() {
            acts.push(Act::Signal { pids: vec![p.pid], sig: libc::SIGSTOP, what: format!("Suspend {}", p.name) });
            ui.close();
        }
        ui.menu_button("Set priority", |ui| {
            for (label, nice) in [("High", -5), ("Above normal", -2), ("Normal", 0), ("Below normal", 5), ("Low", 15)] {
                if ui.radio(priority(p.nice) == priority(nice), label).clicked() {
                    acts.push(Act::Renice { pid: p.pid, nice });
                    ui.close();
                }
            }
        });
        ui.separator();
        if !p.exe.is_empty() && ui.button("Open file location").clicked() {
            if let Some(dir) = std::path::Path::new(&p.exe).parent() {
                acts.push(Act::Open(dir.to_string_lossy().to_string()));
            }
            ui.close();
        }
        if ui.button("Copy command line").clicked() {
            ui.ctx().copy_text(p.cmd.clone());
            ui.close();
        }
        if ui.button("Properties").clicked() {
            self.props = Some(p.pid);
            ui.close();
        }
    }

    fn processes(&mut self, ui: &mut egui::Ui, pal: &Palette, dark: bool, data: &Data, acts: &mut Vec<Act>) {
        let by_pid: HashMap<u32, &Proc> = data.procs.iter().map(|p| (p.pid, p)).collect();
        let kids = procs::children(&data.procs);
        let selected_group = self.sel_pid.and_then(|pid| data.groups.iter().find(|g| g.root == pid));
        command_bar(ui, |ui| {
            if ui.button("Run new task").clicked() {
                self.run = Some(RunDialog { cmd: String::new(), admin: false, error: String::new(), focus: true });
            }
            let end = ui.add_enabled(self.sel_pid.is_some(), egui::Button::new("End task"));
            if end.clicked() {
                if let Some(g) = selected_group {
                    let mut pids = g.pids.clone();
                    pids.reverse();
                    acts.push(Act::Signal { pids, sig: libc::SIGTERM, what: format!("End {}", g.name) });
                } else if let Some(p) = self.sel_pid.and_then(|pid| by_pid.get(&pid)) {
                    acts.push(Act::Signal { pids: vec![p.pid], sig: libc::SIGTERM, what: format!("End {}", p.name) });
                }
            }
        });
        let mem_total = data.sys.mem.total.max(1);
        let disk_total: f64 = data.sys.disks.iter().map(|d| d.read_rate + d.write_rate).sum();
        let disk_active = data.sys.disks.iter().map(|d| d.active).fold(0.0f32, f32::max);
        let totals: HashMap<Col, String> = [
            (Col::Cpu, format!("{:.0}%", data.sys.cpu)),
            (Col::Mem, format!("{:.0}%", data.sys.mem.used() as f64 / mem_total as f64 * 100.0)),
            (Col::Disk, format!("{disk_active:.0}%")),
        ]
        .into_iter()
        .collect();
        let _ = disk_total;
        let cols = [
            c(Col::Name, "Name", 0.0, false),
            c(Col::Status, "Status", 110.0, false),
            c(Col::Cpu, "CPU", 80.0, true),
            c(Col::Mem, "Memory", 100.0, true),
            c(Col::Disk, "Disk", 90.0, true),
        ];
        header(ui, pal, &cols, &totals, &mut self.proc_sort);

        let mut rows: Vec<Row> = vec![];
        for (kind, title) in [(Kind::App, "Apps"), (Kind::Background, "Background processes"), (Kind::System, "System processes")] {
            let mut gs: Vec<&Group> = data
                .groups
                .iter()
                .filter(|g| g.kind() == kind)
                .filter(|g| {
                    self.matches(&g.name)
                        || self.matches(&g.root.to_string())
                        || g.pids.iter().any(|p| by_pid.get(p).is_some_and(|p| self.matches(&p.name) || self.matches(&p.pid.to_string())))
                })
                .collect();
            if gs.is_empty() {
                continue;
            }
            sort_groups(&mut gs, self.proc_sort);
            rows.push(Row::Section(title, gs.len()));
            for g in gs {
                rows.push(Row::Group(g));
                if self.expanded.contains(&g.root) && g.pids.len() > 1 {
                    let mut ps: Vec<&Proc> = g.pids.iter().filter_map(|p| by_pid.get(p).copied()).collect();
                    sort_procs(&mut ps, self.proc_sort);
                    rows.extend(ps.into_iter().map(Row::Child));
                }
            }
        }
        let w = widths(&cols, ui.available_width());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, rows.len(), |ui, range| {
            for row in &rows[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                let mut x = rect.left();
                let cells: Vec<Rect> = w
                    .iter()
                    .map(|cw| {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H));
                        x += cw;
                        r
                    })
                    .collect();
                match row {
                    Row::Section(title, n) => {
                        ui.painter().text(
                            rect.left_center() + vec2(8.0, 2.0),
                            Align2::LEFT_CENTER,
                            format!("{title} ({n})"),
                            FontId::proportional(13.5),
                            pal.accent,
                        );
                    }
                    Row::Group(g) => {
                        let sel = self.sel_pid == Some(g.root);
                        if sel {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), pal.select);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), pal.hover);
                        }
                        for (i, (v, scale)) in
                            [(g.cpu as f64, 25.0f64), (g.mem as f64 / mem_total as f64 * 100.0, 25.0), (g.disk / 1_048_576.0, 20.0)]
                                .into_iter()
                                .enumerate()
                        {
                            let r = cells[2 + i];
                            ui.painter().rect_filled(r.shrink2(vec2(0.0, 1.0)), CornerRadius::ZERO, heat(pal, dark, (v / scale) as f32));
                        }
                        let name_r = cells[0];
                        let many = g.pids.len() > 1;
                        if many {
                            let open = self.expanded.contains(&g.root);
                            let tri = Rect::from_center_size(pos2(name_r.left() + 12.0, name_r.center().y), vec2(18.0, 18.0));
                            triangle(ui, pal, tri.center(), open);
                            if resp.clicked() && resp.interact_pointer_pos().is_some_and(|p| tri.expand(4.0).contains(p)) {
                                if open {
                                    self.expanded.remove(&g.root);
                                } else {
                                    self.expanded.insert(g.root);
                                }
                            }
                        }
                        let icon_r = Rect::from_center_size(pos2(name_r.left() + 32.0, name_r.center().y), vec2(16.0, 16.0));
                        let info = self.apps.find_for(&g.class, by_pid.get(&g.root).map(|p| p.exe.as_str()).unwrap_or(""));
                        let mut names: Vec<&str> = vec![];
                        if let Some(i) = info {
                            names.push(i.icon.as_str());
                        }
                        names.push(if g.kind() == Kind::App { sysicon::DEFAULT_APP_ICON } else { "application-x-executable" });
                        if !sysicon::paint_app(ui.painter(), icon_r, &names) {
                            ui.painter().rect_filled(icon_r.shrink(2.0), CornerRadius::same(3), pal.dim);
                        }
                        let label = if many { format!("{} ({})", g.name, g.pids.len()) } else { g.name.clone() };
                        cell_text(ui, pal, Rect::from_min_max(pos2(name_r.left() + 36.0, name_r.top()), name_r.max), &label, false, None);
                        cell_text(ui, pal, cells[1], if g.suspended { "Suspended" } else { "" }, false, Some(pal.amber));
                        cell_text(ui, pal, cells[2], &format!("{:.1}%", g.cpu), true, None);
                        cell_text(ui, pal, cells[3], &mb(g.mem), true, None);
                        cell_text(ui, pal, cells[4], &rate(g.disk), true, None);
                        if resp.clicked() {
                            self.sel_pid = Some(g.root);
                        }
                        if resp.double_clicked() {
                            if let Some(w) = g.windows.first() {
                                acts.push(Act::Focus(w.address.clone()));
                            } else if many && !self.expanded.remove(&g.root) {
                                self.expanded.insert(g.root);
                            }
                        }
                        let g2 = (*g).clone();
                        resp.context_menu(|ui| {
                            self.sel_pid = Some(g2.root);
                            self.group_menu(ui, &g2, acts);
                        });
                    }
                    Row::Child(p) => {
                        let sel = self.sel_pid == Some(p.pid);
                        if sel {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), pal.select);
                        } else if resp.hovered() {
                            ui.painter().rect_filled(rect, CornerRadius::same(4), pal.hover);
                        }
                        for (i, (v, scale)) in
                            [(p.cpu as f64, 25.0f64), (p.mem as f64 / mem_total as f64 * 100.0, 25.0), (p.disk / 1_048_576.0, 20.0)]
                                .into_iter()
                                .enumerate()
                        {
                            ui.painter().rect_filled(
                                cells[2 + i].shrink2(vec2(0.0, 1.0)),
                                CornerRadius::ZERO,
                                heat(pal, dark, (v / scale) as f32),
                            );
                        }
                        let name_r = cells[0];
                        cell_text(
                            ui,
                            pal,
                            Rect::from_min_max(pos2(name_r.left() + 44.0, name_r.top()), name_r.max),
                            &format!("{}  ({})", p.name, p.pid),
                            false,
                            Some(pal.dim),
                        );
                        cell_text(ui, pal, cells[1], if matches!(p.state, 'T' | 't') { "Suspended" } else { "" }, false, Some(pal.amber));
                        cell_text(ui, pal, cells[2], &format!("{:.1}%", p.cpu), true, None);
                        cell_text(ui, pal, cells[3], &mb(p.mem), true, None);
                        cell_text(ui, pal, cells[4], &rate(p.disk), true, None);
                        if resp.clicked() {
                            self.sel_pid = Some(p.pid);
                        }
                        let p2 = (*p).clone();
                        resp.context_menu(|ui| {
                            self.sel_pid = Some(p2.pid);
                            self.proc_menu(ui, &p2, &kids, acts);
                        });
                    }
                }
            }
        });
    }

    fn details(&mut self, ui: &mut egui::Ui, pal: &Palette, dark: bool, data: &Data, acts: &mut Vec<Act>) {
        let kids = procs::children(&data.procs);
        let sel = self.sel_pid.and_then(|pid| data.procs.iter().find(|p| p.pid == pid));
        command_bar(ui, |ui| {
            if ui.button("Run new task").clicked() {
                self.run = Some(RunDialog { cmd: String::new(), admin: false, error: String::new(), focus: true });
            }
            if ui.add_enabled(sel.is_some(), egui::Button::new("End task")).clicked() {
                if let Some(p) = sel {
                    acts.push(Act::Signal { pids: vec![p.pid], sig: libc::SIGTERM, what: format!("End {}", p.name) });
                }
            }
        });
        let cols = [
            c(Col::Name, "Name", 190.0, false),
            c(Col::Pid, "PID", 70.0, true),
            c(Col::Status, "Status", 100.0, false),
            c(Col::User, "User", 100.0, false),
            c(Col::Cpu, "CPU", 64.0, true),
            c(Col::Mem, "Memory", 90.0, true),
            c(Col::Threads, "Threads", 70.0, true),
            c(Col::Priority, "Priority", 100.0, false),
            c(Col::Cmd, "Command line", 0.0, false),
        ];
        header(ui, pal, &cols, &HashMap::new(), &mut self.detail_sort);
        let mut ps: Vec<&Proc> = data
            .procs
            .iter()
            .filter(|p| !p.kernel)
            .filter(|p| self.matches(&p.name) || self.matches(&p.pid.to_string()) || self.matches(&p.cmd) || self.matches(&p.user))
            .collect();
        sort_procs(&mut ps, self.detail_sort);
        let w = widths(&cols, ui.available_width());
        let mem_total = data.sys.mem.total.max(1);
        let scroll_to = self.sel_pid;
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, ps.len(), |ui, range| {
            for (i, p) in ps[range.clone()].iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                if (range.start + i) % 2 == 1 {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, pal.row_alt);
                }
                if scroll_to == Some(p.pid) {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.select);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.hover);
                }
                let mut x = rect.left();
                let cells: Vec<Rect> = w
                    .iter()
                    .map(|cw| {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H));
                        x += cw;
                        r
                    })
                    .collect();
                ui.painter().rect_filled(cells[4].shrink2(vec2(0.0, 1.0)), CornerRadius::ZERO, heat(pal, dark, p.cpu / 25.0));
                ui.painter().rect_filled(
                    cells[5].shrink2(vec2(0.0, 1.0)),
                    CornerRadius::ZERO,
                    heat(pal, dark, (p.mem as f64 / mem_total as f64 * 4.0) as f32),
                );
                cell_text(ui, pal, cells[0], &p.name, false, None);
                cell_text(ui, pal, cells[1], &p.pid.to_string(), true, None);
                cell_text(ui, pal, cells[2], p.state_text(), false, None);
                cell_text(ui, pal, cells[3], &p.user, false, if p.uid == self.me { None } else { Some(pal.dim) });
                cell_text(ui, pal, cells[4], &format!("{:.1}%", p.cpu), true, None);
                cell_text(ui, pal, cells[5], &mb(p.mem), true, None);
                cell_text(ui, pal, cells[6], &p.threads.to_string(), true, None);
                cell_text(ui, pal, cells[7], priority(p.nice), false, None);
                cell_text(ui, pal, cells[8], &p.cmd, false, Some(pal.dim));
                if resp.clicked() {
                    self.sel_pid = Some(p.pid);
                }
                if resp.double_clicked() {
                    self.props = Some(p.pid);
                }
                let p2 = (*p).clone();
                resp.context_menu(|ui| {
                    self.sel_pid = Some(p2.pid);
                    self.proc_menu(ui, &p2, &kids, acts);
                });
            }
        });
    }

    fn history(&mut self, ui: &mut egui::Ui, pal: &Palette, data: &Data) {
        ui.label(
            RichText::new(
                "What each app and your background programs have used since they started (Linux keeps nothing once a program closes).",
            )
            .color(pal.dim),
        );
        ui.add_space(6.0);
        let cols = [
            c(Col::Name, "Name", 0.0, false),
            c(Col::Started, "Running for", 120.0, true),
            c(Col::CpuTime, "CPU time", 110.0, true),
            c(Col::Read, "Read from disk", 130.0, true),
            c(Col::Written, "Written to disk", 130.0, true),
        ];
        header(ui, pal, &cols, &HashMap::new(), &mut self.hist_sort);
        let mut gs: Vec<&Group> = data.groups.iter().filter(|g| g.kind() != Kind::System && self.matches(&g.name)).collect();
        sort_groups(&mut gs, self.hist_sort);
        let w = widths(&cols, ui.available_width());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, gs.len(), |ui, range| {
            for (i, g) in gs[range.clone()].iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                if (range.start + i) % 2 == 1 {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, pal.row_alt);
                }
                if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.hover);
                }
                let mut x = rect.left();
                let cells: Vec<Rect> = w
                    .iter()
                    .map(|cw| {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H));
                        x += cw;
                        r
                    })
                    .collect();
                let label = if g.kind() == Kind::App { g.name.clone() } else { format!("{}  (background)", g.name) };
                cell_text(ui, pal, cells[0], &label, false, None);
                cell_text(ui, pal, cells[1], &duration(data.sys.uptime - g.started), true, None);
                cell_text(ui, pal, cells[2], &duration(g.cpu_time), true, None);
                cell_text(ui, pal, cells[3], &size(g.read), true, None);
                cell_text(ui, pal, cells[4], &size(g.written), true, None);
            }
        });
    }

    fn startup(&mut self, ui: &mut egui::Ui, pal: &Palette, data: &Data, acts: &mut Vec<Act>) {
        let sel = self.sel_startup.as_ref().and_then(|id| data.startup.iter().find(|i| &i.id == id)).cloned();
        command_bar(ui, |ui| {
            if ui.add_enabled(sel.as_ref().is_some_and(|i| !i.enabled), egui::Button::new("Enable")).clicked() {
                acts.push(Act::Startup(sel.clone().unwrap(), true));
            }
            if ui.add_enabled(sel.as_ref().is_some_and(|i| i.enabled), egui::Button::new("Disable")).clicked() {
                acts.push(Act::Startup(sel.clone().unwrap(), false));
            }
        });
        if data.startup_at.is_none() {
            ui.label(RichText::new("Reading...").color(pal.dim));
            return;
        }
        let cols = [
            c(Col::Name, "Name", 260.0, false),
            c(Col::Desc, "Description", 0.0, false),
            c(Col::Kind, "Kind", 150.0, false),
            c(Col::Status, "Status", 190.0, false),
        ];
        header(ui, pal, &cols, &HashMap::new(), &mut self.startup_sort);
        let mut items: Vec<&startup::Item> =
            data.startup.iter().filter(|i| self.matches(&i.name) || self.matches(&i.comment) || self.matches(&i.id)).collect();
        items.sort_by(|a, b| {
            let o = match self.startup_sort.0 {
                Col::Status => a.enabled.cmp(&b.enabled),
                Col::Kind => a.source.cmp(&b.source),
                Col::Desc => a.comment.cmp(&b.comment),
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            };
            if self.startup_sort.1 {
                o.reverse()
            } else {
                o
            }
        });
        let w = widths(&cols, ui.available_width());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, items.len(), |ui, range| {
            for it in &items[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                if self.sel_startup.as_deref() == Some(it.id.as_str()) {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.select);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.hover);
                }
                let mut x = rect.left();
                let cells: Vec<Rect> = w
                    .iter()
                    .map(|cw| {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H));
                        x += cw;
                        r
                    })
                    .collect();
                let icon_r = Rect::from_center_size(pos2(cells[0].left() + 16.0, cells[0].center().y), vec2(16.0, 16.0));
                if !it.icon.is_empty() || it.source == Some(startup::Source::Autostart) {
                    sysicon::paint_app(ui.painter(), icon_r, &[it.icon.as_str(), sysicon::DEFAULT_APP_ICON]);
                }
                cell_text(ui, pal, Rect::from_min_max(pos2(cells[0].left() + 22.0, cells[0].top()), cells[0].max), &it.name, false, None);
                cell_text(ui, pal, cells[1], &it.comment, false, Some(pal.dim));
                cell_text(
                    ui,
                    pal,
                    cells[2],
                    if it.source == Some(startup::Source::Service) { "Background service" } else { "App" },
                    false,
                    None,
                );
                let status = if !it.note.is_empty() {
                    format!("{} ({})", if it.enabled { "Enabled" } else { "Disabled" }, it.note)
                } else if it.enabled {
                    "Enabled".into()
                } else {
                    "Disabled".into()
                };
                cell_text(ui, pal, cells[3], &status, false, Some(if it.enabled { pal.green } else { pal.dim }));
                if resp.clicked() {
                    self.sel_startup = Some(it.id.clone());
                }
                let it2 = (*it).clone();
                resp.context_menu(|ui| {
                    if ui.button(if it2.enabled { "Disable" } else { "Enable" }).clicked() {
                        acts.push(Act::Startup(it2.clone(), !it2.enabled));
                        ui.close();
                    }
                });
            }
        });
    }

    fn users(&mut self, ui: &mut egui::Ui, pal: &Palette, dark: bool, data: &Data) {
        let mem_total = data.sys.mem.total.max(1);
        let cols = [
            c(Col::Name, "User", 0.0, false),
            c(Col::Status, "Status", 120.0, false),
            c(Col::Cpu, "CPU", 80.0, true),
            c(Col::Mem, "Memory", 100.0, true),
            c(Col::Pid, "Processes", 100.0, true),
        ];
        let mut sort = (Col::Name, false);
        header(ui, pal, &cols, &HashMap::new(), &mut sort);
        let mut sessions = data.sessions.clone();
        if !sessions.iter().any(|s| s.uid == self.me) {
            sessions.push(Session {
                uid: self.me,
                user: data.procs.iter().find(|p| p.uid == self.me).map(|p| p.user.clone()).unwrap_or_default(),
                state: "active".into(),
            });
        }
        let w = widths(&cols, ui.available_width());
        for s in sessions.iter().filter(|s| self.matches(&s.user)) {
            let mine: Vec<&Proc> = data.procs.iter().filter(|p| p.uid == s.uid && !p.kernel).collect();
            let cpu: f32 = mine.iter().map(|p| p.cpu).sum();
            let mem: u64 = mine.iter().map(|p| p.mem).sum();
            let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H + 4.0), Sense::hover());
            let mut x = rect.left();
            let cells: Vec<Rect> = w
                .iter()
                .map(|cw| {
                    let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H + 4.0));
                    x += cw;
                    r
                })
                .collect();
            ui.painter().rect_filled(cells[2].shrink2(vec2(0.0, 1.0)), CornerRadius::ZERO, heat(pal, dark, cpu / 25.0));
            ui.painter().rect_filled(
                cells[3].shrink2(vec2(0.0, 1.0)),
                CornerRadius::ZERO,
                heat(pal, dark, (mem as f64 / mem_total as f64 * 4.0) as f32),
            );
            let you = if s.uid == self.me { "  (you)" } else { "" };
            cell_text(ui, pal, cells[0], &format!("{}{you}", s.user), false, None);
            let state = match s.state.as_str() {
                "active" => "Signed in (in front)",
                "online" => "Signed in",
                "closing" => "Signing out",
                _ => "Signed in",
            };
            cell_text(ui, pal, cells[1], state, false, Some(pal.dim));
            cell_text(ui, pal, cells[2], &format!("{cpu:.1}%"), true, None);
            cell_text(ui, pal, cells[3], &mb(mem), true, None);
            cell_text(ui, pal, cells[4], &mine.len().to_string(), true, None);
            // the user's apps under them
            for g in data.groups.iter().filter(|g| g.uid == s.uid && g.kind() == Kind::App) {
                let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::hover());
                let mut x = rect.left();
                let cells: Vec<Rect> = w
                    .iter()
                    .map(|cw| {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H));
                        x += cw;
                        r
                    })
                    .collect();
                cell_text(
                    ui,
                    pal,
                    Rect::from_min_max(pos2(cells[0].left() + 24.0, cells[0].top()), cells[0].max),
                    &g.name,
                    false,
                    Some(pal.dim),
                );
                cell_text(ui, pal, cells[2], &format!("{:.1}%", g.cpu), true, Some(pal.dim));
                cell_text(ui, pal, cells[3], &mb(g.mem), true, Some(pal.dim));
                cell_text(ui, pal, cells[4], &g.pids.len().to_string(), true, Some(pal.dim));
            }
        }
    }

    fn services(&mut self, ui: &mut egui::Ui, pal: &Palette, data: &Data, acts: &mut Vec<Act>) {
        let sel = self.sel_service.as_ref().and_then(|n| data.services.iter().find(|s| &s.name == n)).cloned();
        command_bar(ui, |ui| {
            let running = sel.as_ref().is_some_and(|s| s.running());
            if ui.add_enabled(sel.is_some() && !running, egui::Button::new("Start")).clicked() {
                acts.push(Act::Service(sel.clone().unwrap(), services::Action::Start));
            }
            if ui.add_enabled(running, egui::Button::new("Stop")).clicked() {
                acts.push(Act::Service(sel.clone().unwrap(), services::Action::Stop));
            }
            if ui.add_enabled(running, egui::Button::new("Restart")).clicked() {
                acts.push(Act::Service(sel.clone().unwrap(), services::Action::Restart));
            }
            ui.label(RichText::new("The system's services ask for your password.").small().color(pal.dim));
        });
        if data.services_at.is_none() {
            ui.label(RichText::new("Reading...").color(pal.dim));
            return;
        }
        let cols = [
            c(Col::Name, "Name", 280.0, false),
            c(Col::Desc, "Description", 0.0, false),
            c(Col::Status, "Status", 130.0, false),
            c(Col::Startup, "Starts", 120.0, false),
            c(Col::Kind, "Whose", 80.0, false),
        ];
        header(ui, pal, &cols, &HashMap::new(), &mut self.svc_sort);
        let mut v: Vec<&services::Service> =
            data.services.iter().filter(|s| self.matches(&s.display()) || self.matches(&s.description)).collect();
        v.sort_by(|a, b| {
            let o = match self.svc_sort.0 {
                Col::Desc => a.description.cmp(&b.description),
                Col::Status => a.status().cmp(b.status()),
                Col::Startup => a.startup().cmp(b.startup()),
                Col::Kind => a.scope().cmp(&b.scope()),
                _ => a.display().to_lowercase().cmp(&b.display().to_lowercase()),
            };
            if self.svc_sort.1 {
                o.reverse()
            } else {
                o
            }
        });
        let w = widths(&cols, ui.available_width());
        egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(ui, ROW_H, v.len(), |ui, range| {
            for (i, s) in v[range.clone()].iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click());
                if (range.start + i) % 2 == 1 {
                    ui.painter().rect_filled(rect, CornerRadius::ZERO, pal.row_alt);
                }
                if self.sel_service.as_deref() == Some(s.name.as_str()) {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.select);
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, CornerRadius::same(4), pal.hover);
                }
                let mut x = rect.left();
                let cells: Vec<Rect> = w
                    .iter()
                    .map(|cw| {
                        let r = Rect::from_min_size(pos2(x, rect.top()), vec2(*cw, ROW_H));
                        x += cw;
                        r
                    })
                    .collect();
                cell_text(ui, pal, cells[0], &s.display(), false, None);
                cell_text(ui, pal, cells[1], &s.description, false, Some(pal.dim));
                let col = match s.status() {
                    "Running" => pal.green,
                    "Failed" => pal.red,
                    _ => pal.dim,
                };
                cell_text(ui, pal, cells[2], s.status(), false, Some(col));
                cell_text(ui, pal, cells[3], s.startup(), false, None);
                cell_text(ui, pal, cells[4], s.scope().label(), false, Some(pal.dim));
                if resp.clicked() {
                    self.sel_service = Some(s.name.clone());
                }
                let s2 = (*s).clone();
                resp.context_menu(|ui| {
                    self.sel_service = Some(s2.name.clone());
                    for (label, a, ok) in [
                        ("Start", services::Action::Start, !s2.running()),
                        ("Stop", services::Action::Stop, s2.running()),
                        ("Restart", services::Action::Restart, s2.running()),
                        ("Start automatically", services::Action::Enable, s2.file_state == "disabled"),
                        ("Don't start automatically", services::Action::Disable, s2.file_state.starts_with("enabled")),
                    ] {
                        if ui.add_enabled(ok, egui::Button::new(label)).clicked() {
                            acts.push(Act::Service(s2.clone(), a));
                            ui.close();
                        }
                    }
                    ui.separator();
                    if ui.button("Search online").clicked() {
                        acts.push(Act::Open(format!("https://duckduckgo.com/?q={}", s2.name.replace(' ', "+"))));
                        ui.close();
                    }
                });
            }
        });
    }
}

// ---------------------------------------------------------------- Performance

/// A line graph of `values` (newest at the right) scaled to `max`, filled under the line.
fn graph(ui: &egui::Ui, pal: &Palette, rect: Rect, values: &VecDeque<f32>, max: f32, color: Color32, fill: bool) {
    let p = ui.painter();
    p.rect_stroke(rect, CornerRadius::ZERO, Stroke::new(1.0, color.gamma_multiply(0.7)), StrokeKind::Inside);
    for i in 1..10 {
        let x = rect.left() + rect.width() * i as f32 / 10.0;
        p.vline(x, rect.y_range(), Stroke::new(1.0, pal.border.gamma_multiply(0.5)));
    }
    for i in 1..4 {
        let y = rect.top() + rect.height() * i as f32 / 4.0;
        p.hline(rect.x_range(), y, Stroke::new(1.0, pal.border.gamma_multiply(0.5)));
    }
    if values.len() < 2 || max <= 0.0 {
        return;
    }
    let step = rect.width() / (HISTORY - 1) as f32;
    let start = HISTORY - values.len();
    let pts: Vec<egui::Pos2> = values
        .iter()
        .enumerate()
        .map(|(i, v)| pos2(rect.left() + (start + i) as f32 * step, rect.bottom() - (v / max).clamp(0.0, 1.0) * rect.height()))
        .collect();
    if fill {
        let mut mesh = egui::Mesh::default();
        let fc = color.gamma_multiply(0.25);
        for (i, pt) in pts.iter().enumerate() {
            mesh.colored_vertex(*pt, fc);
            mesh.colored_vertex(pos2(pt.x, rect.bottom()), fc);
            if i > 0 {
                let b = (i as u32 - 1) * 2;
                mesh.add_triangle(b, b + 1, b + 2);
                mesh.add_triangle(b + 1, b + 2, b + 3);
            }
        }
        p.add(mesh);
    }
    p.add(egui::Shape::line(pts, Stroke::new(1.5, color)));
}

fn peak(q: &VecDeque<f32>) -> f32 {
    q.iter().copied().fold(0.0, f32::max)
}

/// A scale for a rate graph: the next round number above the peak (at least 100 KB/s).
fn rate_scale(peak: f32) -> f32 {
    let mut s = 100.0 * 1024.0;
    while s < peak {
        s *= 2.0;
    }
    s
}

fn stat(ui: &mut egui::Ui, pal: &Palette, label: &str, value: &str, big: bool) {
    ui.vertical(|ui| {
        ui.add(egui::Label::new(RichText::new(label).small().color(pal.dim)).extend());
        ui.add(egui::Label::new(RichText::new(value).size(if big { 20.0 } else { 14.0 })).extend());
    });
}

/// Cache sizes count in 1024s, as processor makers and Windows give them.
fn cache(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{} KB", bytes / 1024)
    }
}

impl App {
    fn performance(&mut self, ui: &mut egui::Ui, pal: &Palette, data: &Data, acts: &mut Vec<Act>) {
        let _ = acts;
        let cpu_col = pal.accent;
        let mem_col = Color32::from_rgb(168, 92, 214);
        let disk_col = pal.green;
        let net_col = Color32::from_rgb(206, 120, 40);
        let gpu_col = Color32::from_rgb(60, 140, 220);
        let h = &data.hist;
        let mem = &data.sys.mem;
        let mut items: Vec<PerfEntry> = vec![
            (
                PerfItem::Cpu,
                "CPU".into(),
                format!("{:.0}%  {}", data.sys.cpu, data.sys.speed.map(|s| format!("{:.2} GHz", s as f32 / 1000.0)).unwrap_or_default()),
                cpu_col,
                &h.cpu,
                100.0,
            ),
            (
                PerfItem::Mem,
                "Memory".into(),
                format!("{}/{} ({:.0}%)", gib(mem.used()), gib(mem.total), mem.used() as f64 / mem.total.max(1) as f64 * 100.0),
                mem_col,
                &h.mem,
                100.0,
            ),
        ];
        let empty = VecDeque::new();
        for (i, d) in data.sys.disks.iter().enumerate() {
            let q = h.disks.get(&d.name).map(|e| &e.0).unwrap_or(&empty);
            items.push((PerfItem::Disk(i), format!("Disk {i} ({})", d.name), format!("{}  {:.0}%", d.kind, d.active), disk_col, q, 100.0));
        }
        for (i, a) in data.sys.adapters.iter().enumerate() {
            let q = h.nets.get(&a.name).map(|e| &e.1).unwrap_or(&empty);
            items.push((
                PerfItem::Net(i),
                a.kind.clone(),
                format!("{}  S: {} R: {}", a.name, bits(a.send_rate), bits(a.recv_rate)),
                net_col,
                q,
                rate_scale(peak(q)),
            ));
        }
        if let Some(g) = &data.sys.gpu {
            items.push((
                PerfItem::Gpu,
                "GPU 0".into(),
                format!("{}  {:.0}%", g.name.trim_start_matches("NVIDIA ").trim_start_matches("GeForce "), g.util),
                gpu_col,
                &h.gpu,
                100.0,
            ));
        } else if h.gpu.is_empty() {
            // the first look at the card comes with the next refresh
        }
        let avail = ui.available_rect_before_wrap();
        let list_w = 230.0;
        let list = Rect::from_min_size(avail.min, vec2(list_w, avail.height()));
        let main = Rect::from_min_max(pos2(avail.left() + list_w + 14.0, avail.top()), avail.max);
        ui.scope_builder(egui::UiBuilder::new().max_rect(list), |ui| {
            egui::ScrollArea::vertical().id_salt("perf-list").show(ui, |ui| {
                for (item, title, sub, col, q, max) in &items {
                    let (rect, resp) = ui.allocate_exact_size(vec2(list_w - 8.0, 62.0), Sense::click());
                    if self.perf == *item {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), pal.select);
                    } else if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), pal.hover);
                    }
                    let g = Rect::from_min_size(rect.left_top() + vec2(8.0, 10.0), vec2(70.0, 42.0));
                    graph(ui, pal, g, q, *max, *col, true);
                    ui.painter().text(
                        pos2(g.right() + 10.0, rect.top() + 18.0),
                        Align2::LEFT_CENTER,
                        title,
                        FontId::proportional(14.0),
                        pal.text,
                    );
                    ui.painter().with_clip_rect(rect).text(
                        pos2(g.right() + 10.0, rect.top() + 40.0),
                        Align2::LEFT_CENTER,
                        sub,
                        FontId::proportional(11.5),
                        pal.dim,
                    );
                    if resp.clicked() {
                        self.perf = *item;
                    }
                }
                if data.sys.gpu.is_none() {
                    ui.label(RichText::new("Looking at the graphics card...").small().color(pal.dim));
                }
            });
        });
        ui.scope_builder(egui::UiBuilder::new().max_rect(main), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("perf-main")
                .show(ui, |ui| self.perf_detail(ui, pal, data, cpu_col, mem_col, disk_col, net_col, gpu_col));
        });
    }

    #[allow(clippy::too_many_arguments)]
    fn perf_detail(
        &mut self,
        ui: &mut egui::Ui,
        pal: &Palette,
        data: &Data,
        cpu_col: Color32,
        mem_col: Color32,
        disk_col: Color32,
        net_col: Color32,
        gpu_col: Color32,
    ) {
        let h = &data.hist;
        let w = ui.available_width();
        let title = |ui: &mut egui::Ui, left: &str, right: &str| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(left).size(24.0).strong());
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(right).size(15.0).color(pal.dim));
                });
            });
        };
        let caption = |ui: &mut egui::Ui, left: &str, right: &str| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(left).small().color(pal.dim));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new(right).small().color(pal.dim));
                });
            });
        };
        match self.perf {
            PerfItem::Cpu => {
                let ci = &self.cpu_info;
                title(ui, "CPU", &ci.model);
                ui.horizontal(|ui| {
                    caption(ui, "% Utilization over 60 seconds", "100%");
                });
                ui.horizontal(|ui| {
                    ui.selectable_value(&mut self.per_core, false, "Overall");
                    ui.selectable_value(&mut self.per_core, true, "Each logical processor");
                });
                if self.per_core && !h.cores.is_empty() {
                    let n = h.cores.len();
                    let cols = ((n as f32).sqrt().ceil() as usize).max(1);
                    let rows = n.div_ceil(cols);
                    let cw = (w - 6.0 * (cols as f32 - 1.0)) / cols as f32;
                    let ch = (cw * 0.55).clamp(40.0, 110.0);
                    let (area, _) = ui.allocate_exact_size(vec2(w, rows as f32 * (ch + 6.0)), Sense::hover());
                    for (i, q) in h.cores.iter().enumerate() {
                        let r = Rect::from_min_size(
                            area.left_top() + vec2((i % cols) as f32 * (cw + 6.0), (i / cols) as f32 * (ch + 6.0)),
                            vec2(cw, ch),
                        );
                        graph(ui, pal, r, q, 100.0, cpu_col, true);
                    }
                } else {
                    let (r, _) = ui.allocate_exact_size(vec2(w, 300.0), Sense::hover());
                    graph(ui, pal, r, &h.cpu, 100.0, cpu_col, true);
                }
                caption(ui, "60 seconds", "0");
                ui.add_space(10.0);
                let procs = data.procs.iter().filter(|p| !p.kernel).count();
                ui.columns(2, |cols| {
                    egui::Grid::new("cpu-live").num_columns(3).spacing([24.0, 10.0]).show(&mut cols[0], |ui| {
                        stat(ui, pal, "Utilization", &format!("{:.0}%", data.sys.cpu), true);
                        stat(
                            ui,
                            pal,
                            "Speed",
                            &data.sys.speed.map(|s| format!("{:.2} GHz", s as f32 / 1000.0)).unwrap_or("-".into()),
                            true,
                        );
                        ui.end_row();
                        stat(ui, pal, "Processes", &procs.to_string(), true);
                        stat(ui, pal, "Threads", &data.threads.to_string(), true);
                        ui.end_row();
                        stat(ui, pal, "Up time", &duration(data.sys.uptime), true);
                        ui.end_row();
                    });
                    egui::Grid::new("cpu-fixed").num_columns(2).spacing([16.0, 4.0]).show(&mut cols[1], |ui| {
                        for (k, v) in [
                            ("Base speed", ci.base_mhz.map(|m| format!("{:.2} GHz", m as f32 / 1000.0)).unwrap_or("-".into())),
                            ("Sockets", ci.sockets.to_string()),
                            ("Cores", ci.cores.to_string()),
                            ("Logical processors", ci.logical.to_string()),
                            ("Virtualization", if ci.virtualization { "Supported".into() } else { "Not supported".into() }),
                            ("L1 cache", cache(ci.l1)),
                            ("L2 cache", cache(ci.l2)),
                            ("L3 cache", cache(ci.l3)),
                        ] {
                            ui.label(RichText::new(k).color(pal.dim));
                            ui.label(v);
                            ui.end_row();
                        }
                    });
                });
            }
            PerfItem::Mem => {
                let m = &data.sys.mem;
                title(ui, "Memory", &gib(m.total));
                caption(ui, "Memory usage", &gib(m.total));
                let (r, _) = ui.allocate_exact_size(vec2(w, 260.0), Sense::hover());
                graph(ui, pal, r, &h.mem, 100.0, mem_col, true);
                caption(ui, "60 seconds", "0");
                ui.add_space(8.0);
                // composition: in use, cached, free
                ui.label(RichText::new("Memory composition").small().color(pal.dim));
                let (bar, _) = ui.allocate_exact_size(vec2(w, 30.0), Sense::hover());
                let t = m.total.max(1) as f32;
                let used_w = bar.width() * m.used() as f32 / t;
                let cached_w = bar.width() * (m.cached.min(m.available)) as f32 / t;
                ui.painter().rect_stroke(bar, CornerRadius::ZERO, Stroke::new(1.0, mem_col), StrokeKind::Inside);
                ui.painter().rect_filled(
                    Rect::from_min_size(bar.min, vec2(used_w, bar.height())),
                    CornerRadius::ZERO,
                    mem_col.gamma_multiply(0.6),
                );
                ui.painter().rect_filled(
                    Rect::from_min_size(bar.min + vec2(used_w, 0.0), vec2(cached_w, bar.height())),
                    CornerRadius::ZERO,
                    mem_col.gamma_multiply(0.2),
                );
                ui.add_space(10.0);
                egui::Grid::new("mem").num_columns(3).spacing([28.0, 10.0]).show(ui, |ui| {
                    stat(ui, pal, "In use", &gib(m.used()), true);
                    stat(ui, pal, "Available", &gib(m.available), true);
                    stat(ui, pal, "Cached", &gib(m.cached), true);
                    ui.end_row();
                    stat(ui, pal, "Committed", &format!("{}/{}", gib(m.committed), gib(m.commit_limit)), false);
                    stat(
                        ui,
                        pal,
                        "Swap in use",
                        &format!("{}/{}", gib(m.swap_total.saturating_sub(m.swap_free)), gib(m.swap_total)),
                        false,
                    );
                    ui.end_row();
                });
            }
            PerfItem::Disk(i) => {
                let Some(d) = data.sys.disks.get(i) else { return };
                let empty = (VecDeque::new(), VecDeque::new(), VecDeque::new());
                let q = h.disks.get(&d.name).unwrap_or(&empty);
                title(ui, &format!("Disk {i} ({})", d.name), &d.model);
                caption(ui, "Active time", "100%");
                let (r, _) = ui.allocate_exact_size(vec2(w, 200.0), Sense::hover());
                graph(ui, pal, r, &q.0, 100.0, disk_col, true);
                caption(ui, "60 seconds", "0");
                ui.add_space(8.0);
                let max = rate_scale(peak(&q.1).max(peak(&q.2)));
                caption(ui, "Disk transfer rate (read: solid, write: light)", &rate(max as f64));
                let (r, _) = ui.allocate_exact_size(vec2(w, 140.0), Sense::hover());
                graph(ui, pal, r, &q.1, max, disk_col, true);
                graph(ui, pal, r, &q.2, max, disk_col.gamma_multiply(0.5), false);
                ui.add_space(10.0);
                egui::Grid::new("disk").num_columns(3).spacing([28.0, 10.0]).show(ui, |ui| {
                    stat(ui, pal, "Active time", &format!("{:.0}%", d.active), true);
                    stat(ui, pal, "Read speed", &rate(d.read_rate), true);
                    stat(ui, pal, "Write speed", &rate(d.write_rate), true);
                    ui.end_row();
                    stat(ui, pal, "Capacity", &size(d.size), false);
                    stat(ui, pal, "Type", &d.kind, false);
                    ui.end_row();
                });
            }
            PerfItem::Net(i) => {
                let Some(a) = data.sys.adapters.get(i) else { return };
                let empty = (VecDeque::new(), VecDeque::new());
                let q = h.nets.get(&a.name).unwrap_or(&empty);
                title(ui, &a.kind, &a.name);
                let max = rate_scale(peak(&q.0).max(peak(&q.1)));
                caption(ui, "Throughput (receive: solid, send: light)", &bits(max as f64));
                let (r, _) = ui.allocate_exact_size(vec2(w, 300.0), Sense::hover());
                graph(ui, pal, r, &q.1, max, net_col, true);
                graph(ui, pal, r, &q.0, max, net_col.gamma_multiply(0.55), false);
                caption(ui, "60 seconds", "0");
                ui.add_space(10.0);
                egui::Grid::new("net").num_columns(3).spacing([28.0, 10.0]).show(ui, |ui| {
                    stat(ui, pal, "Send", &bits(a.send_rate), true);
                    stat(ui, pal, "Receive", &bits(a.recv_rate), true);
                    stat(
                        ui,
                        pal,
                        "Link speed",
                        &a.speed
                            .map(|s| if s >= 1000 { format!("{:.1} Gbps", s as f32 / 1000.0) } else { format!("{s} Mbps") })
                            .unwrap_or("-".into()),
                        true,
                    );
                    ui.end_row();
                });
            }
            PerfItem::Gpu => {
                let Some(g) = &data.sys.gpu else {
                    ui.label("No graphics card numbers yet.");
                    return;
                };
                title(ui, "GPU 0", &g.name);
                caption(ui, "Utilization", "100%");
                let (r, _) = ui.allocate_exact_size(vec2(w, 280.0), Sense::hover());
                graph(ui, pal, r, &h.gpu, 100.0, gpu_col, true);
                caption(ui, "60 seconds", "0");
                ui.add_space(10.0);
                egui::Grid::new("gpu").num_columns(3).spacing([28.0, 10.0]).show(ui, |ui| {
                    stat(ui, pal, "Utilization", &format!("{:.0}%", g.util), true);
                    stat(ui, pal, "Dedicated memory", &format!("{}/{}", gib(g.mem_used), gib(g.mem_total)), true);
                    stat(ui, pal, "Temperature", &g.temp.map(|t| format!("{t:.0} °C")).unwrap_or("-".into()), true);
                    ui.end_row();
                    stat(ui, pal, "Power", &g.power.map(|p| format!("{p:.0} W")).unwrap_or("-".into()), false);
                    stat(ui, pal, "Clock", &g.clock.map(|c| format!("{c} MHz")).unwrap_or("-".into()), false);
                    ui.end_row();
                });
            }
        }
    }
}

// ---------------------------------------------------------------- start

fn pidfile() -> std::path::PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{APP_ID}.pid"))
}

/// An open Task Manager is brought to the front instead of opening a second one (Ctrl+Shift+Esc again).
fn already_open() -> bool {
    let Some(pid) = std::fs::read_to_string(pidfile()).ok().and_then(|s| s.trim().parse::<u32>().ok()) else { return false };
    if pid == std::process::id() {
        return false;
    }
    let (Ok(theirs), Ok(ours)) = (std::fs::read_link(format!("/proc/{pid}/exe")), std::env::current_exe()) else { return false };
    if theirs.to_string_lossy().trim_end_matches(" (deleted)") != ours.to_string_lossy() {
        return false;
    }
    if let Some(w) = desk::windows().into_iter().find(|w| w.pid == pid) {
        desk::focus_window(&w.address);
    }
    true
}

/// `--tab performance` (or processes, history, startup, users, details, services): the tab it opens on.
fn start_tab(args: &[String]) -> Tab {
    let want = args.iter().position(|a| a == "--tab").and_then(|i| args.get(i + 1)).map(|s| s.to_lowercase()).unwrap_or_default();
    TABS.iter()
        .find(|(_, name)| name.to_lowercase().replace("app ", "").starts_with(&want) && !want.is_empty())
        .map(|t| t.0)
        .unwrap_or(Tab::Processes)
}

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().collect();
    let tab = start_tab(&args);
    if already_open() {
        return Ok(());
    }
    let _ = std::fs::write(pidfile(), std::process::id().to_string());
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 720.0])
            .with_min_inner_size([760.0, 480.0])
            .with_title("Task Manager")
            .with_app_id(APP_ID)
            .with_decorations(false),
        ..Default::default()
    };
    let r = eframe::run_native("Task Manager", options, Box::new(move |cc| Ok(Box::new(App::new(cc, tab)))));
    let _ = std::fs::remove_file(pidfile());
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_read_like_windows() {
        assert_eq!(mb(150 * 1024 * 1024), "150.0 MB");
        assert_eq!(mb(12 * 1024 * 1024 * 1024), "12.0 GB");
        assert_eq!(rate(0.0), "0.0 MB/s");
        assert_eq!(rate(20_480.0), "20 KB/s");
        assert_eq!(rate(3.0 * 1_048_576.0), "3.0 MB/s");
        assert_eq!(bits(125_000.0), "1.0 Mbps");
        assert_eq!(duration(3725.0), "1:02:05");
        assert_eq!(duration(90061.0), "1:01:01:01");
        assert_eq!(priority(0), "Normal");
        assert_eq!(priority(-5), "High");
        assert_eq!(priority(19), "Low");
        assert_eq!(size(1_500_000_000), "1.5 GB");
        assert_eq!((cache(512 * 1024), cache(16 << 20)), ("512 KB".to_string(), "16.0 MB".to_string()));
    }

    #[test]
    fn heat_grows() {
        let pal = theme::palette(true);
        assert_eq!(heat(&pal, true, 0.0), Color32::TRANSPARENT);
        assert!(heat(&pal, true, 1.0).a() > heat(&pal, true, 0.2).a());
        assert!(heat(&pal, false, 2.0) == heat(&pal, false, 1.0), "clamped");
    }

    #[test]
    fn sessions() {
        let v = parse_sessions(
            r#"[{"session":"3","uid":1000,"user":"me","seat":"seat0","state":"active"},{"session":"4","uid":1000,"user":"me","state":"online"},{"session":"9","uid":1001,"user":"agent","state":"online"}]"#,
        );
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].user.as_str(), v[0].state.as_str()), ("me", "active"));
        assert!(parse_sessions("x").is_empty());
    }

    #[test]
    fn running_a_task_checks_the_program() {
        assert!(run_task("", false).is_err());
        assert!(run_task("no-such-program-xyz --flag", false).unwrap_err().contains("no-such-program-xyz"));
    }

    #[test]
    fn rate_scales_round_up() {
        assert_eq!(rate_scale(0.0), 102_400.0);
        assert_eq!(rate_scale(150_000.0), 204_800.0);
        let mut q = VecDeque::new();
        for i in 0..70 {
            push(&mut q, i as f32);
        }
        assert_eq!(q.len(), HISTORY);
        assert_eq!(peak(&q), 69.0);
    }

    #[test]
    fn start_tabs() {
        let a = |s: &str| start_tab(&["x".into(), "--tab".into(), s.into()]);
        assert_eq!(a("performance"), Tab::Performance);
        assert_eq!(a("history"), Tab::History);
        assert_eq!(a("Services"), Tab::Services);
        assert_eq!(start_tab(&["x".into()]), Tab::Processes);
        assert_eq!(a(""), Tab::Processes);
    }

    #[test]
    fn widths_share_what_is_left() {
        let cols = [c(Col::Name, "Name", 0.0, false), c(Col::Cpu, "CPU", 80.0, true), c(Col::Mem, "Memory", 100.0, true)];
        assert_eq!(widths(&cols, 500.0), vec![320.0, 80.0, 100.0]);
        assert_eq!(widths(&cols, 200.0)[0], 120.0, "the name column keeps a minimum");
    }
}
