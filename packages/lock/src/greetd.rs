//! The login screen's side of greetd's protocol (https://man.sr.ht/~kennylevinsen/greetd/): JSON messages, each with a 4-byte
//! length in the machine's byte order in front, over the socket named by GREETD_SOCK. greetd runs PAM and starts the session;
//! this only passes along what was typed.

use serde_json::{json, Value};
use std::io::{Read, Write};

pub struct Greetd<S: Read + Write> {
    s: S,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Fail {
    /// The password (or user) was not accepted.
    Auth(String),
    /// greetd or the connection broke.
    Other(String),
}

impl Greetd<std::os::unix::net::UnixStream> {
    pub fn connect() -> Result<Self, String> {
        let path = std::env::var("GREETD_SOCK").map_err(|_| "GREETD_SOCK is not set (this program is started by greetd)".to_string())?;
        std::os::unix::net::UnixStream::connect(path).map(|s| Greetd { s }).map_err(|e| e.to_string())
    }
}

impl<S: Read + Write> Greetd<S> {
    pub fn new(s: S) -> Self {
        Greetd { s }
    }

    fn send(&mut self, v: Value) -> Result<(), String> {
        let body = v.to_string();
        self.s.write_all(&(body.len() as u32).to_ne_bytes()).and_then(|_| self.s.write_all(body.as_bytes())).and_then(|_| self.s.flush()).map_err(|e| e.to_string())
    }

    fn recv(&mut self) -> Result<Value, String> {
        let mut len = [0u8; 4];
        self.s.read_exact(&mut len).map_err(|e| e.to_string())?;
        let n = u32::from_ne_bytes(len) as usize;
        if n > 1 << 20 {
            return Err("greetd sent a message that is too large".into());
        }
        let mut body = vec![0u8; n];
        self.s.read_exact(&mut body).map_err(|e| e.to_string())?;
        serde_json::from_slice(&body).map_err(|e| e.to_string())
    }

    /// Logs `user` in with `password` and starts `cmd`. Ok means greetd started the session: the greeter must exit now.
    pub fn login(&mut self, user: &str, password: &str, cmd: &[String], env: &[String]) -> Result<(), Fail> {
        let other = |e: String| Fail::Other(e);
        self.send(json!({"type": "create_session", "username": user})).map_err(other)?;
        let mut answered = false;
        loop {
            let r = self.recv().map_err(Fail::Other)?;
            match r["type"].as_str().unwrap_or("") {
                "success" => break,
                "error" => {
                    let _ = self.send(json!({"type": "cancel_session"}));
                    let text = r["description"].as_str().unwrap_or("").to_string();
                    return Err(if r["error_type"] == "auth_error" { Fail::Auth(text) } else { Fail::Other(text) });
                }
                "auth_message" => match r["auth_message_type"].as_str().unwrap_or("") {
                    "secret" | "visible" => {
                        if answered {
                            // asked again (a changed-password prompt, a second factor): this greeter cannot answer that
                            let _ = self.send(json!({"type": "cancel_session"}));
                            let _ = self.recv();
                            return Err(Fail::Auth("The system wants more than a password; use another way to sign in".into()));
                        }
                        answered = true;
                        self.send(json!({"type": "post_auth_message_response", "response": password})).map_err(other)?;
                    }
                    _ => self.send(json!({"type": "post_auth_message_response", "response": Value::Null})).map_err(other)?,
                },
                t => return Err(Fail::Other(format!("greetd sent {t}"))),
            }
        }
        self.send(json!({"type": "start_session", "cmd": cmd, "env": env})).map_err(other)?;
        let r = self.recv().map_err(Fail::Other)?;
        if r["type"] == "success" {
            Ok(())
        } else {
            Err(Fail::Other(r["description"].as_str().unwrap_or("the session did not start").to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixStream;

    /// A stand-in greetd: answers the conversation with the given replies and returns what it was sent.
    fn server(replies: Vec<Value>) -> (UnixStream, std::thread::JoinHandle<Vec<Value>>) {
        let (a, mut b) = UnixStream::pair().unwrap();
        let h = std::thread::spawn(move || {
            let mut got = vec![];
            for r in replies {
                let mut len = [0u8; 4];
                if b.read_exact(&mut len).is_err() {
                    break;
                }
                let mut body = vec![0u8; u32::from_ne_bytes(len) as usize];
                b.read_exact(&mut body).unwrap();
                got.push(serde_json::from_slice(&body).unwrap());
                let out = r.to_string();
                b.write_all(&(out.len() as u32).to_ne_bytes()).unwrap();
                b.write_all(out.as_bytes()).unwrap();
            }
            got
        });
        (a, h)
    }

    #[test]
    fn right_password_starts_the_session() {
        let (c, h) = server(vec![
            json!({"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "}),
            json!({"type": "success"}),
            json!({"type": "success"}),
        ]);
        let mut g = Greetd::new(c);
        assert_eq!(g.login("ann", "pw", &["startplasma-wayland".into()], &[]), Ok(()));
        let got = h.join().unwrap();
        assert_eq!(got[0]["type"], "create_session");
        assert_eq!(got[0]["username"], "ann");
        assert_eq!(got[1]["response"], "pw");
        assert_eq!(got[2]["type"], "start_session");
        assert_eq!(got[2]["cmd"][0], "startplasma-wayland");
    }

    #[test]
    fn wrong_password_is_an_auth_failure_and_cancels() {
        let (c, h) = server(vec![
            json!({"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "}),
            json!({"type": "error", "error_type": "auth_error", "description": "Authentication failed"}),
            json!({"type": "success"}),
        ]);
        let mut g = Greetd::new(c);
        assert_eq!(g.login("ann", "bad", &[], &[]), Err(Fail::Auth("Authentication failed".into())));
        let got = h.join().unwrap();
        assert_eq!(got.last().unwrap()["type"], "cancel_session");
    }

    #[test]
    fn info_messages_are_acknowledged_without_the_password() {
        let (c, h) = server(vec![
            json!({"type": "auth_message", "auth_message_type": "info", "auth_message": "hello"}),
            json!({"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "}),
            json!({"type": "success"}),
            json!({"type": "success"}),
        ]);
        let mut g = Greetd::new(c);
        assert_eq!(g.login("ann", "pw", &[], &[]), Ok(()));
        let got = h.join().unwrap();
        assert_eq!(got[1]["response"], Value::Null);
        assert_eq!(got[2]["response"], "pw");
    }

    #[test]
    fn a_second_question_is_refused() {
        let (c, _h) = server(vec![
            json!({"type": "auth_message", "auth_message_type": "secret", "auth_message": "Password: "}),
            json!({"type": "auth_message", "auth_message_type": "secret", "auth_message": "New password: "}),
            json!({"type": "success"}),
        ]);
        let mut g = Greetd::new(c);
        assert!(matches!(g.login("ann", "pw", &[], &[]), Err(Fail::Auth(_))));
    }

    #[test]
    fn a_dead_connection_is_not_an_auth_failure() {
        let (a, b) = UnixStream::pair().unwrap();
        drop(b);
        let mut g = Greetd::new(a);
        assert!(matches!(g.login("ann", "pw", &[], &[]), Err(Fail::Other(_))));
    }
}
