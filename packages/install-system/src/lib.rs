//! yggdrasil-install-system: installs the operating system from its live medium (archiso) onto a disk. Yggdrasil's installer
//! window runs it as root (pkexec) and shows what it says; it can also be run by hand. Nothing is asked here: the choices come
//! as one JSON object on standard input, so passwords never appear on a command line.
//!
//! ```text
//! {"disk": "/dev/nvme0n1", "mode": "erase",
//!  "encrypt": true, "encrypt_password": "...",          (LUKS2; one password at start-up)
//!  "user": {"name": "Full Name", "login": "me", "password": "..."},
//!  "hostname": "my-pc", "timezone": "Europe/Berlin", "locale": "en_US.UTF-8", "keymap": "us", "keymap_variant": "",
//!  "browsers": ["vivaldi", "librewolf"], "default_browser": "vivaldi",      (optional; ids from the OS's browser list)
//!  "extras": ["heimdall"]}                                                  (optional; ids from the OS's optional parts)
//!
//! Partitions chosen by hand instead of the whole disk ("mode": "manual"; only on the chosen disk):
//! {"disk": "/dev/nvme0n1", "mode": "manual", "new_table": false,
//!  "delete": [4],                                       (partition numbers, removed first)
//!  "create": [{"id": "a", "start": 512110190592, "size": 214748364800, "kind": "linux"}],   (bytes; kind linux|efi|swap)
//!  "use": [{"part": "/dev/nvme0n1p1", "mount": "/boot/efi", "format": null},                  (an existing EFI partition, kept)
//!          {"part": "new:a", "mount": "/", "format": "btrfs"}],
//!  "encrypt": true, "encrypt_password": "...", ...}    (encryption is for / only)
//!
//! yggdrasil-install-system --check < plan.json      only check the plan, change nothing
//! yggdrasil-install-system --disks                  the disks it could install onto (JSON), with their partitions and free space
//! yggdrasil-install-system --prefetch < choices      fetch beforehand what the install will download (src/prefetch.rs):
//!                                                    {"browsers": [...], "extras": [...]} on one line; stops when stdin closes
//! ```
//!
//! What it prints, one line each (the installer reads these): `STEP n N text` (step n of N has started), `INFO text`
//! (something worth knowing), `WARN text` (something went wrong but the install goes on), `ERROR text` (the install stopped;
//! nothing after this), `DONE` (finished: the computer can restart into the new system). Exit codes: 0 done, 2 a plan that is
//! refused (nothing was done), 1 the install stopped (or it was not run as root).
//!
//! The steps, what belongs to the operating system (its installer integration file and /etc/os-release) and the contract
//! with the window are described in docs/INSTALLER.md. This is the Rust port of the Python script kept in
//! packages/installer/helpers/python/install-system (for a rollback): the same plan, checks and messages, the same programs
//! with the same arguments in the same order, the same files and exit codes (parity/compare.py runs both).

pub mod catalog;
pub mod conf;
pub mod disks;
pub mod install;
pub mod plan;
pub mod prefetch;
pub mod py;
pub mod pyjson;
pub mod sys;

use std::io::Read;

use install::{Ctx, Host};
use py::Fail;
use pyjson::{loads, Py, PLAN_NESTING};
use sys::System;

/// YGGDRASIL_INSTALL_SFS and YGGDRASIL_INSTALL_TARGET (the system to copy and where it goes), when set.
#[derive(Clone, Debug, Default)]
pub struct Env {
    pub sfs: Option<String>,
    pub target: Option<String>,
}

impl Env {
    pub fn from_process() -> Env {
        let var = |k: &str| std::env::var_os(k).map(|v| v.to_string_lossy().into_owned());
        Env { sfs: var("YGGDRASIL_INSTALL_SFS"), target: var("YGGDRASIL_INSTALL_TARGET") }
    }
}

const PROG: &str = "yggdrasil-install-system";

/// Where the script stopped with a traceback: one line on standard error, exit code 1.
fn traceback(sys: &mut dyn System, e: Fail) -> i32 {
    let m = match e {
        Fail::Stop(m) | Fail::Crash(m) => m,
    };
    sys.err(&format!("{PROG}: unexpected error (the Python script stopped here with a traceback): {m}"));
    1
}

/// The plan from standard input: Err(()) when it is not JSON (not UTF-8, not JSON as json.loads reads it, a lone surrogate,
/// nested deeper than a plan is).
pub fn read_plan(bytes: &[u8]) -> Result<Py, pyjson::NotJson> {
    let text = std::str::from_utf8(bytes).map_err(|_| pyjson::NotJson)?;
    loads(if text.is_empty() { "{}" } else { text }, PLAN_NESTING)
}

/// `--prefetch`: the choices on the first line of `stdin`; the rest of it is watched on a thread of its own, and its end (the
/// window closing it, or going away) stops the fetching between two packages. Returns the exit code.
pub fn prefetch_main(stdin: Box<dyn std::io::BufRead + Send>, env: &Env, host: Host, sys: &mut dyn System) -> i32 {
    use std::sync::atomic::{AtomicBool, Ordering};
    let mut stdin = stdin;
    let mut line = String::new();
    let _ = stdin.read_line(&mut line);
    let stopped = std::sync::Arc::new(AtomicBool::new(false));
    let flag = stopped.clone();
    std::thread::spawn(move || {
        let mut rest = vec![];
        let _ = stdin.read_to_end(&mut rest);
        flag.store(true, Ordering::SeqCst);
    });
    prefetch_with(&line, env, host, sys, &|| stopped.load(Ordering::SeqCst))
}

/// `--prefetch` with the choices' line in hand and `stop` saying when to stop.
pub fn prefetch_with(line: &str, env: &Env, host: Host, sys: &mut dyn System, stop: &dyn Fn() -> bool) -> i32 {
    let conf = conf::installer_integration(&host.integration_dirs.iter().map(String::as_str).collect::<Vec<_>>());
    let release = conf::os_release(&host.os_release);
    let os = match conf::configure(&conf, &release, env.sfs.as_deref(), env.target.as_deref()) {
        Ok(os) => os,
        Err(e) => return traceback(sys, e),
    };
    let dir = host.prefetch.clone();
    let mut ctx = Ctx { os, host, sys, bound: false };
    let Ok(choices) = read_plan(line.trim().as_bytes()) else {
        return match ctx.say("ERROR", "the choices are not JSON") {
            Ok(()) => 2,
            Err(e) => traceback(ctx.sys, e),
        };
    };
    match ctx.prefetch(&choices, &dir, stop) {
        Ok(code) => code,
        Err(e) => traceback(ctx.sys, e),
    }
}

/// The program: its arguments (without its own name), standard input, environment, the live system's places and the
/// computer. Returns the exit code.
pub fn main_with(args: &[String], stdin: &mut dyn Read, env: &Env, host: Host, sys: &mut dyn System) -> i32 {
    let conf = conf::installer_integration(&host.integration_dirs.iter().map(String::as_str).collect::<Vec<_>>());
    let release = conf::os_release(&host.os_release);
    let os = match conf::configure(&conf, &release, env.sfs.as_deref(), env.target.as_deref()) {
        Ok(os) => os,
        Err(e) => return traceback(sys, e),
    };
    let mut ctx = Ctx { os, host, sys, bound: false };
    let first = args.first().map(String::as_str);
    if first == Some("--disks") {
        return match ctx.disks() {
            Ok(found) => match ctx.sys.out(&Py::List(found.iter().map(|d| d.to_py()).collect()).dumps()) {
                Ok(()) => 0,
                Err(e) => traceback(ctx.sys, e),
            },
            Err(e) => traceback(ctx.sys, e),
        };
    }
    let mut bytes = vec![];
    let _ = stdin.read_to_end(&mut bytes);
    let say = |ctx: &mut Ctx, kind: &str, text: &str, code: i32| match ctx.say(kind, text) {
        Ok(()) => code,
        Err(e) => traceback(ctx.sys, e),
    };
    let Ok(plan) = read_plan(&bytes) else {
        return say(&mut ctx, "ERROR", "the plan is not JSON", 2);
    };
    let problem = {
        let Ctx { os, host, sys, .. } = &mut ctx;
        let mut exists = |login: &str| sys.account_exists(login);
        plan::check_plan(&plan, os, &mut plan::Ask { zoneinfo: &host.zoneinfo, account_exists: &mut exists })
    };
    if let Some(problem) = problem {
        return say(&mut ctx, "ERROR", &problem, 2);
    }
    if first == Some("--check") {
        return say(&mut ctx, "DONE", "", 0);
    }
    if ctx.sys.euid() != 0 {
        return say(&mut ctx, "ERROR", &format!("{PROG} needs root"), 1);
    }
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.install(&plan)));
    let message = match outcome {
        Ok(Ok(())) => return 0,
        Ok(Err(Fail::Stop(m))) => m,
        Ok(Err(Fail::Crash(m))) => format!("unexpected: {m}"),
        Err(panic) => format!(
            "unexpected: {}",
            panic.downcast_ref::<String>().cloned().or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default()
        ),
    };
    if let Err(e) = ctx.say("ERROR", &message) {
        return traceback(ctx.sys, e);
    }
    if let Err(e) = ctx.unmount_all() {
        return traceback(ctx.sys, e);
    }
    1
}
