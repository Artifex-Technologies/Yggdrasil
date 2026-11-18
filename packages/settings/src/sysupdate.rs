//! Settings > General > Software Update: a big update icon with a badge (up to date, updates waiting, approved), "You're up to
//! date" or "5 updates available" with "Last checked: Today, 2:14 PM", Check for updates; then More options: Available Updates
//! (each update with its versions, size and repository, grouped), Pause updates (a week without automatic checks:
//! `yggdrasil_updates::pause`, which Agora honours too), Update history (every package transaction from pacman's log) and App
//! Updates (Agora's Updates), and Help with updates.
//!
//! - **Check**: Agora's own checker (packages/updates, shared): a private copy of pacman's sync databases is refreshed without
//!   root, as checkupdates does, so pacman's own databases are never written or locked and a terminal's `pacman -Syu` works as
//!   before. It runs when the page opens if the last check is more than 15 minutes old and updates are not paused, and on
//!   "Check for updates"; never in the background, never on a timer.
//! - **Kept** between openings in `$XDG_CACHE_HOME/yggdrasil/system-updates` (a few lines: "checked SECONDS", "download BYTES",
//!   "offline" when the servers did not answer, then one "name old -> new SIZE REPO" per update), so the page shows the last
//!   answer at once.
//! - **Installing** is Agora's (its Updates page: one administrator password, its progress and cancel); Settings runs no package
//!   command itself.
//! - **Validated before installing**: Sarina (an optional component) is to try updates in a Limbo guest first (her
//!   limbo-validate, with a profile I write). The hook is `validation`: the assistant's integration may name a file
//!   (`update_verdict`) whose first line is "approved" or "held", followed by the "name old -> new" lines it tried. A verdict
//!   counts only for exactly the updates available now. Only "approved" turns Check for updates into Install updates.
//! - **Update History** is pacman's log read by `yggdrasil_updates::history` (a module of its own, which an assistant can use
//!   to tell which update came before a problem), read when the page is opened.
//!
//! A made-up system (fixture) answers from its `run` entries "pacman -Qu", "pacman -Sup --print-format %n %r %s" and "cat
//! /var/log/pacman.log", and keeps nothing.

use yggdrasil_updates::history::{self, Transaction};
use yggdrasil_updates::{add_sizes, parse_download, parse_updates, Update};

/// A check older than this is done again when the page opens.
pub const STALE_SECS: i64 = 15 * 60;

/// How many transactions Update History shows (the newest).
pub const HISTORY_MAX: usize = 300;

/// Which part of Software Update is open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Sub {
    #[default]
    Main,
    Available,
    History,
}

/// What the page shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// When the repositories were last asked (seconds since 1970); 0: never.
    pub checked: i64,
    pub updates: Vec<Update>,
    /// Bytes still to download (what pacman's cache holds counts nothing).
    pub download: u64,
    /// The update servers did not answer that time: the list is what pacman last knew.
    pub offline: bool,
    /// Read when the page opens or checks, not kept.
    pub validation: Validation,
}

impl State {
    /// Whether the page should ask again on opening.
    pub fn stale(&self, now: i64) -> bool {
        self.checked == 0 || now - self.checked > STALE_SECS || now < self.checked
    }
}

/// Whether the available updates were tried in a Limbo guest before installing.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Validation {
    /// No assistant is installed: nothing is said about it.
    #[default]
    Hidden,
    /// The assistant (its name) has no verdict on these updates.
    NotYet(String),
    Approved(String),
    Held(String),
}

impl Validation {
    pub fn note(&self) -> Option<String> {
        match self {
            Validation::Hidden => None,
            Validation::NotYet(n) => Some(format!("Not validated yet: {n}'s update check comes later")),
            Validation::Approved(n) => Some(format!("{n} tried these updates in a Limbo guest: they can be installed")),
            Validation::Held(n) => Some(format!("{n} held these updates back after trying them in a Limbo guest")),
        }
    }
}

/// An assistant's verdict file for `updates`: Some(true) "approved", Some(false) "held", None when it is about other updates
/// (or says neither).
pub fn verdict(text: &str, updates: &[Update]) -> Option<bool> {
    let mut lines = text.lines();
    let word = lines.next()?.trim();
    let mut tried: Vec<(String, String, String)> =
        parse_updates(&lines.collect::<Vec<_>>().join("\n"), false).into_iter().map(|u| (u.name, u.old, u.new)).collect();
    let mut now: Vec<(String, String, String)> = updates.iter().map(|u| (u.name.clone(), u.old.clone(), u.new.clone())).collect();
    tried.sort();
    now.sort();
    if updates.is_empty() || tried != now {
        return None;
    }
    match word {
        "approved" => Some(true),
        "held" => Some(false),
        _ => None,
    }
}

/// The hook for validated updates: the assistant's integration (kind "assistant"), and its `update_verdict` file if it names
/// one.
pub fn validation(updates: &[Update]) -> Validation {
    let Some(i) = yggdrasil_conf::integrations::first("assistant") else { return Validation::Hidden };
    let name = i.get("name").unwrap_or("The assistant").to_string();
    let v = i.path("update_verdict").and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| verdict(&t, updates));
    match v {
        Some(true) => Validation::Approved(name),
        Some(false) => Validation::Held(name),
        None => Validation::NotYet(name),
    }
}

/// The kept answer, as `cache_text` writes it. Lines it does not know are skipped.
pub fn parse_cache(text: &str) -> State {
    let mut s = State::default();
    for l in text.lines() {
        let w: Vec<&str> = l.split_whitespace().collect();
        match w.as_slice() {
            ["checked", t] => s.checked = t.parse().unwrap_or(0),
            ["download", b] => s.download = b.parse().unwrap_or(0),
            ["offline"] => s.offline = true,
            [name, old, "->", new, rest @ ..] => s.updates.push(Update {
                name: name.to_string(),
                old: old.to_string(),
                new: new.to_string(),
                aur: false,
                size: rest.first().and_then(|v| v.parse().ok()).unwrap_or(0),
                repo: rest.get(1).map(|r| r.to_string()).unwrap_or_default(),
            }),
            _ => {}
        }
    }
    s
}

pub fn cache_text(s: &State) -> String {
    let mut t = format!("checked {}\ndownload {}\n", s.checked, s.download);
    if s.offline {
        t.push_str("offline\n");
    }
    for u in &s.updates {
        t.push_str(format!("{} {} -> {} {} {}", u.name, u.old, u.new, u.size, u.repo).trim_end());
        t.push('\n');
    }
    t
}

fn cache_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".cache")))?;
    Some(base.join("yggdrasil/system-updates"))
}

/// The last answer kept (quick: one small file). A made-up system has none, so the page checks.
pub fn cached() -> State {
    let mut s = if crate::fixture::get().is_some() {
        State::default()
    } else {
        cache_path().and_then(|p| std::fs::read_to_string(p).ok()).map(|t| parse_cache(&t)).unwrap_or_default()
    };
    s.validation = validation(&s.updates);
    s
}

/// Asks the repositories (needs the network; a few seconds, on a thread of its own) and keeps the answer.
pub fn check() -> State {
    if crate::fixture::get().is_some() {
        let mut updates = parse_updates(&crate::sys::run("pacman", &["-Qu"]).unwrap_or_default(), false);
        let d = parse_download(&crate::sys::run("pacman", &["-Sup", "--print-format", "%n %r %s"]).unwrap_or_default());
        let download = add_sizes(&mut updates, &d);
        let validation = validation(&updates);
        return State { checked: yggdrasil_updates::pause::now(), updates, download, offline: false, validation };
    }
    let c = yggdrasil_updates::check_repos(true);
    let validation = validation(&c.updates);
    let s = State { checked: yggdrasil_updates::pause::now(), updates: c.updates, download: c.download, offline: !c.refreshed, validation };
    if let Some(p) = cache_path() {
        if let Some(d) = p.parent() {
            let _ = std::fs::create_dir_all(d);
        }
        let _ = std::fs::write(p, cache_text(&s));
    }
    s
}

/// Update History: the newest transactions in pacman's log (on a thread of its own: the log can be a few megabytes).
pub fn history() -> Vec<Transaction> {
    let text = if crate::fixture::get().is_some() {
        crate::sys::run("cat", &[history::LOG]).unwrap_or_default()
    } else {
        std::fs::read_to_string(history::LOG).unwrap_or_default()
    };
    history::recent(&text, HISTORY_MAX)
}

/// The big line: up to date, how many updates, or checking.
pub fn headline(s: &State, checking: bool) -> String {
    match s.updates.len() {
        _ if checking => "Checking for updates…".to_string(),
        0 if s.checked == 0 => "Not checked yet".to_string(),
        0 => "You're up to date".to_string(),
        1 => "1 update available".to_string(),
        n => format!("{n} updates available"),
    }
}

/// The line under it: "Last checked: Today, 2:14 PM" (and when the servers did not answer).
pub fn last_checked(s: &State, now: i64, clock_24h: bool) -> String {
    if s.checked == 0 {
        return "Last checked: never".into();
    }
    let when = checked_at(s.checked, now, clock_24h);
    if s.offline {
        format!("Last checked: {when} (the update servers did not answer)")
    } else {
        format!("Last checked: {when}")
    }
}

/// "Today, 10:42 AM" (or "Today, 10:42"), "Yesterday, 10:42 AM", else "Nov 12".
pub fn checked_at(t: i64, now: i64, clock_24h: bool) -> String {
    let tz = jiff::tz::TimeZone::system();
    let (Ok(at), Ok(today)) = (jiff::Timestamp::from_second(t), jiff::Timestamp::from_second(now)) else { return String::new() };
    let (at, today) = (at.to_zoned(tz.clone()), today.to_zoned(tz));
    let clock = if clock_24h { at.strftime("%H:%M").to_string() } else { at.strftime("%-I:%M %p").to_string() };
    match (today.date() - at.date()).get_days() {
        0 => format!("Today, {clock}"),
        1 => format!("Yesterday, {clock}"),
        _ => at.strftime("%b %-d").to_string(),
    }
}

/// A day, for "Paused until Mon, Nov 24".
pub fn day(t: i64) -> String {
    jiff::Timestamp::from_second(t)
        .map(|ts| ts.to_zoned(jiff::tz::TimeZone::system()).strftime("%a, %b %-d").to_string())
        .unwrap_or_default()
}

/// The groups Available Updates shows, in order: the kernel, drivers and firmware, the base system, then the rest.
pub fn group(u: &Update) -> &'static str {
    let n = u.name.as_str();
    let kernel = n == "linux" || n.starts_with("linux-") && !n.starts_with("linux-firmware") && !n.starts_with("linux-api");
    let driver = n.starts_with("linux-firmware")
        || n.starts_with("nvidia")
        || n.starts_with("lib32-nvidia")
        || n.contains("mesa")
        || n.starts_with("vulkan-")
        || n.starts_with("lib32-vulkan-")
        || n.starts_with("xf86-video-")
        || n.ends_with("-dkms")
        || n.ends_with("-ucode");
    if kernel {
        "Kernel"
    } else if driver {
        "Drivers and firmware"
    } else if u.repo.split('-').any(|w| w == "core") {
        "System"
    } else {
        "Apps and libraries"
    }
}

pub const GROUPS: [&str; 4] = ["Kernel", "Drivers and firmware", "System", "Apps and libraries"];

/// A download size: kB under a megabyte, MB under a gigabyte, else gigabytes with one decimal.
pub fn size(bytes: u64) -> String {
    match bytes {
        0..=999_999 => format!("{} kB", bytes.div_ceil(1000)),
        1_000_000..=999_999_999 => format!("{:.0} MB", bytes as f64 / 1e6),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn up(name: &str, old: &str, new: &str) -> Update {
        Update { name: name.into(), old: old.into(), new: new.into(), aur: false, size: 0, repo: String::new() }
    }

    #[test]
    fn the_kept_answer_reads_back() {
        let mut k = up("linux", "6.17.1-1", "6.17.2-1");
        k.size = 152_000_000;
        k.repo = "core".into();
        let s = State {
            checked: 1_763_400_000,
            updates: vec![k, up("mesa", "1:25.2-1", "1:25.3-1")],
            download: 182_000_000,
            offline: true,
            validation: Validation::Hidden,
        };
        assert_eq!(parse_cache(&cache_text(&s)), s);
        assert_eq!(parse_cache(""), State::default());
        let odd = parse_cache("checked soon\ndownload -5\nnoise\nzlib 1.3-1 -> 1.3.1-1\n");
        assert_eq!((odd.checked, odd.download, odd.updates.len()), (0, 0, 1));
    }

    #[test]
    fn checked_again_on_opening_only_when_old() {
        let s = |checked| State { checked, ..Default::default() };
        assert!(s(0).stale(1_000_000), "never checked");
        assert!(!s(1_000_000).stale(1_000_000 + 60));
        assert!(s(1_000_000).stale(1_000_000 + STALE_SECS + 1));
        assert!(s(1_000_000).stale(999_000), "a clock set back");
    }

    #[test]
    fn the_header_says_how_many_and_when() {
        let mut s = State::default();
        assert_eq!(headline(&s, false), "Not checked yet");
        assert_eq!(headline(&s, true), "Checking for updates…");
        assert_eq!(last_checked(&s, 100, true), "Last checked: never");
        s.checked = 1_763_400_000;
        assert_eq!(headline(&s, false), "You're up to date");
        assert!(last_checked(&s, s.checked + 60, true).starts_with("Last checked: Today, "));
        s.updates = vec![up("a", "1", "2"), up("b", "1", "2")];
        assert_eq!(headline(&s, false), "2 updates available");
        s.offline = true;
        assert!(last_checked(&s, s.checked + 60, true).ends_with("(the update servers did not answer)"));
        assert!(
            checked_at(s.checked, s.checked + 86_400, true).starts_with("Yesterday, ")
                || checked_at(s.checked, s.checked + 86_400, true).starts_with("Today, ")
        );
        assert_eq!(checked_at(s.checked, s.checked + 3 * 86_400, true).split(' ').count(), 2, "a date: \"Nov 17\"");
        assert!(checked_at(s.checked, s.checked, false).ends_with('M'), "12-hour clock");
        assert_eq!(day(s.checked).split(' ').count(), 3, "\"Mon, Nov 17\"");
        assert_eq!(size(1), "1 kB");
        assert_eq!(size(182_400_000), "182 MB");
        assert_eq!(size(2_500_000_000), "2.5 GB");
    }

    #[test]
    fn updates_are_grouped() {
        let g = |n: &str, repo: &str| {
            let mut u = up(n, "1", "2");
            u.repo = repo.into();
            group(&u)
        };
        assert_eq!(g("linux", "core"), "Kernel");
        assert_eq!(g("linux-zen-headers", "extra"), "Kernel");
        assert_eq!(g("linux-firmware-intel", "core"), "Drivers and firmware");
        assert_eq!(g("nvidia-open-dkms", "extra"), "Drivers and firmware");
        assert_eq!(g("lib32-mesa", "multilib"), "Drivers and firmware");
        assert_eq!(g("intel-ucode", "extra"), "Drivers and firmware");
        assert_eq!(g("glibc", "core"), "System");
        assert_eq!(g("glibc", "cachyos-core-v3"), "System");
        assert_eq!(g("linux-api-headers", "core"), "System");
        assert_eq!(g("krita", "extra"), "Apps and libraries");
        assert!(GROUPS.contains(&g("x", "")));
    }

    #[test]
    fn a_verdict_counts_only_for_the_updates_available_now() {
        let now = vec![up("linux", "6.17.7-1", "6.17.8-1"), up("mesa", "1", "2")];
        let tried = "approved\nmesa 1 -> 2\nlinux 6.17.7-1 -> 6.17.8-1\n";
        assert_eq!(verdict(tried, &now), Some(true), "in any order");
        assert_eq!(verdict(&tried.replace("approved", "held"), &now), Some(false));
        assert_eq!(verdict(&tried.replace("approved", "maybe"), &now), None);
        assert_eq!(verdict("approved\nmesa 1 -> 2\n", &now), None, "about fewer updates");
        assert_eq!(verdict("approved\nmesa 1 -> 3\nlinux 6.17.7-1 -> 6.17.8-1\n", &now), None, "about another version");
        assert_eq!(verdict("approved\n", &[]), None, "nothing to install");
        assert_eq!(verdict("", &now), None);
        assert_eq!(Validation::Hidden.note(), None);
        assert_eq!(Validation::NotYet("Sarina".into()).note().as_deref(), Some("Not validated yet: Sarina's update check comes later"));
    }
}
