//! What to do as the user stays away: a pure description of the timeline, so it can be tested without a screen.

/// One step of the away-timeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Step {
    Dim,
    Lock,
    DisplayOff,
    Suspend,
}

impl Step {
    pub fn name(self) -> &'static str {
        match self {
            Step::Dim => "dim",
            Step::Lock => "lock",
            Step::DisplayOff => "display-off",
            Step::Suspend => "suspend",
        }
    }
}

/// The settings: seconds of inactivity at which each step happens (0 = never), and whether the manager may act at all.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    pub act: bool,
    pub dim: u32,
    pub lock: u32,
    pub display_off: u32,
    pub suspend: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { act: false, dim: 0, lock: 0, display_off: 0, suspend: 0 }
    }
}

impl Config {
    /// `key = value` lines; `#` starts a comment. Unknown keys and bad values are ignored (the default stays).
    pub fn parse(text: &str) -> Config {
        let mut c = Config::default();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let secs = v.parse::<u32>().ok();
            match k {
                "mode" => c.act = v == "act",
                "dim" => c.dim = secs.unwrap_or(c.dim),
                "lock" => c.lock = secs.unwrap_or(c.lock),
                "display_off" => c.display_off = secs.unwrap_or(c.display_off),
                "suspend" => c.suspend = secs.unwrap_or(c.suspend),
                _ => {}
            }
        }
        c
    }

    /// Every step that has a time, in the order it happens (ties keep the order dim, lock, display-off, suspend).
    pub fn steps(&self) -> Vec<(u32, Step)> {
        let mut v: Vec<(u32, Step)> = [(self.dim, Step::Dim), (self.lock, Step::Lock), (self.display_off, Step::DisplayOff), (self.suspend, Step::Suspend)]
            .into_iter()
            .filter(|(s, _)| *s > 0)
            .collect();
        v.sort_by_key(|(s, _)| *s);
        v
    }
}

/// The inactivity marks the observer reports on, whatever the settings (seconds).
pub const OBSERVE_MARKS: [u32; 11] = [30, 60, 120, 180, 240, 300, 420, 600, 900, 1800, 3600];

/// All the marks to ask the compositor about: the observer's and the configured steps', sorted, without repeats.
pub fn marks(c: &Config) -> Vec<u32> {
    let mut m: Vec<u32> = OBSERVE_MARKS.to_vec();
    m.extend(c.steps().iter().map(|(s, _)| *s));
    m.sort_unstable();
    m.dedup();
    m
}

/// Steps that fall due at exactly this mark.
pub fn due_at(c: &Config, mark: u32) -> Vec<Step> {
    c.steps().into_iter().filter(|(s, _)| *s == mark).map(|(_, st)| st).collect()
}

/// Something that must stop a step from happening (a video playing, a download, a game): the logind inhibitors that block idle,
/// sleep or the lock. Dimming is allowed to go on; locking and sleeping are not.
pub fn blocked_by_inhibitors(step: Step, what: &[&str]) -> bool {
    match step {
        Step::Dim => false,
        Step::Lock | Step::DisplayOff => what.iter().any(|w| w.contains("idle")),
        Step::Suspend => what.iter().any(|w| w.contains("idle") || w.contains("sleep")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_ignores_junk() {
        let c = Config::parse("# comment\nmode = act\ndim = 300  # five minutes\nlock = soon\nsuspend=1800\nbogus = 1\n");
        assert_eq!(c, Config { act: true, dim: 300, lock: 0, display_off: 0, suspend: 1800 });
        assert_eq!(Config::parse(""), Config::default());
        assert!(!Config::parse("mode = observe").act);
    }

    #[test]
    fn steps_are_ordered_and_zero_means_never() {
        let c = Config { act: true, dim: 300, lock: 0, display_off: 600, suspend: 450 };
        assert_eq!(c.steps(), vec![(300, Step::Dim), (450, Step::Suspend), (600, Step::DisplayOff)]);
        assert!(Config::default().steps().is_empty());
    }

    #[test]
    fn marks_include_the_settings_once() {
        let c = Config { act: false, dim: 300, lock: 301, display_off: 0, suspend: 0 };
        let m = marks(&c);
        assert!(m.windows(2).all(|w| w[0] < w[1]), "sorted, no repeats");
        assert!(m.contains(&300) && m.contains(&301) && m.contains(&60));
        assert_eq!(due_at(&c, 300), vec![Step::Dim]);
        assert_eq!(due_at(&c, 301), vec![Step::Lock]);
        assert!(due_at(&c, 60).is_empty());
    }

    #[test]
    fn inhibitors_hold_back_locking_and_sleeping_not_dimming() {
        assert!(!blocked_by_inhibitors(Step::Dim, &["idle"]));
        assert!(blocked_by_inhibitors(Step::Lock, &["idle"]));
        assert!(blocked_by_inhibitors(Step::Suspend, &["sleep"]));
        assert!(!blocked_by_inhibitors(Step::Suspend, &["shutdown"]));
        assert!(!blocked_by_inhibitors(Step::Lock, &[]));
    }
}
