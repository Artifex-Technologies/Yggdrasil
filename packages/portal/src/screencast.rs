//! `org.freedesktop.impl.portal.ScreenCast`: screen sharing on Yggdrasil's own compositor. A program asks to share the screen;
//! the share picker (`yggdrasil-share-picker`) asks the person which screens; one `yggdrasil-screencast` per chosen screen
//! publishes it as a PipeWire stream, and the answer is the streams' node numbers (the portal frontend gives the program those
//! nodes and nothing else of PipeWire). Nothing is shared without the person choosing it in the picker.
//!
//! A worker ends when the program closes the session (Session.Close), when its screen goes or its compositor ends (once the
//! last of a session's workers has ended the session says Closed), and with this portal: each worker is tied to a thread of
//! its own here by a parent-death signal, so a screen is never left shared by a worker nobody owns.
//!
//! While anything is shared the bar says so: the portal writes `~/.local/state/yggdrasil/screen-sharing.json`, which the bar's
//! screen sharing item (`data/screen-sharing.conf`, a `bar-item` integration) shows as a red "Sharing" pill whose card names
//! each program and its screens, with Stop sharing (`yggdrasil-portal --stop-sharing`, `org.yggdrasil.ScreenSharing.StopAll`).

use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use zbus::object_server::{ObjectServer, SignalEmitter};
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

use crate::{opt_bool, Options, Request, Results};

/// Source types offered: whole screens (MONITOR). Single windows and virtual screens are not.
const SOURCE_MONITOR: u32 = 1;
/// Cursor modes offered: hidden, or painted into the frames (embedded). Not sent apart as metadata.
const CURSOR_HIDDEN: u32 = 1;
const CURSOR_EMBEDDED: u32 = 2;
/// How long a worker may take to publish its stream.
const NODE_WAIT: Duration = Duration::from_secs(10);
const SESSION_IFACE: &str = "org.freedesktop.impl.portal.Session";

/// The sessions by their object path.
pub type Sessions = Arc<Mutex<HashMap<String, Sess>>>;

/// A session: what the program chose in SelectSources, the workers sharing its screens, whether Start answered, and the
/// program's name for the bar.
pub struct Sess {
    multiple: bool,
    cursor: u32,
    workers: Vec<Worker>,
    started: bool,
    app: String,
}

struct Worker {
    id: u64,
    /// the worker's process as a pidfd: a signal reaches this process and never one that took its number later
    pidfd: OwnedFd,
    /// the screen it shares
    screen: String,
}

impl Sess {
    fn new() -> Sess {
        Sess { multiple: false, cursor: CURSOR_HIDDEN, workers: vec![], started: false, app: String::new() }
    }

    fn stop(&mut self) {
        for w in self.workers.drain(..) {
            let _ = rustix::process::pidfd_send_signal(&w.pidfd, rustix::process::Signal::TERM);
        }
    }
}

/// A string as JSON.
fn js(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The bar item's JSON (docs/INTEGRATIONS.md, `bar-item`): off while nothing is shared; else a red pill, each program with the
/// screens it sees, and Stop sharing.
fn bar_json(shares: &[(String, Vec<String>)]) -> String {
    let n: usize = shares.iter().map(|(_, s)| s.len()).sum();
    if n == 0 {
        return "{\"state\": \"off\"}\n".into();
    }
    let text = if n == 1 { "Sharing".to_string() } else { format!("Sharing {n} screens") };
    let lines: Vec<String> = shares
        .iter()
        .filter(|(_, s)| !s.is_empty())
        .map(|(app, screens)| format!("{{\"title\": {}, \"value\": {}}}", js(app), js(&screens.join(", "))))
        .collect();
    format!(
        "{{\"text\": {}, \"state\": \"alert\", \"icon\": \"screen-shared-symbolic\", \"lines\": [{}], \
         \"card\": [{{\"buttons\": [{{\"id\": \"stop\", \"label\": \"Stop sharing\", \"accent\": true, \"close\": true}}]}}], \
         \"help\": \"These programs see the screens named. Stop sharing ends every share at once; a program shares again only by asking again.\"}}\n",
        js(&text),
        lines.join(", ")
    )
}

/// The bar item's file.
fn bar_file() -> std::path::PathBuf {
    yggdrasil_conf::dirs::state().join("screen-sharing.json")
}

/// Tells the bar what is shared now (written whole, then moved into place, so the bar never reads half of it).
pub fn publish(sessions: &Sessions) {
    publish_to(&bar_file(), sessions);
}

/// One write at a time, each from the sessions as they are at its turn. Two at once (a screen unplugged: its worker ends as
/// its session closes) shared the one temporary file, so one could move the other's emptied or half-written file into
/// place, which the bar then could not read, or an older state could land after a newer one.
static WRITING: Mutex<()> = Mutex::new(());

fn publish_to(path: &std::path::Path, sessions: &Sessions) {
    let _turn = WRITING.lock().unwrap_or_else(|e| e.into_inner());
    let shares: Vec<(String, Vec<String>)> = sessions
        .lock()
        .unwrap()
        .values()
        .filter(|s| s.started)
        .map(|s| (s.app.clone(), s.workers.iter().map(|w| w.screen.clone()).collect()))
        .collect();
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let tmp = path.with_extension("json.new");
    if std::fs::write(&tmp, bar_json(&shares)).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

fn opt_u32(o: &Options, k: &str) -> Option<u32> {
    o.get(k).and_then(|v| u32::try_from(v).ok())
}

/// A screen's size in the compositor's space (logical pixels): the mode's size turned with the screen, divided by its scale.
pub fn logical_size(s: &yggdrasil_wm::Screen) -> (i32, i32) {
    let (w, h) = if s.transform % 2 == 1 { (s.height, s.width) } else { (s.width, s.height) };
    let scale = s.scale.filter(|f| *f > 0.0).unwrap_or(1.0);
    ((w as f64 / scale).round() as i32, (h as f64 / scale).round() as i32)
}

/// One stream of Start's answer: the node, with the screen's place and size when the compositor told them.
fn stream_entry(node: u32, index: usize, screen: Option<&yggdrasil_wm::Screen>) -> (u32, HashMap<String, OwnedValue>) {
    let mut p: HashMap<String, OwnedValue> = HashMap::new();
    let put = |p: &mut HashMap<String, OwnedValue>, k: &str, v: Value| {
        if let Ok(v) = OwnedValue::try_from(v) {
            p.insert(k.to_string(), v);
        }
    };
    // an id of this session's own (the screens' connector names are not handed out)
    put(&mut p, "id", Value::from(index.to_string()));
    put(&mut p, "source_type", Value::from(SOURCE_MONITOR));
    if let Some(s) = screen {
        put(&mut p, "position", Value::from((s.x, s.y)));
        let (w, h) = logical_size(s);
        put(&mut p, "size", Value::from((w, h)));
    }
    (node, p)
}

/// The share picker's answer: the chosen screens' names, one a line; None when the person cancelled (or it could not start).
fn pick_screens(handle: &str, running: &Arc<Mutex<HashMap<String, u32>>>, app_id: &str, multiple: bool) -> Option<Vec<String>> {
    let mut cmd = Command::new("yggdrasil-share-picker");
    if !app_id.is_empty() {
        cmd.args(["--app", app_id]);
    }
    if multiple {
        cmd.arg("--multiple");
    }
    let child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn();
    let child = match child {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yggdrasil-portal: could not start yggdrasil-share-picker: {e}");
            return None;
        }
    };
    running.lock().unwrap().insert(handle.to_string(), child.id());
    let out = child.wait_with_output();
    running.lock().unwrap().remove(handle);
    let out = out.ok()?;
    if !out.status.success() {
        return None;
    }
    let names = picked_names(&String::from_utf8_lossy(&out.stdout), multiple);
    (!names.is_empty()).then_some(names)
}

/// The picker's lines as screen names: no empty lines, no repeats, and one only when the program asked for one.
fn picked_names(out: &str, multiple: bool) -> Vec<String> {
    let mut names: Vec<String> = vec![];
    for l in out.lines().map(str::trim).filter(|l| !l.is_empty()) {
        if !names.iter().any(|n| n == l) {
            names.push(l.to_string());
        }
    }
    if !multiple {
        names.truncate(1);
    }
    names
}

/// What a worker's thread tells Start.
enum Ev {
    Spawned(OwnedFd),
    Node(u32),
    Failed(String),
}

/// Starts a worker for `output` on a thread of its own, which it is tied to (parent-death signal) and which waits for it;
/// `on_exit` runs there once it has ended. Answers the stream's node, or why there is none.
fn launch(
    sessions: &Sessions,
    path: &str,
    id: u64,
    output: &str,
    cursor: bool,
    on_exit: impl FnOnce() + Send + 'static,
) -> Result<u32, String> {
    let (tx, rx) = mpsc::channel::<Ev>();
    let output = output.to_string();
    let output_name = output.clone();
    std::thread::Builder::new()
        .name("screencast".into())
        .spawn(move || {
            let mut cmd = Command::new("yggdrasil-screencast");
            cmd.args(["--output", &output]);
            if cursor {
                cmd.arg("--cursor");
            }
            cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::inherit());
            // SAFETY: only an async-signal-safe prctl between fork and exec; the signal comes when this thread ends, which it
            // does only after the worker has ended (below) or when this whole process goes
            unsafe {
                cmd.pre_exec(|| rustix::process::set_parent_process_death_signal(Some(rustix::process::Signal::TERM)).map_err(Into::into));
            }
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(Ev::Failed(format!("could not start yggdrasil-screencast: {e}")));
                    return;
                }
            };
            let pidfd = rustix::process::Pid::from_raw(child.id() as i32)
                .ok_or(rustix::io::Errno::SRCH)
                .and_then(|p| rustix::process::pidfd_open(p, rustix::process::PidfdFlags::empty()));
            match pidfd {
                Ok(fd) => {
                    let _ = tx.send(Ev::Spawned(fd));
                }
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = tx.send(Ev::Failed(format!("the worker's pidfd: {e}")));
                    return;
                }
            }
            if let Some(out) = child.stdout.take() {
                let mut told = false;
                for l in BufReader::new(out).lines().map_while(Result::ok) {
                    if let Some(n) = l.strip_prefix("node ").and_then(|n| n.trim().parse().ok()) {
                        if !told {
                            told = true;
                            let _ = tx.send(Ev::Node(n));
                        }
                    }
                }
                if !told {
                    let _ = tx.send(Ev::Failed("the worker ended before its stream was there".into()));
                }
            }
            let _ = child.wait();
            on_exit();
        })
        .map_err(|e| e.to_string())?;
    match rx.recv_timeout(NODE_WAIT) {
        Ok(Ev::Spawned(fd)) => {
            let mut s = sessions.lock().unwrap();
            match s.get_mut(path) {
                Some(sess) => sess.workers.push(Worker { id, pidfd: fd, screen: output_name }),
                // the program closed the session meanwhile
                None => {
                    let _ = rustix::process::pidfd_send_signal(&fd, rustix::process::Signal::TERM);
                    return Err("the session was closed".into());
                }
            }
        }
        Ok(Ev::Failed(e)) => return Err(e),
        Ok(Ev::Node(_)) | Err(_) => return Err("the worker did not start".into()),
    }
    match rx.recv_timeout(NODE_WAIT) {
        Ok(Ev::Node(n)) => Ok(n),
        Ok(Ev::Failed(e)) => Err(e),
        _ => Err(format!("no stream within {} s", NODE_WAIT.as_secs())),
    }
}

pub struct ScreenCast {
    pub sessions: Sessions,
    /// the pickers running, by request handle (shared with the file chooser's: Request.Close stops one)
    pub running: Arc<Mutex<HashMap<String, u32>>>,
    pub next: AtomicU64,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCast {
    async fn create_session(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        _options: Options,
    ) -> (u32, Results) {
        let path = session_handle.to_string();
        self.sessions.lock().unwrap().insert(path.clone(), Sess::new());
        let made = server.at(session_handle.as_str(), Session { path: path.clone(), sessions: self.sessions.clone() }).await;
        if made.is_err() {
            self.sessions.lock().unwrap().remove(&path);
            return (2, Results::new());
        }
        let mut r = Results::new();
        if let Ok(v) = OwnedValue::try_from(Value::from(path)) {
            r.insert("session_id".into(), v);
        }
        (0, r)
    }

    async fn select_sources(
        &self,
        _handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        _app_id: String,
        options: Options,
    ) -> (u32, Results) {
        let mut s = self.sessions.lock().unwrap();
        let Some(sess) = s.get_mut(session_handle.as_str()) else { return (2, Results::new()) };
        sess.multiple = opt_bool(&options, "multiple");
        sess.cursor = if opt_u32(&options, "cursor_mode") == Some(CURSOR_EMBEDDED) { CURSOR_EMBEDDED } else { CURSOR_HIDDEN };
        (0, Results::new())
    }

    #[allow(clippy::too_many_arguments)]
    async fn start(
        &self,
        #[zbus(object_server)] server: &ObjectServer,
        #[zbus(connection)] conn: &zbus::Connection,
        handle: OwnedObjectPath,
        session_handle: OwnedObjectPath,
        app_id: String,
        _parent_window: String,
        _options: Options,
    ) -> (u32, Results) {
        let path = session_handle.to_string();
        let (multiple, cursor) = match self.sessions.lock().unwrap().get(&path) {
            Some(s) if !s.started && s.workers.is_empty() => (s.multiple, s.cursor == CURSOR_EMBEDDED),
            _ => return (2, Results::new()),
        };
        let _ = server.at(handle.as_str(), Request { handle: handle.to_string(), running: self.running.clone() }).await;
        let (h, run, id) = (handle.to_string(), self.running.clone(), app_id.clone());
        let picked = blocking::unblock(move || pick_screens(&h, &run, &id, multiple)).await;
        let _ = server.remove::<Request, _>(handle.as_str()).await;
        let Some(names) = picked else { return (1, Results::new()) };
        let screens = blocking::unblock(|| yggdrasil_wm::Desk::detect().map(|d| d.screens()).unwrap_or_default()).await;

        let bus = zbus::blocking::Connection::from(conn.clone());
        let mut streams = vec![];
        for (i, name) in names.iter().enumerate() {
            let id = self.next.fetch_add(1, Ordering::Relaxed);
            let (sessions, p, b) = (self.sessions.clone(), path.clone(), bus.clone());
            let on_exit = move || worker_ended(&sessions, &b, &p, id);
            let (sessions, p, n) = (self.sessions.clone(), path.clone(), name.clone());
            let node = blocking::unblock(move || launch(&sessions, &p, id, &n, cursor, on_exit)).await;
            match node {
                Ok(node) => streams.push(stream_entry(node, i, screens.iter().find(|s| s.name == *name))),
                Err(e) => {
                    eprintln!("yggdrasil-portal: sharing {name}: {e}");
                    if let Some(s) = self.sessions.lock().unwrap().get_mut(&path) {
                        s.stop();
                    }
                    return (2, Results::new());
                }
            }
        }
        let app = yggdrasil_conf::apps::app_name(&app_id).unwrap_or_else(|| "A program".into());
        match self.sessions.lock().unwrap().get_mut(&path) {
            Some(s) => {
                s.started = true;
                s.app = app;
            }
            None => return (2, Results::new()),
        }
        publish(&self.sessions);
        let mut r = Results::new();
        if let Ok(v) = OwnedValue::try_from(Value::from(streams)) {
            r.insert("streams".into(), v);
        }
        (0, r)
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        SOURCE_MONITOR
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        CURSOR_HIDDEN | CURSOR_EMBEDDED
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        5
    }
}

/// A worker ended: out of its session; when it was the session's last and the session had started, the session is over and
/// says Closed.
fn worker_ended(sessions: &Sessions, bus: &zbus::blocking::Connection, path: &str, id: u64) {
    let closed = {
        let mut s = sessions.lock().unwrap();
        let Some(sess) = s.get_mut(path) else { return };
        sess.workers.retain(|w| w.id != id);
        let over = sess.started && sess.workers.is_empty();
        if over {
            s.remove(path);
        }
        over
    };
    publish(sessions);
    if closed {
        let _ = bus.emit_signal(None::<zbus::names::BusName>, path, SESSION_IFACE, "Closed", &());
        let _ = bus.object_server().remove::<Session, _>(path);
    }
}

/// Yggdrasil's own control of screen sharing: the bar's Stop sharing.
pub struct Sharing {
    pub sessions: Sessions,
}

#[zbus::interface(name = "org.yggdrasil.ScreenSharing")]
impl Sharing {
    /// Every share ends at once: each worker is stopped, and each session says Closed once its workers have ended.
    fn stop_all(&self) {
        for s in self.sessions.lock().unwrap().values_mut() {
            s.stop();
        }
        publish(&self.sessions);
    }
}

/// A session's object: the program (through the frontend) closes it.
pub struct Session {
    path: String,
    sessions: Sessions,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Session")]
impl Session {
    async fn close(&self, #[zbus(object_server)] server: &ObjectServer) {
        let gone = self.sessions.lock().unwrap().remove(&self.path);
        if let Some(mut s) = gone {
            s.stop();
            publish(&self.sessions);
        }
        let _ = server.remove::<Session, _>(self.path.as_str()).await;
    }

    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        1
    }
}

#[cfg(test)]
mod tests {

    /// Many writes at once (as when a screen is unplugged: its worker ends while its session closes) never leave the bar a
    /// file it cannot read.
    #[test]
    fn writes_at_once_always_leave_a_whole_file() {
        let dir = std::env::temp_dir().join(format!("ygg-portal-bar-{}", std::process::id()));
        let path = dir.join("screen-sharing.json");
        let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));
        publish_to(&path, &sessions);
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                let mut bad = 0;
                while !stop.load(Ordering::Relaxed) {
                    let text = std::fs::read_to_string(&path).unwrap_or_default();
                    if serde_json::from_str::<serde_json::Value>(&text).is_err() {
                        bad += 1;
                    }
                }
                bad
            })
        };
        let writers: Vec<_> = (0..8)
            .map(|_| {
                let (path, sessions) = (path.clone(), sessions.clone());
                std::thread::spawn(move || (0..300).for_each(|_| publish_to(&path, &sessions)))
            })
            .collect();
        writers.into_iter().for_each(|w| w.join().unwrap());
        stop.store(true, Ordering::Relaxed);
        assert_eq!(reader.join().unwrap(), 0, "a read found the file empty, half written or missing");
        let _ = std::fs::remove_dir_all(&dir);
    }
    use super::*;

    fn screen(name: &str, x: i32, w: u32, h: u32, scale: f64, transform: u32) -> yggdrasil_wm::Screen {
        yggdrasil_wm::Screen {
            id: None,
            name: name.into(),
            make: String::new(),
            model: String::new(),
            x,
            y: 0,
            width: w,
            height: h,
            scale: Some(scale),
            transform,
            reserved: [0; 4],
            focused: false,
            disabled: false,
            active_workspace: None,
            special_workspace: None,
            special_workspace_id: None,
            refresh: None,
            modes: vec![],
            mirror_of: None,
        }
    }

    #[test]
    fn sizes_are_the_compositors() {
        assert_eq!(logical_size(&screen("DP-1", 0, 3440, 1440, 1.0, 0)), (3440, 1440));
        assert_eq!(logical_size(&screen("eDP-1", 0, 2880, 1800, 2.0, 0)), (1440, 900));
        // turned a quarter: width and height swap
        assert_eq!(logical_size(&screen("DP-2", 0, 1920, 1080, 1.0, 1)), (1080, 1920));
        assert_eq!(logical_size(&screen("DP-3", 0, 2560, 1440, 1.25, 0)), (2048, 1152));
    }

    #[test]
    fn the_bar_shows_what_is_shared_and_nothing_else() {
        assert_eq!(bar_json(&[]), "{\"state\": \"off\"}\n");
        // a session whose workers have all ended shares nothing
        assert_eq!(bar_json(&[("Example Call".into(), vec![])]), "{\"state\": \"off\"}\n");
        let one = bar_json(&[("Example Call".into(), vec!["DP-1".into()])]);
        assert!(one.contains("\"text\": \"Sharing\"") && one.contains("\"state\": \"alert\""), "{one}");
        assert!(one.contains("{\"title\": \"Example Call\", \"value\": \"DP-1\"}"), "{one}");
        assert!(one.contains("\"id\": \"stop\""), "{one}");
        let two = bar_json(&[("Say \"hi\"\\".into(), vec!["DP-1".into(), "DP-2".into()]), ("A program".into(), vec!["eDP-1".into()])]);
        assert!(two.contains("\"Sharing 3 screens\"") && two.contains("\"DP-1, DP-2\""), "{two}");
        assert!(two.contains(r#""title": "Say \"hi\"\\""#), "a name is quoted, never ends the string: {two}");
        // valid JSON, read back as the panel reads it
        for text in [one, two, bar_json(&[])] {
            let v: serde_json::Value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{e}: {text}"));
            assert!(v["state"].is_string());
        }
        let v: serde_json::Value = serde_json::from_str(&bar_json(&[("Say \"hi\"\\\n".into(), vec!["DP-1".into()])])).unwrap();
        assert_eq!(v["lines"][0]["title"], "Say \"hi\"\\\n");
        assert_eq!(v["card"][0]["buttons"][0]["id"], "stop");
    }

    #[test]
    fn the_bar_item_reads_the_file_the_portal_writes() {
        let i = yggdrasil_conf::integrations::Integration::parse(
            std::path::Path::new("screen-sharing.conf"),
            include_str!("../data/screen-sharing.conf"),
        )
        .unwrap();
        assert_eq!(i.kind, "bar-item");
        // bar_file() with the state folder at its usual place (%h/.local/state/yggdrasil)
        assert_eq!(i.get("file"), Some("%h/.local/state/yggdrasil/screen-sharing.json"));
        assert_eq!(bar_file().file_name().and_then(|n| n.to_str()), Some("screen-sharing.json"));
        // Stop sharing: the panel runs act with the button's id
        assert_eq!(i.command("act"), Some(vec!["/usr/bin/yggdrasil-portal".to_string(), "--stop-sharing".to_string()]));
    }

    #[test]
    fn the_pickers_answer_is_read_carefully() {
        assert_eq!(picked_names("DP-1\n\nDP-1\nHDMI-A-1\n", true), vec!["DP-1", "HDMI-A-1"]);
        assert_eq!(picked_names("DP-1\nHDMI-A-1\n", false), vec!["DP-1"]);
        assert!(picked_names("\n  \n", true).is_empty());
    }

    #[test]
    fn a_stream_says_where_its_screen_is() {
        let s = screen("DP-2", 3440, 2560, 1440, 1.0, 0);
        let (node, p) = stream_entry(42, 1, Some(&s));
        assert_eq!(node, 42);
        assert_eq!(<(i32, i32)>::try_from(p["position"].clone()).unwrap(), (3440, 0));
        assert_eq!(<(i32, i32)>::try_from(p["size"].clone()).unwrap(), (2560, 1440));
        assert_eq!(u32::try_from(&p["source_type"]).unwrap(), SOURCE_MONITOR);
        assert_eq!(String::try_from(p["id"].clone()).unwrap(), "1");
        // the connector's name is not handed to the program
        assert!(!p.values().any(|v| String::try_from(v.clone()).is_ok_and(|t| t.contains("DP"))));
        // a screen the compositor did not describe: the node alone, with its type
        let (_, bare) = stream_entry(7, 0, None);
        assert!(!bare.contains_key("position") && !bare.contains_key("size"));
    }
}
