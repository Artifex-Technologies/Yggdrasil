//! account: add, remove and change accounts for Settings > Users (run with pkexec; `list` and `groups` need no root). The
//! Python script's rules, line for line (doc/account.txt is its help):
//!
//!   - a new name is 1 to 31 of a-z 0-9 _ -, starting with a letter, not reserved, not kept by the operating system
//!     (/etc/yggdrasil/protected-accounts: a name per line, `name*` for every name starting so);
//!   - only accounts from uid 1000 up to 59999 are touched, never one the operating system keeps, never the caller's own
//!     (PKEXEC_UID, else SUDO_UID) where that matters, never one that is logged in, never the last administrator;
//!   - a password only ever goes to chpasswd on its standard input, never on a command line;
//!   - the only groups switched are the optional ones this system has (OPTIONAL_GROUPS).
//!
//! One check is stricter than the script's: its name pattern ended in `$`, which in Python also matches before a final
//! newline, so "alex\n" passed it (useradd then refused the name). Here a newline is never part of a valid name.

use std::fs;
use std::os::unix::fs::PermissionsExt;

use pycompat::json::{self, Map, Value};

use crate::py::{self, Crash, R};

const DOC: &str = include_str!("doc/account.txt");

const RESERVED: &[&str] = &[
    "root", "nobody", "daemon", "bin", "sys", "sync", "games", "man", "lp", "mail", "news", "uucp", "proxy", "www-data", "backup", "list", "irc",
    "gnats", "sddm", "polkitd", "systemd",
];
const DESKTOP_GROUPS: &[&str] = &["users", "audio", "video", "network", "storage", "lp"];
/// Groups Settings may switch per account, with what they allow (in this order). Only those that exist on the system are
/// offered. Groups that are root in disguise for everyday use (disk, input: read every key typed) are not on the list.
pub const OPTIONAL_GROUPS: &[(&str, &str)] = &[
    ("libvirt", "Run virtual machines"),
    ("docker", "Run Docker containers (this is administrator-level access)"),
    ("wireshark", "Capture network traffic (Wireshark)"),
    ("uucp", "Use serial ports (Arduino, 3D printers, modems)"),
    ("gamemode", "Ask for the games performance mode"),
];
const NOLOGIN: &str = "/usr/bin/nologin";

struct Env {
    passwd: String,
    group: String,
    protected_file: String,
    sudoers_d: String,
    /// tests: no root needed, no visudo check
    test: bool,
}

impl Env {
    fn get() -> Env {
        Env {
            passwd: py::env_or("YGGDRASIL_PASSWD", "/etc/passwd"),
            group: py::env_or("YGGDRASIL_GROUP", "/etc/group"),
            protected_file: py::env_or("YGGDRASIL_PROTECTED_ACCOUNTS", "/etc/yggdrasil/protected-accounts"),
            sudoers_d: py::env_or("YGGDRASIL_SUDOERS_D", "/etc/sudoers.d"),
            test: py::env_set("YGGDRASIL_ACCOUNT_TEST"),
        }
    }
}

/// One login account, as `list` gives it.
#[derive(Debug, Clone)]
pub struct Account {
    pub login: String,
    pub uid: i128,
    pub full_name: String,
    pub home: String,
    pub shell: String,
    pub admin: bool,
    pub protected: bool,
    pub can_login: bool,
    pub sudo_nopassword: bool,
    pub groups: Vec<String>,
}

impl Account {
    fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("login".into(), self.login.clone().into());
        m.insert("uid".into(), Value::from(self.uid as i64));
        m.insert("full_name".into(), self.full_name.clone().into());
        m.insert("home".into(), self.home.clone().into());
        m.insert("shell".into(), self.shell.clone().into());
        m.insert("admin".into(), self.admin.into());
        m.insert("protected".into(), self.protected.into());
        m.insert("can_login".into(), self.can_login.into());
        m.insert("sudo_nopassword".into(), self.sudo_nopassword.into());
        m.insert("groups".into(), Value::from(self.groups.clone()));
        Value::Object(m)
    }
}

fn fail(msg: &str, code: i32) -> R<i32> {
    eprintln!("account: {msg}");
    Ok(code)
}

fn protected_patterns(env: &Env) -> R<Vec<String>> {
    let Some(text) = py::read_text(&env.protected_file)? else { return Ok(vec![]) };
    Ok(py::lines(&text)
        .into_iter()
        .map(|l| pycompat::strip(l.split('#').next().unwrap_or("")).to_string())
        .filter(|p| !p.is_empty())
        .collect())
}

/// An account the operating system keeps for itself.
fn protected(env: &Env, name: &str) -> R<bool> {
    for p in protected_patterns(env)? {
        if let Some(prefix) = p.strip_suffix('*') {
            if name.starts_with(prefix) {
                return Ok(true);
            }
        }
        if name == p {
            return Ok(true);
        }
    }
    Ok(false)
}

fn nopasswd_file(env: &Env, name: &str) -> String {
    py::join(&env.sudoers_d, &format!("60-yggdrasil-nopasswd-{name}"))
}

/// The members of a group (from the first line with its name and at least four fields).
fn group_members(env: &Env, name: &str) -> R<Vec<String>> {
    let Some(text) = py::read_text(&env.group)? else { return Ok(vec![]) };
    for line in py::lines(&text) {
        let p: Vec<&str> = line.strip_suffix('\n').unwrap_or(line).split(':').collect();
        if p[0] == name && p.len() >= 4 {
            return Ok(p[3].split(',').filter(|x| !x.is_empty()).map(str::to_string).collect());
        }
    }
    Ok(vec![])
}

/// The names of `names` that are groups on this system.
fn existing_groups(env: &Env, names: &[&str]) -> R<Vec<String>> {
    let have: Vec<String> = match py::read_text(&env.group)? {
        Some(text) => py::lines(&text).into_iter().map(|l| l.split(':').next().unwrap_or("").to_string()).collect(),
        None => vec![],
    };
    Ok(names.iter().filter(|g| have.iter().any(|h| h == *g)).map(|g| g.to_string()).collect())
}

/// Every login account (uid 1000 to 59999) with its access.
fn accounts_in(env: &Env) -> R<Vec<Account>> {
    let mut out = vec![];
    if let Some(text) = py::read_text(&env.passwd)? {
        for line in py::lines(&text) {
            let p: Vec<&str> = line.strip_suffix('\n').unwrap_or(line).split(':').collect();
            if p.len() < 7 || !pycompat::isdigit(p[2]) {
                continue;
            }
            // isdigit() takes a superscript two, int() does not: the script stopped there
            let uid = pycompat::int(p[2]).ok_or_else(|| Crash(format!("ValueError: invalid literal for int() with base 10: {}", pycompat::repr(p[2]))))?;
            if !(1000..60000).contains(&uid) {
                continue;
            }
            out.push(Account {
                login: p[0].into(),
                uid,
                full_name: p[4].split(',').next().unwrap_or("").into(),
                home: p[5].into(),
                shell: p[6].into(),
                admin: false,
                protected: false,
                can_login: true,
                sudo_nopassword: false,
                groups: vec![],
            });
        }
    }
    let admins = group_members(env, "wheel")?;
    let names: Vec<&str> = OPTIONAL_GROUPS.iter().map(|g| g.0).collect();
    let mut optional = vec![];
    for g in existing_groups(env, &names)? {
        let members = group_members(env, &g)?;
        optional.push((g, members));
    }
    for a in out.iter_mut() {
        a.admin = admins.contains(&a.login);
        a.protected = protected(env, &a.login)?;
        a.can_login = !(a.shell.ends_with("nologin") || a.shell.ends_with("/false"));
        // the folder can be listed by anyone, the rules in it read only by root: our own file tells
        a.sudo_nopassword = std::path::Path::new(&nopasswd_file(env, &a.login)).exists();
        let mut gs: Vec<String> = optional.iter().filter(|(_, m)| m.contains(&a.login)).map(|(g, _)| g.clone()).collect();
        gs.sort();
        a.groups = gs;
    }
    Ok(out)
}

/// The optional groups this system has, with what each allows.
fn optional_groups(env: &Env) -> R<Vec<(String, String)>> {
    let names: Vec<&str> = OPTIONAL_GROUPS.iter().map(|g| g.0).collect();
    Ok(existing_groups(env, &names)?.into_iter().map(|g| {
        let about = OPTIONAL_GROUPS.iter().find(|x| x.0 == g).unwrap().1.to_string();
        (g, about)
    }).collect())
}

/// PKEXEC_UID (else SUDO_UID) when it is a number.
fn caller_uid() -> R<Option<i128>> {
    let v = [std::env::var("PKEXEC_UID").ok(), std::env::var("SUDO_UID").ok()].into_iter().flatten().find(|v| !v.is_empty());
    match v {
        Some(v) if pycompat::isdigit(&v) => {
            pycompat::int(&v).map(Some).ok_or_else(|| Crash(format!("ValueError: invalid literal for int() with base 10: {}", pycompat::repr(&v))))
        }
        _ => Ok(None),
    }
}

fn logged_in(name: &str) -> R<bool> {
    let r = py::run(&["loginctl", "list-sessions", "--no-legend"], None)?;
    Ok(r.stdout.lines().any(|l| {
        let w = pycompat::split(l);
        w.len() >= 3 && w[2] == name
    }))
}

fn need_root(env: &Env) -> bool {
    if !env.test && unsafe { libc::geteuid() } != 0 {
        eprintln!("account: this needs root (run it with pkexec)");
        return true;
    }
    false
}

/// ^[a-z][a-z0-9_-]{0,30}$ (without the final newline Python's $ let through)
fn name_pattern(name: &str) -> bool {
    let b = name.as_bytes();
    !b.is_empty() && b.len() <= 31 && b[0].is_ascii_lowercase() && b[1..].iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

fn valid_name(env: &Env, name: &str) -> R<bool> {
    Ok(name_pattern(name) && !RESERVED.contains(&name) && !protected(env, name)?)
}

/// re.sub(r"[:,\n\r\x00-\x1f]", " ", text).strip()[:60]
pub fn clean_full_name(text: &str) -> String {
    let replaced: String = text.chars().map(|c| if c == ':' || c == ',' || (c as u32) < 0x20 { ' ' } else { c }).collect();
    py::take_chars(pycompat::strip(&replaced), 60)
}

fn find<'a>(accts: &'a [Account], name: &str) -> Option<&'a Account> {
    accts.iter().find(|a| a.login == name)
}

/// The account, if Settings may change it (uid 1000 up, not kept by the operating system).
fn changeable(env: &Env, name: &str) -> R<Option<Account>> {
    if protected(env, name)? {
        return Ok(None);
    }
    Ok(find(&accounts_in(env)?, name).cloned())
}

fn err_or(r: &py::Output, default: &str) -> String {
    let e = pycompat::strip(&r.stderr);
    if e.is_empty() { default.to_string() } else { e.to_string() }
}

fn read_password() -> R<String> {
    let line = py::read_stdin_line()?;
    Ok(line.trim_end_matches('\n').to_string())
}

fn cmd_add(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.is_empty() || !valid_name(env, &argv[0])? {
        return fail("the user name must be 1 to 31 lowercase letters, digits, - or _, starting with a letter, and not a system or reserved name", 2);
    }
    let name = &argv[0];
    let admin = argv.iter().any(|a| a == "--admin");
    let mut full = String::new();
    if let Some(i) = argv.iter().position(|a| a == "--full-name") {
        full = argv.get(i + 1).cloned().unwrap_or_default();
    }
    let full = clean_full_name(&full);
    if find(&accounts_in(env)?, name).is_some() || !pycompat::strip(&py::run(&["getent", "passwd", name.as_str()], None)?.stdout).is_empty() {
        return fail(&format!("an account called {name} already exists"), 1);
    }
    let pw = read_password()?;
    if pw.chars().count() < 8 {
        return fail("the password must be at least 8 characters", 2);
    }
    let mut want: Vec<&str> = DESKTOP_GROUPS.to_vec();
    if admin {
        want.push("wheel");
    }
    let groups = existing_groups(env, &want)?;
    let comment = if full.is_empty() { name.clone() } else { full };
    let mut cmd: Vec<String> = vec!["useradd".into(), "-m".into(), "-s".into(), "/bin/bash".into(), "-c".into(), comment];
    if !groups.is_empty() {
        cmd.push("-G".into());
        cmd.push(groups.join(","));
    }
    cmd.push(name.clone());
    let r = py::run(&cmd, None)?;
    if r.code != 0 {
        return fail(&format!("could not create the account: {}", err_or(&r, "useradd failed")), 1);
    }
    let r = py::run(&["chpasswd"], Some(&format!("{name}:{pw}\n")))?;
    if r.code != 0 {
        py::run(&["userdel", "-r", name.as_str()], None)?;
        return fail(&format!("could not set the password (the account was not kept): {}", err_or(&r, "chpasswd failed")), 1);
    }
    println!("created {name}");
    Ok(0)
}

fn admins_of(env: &Env, accts: &[Account], need_login: bool) -> R<usize> {
    let mut n = 0;
    for a in accts {
        if a.admin && (!need_login || a.can_login) && !protected(env, &a.login)? {
            n += 1;
        }
    }
    Ok(n)
}

fn unlink_if_there(path: &str) -> R<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(Crash(py::oserror(&e, Some(path)))),
    }
}

fn cmd_admin(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.len() != 2 || (argv[1] != "on" && argv[1] != "off") {
        return fail("usage: admin NAME on|off", 2);
    }
    let (name, on) = (&argv[0], argv[1] == "on");
    let accts = accounts_in(env)?;
    let target = find(&accts, name).cloned();
    let Some(target) = target else { return fail("no such account to change", 1) };
    if protected(env, name)? {
        return fail("no such account to change", 1);
    }
    if !on {
        let admins = admins_of(env, &accts, false)?;
        let cu = caller_uid()?;
        if Some(name.as_str()) == accts.iter().find(|a| Some(a.uid) == cu).map(|a| a.login.as_str()) {
            return fail("you cannot remove your own administrator rights", 1);
        }
        if target.admin && admins <= 1 {
            return fail("this is the last administrator; make another one first", 1);
        }
    }
    let r = py::run(&["gpasswd", if on { "-a" } else { "-d" }, name.as_str(), "wheel"], None)?;
    if r.code != 0 && !(!on && r.stderr.contains("not a member")) {
        return fail(&err_or(&r, "gpasswd failed"), 1);
    }
    if !on {
        // a standard user has no sudo at all: a "no password" rule of theirs goes too
        unlink_if_there(&nopasswd_file(env, name))?;
    }
    println!("{name} is now {}", if on { "an administrator" } else { "a standard user" });
    Ok(0)
}

fn cmd_remove(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.is_empty() {
        return fail("usage: remove NAME [--delete-home]", 2);
    }
    let name = &argv[0];
    let delete_home = argv.iter().any(|a| a == "--delete-home");
    let accts = accounts_in(env)?;
    let Some(target) = find(&accts, name).cloned() else {
        return fail("no such account (only accounts from uid 1000 up can be removed)", 1);
    };
    if protected(env, name)? {
        return fail(&format!("{name} is kept by the operating system: Settings does not change it"), 1);
    }
    if Some(target.uid) == caller_uid()? {
        return fail("you cannot remove the account you are using", 1);
    }
    if target.admin && admins_of(env, &accts, false)? <= 1 {
        return fail("this is the last administrator", 1);
    }
    if logged_in(name)? {
        return fail(&format!("{name} is logged in: log them out first"), 1);
    }
    let mut cmd = vec!["userdel".to_string()];
    if delete_home {
        cmd.push("-r".into());
    }
    cmd.push(name.clone());
    let r = py::run(&cmd, None)?;
    if r.code != 0 {
        return fail(&err_or(&r, "userdel failed"), 1);
    }
    println!("removed {name}{}", if delete_home { " and their home folder" } else { " (their home folder was kept)" });
    Ok(0)
}

fn cmd_name(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.len() != 2 || changeable(env, &argv[0])?.is_none() {
        return fail("usage: name NAME TEXT (an account Settings may change)", 2);
    }
    let full = clean_full_name(&argv[1]);
    let comment = if full.is_empty() { argv[0].clone() } else { full };
    let r = py::run(&["usermod", "-c", comment.as_str(), argv[0].as_str()], None)?;
    if r.code != 0 {
        return fail(&err_or(&r, "usermod failed"), 1);
    }
    println!("renamed {}", argv[0]);
    Ok(0)
}

fn cmd_password(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.len() != 1 || changeable(env, &argv[0])?.is_none() {
        return fail("usage: password NAME (an account Settings may change)", 2);
    }
    let Some(target) = changeable(env, &argv[0])? else { return Err(Crash("TypeError: 'NoneType' object is not subscriptable".into())) };
    if Some(target.uid) == caller_uid()? {
        return fail("change your own password with the password form (it keeps your keyring in step)", 1);
    }
    let pw = read_password()?;
    if pw.chars().count() < 8 {
        return fail("the password must be at least 8 characters", 2);
    }
    let r = py::run(&["chpasswd"], Some(&format!("{}:{pw}\n", argv[0])))?;
    if r.code != 0 {
        return fail(&err_or(&r, "chpasswd failed"), 1);
    }
    println!("new password set for {}", argv[0]);
    Ok(0)
}

fn cmd_login(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.len() != 2 || (argv[1] != "on" && argv[1] != "off") {
        return fail("usage: login NAME on|off", 2);
    }
    let (name, on) = (&argv[0], argv[1] == "on");
    let Some(target) = changeable(env, name)? else { return fail("no such account to change", 1) };
    if !on {
        if Some(target.uid) == caller_uid()? {
            return fail("you cannot stop your own account from signing in", 1);
        }
        let accts = accounts_in(env)?;
        if target.admin && admins_of(env, &accts, true)? <= 1 {
            return fail("this is the last administrator who can sign in", 1);
        }
        if logged_in(name)? {
            return fail(&format!("{name} is logged in: log them out first"), 1);
        }
    }
    let r = py::run(&["usermod", "-s", if on { "/bin/bash" } else { NOLOGIN }, if on { "-U" } else { "-L" }, name.as_str()], None)?;
    if r.code != 0 {
        return fail(&err_or(&r, "usermod failed"), 1);
    }
    println!("{name} {}", if on { "can sign in again" } else { "can no longer sign in" });
    Ok(0)
}

fn cmd_sudo(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.len() != 2 || (argv[1] != "password" && argv[1] != "nopassword") {
        return fail("usage: sudo NAME password|nopassword", 2);
    }
    let (name, ask) = (&argv[0], argv[1] == "password");
    let Some(target) = changeable(env, name)? else { return fail("no such account to change", 1) };
    let path = nopasswd_file(env, name);
    if ask {
        unlink_if_there(&path)?;
        println!("sudo asks {name} for the password");
        return Ok(0);
    }
    if !target.admin {
        return fail(&format!("{name} is not an administrator: there is no sudo to change"), 1);
    }
    let tmp = format!("{path}.new");
    let rule = format!("# Written by Settings > Users: sudo does not ask {name} for the password.\n{name} ALL=(ALL:ALL) NOPASSWD: ALL\n");
    fs::write(&tmp, rule).map_err(|e| Crash(py::oserror(&e, Some(&tmp))))?;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o440)).map_err(|e| Crash(py::oserror(&e, Some(&tmp))))?;
    if !env.test {
        let r = py::run(&["visudo", "-cf", tmp.as_str()], None)?;
        if r.code != 0 {
            fs::remove_file(&tmp).map_err(|e| Crash(py::oserror(&e, Some(&tmp))))?;
            let why = if pycompat::strip(&r.stderr).is_empty() { pycompat::strip(&r.stdout).to_string() } else { pycompat::strip(&r.stderr).to_string() };
            return fail(&format!("the rule did not pass sudo's check: {why}"), 1);
        }
    }
    fs::rename(&tmp, &path).map_err(|e| Crash(py::oserror(&e, Some(&tmp))))?;
    println!("sudo no longer asks {name} for the password");
    Ok(0)
}

fn cmd_group(env: &Env, argv: &[String]) -> R<i32> {
    if need_root(env) {
        return Ok(1);
    }
    if argv.len() != 3 || (argv[2] != "on" && argv[2] != "off") {
        return fail("usage: group NAME GROUP on|off", 2);
    }
    let (name, group, on) = (&argv[0], &argv[1], argv[2] == "on");
    if !optional_groups(env)?.iter().any(|(g, _)| g == group) {
        return fail(&format!("{group} is not one of the groups Settings changes"), 2);
    }
    if changeable(env, name)?.is_none() {
        return fail("no such account to change", 1);
    }
    let r = py::run(&["gpasswd", if on { "-a" } else { "-d" }, name.as_str(), group.as_str()], None)?;
    if r.code != 0 && !(!on && r.stderr.contains("not a member")) {
        return fail(&err_or(&r, "gpasswd failed"), 1);
    }
    println!("{name} {} {group}", if on { "joined" } else { "left" });
    Ok(0)
}

fn dispatch(argv: &[String]) -> R<i32> {
    if argv.len() < 2 || argv[1] == "-h" || argv[1] == "--help" {
        println!("{DOC}");
        return Ok(if argv.len() >= 2 { 0 } else { 2 });
    }
    let env = Env::get();
    let (c, rest) = (argv[1].as_str(), &argv[2..]);
    match c {
        "list" => {
            let v: Vec<Value> = accounts_in(&env)?.iter().map(Account::to_json).collect();
            println!("{}", json::dumps(&Value::from(v)));
            Ok(0)
        }
        "groups" => {
            let mut m = Map::new();
            for (g, about) in optional_groups(&env)? {
                m.insert(g, about.into());
            }
            println!("{}", json::dumps(&Value::Object(m)));
            Ok(0)
        }
        "name" => cmd_name(&env, rest),
        "password" => cmd_password(&env, rest),
        "login" => cmd_login(&env, rest),
        "sudo" => cmd_sudo(&env, rest),
        "group" => cmd_group(&env, rest),
        "add" => cmd_add(&env, rest),
        "admin" => cmd_admin(&env, rest),
        "remove" => cmd_remove(&env, rest),
        _ => fail("unknown command (see --help)", 2),
    }
}

/// The helper: `argv` with the program's name first, as sys.argv.
pub fn main(argv: &[String]) -> i32 {
    match dispatch(argv) {
        Ok(code) => code,
        Err(c) => py::crashed("account", &c),
    }
}

// the tests run the helper as Settings does, with stand-in tools: tests/account.rs
