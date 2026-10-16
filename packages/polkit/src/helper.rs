//! Talking to polkit's authentication helper: the small root program that asks PAM to check the password and then tells polkitd the
//! result. The agent never sees the PAM modules and never claims success itself; it only passes the password to the helper.
//!
//! The helper speaks a line protocol (the same on its socket and on a pipe):
//!   agent -> helper:  USER\n  COOKIE\n          (then, for each prompt, the answer + \n)
//!   helper -> agent:  PAM_PROMPT_ECHO_OFF Password: \n | PAM_PROMPT_ECHO_ON ...\n | PAM_TEXT_INFO ...\n | PAM_ERROR_MSG ...\n | SUCCESS\n | FAILURE\n
//! polkit 126 and later run the helper on /run/polkit/agent-helper.socket (a systemd socket unit); older ones use a setuid
//! program, which is used as a fallback.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

use zeroize::Zeroize;

pub const SOCKET: &str = "/run/polkit/agent-helper.socket";
pub const HELPER: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";

#[derive(Debug, PartialEq, Eq)]
pub enum Line {
    Prompt { echo: bool, text: String },
    Info(String),
    Error(String),
    Success,
    Failure,
    Other(String),
}

pub fn parse_line(l: &str) -> Line {
    let l = l.trim_end_matches(['\n', '\r']);
    if l == "SUCCESS" {
        return Line::Success;
    }
    if l == "FAILURE" {
        return Line::Failure;
    }
    let (word, rest) = l.split_once(' ').unwrap_or((l, ""));
    match word {
        "PAM_PROMPT_ECHO_OFF" => Line::Prompt { echo: false, text: rest.trim().to_string() },
        "PAM_PROMPT_ECHO_ON" => Line::Prompt { echo: true, text: rest.trim().to_string() },
        "PAM_TEXT_INFO" => Line::Info(rest.to_string()),
        "PAM_ERROR_MSG" => Line::Error(rest.to_string()),
        _ => Line::Other(l.to_string()),
    }
}

/// What the dialog has to do for the protocol.
pub trait Ui {
    /// Ask for an answer; None means the user cancelled.
    fn prompt(&mut self, text: &str, echo: bool) -> Option<String>;
    fn message(&mut self, text: &str, error: bool);
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Authenticated,
    Failed,
    Cancelled,
}

/// Runs one authentication over `rd`/`wr` for `user` and the request's `cookie`.
pub fn run_session<R: std::io::Read, W: Write>(rd: R, wr: W, user: &str, cookie: &str, ui: &mut dyn Ui) -> std::io::Result<Outcome> {
    session(rd, wr, Some(user), cookie, ui)
}

/// `user` is None for the setuid helper, which takes the user as its argument and then reads only the cookie.
fn session<R: std::io::Read, W: Write>(rd: R, mut wr: W, user: Option<&str>, cookie: &str, ui: &mut dyn Ui) -> std::io::Result<Outcome> {
    if let Some(user) = user {
        writeln!(wr, "{user}")?;
    }
    writeln!(wr, "{cookie}")?;
    wr.flush()?;
    let mut rd = BufReader::new(rd);
    let mut line = String::new();
    loop {
        line.clear();
        if rd.read_line(&mut line)? == 0 {
            return Ok(Outcome::Failed); // the helper went away without a verdict
        }
        match parse_line(&line) {
            Line::Success => return Ok(Outcome::Authenticated),
            Line::Failure => return Ok(Outcome::Failed),
            Line::Info(t) => ui.message(&t, false),
            Line::Error(t) => ui.message(&t, true),
            Line::Other(_) => {}
            Line::Prompt { echo, text } => match ui.prompt(&text, echo) {
                None => return Ok(Outcome::Cancelled),
                Some(mut secret) => {
                    let r = writeln!(wr, "{secret}").and_then(|_| wr.flush());
                    secret.zeroize();
                    r?;
                }
            },
        }
    }
}

/// Authenticates with the real helper: its socket if there is one, else the setuid program.
pub fn authenticate(user: &str, cookie: &str, ui: &mut dyn Ui) -> std::io::Result<Outcome> {
    if std::path::Path::new(SOCKET).exists() {
        let s = UnixStream::connect(SOCKET)?;
        return run_session(s.try_clone()?, s, user, cookie, ui);
    }
    let mut child = Command::new(HELPER).arg(user).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
    let out = child.stdout.take().ok_or_else(|| std::io::Error::other("no helper output"))?;
    let inp = child.stdin.take().ok_or_else(|| std::io::Error::other("no helper input"))?;
    let r = session(out, inp, None, cookie, ui);
    let _ = child.wait();
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    struct Scripted {
        answers: Vec<Option<String>>,
        seen: Vec<String>,
    }
    impl Ui for Scripted {
        fn prompt(&mut self, text: &str, _echo: bool) -> Option<String> {
            self.seen.push(format!("prompt:{text}"));
            if self.answers.is_empty() {
                None
            } else {
                self.answers.remove(0)
            }
        }
        fn message(&mut self, text: &str, error: bool) {
            self.seen.push(format!("{}:{text}", if error { "error" } else { "info" }));
        }
    }

    #[test]
    fn lines_are_parsed() {
        assert_eq!(parse_line("PAM_PROMPT_ECHO_OFF Password: \n"), Line::Prompt { echo: false, text: "Password:".into() });
        assert_eq!(parse_line("PAM_PROMPT_ECHO_ON Name: "), Line::Prompt { echo: true, text: "Name:".into() });
        assert_eq!(parse_line("PAM_TEXT_INFO Touch the key"), Line::Info("Touch the key".into()));
        assert_eq!(parse_line("PAM_ERROR_MSG Wrong\n"), Line::Error("Wrong".into()));
        assert_eq!(parse_line("SUCCESS\n"), Line::Success);
        assert_eq!(parse_line("FAILURE"), Line::Failure);
        assert_eq!(parse_line("something else"), Line::Other("something else".into()));
    }

    /// A stand-in for the helper: reads user and cookie, asks for a password, says SUCCESS only for "right".
    fn fake_helper(mut stream: UnixStream) {
        let mut rd = BufReader::new(stream.try_clone().unwrap());
        let mut user = String::new();
        let mut cookie = String::new();
        rd.read_line(&mut user).unwrap();
        rd.read_line(&mut cookie).unwrap();
        assert_eq!(user.trim(), "alice");
        assert_eq!(cookie.trim(), "cookie-1");
        writeln!(stream, "PAM_TEXT_INFO hello").unwrap();
        writeln!(stream, "PAM_PROMPT_ECHO_OFF Password: ").unwrap();
        let mut pw = String::new();
        rd.read_line(&mut pw).unwrap();
        writeln!(stream, "{}", if pw.trim() == "right" { "SUCCESS" } else { "FAILURE" }).unwrap();
    }

    fn session(answers: Vec<Option<String>>) -> (Outcome, Vec<String>) {
        let (a, b) = UnixStream::pair().unwrap();
        let h = std::thread::spawn(move || fake_helper(b));
        let mut ui = Scripted { answers, seen: vec![] };
        let out = run_session(a.try_clone().unwrap(), a, "alice", "cookie-1", &mut ui).unwrap();
        let _ = h.join();
        (out, ui.seen)
    }

    #[test]
    fn right_password_authenticates() {
        let (o, seen) = session(vec![Some("right".into())]);
        assert_eq!(o, Outcome::Authenticated);
        assert_eq!(seen, vec!["info:hello", "prompt:Password:"]);
    }

    #[test]
    fn wrong_password_fails() {
        assert_eq!(session(vec![Some("wrong".into())]).0, Outcome::Failed);
    }

    #[test]
    fn cancelling_the_prompt_is_not_a_failure() {
        let (a, b) = UnixStream::pair().unwrap();
        let h = std::thread::spawn(move || {
            let mut rd = BufReader::new(b.try_clone().unwrap());
            let mut s = String::new();
            rd.read_line(&mut s).unwrap();
            rd.read_line(&mut s).unwrap();
            let mut b = b;
            writeln!(b, "PAM_PROMPT_ECHO_OFF Password: ").unwrap();
            let mut rest = String::new();
            let _ = BufReader::new(b).read_to_string(&mut rest); // ends when the agent closes
        });
        let mut ui = Scripted { answers: vec![None], seen: vec![] };
        let o = run_session(a.try_clone().unwrap(), &a, "alice", "cookie-1", &mut ui).unwrap();
        assert_eq!(o, Outcome::Cancelled);
        drop(a);
        let _ = h.join();
    }
}
