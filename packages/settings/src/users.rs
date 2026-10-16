//! The Users page's system side: who the account is, and changing its password through the system's own `passwd` (so PAM sees the old and the new
//! password, which is how gnome-keyring's login keyring follows the login password; AccountsService's SetPassword has no old password and would leave
//! the keyring behind).

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    pub full_name: String,
    pub uid: u32,
    pub admin: bool,
    pub home: String,
}

/// One line of /etc/passwd ("login:x:1000:1000:Full Name,room,phone:/home/login:/bin/bash").
pub fn parse_passwd(line: &str) -> Option<Account> {
    let f: Vec<&str> = line.trim().split(':').collect();
    if f.len() < 6 {
        return None;
    }
    Some(Account { login: f[0].to_string(), full_name: f[4].split(',').next().unwrap_or("").to_string(), uid: f[2].parse().ok()?, admin: false, home: f[5].to_string() })
}

/// The signed-in user's account.
pub fn account() -> Account {
    let login = crate::sys::run("id", &["-un"]).map(|s| s.trim().to_string()).unwrap_or_default();
    let mut a = crate::sys::run("getent", &["passwd", &login]).and_then(|l| parse_passwd(&l)).unwrap_or(Account { login: login.clone(), ..Default::default() });
    let groups = crate::sys::run("id", &["-Gn"]).unwrap_or_default();
    a.admin = groups.split_whitespace().any(|g| g == "wheel" || g == "sudo");
    a
}

/// The state of gnome-keyring's login keyring as `labyrinth-keyring-sync --check` reports it ("locked", "unlocked", "noservice", "nokeyring"), or "" without the tool.
pub fn keyring_state() -> String {
    crate::sys::run("labyrinth-keyring-sync", &["--check"]).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// What to tell the user about a failed change, from what `passwd` printed.
pub fn explain_failure(output: &str) -> String {
    let lower = output.to_lowercase();
    if let Some(i) = lower.find("bad password:") {
        let reason = output[i + "bad password:".len()..].lines().next().unwrap_or("").trim().trim_end_matches('.');
        return format!("The new password is too weak: {}.", reason.to_lowercase());
    }
    if lower.contains("do not match") {
        return "The two new passwords do not match.".into();
    }
    if lower.contains("authentication token manipulation error") || lower.contains("authentication failure") || lower.contains("incorrect password") {
        return "The current password is not right.".into();
    }
    if lower.contains("locked") {
        return "Too many wrong tries; the account is locked for a few minutes.".into();
    }
    "The password was not changed.".into()
}

/// Runs `cmdline` (normally `passwd`) in a terminal of its own and answers its three questions (current, new, new again).
pub fn change_password_with(cmdline: &str, current: &str, new: &str) -> Result<(), String> {
    let mut child = Command::new("script").args(["-qefc", cmdline, "/dev/null"]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().map_err(|e| format!("cannot run passwd: {e}"))?;
    let buf = Arc::new(Mutex::new(String::new()));
    let reader = {
        let (buf, mut out) = (buf.clone(), child.stdout.take().ok_or("no output")?);
        std::thread::spawn(move || {
            let mut chunk = [0u8; 512];
            while let Ok(n) = out.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                buf.lock().unwrap().push_str(&String::from_utf8_lossy(&chunk[..n]).replace('\r', ""));
            }
        })
    };
    let mut stdin = child.stdin.take().ok_or("no input")?;
    let (mut stage, mut consumed) = (0, 0usize);
    let deadline = Instant::now() + Duration::from_secs(40);
    let result = loop {
        if Instant::now() > deadline {
            let _ = child.kill();
            break Err("passwd did not answer".to_string());
        }
        if let Ok(Some(status)) = child.try_wait() {
            let _ = reader.join();
            let text = buf.lock().unwrap().clone();
            if std::env::var_os("LABYRINTH_PW_DEBUG").is_some() { eprintln!("passwd exit {:?}, output: {:?}", status, text); }
            break if status.success() { Ok(()) } else { Err(explain_failure(&text)) };
        }
        let text = buf.lock().unwrap().clone();
        let fresh = text[consumed.min(text.len())..].to_lowercase();
        if fresh.contains("bad password:") || fresh.contains("do not match") {
            let _ = child.kill();
            break Err(explain_failure(&text));
        }
        let send = |s: &str, stdin: &mut std::process::ChildStdin| stdin.write_all(format!("{s}\n").as_bytes()).and_then(|_| stdin.flush());
        let mut sent = false;
        if stage == 0 && fresh.contains("current password") {
            sent = send(current, &mut stdin).is_ok();
        } else if stage == 1 && fresh.contains("new password:") && !fresh.contains("retype") {
            sent = send(new, &mut stdin).is_ok();
        } else if stage == 2 && fresh.contains("retype new password") {
            sent = send(new, &mut stdin).is_ok();
        }
        if sent {
            stage += 1;
            consumed = text.len();
        }
        std::thread::sleep(Duration::from_millis(60));
    };
    buf.lock().unwrap().clear();
    result
}

/// Changes the signed-in user's password with the system's `passwd`.
pub fn change_password(current: &str, new: &str) -> Result<(), String> {
    change_password_with("passwd", current, new)
}

/// A rough read of how good a new password is, for the form (length only: the system's own rules decide in the end).
pub fn strength(pw: &str) -> (&'static str, f32) {
    let n = pw.chars().count();
    let classes = [pw.chars().any(|c| c.is_lowercase()), pw.chars().any(|c| c.is_uppercase()), pw.chars().any(|c| c.is_ascii_digit()), pw.chars().any(|c| !c.is_alphanumeric())].iter().filter(|b| **b).count();
    match (n, classes) {
        (0, _) => ("", 0.0),
        (1..=7, _) => ("Too short", 0.15),
        (8..=11, 1..=2) => ("Weak", 0.35),
        (8..=11, _) => ("Fair", 0.6),
        (12..=15, 1) => ("Fair", 0.6),
        (12..=15, _) => ("Good", 0.8),
        _ => ("Strong", 1.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn passwd_line_is_read() {
        let a = parse_passwd("zlothra:x:1000:1000:Z Lothra,,,:/home/zlothra:/bin/bash").unwrap();
        assert_eq!((a.login.as_str(), a.full_name.as_str(), a.uid, a.home.as_str()), ("zlothra", "Z Lothra", 1000, "/home/zlothra"));
        assert!(parse_passwd("broken").is_none());
    }

    #[test]
    fn failures_are_explained_in_plain_words() {
        assert!(explain_failure("BAD PASSWORD: The password is shorter than 8 characters\n").contains("too weak: the password is shorter than 8 characters"));
        assert_eq!(explain_failure("passwd: Authentication token manipulation error"), "The current password is not right.");
        assert_eq!(explain_failure("Sorry, passwords do not match."), "The two new passwords do not match.");
        assert_eq!(explain_failure("something odd"), "The password was not changed.");
    }

    #[test]
    fn strength_reads_length_and_variety() {
        assert_eq!(strength("").0, "");
        assert_eq!(strength("abc").0, "Too short");
        assert_eq!(strength("abcdefgh").0, "Weak");
        assert_eq!(strength("Abcdefg1").0, "Fair");
        assert_eq!(strength("correct horse battery staple").0, "Strong");
    }

    /// A stand-in for `passwd` that asks the same three questions and fails in the same ways.
    fn fake_passwd() -> std::path::PathBuf {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let p = std::env::temp_dir().join(format!("fake-passwd-{}-{}.sh", std::process::id(), N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)));
        std::fs::write(
            &p,
            r#"#!/bin/bash
echo "Changing password for tester."
read -r -s -p "Current password: " cur; echo
[ "$cur" = "cur-pass" ] || { echo "passwd: Authentication token manipulation error"; exit 10; }
read -r -s -p "New password: " new; echo
if [ ${#new} -lt 8 ]; then echo "BAD PASSWORD: The password is shorter than 8 characters"; read -r -s -p "New password: " x; echo; exit 10; fi
read -r -s -p "Retype new password: " again; echo
[ "$new" = "$again" ] || { echo "Sorry, passwords do not match."; exit 10; }
echo "passwd: password updated successfully"
"#,
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn the_right_answers_change_the_password() {
        let f = fake_passwd();
        assert_eq!(change_password_with(f.to_str().unwrap(), "cur-pass", "a-new-pass-1"), Ok(()));
    }

    #[test]
    fn a_wrong_current_password_is_reported() {
        let f = fake_passwd();
        assert_eq!(change_password_with(f.to_str().unwrap(), "nope", "a-new-pass-1"), Err("The current password is not right.".to_string()));
    }

    #[test]
    fn a_weak_new_password_is_reported_with_the_reason() {
        let f = fake_passwd();
        let e = change_password_with(f.to_str().unwrap(), "cur-pass", "short").unwrap_err();
        assert!(e.contains("too weak"), "{e}");
    }
}
