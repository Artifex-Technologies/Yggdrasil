//! Installing, reinstalling, removing and updating inside the Marketplace, with progress. The command runs as the user, with its output
//! read line by line; anything that needs administrator rights asks through Labyrinth's own password box (pkexec / polkit), so no terminal
//! is needed. Only apps from Labyrinth's own (reviewed) catalog are installed this way: for anything else the Marketplace keeps opening a
//! terminal, where the prompts (an AUR package's build file, for instance) can be read before saying yes.

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

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
    Running,
    Done,
    Failed(String),
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
}

pub type Shared = Arc<Mutex<Job>>;

const SUDO_WRAPPER: &str = "/usr/local/bin/labyrinth-sudo-pkexec";

/// The command for an entry and operation, as program + arguments; None when it cannot be done here (a plugin).
pub fn command(e: &Entry, op: Op) -> Option<(String, Vec<String>)> {
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let with = |mut base: Vec<String>, pk: &[String]| {
        base.extend(pk.iter().cloned());
        base
    };
    Some(match (&e.source, op) {
        (Source::Pacman(p), Op::Install) => ("pkexec".into(), with(s(&["pacman", "-S", "--needed", "--noconfirm"]), p)),
        (Source::Pacman(p), Op::Reinstall) => ("pkexec".into(), with(s(&["pacman", "-S", "--noconfirm"]), p)),
        (Source::Pacman(p), Op::Remove) | (Source::Aur(p), Op::Remove) => ("pkexec".into(), with(s(&["pacman", "-Rs", "--noconfirm"]), p)),
        (Source::Aur(p), Op::Install) => ("yay".into(), with(s(&["-S", "--needed", "--noconfirm", "--sudo", SUDO_WRAPPER]), p)),
        (Source::Aur(p), Op::Reinstall) => ("yay".into(), with(s(&["-S", "--noconfirm", "--sudo", SUDO_WRAPPER]), p)),
        (Source::Flatpak(a), Op::Install) => ("flatpak".into(), s(&["install", "--user", "-y", "--noninteractive", "flathub", a])),
        (Source::Flatpak(a), Op::Reinstall) => ("flatpak".into(), s(&["install", "--user", "-y", "--noninteractive", "--reinstall", "flathub", a])),
        (Source::Flatpak(a), Op::Remove) => ("flatpak".into(), s(&["uninstall", "--user", "-y", "--noninteractive", a])),
        (Source::Component(c), Op::Install) => ("labyrinth-apps".into(), s(&["install", c])),
        (Source::Component(c), Op::Reinstall) => ("labyrinth-apps".into(), s(&["install", c])),
        (Source::Component(c), Op::Remove) => ("labyrinth-apps".into(), s(&["remove", c])),
        (_, Op::UpdateAll) => ("yay".into(), s(&["-Syu", "--noconfirm", "--sudo", SUDO_WRAPPER])),
        (Source::Plugin { .. }, _) => return None,
    })
}

/// Fold one line of output into the job: its phase, and its progress when the line says one.
pub fn read_line(job: &mut Job, raw: &str) {
    let line = raw.trim();
    if line.is_empty() {
        return;
    }
    // pacman: "(2/5) installing foo"
    if let Some(rest) = line.strip_prefix('(') {
        if let Some((counts, text)) = rest.split_once(") ") {
            if let Some((n, m)) = counts.split_once('/') {
                if let (Ok(n), Ok(m)) = (n.trim().parse::<f32>(), m.trim().parse::<f32>()) {
                    if m > 0.0 {
                        job.frac = Some(((n - 1.0) / m).clamp(0.0, 1.0));
                        job.phase = text.trim().to_string();
                    }
                }
            }
        }
    }
    // yay / makepkg stages
    for (needle, phase) in [
        ("Downloading", "Downloading"),
        ("Cloning", "Downloading the build files"),
        ("Making package", "Building"),
        ("Starting build()", "Building"),
        ("Starting package()", "Packaging"),
        ("Finished making", "Built, installing"),
        ("Installing", "Installing"),
        ("Removing", "Removing"),
        ("Updating", "Updating"),
        ("checking", "Checking"),
    ] {
        if line.contains(needle) && !line.starts_with('(') {
            job.phase = phase.to_string();
            break;
        }
    }
    // a percentage anywhere in the line (flatpak, curl, pacman's download bars)
    if let Some(p) = line.split_whitespace().rev().find_map(|w| w.strip_suffix('%').and_then(|n| n.parse::<f32>().ok())) {
        if (0.0..=100.0).contains(&p) && !line.contains("memory") {
            // pacman's per-file bars restart at 0 each file: only trust them when there is no (n/m) count
            if job.frac.is_none() || line.contains("Installing") {
                job.frac = Some(p / 100.0);
            }
        }
    }
    if job.log.last().map(|l| l.as_str()) != Some(line) {
        job.log.push(line.chars().take(300).collect());
        if job.log.len() > 400 {
            job.log.remove(0);
        }
    }
}

/// Run the command in the background; the returned job fills in as it goes. `repaint` is called on every change.
pub fn start(id: &str, title: &str, op: Op, prog: String, args: Vec<String>, repaint: impl Fn() + Send + Sync + 'static, done: impl FnOnce(Status) + Send + 'static) -> Shared {
    let job = Arc::new(Mutex::new(Job { id: id.to_string(), title: title.to_string(), op, log: vec![], phase: format!("{}...", op.verb()), frac: None, status: Status::Running }));
    let j = job.clone();
    let repaint = Arc::new(repaint);
    std::thread::spawn(move || {
        let child = Command::new(&prog).args(&args).env("LABYRINTH_SUDO", SUDO_WRAPPER).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                j.lock().unwrap().status = Status::Failed(format!("could not start {prog}: {e}"));
                repaint();
                done(Status::Failed(e.to_string()));
                return;
            }
        };
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
        let status = match code {
            Some(0) => Status::Done,
            // pkexec: 126 = the password box was dismissed or the password was wrong, 127 = not authorised
            Some(126) | Some(127) => Status::Failed("Cancelled: administrator rights were not given".into()),
            Some(c) => Status::Failed(format!("it stopped with an error (code {c})")),
            None => Status::Failed("it was stopped".into()),
        };
        {
            let mut g = j.lock().unwrap();
            g.status = status.clone();
            if status == Status::Done {
                g.frac = Some(1.0);
                g.phase = "Done".into();
            }
        }
        repaint();
        done(status);
    });
    job
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{parse_catalog, BUILTIN};

    fn job() -> Job {
        Job { id: "x".into(), title: "X".into(), op: Op::Install, log: vec![], phase: String::new(), frac: None, status: Status::Running }
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
    fn build_stages_set_the_phase() {
        let mut j = job();
        read_line(&mut j, "==> Making package: pdfcraft-bin 0.8.0-1 (Sat Oct 17)");
        assert_eq!(j.phase, "Building");
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
    fn a_job_reads_the_output_and_reports_the_end() {
        let job = start("x", "X", Op::Install, "sh".into(), vec!["-c".into(), "echo '(1/2) installing a'; printf '(2/2) installing b\\r(2/2) done\\n'; exit 0".into()], || {}, |_| {});
        let j = wait(&job);
        assert_eq!(j.status, Status::Done);
        assert_eq!(j.frac, Some(1.0));
        assert!(j.log.iter().any(|l| l.contains("installing a")));
    }

    #[test]
    fn a_dismissed_password_box_is_a_cancel_not_a_crash() {
        let job = start("x", "X", Op::Install, "sh".into(), vec!["-c".into(), "exit 126".into()], || {}, |_| {});
        match wait(&job).status {
            Status::Failed(why) => assert!(why.starts_with("Cancelled")),
            other => panic!("{other:?}"),
        }
        let job = start("y", "Y", Op::Install, "sh".into(), vec!["-c".into(), "echo oops >&2; exit 3".into()], || {}, |_| {});
        let j = wait(&job);
        assert!(matches!(j.status, Status::Failed(ref w) if w.contains("code 3")));
        assert!(j.log.iter().any(|l| l == "oops"), "errors are read too");
    }

    #[test]
    fn commands_for_each_kind_of_source() {
        let c = parse_catalog(BUILTIN, "Labyrinth", true).unwrap();
        let find = |id: &str| c.apps.iter().find(|a| a.id == id).unwrap().clone();
        let (p, a) = command(&find("krita"), Op::Install).unwrap();
        assert_eq!((p.as_str(), a.join(" ")), ("pkexec", "pacman -S --needed --noconfirm krita".to_string()));
        assert_eq!(command(&find("krita"), Op::Reinstall).unwrap().1.join(" "), "pacman -S --noconfirm krita");
        let (p, a) = command(&find("pdfcraft"), Op::Install).unwrap();
        assert_eq!(p, "yay");
        assert!(a.contains(&"--noconfirm".to_string()) && a.contains(&"pdfcraft-bin".to_string()) && a.contains(&SUDO_WRAPPER.to_string()));
        assert_eq!(command(&find("krita"), Op::Remove).unwrap().1.join(" "), "pacman -Rs --noconfirm krita");
    }
}
