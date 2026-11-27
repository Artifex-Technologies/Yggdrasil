//! The checks alone, on hostile input (the program as Agora runs it is tested in tests/pkg_root.rs).

use super::*;

const GOOD: &str = "/run/user/1000/agora/x.cancel";

fn v(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

fn check(args: &[&str]) -> Result<Plan, Refused> {
    let mut all = vec![GOOD, "pacman"];
    all.extend_from_slice(args);
    plan(&v(&all), "1000", &|p| p.starts_with("/tmp/pkgs/"))
}

#[test]
fn cancel_files_outside_the_callers_runtime_folder_are_refused() {
    for (path, uid, ok) in [
        (GOOD, "1000", true),
        ("/run/user/1000/agora/sub/x", "1000", true),
        ("/run/user/1000//agora/x", "1000", true),
        ("/run/user/1000/./agora/x", "1000", true),
        ("/run/user/1000/agora/", "1000", false),
        ("/run/user/1000/agora", "1000", false),
        ("/run/user/1000/agora/../x", "1000", false),
        ("/run/user/1000/agora/x..y", "1000", false),
        ("//run/user/1000/agora/x", "1000", false),
        ("///run/user/1000/agora/x", "1000", true),
        ("run/user/1000/agora/x", "1000", false),
        ("/run/user/10000/agora/x", "1000", false),
        ("/run/user/1000/agora/x", "", false),
        ("/run/user/1000/agora/x", "1000\n", false),
        ("/run/user/ 1000/agora/x", " 1000", false),
        ("/run/user/\u{0661}/agora/x", "\u{0661}", true),
        ("/run/user/1000/agoraX/x", "1000", false),
    ] {
        assert_eq!(valid_cancel_file(path, uid), ok, "{path:?} {uid:?}");
    }
}

#[test]
fn names_and_lists_are_plain_without_a_final_newline() {
    for ok in ["krita", "lib32-mesa", "qt6-base", "gtk+3", "a@b", "core/krita", "1:2.0-1", "x~y"] {
        assert!(plain_name(ok), "{ok}");
    }
    for bad in ["", "krita\n", "kri ta", "krita;ls", "$(id)", "`x`", "a|b", "a\u{0430}", "a\0b", "a\\b", "a'b", "a\"b", "a*", "a,b"] {
        assert!(!plain_name(bad), "{bad:?}");
    }
    assert!(plain_list("a,b,c"));
    for bad in ["", "a b", "x;y", "a\n", "a/b", "a:b"] {
        assert!(!plain_list(bad), "{bad:?}");
    }
}

#[test]
fn the_plan_is_the_scripts() {
    assert_eq!(check(&["-S", "--needed", "krita"]).unwrap(), Plan { args: v(&["-S", "--needed", "krita"]), installing: true });
    assert!(!check(&["-Rns", "--noconfirm", "krita"]).unwrap().installing);
    assert!(!check(&["-Syu", "--ignore", "a,b"]).unwrap().installing, "an upgrade is not undone");
    // -S -y: always the full upgrade, -y taken out, every -S made -Syu
    assert_eq!(
        check(&["-S", "-y", "--ignore", "x", "--noconfirm", "--config", "/etc/pacman.conf", "--"]).unwrap(),
        Plan { args: v(&["-Syu", "--ignore", "x", "--noconfirm", "--config", "/etc/pacman.conf", "--"]), installing: false }
    );
    assert!(check(&["-U", "--", "/tmp/pkgs/a-1-1-x86_64.pkg.tar.zst"]).unwrap().installing);
}

#[test]
fn hostile_arguments_are_refused() {
    let refused = |args: &[&str]| check(args).is_err();
    for args in [
        &["-S", "krita\n"][..],
        &["-S", "--ignore", "a\n", "krita"],
        &["-S", "--root", "/", "krita"],
        &["-S", "--root=/", "krita"],
        &["-S", "--config=/tmp/x", "krita"],
        &["-S", "--config", "/etc/pacman.conf\n", "krita"],
        &["-S", "--config", "/etc//pacman.conf", "krita"],
        &["-S", "--config"],
        &["-S", "-S", "krita"],
        &["-S", "-R", "krita"],
        &["-Sy", "krita"],
        &["-Q", "krita"],
        &["--sync", "krita"],
        &["-S", "--overwrite", "*", "krita"],
        &["-S", "--hookdir", "/tmp", "krita"],
        &["-S", "--dbpath", "/tmp", "krita"],
        &["-S", "--cachedir", "/tmp", "krita"],
        &["-S", "--gpgdir", "/tmp", "krita"],
        &["-S", "--assume-installed", "x", "krita"],
        &["-S", "-y", "krita"],
        &["-U", "-y"],
        &["-D", "krita"],
        &["-U", "--", "/tmp/pkgs/../../etc/x.pkg.tar.zst.sig"],
        &["-U", "--", "/etc/passwd"],
        &["-U", "--", "relative-1-1-x86_64.pkg.tar.zst"],
        &["-U", "--", "https://example.com/x.pkg.tar.zst"],
        &["-U", "--", "/elsewhere/x-1-1-x86_64.pkg.tar.zst"],
        &["-U", "--", "/tmp/pkgs/x-1-1-x86_64.pkg.tar.bz2"],
        &["--noconfirm", "--", "-S", "krita"],
        &["-U", "--", "/tmp/pkgs/a.pkg.tar.zst", "-R"],
    ] {
        assert!(refused(args), "{args:?}");
    }
    // the usage: pacman and at least two of its arguments
    assert!(plan(&v(&[GOOD, "pacman", "-Syu"]), "1000", &|_| true).is_err());
    assert!(plan(&v(&[GOOD, "/usr/bin/pacman", "-S", "krita"]), "1000", &|_| true).is_err());
    assert!(plan(&v(&[GOOD, "sh", "-c", "id"]), "1000", &|_| true).is_err());
}

#[test]
fn signals_give_the_exit_code_python_gave() {
    let st = |raw: i32| std::process::ExitStatus::from_raw(raw);
    assert_eq!(exit_of(st(0)), 0);
    assert_eq!(exit_of(st(1 << 8)), 1);
    assert_eq!(exit_of(st(libc::SIGTERM)), 241);
    assert_eq!(exit_of(st(libc::SIGKILL)), 247);
}

// ---- A component's setup steps: the OS's component command, run as root with one remembered password for a whole install

/// A scratch folder, removed afterwards.
struct Dir(PathBuf);

impl Dir {
    fn new(name: &str) -> Dir {
        let p = std::env::temp_dir().join(format!("pkg-root-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        Dir(p)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn me() -> u32 {
    // SAFETY: getuid has no preconditions
    unsafe { libc::getuid() }
}

fn executable(p: &Path, text: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// The OS's integration folder (a packages integration naming `component`) and the administrator's, with the component tool.
fn world(d: &Dir, component: &str) -> Vec<PathBuf> {
    let (os, admin) = (d.0.join("share"), d.0.join("etc"));
    std::fs::create_dir_all(&os).unwrap();
    std::fs::create_dir_all(&admin).unwrap();
    executable(&d.0.join("bin/os-apps"), "#!/bin/sh\nexit 0\n");
    std::fs::write(os.join("os-packages.conf"), format!("kind = packages\nname = OS\ncomponent = {component} # install ID\n")).unwrap();
    vec![os, admin]
}

#[test]
fn the_component_command_is_the_os_packages_integrations() {
    let d = Dir::new("found");
    let tool = d.0.join("bin/os-apps");
    let dirs = world(&d, tool.to_str().unwrap());
    assert_eq!(component_command(&dirs, me()), Ok(tool.clone()));
    // the administrator's file of the same name wins, as everywhere else
    let other = d.0.join("bin/admin-apps");
    executable(&other, "#!/bin/sh\n");
    std::fs::write(dirs[1].join("os-packages.conf"), format!("kind = packages\ncomponent = {}\n", other.display())).unwrap();
    assert_eq!(component_command(&dirs, me()), Ok(other));
}

#[test]
fn a_component_command_that_someone_else_could_change_is_refused() {
    use std::os::unix::fs::PermissionsExt;
    let set = |p: &Path, m: u32| std::fs::set_permissions(p, std::fs::Permissions::from_mode(m)).unwrap();
    let d = Dir::new("refused");
    let tool = d.0.join("bin/os-apps");
    let dirs = world(&d, tool.to_str().unwrap());
    assert!(component_command(&dirs, me() + 1).is_err(), "not the owner's");
    set(&tool, 0o777);
    assert!(component_command(&dirs, me()).is_err(), "anyone can write the program");
    set(&tool, 0o755);
    set(&dirs[0], 0o777);
    assert!(component_command(&dirs, me()).is_err(), "anyone can replace the integration file");
    set(&dirs[0], 0o755);
    set(&d.0.join("bin"), 0o777);
    assert!(component_command(&dirs, me()).is_err(), "anyone can replace the program");
    set(&d.0.join("bin"), 0o755);
    assert!(component_command(&dirs, me()).is_ok());
    for bad in ["os-apps".to_string(), format!("{} --flag", tool.display()), format!("{}/../bin/os-apps", d.0.join("bin").display())] {
        let d = Dir::new("odd");
        let dirs = world(&d, &bad);
        assert!(component_command(&dirs, me()).is_err(), "{bad}");
    }
    let d = Dir::new("none");
    assert!(component_command(&[d.0.join("share"), d.0.join("etc")], me()).is_err(), "no packages integration");
}

fn setup_check(args: &[&str], dirs: &[PathBuf]) -> Result<ComponentPlan, Refused> {
    let mut all = vec![GOOD, "component"];
    all.extend_from_slice(args);
    component_plan(&v(&all), "1000", dirs, me())
}

#[test]
fn component_steps_are_setup_or_teardown_of_plain_ids() {
    let d = Dir::new("plan");
    let tool = d.0.join("bin/os-apps");
    let dirs = world(&d, tool.to_str().unwrap());
    assert_eq!(
        setup_check(&["setup", "waydroid", "razer"], &dirs),
        Ok(ComponentPlan { command: tool.clone(), verb: "setup".into(), ids: v(&["waydroid", "razer"]) })
    );
    assert_eq!(setup_check(&["teardown", "waydroid"], &dirs).map(|p| p.verb), Ok("teardown".into()));
    for bad in [&["install", "waydroid"][..], &["remove", "x"], &["setup"], &["setup", "-x"], &["setup", "../a"], &["setup", "a b"]] {
        assert!(setup_check(bad, &dirs).is_err(), "{bad:?}");
    }
    for bad in ["a;b", "$(id)", "", "A", "a/b", "x\n"] {
        assert!(setup_check(&["setup", bad], &dirs).is_err(), "{bad:?}");
    }
    assert!(component_plan(&v(&["/etc/passwd", "component", "setup", "x"]), "1000", &dirs, me()).is_err());
    assert!(component_plan(&v(&[GOOD, "component", "setup", "x"]), "", &dirs, me()).is_err());
}

#[test]
fn the_setup_runs_with_a_clean_environment_as_the_user_who_asked() {
    let d = Dir::new("run");
    let tool = d.0.join("bin/os-apps");
    executable(&tool, &format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > {o}/args\nenv > {o}/env\nexit 3\n", o = d.0.display()));
    let p = ComponentPlan { command: tool, verb: "setup".into(), ids: v(&["waydroid"]) };
    assert_eq!(run_component(&p, d.0.join("never.cancel").to_str().unwrap(), "someone"), Ok(3), "its exit code comes back");
    assert_eq!(std::fs::read_to_string(d.0.join("args")).unwrap(), "setup\nwaydroid\n");
    let env = std::fs::read_to_string(d.0.join("env")).unwrap();
    let mut keys: Vec<&str> =
        env.lines().filter_map(|l| l.split_once('=')).map(|(k, _)| k).filter(|k| !["PWD", "SHLVL", "_"].contains(k)).collect();
    keys.sort();
    assert_eq!(keys, ["HOME", "LANG", "PATH", "SUDO_USER"], "nothing of the caller's environment: {env}");
    assert!(env.lines().any(|l| l == "SUDO_USER=someone"), "{env}");
    assert!(env.lines().any(|l| l == "HOME=/root"), "{env}");
}

#[test]
fn a_cancelled_setup_is_stopped() {
    let d = Dir::new("cancel");
    let tool = d.0.join("bin/os-apps");
    executable(&tool, "#!/bin/sh\nsleep 30\n");
    let cancel = d.0.join("x.cancel");
    std::fs::write(&cancel, "").unwrap();
    let p = ComponentPlan { command: tool, verb: "setup".into(), ids: v(&["waydroid"]) };
    let t = std::time::Instant::now();
    assert_eq!(run_component(&p, cancel.to_str().unwrap(), "someone"), Ok(130));
    assert!(t.elapsed() < Duration::from_secs(10), "stopped, not waited for");
}

#[test]
fn the_user_who_asked_is_named_from_pkexec_uid() {
    assert_eq!(user_name("0").as_deref(), Some("root"));
    assert_eq!(user_name("x"), None);
    assert_eq!(user_name(""), None);
}
