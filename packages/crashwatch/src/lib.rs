//! Crash watch: when one of your own programs crashes and systemd-coredump stores its core, a notice says which program and
//! which signal; when the system has an assistant that answers typed questions (an `assistant` integration file with a
//! `question` command, docs/INTEGRATIONS.md), the notice offers to ask it. See README.md.
//!
//! The watcher (`yggdrasil-crashwatch`) follows the core folder with inotify and learns everything it needs from the new file's
//! name (whose crash) and its extended attributes (which program, which signal), which systemd-coredump sets and which the
//! crashing user may read through the file's ACL. It never opens a core. Each notice runs in its own short-lived process
//! (`crashwatch-notice`), so the watcher never holds a D-Bus connection.

pub mod core;
pub mod notice;
pub mod quiet;

/// Seconds since boot, suspend included (CLOCK_BOOTTIME): ten minutes of quiet still pass while the computer sleeps.
pub fn boot_secs() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime fills the struct it is given
    unsafe { libc::clock_gettime(libc::CLOCK_BOOTTIME, &mut ts) };
    ts.tv_sec.max(0) as u64
}
