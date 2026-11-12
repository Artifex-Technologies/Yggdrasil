//! The service as it runs, on this user's own systemd: a made-up app's scope is started (systemd-run --user --scope, a short
//! sleep), and the service, with a scratch XDG_DATA_HOME that has the app's desktop entry, must count its seconds. Skipped
//! where there is no systemd user manager (a package build's chroot).

use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn user_manager() -> bool {
    Command::new("systemctl")
        .args(["--user", "is-system-running"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.code().is_some())
        .unwrap_or(false)
        && Command::new("systemd-run").arg("--version").stdout(Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
        && std::env::var_os("XDG_RUNTIME_DIR").is_some()
}

#[test]
fn a_scope_that_starts_and_ends_is_counted() {
    if !user_manager() {
        eprintln!("skipped: no systemd user manager");
        return;
    }
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("live-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("applications")).unwrap();
    let id = format!("appusage-livetest{}", std::process::id());
    fs::write(dir.join("applications").join(format!("{id}.desktop")), "[Desktop Entry]\nType=Application\nName=x\nExec=true\n").unwrap();
    let mut svc = Command::new(env!("CARGO_BIN_EXE_labyrinth-appusage"))
        .env("XDG_DATA_HOME", &dir)
        .env("XDG_DATA_DIRS", dir.join("none"))
        .env("LABYRINTH_APPUSAGE_FLUSH", "1")
        .stderr(Stdio::inherit())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let start = Instant::now();
    let ok = Command::new("systemd-run")
        .args(["--user", "--scope", "--quiet", &format!("--unit=app-{id}-{}", std::process::id()), "sleep", "3"])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    let ran = start.elapsed().as_secs_f64();
    assert!(ok, "systemd-run");
    // the service's own memory while it follows the tree
    let status = fs::read_to_string(format!("/proc/{}/status", svc.id())).unwrap_or_default();
    let rss = status.lines().find(|l| l.starts_with("VmRSS")).unwrap_or("").to_string();
    std::thread::sleep(Duration::from_millis(500));
    unsafe { libc::kill(svc.id() as i32, libc::SIGTERM) };
    let code = svc.wait().unwrap();
    assert!(code.success(), "stops cleanly on SIGTERM");
    let text = fs::read_to_string(dir.join("labyrinth/app-usage.json")).expect("the file was written");
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let secs = v[&id]["seconds"].as_i64().unwrap();
    eprintln!("counted {secs} s for a scope that ran {ran:.1} s; {rss}");
    assert!((2..=4).contains(&secs), "{secs} s for {ran:.1} s");
    let _ = fs::remove_dir_all(&dir);
}
