//! One notice per program every ten minutes, so a crash loop does not become a storm of notices; and one per crash, though
//! both its core file and its journal entry tell of it.

use std::collections::{HashMap, VecDeque};

/// How long a program stays quiet after a notice about it, in seconds.
pub const QUIET_SECS: u64 = 600;

pub struct Quiet {
    secs: u64,
    last: HashMap<String, u64>,
}

impl Quiet {
    pub fn new(secs: u64) -> Quiet {
        Quiet { secs, last: HashMap::new() }
    }

    /// Whether a notice about `program` may be shown at `now` (seconds); if so, it counts as shown. Programs whose quiet time is
    /// over are forgotten, so the list only holds what crashed in the last ten minutes.
    pub fn allow(&mut self, program: &str, now: u64) -> bool {
        let secs = self.secs;
        self.last.retain(|_, t| now.saturating_sub(*t) < secs);
        if self.last.contains_key(program) {
            return false;
        }
        self.last.insert(program.to_string(), now);
        true
    }

    pub fn len(&self) -> usize {
        self.last.len()
    }

    pub fn is_empty(&self) -> bool {
        self.last.is_empty()
    }
}

/// The crashes already handled, by process id: a stored core's file and its journal entry both tell of one crash, and only
/// the first to arrive counts. The last 64 are kept (a process id comes back only after millions of others).
#[derive(Default)]
pub struct Seen {
    pids: VecDeque<u32>,
}

impl Seen {
    /// Whether this is the first word of the crash of process `pid`; it counts as seen from now on.
    pub fn first(&mut self, pid: u32) -> bool {
        if self.pids.contains(&pid) {
            return false;
        }
        if self.pids.len() == 64 {
            self.pids.pop_front();
        }
        self.pids.push_back(pid);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_crash_once() {
        let mut s = Seen::default();
        assert!(s.first(4242));
        assert!(!s.first(4242), "the journal entry after the core file");
        assert!(s.first(4243));
        for p in 0..64 {
            s.first(10_000 + p);
        }
        assert!(s.first(4242), "long forgotten");
    }

    #[test]
    fn one_notice_per_program_every_ten_minutes() {
        let mut q = Quiet::new(QUIET_SECS);
        // right after boot too (the clock counts from boot: an unknown program is never "seen at 0")
        assert!(q.allow("/usr/bin/game", 5));
        assert!(!q.allow("/usr/bin/game", 6), "a crash loop stays quiet");
        assert!(!q.allow("/usr/bin/game", 5 + 599));
        assert!(q.allow("/usr/bin/other", 300), "another program has its own ten minutes");
        assert!(q.allow("/usr/bin/game", 5 + 600), "ten minutes later it may show again");
        assert!(!q.allow("/usr/bin/game", 5 + 601));
    }

    #[test]
    fn forgets_programs_whose_time_is_over() {
        let mut q = Quiet::new(QUIET_SECS);
        for i in 0..50 {
            assert!(q.allow(&format!("/opt/p{i}"), 100));
        }
        assert_eq!(q.len(), 50);
        assert!(q.allow("/opt/late", 100 + 600));
        assert_eq!(q.len(), 1, "only what crashed in the last ten minutes is kept");
    }
}
