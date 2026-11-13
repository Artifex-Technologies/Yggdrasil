//! One Settings at a time. Two windows each kept their own copy of the settings and wrote over each other's changes, so a
//! second `yggdrasil-settings [PAGE]` hands its page to the one already open (over a socket in the runtime folder) and ends;
//! the open one turns to that page and comes to the front.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::time::Duration;

fn socket_path() -> Option<PathBuf> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    Some(dir.join("yggdrasil-settings.sock"))
}

/// The page asked for, as one line (the first argument, cut short and on one line).
pub fn request_line(arg: &str) -> String {
    let one: String = arg.chars().filter(|c| !c.is_control()).take(80).collect();
    format!("{one}\n")
}

/// Hands `arg` to the Settings already open; true when one took it (this one should then end).
pub fn hand_over(arg: &str) -> bool {
    let Some(path) = socket_path() else { return false };
    let Ok(mut s) = UnixStream::connect(&path) else { return false };
    let _ = s.set_write_timeout(Some(Duration::from_secs(2)));
    s.write_all(request_line(arg).as_bytes()).is_ok()
}

/// This is the Settings that is open: listens for the pages later ones hand over, and gives each to `on_request` (run on a
/// thread of its own). A socket left by a Settings that crashed is taken over (nothing answered on it).
pub fn listen(on_request: impl Fn(String) + Send + 'static) {
    let Some(path) = socket_path() else { return };
    let _ = std::fs::remove_file(&path);
    let Ok(listener) = UnixListener::bind(&path) else { return };
    std::thread::spawn(move || {
        for conn in listener.incoming().flatten() {
            let _ = conn.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            if BufReader::new(conn).read_line(&mut line).is_ok() {
                on_request(line.trim().to_string());
            }
        }
    });
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_request_is_one_short_line() {
        assert_eq!(super::request_line("bluetooth"), "bluetooth\n");
        assert_eq!(super::request_line("wifi\nrm -rf"), "wifirm -rf\n");
        assert_eq!(super::request_line(&"x".repeat(200)).len(), 81);
    }
}
