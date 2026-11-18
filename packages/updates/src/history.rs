//! Every package transaction this computer made, from pacman's log (/var/log/pacman.log, readable by everyone): a `pacman
//! -Syu` in a terminal, Agora's updates and installs, any pacman run. Settings > General > Software Update > Update History shows
//! it; it is also what an assistant (Sarina) can read later to tell which update came before a problem, so it is kept here, apart
//! from any window.
//!
//! The log's lines look like `[2026-11-10T09:12:21+0100] [ALPM] upgraded linux (6.17.7-1 -> 6.17.8-1)`. A transaction runs from
//! `[ALPM] transaction started` to `[ALPM] transaction completed` (or `failed` / `interrupted`); the command that started it is
//! the last `[PACMAN] Running '...'` line before it. Package changes are `upgraded`, `downgraded` (both `NAME (OLD -> NEW)`),
//! `installed`, `removed` and `reinstalled` (`NAME (VERSION)`). Everything else (hooks, warnings, syncs) is skipped.

/// How a transaction ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Failed,
    /// The log ends, or another transaction starts, before it said how it ended (a crash or a power cut in the middle).
    Unfinished,
}

/// What happened to one package.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    Upgraded,
    Downgraded,
    Installed,
    Removed,
    Reinstalled,
}

impl Change {
    pub fn word(self) -> &'static str {
        match self {
            Change::Upgraded => "upgraded",
            Change::Downgraded => "downgraded",
            Change::Installed => "installed",
            Change::Removed => "removed",
            Change::Reinstalled => "reinstalled",
        }
    }
}

/// One package in a transaction. `old` is empty for installed, `new` for removed; a reinstall has the same version in both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Package {
    pub change: Change,
    pub name: String,
    pub old: String,
    pub new: String,
}

/// One transaction.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Transaction {
    /// When it started, as the log writes it ("2026-11-10T09:12:20+0100": the computer's time then, with its offset).
    pub started: String,
    /// The command that ran it ("pacman -Syu"), "" when the log does not say.
    pub command: String,
    pub outcome: Outcome,
    pub packages: Vec<Package>,
}

impl Transaction {
    /// How many packages changed in each way, for a one-line summary ("12 upgraded, 1 installed").
    pub fn counts(&self) -> Vec<(Change, usize)> {
        let all = [Change::Upgraded, Change::Installed, Change::Removed, Change::Downgraded, Change::Reinstalled];
        all.into_iter().map(|c| (c, self.packages.iter().filter(|p| p.change == c).count())).filter(|(_, n)| *n > 0).collect()
    }
}

/// `[TIME] [SOURCE] REST` -> (TIME, SOURCE, REST).
fn split(line: &str) -> Option<(&str, &str, &str)> {
    let rest = line.strip_prefix('[')?;
    let (time, rest) = rest.split_once("] [")?;
    let (source, rest) = rest.split_once("] ")?;
    Some((time, source, rest))
}

/// "NAME (A -> B)" or "NAME (A)".
fn package(change: Change, rest: &str) -> Option<Package> {
    let (name, v) = rest.split_once(" (")?;
    let v = v.strip_suffix(')')?;
    let (old, new) = match (change, v.split_once(" -> ")) {
        (Change::Upgraded | Change::Downgraded, Some((a, b))) => (a, b),
        (Change::Installed, None) => ("", v),
        (Change::Removed, None) => (v, ""),
        (Change::Reinstalled, None) => (v, v),
        _ => return None,
    };
    (!name.is_empty() && !name.contains(' ')).then(|| Package {
        change,
        name: name.to_string(),
        old: old.to_string(),
        new: new.to_string(),
    })
}

/// Every transaction in the log, oldest first.
pub fn parse(log: &str) -> Vec<Transaction> {
    let mut out: Vec<Transaction> = vec![];
    let mut open = false;
    let mut command = String::new();
    for line in log.lines() {
        let Some((time, source, rest)) = split(line) else { continue };
        match source {
            "PACMAN" => {
                if let Some(c) = rest.strip_prefix("Running '").and_then(|c| c.strip_suffix('\'')) {
                    command = c.to_string();
                }
            }
            "ALPM" => match rest {
                "transaction started" => {
                    // one that never said how it ended stays Unfinished
                    out.push(Transaction {
                        started: time.to_string(),
                        command: std::mem::take(&mut command),
                        outcome: Outcome::Unfinished,
                        packages: vec![],
                    });
                    open = true;
                }
                "transaction completed" | "transaction failed" | "transaction interrupted" if open => {
                    if let Some(t) = out.last_mut() {
                        t.outcome = if rest == "transaction completed" { Outcome::Completed } else { Outcome::Failed };
                    }
                    open = false;
                }
                _ if open => {
                    let (word, what) = rest.split_once(' ').unwrap_or((rest, ""));
                    let change = match word {
                        "upgraded" => Change::Upgraded,
                        "downgraded" => Change::Downgraded,
                        "installed" => Change::Installed,
                        "removed" => Change::Removed,
                        "reinstalled" => Change::Reinstalled,
                        _ => continue,
                    };
                    if let (Some(p), Some(t)) = (package(change, what), out.last_mut()) {
                        t.packages.push(p);
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    out
}

/// pacman's log.
pub const LOG: &str = "/var/log/pacman.log";

/// The last `max` transactions of the log's text, newest first.
pub fn recent(log: &str, max: usize) -> Vec<Transaction> {
    let mut t = parse(log);
    t.reverse();
    t.truncate(max);
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Made-up packages and times, in pacman's log format.
    const LOG_TEXT: &str = "\
[2026-11-08T20:01:02+0100] [PACMAN] Running 'pacman -S example-editor'
[2026-11-08T20:01:05+0100] [ALPM] transaction started
[2026-11-08T20:01:06+0100] [ALPM] installed example-lib (2.1-1)
[2026-11-08T20:01:06+0100] [ALPM] installed example-editor (4.0-3)
[2026-11-08T20:01:07+0100] [ALPM-SCRIPTLET] some output (with brackets)
[2026-11-08T20:01:07+0100] [ALPM] running '30-systemd-update.hook'...
[2026-11-08T20:01:08+0100] [ALPM] transaction completed
[2026-11-10T09:12:01+0100] [PACMAN] Running 'pacman -Syu'
[2026-11-10T09:12:01+0100] [PACMAN] synchronizing package lists
[2026-11-10T09:12:20+0100] [ALPM] transaction started
[2026-11-10T09:12:21+0100] [ALPM] upgraded example-kernel (6.17.7-1 -> 6.17.8-1)
[2026-11-10T09:12:21+0100] [ALPM] downgraded example-tool (2.0-1 -> 1.9-4)
[2026-11-10T09:12:22+0100] [ALPM] removed example-old (0.9-2)
[2026-11-10T09:12:22+0100] [ALPM] reinstalled example-font (1.0-1)
[2026-11-10T09:12:22+0100] [ALPM] warning: /etc/example.conf installed as /etc/example.conf.pacnew
[2026-11-10T09:12:30+0100] [ALPM] transaction completed
[2026-11-11T07:00:00+0100] [PACMAN] Running '/usr/bin/pacman -Su --noconfirm'
[2026-11-11T07:00:02+0100] [ALPM] transaction started
[2026-11-11T07:00:03+0100] [ALPM] upgraded example-browser (145.0-1 -> 145.0.1-1)
[2026-11-11T07:00:04+0100] [ALPM] transaction failed
[2026-11-12T10:00:00+0100] [PACMAN] Running 'pacman -Sy'
[2026-11-12T10:00:00+0100] [PACMAN] synchronizing package lists
[2026-11-12T10:05:00+0100] [ALPM] transaction started
[2026-11-12T10:05:01+0100] [ALPM] upgraded example-shell (5.3-1 -> 5.3-2)
not a log line
[2026-11-12T10:06:00+0100] [ALPM] transaction started
[2026-11-12T10:06:01+0100] [ALPM] upgraded example-shell (5.3-1 -> 5.3-2)
[2026-11-12T10:06:01+0100] [ALPM] upgraded broken (no arrow)
[2026-11-12T10:06:01+0100] [ALPM] installed two words (1-1)
[2026-11-12T10:06:02+0100] [ALPM] transaction completed
[2026-11-12T10:07:00+0100] [ALPM] upgraded outside (1 -> 2)
";

    #[test]
    fn transactions_are_read_from_the_log() {
        let t = parse(LOG_TEXT);
        assert_eq!(t.len(), 5);
        assert_eq!(t[0].command, "pacman -S example-editor");
        assert_eq!(t[0].outcome, Outcome::Completed);
        assert_eq!(t[0].packages.len(), 2, "scriptlet output, hooks and warnings are not packages");
        assert_eq!(
            t[0].packages[1],
            Package { change: Change::Installed, name: "example-editor".into(), old: "".into(), new: "4.0-3".into() }
        );
        let u = &t[1];
        assert_eq!((u.started.as_str(), u.command.as_str()), ("2026-11-10T09:12:20+0100", "pacman -Syu"));
        assert_eq!(
            u.packages[0],
            Package { change: Change::Upgraded, name: "example-kernel".into(), old: "6.17.7-1".into(), new: "6.17.8-1".into() }
        );
        assert_eq!((u.packages[1].change, u.packages[1].new.as_str()), (Change::Downgraded, "1.9-4"));
        assert_eq!((u.packages[2].change, u.packages[2].old.as_str(), u.packages[2].new.as_str()), (Change::Removed, "0.9-2", ""));
        assert_eq!((u.packages[3].old.as_str(), u.packages[3].new.as_str()), ("1.0-1", "1.0-1"));
        assert_eq!(u.counts(), vec![(Change::Upgraded, 1), (Change::Removed, 1), (Change::Downgraded, 1), (Change::Reinstalled, 1)]);
        assert_eq!(t[2].outcome, Outcome::Failed);
        assert_eq!(t[2].command, "/usr/bin/pacman -Su --noconfirm");
        assert_eq!((t[3].outcome, t[3].command.as_str()), (Outcome::Unfinished, "pacman -Sy"), "cut off by the next one");
        assert_eq!(t[4].command, "", "no Running line of its own");
        assert_eq!(t[4].packages.len(), 1, "odd package lines are skipped; one outside a transaction too");
    }

    #[test]
    fn newest_first_and_bounded() {
        let r = recent(LOG_TEXT, 2);
        assert_eq!(r.len(), 2);
        assert_eq!(r[0].started, "2026-11-12T10:06:00+0100");
        assert!(recent("", 10).is_empty());
        assert_eq!(parse("[2026-11-12T10:06:00+0100] [ALPM] transaction started").last().unwrap().outcome, Outcome::Unfinished);
    }
}
