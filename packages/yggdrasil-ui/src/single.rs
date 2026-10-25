//! One window per app, opened and closed by the same command: a bar button or a shortcut runs the app, and running it again while
//! its window is open closes that window instead of opening a second one.

use std::path::PathBuf;

fn pidfile(app_id: &str) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    dir.join(format!("{app_id}.pid"))
}

/// The process `pid` runs the same program as this one.
fn same_program(pid: u32) -> bool {
    let (Ok(theirs), Ok(ours)) = (std::fs::read_link(format!("/proc/{pid}/exe")), std::env::current_exe()) else { return false };
    // an upgraded binary shows as "PATH (deleted)"
    theirs == ours || theirs.to_string_lossy().trim_end_matches(" (deleted)") == ours.to_string_lossy()
}

/// Call first thing in `main`. Closes an open window of `app_id` and returns true (then exit); otherwise records this process as
/// the open one and returns false.
pub fn toggle(app_id: &str) -> bool {
    let f = pidfile(app_id);
    if let Some(pid) = std::fs::read_to_string(&f).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
        if pid != std::process::id() && same_program(pid) {
            let closed = std::process::Command::new("kill").arg(pid.to_string()).status().map(|s| s.success()).unwrap_or(false);
            if closed {
                let _ = std::fs::remove_file(&f);
                return true;
            }
        }
    }
    let _ = std::fs::write(&f, std::process::id().to_string());
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_or_foreign_pid_is_replaced() {
        let id = format!("ygg-single-test-{}", std::process::id());
        // pid 1 runs another program: not ours, so this run opens
        std::fs::write(pidfile(&id), "1").unwrap();
        assert!(!toggle(&id));
        assert_eq!(std::fs::read_to_string(pidfile(&id)).unwrap(), std::process::id().to_string());
        // our own pid is never closed
        assert!(!toggle(&id));
        std::fs::remove_file(pidfile(&id)).ok();
    }
}
