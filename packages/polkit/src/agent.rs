//! The D-Bus side: registers with polkit as an authentication agent and answers `BeginAuthentication`.

use std::collections::HashMap;
use std::sync::mpsc::{channel, Sender};

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
    pub ui: Sender<UiMsg>,
    pub wake: std::sync::Arc<dyn Fn() + Send + Sync>,
    pub uid: u32,
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

struct Bridge<'a> {
    agent: &'a Agent,
}

impl Ui for Bridge<'_> {
    fn prompt(&mut self, text: &str, echo: bool) -> Option<String> {
        let (tx, rx) = channel();
        self.agent.ui.send(UiMsg::Prompt { text: text.to_string(), echo, reply: tx }).ok()?;
        (self.agent.wake)();
        rx.recv().ok().flatten()
    }
    fn message(&mut self, text: &str, error: bool) {
        let _ = self.agent.ui.send(UiMsg::Message { text: text.to_string(), error });
        (self.agent.wake)();
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
        let _ = self.ui.send(UiMsg::Begin { message, action_id, user: user.clone() });
        (self.wake)();
        // a wrong password gets three tries, each a new session with the helper (as the standard agents do)
        let mut result = Outcome::Failed;
        for attempt in 0..3 {
            let mut ui = Bridge { agent: self };
            result = helper::authenticate(&user, &cookie, &mut ui).map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
            if result != Outcome::Failed {
                break;
            }
            if attempt < 2 {
                ui.message("Wrong password, try again.", true);
            }
        }
        let _ = self.ui.send(UiMsg::Done { ok: result == Outcome::Authenticated });
        (self.wake)();
        match result {
            Outcome::Authenticated => Ok(()),
            Outcome::Cancelled => Err(zbus::fdo::Error::Failed("org.freedesktop.PolicyKit1.Error.Cancelled".into())),
            Outcome::Failed => Err(zbus::fdo::Error::AuthFailed("authentication failed".into())),
        }
    }

    fn cancel_authentication(&self, _cookie: String) {
        let _ = self.ui.send(UiMsg::Done { ok: false });
        (self.wake)();
    }
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
