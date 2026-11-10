//! The account helper as Settings runs it, against a made-up /etc and stand-in system tools that only write down what they
//! were asked: the Python script's two test files carried over (helpers/tests/test_account.py, the access commands, and
//! tests/test_settings_account.py, add/remove/admin), then hostile input: names, full names, passwords and caller ids
//! built to slip past a check.

use std::collections::HashMap;
use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};

const BIN: &str = env!("CARGO_BIN_EXE_account");

struct World {
    d: PathBuf,
    env: HashMap<String, String>,
}

impl Drop for World {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.d);
    }
}

fn tmp() -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let d = std::env::temp_dir().join(format!("account-test-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn tool(dir: &PathBuf, name: &str, body: &str) {
    let p = dir.join(name);
    fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

impl World {
    fn run(&self, args: &[&str], stdin: &str, extra: &[(&str, &str)], remove: &[&str]) -> Out {
        let mut c = Command::new(BIN);
        c.args(args).env_clear().stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in &self.env {
            if !remove.contains(&k.as_str()) {
                c.env(k, v);
            }
        }
        for (k, v) in extra {
            c.env(k, v);
        }
        let mut child = c.spawn().unwrap();
        // a helper that stops before it reads leaves the pipe closed
        let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
        let o = child.wait_with_output().unwrap();
        Out { code: o.status.code().unwrap_or(-1), stdout: String::from_utf8_lossy(&o.stdout).into(), stderr: String::from_utf8_lossy(&o.stderr).into() }
    }

    fn calls(&self) -> String {
        fs::read_to_string(self.d.join("calls")).unwrap_or_default() + &fs::read_to_string(self.d.join("calls.log")).unwrap_or_default()
    }

    fn write(&self, name: &str, text: &str) {
        fs::write(self.d.join(name), text).unwrap();
    }

    fn listed(&self) -> serde_json::Value {
        serde_json::from_str(&self.run(&["list"], "", &[], &[]).stdout).unwrap()
    }
}

fn account<'a>(list: &'a serde_json::Value, login: &str) -> &'a serde_json::Value {
    list.as_array().unwrap().iter().find(|a| a["login"] == login).unwrap()
}

/// test_account.py's world: ada (an administrator, the caller), bob, kept (protected); stand-ins that log "tool args".
fn access() -> World {
    let d = tmp();
    fs::write(d.join("passwd"), "root:x:0:0::/root:/bin/bash\nada:x:1000:1000:Ada:/home/ada:/bin/bash\nbob:x:1001:1001:Bob:/home/bob:/bin/bash\nkept:x:1002:1002:Kept:/home/kept:/bin/bash\n").unwrap();
    fs::write(d.join("group"), "wheel:x:998:ada\nlibvirt:x:990:bob\nuucp:x:987:\n").unwrap();
    fs::write(d.join("protected"), "kept\n").unwrap();
    fs::create_dir(d.join("sudoers.d")).unwrap();
    let bin = d.join("bin");
    fs::create_dir(&bin).unwrap();
    for t in ["gpasswd", "usermod", "chpasswd", "loginctl", "visudo"] {
        tool(&bin, t, &format!("echo \"{t} $*\" >> \"{}/calls\"", d.display()));
    }
    let p = |n: &str| d.join(n).to_string_lossy().into_owned();
    let env = HashMap::from([
        ("YGGDRASIL_ACCOUNT_TEST".to_string(), "1".to_string()),
        ("YGGDRASIL_PASSWD".into(), p("passwd")),
        ("YGGDRASIL_GROUP".into(), p("group")),
        ("YGGDRASIL_PROTECTED_ACCOUNTS".into(), p("protected")),
        ("YGGDRASIL_SUDOERS_D".into(), p("sudoers.d")),
        ("PKEXEC_UID".into(), "1000".into()),
        ("PATH".into(), format!("{}:/usr/bin:/bin", bin.display())),
    ]);
    World { d, env }
}

/// test_settings_account.py's world: robin (administrator, the caller), claude-agent (administrator, protected), sam.
fn accounts() -> World {
    let d = tmp();
    let calls = d.join("calls.log").to_string_lossy().into_owned();
    fs::write(d.join("passwd"), "root:x:0:0::/root:/bin/bash\nrobin:x:1000:1000:Robin,,,:/home/robin:/bin/bash\nclaude-agent:x:1001:1001::/home/claude-agent:/bin/bash\nsam:x:1002:1002:Sam Doe:/home/sam:/bin/bash\nnobody:x:65534:65534::/:/usr/bin/nologin\n").unwrap();
    fs::write(d.join("group"), "wheel:x:998:robin,claude-agent\nusers:x:985:\naudio:x:996:\nvideo:x:983:\nstorage:x:100:\nlp:x:7:\n").unwrap();
    for (name, body) in [
        ("useradd", format!("echo \"useradd $*\" >> {calls}")),
        ("userdel", format!("echo \"userdel $*\" >> {calls}")),
        ("gpasswd", format!("echo \"gpasswd $*\" >> {calls}")),
        ("chpasswd", format!("echo \"chpasswd $(cat)\" >> {calls}\n[ -z \"$CHPASSWD_FAIL\" ]")),
        ("getent", "exit 2".to_string()),
        ("loginctl", "echo \"$SESSIONS\"".to_string()),
    ] {
        tool(&d, name, &body);
    }
    fs::write(d.join("protected-accounts"), "# the OS's own\nclaude-agent*\nsarina\n").unwrap();
    let p = |n: &str| d.join(n).to_string_lossy().into_owned();
    let env = HashMap::from([
        ("PATH".to_string(), format!("{}:/usr/bin:/bin", d.display())),
        ("YGGDRASIL_PASSWD".into(), p("passwd")),
        ("YGGDRASIL_GROUP".into(), p("group")),
        ("YGGDRASIL_ACCOUNT_TEST".into(), "1".into()),
        ("PKEXEC_UID".into(), "1000".into()),
        ("YGGDRASIL_PROTECTED_ACCOUNTS".into(), p("protected-accounts")),
    ]);
    World { d, env }
}

// ---- test_account.py

#[test]
fn test_the_list_tells_access() {
    let w = access();
    let a = w.listed();
    assert!(account(&a, "ada")["admin"] == true && account(&a, "bob")["admin"] == false);
    assert_eq!(account(&a, "bob")["groups"], serde_json::json!(["libvirt"]));
    assert_eq!(account(&a, "kept")["protected"], true);
    let g: serde_json::Value = serde_json::from_str(&w.run(&["groups"], "", &[], &[]).stdout).unwrap();
    assert_eq!(g, serde_json::json!({"libvirt": "Run virtual machines", "uucp": "Use serial ports (Arduino, 3D printers, modems)"}));
}

#[test]
fn test_sudo_without_password_is_a_file_of_ours_and_goes_with_the_admin_rights() {
    let w = access();
    assert_eq!(w.run(&["sudo", "ada", "nopassword"], "", &[], &[]).code, 0);
    let rule = fs::read_to_string(w.d.join("sudoers.d/60-yggdrasil-nopasswd-ada")).unwrap();
    assert!(rule.contains("ada ALL=(ALL:ALL) NOPASSWD: ALL"));
    assert_eq!(fs::metadata(w.d.join("sudoers.d/60-yggdrasil-nopasswd-ada")).unwrap().permissions().mode() & 0o777, 0o440);
    assert_eq!(account(&w.listed(), "ada")["sudo_nopassword"], true);
    assert_ne!(w.run(&["sudo", "bob", "nopassword"], "", &[], &[]).code, 0, "not an administrator");
    assert_eq!(w.run(&["sudo", "ada", "password"], "", &[], &[]).code, 0);
    assert_eq!(account(&w.listed(), "ada")["sudo_nopassword"], false);
}

#[test]
fn test_accounts_kept_by_the_system_are_never_changed() {
    let w = access();
    for args in [&["sudo", "kept", "nopassword"][..], &["group", "kept", "uucp", "on"], &["login", "kept", "off"], &["name", "kept", "X"]] {
        assert_ne!(w.run(args, "", &[], &[]).code, 0, "{args:?}");
    }
    assert_eq!(w.calls(), "");
}

#[test]
fn test_only_listed_groups_can_be_switched() {
    let w = access();
    assert_eq!(w.run(&["group", "bob", "uucp", "on"], "", &[], &[]).code, 0);
    assert!(w.calls().contains("gpasswd -a bob uucp"));
    assert_ne!(w.run(&["group", "bob", "wheel", "on"], "", &[], &[]).code, 0);
    assert_ne!(w.run(&["group", "bob", "docker", "on"], "", &[], &[]).code, 0, "not on this system");
}

#[test]
fn test_signing_in_cannot_be_taken_from_yourself_or_the_last_administrator() {
    let w = access();
    assert_ne!(w.run(&["login", "ada", "off"], "", &[], &[]).code, 0);
    assert_eq!(w.run(&["login", "bob", "off"], "", &[], &[]).code, 0);
    assert!(w.calls().contains("usermod -s /usr/bin/nologin -L bob"));
}

#[test]
fn test_a_password_for_someone_else_never_on_the_command_line() {
    let w = access();
    assert_eq!(w.run(&["password", "bob"], "a-long-password\n", &[], &[]).code, 0);
    assert!(!w.calls().contains("a-long-password"));
    assert_ne!(w.run(&["password", "ada"], "a-long-password\n", &[], &[]).code, 0, "your own goes through passwd");
    assert_ne!(w.run(&["password", "bob"], "short\n", &[], &[]).code, 0);
}

// ---- test_settings_account.py

#[test]
fn test_list_shows_login_accounts_and_who_is_admin() {
    let w = accounts();
    let a = w.listed();
    let got: Vec<(String, bool)> = a.as_array().unwrap().iter().map(|x| (x["login"].as_str().unwrap().to_string(), x["admin"].as_bool().unwrap())).collect();
    assert_eq!(got, vec![("robin".into(), true), ("claude-agent".into(), true), ("sam".into(), false)]);
    assert_eq!(a[2]["full_name"], "Sam Doe");
}

#[test]
fn test_add_standard_user_never_puts_the_password_on_a_command_line() {
    let w = accounts();
    let r = w.run(&["add", "alex", "--full-name", "Alex: The Great\nroot"], "correct horse 9\n", &[], &[]);
    assert_eq!(r.code, 0, "{}", r.stderr);
    let c = w.calls();
    assert!(c.contains("useradd -m -s /bin/bash -c Alex  The Great root -G users,audio,video,storage,lp alex"), "{c}");
    assert!(!c.split("chpasswd").next().unwrap().contains("wheel"));
    assert!(c.contains("chpasswd alex:correct horse 9"));
    assert!(!c.split("chpasswd").next().unwrap().contains("correct horse"));
}

#[test]
fn test_add_administrator() {
    let w = accounts();
    assert_eq!(w.run(&["add", "pat", "--admin"], "longenough1\n", &[], &[]).code, 0);
    assert!(w.calls().split("chpasswd").next().unwrap().contains("wheel"));
}

#[test]
fn test_bad_names_short_passwords_and_duplicates_are_refused() {
    let w = accounts();
    let long = "x".repeat(40);
    for bad in ["Root", "1abc", "a b", "a:b", "../x", "", "root", "sarina", "claude-agent2", long.as_str()] {
        assert_ne!(w.run(&["add", bad], "longenough1\n", &[], &[]).code, 0, "{bad}");
    }
    assert_ne!(w.run(&["add", "alex"], "short\n", &[], &[]).code, 0);
    assert_ne!(w.run(&["add", "sam"], "longenough1\n", &[], &[]).code, 0);
    assert!(!w.calls().contains("useradd"));
}

#[test]
fn test_a_failed_password_rolls_the_account_back() {
    let w = accounts();
    let r = w.run(&["add", "alex"], "longenough1\n", &[("CHPASSWD_FAIL", "1")], &[]);
    assert_ne!(r.code, 0);
    assert!(w.calls().contains("userdel -r alex"));
}

#[test]
fn test_remove_rules() {
    let w = accounts();
    assert_eq!(w.run(&["remove", "sam"], "", &[], &[]).code, 0);
    assert!(w.calls().contains("userdel sam"));
    assert!(!w.calls().split("userdel").nth(1).unwrap().contains("-r"), "home kept by default");
    assert_eq!(w.run(&["remove", "sam", "--delete-home"], "", &[], &[]).code, 0);
    assert!(w.calls().contains("userdel -r sam"));
    for who in ["robin", "claude-agent", "root", "nobody", "nosuch"] {
        assert_ne!(w.run(&["remove", who], "", &[], &[]).code, 0, "{who}");
    }
    assert_ne!(w.run(&["remove", "sam"], "", &[("SESSIONS", "1 1002 sam seat0")], &[]).code, 0, "logged in");
}

#[test]
fn test_admin_changes_and_the_last_administrator() {
    let w = accounts();
    assert_eq!(w.run(&["admin", "sam", "on"], "", &[], &[]).code, 0);
    assert!(w.calls().contains("gpasswd -a sam wheel"));
    assert_ne!(w.run(&["admin", "robin", "off"], "", &[], &[]).code, 0, "your own rights");
    assert_ne!(w.run(&["admin", "claude-agent", "off"], "", &[], &[]).code, 0, "not ours to change here");
    w.write("group", "wheel:x:998:sam,claude-agent\nusers:x:985:\n");
    assert_ne!(w.run(&["admin", "sam", "off"], "", &[("PKEXEC_UID", "1000")], &[]).code, 0, "the only human administrator");
}

#[test]
fn test_needs_root_when_not_testing() {
    let w = accounts();
    if unsafe { libc::geteuid() } == 0 {
        return;
    }
    let r = w.run(&["add", "alex"], "longenough1\n", &[], &["YGGDRASIL_ACCOUNT_TEST"]);
    assert_ne!(r.code, 0);
    assert!(r.stderr.contains("root"));
}

// ---- hostile input

#[test]
fn names_built_to_slip_through_are_refused_before_any_tool_runs() {
    let w = accounts();
    for bad in ["alex\n", "alex\nroot", "-ralex", "--admin", "al ex", "alex;id", "$(id)", "al\u{0435}x", "\u{fb01}x", "ALEX", "_alex", "alex/..", "a\0b", "claude-agentx", "Sarina", "x".repeat(32).as_str()] {
        if bad.contains('\0') {
            continue; // a command line cannot carry it
        }
        let r = w.run(&["add", bad], "longenough1\n", &[], &[]);
        assert_eq!(r.code, 2, "{bad:?}: {}", r.stderr);
    }
    // the longest name that may be
    assert_eq!(w.run(&["add", &format!("a{}", "b".repeat(30))], "longenough1\n", &[], &[]).code, 0);
    assert_eq!(w.calls().matches("useradd").count(), 1);
}

#[test]
fn a_full_name_cannot_add_fields_or_lines_to_the_password_file() {
    let w = accounts();
    let evil = "x:0:0:root:/root:/bin/bash\nmallory::0:0::/:/bin/sh,\r\u{1}\u{1f}";
    assert_eq!(w.run(&["add", "alex", "--full-name", evil], "longenough1\n", &[], &[]).code, 0);
    let line = w.calls().lines().find(|l| l.starts_with("useradd")).unwrap().to_string();
    let comment = line.split(" -c ").nth(1).unwrap().split(" -G ").next().unwrap().to_string();
    assert!(!comment.contains(':') && !comment.contains(','), "{line}");
    tool(&w.d, "usermod", &format!("echo \"usermod $*\" >> {}/calls.log", w.d.display()));
    assert_eq!(w.run(&["name", "sam", evil], "", &[], &[]).code, 0);
    let c = w.calls();
    let usermod: Vec<&str> = c.lines().filter(|l| l.starts_with("gpasswd") || l.contains("usermod")).collect();
    assert!(usermod.iter().all(|l| !l.contains(':')));
    // at most 60 characters, the comment of a full name that is only spaces is the login
    assert_eq!(w.run(&["add", "long", "--full-name", &"\u{e9}".repeat(70)], "longenough1\n", &[], &[]).code, 0);
    assert!(w.calls().contains(&format!("-c {} -G", "\u{e9}".repeat(60))));
    assert_eq!(w.run(&["add", "blank", "--full-name", " :,\n "], "longenough1\n", &[], &[]).code, 0);
    assert!(w.calls().contains("-c blank -G"));
}

#[test]
fn a_password_goes_to_chpasswd_whole_and_only_there() {
    let w = accounts();
    // a colon and a carriage return stay in the password; the line ends at the first newline
    assert_eq!(w.run(&["password", "sam"], "pa:ss word\r\nnext line\n", &[], &[]).code, 0);
    assert!(w.calls().contains("chpasswd sam:pa:ss word\r"));
    assert!(!w.calls().contains("next line"));
    // eight characters, not eight bytes
    assert_eq!(w.run(&["password", "sam"], "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\n", &[], &[]).code, 2);
    assert_eq!(w.run(&["password", "sam"], "\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\u{e9}\n", &[], &[]).code, 0);
    // nothing on standard input, or no newline
    assert_eq!(w.run(&["password", "sam"], "", &[], &[]).code, 2);
    assert_eq!(w.run(&["password", "sam"], "longenough-no-newline", &[], &[]).code, 0);
}

#[test]
fn a_password_that_is_not_text_is_refused_without_running_anything() {
    let w = accounts();
    let mut c = Command::new(BIN);
    c.args(["password", "sam"]).env_clear().envs(&w.env).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    child.stdin.take().unwrap().write_all(b"longenough\xff\n").unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(1));
    assert!(!w.calls().contains("chpasswd"));
}

#[test]
fn the_caller_id_must_be_a_plain_number() {
    let w = accounts();
    // robin is 1000: a caller id that is not a plain number counts as no caller at all (as in the script; pkexec and sudo
    // always set a plain one)
    for (uid, can_remove_robin) in [("1000", false), ("01000", false), ("\u{0661}\u{0660}\u{0660}\u{0660}", false), ("1000\n", true), (" 1000", true), ("abc", true), ("", true)] {
        // robin is an administrator; make sam one too so the last-administrator rule does not decide
        w.write("group", "wheel:x:998:robin,sam\nusers:x:985:\n");
        let r = w.run(&["remove", "robin"], "", &[("PKEXEC_UID", uid), ("SUDO_UID", "")], &[]);
        assert_eq!(r.code == 0, can_remove_robin, "PKEXEC_UID={uid:?}: {}", r.stderr);
    }
    // SUDO_UID counts when PKEXEC_UID is not there
    let r = w.run(&["remove", "robin"], "", &[("SUDO_UID", "1000")], &["PKEXEC_UID"]);
    assert_ne!(r.code, 0);
    // a superscript digit: isdigit() said yes and int() no, so the script stopped (exit code 1)
    let r = w.run(&["remove", "sam"], "", &[("PKEXEC_UID", "\u{b9}")], &[]);
    assert_eq!(r.code, 1);
    assert!(!w.calls().contains("userdel sam"));
}

#[test]
fn groups_outside_the_list_and_odd_commands_are_refused() {
    let w = access();
    for g in ["wheel", "disk", "input", "root", "libvirt,wheel", "../wheel", "-", "uucp\n"] {
        assert_eq!(w.run(&["group", "bob", g, "on"], "", &[], &[]).code, 2, "{g:?}");
    }
    for args in [&["group", "bob", "uucp", "yes"][..], &["group", "bob", "uucp"], &["admin", "bob"], &["admin", "bob", "On"], &["sudo", "bob", "all"], &["login", "bob", "0"], &["frobnicate"], &["name", "bob"]] {
        assert_eq!(w.run(args, "", &[], &[]).code, 2, "{args:?}");
    }
    assert_eq!(w.calls(), "");
    // help, and nothing at all
    let r = w.run(&["--help"], "", &[], &[]);
    assert_eq!(r.code, 0);
    assert!(r.stdout.starts_with("account: add, remove and change accounts"));
    assert_eq!(w.run(&[], "", &[], &[]).code, 2);
}

#[test]
fn accounts_below_1000_or_from_60000_are_never_touched() {
    let w = access();
    w.write("passwd", "root:x:0:0::/root:/bin/bash\nada:x:1000:1000:Ada:/home/ada:/bin/bash\nsvc:x:999:999::/:/bin/sh\nhi:x:60000:60000::/:/bin/sh\nodd:x:\u{0661}\u{0660}\u{0660}\u{0660}:1:x:/:/bin/sh\nshort:x:1500\n");
    let a = w.listed();
    let logins: Vec<&str> = a.as_array().unwrap().iter().map(|x| x["login"].as_str().unwrap()).collect();
    assert_eq!(logins, vec!["ada", "odd"], "Arabic-Indic digits are a uid as int() read them");
    for who in ["svc", "hi", "root", "short"] {
        for args in [vec!["name", who, "X"], vec!["login", who, "off"], vec!["group", who, "uucp", "on"], vec!["sudo", who, "password"]] {
            assert_ne!(w.run(&args, "", &[], &[]).code, 0, "{args:?}");
        }
    }
    assert_eq!(w.calls(), "");
}

#[test]
fn the_list_is_the_scripts_json() {
    let w = access();
    w.write("passwd", "ada:x:1000:1000:Ad\u{e9} \"A\",x:/home/ada:/usr/bin/nologin\n");
    let out = w.run(&["list"], "", &[], &[]).stdout;
    assert_eq!(
        out,
        "[{\"login\": \"ada\", \"uid\": 1000, \"full_name\": \"Ad\\u00e9 \\\"A\\\"\", \"home\": \"/home/ada\", \"shell\": \"/usr/bin/nologin\", \"admin\": true, \"protected\": false, \"can_login\": false, \"sudo_nopassword\": false, \"groups\": []}]\n"
    );
}
