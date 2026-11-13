//! yggdrasil-hypr-logkeep: keeps a copy of this Hyprland session's log in ~/.local/state/yggdrasil/hyprland-logs/, so a crash
//! leaves its log behind. Hyprland writes the log into its instance folder under $XDG_RUNTIME_DIR/hypr/ and deletes a finished
//! instance's folder when the next one starts, so after a crash the crashed session's log is gone (2026-11-03). Run at the
//! session's start; it follows the log until that Hyprland exits. The newest five copies are kept, older ones compressed.
//!
//! As the script: older copies compressed with `zstd -q --rm` (one call each, in name order), then only the four newest
//! compressed ones kept (by modification time, newest first; the copy this run writes is the fifth); up to five seconds' wait
//! for Hyprland's log and lock file; then this process becomes `tail -n +1 --pid=PID -F LOG` writing into
//! DATE-hyprland.log. The script's find | sort | tail | cut | xargs rm is done here without them.

use crate::run;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const ME: &str = "yggdrasil-hypr-logkeep";

pub fn main() {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").unwrap_or_default();
    if sig.is_empty() || !sig.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
        eprintln!("{ME}: no Hyprland instance");
        std::process::exit(0);
    }
    let inst = run::runtime_dir().join("hypr").join(&sig);
    let dir = run::xdg("XDG_STATE_HOME", ".local/state").join("yggdrasil/hyprland-logs");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("mkdir: cannot create directory '{}': {}", dir.display(), e);
    }

    // older copies: compressed, and only the newest five sessions kept
    for f in logs(&dir) {
        let _ = Command::new("zstd").args(["-q", "--rm", "--"]).arg(&f).stderr(Stdio::null()).status();
    }
    for f in to_remove(&dir) {
        let _ = std::fs::remove_file(f);
    }

    let (log, lock) = (inst.join("hyprland.log"), inst.join("hyprland.lock"));
    for _ in 0..50 {
        if log.is_file() && std::fs::metadata(&lock).is_ok_and(|m| m.len() > 0) {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let pid = std::fs::read(&lock).ok().and_then(|b| {
        let first = b.split(|&c| c == b'\n').next().unwrap_or_default().to_vec();
        (!first.is_empty() && first.iter().all(u8::is_ascii_digit)).then(|| String::from_utf8(first).unwrap_or_default())
    });
    let Some(pid) = pid else {
        eprintln!("{ME}: no Hyprland process in {}", inst.display());
        std::process::exit(0);
    };
    let out = dir.join(format!("{}-hyprland.log", run::local_time("%Y-%m-%d-%H%M")));
    let file = match std::fs::File::create(&out) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("{ME}: {}: {}", out.display(), e);
            std::process::exit(1);
        }
    };
    let mut c = Command::new("tail");
    c.args(["-n", "+1"]).arg(format!("--pid={pid}")).arg("-F").arg(&log).stdout(file).stderr(Stdio::null());
    run::exec(ME, c)
}

/// "$dir"/*.log that are files (links to files too, as [[ -f ]]), in name order; names starting with a dot are not matched.
fn logs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| {
                    let n = e.file_name();
                    let n = n.as_encoded_bytes();
                    !n.starts_with(b".") && n.ends_with(b".log")
                })
                .map(|e| e.path())
                .filter(|p| p.is_file())
                .collect()
        })
        .unwrap_or_default();
    v.sort();
    v
}

/// What `find DIR -maxdepth 1 -name '*.log.zst' -printf '%T@ %p\n' | sort -rn | tail -n +5` named: every entry ending in
/// .log.zst (dot files and folders too; links by their own time) but the four newest. Equal times are ordered as sort -rn
/// orders them, by the whole line backwards.
pub fn to_remove(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<(i64, i64, PathBuf)> = std::fs::read_dir(dir)
        .map(|d| {
            d.flatten()
                .filter(|e| e.file_name().as_encoded_bytes().ends_with(b".log.zst"))
                .filter_map(|e| {
                    let m = std::fs::symlink_metadata(e.path()).ok()?;
                    Some((m.mtime(), m.mtime_nsec(), e.path()))
                })
                .collect()
        })
        .unwrap_or_default();
    v.sort_by(|a, b| b.cmp(a));
    v.into_iter().skip(4).map(|(_, _, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_four_newest_compressed_copies_stay() {
        let d = std::env::temp_dir().join(format!("logkeep-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("dir.log.zst")).unwrap();
        for (i, n) in ["a.log.zst", "b.log.zst", ".c.log.zst", "d.log.zst", "e.log.zst", "f.log", "g.log.zst.bak"].iter().enumerate() {
            let p = d.join(n);
            std::fs::write(&p, b"x").unwrap();
            let t = libc::timespec { tv_sec: 1_700_000_000 + i as i64 * 10, tv_nsec: 0 };
            let c = std::ffi::CString::new(p.as_os_str().as_encoded_bytes()).unwrap();
            // SAFETY: a path and two timespecs that live across the call
            unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [t, t].as_ptr(), 0) };
        }
        let t = libc::timespec { tv_sec: 1_600_000_000, tv_nsec: 0 };
        let c = std::ffi::CString::new(d.join("dir.log.zst").as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: as above
        unsafe { libc::utimensat(libc::AT_FDCWD, c.as_ptr(), [t, t].as_ptr(), 0) };
        let mut gone: Vec<String> = to_remove(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
        gone.sort();
        assert_eq!(gone, ["a.log.zst", "dir.log.zst"]);
        assert_eq!(logs(&d), [d.join("f.log")]);
        let _ = std::fs::remove_dir_all(&d);
    }
}
