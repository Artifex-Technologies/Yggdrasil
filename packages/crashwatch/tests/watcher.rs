//! The watcher as it runs: a folder standing in for /var/lib/systemd/coredump, core files with systemd-coredump's attributes
//! appearing in it the two ways systemd stores them (linked in place, or renamed from a temporary name), and a stand-in notice
//! program that writes down what it was given. Nothing is shown and nothing crashes.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BOOT: &str = "86b6f1565fe940188f8f77e469c8a952";

fn uid() -> u32 {
    // SAFETY: getuid cannot fail
    unsafe { libc::getuid() }
}

fn tmp(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("crashwatch-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn set_xattr(f: &Path, k: &str, v: &str) -> bool {
    let p = CString::new(f.as_os_str().as_bytes()).unwrap();
    let n = CString::new(k).unwrap();
    // SAFETY: plain buffers of the given lengths
    unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
}

/// A core the way systemd-coredump leaves it where it cannot link an unnamed file: written under a temporary name in the same
/// folder with its attributes set, then renamed into place. False when this file system keeps no user.* attributes.
fn store_core(dir: &Path, comm: &str, owner: u32, pid: u32, exe: &str, signal: &str) -> bool {
    let name = format!("core.{comm}.{owner}.{BOOT}.{pid}.1794175822000000.zst");
    let temp = dir.join(format!(".#{name}"));
    std::fs::write(&temp, b"not a core").unwrap();
    let ok = set_xattr(&temp, "user.coredump.exe", exe) && set_xattr(&temp, "user.coredump.signal", signal) && set_xattr(&temp, "user.coredump.uid", &owner.to_string());
    std::fs::rename(&temp, dir.join(name)).unwrap();
    ok
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn start(dir: &Path, notice: &Path) -> Running {
    let c = Command::new(env!("CARGO_BIN_EXE_yggdrasil-crashwatch"))
        .args(["--dir", dir.to_str().unwrap()])
        .env("CRASHWATCH_NOTICE", notice)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    // its watch is set up before the first read; a moment is plenty
    std::thread::sleep(Duration::from_millis(300));
    Running(c)
}

fn lines(log: &Path, at_least: usize) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let v: Vec<String> = std::fs::read_to_string(log).unwrap_or_default().lines().map(str::to_string).collect();
        if v.len() >= at_least || Instant::now() > deadline {
            return v;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A notice program that only writes down its arguments.
fn fake_notice(d: &Path) -> (PathBuf, PathBuf) {
    let log = d.join("notices.log");
    let prog = d.join("notice");
    std::fs::write(&prog, format!("#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> '{}'\n", log.display())).unwrap();
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    (prog, log)
}

#[test]
fn own_crashes_once_per_program() {
    let d = tmp("watch");
    let cores = d.join("coredump");
    std::fs::create_dir_all(&cores).unwrap();
    let (prog, log) = fake_notice(&d);
    let _w = start(&cores, &prog);
    let me = uid();
    if !store_core(&cores, "game.x86_64", me, 101, "/home/me/Games/game.x86_64", "11") {
        eprintln!("no user.* attributes in {}: skipped", d.display());
        return;
    }
    assert_eq!(lines(&log, 1), vec!["/home/me/Games/game.x86_64|SIGSEGV".to_string()]);

    // another account's crash (its uid in the name): nothing
    store_core(&cores, "game.x86_64", me + 1, 102, "/usr/bin/theirs", "6");
    // the same program again (a crash loop): quiet for ten minutes
    store_core(&cores, "game.x86_64", me, 103, "/home/me/Games/game.x86_64", "6");
    // something else of mine, written straight under its final name: shown
    let f = cores.join(format!("core.vivaldi-bin.{me}.{BOOT}.104.1794175822000000.zst"));
    std::fs::write(&f, b"x").unwrap();
    // (inotify reported the name before the attributes were set: the short name from the file name is used)
    assert_eq!(
        lines(&log, 2),
        vec!["/home/me/Games/game.x86_64|SIGSEGV".to_string(), "vivaldi-bin|a signal".to_string()]
    );
    // files that are not cores, and folders: nothing
    std::fs::write(cores.join("notes.txt"), b"x").unwrap();
    std::fs::create_dir(cores.join(format!("core.dir.{me}.{BOOT}.1.2"))).unwrap();
    store_core(&cores, "steam", me, 105, "/usr/lib/steam/steam", "5");
    assert_eq!(lines(&log, 3).last().map(String::as_str), Some("/usr/lib/steam/steam|SIGTRAP"));
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(lines(&log, 3).len(), 3, "{:?}", lines(&log, 3));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn waits_for_the_folder_to_exist() {
    let d = tmp("later");
    let cores = d.join("coredump");
    let (prog, log) = fake_notice(&d);
    let _w = start(&cores, &prog);
    std::fs::create_dir_all(&cores).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    if !store_core(&cores, "crashy", uid(), 7, "/tmp/crashy", "11") {
        eprintln!("no user.* attributes in {}: skipped", d.display());
        return;
    }
    assert_eq!(lines(&log, 1), vec!["/tmp/crashy|SIGSEGV".to_string()]);
    // removed and made again (a cleanup): still watched
    std::fs::remove_dir_all(&cores).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    std::fs::create_dir_all(&cores).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    store_core(&cores, "other", uid(), 8, "/tmp/other", "7");
    assert_eq!(lines(&log, 2).last().map(String::as_str), Some("/tmp/other|SIGBUS"));
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn dry_run_prints_instead() {
    let d = tmp("dry");
    let mut c = Command::new(env!("CARGO_BIN_EXE_yggdrasil-crashwatch"))
        .args(["--dry-run", "--dir", d.to_str().unwrap()])
        .env("CRASHWATCH_NOTICE", "/nonexistent")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let stored = store_core(&d, "game", uid(), 1, "/usr/bin/game", "8");
    std::thread::sleep(Duration::from_millis(300));
    let _ = c.kill();
    let out = c.wait_with_output().unwrap();
    if stored {
        assert_eq!(String::from_utf8_lossy(&out.stdout), "/usr/bin/game crashed (SIGFPE)\n");
    }
    let _ = std::fs::remove_dir_all(&d);
}
