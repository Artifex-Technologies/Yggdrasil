//! pkg-root: run one pacman command with administrator rights for Agora (the app store), so it can be cancelled and cleaned
//! up. The Python script's rules, line for line:
//!
//!   pkexec /usr/lib/agora/pkg-root CANCELFILE pacman OPTIONS... PACKAGES...
//!
//! Only pacman, and only install (-S, -U), remove (-R, -Rs, -Rns), mark (-D with --asdeps or --asexplicit) and a refresh that
//! is always a full upgrade (-S -y becomes -Syu) are accepted, with a short list of plain options; --config may only name
//! /etc/pacman.conf, --ignore only a plain list of names, -U only real package files. The cancel file must be in the invoking
//! user's runtime folder (/run/user/PKEXEC_UID/agora/). While pacman runs (in a session of its own) the file is looked for
//! every 0.25 s: on cancel pacman is stopped (SIGTERM to its group, SIGKILL after 15 s), the packages this run installed are
//! removed, the package files it downloaded deleted, and the exit code is 130. Nothing else is changed.
//!
//!   pkexec /usr/lib/agora/pkg-root CANCELFILE component setup|teardown ID...
//!
//! runs the operating system's component command (its `packages` integration's `component`, Yggdrasil's docs/INTEGRATIONS.md)
//! as `COMMAND setup ID...` (a component's root steps after its packages: setup commands, groups, services) or `teardown ID...`
//! (before they are removed), so a whole install asks for the password once. Only the verb and plain ids come from the caller:
//! the program is the integration's, and both the integration file and the program must be root's and writable by no one else.
//! It runs with nothing of the caller's environment but SUDO_USER (the invoking user), no standard input, in a session of its
//! own; a cancel stops it (exit code 130), with nothing to undo.
//!
//! One check is stricter than the script's: its patterns ended in `$`, which in Python also matches before a final newline,
//! so a package name or --ignore list ending in a newline passed them. Here a newline is never part of either.

use std::collections::BTreeSet;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ALLOWED_OPTS: &[&str] =
    &["-S", "-U", "-R", "-Rs", "-Rns", "-D", "-s", "-n", "-q", "-y", "-Syu", "--needed", "--noconfirm", "--asdeps", "--asexplicit"];
const OPS: &[&str] = &["-S", "-U", "-R", "-Rs", "-Rns", "-Syu", "-D"];
/// yay passes pacman's own settings file explicitly; no other one is accepted (another file could point pacman anywhere)
const CONFIG: &str = "/etc/pacman.conf";

/// Where pacman keeps the files it downloads (fixed: a root helper takes no path from its environment; the tests put a
/// scratch folder there in a mount namespace of their own).
const CACHE: &str = "/var/cache/pacman/pkg";

/// Where an operating system declares its integrations: its own, then the administrator's (the same file name wins). Fixed, as
/// everything a root helper reads.
pub const INTEGRATIONS: [&str; 2] = ["/usr/share/yggdrasil/integrations", "/etc/yggdrasil/integrations"];

/// The search path a component's setup runs with.
const SETUP_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/bin";

/// The refusal: "agora pkg-root: ..." on standard error, exit code 2.
#[derive(Debug, PartialEq)]
pub struct Refused(pub String);

/// ^[A-Za-z0-9@._+,-]+$ (an --ignore list)
pub fn plain_list(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._+,-".contains(&b))
}

/// ^[A-Za-z0-9@._+/:~-]+$ (a package name, or a package file's path)
pub fn plain_name(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b"@._+/:~-".contains(&b))
}

/// \.pkg\.tar\.(zst|xz|gz)$
fn package_file_name(p: &str) -> bool {
    p.ends_with(".pkg.tar.zst") || p.ends_with(".pkg.tar.xz") || p.ends_with(".pkg.tar.gz")
}

/// The cancel file is in the invoking user's runtime folder: /run/user/PKEXEC_UID/agora/..., with no ".." anywhere.
pub fn valid_cancel_file(path: &str, pkexec_uid: &str) -> bool {
    if !pycompat::isdigit(pkexec_uid) {
        return false;
    }
    let base = format!("/run/user/{pkexec_uid}/agora/");
    pycompat::normpath(path).starts_with(&base) && !path.contains("..")
}

/// What will run: pacman's arguments (with -S -y made -Syu), and whether it installs (then a cancel removes what it added).
#[derive(Debug, PartialEq)]
pub struct Plan {
    pub args: Vec<String>,
    pub installing: bool,
}

/// The script's checks, in its order. `argv` is everything after the program's name.
pub fn plan(argv: &[String], pkexec_uid: &str, is_file: &dyn Fn(&str) -> bool) -> Result<Plan, Refused> {
    let refuse = |m: &str| Err(Refused(m.to_string()));
    if argv.len() < 4 || argv[1] != "pacman" {
        return refuse("usage: pkg-root CANCELFILE pacman OPTIONS... PACKAGES... (or CANCELFILE component setup|teardown ID...)");
    }
    let (cancel_file, mut args) = (&argv[0], argv[2..].to_vec());
    if !valid_cancel_file(cancel_file, pkexec_uid) {
        return refuse("the cancel file must be in the invoking user's runtime folder");
    }
    let mut packages: Vec<String> = vec![];
    let mut options: Vec<String> = vec![];
    let mut skip = false;
    let mut only_names = false;
    for (i, a) in args.iter().enumerate() {
        if skip {
            skip = false;
            continue;
        }
        if only_names {
        } else if a == "--" {
            // the end of the options: everything after it is a package
            only_names = true;
            continue;
        } else if a == "--config" {
            if i + 1 >= args.len() || args[i + 1] != CONFIG {
                return refuse(&format!("--config may only name {CONFIG}"));
            }
            skip = true;
            continue;
        }
        if !only_names && a == "--ignore" {
            // --ignore takes the next argument: a comma-separated list of plain package names
            if i + 1 >= args.len() || !plain_list(&args[i + 1]) {
                return refuse("--ignore needs a plain list of package names");
            }
            skip = true;
        } else if !only_names && a.starts_with('-') {
            if !ALLOWED_OPTS.contains(&a.as_str()) {
                return refuse(&format!("option not allowed: {a}"));
            }
            options.push(a.clone());
        } else {
            if !plain_name(a) {
                return refuse(&format!("odd package name: {a}"));
            }
            packages.push(a.clone());
        }
    }
    let mut ops: Vec<&str> = options.iter().map(String::as_str).filter(|a| OPS.contains(a)).collect();
    if ops.len() != 1 {
        return refuse("one install, remove or mark command at a time");
    }
    if options.iter().any(|o| o == "-y") {
        // a refresh of the package lists (yay's first step of an update). Refreshing without upgrading leaves the system half
        // updated, where the next single install can break it, so a refresh is always a full upgrade of the repositories here.
        if ops != ["-S"] || !packages.is_empty() {
            return refuse("-y only refreshes, as the first step of a full update");
        }
        args = args.into_iter().filter(|a| a != "-y").map(|a| if a == "-S" { "-Syu".to_string() } else { a }).collect();
        ops = vec!["-Syu"];
    }
    if ops.contains(&"-D") && !options.iter().any(|o| o == "--asdeps" || o == "--asexplicit") {
        return refuse("-D only marks packages as dependencies or explicitly installed");
    }
    if ops.contains(&"-U") {
        // package files only: what yay built (or a file you chose), never a web address or anything else
        for p in &packages {
            if !(p.starts_with('/') && package_file_name(p) && is_file(p)) {
                return refuse(&format!("not a package file: {p}"));
            }
        }
    }
    Ok(Plan { args, installing: ops[0] == "-S" || ops[0] == "-U" })
}

/// A component's root steps: `COMMAND setup|teardown ID...`.
#[derive(Debug, PartialEq)]
pub struct ComponentPlan {
    pub command: PathBuf,
    pub verb: String,
    pub ids: Vec<String>,
}

/// ^[a-z0-9][a-z0-9._+-]*$, at most 64 characters (a component's id)
fn plain_id(s: &str) -> bool {
    s.len() <= 64
        && s.bytes().next().is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
        && s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"._+-".contains(&b))
}

/// Owned by `owner` and writable by no one else.
fn owners_only(p: &Path, owner: u32) -> bool {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(p).is_ok_and(|m| m.uid() == owner && m.mode() & 0o022 == 0)
}

/// The file (links followed) and the folder it is in are `owner`'s, and no one else can change or replace it.
fn safe(p: &Path, owner: u32) -> bool {
    std::fs::canonicalize(p).is_ok_and(|real| owners_only(&real, owner) && real.parent().is_some_and(|d| owners_only(d, owner)))
}

/// The operating system's component command, as Agora finds it (the first `packages` integration's `component`): one absolute
/// path with nothing after it, and both the integration file and the program `owner`'s and safe from everyone else.
pub fn component_command(dirs: &[PathBuf], owner: u32) -> Result<PathBuf, String> {
    let all = yggdrasil_conf::integrations::all_in(dirs);
    let i = all.iter().find(|i| i.kind == "packages").ok_or("the system has no packages integration")?;
    let c = i.get("component").ok_or("the packages integration names no component command")?;
    if !safe(&i.file, owner) {
        return Err(format!("{} could be changed by someone else", i.file.display()));
    }
    if !c.starts_with('/') || c.split_whitespace().count() != 1 || c.split('/').any(|p| p == "..") {
        return Err(format!("the component command must be one absolute path: {c}"));
    }
    let p = PathBuf::from(c);
    if !safe(&p, owner) {
        return Err(format!("{c} could be changed by someone else"));
    }
    Ok(p)
}

/// `CANCELFILE component setup|teardown ID...`, checked: the cancel file as for pacman, plain ids, the integration's program.
pub fn component_plan(argv: &[String], pkexec_uid: &str, dirs: &[PathBuf], owner: u32) -> Result<ComponentPlan, Refused> {
    if argv.len() < 4 || argv[1] != "component" || !matches!(argv[2].as_str(), "setup" | "teardown") {
        return Err(Refused("usage: pkg-root CANCELFILE component setup|teardown ID...".into()));
    }
    if !valid_cancel_file(&argv[0], pkexec_uid) {
        return Err(Refused("the cancel file must be in the invoking user's runtime folder".into()));
    }
    if let Some(bad) = argv[3..].iter().find(|i| !plain_id(i)) {
        return Err(Refused(format!("odd component id: {bad:?}")));
    }
    let command = component_command(dirs, owner).map_err(Refused)?;
    Ok(ComponentPlan { command, verb: argv[2].clone(), ids: argv[3..].to_vec() })
}

/// The name of the user with this id (PKEXEC_UID), if there is one.
pub fn user_name(uid: &str) -> Option<String> {
    if !pycompat::isdigit(uid) {
        return None;
    }
    let uid: libc::uid_t = uid.parse().ok()?;
    // SAFETY: passwd is plain data; getpwuid_r writes into it and into buf (whose length it is given), and sets out to it or null
    let mut pw: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0 as libc::c_char; 4096];
    let mut out: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe { libc::getpwuid_r(uid, &mut pw, buf.as_mut_ptr(), buf.len(), &mut out) };
    if rc != 0 || out.is_null() || pw.pw_name.is_null() {
        return None;
    }
    // SAFETY: pw_name points into buf, a NUL-terminated string getpwuid_r wrote
    let name = unsafe { std::ffi::CStr::from_ptr(pw.pw_name) }.to_str().ok()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Runs a component's root steps; returns the exit code (130 when cancelled).
pub fn run_component(p: &ComponentPlan, cancel_file: &str, user: &str) -> Result<i32, String> {
    let mut c = Command::new(&p.command);
    c.arg(&p.verb).args(&p.ids).env_clear().env("PATH", SETUP_PATH).env("HOME", "/root").env("LANG", "C.UTF-8");
    c.env("SUDO_USER", user).stdin(Stdio::null());
    own_session(&mut c);
    let mut child = c.spawn().map_err(|e| format!("{}: {e}", p.command.display()))?;
    let (status, cancelled) = wait_or_cancel(&mut child, cancel_file)?;
    Ok(if cancelled { 130 } else { exit_of(status) })
}

/// start_new_session=True: the program and what it starts are a group of their own, stopped together.
fn own_session(c: &mut Command) {
    // SAFETY: setsid is async-signal-safe, as pre_exec requires
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
}

/// Waits for `child` (the leader of its own process group); when the cancel file appears, the group is stopped (SIGTERM, SIGKILL
/// after 15 s). The exit status, and whether it was cancelled.
fn wait_or_cancel(child: &mut Child, cancel_file: &str) -> Result<(ExitStatus, bool), String> {
    let pgid = child.id() as i32;
    loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            return Ok((s, false));
        }
        if Path::new(cancel_file).exists() {
            unsafe { libc::killpg(pgid, libc::SIGTERM) };
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                    return Ok((s, true));
                }
                if Instant::now() >= until {
                    unsafe { libc::killpg(pgid, libc::SIGKILL) };
                    return Ok((child.wait().map_err(|e| e.to_string())?, true));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// `pacman -Qq`: the installed packages.
fn installed() -> Result<BTreeSet<String>, String> {
    // its standard error is captured and dropped, as subprocess.run(capture_output=True) did
    let out = Command::new("pacman").arg("-Qq").stdin(Stdio::null()).output().map_err(|e| format!("pacman: {e}"))?;
    Ok(pycompat::split(&String::from_utf8_lossy(&out.stdout)).into_iter().map(str::to_string).collect())
}

fn now() -> f64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// The package files this run downloaded (changed from two seconds before it started on, ".part" or ".pkg.tar" in the name).
/// Stops at the first error, as the script did.
fn clean_cache(started: f64) {
    let dir = CACHE;
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd {
        let Ok(e) = e else { return };
        let name = e.file_name().to_string_lossy().into_owned();
        let p = Path::new(dir).join(&name);
        if !p.is_file() {
            continue;
        }
        let Ok(m) = std::fs::metadata(&p).and_then(|m| m.modified()) else { return };
        let mtime = m.duration_since(UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0);
        if mtime >= started - 2.0 && (name.ends_with(".part") || name.contains(".pkg.tar")) && std::fs::remove_file(&p).is_err() {
            return;
        }
    }
}

/// The exit code sys.exit(child.returncode) gave: the program's own, or for a signal (returncode -N) 256 - N.
fn exit_of(status: std::process::ExitStatus) -> i32 {
    match (status.code(), status.signal()) {
        (Some(c), _) => c,
        (None, Some(s)) => (256 - s) & 0xff,
        _ => 1,
    }
}

/// Runs the plan; returns the exit code.
pub fn run(plan: &Plan, cancel_file: &str) -> Result<i32, String> {
    let before = if plan.installing { installed()? } else { BTreeSet::new() };
    let started = now();
    let mut c = Command::new("pacman");
    c.args(&plan.args);
    own_session(&mut c);
    let mut child = c.spawn().map_err(|e| format!("pacman: {e}"))?;
    let (status, mut cancelled) = wait_or_cancel(&mut child, cancel_file)?;
    if !cancelled && Path::new(cancel_file).exists() && status.code() == Some(0) && plan.installing {
        // cancelled just as it finished: undo it all the same
        cancelled = true;
    }
    if !cancelled {
        return Ok(exit_of(status));
    }
    if plan.installing {
        let newly: Vec<String> = installed()?.difference(&before).cloned().collect();
        if !newly.is_empty() {
            use std::io::Write;
            println!("Removing what this install added: {}", newly.join(" "));
            let _ = std::io::stdout().flush();
            let _ = Command::new("pacman").args(["-Rns", "--noconfirm"]).args(&newly).status();
        }
        clean_cache(started);
    }
    Ok(130)
}

/// The helper: `argv` without the program's name.
pub fn main(argv: &[String]) -> i32 {
    let uid = std::env::var("PKEXEC_UID").unwrap_or_default();
    if argv.get(1).is_some_and(|a| a == "component") {
        let dirs: Vec<PathBuf> = INTEGRATIONS.iter().map(PathBuf::from).collect();
        let plan = component_plan(argv, &uid, &dirs, 0);
        return match plan.and_then(|p| user_name(&uid).map(|u| (p, u)).ok_or(Refused(format!("no user has the id {uid}")))) {
            Err(Refused(m)) => {
                eprintln!("agora pkg-root: {m}");
                2
            }
            Ok((p, user)) => run_component(&p, &argv[0], &user).unwrap_or_else(|e| {
                eprintln!("agora pkg-root: {e}");
                1
            }),
        };
    }
    match plan(argv, &uid, &|p| Path::new(p).is_file()) {
        Err(Refused(m)) => {
            eprintln!("agora pkg-root: {m}");
            2
        }
        Ok(p) => match run(&p, &argv[0]) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("agora pkg-root: unexpected error (the Python script stopped here with a traceback): {e}");
                1
            }
        },
    }
}

#[cfg(test)]
mod tests;
