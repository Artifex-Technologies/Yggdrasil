//! yggdrasil-gametime: background chores wait while a game runs and catch up afterwards (see lib.rs for how a game is found).
//!
//!   yggdrasil-gametime status [--json] [--proc DIR]   is a game running, and why; what is waiting. Exit 0: a game, 1: none
//!   yggdrasil-gametime no-game UNIT                    for ExecCondition=. Exit 0: no game, UNIT runs now. During a game UNIT
//!                                                      is noted and the exit is 100: systemd skips this run, not a failure
//!   yggdrasil-gametime catch-up [--system|--user]      after the game, starts what waited (a timer, every 5 minutes)
//!
//! The system manager's chores are noted in /run/yggdrasil-gametime, a user manager's in $XDG_RUNTIME_DIR/yggdrasil-gametime.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use yggdrasil_gametime::*;

/// ExecCondition='s "skip this run". Any of 1-254 skips, unless the unit lists the code in SuccessExitStatus=, and then the
/// chore runs after all: the weather units list 1. 100 is in nobody's list.
const SKIP: i32 = 100;

extern "C" {
    fn geteuid() -> u32;
}

fn euid() -> u32 {
    // SAFETY: geteuid has no preconditions and cannot fail
    unsafe { geteuid() }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    System,
    User,
}

/// --system or --user when given; otherwise a per-user manager (it sets $MANAGERPID) or an ordinary account is a user's,
/// and root is the system's.
fn scope(flags: &[String]) -> Scope {
    if flags.iter().any(|f| f == "--system") {
        Scope::System
    } else if flags.iter().any(|f| f == "--user") || std::env::var_os("MANAGERPID").is_some() || euid() != 0 {
        Scope::User
    } else {
        Scope::System
    }
}

fn user_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", euid())))
        .join("yggdrasil-gametime")
}

fn waitlist(scope: Scope) -> Waitlist {
    match scope {
        Scope::System => Waitlist::new(SYSTEM_DIR),
        Scope::User => Waitlist::new(user_dir()),
    }
}

fn names() -> Vec<String> {
    game_names(&std::fs::read_to_string(CONFIG).unwrap_or_default())
}

fn game_now(proc_root: &Path) -> Option<Game> {
    find_game(proc_root, &names(), std::process::id())
}

/// ExecCondition=: never stops a chore by mistake. Only a game that was found and a note that was written give SKIP; any
/// trouble (a wrong command line, an unwritable folder) lets the chore run now, saying why.
fn no_game(args: &[String]) -> i32 {
    let (flags, rest): (Vec<String>, Vec<String>) = args.iter().cloned().partition(|a| a.starts_with("--"));
    let unit = match rest.as_slice() {
        [u] if valid_unit(u) => u.clone(),
        _ => {
            println!("yggdrasil-gametime no-game: expected one service name (ExecCondition=+/usr/bin/yggdrasil-gametime no-game %n), got {rest:?}; it runs now");
            return 0;
        }
    };
    let Some(game) = game_now(Path::new("/proc")) else { return 0 };
    let wl = waitlist(scope(&flags));
    match wl.add(&unit) {
        Ok(_) => {
            println!("{unit} waits until the game is over: {}. It runs within 5 minutes of the game ending.", game.reason());
            SKIP
        }
        Err(e) => {
            println!("{unit} runs now although {}: it could not be noted for later in {} ({e})", game.reason(), wl.dir.display());
            0
        }
    }
}

/// Starts what waited, once no game runs. With nothing waiting it is one stat and exit.
fn catch_up(args: &[String]) -> i32 {
    let scope = scope(args);
    let wl = waitlist(scope);
    if !wl.has_any() {
        return 0;
    }
    if game_now(Path::new("/proc")).is_some() {
        return 0;
    }
    let units = match wl.take() {
        Ok(u) => u,
        Err(e) => {
            eprintln!("yggdrasil-gametime catch-up: cannot take the list in {}: {e}", wl.dir.display());
            return 1;
        }
    };
    for unit in &units {
        let mut cmd = Command::new("systemctl");
        if scope == Scope::User {
            cmd.arg("--user");
        }
        cmd.args(["start", "--no-block", "--", unit]).stdin(Stdio::null());
        match cmd.status() {
            Ok(s) if s.success() => println!("{unit}: the game is over, started now"),
            Ok(s) => eprintln!("{unit}: could not be started ({s}); its own timer runs it next time"),
            Err(e) => eprintln!("{unit}: could not run systemctl ({e}); its own timer runs it next time"),
        }
    }
    if let Err(e) = wl.done() {
        eprintln!("yggdrasil-gametime catch-up: cannot clear the list in {}: {e}", wl.dir.display());
    }
    0
}

fn status(args: &[String]) -> i32 {
    let mut json = false;
    let mut proc_root = PathBuf::from("/proc");
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--json" => json = true,
            "--proc" => match it.next() {
                Some(d) => proc_root = PathBuf::from(d),
                None => return usage(),
            },
            _ => return usage(),
        }
    }
    let game = game_now(&proc_root);
    let system = Waitlist::new(SYSTEM_DIR).list();
    // root with no session of its own has no user list
    let user = if euid() != 0 || std::env::var_os("XDG_RUNTIME_DIR").is_some() { Waitlist::new(user_dir()).list() } else { Vec::new() };
    if json {
        println!("{}", status_json(game.as_ref(), &system, &user));
    } else {
        match &game {
            Some(g) => println!("A game is running: {}.", g.reason()),
            None => println!("No game is running."),
        }
        if system.is_empty() && user.is_empty() {
            println!("Nothing is waiting for a game to end.");
        } else {
            println!("Waiting for the game to end (they start within 5 minutes of it):");
            if !system.is_empty() {
                println!("  the computer's: {}", system.join(", "));
            }
            if !user.is_empty() {
                println!("  this account's: {}", user.join(", "));
            }
        }
    }
    if game.is_some() {
        0
    } else {
        1
    }
}

fn usage() -> i32 {
    eprintln!(
        "usage: yggdrasil-gametime status [--json] [--proc DIR]   is a game running (exit 0) or not (exit 1), and what waits
       yggdrasil-gametime no-game UNIT                    for ExecCondition=: 0 = run now, 100 = a game runs, UNIT waits
       yggdrasil-gametime catch-up [--system|--user]      start what waited, once no game runs
Games: Steam (its reaper with an AppId), gamescope, Lutris, and `games = name, name` in {CONFIG}."
    );
    2
}

fn run(args: &[String]) -> i32 {
    match args.first().map(String::as_str) {
        Some("status") => status(&args[1..]),
        Some("no-game") => no_game(&args[1..]),
        Some("catch-up") => catch_up(&args[1..]),
        Some("-h" | "--help" | "help") => {
            usage();
            0
        }
        _ => usage(),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    // A panic exits 101, which ExecCondition= would take as "skip": for no-game it must mean "run it" instead.
    let on_panic = if args.first().map(String::as_str) == Some("no-game") { 0 } else { 2 };
    let code = std::panic::catch_unwind(|| run(&args)).unwrap_or(on_panic);
    std::process::exit(code);
}
