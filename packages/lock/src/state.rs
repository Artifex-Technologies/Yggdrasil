//! The lock screen's logic without any screen: what the typed password is, what a key does, how long after wrong passwords it makes you wait.

use std::time::{Duration, Instant};

use zeroize::Zeroize;

pub const MAX_LEN: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Char(char),
    Backspace,
    Enter,
    /// Clears what was typed (it never unlocks).
    Escape,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Redraw,
    /// Check this password now.
    Submit(String),
}

#[derive(Default)]
pub struct Lock {
    typed: String,
    pub failures: u32,
    pub message: String,
    wait_until: Option<Instant>,
}

impl Lock {
    pub fn dots(&self) -> usize {
        self.typed.chars().count()
    }

    pub fn waiting(&self, now: Instant) -> bool {
        self.wait_until.map(|t| now < t).unwrap_or(false)
    }

    pub fn key(&mut self, k: Key, now: Instant) -> Action {
        if self.waiting(now) {
            return Action::Nothing; // after several wrong passwords: no typing until the wait is over
        }
        match k {
            Key::Char(c) if !c.is_control() => {
                if self.typed.chars().count() < MAX_LEN {
                    self.typed.push(c);
                }
                self.message.clear();
                Action::Redraw
            }
            Key::Char(_) => Action::Nothing,
            Key::Backspace => {
                self.typed.pop();
                Action::Redraw
            }
            Key::Escape => {
                self.typed.zeroize();
                self.message.clear();
                Action::Redraw
            }
            Key::Enter => {
                if self.typed.is_empty() {
                    return Action::Nothing;
                }
                let pw = std::mem::take(&mut self.typed);
                Action::Submit(pw)
            }
        }
    }

    /// The answer of the check. A wrong password counts; from the fourth in a row each one makes you wait longer (2 s doubling, at most 60 s).
    pub fn result(&mut self, ok: bool, now: Instant) {
        if ok {
            self.failures = 0;
            self.message.clear();
            return;
        }
        self.failures += 1;
        self.message = "Wrong password".into();
        if self.failures >= 4 {
            let secs = (2u64 << (self.failures - 4).min(5)).min(60);
            self.wait_until = Some(now + Duration::from_secs(secs));
            self.message = format!("Wrong password: wait {secs} s");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn type_str(l: &mut Lock, s: &str, now: Instant) {
        for c in s.chars() {
            l.key(Key::Char(c), now);
        }
    }

    #[test]
    fn typing_editing_and_submitting() {
        let now = Instant::now();
        let mut l = Lock::default();
        type_str(&mut l, "abc", now);
        assert_eq!(l.dots(), 3);
        l.key(Key::Backspace, now);
        assert_eq!(l.dots(), 2);
        assert_eq!(l.key(Key::Enter, now), Action::Submit("ab".into()));
        assert_eq!(l.dots(), 0, "the password is taken out of the screen state when it is submitted");
        assert_eq!(l.key(Key::Enter, now), Action::Nothing, "an empty password is not submitted");
    }

    #[test]
    fn escape_clears_but_never_unlocks() {
        let now = Instant::now();
        let mut l = Lock::default();
        type_str(&mut l, "secret", now);
        assert_eq!(l.key(Key::Escape, now), Action::Redraw);
        assert_eq!(l.dots(), 0);
    }

    #[test]
    fn control_characters_and_long_input_are_handled() {
        let now = Instant::now();
        let mut l = Lock::default();
        assert_eq!(l.key(Key::Char('\u{7}'), now), Action::Nothing);
        type_str(&mut l, &"x".repeat(MAX_LEN + 50), now);
        assert_eq!(l.dots(), MAX_LEN);
    }

    #[test]
    fn wrong_passwords_slow_you_down_after_three() {
        let now = Instant::now();
        let mut l = Lock::default();
        for _ in 0..3 {
            l.result(false, now);
            assert!(!l.waiting(now), "the first three wrong passwords cost nothing");
        }
        l.result(false, now);
        assert!(l.waiting(now));
        assert_eq!(l.key(Key::Char('a'), now), Action::Nothing, "typing is ignored while waiting");
        let later = now + Duration::from_secs(3);
        assert!(!l.waiting(later));
        l.result(false, later);
        assert!(l.waiting(later + Duration::from_secs(3)), "the wait doubles");
        l.result(true, later);
        assert_eq!(l.failures, 0);
    }

    #[test]
    fn the_wait_is_capped() {
        let now = Instant::now();
        let mut l = Lock::default();
        for _ in 0..30 {
            l.result(false, now);
        }
        assert!(l.waiting(now + Duration::from_secs(59)));
        assert!(!l.waiting(now + Duration::from_secs(61)));
    }
}
