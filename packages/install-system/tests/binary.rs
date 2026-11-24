//! The program itself, run as a program: only what changes nothing (a plan refused, --check, --disks), with a PATH that holds
//! nothing but stand-ins writing down that they were called. (The installs are run by tests/install.rs on a made-up computer,
//! and as programs by parity/compare.py, in a namespace of their own.)

mod fake;

use std::process::{Command, Stdio};

fn stand_ins(dir: &fake::Dir, lsblk: &str) -> String {
    for t in [
        "lsblk",
        "findmnt",
        "sfdisk",
        "wipefs",
        "cryptsetup",
        "mkfs.btrfs",
        "mkfs.vfat",
        "mount",
        "umount",
        "blkid",
        "udevadm",
        "unsquashfs",
        "chroot",
    ] {
        let body = if t == "lsblk" { lsblk.to_string() } else { String::new() };
        std::fs::create_dir_all(dir.path("bin")).unwrap();
        write_script(
            std::path::Path::new(&dir.path(&format!("bin/{t}"))),
            &format!("#!/bin/sh\necho \"$0 $*\" >> \"{}\"\n{body}", dir.path("calls")),
        );
    }
    dir.path("bin")
}

fn run(args: &[&str], stdin: &[u8], path: &str) -> (i32, String, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_yggdrasil-install-system"))
        .args(args)
        .env_clear()
        .env("PATH", path)
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child.stdin.take().unwrap().write_all(stdin).unwrap();
    let out = child.wait_with_output().unwrap();
    (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into(), String::from_utf8_lossy(&out.stderr).into())
}

#[test]
fn refused_plans_run_nothing() {
    let dir = fake::Dir::new();
    let path = stand_ins(&dir, "");
    for (args, stdin, said) in [
        (&[][..], &b"{\"mode\": \"shrink\"}"[..], "ERROR unknown way to install (erase or manual)\n"),
        (&["--check"][..], &b"{\"mode\": \"erase\", \"disk\": 5}"[..], "ERROR no disk chosen\n"),
        (&[][..], &b"\xff"[..], "ERROR the plan is not JSON\n"),
        (&[][..], &b"{\"user\": {\"name\": \"\\ud800\"}}"[..], "ERROR the plan is not JSON\n"),
        (&[][..], &b"7"[..], "ERROR the plan is not a JSON object\n"),
    ] {
        let (code, out, err) = run(args, stdin, &path);
        assert_eq!((code, out.as_str(), err.as_str()), (2, said, ""), "{stdin:?}");
    }
    assert!(!std::path::Path::new(&dir.path("calls")).exists(), "a program was run");
}

#[test]
fn the_disks_from_lsblk() {
    let dir = fake::Dir::new();
    let disks = r#"{"blockdevices": [{"name": "vdz", "path": "/dev/vdz", "size": 536870912000, "model": " TEST DISK 0001 ", "tran": null, "ro": false, "type": "disk", "rm": false},
        {"name": "loop0", "path": "/dev/loop0", "size": 1, "model": null, "tran": null, "ro": true, "type": "loop", "rm": false}]}"#;
    let lsblk = format!("case \"$*\" in\n  \"-J -b -d -o \"*) echo '{disks}' ;;\n  *) echo '{{\"blockdevices\": [{{\"path\": \"/dev/vdz\", \"pttype\": null, \"type\": \"disk\"}}]}}' ;;\nesac\n");
    let path = stand_ins(&dir, &lsblk);
    let (code, out, err) = run(&["--disks"], b"", &path);
    assert_eq!((code, err.as_str()), (0, ""));
    assert!(
        out.starts_with(
            r#"[{"path": "/dev/vdz", "size": 536870912000, "model": "TEST DISK 0001", "removable": false, "transport": "", "big_enough": "#
        ),
        "{out}"
    );
    assert!(out.trim_end().ends_with(r#""table": "", "partitions": [], "free": [{"start": 1048576, "size": 536868814848}]}]"#), "{out}");
    let calls = std::fs::read_to_string(dir.path("calls")).unwrap();
    assert!(calls.lines().all(|l| l.contains("/bin/lsblk ") || l.contains("/bin/findmnt ")), "{calls}");
}

/// Writes an executable script with `install -m 755`, a program of its own, so that no thread of this test process ever holds
/// it open for writing: another test's program started meanwhile would inherit that handle until its exec, and running the
/// script then fails with "Text file busy" (ETXTBSY).
fn write_script(path: &std::path::Path, text: &str) {
    let src = path.with_extension("script-text");
    std::fs::write(&src, text).unwrap();
    let ok = std::process::Command::new("install").args(["-m", "755"]).arg(&src).arg(path).status().is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&src);
    assert!(ok, "install could not write {}", path.display());
}
