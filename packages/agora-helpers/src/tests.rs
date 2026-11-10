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
    assert_eq!(check(&["-Rns", "--noconfirm", "krita"]).unwrap().installing, false);
    assert_eq!(check(&["-Syu", "--ignore", "a,b"]).unwrap().installing, false, "an upgrade is not undone");
    // -S -y: always the full upgrade, -y taken out, every -S made -Syu
    assert_eq!(check(&["-S", "-y", "--ignore", "x", "--noconfirm", "--config", "/etc/pacman.conf", "--"]).unwrap(),
               Plan { args: v(&["-Syu", "--ignore", "x", "--noconfirm", "--config", "/etc/pacman.conf", "--"]), installing: false });
    assert_eq!(check(&["-U", "--", "/tmp/pkgs/a-1-1-x86_64.pkg.tar.zst"]).unwrap().installing, true);
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
