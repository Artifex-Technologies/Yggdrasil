//! Starting other programs as the scripts did, by argv and never through a shell: `command -v`, `exec`, a command's exit status
//! and its output.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// `command -v NAME` for a plain name: the first file called NAME in a PATH folder that this account may run.
pub fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| if d.as_os_str().is_empty() { PathBuf::from(".").join(name) } else { d.join(name) })
        .find(|p| runnable(p))
}

fn runnable(p: &Path) -> bool {
    let Ok(c) = CString::new(p.as_os_str().as_bytes()) else { return false };
    // SAFETY: c is a NUL-terminated path that lives across the call
    p.is_file() && unsafe { libc::access(c.as_ptr(), libc::X_OK) } == 0
}

/// `exec PROGRAM ARGS...`: this process becomes the program. When it cannot be started, the shell's exit codes: 127 when it is
/// not there, 126 otherwise, with one line on standard error.
pub fn exec(me: &str, mut c: Command) -> ! {
    let e = c.exec();
    let prog = c.get_program().to_string_lossy().into_owned();
    if e.kind() == std::io::ErrorKind::NotFound {
        eprintln!("{me}: {prog}: not found");
        std::process::exit(127)
    }
    eprintln!("{me}: {prog}: {e}");
    std::process::exit(126)
}

/// A command's exit status as the shell gives it: its code, 128 + the signal that ended it, 127 when it is not there (said on
/// standard error as the shell says it), 126 when it cannot be started.
pub fn status(me: &str, c: &mut Command) -> i32 {
    match c.status() {
        Ok(s) => code(s),
        Err(e) => {
            let prog = c.get_program().to_string_lossy().into_owned();
            if e.kind() == std::io::ErrorKind::NotFound {
                eprintln!("{me}: {prog}: not found");
                127
            } else {
                eprintln!("{me}: {prog}: {e}");
                126
            }
        }
    }
}

pub fn code(s: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    s.code().unwrap_or_else(|| 128 + s.signal().unwrap_or(0))
}

/// `$(COMMAND 2>/dev/null)`: what it printed, without the newlines at the end; nothing when it could not be started.
pub fn output(c: &mut Command) -> Vec<u8> {
    let mut out = c.stdin(Stdio::inherit()).stderr(Stdio::null()).output().map(|o| o.stdout).unwrap_or_default();
    while out.last() == Some(&b'\n') {
        out.pop();
    }
    out
}

/// The runtime folder as the scripts named it: ${XDG_RUNTIME_DIR:-/run/user/$UID}.
pub fn runtime_dir() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        // SAFETY: getuid cannot fail
        _ => PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })),
    }
}

/// ${NAME:-$HOME/REST}: an XDG folder or its default under the home folder.
pub fn xdg(name: &str, rest: &str) -> PathBuf {
    match std::env::var_os(name) {
        Some(d) if !d.is_empty() => PathBuf::from(d),
        _ => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
    }
}

/// `date +FORMAT` in local time (strftime), for the names of log files.
pub fn local_time(format: &str) -> String {
    let Ok(f) = CString::new(format) else { return String::new() };
    // SAFETY: time with a null pointer, localtime_r into a zeroed tm, strftime into a buffer of the size given
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&t, &mut tm).is_null() {
            return String::new();
        }
        let mut buf = [0u8; 128];
        let n = libc::strftime(buf.as_mut_ptr().cast(), buf.len(), f.as_ptr(), &tm);
        String::from_utf8_lossy(&buf[..n]).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_status_as_the_shell_gives_it() {
        assert_eq!(status("t", Command::new("true").stdout(Stdio::null())), 0);
        assert_eq!(status("t", &mut Command::new("false")), 1);
        assert_eq!(status("t", &mut Command::new("/nonexistent/program")), 127);
    }

    #[test]
    fn output_loses_only_the_newlines_at_the_end() {
        assert_eq!(output(Command::new("printf").arg("a\\n\\nb\\n\\n")), b"a\n\nb");
        assert!(output(&mut Command::new("/nonexistent/program")).is_empty());
    }

    #[test]
    fn local_time_is_formatted() {
        let s = local_time("%Y-%m-%d-%H%M");
        assert_eq!(s.len(), 15, "{s}");
        assert!(s.bytes().all(|b| b.is_ascii_digit() || b == b'-'), "{s}");
    }
}
