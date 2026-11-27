//! pkg-root as Agora runs it (the Python script's tests/test_agora_pkg_root.py, carried over): its refusals, and the exact
//! shapes yay sends through the sudo stand-in, against a stand-in pacman that writes down its arguments. Then a cancel, in a
//! user and mount namespace of its own (tests/sandbox.sh): pacman stopped, what it installed removed, its downloads deleted.
//! The cancel tests skip themselves where `unshare -rm` is not allowed.

use std::fs;
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
fn test_a_components_setup_takes_only_its_verb_and_plain_ids() {
    // each is refused before anything of the system is read: the component command itself is never started from a test
    let usage = "usage: pkg-root CANCELFILE component setup|teardown";
    for (bad, said) in [
        (&[GOOD, "component", "install", "waydroid"][..], usage),
        (&[GOOD, "component", "setup"], usage),
        (&[GOOD, "component", "setup", "a;b"], "odd component id"),
        (&[GOOD, "component", "setup", "--root=/"], "odd component id"),
        (&[GOOD, "component", "teardown", "../x"], "odd component id"),
        (&["/etc/passwd", "component", "setup", "waydroid"], "the cancel file must be"),
    ] {
        let r = run(bad, "1000");
        assert_eq!(code(&r), 2, "{bad:?}: {}", err(&r));
        assert!(err(&r).starts_with(&format!("agora pkg-root: {said}")), "{bad:?}: {}", err(&r));
    }
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
    write_script(&d.join("pacman"), &format!("#!/bin/sh\n[ \"$1\" = -Qq ] && exit 0\necho \"$*\" >> {}\n", log.display()));
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
    assert_eq!(
        code(&y.go(&["-D", "-q", "--asexplicit", "--noconfirm", "--config", "/etc/pacman.conf", "--", "deckcraft-bin", "gridcraft-bin"])),
        0
    );
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
    write_script(&y.d.join("pacman"), "#!/bin/sh\n[ \"$1\" = -Qq ] && exit 0\nexit 3\n");
    assert_eq!(code(&y.go(&["-S", "--noconfirm", "krita"])), 3);
    write_script(&y.d.join("pacman"), "#!/bin/sh\n[ \"$1\" = -Qq ] && exit 0\nkill -TERM $$\n");
    assert_eq!(code(&y.go(&["-S", "--noconfirm", "krita"])), 241, "killed by SIGTERM: sys.exit(-15)");
}

/// Whether the sandbox works here: a user and mount namespace, and the mounts tests/sandbox.sh makes in it (a container or a
/// build chroot may allow neither).
fn namespaces() -> bool {
    if !std::path::Path::new("/var/cache/pacman/pkg").is_dir() || !std::path::Path::new("/run/user").is_dir() {
        return false;
    }
    let w = tmp("probe");
    fs::create_dir_all(w.join("agora")).unwrap();
    fs::create_dir_all(w.join("cache")).unwrap();
    let ok = Command::new("unshare")
        .args(["-rm", "sh", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sandbox.sh")])
        .arg(&w)
        .arg("true")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    let _ = fs::remove_dir_all(&w);
    ok
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
    write_script(
        &w.join("bin/pacman"),
        &format!(
            "#!/bin/sh\nif [ \"$1\" = -Qq ]; then cat {wd}/installed; exit 0; fi\necho \"pacman $*\" >> {wd}/calls\n\
             case \"$1\" in -Rns) exit 0 ;; esac\necho newpkg >> {wd}/installed\necho x > /var/cache/pacman/pkg/newpkg-1-1-x86_64.pkg.tar.zst\n\
             echo x > /var/cache/pacman/pkg/other.pkg.tar.zst.part\nexec sleep 30\n"
        ),
    );
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
    let mut left: Vec<String> =
        fs::read_dir(w.join("cache")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
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

// ---- a component's setup, the whole way: the sudo stand-in, pkexec, pkg-root reading the integration, the component command

const SUDO: &str = env!("CARGO_BIN_EXE_agora-sudo");

/// Runs `agora-sudo ARGS...` (`{W}` in them: the world's folder) in the sandbox, with a scratch integration folder over the
/// system's, this build of pkg-root over the installed one and a stand-in pkexec (PKEXEC_UID 0: inside the namespace everything
/// here is root's). The world's folder and the output, or None where the sandbox or the paths it covers are missing (a
/// container: the test skips).
fn through_the_stand_in(args: &[&str]) -> Option<(PathBuf, Output)> {
    let (ints, helper) = ("/usr/share/yggdrasil/integrations", "/usr/lib/agora/pkg-root");
    if !namespaces() || !std::path::Path::new(ints).is_dir() || !std::path::Path::new(helper).is_file() {
        return None;
    }
    let w = tmp("setup");
    for d in ["agora", "cache", "bin", "integrations"] {
        fs::create_dir_all(w.join(d)).unwrap();
    }
    let wd = w.display().to_string();
    fs::write(w.join("integrations/os-packages.conf"), format!("kind = packages\nname = OS\ncomponent = {wd}/bin/os-apps\n")).unwrap();
    write_script(&w.join("bin/os-apps"), &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {wd}/args\nenv > {wd}/env\n"));
    write_script(&w.join("bin/other"), "#!/bin/sh\nexit 0\n");
    write_script(&w.join("bin/pkexec"), &format!("#!/bin/sh\necho \"$*\" >> {wd}/pkexec\nPKEXEC_UID=0 exec \"$@\"\n"));
    let inner = format!("mount --bind {wd}/integrations {ints} && mount --bind {BIN} {helper} && exec \"$0\" \"$@\"");
    let args: Vec<String> = args.iter().map(|a| a.replace("{W}", &wd)).collect();
    let o = Command::new("unshare")
        .args(["-rm", "sh", concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sandbox.sh")])
        .arg(&w)
        .args(["sh", "-c", &inner, SUDO])
        .args(&args)
        .env("PATH", format!("{wd}/bin:/usr/bin:/bin"))
        .env("CALLERS_OWN", "not passed on")
        .env_remove("AGORA_CANCEL_FILE")
        .output()
        .unwrap();
    Some((w, o))
}

#[test]
fn a_components_setup_goes_from_the_sudo_stand_in_through_pkg_root_to_its_command() {
    let Some((w, o)) = through_the_stand_in(&["{W}/bin/os-apps", "setup", "waydroid", "razer"]) else {
        eprintln!("skipped: no user namespaces, or no installed integration folder and pkg-root to cover");
        return;
    };
    assert_eq!(code(&o), 0, "{}", err(&o));
    let asked = fs::read_to_string(w.join("pkexec")).unwrap();
    assert!(asked.starts_with("/usr/lib/agora/pkg-root /run/user/0/agora/sudo-"), "through pkg-root: {asked}");
    assert!(asked.ends_with(".cancel component setup waydroid razer\n"), "{asked}");
    assert_eq!(fs::read_to_string(w.join("args")).unwrap(), "setup\nwaydroid\nrazer\n");
    let env = fs::read_to_string(w.join("env")).unwrap();
    assert!(env.lines().any(|l| l == "SUDO_USER=root"), "{env}");
    assert!(env.lines().any(|l| l == "PATH=/usr/local/sbin:/usr/local/bin:/usr/bin"), "{env}");
    assert!(!env.contains("CALLERS_OWN"), "nothing of the caller's environment: {env}");
    let _ = fs::remove_dir_all(&w);

    // another program's "setup" is not the component command's: plain pkexec, asked every time
    let Some((w, o)) = through_the_stand_in(&["{W}/bin/other", "setup", "waydroid"]) else { return };
    assert_eq!(code(&o), 0, "{}", err(&o));
    assert_eq!(fs::read_to_string(w.join("pkexec")).unwrap(), format!("{}/bin/other setup waydroid\n", w.display()));
    assert!(!w.join("args").exists());
    let _ = fs::remove_dir_all(&w);
}
