//! `yggdrasil-clipboard daemon`: watches the clipboard (with `wl-paste --watch`) while Settings' "Clipboard history" is on, keeps
//! what is copied in memory only, and answers the window on a socket in the runtime folder (only this user can open it). Turning
//! the setting off stops the watching and forgets everything; so does logging out.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::store::{self, Kind, Request, Store};

/// Run by `wl-paste --watch` on every copy: "T <base64>" for text, "I <base64>" for a PNG, nothing for what a password manager
/// marked as secret (wl-clipboard's CLIPBOARD_STATE, KDE's hint) or for anything else.
const WATCH_SCRIPT: &str = r#"case "$CLIPBOARD_STATE" in sensitive|clear|nil) cat >/dev/null; exit 0;; esac
t=$(wl-paste --list-types 2>/dev/null)
case "$t" in *x-kde-passwordManagerHint*) cat >/dev/null; exit 0;; esac
if printf '%s\n' "$t" | grep -qE '^(text/plain|UTF8_STRING|TEXT|STRING)'; then
  printf 'T '; wl-paste --no-newline --type text 2>/dev/null | head -c 65537 | base64 -w0; echo
elif printf '%s\n' "$t" | grep -qx 'image/png'; then
  printf 'I '; wl-paste --type image/png 2>/dev/null | head -c 8388609 | base64 -w0; echo
fi
cat >/dev/null"#;

pub fn socket_path() -> std::path::PathBuf {
    let run = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| std::env::temp_dir());
    run.join("yggdrasil-clipboard.sock")
}

fn now() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

struct Shared {
    store: Store,
    on: bool,
}

/// Puts bytes on the clipboard with wl-copy (which forks to serve it, so waiting is short).
pub fn wl_copy(data: &[u8], mime: Option<&str>) -> bool {
    let mut cmd = Command::new("wl-copy");
    if let Some(m) = mime {
        cmd.args(["--type", m]);
    }
    let Ok(mut c) = cmd.stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let ok = c.stdin.take().map(|mut i| i.write_all(data).is_ok()).unwrap_or(false);
    c.wait().map(|s| s.success()).unwrap_or(false) && ok
}

fn start_watch(shared: Arc<Mutex<Shared>>) -> Option<Child> {
    use std::os::unix::process::CommandExt;
    let mut cmd = Command::new("wl-paste");
    cmd.args(["--watch", "sh", "-c", WATCH_SCRIPT]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
    // the watcher ends with the daemon, however the daemon ends
    unsafe {
        cmd.pre_exec(|| {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            Ok(())
        });
    }
    let mut child = cmd.spawn().ok()?;
    let out = child.stdout.take()?;
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            let (tag, b64) = line.split_at(line.len().min(2));
            let Some(bytes) = store::b64_decode(b64) else { continue };
            let Ok(mut g) = shared.lock() else { return };
            if !g.on {
                continue;
            }
            match tag {
                "T " => {
                    if let Ok(t) = String::from_utf8(bytes) {
                        g.store.push_text(t, now());
                    }
                }
                "I " => {
                    g.store.push_image(bytes, now());
                }
                _ => {}
            }
        }
    });
    Some(child)
}

fn answer(shared: &Arc<Mutex<Shared>>, req: Request) -> serde_json::Value {
    use serde_json::json;
    let Ok(mut g) = shared.lock() else { return json!({"ok": false}) };
    match req {
        Request::List => g.store.list_json(g.on),
        Request::Copy(id) => {
            let Some(item) = g.store.get(id).cloned() else { return json!({"ok": false, "error": "gone"}) };
            drop(g);
            let ok = match &item.kind {
                Kind::Text(t) => wl_copy(t.as_bytes(), None),
                Kind::Image { png, .. } => wl_copy(png, Some("image/png")),
            };
            json!({"ok": ok})
        }
        Request::Delete(id) => json!({"ok": g.store.delete(id)}),
        Request::Clear => {
            g.store.clear();
            json!({"ok": true})
        }
    }
}

fn serve(shared: Arc<Mutex<Shared>>, stream: UnixStream) {
    let mut w = match stream.try_clone() {
        Ok(w) => w,
        Err(_) => return,
    };
    let mut r = BufReader::new(stream);
    loop {
        let mut line = String::new();
        // one request per line, none longer than 4 KiB
        match (&mut r).take(4096).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let reply = match store::parse_request(&line) {
            Some(req) => answer(&shared, req),
            None => serde_json::json!({"ok": false, "error": "bad request"}),
        };
        if writeln!(w, "{reply}").is_err() {
            return;
        }
    }
}

/// The setting, and when its file last changed (so it is read again only then).
fn setting_stamp() -> Option<SystemTime> {
    yggdrasil_conf::PanelConf::path().and_then(|p| std::fs::metadata(p).ok()).and_then(|m| m.modified().ok())
}

pub fn run() -> std::process::ExitCode {
    let path = socket_path();
    if UnixStream::connect(&path).is_ok() {
        eprintln!("yggdrasil-clipboard: the daemon is already running");
        return std::process::ExitCode::from(1);
    }
    let _ = std::fs::remove_file(&path);
    let listener = match UnixListener::bind(&path) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("yggdrasil-clipboard: cannot listen on {}: {e}", path.display());
            return std::process::ExitCode::from(1);
        }
    };
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    let shared = Arc::new(Mutex::new(Shared { store: Store::default(), on: false }));
    {
        let shared = shared.clone();
        std::thread::spawn(move || {
            for s in listener.incoming().flatten() {
                let shared = shared.clone();
                std::thread::spawn(move || serve(shared, s));
            }
        });
    }
    let mut child: Option<Child> = None;
    let mut stamp: Option<Option<SystemTime>> = None;
    let mut on = false;
    loop {
        let st = setting_stamp();
        if stamp != Some(st) {
            stamp = Some(st);
            on = yggdrasil_conf::PanelConf::load().clipboard_history;
            if let Ok(mut g) = shared.lock() {
                g.on = on;
                if !on {
                    g.store.clear();
                }
            }
        }
        // a watcher that ended (no display yet, wl-paste restarted) is started again on the next round
        if let Some(c) = &mut child {
            if !on || !matches!(c.try_wait(), Ok(None)) {
                let _ = c.kill();
                let _ = c.wait();
                child = None;
            }
        }
        if on && child.is_none() {
            child = start_watch(shared.clone());
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}
