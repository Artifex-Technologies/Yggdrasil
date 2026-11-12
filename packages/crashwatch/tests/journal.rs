//! The watcher following a journal: a journal file standing in for the user's own user-UID.journal (written with
//! systemd-journal-remote from journald's export format, appended to in place as journald does, and replaced by a new one
//! as journald's rotation does), a folder standing in for /var/lib/systemd/coredump, and a stand-in notice program that
//! writes down what it was given. The crashes whose core was not stored are seen through their journal entry; a crash whose
//! core was stored as well is shown once. Nothing is shown and nothing crashes; where systemd-journal-remote is not
//! installed, the test says so and passes.

use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const BOOT: &str = "86b6f1565fe940188f8f77e469c8a952";
const REMOTE: &str = "/usr/lib/systemd/systemd-journal-remote";
const COREDUMP: &str = "fc2e22bc6ee647b6b90729ab34a250b1";

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

/// Entries appended to the journal file `file` (made when it is not there). Each entry is (MESSAGE_ID, fields); the
/// timestamps follow on from those before.
fn write_journal(file: &Path, first_n: u64, entries: &[(&str, Vec<(&str, String)>)]) {
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_micros() as u64;
    let mut export = String::new();
    for (i, (message_id, fields)) in entries.iter().enumerate() {
        let n = first_n + i as u64;
        export += &format!("__REALTIME_TIMESTAMP={}\n__MONOTONIC_TIMESTAMP={}\n_BOOT_ID={BOOT}\n", now + n, 1000 * n);
        export += &format!("MESSAGE=entry {n}\nMESSAGE_ID={message_id}\n");
        if !fields.iter().any(|(k, _)| *k == "_EXE") {
            // what journald adds for systemd-coredump's entries
            export += "_EXE=/usr/lib/systemd/systemd-coredump\n";
        }
        for (k, v) in fields {
            export += &format!("{k}={v}\n");
        }
        export += "\n";
    }
    // beside the journal folder, not in it
    let side = file.parent().unwrap().with_extension(format!("{first_n}.export"));
    std::fs::write(&side, export).unwrap();
    let out = Command::new(REMOTE).arg(format!("--output={}", file.display())).arg(&side).output().unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    std::fs::remove_file(&side).unwrap();
}

fn crash(owner: u32, pid: u32, exe: &str, signal: &str, stored: Option<&str>) -> (&'static str, Vec<(&'static str, String)>) {
    let mut f = vec![
        ("COREDUMP_UID", owner.to_string()),
        ("COREDUMP_PID", pid.to_string()),
        ("COREDUMP_EXE", exe.to_string()),
        ("COREDUMP_SIGNAL", signal.to_string()),
    ];
    if let Some(file) = stored {
        f.push(("COREDUMP_FILENAME", file.to_string()));
    }
    (COREDUMP, f)
}

fn set_xattr(f: &Path, k: &str, v: &str) -> bool {
    let p = CString::new(f.as_os_str().as_bytes()).unwrap();
    let n = CString::new(k).unwrap();
    // SAFETY: plain buffers of the given lengths
    unsafe { libc::setxattr(p.as_ptr(), n.as_ptr(), v.as_ptr().cast(), v.len(), 0) == 0 }
}

struct Running(Child);

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
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

#[test]
fn crashes_without_a_stored_core_come_from_the_journal() {
    if !Path::new(REMOTE).exists() {
        eprintln!("{REMOTE} is not installed: skipped");
        return;
    }
    let me = uid();
    let d = tmp("journal");
    let (cores, journal) = (d.join("coredump"), d.join("journal"));
    std::fs::create_dir_all(&cores).unwrap();
    std::fs::create_dir_all(&journal).unwrap();
    let file = journal.join(format!("user-{me}.journal"));
    // a crash from before the watcher started: never shown
    write_journal(&file, 1, &[crash(me, 400, "/usr/bin/earlier", "11", None)]);

    let log = d.join("notices.log");
    let prog = d.join("notice");
    std::fs::write(&prog, format!("#!/bin/sh\nprintf '%s|%s\\n' \"$1\" \"$2\" >> '{}'\n", log.display())).unwrap();
    std::fs::set_permissions(&prog, std::fs::Permissions::from_mode(0o755)).unwrap();
    let w = Command::new(env!("CARGO_BIN_EXE_yggdrasil-crashwatch"))
        .args(["--dir", cores.to_str().unwrap(), "--journal-dir", journal.to_str().unwrap()])
        .env("CRASHWATCH_NOTICE", &prog)
        .stdin(Stdio::null())
        .spawn()
        .unwrap();
    let _w = Running(w);
    std::thread::sleep(Duration::from_millis(300));

    write_journal(
        &file,
        10,
        &[
            // a core not stored (Storage=none, or over the size limit): only the journal tells of it
            crash(me, 501, "/opt/Game/game.x86_64", "11", None),
            // another account's crash, and another message that names this user: nothing
            crash(me + 1, 502, "/usr/bin/theirs", "6", None),
            (
                "0123456789abcdef0123456789abcdef",
                vec![("COREDUMP_UID", me.to_string()), ("COREDUMP_EXE", "/usr/bin/not-a-crash".to_string())],
            ),
            // the right message, written by some other program of this user (journald's _EXE is the writer): nothing
            (
                COREDUMP,
                vec![
                    ("_EXE", "/usr/bin/python3.14".to_string()),
                    ("COREDUMP_UID", me.to_string()),
                    ("COREDUMP_PID", "504".to_string()),
                    ("COREDUMP_EXE", "/usr/bin/forged".to_string()),
                    ("COREDUMP_SIGNAL", "11".to_string()),
                ],
            ),
            // the same program again: quiet for ten minutes, as for cores
            crash(me, 503, "/opt/Game/game.x86_64", "6", None),
        ],
    );
    assert_eq!(lines(&log, 1), vec!["/opt/Game/game.x86_64|SIGSEGV".to_string()]);

    // a crash whose core is stored: the core file, then its journal entry; one notice
    let name = format!("core.steam.{me}.{BOOT}.777.1794175822000000.zst");
    let temp = cores.join(format!(".#{name}"));
    std::fs::write(&temp, b"not a core").unwrap();
    let attrs = set_xattr(&temp, "user.coredump.exe", "/usr/lib/steam/steam") && set_xattr(&temp, "user.coredump.signal", "5");
    std::fs::rename(&temp, cores.join(&name)).unwrap();
    write_journal(&file, 20, &[crash(me, 777, "/usr/lib/steam/steam", "5", Some(&format!("/var/lib/systemd/coredump/{name}")))]);
    let want = if attrs { "/usr/lib/steam/steam|SIGTRAP" } else { "steam|a signal" };
    assert_eq!(lines(&log, 2).last().map(String::as_str), Some(want));
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(lines(&log, 2).len(), 2, "{:?}", lines(&log, 2));

    // journald's rotation: the full file renamed aside, a new one in its place, read from its first entry
    std::fs::rename(&file, journal.join(format!("user-{me}@0123456789abcdef0123456789abcdef-0000000000000001-0006000000000000.journal")))
        .unwrap();
    let next = d.join(format!("user-{me}.journal"));
    write_journal(&next, 30, &[crash(me, 901, "/usr/bin/after-rotation", "4", None)]);
    std::fs::rename(&next, &file).unwrap();
    assert_eq!(lines(&log, 3).last().map(String::as_str), Some("/usr/bin/after-rotation|SIGILL"));
    // and the new file is followed as it is written
    write_journal(&file, 40, &[crash(me, 902, "/usr/bin/later-still", "8", None)]);
    assert_eq!(lines(&log, 4).last().map(String::as_str), Some("/usr/bin/later-still|SIGFPE"));
    let _ = std::fs::remove_dir_all(&d);
}
