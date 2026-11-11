//! subprocess's `timeout=`: wait for a child at most so long, woken the moment it ends (no polling).

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, ExitStatus};
use std::time::{Duration, Instant};

/// Waits for `child` to end, at most `limit`: Some(status) when it ended, None when it is still running (it is left
/// running; kill it and wait if it must go). The wait sleeps on a pidfd, so it returns as soon as the child ends.
pub fn wait_timeout(child: &mut Child, limit: Duration) -> io::Result<Option<ExitStatus>> {
    if let Some(s) = child.try_wait()? {
        return Ok(Some(s));
    }
    // SAFETY: pidfd_open takes a pid and flags and returns a new descriptor or -1; the pid is our own child, not yet waited
    // for, so it cannot have been reused
    let raw = unsafe { libc::syscall(libc::SYS_pidfd_open, child.id() as libc::pid_t, 0 as libc::c_uint) };
    if raw < 0 {
        // a kernel older than 5.3, or a sandbox that refuses the call: Python's way, short sleeps until the time is up
        return poll_wait(child, limit);
    }
    // SAFETY: a descriptor pidfd_open just made, owned by nothing else
    let fd = unsafe { OwnedFd::from_raw_fd(raw as i32) };
    let deadline = Instant::now() + limit;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        // milliseconds, rounded up so the wait never ends early
        let ms = left.as_micros().div_ceil(1000).min(i32::MAX as u128) as i32;
        let mut pfd = libc::pollfd { fd: fd.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd
        let r = unsafe { libc::poll(&mut pfd, 1, ms) };
        if r > 0 {
            // readable: the child has ended, so this returns at once
            return child.wait().map(Some);
        }
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e);
        }
        if left.is_zero() {
            return child.try_wait();
        }
    }
}

/// The fallback: what Python's Popen.wait(timeout) does, sleeps of 0.5 ms doubling up to 50 ms.
fn poll_wait(child: &mut Child, limit: Duration) -> io::Result<Option<ExitStatus>> {
    let deadline = Instant::now() + limit;
    let mut nap = Duration::from_micros(500);
    loop {
        if let Some(s) = child.try_wait()? {
            return Ok(Some(s));
        }
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        std::thread::sleep(nap.min(left));
        nap = (nap * 2).min(Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    fn sh(script: &str) -> Child {
        Command::new("sh").args(["-c", script]).stdin(Stdio::null()).stdout(Stdio::null()).spawn().unwrap()
    }

    #[test]
    fn returns_when_the_child_ends_not_at_the_limit() {
        let mut c = sh("exit 3");
        let t = Instant::now();
        let s = wait_timeout(&mut c, Duration::from_secs(20)).unwrap().expect("it ended");
        assert_eq!(s.code(), Some(3));
        assert!(t.elapsed() < Duration::from_secs(5), "took {:?}", t.elapsed());
    }

    #[test]
    fn a_child_still_running_at_the_limit_is_left_running() {
        let mut c = sh("sleep 30");
        let t = Instant::now();
        assert!(wait_timeout(&mut c, Duration::from_millis(200)).unwrap().is_none());
        assert!(t.elapsed() >= Duration::from_millis(200));
        assert!(c.try_wait().unwrap().is_none(), "not killed by the wait");
        c.kill().unwrap();
        assert!(wait_timeout(&mut c, Duration::from_secs(20)).unwrap().is_some());
    }

    #[test]
    fn an_ended_child_and_a_zero_limit() {
        let mut c = sh("exit 0");
        // a zero limit still sees a child that has ended
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut got = None;
        while got.is_none() && Instant::now() < deadline {
            got = wait_timeout(&mut c, Duration::ZERO).unwrap();
        }
        assert_eq!(got.and_then(|s| s.code()), Some(0));
        // waiting again for a child already waited for gives its status again
        assert_eq!(wait_timeout(&mut c, Duration::from_secs(1)).unwrap().and_then(|s| s.code()), Some(0));
    }

    #[test]
    fn the_fallback_behaves_the_same() {
        let mut c = sh("exit 4");
        assert_eq!(poll_wait(&mut c, Duration::from_secs(20)).unwrap().and_then(|s| s.code()), Some(4));
        let mut c = sh("sleep 30");
        assert!(poll_wait(&mut c, Duration::from_millis(100)).unwrap().is_none());
        c.kill().unwrap();
        let _ = c.wait();
    }
}
