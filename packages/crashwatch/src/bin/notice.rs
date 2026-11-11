//! crashwatch-notice EXE SIGNAL: one crash notice, started by yggdrasil-crashwatch. It speaks org.freedesktop.Notifications on
//! the session bus. With an assistant installed, the notice has an "Ask NAME" action for as long as the program stays quiet
//! (ten minutes); clicking it asks the assistant's `question` command, shows "NAME is looking at it" at once and replaces that
//! with the answer. Then, or when the notice is closed, it exits.
//!
//!   crashwatch-notice --print EXE SIGNAL    what it would show, printed; no D-Bus
//!
//! `CRASHWATCH_INTEGRATIONS` (folders separated by `:`) reads the integration files from elsewhere (tests).

use std::collections::HashMap;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use yggdrasil_conf::integrations::{self, Integration};
use yggdrasil_crashwatch::core::{clean_exe, program_name};
use yggdrasil_crashwatch::notice::{self, Assistant, Notice, ASK};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::Value;

/// How long "Ask" can be clicked: as long as the program stays quiet.
const OFFER: Duration = Duration::from_secs(yggdrasil_crashwatch::quiet::QUIET_SECS);
/// How long the assistant may take to answer.
const ANSWER: Duration = Duration::from_secs(180);

static ASKING: AtomicBool = AtomicBool::new(false);

fn integration_files() -> Vec<Integration> {
    match std::env::var_os("CRASHWATCH_INTEGRATIONS") {
        Some(d) => integrations::all_in(&std::env::split_paths(&d).collect::<Vec<_>>()),
        None => integrations::all(),
    }
}

fn notify(p: &Proxy, n: &Notice, replaces: u32) -> zbus::Result<u32> {
    let actions: Vec<&str> = n.actions.iter().flat_map(|(k, l)| [k.as_str(), l.as_str()]).collect();
    let hints: HashMap<&str, Value> = HashMap::new();
    p.call("Notify", &(n.app.as_str(), replaces, n.icon, n.summary.as_str(), n.body.as_str(), actions, hints, n.timeout))
}

/// Runs the assistant's question command with the question as its last argument, in the home folder, for at most ANSWER. It
/// runs in a process group of its own, so a timeout stops what it started too. Returns (standard output, standard error).
fn run_question(cmd: &[String], question: &str) -> Result<(String, String), String> {
    let home = std::env::var_os("HOME").map(PathBuf::from).filter(|h| h.is_dir()).unwrap_or_else(|| PathBuf::from("/"));
    let mut child = Command::new(&cmd[0])
        .args(&cmd[1..])
        .arg(question)
        .current_dir(&home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .map_err(|e| e.to_string())?;
    let (done_tx, done_rx) = mpsc::channel();
    let mut bufs = vec![];
    let pipes: [Option<Box<dyn Read + Send>>; 2] =
        [child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>), child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>)];
    for pipe in pipes.into_iter().flatten() {
        let buf = Arc::new(Mutex::new(Vec::new()));
        bufs.push(buf.clone());
        let tx = done_tx.clone();
        std::thread::spawn(move || {
            let mut pipe = pipe;
            let mut chunk = [0u8; 4096];
            while let Ok(n) = pipe.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.lock().unwrap().extend_from_slice(&chunk[..n]);
            }
            let _ = tx.send(());
        });
    }
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => {}
            Err(e) => return Err(e.to_string()),
        }
        if start.elapsed() > ANSWER {
            // SAFETY: a signal to the process group it leads
            unsafe { libc::kill(-(child.id() as i32), libc::SIGKILL) };
            let _ = child.wait();
            return Err("no answer within three minutes".into());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    // the pipes close when it ends; a helper it left running would keep them open, so what came so far is used after a moment
    for _ in 0..bufs.len() {
        if done_rx.recv_timeout(Duration::from_secs(2)).is_err() {
            break;
        }
    }
    let text = |b: &Arc<Mutex<Vec<u8>>>| String::from_utf8_lossy(&b.lock().unwrap()).into_owned();
    Ok((bufs.first().map(text).unwrap_or_default(), bufs.get(1).map(text).unwrap_or_default()))
}

fn ask(p: &Proxy, a: &Assistant, exe: &str, signal: &str) -> zbus::Result<()> {
    let program = program_name(exe);
    // at once, so the click visibly did something (the answer takes a while and replaces this one)
    let shown = notify(p, &notice::looking_notice(program, a), 0).unwrap_or(0);
    let text = match run_question(&a.question, &notice::question(program, signal, a)) {
        Ok((out, err)) => notice::answer_text(&out, &err, &a.name),
        Err(e) => format!("{} could not look: {e}", a.name),
    };
    notify(p, &notice::answer_notice(program, a, &text), shown)?;
    Ok(())
}

fn show(exe: &str, signal: &str, a: Option<Assistant>, n: &Notice) -> zbus::Result<()> {
    let conn = Connection::session()?;
    let p = Proxy::new(&conn, "org.freedesktop.Notifications", "/org/freedesktop/Notifications", "org.freedesktop.Notifications")?;
    let Some(a) = a.filter(|_| !n.actions.is_empty()) else {
        notify(&p, n, 0)?;
        return Ok(());
    };
    // listening before it is shown, so a quick click is not missed
    let mut signals = p.receive_all_signals()?;
    let id = notify(&p, n, 0)?;
    std::thread::spawn(|| {
        std::thread::sleep(OFFER);
        if !ASKING.load(Ordering::SeqCst) {
            std::process::exit(0);
        }
    });
    loop {
        let Some(m) = signals.next() else { return Ok(()) };
        let body = m.body();
        match m.header().member().map(|x| x.as_str()) {
            Some("ActionInvoked") if body.deserialize::<(u32, String)>().is_ok_and(|(nid, key)| nid == id && key == ASK) => {
                ASKING.store(true, Ordering::SeqCst);
                break;
            }
            Some("NotificationClosed") if body.deserialize::<(u32, u32)>().is_ok_and(|(nid, _)| nid == id) => return Ok(()),
            _ => {}
        }
    }
    drop(signals);
    ask(&p, &a, exe, signal)
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let print = args.first().is_some_and(|a| a == "--print");
    if print {
        args.remove(0);
    }
    if args.len() != 2 {
        eprintln!("usage: crashwatch-notice [--print] EXE SIGNAL");
        std::process::exit(2);
    }
    let exe = Some(clean_exe(&args[0])).filter(|e| !e.is_empty()).unwrap_or_else(|| "a program".into());
    let signal: String = args[1].chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, ' ' | '+')).take(20).collect();
    let signal = if signal.trim().is_empty() { "a signal".to_string() } else { signal };
    let a = notice::assistant_from(&integration_files());
    let n = notice::crash_notice(&exe, &signal, a.as_ref());
    if print {
        println!("app: {}\nicon: {}\nsummary: {}\nbody: {}\ntimeout: {}", n.app, n.icon, n.summary, n.body, n.timeout);
        for (k, l) in &n.actions {
            println!("action: {k} = {l}");
        }
        if let Some(a) = &a {
            println!("question: {}", notice::question(program_name(&exe), &signal, a));
        }
        return;
    }
    if let Err(e) = show(&exe, &signal, a, &n) {
        eprintln!("crashwatch-notice: {e}");
        std::process::exit(1);
    }
}
