//! SIGTERM, SIGINT and SIGHUP end Settings cleanly, so a shutdown or a log out no longer waits for it ("A stop job is
//! running"). The window is asked to close as its own close button does (eframe ends its loop, `on_exit` tidies up); when it
//! cannot draw any more (its compositor gone), the process ends by itself a moment later.
//!
//! A handler of its own rather than a blocked mask and a signalfd: the programs Settings starts get the default handling back
//! when they start, and a mask its parent left blocked is cleared first. On 2026-11-15 Yggdrasil's compositor, which blocks
//! these signals for its own signalfd, passed them on blocked to Settings, which then never saw them.

use std::io::Read;
use std::os::fd::FromRawFd;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::Duration;

use eframe::egui;

const SIGNALS: [libc::c_int; 3] = [libc::SIGTERM, libc::SIGINT, libc::SIGHUP];

/// The pipe's writing end, for the handler (a byte per signal: its number).
static WRITE_FD: AtomicI32 = AtomicI32::new(-1);

extern "C" fn on_signal(sig: libc::c_int) {
    let fd = WRITE_FD.load(Ordering::Relaxed);
    if fd >= 0 {
        let b = sig as u8;
        // SAFETY: write is async-signal-safe; the writing end never blocks (O_NONBLOCK)
        unsafe { libc::write(fd, (&b as *const u8).cast(), 1) };
    }
}

/// Take the three signals (one already ignored, as SIGINT for a shell's background job or SIGHUP under nohup, stays ignored)
/// and unblock them. Call it first in `main`, before any thread starts, so every thread has them unblocked. The reading end
/// gives a byte per signal.
pub fn install() -> std::io::Result<std::fs::File> {
    let mut fds = [0; 2];
    // SAFETY: plain system calls on memory of our own; the descriptors are owned from here on
    unsafe {
        if libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC) != 0 {
            return Err(std::io::Error::last_os_error());
        }
        let flags = libc::fcntl(fds[1], libc::F_GETFL);
        libc::fcntl(fds[1], libc::F_SETFL, flags | libc::O_NONBLOCK);
        WRITE_FD.store(fds[1], Ordering::Relaxed);
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        for sig in SIGNALS {
            let mut old: libc::sigaction = std::mem::zeroed();
            libc::sigaction(sig, std::ptr::null(), &mut old);
            if old.sa_sigaction == libc::SIG_IGN {
                continue;
            }
            let mut act: libc::sigaction = std::mem::zeroed();
            act.sa_sigaction = on_signal as extern "C" fn(libc::c_int) as libc::sighandler_t;
            act.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut act.sa_mask);
            if libc::sigaction(sig, &act, std::ptr::null_mut()) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            libc::sigaddset(&mut set, sig);
        }
        libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        Ok(std::fs::File::from_raw_fd(fds[0]))
    }
}

/// On the first signal: `close` (ask the window to close), then, if the process is still there after `grace`, `last` (what
/// must be done before the end) and the end, with the shell's code for that signal (128 + its number).
pub fn watch(mut signals: std::fs::File, close: impl FnOnce(i32) + Send + 'static, grace: Duration, last: impl FnOnce() + Send + 'static) {
    std::thread::spawn(move || {
        let mut b = [0u8; 1];
        if signals.read_exact(&mut b).is_err() {
            return;
        }
        let sig = i32::from(b[0]);
        close(sig);
        std::thread::sleep(grace);
        last();
        std::process::exit(128 + sig);
    });
}

/// Settings' window (or the pop-out's), given the reading end `install` gave: closed on SIGTERM, SIGINT or SIGHUP; `last` runs
/// before an end that has to be forced.
pub fn close_on_signals(ctx: &egui::Context, signals: std::fs::File, last: impl FnOnce() + Send + 'static) {
    let ctx = ctx.clone();
    watch(
        signals,
        move |_| {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            ctx.request_repaint();
        },
        Duration::from_secs(3),
        last,
    );
}

#[cfg(test)]
mod tests {
    use std::os::unix::process::CommandExt;
    use std::process::{Command, Stdio};

    /// Run in a process of its own by the test below (it changes how the whole process takes signals).
    #[test]
    fn child() {
        if std::env::var_os("YGG_SETTINGS_QUIT_CHILD").is_none() {
            return;
        }
        let r = super::install().expect("installed");
        super::watch(r, |sig| println!("asked to close on {sig}"), std::time::Duration::from_millis(300), || println!("last"));
        println!("ready");
        std::thread::sleep(std::time::Duration::from_secs(30));
        println!("not ended");
    }

    fn run(sig: libc::c_int, blocked: bool) -> (Option<i32>, String) {
        let mut c = Command::new(std::env::current_exe().unwrap());
        c.args(["--exact", "quit::tests::child", "--nocapture", "--test-threads=1"])
            .env("YGG_SETTINGS_QUIT_CHILD", "1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        // SAFETY: only async-signal-safe calls between fork and exec
        unsafe {
            c.pre_exec(move || {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                for s in super::SIGNALS {
                    // the default handling, whatever the test runner was started with
                    libc::signal(s, libc::SIG_DFL);
                    libc::sigaddset(&mut set, s);
                }
                // as a parent that blocks them for a signalfd of its own and does not clear the mask for its children
                if blocked {
                    libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
                }
                Ok(())
            });
        }
        let mut child = c.spawn().unwrap();
        let mut out = child.stdout.take().unwrap();
        let mut text = String::new();
        let mut b = [0u8; 256];
        while !text.contains("ready") {
            let n = std::io::Read::read(&mut out, &mut b).unwrap();
            assert!(n > 0, "the child ended before it was ready: {text}");
            text.push_str(&String::from_utf8_lossy(&b[..n]));
        }
        // SAFETY: a signal to our own child
        unsafe { libc::kill(child.id() as i32, sig) };
        let st = child.wait().unwrap();
        let mut rest = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut rest);
        (st.code(), text + &rest)
    }

    #[test]
    fn the_end_signals_close_the_window_then_end_it() {
        for sig in super::SIGNALS {
            for blocked in [false, true] {
                let (code, out) = run(sig, blocked);
                assert!(out.contains(&format!("asked to close on {sig}")), "signal {sig}, blocked {blocked}: {out}");
                assert!(out.contains("last") && !out.contains("not ended"), "signal {sig}, blocked {blocked}: {out}");
                assert_eq!(code, Some(128 + sig), "signal {sig}, blocked {blocked}: {out}");
            }
        }
    }
}
