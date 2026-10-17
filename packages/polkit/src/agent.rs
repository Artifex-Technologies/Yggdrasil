//! The D-Bus side: registers with polkit as an authentication agent and answers `BeginAuthentication`.

use std::collections::HashMap;
use std::sync::mpsc::Sender;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

use crate::helper::{self, Outcome, Ui};

pub const OBJECT_PATH: &str = "/org/labyrinth/PolicyKit1/AuthenticationAgent";

/// What the dialog is asked to do.
pub enum UiMsg {
    /// Start (or continue) a dialog for this request.
    Begin { message: String, action_id: String, user: String },
    Prompt { text: String, echo: bool, reply: Sender<Option<String>> },
    Message { text: String, error: bool },
    /// The request is over (close the dialog).
    Done { ok: bool },
}

pub struct Agent {
    pub uid: u32,
    /// The password-box process of the request being answered (its pid), so a cancel from polkit can close it.
    pub child: std::sync::Mutex<Option<u32>>,
}

/// The user name for a uid.
pub fn user_name(uid: u32) -> Option<String> {
    let mut buf = vec![0u8; 4096];
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut res: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr() as *mut libc::c_char, buf.len(), &mut res) };
    if rc != 0 || res.is_null() {
        return None;
    }
    Some(unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }.to_string_lossy().into_owned())
}

/// Which identity to authenticate as: our own user when polkit offers it, else the first user it lists.
pub fn choose_identity(identities: &[(String, HashMap<String, OwnedValue>)], own_uid: u32) -> Option<u32> {
    let uids: Vec<u32> = identities.iter().filter(|(k, _)| k == "unix-user").filter_map(|(_, d)| d.get("uid").and_then(|v| u32::try_from(v).ok())).collect();
    if uids.contains(&own_uid) {
        Some(own_uid)
    } else {
        uids.first().copied()
    }
}

/// The password box runs as its own process, one per request (`labyrinth-polkit --dialog`), talking JSON lines on its pipes:
/// requests go to its stdin, the typed answers come back on its stdout. When the request is over the process ends and its
/// window goes with it.
struct Bridge {
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl Bridge {
    fn send(&mut self, v: serde_json::Value) {
        use std::io::Write;
        let _ = writeln!(self.stdin, "{v}");
        let _ = self.stdin.flush();
    }
}

impl Ui for Bridge {
    fn prompt(&mut self, text: &str, echo: bool) -> Option<String> {
        use std::io::BufRead;
        self.send(serde_json::json!({"t": "prompt", "text": text, "echo": echo}));
        let mut line = String::new();
        self.stdout.read_line(&mut line).ok()?;
        let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
        v.get("answer").and_then(|a| a.as_str()).map(str::to_string)
    }
    fn message(&mut self, text: &str, error: bool) {
        self.send(serde_json::json!({"t": "message", "text": text, "error": error}));
    }
}

#[zbus::interface(name = "org.freedesktop.PolicyKit1.AuthenticationAgent")]
impl Agent {
    #[allow(clippy::too_many_arguments)]
    fn begin_authentication(&self, action_id: String, message: String, _icon_name: String, _details: HashMap<String, String>, cookie: String, identities: Vec<(String, HashMap<String, OwnedValue>)>) -> zbus::fdo::Result<()> {
        let Some(uid) = choose_identity(&identities, self.uid) else {
            return Err(zbus::fdo::Error::Failed("no user to authenticate as".into()));
        };
        let user = user_name(uid).ok_or_else(|| zbus::fdo::Error::Failed("unknown user".into()))?;
        let exe = std::env::current_exe().map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        let mut child = std::process::Command::new(exe)
            .arg("--dialog")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| zbus::fdo::Error::Failed(format!("cannot open the password box: {e}")))?;
        *self.child.lock().unwrap() = Some(child.id());
        let mut ui = Bridge { stdin: child.stdin.take().unwrap(), stdout: std::io::BufReader::new(child.stdout.take().unwrap()) };
        ui.send(serde_json::json!({"t": "begin", "message": message, "action_id": action_id, "user": user}));
        // a wrong password gets three tries, each a new session with the helper (as the standard agents do)
        let mut result = Outcome::Failed;
        for attempt in 0..3 {
            result = match helper::authenticate(&user, &cookie, &mut ui) {
                Ok(r) => r,
                Err(e) => {
                    ui.send(serde_json::json!({"t": "done"}));
                    let _ = child.wait();
                    return Err(zbus::fdo::Error::Failed(e.to_string()));
                }
            };
            if result != Outcome::Failed {
                break;
            }
            if attempt < 2 {
                ui.message("Wrong password, try again.", true);
            }
        }
        ui.send(serde_json::json!({"t": "done"}));
        drop(ui);
        let _ = child.wait();
        *self.child.lock().unwrap() = None;
        match result {
            Outcome::Authenticated => Ok(()),
            Outcome::Cancelled => Err(zbus::fdo::Error::Failed("org.freedesktop.PolicyKit1.Error.Cancelled".into())),
            Outcome::Failed => Err(zbus::fdo::Error::AuthFailed("authentication failed".into())),
        }
    }

    /// polkit gave up on the request (the program asking ended): close the box.
    fn cancel_authentication(&self, _cookie: String) {
        if let Some(pid) = self.child.lock().unwrap().take() {
            unsafe { kill(pid as i32, 15) };
        }
    }
}

extern "C" {
    fn kill(pid: i32, sig: i32) -> i32;
}

/// Which session or process the agent serves.
pub enum Subject {
    Session(String),
    Process(u32),
}

fn process_start_time(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // field 22, counted after the "(comm)" which may contain spaces
    let after = stat.rsplit_once(')')?.1;
    after.split_whitespace().nth(19)?.parse().ok()
}

/// Exports the agent on the system bus and registers it with polkit. Keeps the connection alive for the caller.
pub fn register(agent: Agent, subject: Subject) -> Result<Connection, String> {
    let conn = Connection::system().map_err(|e| e.to_string())?;
    conn.object_server().at(OBJECT_PATH, agent).map_err(|e| e.to_string())?;
    let (kind, mut details): (&str, HashMap<&str, Value>) = match subject {
        Subject::Session(id) => ("unix-session", HashMap::from([("session-id", Value::from(id))])),
        Subject::Process(pid) => {
            let start = process_start_time(pid).ok_or("cannot read that process's start time")?;
            ("unix-process", HashMap::from([("pid", Value::from(pid)), ("start-time", Value::from(start))]))
        }
    };
    let _ = &mut details;
    let proxy = Proxy::new(&conn, "org.freedesktop.PolicyKit1", "/org/freedesktop/PolicyKit1/Authority", "org.freedesktop.PolicyKit1.Authority").map_err(|e| e.to_string())?;
    proxy.call::<_, _, ()>("RegisterAuthenticationAgent", &((kind, details), "en_US.UTF-8", OBJECT_PATH)).map_err(|e| e.to_string())?;
    Ok(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(kind: &str, key: &str, v: u32) -> (String, HashMap<String, OwnedValue>) {
        (kind.to_string(), HashMap::from([(key.to_string(), OwnedValue::try_from(Value::from(v)).unwrap())]))
    }

    #[test]
    fn identity_prefers_the_current_user() {
        let ids = vec![id("unix-user", "uid", 0), id("unix-user", "uid", 1000), id("unix-group", "gid", 10)];
        assert_eq!(choose_identity(&ids, 1000), Some(1000));
        assert_eq!(choose_identity(&ids, 1234), Some(0));
        assert_eq!(choose_identity(&[id("unix-group", "gid", 10)], 1000), None);
    }

    #[test]
    fn root_has_a_name() {
        assert_eq!(user_name(0).as_deref(), Some("root"));
        assert!(process_start_time(std::process::id()).is_some());
    }
}
