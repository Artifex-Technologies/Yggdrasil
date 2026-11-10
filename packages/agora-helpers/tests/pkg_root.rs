//! pkg-root as Agora runs it (the Python script's tests/test_agora_pkg_root.py, carried over): its refusals, and the exact
//! shapes yay sends through the sudo stand-in, against a stand-in pacman that writes down its arguments. Then a cancel, in a
//! user and mount namespace of its own (tests/sandbox.sh): pacman stopped, what it installed removed, its downloads deleted.
//! The cancel tests skip themselves where `unshare -rm` is not allowed.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_pkg-root");
const GOOD: &str = "/run/user/1000/agora/x.cancel";

fn run(args: &[&str], uid: &str) -> Output {
    Command::new(BIN).args(args).env("PKEXEC_UID", uid).output().unwrap()
}

fn code(o: &Output) -> i32 {
    o.status.code().unwrap_or(-1)
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

// ---- Refusals

#[test]
fn test_needs_pacman_and_arguments() {
    assert_eq!(code(&run(&[GOOD, "ls", "-S", "krita"], "1000")), 2);
    assert_eq!(code(&run(&[GOOD], "1000")), 2);
}

#[test]
fn test_cancel_file_must_be_the_invoking_users_runtime_folder() {
    assert_eq!(code(&run(&["/etc/passwd", "pacman", "-S", "krita"], "1000")), 2);
    assert_eq!(code(&run(&["/run/user/1000/agora/../../0/x", "pacman", "-S", "krita"], "1000")), 2);
    assert_eq!(code(&run(&["/run/user/1001/agora/x", "pacman", "-S", "krita"], "1000")), 2);
    assert_eq!(code(&run(&[GOOD, "pacman", "-S", "krita"], "")), 2);
}

#[test]
fn test_options_outside_the_short_list_are_refused() {
    for bad in ["--root", "--config", "--hookdir", "--sysroot", "-Sy", "--overwrite", "--dbpath"] {
        let r = run(&[GOOD, "pacman", "-S", bad, "krita"], "1000");
        assert_eq!(code(&r), 2, "{bad}");
        assert!(err(&r).contains("not allowed") || err(&r).contains("may only name"), "{}", err(&r));
    }
}

#[test]
fn test_odd_package_names_are_refused() {
    for bad in ["krita;ls", "$(id)", "a b", "`x`"] {
        assert_eq!(code(&run(&[GOOD, "pacman", "-S", bad], "1000")), 2, "{bad}");
    }
}

#[test]
fn test_ignore_takes_a_plain_list_and_syu_is_allowed() {
    assert_eq!(code(&run(&[GOOD, "pacman", "-Syu", "--ignore", "a b"], "1000")), 2);
    assert_eq!(code(&run(&[GOOD, "pacman", "-Syu", "--ignore", "x;y"], "1000")), 2);
    assert_eq!(code(&run(&[GOOD, "pacman", "-Syu", "--ignore"], "1000")), 2);
}

#[test]
fn test_only_install_and_remove() {
    assert_eq!(code(&run(&[GOOD, "pacman", "--needed", "krita"], "1000")), 2);
}

// ---- WhatYaySends: yay installs what it built through the sudo stand-in; these exact shapes must pass, and nothing wider

struct Yay {
    d: PathBuf,
    log: PathBuf,
    pkg: String,
}

impl Drop for Yay {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.d);
    }
}

fn tmp(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("pkg-root-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn yay() -> Yay {
    let d = tmp("yay");
    let log = d.join("pacman.log");
    // a stand-in pacman: records its arguments, lists nothing as installed
    fs::write(d.join("pacman"), format!("#!/bin/sh\n[ \"$1\" = -Qq ] && exit 0\necho \"$*\" >> {}\n", log.display())).unwrap();
    fs::set_permissions(d.join("pacman"), fs::Permissions::from_mode(0o755)).unwrap();
    let pkg = d.join("cadcraft-bin-0.12.0-1-x86_64.pkg.tar.zst");
    fs::write(&pkg, "").unwrap();
    Yay { d, log, pkg: pkg.to_string_lossy().into_owned() }
}

impl Yay {
    fn go(&self, args: &[&str]) -> Output {
        let path = format!("{}:{}", self.d.display(), std::env::var("PATH").unwrap_or_default());
        Command::new(BIN).arg(GOOD).arg("pacman").args(args).env("PKEXEC_UID", "1000").env("PATH", path).output().unwrap()
    }
    fn log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }
}

#[test]
fn test_installing_built_packages_passes() {
    let y = yay();
    let r = y.go(&["-U", "--ignore", "klassy", "--ignore", "sarina-git", "--noconfirm", "--config", "/etc/pacman.conf", "--", &y.pkg]);
    assert_eq!(code(&r), 0, "{}", err(&r));
    assert!(y.log().contains("-U --ignore klassy"));
}

#[test]
fn test_marking_install_reasons_passes() {
    let y = yay();
    assert_eq!(code(&y.go(&["-D", "--asdeps", "--config", "/etc/pacman.conf", "--", "somelib"])), 0);
    // exactly what yay sends after installing what it built
    assert_eq!(code(&y.go(&["-D", "-q", "--asexplicit", "--noconfirm", "--config", "/etc/pacman.conf", "--", "deckcraft-bin", "gridcraft-bin"])), 0);
    assert_eq!(code(&y.go(&["-D", "--", "somelib"])), 2, "-D without --asdeps/--asexplicit");
}

#[test]
fn test_another_config_file_is_refused() {
    let y = yay();
    let r = y.go(&["-U", "--noconfirm", "--config", "/tmp/evil.conf", "--", &y.pkg]);
    assert_eq!(code(&r), 2);
    assert!(err(&r).contains("--config"));
}

#[test]
fn test_install_from_file_needs_a_real_package_file() {
    let y = yay();
    let missing = y.d.join("missing-1-x86_64.pkg.tar.zst").to_string_lossy().into_owned();
    for bad in ["https://example.com/x.pkg.tar.zst", "/etc/passwd", missing.as_str()] {
        assert_eq!(code(&y.go(&["-U", "--noconfirm", "--", bad])), 2, "{bad}");
    }
}

#[test]
fn test_a_refresh_is_always_a_full_upgrade() {
    let y = yay();
    let r = y.go(&["-S", "-y", "--ignore", "klassy", "--noconfirm", "--config", "/etc/pacman.conf", "--"]);
    assert_eq!(code(&r), 0, "{}", err(&r));
    let ran = y.log();
    let words: Vec<&str> = ran.split_whitespace().collect();
    assert!(words.contains(&"-Syu"));
    assert!(!words.contains(&"-y"));
    assert_eq!(code(&y.go(&["-S", "-y", "--noconfirm", "--", "krita"])), 2, "refresh, then install one package: a partial upgrade");
}

#[test]
fn test_options_after_the_separator_are_not_options() {
    let y = yay();
    let r = y.go(&["-U", "--noconfirm", "--", &y.pkg, "-S"]);
    assert_eq!(code(&r), 2, "a second operation hidden after --");
    assert_eq!(code(&y.go(&["--noconfirm", "--", "-S", "krita"])), 2, "no operation before --");
}

// ---- more: pacman's own exit code, and a cancel

#[test]
fn pacmans_exit_code_is_given_back() {
    let y = yay();
    fs::write(y.d.join("pacman"), "#!/bin/sh\n[ \"$1\" = -Qq ] && exit 0\nexit 3\n").unwrap();
    assert_eq!(code(&y.go(&["-S", "--noconfirm", "krita"])), 3);
    fs::write(y.d.join("pacman"), "#!/bin/sh\n[ \"$1\" = -Qq ] && exit 0\nkill -TERM $$\n").unwrap();
    assert_eq!(code(&y.go(&["-S", "--noconfirm", "krita"])), 241, "killed by SIGTERM: sys.exit(-15)");
}

fn namespaces() -> bool {
    Command::new("unshare").args(["-rm", "true"]).output().map(|o| o.status.success()).unwrap_or(false)
        && std::path::Path::new("/var/cache/pacman/pkg").is_dir()
        && std::path::Path::new("/run/user").is_dir()
}

/// A world for a cancel: a stand-in pacman whose install adds "newpkg", downloads a file and then takes its time.
fn cancel_world() -> PathBuf {
    let w = tmp("cancel");
    for d in ["agora", "cache", "bin"] {
        fs::create_dir_all(w.join(d)).unwrap();
    }
    fs::write(w.join("installed"), "base\nkrita\n").unwrap();
    fs::write(w.join("cache/old-1-1-x86_64.pkg.tar.zst"), "old").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    fs::File::options().write(true).open(w.join("cache/old-1-1-x86_64.pkg.tar.zst")).unwrap().set_modified(old).unwrap();
    let wd = w.display();
    fs::write(
        w.join("bin/pacman"),
        format!(
            "#!/bin/sh\nif [ \"$1\" = -Qq ]; then cat {wd}/installed; exit 0; fi\necho \"pacman $*\" >> {wd}/calls\n\
             case \"$1\" in -Rns) exit 0 ;; esac\necho newpkg >> {wd}/installed\necho x > /var/cache/pacman/pkg/newpkg-1-1-x86_64.pkg.tar.zst\n\
             echo x > /var/cache/pacman/pkg/other.pkg.tar.zst.part\nexec sleep 30\n"
        ),
    )
    .unwrap();
    fs::set_permissions(w.join("bin/pacman"), fs::Permissions::from_mode(0o755)).unwrap();
    w
}

#[test]
fn a_cancel_stops_pacman_and_undoes_the_install() {
    if !namespaces() {
        eprintln!("skipped: no user namespaces here");
        return;
    }
    let w = cancel_world();
    let sandbox = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sandbox.sh");
    let path = format!("{}:/usr/bin:/bin", w.join("bin").display());
    let child = Command::new("unshare")
        .args(["-rm", "sh", sandbox])
        .arg(&w)
        .args([BIN, GOOD, "pacman", "-S", "--noconfirm", "newpkg"])
        .env("PKEXEC_UID", "1000")
        .env("PATH", &path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800));
    fs::write(w.join("agora/x.cancel"), "").unwrap();
    let start = std::time::Instant::now();
    let o = child.wait_with_output().unwrap();
    assert!(start.elapsed().as_secs() < 10, "pacman was stopped, not waited for");
    assert_eq!(o.status.code(), Some(130), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "Removing what this install added: newpkg\n");
    let calls = fs::read_to_string(w.join("calls")).unwrap();
    assert_eq!(calls, "pacman -S --noconfirm newpkg\npacman -Rns --noconfirm newpkg\n");
    let mut left: Vec<String> = fs::read_dir(w.join("cache")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    assert_eq!(left, vec!["old-1-1-x86_64.pkg.tar.zst".to_string()], "only this run's downloads go");
    let _ = fs::remove_dir_all(&w);
}

#[test]
fn a_remove_that_is_cancelled_is_not_undone() {
    if !namespaces() {
        eprintln!("skipped: no user namespaces here");
        return;
    }
    let w = cancel_world();
    // the cancel file is there before it starts: pacman is stopped at the first look
    fs::write(w.join("agora/x.cancel"), "").unwrap();
    let sandbox = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sandbox.sh");
    let o = Command::new("unshare")
        .args(["-rm", "sh", sandbox])
        .arg(&w)
        .args([BIN, GOOD, "pacman", "-R", "--noconfirm", "krita"])
        .env("PKEXEC_UID", "1000")
        .env("PATH", format!("{}:/usr/bin:/bin", w.join("bin").display()))
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(130));
    assert_eq!(String::from_utf8_lossy(&o.stdout), "");
    assert!(w.join("cache/old-1-1-x86_64.pkg.tar.zst").exists());
    let _ = fs::remove_dir_all(&w);
}
