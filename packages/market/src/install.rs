//! Installing, reinstalling, removing and updating inside the Marketplace, with progress. The command runs as the user, with its output
//! read line by line; anything that needs administrator rights asks through the desktop's own password box (pkexec / polkit), so no terminal
//! is needed. Only apps from the operating system's own (reviewed) catalog are installed this way: for anything else the Marketplace keeps opening a
//! terminal, where the prompts (an AUR package's build file, for instance) can be read before saying yes.

use std::io::{BufRead, BufReader, Read};
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::catalog::{Entry, Source};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Install,
    Reinstall,
    Remove,
    UpdateAll,
}

impl Op {
    pub fn verb(self) -> &'static str {
        match self {
            Op::Install => "Installing",
            Op::Reinstall => "Reinstalling",
            Op::Remove => "Removing",
            Op::UpdateAll => "Updating",
        }
    }
    pub fn done(self) -> &'static str {
        match self {
            Op::Install => "installed",
            Op::Reinstall => "reinstalled",
            Op::Remove => "removed",
            Op::UpdateAll => "updated",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    /// Waiting for the job before it: one package operation runs at a time (they share the package database's lock).
    Queued,
    Running,
    Done,
    Failed(String),
    /// The user cancelled it; whatever it had downloaded or installed was removed.
    Cancelled,
}

impl Status {
    /// Running or waiting to run.
    pub fn active(&self) -> bool {
        matches!(self, Status::Queued | Status::Running)
    }
}

#[derive(Clone, Debug)]
pub struct Job {
    /// The entry's id (or "*update*").
    pub id: String,
    pub title: String,
    pub op: Op,
    pub log: Vec<String>,
    /// What it is doing now, in words.
    pub phase: String,
    /// How far along, when the tool says (otherwise the bar just moves).
    pub frac: Option<f32>,
    pub status: Status,
    /// Progress of the downloading part and of the installing part (pacman does them one after the other).
    pub dl: Option<f32>,
    pub inst: Option<f32>,
    pub dl_files: usize,
    pub dl_done: usize,
    pub dl_current: String,
    /// An AUR build through yay: how many packages it builds (its "(n/m) Parsing SRCINFO" lines), how many it has started, and
    /// the one it is on. While it is known, the bar follows the builds, not pacman's installs between them.
    pub aur_total: usize,
    pub aur_started: usize,
    pub aur_current: String,
    pub started: Instant,
    /// When the progress last moved (the bar creeps on slowly while a build gives no numbers).
    pub moved: Instant,
    pub cancel_requested: bool,
    pub cancel_file: PathBuf,
    pub pid: Option<i32>,
}

pub type Shared = Arc<Mutex<Job>>;

/// A stand-in for `sudo` that asks through the desktop's password box (pkexec), for yay and for an OS's component command (AGORA_SUDO).
const SUDO_WRAPPER: &str = "/usr/lib/agora/sudo";
/// Runs one narrow pacman command as root and can cancel and clean it up (see packages/agora-helpers; the Python script it replaced is in git, packages/market/helpers/python/pkg-root until 2026-11-15).
const ROOT_HELPER: &str = "/usr/lib/agora/pkg-root";
/// Replaced by the job's cancel file when it starts.
const CANCEL: &str = "{CANCEL}";

/// The command for an entry and operation, as program + arguments; None when it cannot be done here (a plugin).
pub fn command(e: &Entry, op: Op) -> Option<(String, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let with = |mut base: Vec<String>, pk: &[String]| {
        base.extend(pk.iter().cloned());
        base
    };
    Some(match (&e.source, op) {
        (Source::Pacman(p), Op::Install) => {
            ("pkexec".into(), with(s(&[ROOT_HELPER, CANCEL, "pacman", "-S", "--needed", "--noconfirm"]), p))
        }
        (Source::Pacman(p), Op::Reinstall) => ("pkexec".into(), with(s(&[ROOT_HELPER, CANCEL, "pacman", "-S", "--noconfirm"]), p)),
        (Source::Pacman(p), Op::Remove) | (Source::Aur(p), Op::Remove) => {
            ("pkexec".into(), with(s(&[ROOT_HELPER, CANCEL, "pacman", "-Rs", "--noconfirm"]), p))
        }
        (Source::Aur(p), Op::Install) => ("yay".into(), with(s(&["-S", "--needed", "--noconfirm", "--sudo", SUDO_WRAPPER]), p)),
        (Source::Aur(p), Op::Reinstall) => ("yay".into(), with(s(&["-S", "--noconfirm", "--sudo", SUDO_WRAPPER]), p)),
        (Source::Flatpak(a), Op::Install) => ("flatpak".into(), s(&["install", "--user", "-y", "--noninteractive", "flathub", a])),
        (Source::Flatpak(a), Op::Reinstall) => {
            ("flatpak".into(), s(&["install", "--user", "-y", "--noninteractive", "--reinstall", "flathub", a]))
        }
        (Source::Flatpak(a), Op::Remove) => ("flatpak".into(), s(&["uninstall", "--user", "-y", "--noninteractive", a])),
        (Source::Component(c), op) if op != Op::UpdateAll => {
            let mut cmd = crate::catalog::component_command()?;
            let prog = cmd.remove(0);
            cmd.extend(s(&[if op == Op::Remove { "remove" } else { "install" }, c]));
            (prog, cmd)
        }
        (_, Op::UpdateAll) => {
            // One yay run does it all: the repositories (its refresh becomes a full upgrade in the helper), then the AUR. Every root
            // step then comes from the same process, so the password box asks once (polkit keeps it a few minutes for the helper only).
            // Nothing from outside the repositories is taken from the AUR unless it is known to come from there: the operating
            // system's own builds and anything installed by hand keep what they are.
            let skip = crate::catalog::aur_ignore(
                &crate::catalog::foreign_packages(),
                &crate::catalog::aur_allowed(),
                &crate::catalog::local_builds(),
            )
            .join(",");
            let mut a = s(&["-Syu", "--noconfirm", "--sudo", SUDO_WRAPPER]);
            if !skip.is_empty() {
                a.extend(s(&["--ignore", &skip]));
            }
            ("yay".into(), a)
        }
        (Source::Plugin { .. }, _) | (Source::Component(_), _) => return None,
    })
}

/// The overall progress from what is known: downloading and installing are two stretches (when there was a download, each is half).
fn overall(job: &Job) -> Option<f32> {
    match (job.dl, job.inst) {
        (Some(d), Some(i)) => Some(0.5 * d + 0.5 * i),
        (Some(d), None) => Some(0.5 * d),
        (None, Some(i)) => Some(i),
        (None, None) => None,
    }
}

fn set_progress(job: &mut Job) {
    let new = overall(job);
    if new != job.frac {
        // never backwards
        job.frac = match (job.frac, new) {
            (Some(old), Some(n)) if n < old => Some(old),
            (_, n) => n,
        };
        job.moved = Instant::now();
    }
}

/// The line without terminal colour codes (ESC [ ... letter).
fn plain(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.next() == Some('[') {
                for x in chars.by_ref() {
                    if x.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// "(n/m)" somewhere in a line: (n, m).
fn count_in(line: &str) -> Option<(usize, usize)> {
    let (_, rest) = line.split_once('(')?;
    let (inner, _) = rest.split_once(')')?;
    let (n, m) = inner.split_once('/')?;
    Some((n.trim().parse().ok()?, m.trim().parse().ok()?))
}

/// The bar during an AUR build: the packages built so far plus how far the current one is (`stage`, 0 to 1), never backwards.
fn aur_progress(job: &mut Job, stage: f32) {
    let total = job.aur_total.max(job.aur_started).max(1) as f32;
    let v = ((job.aur_started.saturating_sub(1) as f32 + stage) / total * 0.97).clamp(0.0, 1.0);
    if job.frac.map(|f| v > f).unwrap_or(true) {
        job.frac = Some(v);
        job.moved = Instant::now();
    }
}

/// What yay and makepkg say about an AUR build; true when the line was one of theirs.
fn read_aur(job: &mut Job, line: &str) -> bool {
    // yay: "(1/2) Parsing SRCINFO: name", "(1/2) Downloaded PKGBUILD: name" (with or without ":: " in front)
    if line.contains("Parsing SRCINFO") || line.contains("Downloaded PKGBUILD") || line.contains("Failed to download PKGBUILD") {
        if let Some((_, m)) = count_in(line) {
            job.aur_total = job.aur_total.max(m);
        }
        job.phase = "Downloading the build files".into();
        return true;
    }
    let named = |prefix: &str| {
        line.strip_prefix("==> ").and_then(|l| l.strip_prefix(prefix)).map(|r| r.split_whitespace().next().unwrap_or("").to_string())
    };
    let which = |job: &Job| {
        if job.aur_total > 1 {
            format!("{} ({} of {})", job.aur_current, job.aur_started.min(job.aur_total), job.aur_total)
        } else {
            job.aur_current.clone()
        }
    };
    if let Some(name) = named("Making package: ") {
        job.aur_started += 1;
        job.aur_current = name;
        job.phase = format!("Building {}", which(job));
        aur_progress(job, 0.02);
        return true;
    }
    if job.aur_current.is_empty() {
        return false;
    }
    for (needle, word, stage) in [
        ("==> Starting build()", "Building", 0.1),
        ("==> Starting package()", "Packaging", 0.7),
        ("==> Finished making:", "Installing", 0.85),
    ] {
        if line.starts_with(needle) {
            job.phase = format!("{word} {}", which(job));
            aur_progress(job, stage);
            return true;
        }
    }
    // makepkg's other steps ("==> Retrieving sources...", "  -> Removing libtool files...") keep the build's words, and a
    // source download's percentages are not the whole job's
    line.starts_with("==> ")
        || line.starts_with("-> ")
        || line.starts_with("% Total")
        || line.split_whitespace().next().is_some_and(|w| w.parse::<u32>().is_ok() && line.contains("--:--"))
}

/// Fold one line of output into the job: its phase, and its progress when the line says one.
pub fn read_line(job: &mut Job, raw: &str) {
    let line = plain(raw);
    let line = line.trim();
    if line.is_empty() {
        return;
    }
    if read_aur(job, line) {
        push_log(job, line);
        return;
    }
    let percent = line
        .split_whitespace()
        .rev()
        .find_map(|w| w.strip_suffix('%').and_then(|n| n.parse::<f32>().ok()))
        .filter(|p| (0.0..=100.0).contains(p));
    // pacman: "Packages (3) a-1 b-2 c-3" says how many files may need downloading
    if let Some(rest) = line.strip_prefix("Packages (") {
        if let Some((n, _)) = rest.split_once(')') {
            if let Ok(n) = n.trim().parse::<usize>() {
                job.dl_files = n.max(1);
            }
        }
    }
    // a download bar: "krita-5.3-1-x86_64.pkg.tar.zst  64.1 MiB  9.5 MiB/s 00:07 [####] 45%"
    if line.contains(".pkg.tar") || (line.contains("MiB/s") || line.contains("KiB/s")) && percent.is_some() {
        let name = line.split_whitespace().next().unwrap_or("").to_string();
        if name != job.dl_current {
            if !job.dl_current.is_empty() {
                job.dl_done += 1;
            }
            job.dl_current = name;
        }
        if let Some(p) = percent {
            let files = job.dl_files.max(1) as f32;
            job.dl = Some(((job.dl_done as f32 + p / 100.0) / files).clamp(0.0, 1.0));
        }
        job.phase = "Downloading".into();
        if job.aur_total == 0 {
            set_progress(job);
        }
    } else if let Some(rest) = line.strip_prefix('(') {
        // pacman: "(2/5) installing foo" -- the download part is over by now
        if let Some((counts, text)) = rest.split_once(") ") {
            if let Some((n, m)) = counts.split_once('/') {
                if let (Ok(n), Ok(m)) = (n.trim().parse::<f32>(), m.trim().parse::<f32>()) {
                    if m > 0.0 {
                        if job.dl.is_some() {
                            job.dl = Some(1.0);
                        }
                        let within = percent.map(|p| p / 100.0).unwrap_or(0.0);
                        job.inst = Some(((n - 1.0 + within) / m).clamp(0.0, 1.0));
                        job.phase = text.split(" [").next().unwrap_or(text).trim().to_string();
                        if job.aur_total == 0 {
                            set_progress(job);
                        } else if !job.aur_current.is_empty() {
                            // a built package going in; the bar stays with the builds
                            aur_progress(job, 0.95);
                        }
                    }
                }
            }
        }
    } else {
        // yay / makepkg stages
        for (needle, phase, floor) in [
            ("Cloning", "Downloading the build files", 0.02),
            ("Downloading", "Downloading", 0.04),
            ("Making package", "Building", 0.08),
            ("Starting build()", "Building", 0.12),
            ("Starting package()", "Packaging", 0.6),
            ("Finished making", "Built, installing", 0.7),
            ("Installing", "Installing", 0.0),
            ("Removing", "Removing", 0.0),
            ("Updating", "Updating", 0.0),
        ] {
            if line.contains(needle) {
                job.phase = phase.to_string();
                if floor > 0.0 && job.frac.map(|f| f < floor).unwrap_or(true) {
                    job.frac = Some(floor);
                    job.moved = Instant::now();
                }
                break;
            }
        }
        // a lone percentage (flatpak, curl)
        if let Some(p) = percent {
            if job.dl.is_none() && job.inst.is_none() && !line.contains("memory") {
                let v = p / 100.0;
                if job.frac.map(|f| v >= f).unwrap_or(true) {
                    job.frac = Some(v);
                    job.moved = Instant::now();
                }
            }
        }
    }
    push_log(job, line);
}

fn push_log(job: &mut Job, line: &str) {
    if job.log.last().map(|l| l.as_str()) != Some(line) {
        job.log.push(line.chars().take(300).collect());
        if job.log.len() > 400 {
            job.log.remove(0);
        }
    }
}

/// Ask a running job to stop: the Marketplace's side is signalled, the administrator side is told through its cancel file.
pub fn request_cancel(job: &Shared) {
    let mut g = job.lock().unwrap();
    if g.status == Status::Queued {
        // nothing ran: it just leaves the queue
        g.status = Status::Cancelled;
        g.phase = "Cancelled before it started".into();
        return;
    }
    if g.status != Status::Running || g.cancel_requested {
        return;
    }
    g.cancel_requested = true;
    g.phase = "Cancelling and cleaning up...".into();
    let _ = std::fs::write(&g.cancel_file, b"cancel");
    if let Some(pid) = g.pid {
        // the user-level programs (yay, makepkg, flatpak ...) are in their own group
        unsafe { libc_killpg(pid, 15) };
    }
}

extern "C" {
    #[link_name = "killpg"]
    fn libc_killpg(pgrp: i32, sig: i32) -> i32;
}

fn cancel_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")).join("agora")
}

/// What to undo when a job is cancelled, beyond what the administrator helper removes: folders the build left in the user's cache, and
/// a Flatpak that was not installed before.
#[derive(Default)]
pub struct Undo {
    pub dirs: Vec<PathBuf>,
    pub flatpak: Option<String>,
}

/// A job that waits (`Status::Queued`) until `run` starts it.
pub fn queued(id: &str, title: &str, op: Op) -> Shared {
    let _ = std::fs::create_dir_all(cancel_dir());
    let safe: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
    let cancel_file = cancel_dir().join(format!("{safe}-{}.cancel", std::process::id()));
    let _ = std::fs::remove_file(&cancel_file);
    let now = Instant::now();
    Arc::new(Mutex::new(Job {
        id: id.to_string(),
        title: title.to_string(),
        op,
        log: vec![],
        phase: "Waiting until the one before it is done".into(),
        frac: None,
        status: Status::Queued,
        dl: None,
        inst: None,
        dl_files: 1,
        dl_done: 0,
        dl_current: String::new(),
        aur_total: 0,
        aur_started: 0,
        aur_current: String::new(),
        started: now,
        moved: now,
        cancel_requested: false,
        cancel_file,
        pid: None,
    }))
}

/// Runs a job made by `queued` in the background; it fills in as it goes. `repaint` is called on every change, `done` at the end.
pub fn run(
    job: Shared,
    prog: String,
    args: Vec<String>,
    undo: Undo,
    repaint: impl Fn() + Send + Sync + 'static,
    done: impl FnOnce(Status) + Send + 'static,
) {
    let cancel_file = {
        let mut g = job.lock().unwrap();
        g.status = Status::Running;
        g.phase = format!("{}...", g.op.verb());
        g.started = Instant::now();
        g.moved = g.started;
        g.cancel_file.clone()
    };
    let j = job;
    let repaint = Arc::new(repaint);
    let args: Vec<String> = args.into_iter().map(|a| if a == CANCEL { cancel_file.to_string_lossy().into_owned() } else { a }).collect();
    std::thread::spawn(move || {
        let child = Command::new(&prog)
            .args(&args)
            .env("AGORA_SUDO", SUDO_WRAPPER)
            .env("AGORA_CANCEL_FILE", &cancel_file)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                j.lock().unwrap().status = Status::Failed(format!("could not start {prog}: {e}"));
                repaint();
                done(Status::Failed(e.to_string()));
                return;
            }
        };
        j.lock().unwrap().pid = Some(child.id() as i32);
        // cancelled before the process existed
        if j.lock().unwrap().cancel_requested {
            unsafe { libc_killpg(child.id() as i32, 15) };
        }
        let mut readers = vec![];
        let mut streams: Vec<Box<dyn Read + Send>> = vec![];
        if let Some(o) = child.stdout.take() {
            streams.push(Box::new(o));
        }
        if let Some(e) = child.stderr.take() {
            streams.push(Box::new(e));
        }
        for st in streams {
            let (j, repaint) = (j.clone(), repaint.clone());
            readers.push(std::thread::spawn(move || {
                // programs redraw progress with carriage returns: both end a "line"
                let mut r = BufReader::new(st);
                let mut buf = Vec::new();
                loop {
                    buf.clear();
                    let n = r.read_until(b'\n', &mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&buf).into_owned();
                    for part in text.split('\r') {
                        read_line(&mut j.lock().unwrap(), part);
                    }
                    repaint();
                }
            }));
        }
        let code = child.wait().ok().and_then(|s| s.code());
        for r in readers {
            let _ = r.join();
        }
        let cancelled = j.lock().unwrap().cancel_requested;
        let status = if cancelled {
            // what the administrator helper could not reach: the build folders in the user's cache, a Flatpak
            for d in &undo.dirs {
                let _ = std::fs::remove_dir_all(d);
            }
            if let Some(app) = &undo.flatpak {
                let _ = Command::new("flatpak").args(["uninstall", "--user", "-y", "--noninteractive", app]).stdin(Stdio::null()).output();
            }
            Status::Cancelled
        } else {
            match code {
                Some(0) => Status::Done,
                // pkexec: 126 = the password box was dismissed or the password was wrong, 127 = not authorised
                Some(126) | Some(127) => Status::Failed("Cancelled: administrator rights were not given".into()),
                Some(c) => Status::Failed(format!("it stopped with an error (code {c})")),
                None => Status::Failed("it was stopped".into()),
            }
        };
        let _ = std::fs::remove_file(&cancel_file);
        {
            let mut g = j.lock().unwrap();
            g.status = status.clone();
            match status {
                Status::Done => {
                    g.frac = Some(1.0);
                    g.phase = "Done".into();
                }
                Status::Cancelled => g.phase = "Cancelled; what it had downloaded or installed was removed".into(),
                _ => {}
            }
        }
        repaint();
        done(status);
    });
}

/// A job waiting to run, with what it runs.
pub struct Pending {
    pub job: Shared,
    pub prog: String,
    pub args: Vec<String>,
    pub undo: Undo,
}

/// The jobs waiting their turn: one package operation at a time.
#[derive(Default)]
pub struct Queue {
    waiting: std::collections::VecDeque<Pending>,
}

fn running(jobs: &[Shared]) -> bool {
    jobs.iter().any(|j| j.lock().map(|g| g.status == Status::Running).unwrap_or(false))
}

impl Queue {
    /// Whether a new job has to wait: one runs, or others wait already.
    pub fn must_wait(&self, jobs: &[Shared]) -> bool {
        !self.waiting.is_empty() || running(jobs)
    }

    pub fn push(&mut self, p: Pending) {
        self.waiting.push_back(p);
    }

    /// The next job to run once none runs (those cancelled while waiting are dropped).
    pub fn next(&mut self, jobs: &[Shared]) -> Option<Pending> {
        if running(jobs) {
            return None;
        }
        while let Some(p) = self.waiting.pop_front() {
            if p.job.lock().map(|g| g.status == Status::Queued).unwrap_or(false) {
                return Some(p);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A job made and run at once (the tests' shorthand for `queued` and `run`).
    #[allow(clippy::too_many_arguments)]
    fn start(
        id: &str,
        title: &str,
        op: Op,
        prog: String,
        args: Vec<String>,
        undo: Undo,
        repaint: impl Fn() + Send + Sync + 'static,
        done: impl FnOnce(Status) + Send + 'static,
    ) -> Shared {
        let job = queued(id, title, op);
        run(job.clone(), prog, args, undo, repaint, done);
        job
    }

    use crate::catalog::{parse_catalog, SAMPLE};

    fn job() -> Job {
        let now = Instant::now();
        Job {
            id: "x".into(),
            title: "X".into(),
            op: Op::Install,
            log: vec![],
            phase: String::new(),
            frac: None,
            status: Status::Running,
            dl: None,
            inst: None,
            dl_files: 1,
            dl_done: 0,
            dl_current: String::new(),
            aur_total: 0,
            aur_started: 0,
            aur_current: String::new(),
            started: now,
            moved: now,
            cancel_requested: false,
            cancel_file: PathBuf::new(),
            pid: None,
        }
    }

    /// A recorded AUR install (an AUR dependency built first): the bar says which package is being built, and follows the builds
    /// instead of jumping to full at the first pacman install.
    #[test]
    fn an_aur_build_says_which_package_it_is_on() {
        let mut j = job();
        let mut phases: Vec<String> = vec![];
        let mut fracs: Vec<f32> = vec![];
        for line in include_str!("../tests/fixtures/yay-aur-build.txt").lines().filter(|l| !l.starts_with('#')) {
            read_line(&mut j, line);
            if phases.last() != Some(&j.phase) {
                phases.push(j.phase.clone());
            }
            if let Some(f) = j.frac {
                if line.starts_with("(1/1) installing cmake") {
                    assert!(f < 0.05, "the make dependency's install is not the job: {f}");
                }
                if line.starts_with("==> Finished making: libexample") {
                    assert!((0.3..0.5).contains(&f), "one of two built: {f}");
                }
                fracs.push(f);
            }
        }
        assert_eq!(j.aur_total, 2);
        for want in [
            "Building libexample (1 of 2)",
            "Packaging libexample (1 of 2)",
            "Installing libexample (1 of 2)",
            "Building example-app (2 of 2)",
            "Packaging example-app (2 of 2)",
        ] {
            assert!(phases.iter().any(|p| p == want), "{want:?} not in {phases:?}");
        }
        // from the first build on, makepkg's own steps ("-> Removing libtool files", its source download) keep the build's words
        let builds = phases.iter().skip_while(|p| !p.starts_with("Building"));
        assert!(!builds.clone().any(|p| p == "Removing" || p == "Downloading"), "makepkg's own steps leaked: {phases:?}");
        assert!(fracs.windows(2).all(|w| w[1] >= w[0]), "{fracs:?}");
        assert!((0.9..=1.0).contains(fracs.last().unwrap()), "{fracs:?}");
        // a single AUR package is named without a count
        let mut one = job();
        for l in [":: (1/1) Parsing SRCINFO: tool", "==> Making package: tool 2.0-1 (date)", "\u{1b}[1;32m==>\u{1b}[0m Starting build()..."]
        {
            read_line(&mut one, l);
        }
        assert_eq!(one.phase, "Building tool");
    }

    #[test]
    fn pacman_counts_become_progress() {
        let mut j = job();
        read_line(&mut j, "(2/4) installing krita");
        assert_eq!(j.frac, Some(0.25));
        assert_eq!(j.phase, "installing krita");
        read_line(&mut j, "(4/4) running post-transaction hooks...");
        assert_eq!(j.frac, Some(0.75));
    }

    #[test]
    fn a_percentage_is_used_when_there_is_no_count() {
        let mut j = job();
        read_line(&mut j, "Installing org.kde.krita ━━━━━ 45%");
        assert_eq!(j.frac, Some(0.45));
        assert_eq!(j.phase, "Installing");
    }

    #[test]
    fn downloading_and_installing_share_the_bar() {
        let mut j = job();
        read_line(&mut j, "Packages (2) a-1 b-2");
        read_line(&mut j, "a-1-x86_64.pkg.tar.zst  10.0 MiB  5.0 MiB/s 00:02 [######] 100%");
        read_line(&mut j, "b-2-x86_64.pkg.tar.zst  10.0 MiB  5.0 MiB/s 00:01 [###---] 50%");
        // 1.5 of 2 files downloaded = three quarters of the first half
        assert!((j.frac.unwrap() - 0.375).abs() < 0.001, "{:?}", j.frac);
        read_line(&mut j, "(1/2) installing a");
        read_line(&mut j, "(2/2) installing b");
        assert!((j.frac.unwrap() - 0.75).abs() < 0.001, "{:?}", j.frac);
    }

    #[test]
    fn the_bar_never_goes_backwards() {
        let mut j = job();
        read_line(&mut j, "(3/4) installing c");
        let before = j.frac.unwrap();
        read_line(&mut j, "(1/4) installing a");
        assert!(j.frac.unwrap() >= before);
    }

    #[test]
    fn build_stages_set_the_phase() {
        let mut j = job();
        read_line(&mut j, "==> Making package: pdfcraft-bin 0.8.0-1 (Sat Oct 17)");
        assert_eq!(j.phase, "Building pdfcraft-bin");
        read_line(&mut j, "==> Starting package()...");
        assert_eq!(j.phase, "Packaging pdfcraft-bin");
    }

    fn wait(job: &Shared) -> Job {
        for _ in 0..100 {
            let j = job.lock().unwrap().clone();
            if j.status != Status::Running {
                return j;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the job did not finish");
    }

    #[test]
    fn a_second_install_waits_for_the_first_and_both_finish() {
        let log = std::env::temp_dir().join(format!("agora-queue-{}", std::process::id()));
        let _ = std::fs::remove_file(&log);
        let cmd =
            |name: &str| vec!["-c".to_string(), format!("echo start-{name} >> '{0}'; sleep 0.3; echo end-{name} >> '{0}'", log.display())];
        let mut q = Queue::default();
        let mut jobs: Vec<Shared> = vec![];
        let first = queued("a", "A", Op::Install);
        jobs.push(first.clone());
        assert!(!q.must_wait(&jobs));
        run(first.clone(), "sh".into(), cmd("a"), Undo::default(), || {}, |_| {});
        let second = queued("b", "B", Op::Install);
        jobs.push(second.clone());
        assert!(q.must_wait(&jobs), "the first runs");
        q.push(Pending { job: second.clone(), prog: "sh".into(), args: cmd("b"), undo: Undo::default() });
        let third = queued("c", "C", Op::Install);
        jobs.push(third.clone());
        q.push(Pending { job: third.clone(), prog: "sh".into(), args: cmd("c"), undo: Undo::default() });
        request_cancel(&third);
        assert_eq!(third.lock().unwrap().status, Status::Cancelled, "cancelled while it waited: nothing ran");
        assert_eq!(second.lock().unwrap().status, Status::Queued);
        assert!(q.next(&jobs).is_none(), "not while the first runs");
        assert_eq!(wait(&first).status, Status::Done);
        let p = q.next(&jobs).expect("the second's turn");
        run(p.job, p.prog, p.args, p.undo, || {}, |_| {});
        assert_eq!(wait(&second).status, Status::Done);
        assert!(q.next(&jobs).is_none(), "the cancelled one is dropped");
        let lines = std::fs::read_to_string(&log).unwrap();
        let _ = std::fs::remove_file(&log);
        assert_eq!(lines.lines().collect::<Vec<_>>(), ["start-a", "end-a", "start-b", "end-b"], "one after the other");
    }

    #[test]
    fn a_job_reads_the_output_and_reports_the_end() {
        let job = start(
            "x",
            "X",
            Op::Install,
            "sh".into(),
            vec!["-c".into(), "echo '(1/2) installing a'; printf '(2/2) installing b\\r(2/2) done\\n'; exit 0".into()],
            Undo::default(),
            || {},
            |_| {},
        );
        let j = wait(&job);
        assert_eq!(j.status, Status::Done);
        assert_eq!(j.frac, Some(1.0));
        assert!(j.log.iter().any(|l| l.contains("installing a")));
    }

    #[test]
    fn a_dismissed_password_box_is_a_cancel_not_a_crash() {
        let job = start("x", "X", Op::Install, "sh".into(), vec!["-c".into(), "exit 126".into()], Undo::default(), || {}, |_| {});
        match wait(&job).status {
            Status::Failed(why) => assert!(why.starts_with("Cancelled")),
            other => panic!("{other:?}"),
        }
        let job =
            start("y", "Y", Op::Install, "sh".into(), vec!["-c".into(), "echo oops >&2; exit 3".into()], Undo::default(), || {}, |_| {});
        let j = wait(&job);
        assert!(matches!(j.status, Status::Failed(ref w) if w.contains("code 3")));
        assert!(j.log.iter().any(|l| l == "oops"), "errors are read too");
    }

    #[test]
    fn update_everything_is_one_yay_run_through_the_password_box() {
        let c = parse_catalog(SAMPLE, "Sample", true).unwrap();
        let e = c.apps[0].clone();
        let (p, a) = command(&e, Op::UpdateAll).unwrap();
        assert_eq!(p, "yay", "one process asks for every root step, so the password is asked once");
        assert_eq!(&a[..4], &["-Syu", "--noconfirm", "--sudo", SUDO_WRAPPER]);
        assert!(!a.iter().any(|x| x == "sudo" || x == "-c"), "no shell, no plain sudo");
    }

    #[test]
    fn commands_for_each_kind_of_source() {
        let c = parse_catalog(SAMPLE, "Sample", true).unwrap();
        let find = |id: &str| c.apps.iter().find(|a| a.id == id).unwrap().clone();
        let (p, a) = command(&find("krita"), Op::Install).unwrap();
        assert_eq!((p.as_str(), a.join(" ")), ("pkexec", format!("{ROOT_HELPER} {CANCEL} pacman -S --needed --noconfirm krita")));
        assert_eq!(
            command(&find("krita"), Op::Reinstall).unwrap().1.join(" "),
            format!("{ROOT_HELPER} {CANCEL} pacman -S --noconfirm krita")
        );
        let (p, a) = command(&find("pdfcraft"), Op::Install).unwrap();
        assert_eq!(p, "yay");
        assert!(a.contains(&"--noconfirm".to_string()) && a.contains(&"pdfcraft-bin".to_string()) && a.contains(&SUDO_WRAPPER.to_string()));
        assert_eq!(
            command(&find("krita"), Op::Remove).unwrap().1.join(" "),
            format!("{ROOT_HELPER} {CANCEL} pacman -Rs --noconfirm krita")
        );
    }
}
