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
//! One check is stricter than the script's: its patterns ended in `$`, which in Python also matches before a final newline,
//! so a package name or --ignore list ending in a newline passed them. Here a newline is never part of either.

use std::collections::BTreeSet;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ALLOWED_OPTS: &[&str] = &["-S", "-U", "-R", "-Rs", "-Rns", "-D", "-s", "-n", "-q", "-y", "-Syu", "--needed", "--noconfirm", "--asdeps", "--asexplicit"];
const OPS: &[&str] = &["-S", "-U", "-R", "-Rs", "-Rns", "-Syu", "-D"];
/// yay passes pacman's own settings file explicitly; no other one is accepted (another file could point pacman anywhere)
const CONFIG: &str = "/etc/pacman.conf";

/// Where pacman keeps the files it downloads (fixed: a root helper takes no path from its environment; the tests put a
/// scratch folder there in a mount namespace of their own).
const CACHE: &str = "/var/cache/pacman/pkg";

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
        return refuse("usage: pkg-root CANCELFILE pacman OPTIONS... PACKAGES...");
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
    // start_new_session=True: pacman and what it starts are a group of their own, stopped together
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = c.spawn().map_err(|e| format!("pacman: {e}"))?;
    let pgid = child.id() as i32;
    let mut cancelled = false;
    let status;
    loop {
        if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
            status = s;
            break;
        }
        if Path::new(cancel_file).exists() {
            cancelled = true;
            unsafe { libc::killpg(pgid, libc::SIGTERM) };
            let until = Instant::now() + Duration::from_secs(15);
            loop {
                if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                    status = s;
                    break;
                }
                if Instant::now() >= until {
                    unsafe { libc::killpg(pgid, libc::SIGKILL) };
                    status = child.wait().map_err(|e| e.to_string())?;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            break;
        }
        std::thread::sleep(Duration::from_millis(250));
    }

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
