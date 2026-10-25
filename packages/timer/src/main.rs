//! yggdrasil-timer: a countdown timer in Yggdrasil's top bar. The panel draws its pill and card from
//! ~/.local/state/yggdrasil/timer/bar.json (timer.conf) and runs `press` for the card's buttons, so nothing stays running.
//!
//!   yggdrasil-timer press ID           a press on the card: preset:N, pause, resume, add, again, reset, custom:-, custom:+, start-custom
//!   yggdrasil-timer start DUR [LABEL]  start a timer ("7" minutes, "1:30", "90s", "1h30m")
//!   yggdrasil-timer ring ID GEN        run by the systemd timer set at start: marks it done, notifies, plays a sound
//!   yggdrasil-timer reconcile          at login: finished timers are marked done, running ones get their alarm back

mod timers;

use std::process::ExitCode;

use timers::{State, Store, Timer};

fn store() -> Store {
    Store::new(timers::dir())
}

/// Finished timers whose alarm was lost (the computer was off) are marked done; running ones whose alarm is gone get it back.
fn reconcile(s: &Store) {
    let now = timers::now();
    // a few seconds of grace, so an alarm that is firing right now still rings
    let Ok((st, _)) = s.change(|st| timers::expire(st, now - 5)) else { return };
    for t in st.timers.iter().filter(|t| t.running()) {
        if !timers::armed(t) {
            timers::arm(t, now);
        }
    }
}

fn ring(s: &Store, id: u64, generation: u32) -> ExitCode {
    let now = timers::now();
    let r = s.change(|st| {
        let t = st.timers.iter_mut().find(|t| t.id == id && t.generation == generation && t.running() && t.until.unwrap_or(0) <= now + 2)?;
        t.done = true;
        Some(t.clone())
    });
    if let Ok((_, Some(t))) = r {
        timers::alert(&t);
    }
    ExitCode::SUCCESS
}

/// Makes a change and moves the alarms to match: an alarm for each running timer as it now is, none for the rest.
fn apply<R>(s: &Store, change: impl FnOnce(&mut State, i64) -> R) -> Option<R> {
    let now = timers::now();
    let (st, (before, r)) = s
        .change(|st| {
            let before: Vec<Timer> = st.timers.iter().filter(|t| t.running()).cloned().collect();
            let r = change(st, now);
            (before, r)
        })
        .ok()?;
    let same = |a: &Timer, b: &Timer| a.id == b.id && a.generation == b.generation;
    for old in before.iter().filter(|o| !st.timers.iter().any(|t| t.running() && same(t, o))) {
        timers::disarm(old);
    }
    for new in st.timers.iter().filter(|t| t.running() && !before.iter().any(|o| same(t, o))) {
        timers::arm(new, now);
    }
    Some(r)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let s = store();
    match args.first().map(String::as_str) {
        Some("press") => match args.get(1) {
            Some(id) => match apply(&s, |st, now| timers::press(st, id, now)) {
                Some(true) => ExitCode::SUCCESS,
                Some(false) => ExitCode::from(2),
                None => ExitCode::FAILURE,
            },
            None => ExitCode::from(2),
        },
        Some("ring") => match (args.get(1).and_then(|v| v.parse().ok()), args.get(2).and_then(|v| v.parse().ok())) {
            (Some(id), Some(generation)) => ring(&s, id, generation),
            _ => ExitCode::from(2),
        },
        Some("reconcile") => {
            reconcile(&s);
            ExitCode::SUCCESS
        }
        Some("start") => match args.get(1).and_then(|d| timers::parse_duration(d)) {
            Some(secs) => {
                let label = args[2..].join(" ");
                if apply(&s, |st, now| timers::start(st, secs, &label, now)).is_some() {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                }
            }
            None => {
                eprintln!("yggdrasil-timer: give a length: 7 (minutes), 1:30, 90s, 1h30m");
                ExitCode::from(2)
            }
        },
        _ => {
            eprintln!("usage: yggdrasil-timer press ID | start DURATION [LABEL] | reconcile | ring ID GEN");
            ExitCode::from(2)
        }
    }
}
