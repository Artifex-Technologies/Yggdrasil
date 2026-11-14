//! Getting ready while the person chooses: as soon as the computer is online, the install program's `--prefetch` fetches what
//! the install will download (the chosen browsers the medium lacks, the chosen optional parts' packages, the updates its
//! fallback would pull), so the install itself is faster. It is run as the install is (pkexec), only when the OS's installer
//! integration asks for it (`prefetch = yes`), gets the choices as one line of JSON on its standard input, and stops when that
//! is closed. One at a time: when the choices change (and then stay put for a moment) the running one is stopped and, once it
//! has gone, started again with the new ones (what it already fetched stays). Going offline stops it; coming back online
//! starts it again. Nothing waits on it: a failure only means the install downloads as it always did.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// How long the choices stay put before a new run starts (ticking boxes one after another starts one run, not five).
pub const SETTLE: Duration = Duration::from_secs(2);

/// What the summary says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Not started (offline, or the OS does not ask for it).
    Idle,
    Fetching(usize, usize),
    Done(usize),
    /// Not done: the install downloads as usual (why, quietly).
    NotDone(String),
}

impl Status {
    /// The summary's quiet line (empty: nothing to say).
    pub fn line(&self) -> String {
        match self {
            Status::Idle => String::new(),
            Status::Fetching(_, 0) => "Getting ready: working out what to download…".into(),
            Status::Fetching(k, n) => format!("Getting ready: {k} of {n} packages downloaded"),
            Status::Done(0) => "Getting ready: nothing to download".into(),
            Status::Done(n) => format!("Getting ready: all {n} packages downloaded"),
            Status::NotDone(why) => format!("Getting ready stopped ({why}); the install downloads as usual"),
        }
    }
}

/// A run of `--prefetch`: its standard input (dropping it stops the run) and its lines.
pub struct Running {
    pub choices: String,
    pub stdin: Option<Box<dyn Write + Send>>,
    pub lines: Receiver<String>,
}

/// Starts `command --prefetch` with these choices.
pub type Starter = fn(&[String], &str) -> Result<Running, String>;

/// The install program through pkexec, as the install itself is run.
pub fn start_with_pkexec(command: &[String], choices: &str) -> Result<Running, String> {
    let mut child = Command::new("pkexec")
        .args(command)
        .arg("--prefetch")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let mut stdin = child.stdin.take().ok_or("no standard input")?;
    stdin.write_all(format!("{choices}\n").as_bytes()).map_err(|e| e.to_string())?;
    let out = child.stdout.take().ok_or("no standard output")?;
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
        let _ = child.wait();
    });
    Ok(Running { choices: choices.to_string(), stdin: Some(Box::new(stdin)), lines: rx })
}

pub struct Prefetch {
    /// The install program (None: no getting ready).
    command: Option<Vec<String>>,
    start: Starter,
    pub running: Option<Running>,
    pub status: Status,
    /// The choices a run was last started for (since the computer came online).
    tried: Option<String>,
    /// The choices as they are, and since when.
    wanted: Option<(String, Instant)>,
}

impl Prefetch {
    pub fn new(command: Option<Vec<String>>, start: Starter) -> Prefetch {
        Prefetch { command, start, running: None, status: Status::Idle, tried: None, wanted: None }
    }

    /// Stops the run (closes its standard input); it says STOPPED and goes.
    pub fn stop(&mut self) {
        if let Some(r) = &mut self.running {
            r.stdin = None;
        }
    }

    /// Once a frame (or so): takes in what the run said, and starts, stops or restarts it for `online` and `choices`.
    pub fn tick(&mut self, online: bool, choices: &str, now: Instant) {
        if let Some(r) = &mut self.running {
            loop {
                match r.lines.try_recv() {
                    Ok(line) => {
                        let (kind, rest) = line.split_once(' ').unwrap_or((line.as_str(), ""));
                        match kind {
                            "PREFETCH" => {
                                let mut it = rest.split(' ').filter_map(|x| x.parse::<usize>().ok());
                                if let (Some(k), Some(n)) = (it.next(), it.next()) {
                                    self.status = Status::Fetching(k, n);
                                }
                            }
                            "DONE" => {
                                let n = if let Status::Fetching(_, n) = self.status { n } else { 0 };
                                self.status = Status::Done(n);
                            }
                            "WARN" | "ERROR" => self.status = Status::NotDone(rest.trim().to_string()),
                            // stopped on purpose (new choices, offline, the install starting): nothing to report
                            "STOPPED" => self.status = Status::Idle,
                            _ => {}
                        }
                    }
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        // it has gone: stopped, finished or failed
                        if matches!(self.status, Status::Fetching(..)) {
                            self.status = Status::NotDone("it stopped part way".into());
                        }
                        self.running = None;
                        break;
                    }
                }
            }
        }
        let Some(command) = self.command.clone() else { return };
        if !online {
            // no network: nothing to fetch; coming back online tries again
            self.stop();
            self.tried = None;
            return;
        }
        if self.wanted.as_ref().is_none_or(|w| w.0 != choices) {
            self.wanted = Some((choices.to_string(), now));
        }
        if let Some(r) = &self.running {
            if r.choices != choices {
                self.stop();
            }
            return;
        }
        let settled = self.wanted.as_ref().is_some_and(|w| now.duration_since(w.1) >= SETTLE);
        if settled && self.tried.as_deref() != Some(choices) {
            self.tried = Some(choices.to_string());
            match (self.start)(&command, choices) {
                Ok(r) => {
                    self.status = Status::Fetching(0, 0);
                    self.running = Some(r);
                }
                Err(e) => self.status = Status::NotDone(e),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    /// What the made-up install program was started with, in order.
    static STARTED: Mutex<Vec<String>> = Mutex::new(Vec::new());

    /// Standard input that notes when it is closed.
    struct Pipe(Arc<AtomicBool>);
    impl Write for Pipe {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    impl Drop for Pipe {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    /// A made-up install program: 2 packages, one every 20 ms, STOPPED when its standard input is closed. Nothing is run.
    fn fake(_: &[String], choices: &str) -> Result<Running, String> {
        STARTED.lock().unwrap().push(choices.to_string());
        let closed = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let c = closed.clone();
        std::thread::spawn(move || {
            let _ = tx.send("PREFETCH 0 2".to_string());
            for k in 1..=2 {
                std::thread::sleep(Duration::from_millis(20));
                if c.load(Ordering::SeqCst) {
                    let _ = tx.send("STOPPED".to_string());
                    return;
                }
                let _ = tx.send(format!("PREFETCH {k} 2"));
            }
            let _ = tx.send("DONE".to_string());
        });
        Ok(Running { choices: choices.into(), stdin: Some(Box::new(Pipe(closed))), lines: rx })
    }

    fn until(p: &mut Prefetch, online: bool, choices: &str, now: &mut Instant, done: impl Fn(&Prefetch) -> bool) {
        for _ in 0..500 {
            p.tick(online, choices, *now);
            if done(p) {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
            *now += Duration::from_millis(50);
        }
        panic!("never: {:?}", p.status);
    }

    #[test]
    fn it_runs_once_online_restarts_on_new_choices_and_never_twice_at_once() {
        STARTED.lock().unwrap().clear();
        let cmd = Some(vec!["/usr/bin/example-install".to_string()]);
        let mut p = Prefetch::new(cmd, fake);
        let mut now = Instant::now();
        let a = r#"{"browsers":["firefox"]}"#;
        let b = r#"{"browsers":["librewolf"]}"#;
        // offline: nothing starts
        for _ in 0..5 {
            p.tick(false, a, now);
            now += SETTLE;
        }
        assert!(STARTED.lock().unwrap().is_empty() && p.status == Status::Idle);
        // online: once the choices have stayed put
        p.tick(true, a, now);
        assert!(p.running.is_none(), "not before the choices settle");
        until(&mut p, true, a, &mut now, |p| p.status == Status::Done(2));
        assert_eq!(Status::Done(2).line(), "Getting ready: all 2 packages downloaded");
        until(&mut p, true, a, &mut now, |p| p.running.is_none());
        for _ in 0..10 {
            p.tick(true, a, now);
            now += SETTLE;
        }
        assert_eq!(STARTED.lock().unwrap().len(), 1, "the same choices again: not started again");
        // new choices while it runs: stopped first, the new run only once it has gone
        let started = |n| move |p: &Prefetch| STARTED.lock().unwrap().len() == n && p.running.is_some();
        until(&mut p, true, b, &mut now, started(2));
        until(&mut p, true, a, &mut now, |p| p.running.as_ref().is_some_and(|r| r.stdin.is_none()));
        assert_eq!(STARTED.lock().unwrap().len(), 2, "never two at once");
        until(&mut p, true, a, &mut now, started(3));
        assert_eq!(STARTED.lock().unwrap().as_slice(), [a, b, a]);
        // offline: stopped; back online: again
        until(&mut p, false, a, &mut now, |p| p.running.is_none());
        until(&mut p, true, a, &mut now, started(4));
    }

    #[test]
    fn what_the_summary_says() {
        assert_eq!(Status::Idle.line(), "");
        assert_eq!(Status::Fetching(3, 7).line(), "Getting ready: 3 of 7 packages downloaded");
        assert_eq!(Status::Done(0).line(), "Getting ready: nothing to download");
        assert!(Status::NotDone("no room".into()).line().ends_with("the install downloads as usual"));
        // an OS that does not ask for it: nothing ever starts
        let mut p = Prefetch::new(None, |_, _| panic!("never started"));
        p.tick(true, "{}", Instant::now() + SETTLE * 10);
        assert_eq!(p.status, Status::Idle);
    }
}
