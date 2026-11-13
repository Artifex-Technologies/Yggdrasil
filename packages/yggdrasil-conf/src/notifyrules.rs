//! Which notifications pop up: quiet hours (a daily stretch with no pop-ups, as Do not disturb) and apps whose pop-ups are
//! muted. Everything is still kept in the Notification Center, and a critical notification (a security alarm, a low battery)
//! always pops up: muting an app can never hide one.
//!
//! The file is `~/.config/yggdrasil/notifications.conf` (Settings > Appearance > Notifications writes it):
//!
//! ```text
//! quiet = on
//! quiet_from = 22:00
//! quiet_to = 07:00
//! muted = spotify, discord
//! ```
//!
//! An app is named by the panel's group key: its desktop entry, else its name, lowercase. The panel lists the apps it has seen
//! notify in `~/.local/state/yggdrasil/notification-apps` (key, a tab, the name shown), so Settings can offer each one.

use std::collections::BTreeSet;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotifyRules {
    pub quiet: bool,
    /// Minutes into the day.
    pub quiet_from: u32,
    pub quiet_to: u32,
    pub muted: BTreeSet<String>,
}

impl Default for NotifyRules {
    fn default() -> Self {
        NotifyRules { quiet: false, quiet_from: 22 * 60, quiet_to: 7 * 60, muted: BTreeSet::new() }
    }
}

/// "22:00" (or "7:30") as minutes into the day.
pub fn parse_time(s: &str) -> Option<u32> {
    let (h, m) = s.trim().split_once(':')?;
    let (h, m) = (h.trim().parse::<u32>().ok()?, m.trim().parse::<u32>().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

pub fn show_time(min: u32) -> String {
    format!("{:02}:{:02}", (min / 60) % 24, min % 60)
}

pub fn path() -> PathBuf {
    crate::dirs::config().join("notifications.conf")
}

pub fn apps_path() -> PathBuf {
    crate::dirs::state().join("notification-apps")
}

impl NotifyRules {
    pub fn parse(text: &str) -> NotifyRules {
        let mut r = NotifyRules::default();
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let v = v.trim();
            match k.trim() {
                "quiet" => r.quiet = matches!(v, "on" | "yes" | "true" | "1"),
                "quiet_from" => r.quiet_from = parse_time(v).unwrap_or(r.quiet_from),
                "quiet_to" => r.quiet_to = parse_time(v).unwrap_or(r.quiet_to),
                "muted" => r.muted = v.split(',').map(|a| a.trim().to_lowercase()).filter(|a| !a.is_empty()).collect(),
                _ => {}
            }
        }
        r
    }

    pub fn to_text(&self) -> String {
        format!(
            "# Which notifications pop up. Edited by Settings (Appearance > Notifications).\nquiet = {}\nquiet_from = {}\nquiet_to = {}\nmuted = {}\n",
            if self.quiet { "on" } else { "off" },
            show_time(self.quiet_from),
            show_time(self.quiet_to),
            self.muted.iter().cloned().collect::<Vec<_>>().join(", ")
        )
    }

    pub fn load() -> NotifyRules {
        std::fs::read_to_string(path()).map(|t| NotifyRules::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self) -> std::io::Result<()> {
        let p = path();
        if let Some(d) = p.parent() {
            std::fs::create_dir_all(d)?;
        }
        std::fs::write(p, self.to_text())
    }

    /// Whether `minute` (into the day) falls in the quiet hours; a stretch past midnight (22:00 to 07:00) works too.
    pub fn quiet_at(&self, minute: u32) -> bool {
        if !self.quiet || self.quiet_from == self.quiet_to {
            return false;
        }
        if self.quiet_from < self.quiet_to {
            (self.quiet_from..self.quiet_to).contains(&minute)
        } else {
            minute >= self.quiet_from || minute < self.quiet_to
        }
    }

    /// Whether a notification of app `key` with `urgency` (2 critical) pops up, with Do not disturb `dnd` and the clock at `minute`.
    pub fn pops_up(&self, key: &str, urgency: u8, dnd: bool, minute: u32) -> bool {
        urgency == 2 || !(dnd || self.quiet_at(minute) || self.muted.contains(&key.to_lowercase()))
    }
}

/// The local time as minutes into the day.
pub fn minute_now() -> u32 {
    unsafe {
        let t = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        // tzset: glibc keeps the zone it first read; this notices a new /etc/localtime (the time zone changed)
        unsafe extern "C" {
            fn tzset();
        }
        tzset();
        libc::localtime_r(&t, &mut tm);
        (tm.tm_hour * 60 + tm.tm_min) as u32
    }
}

/// The apps seen notifying: (key, name), in the order first seen.
pub fn seen_apps() -> Vec<(String, String)> {
    std::fs::read_to_string(apps_path())
        .map(|t| t.lines().filter_map(|l| l.split_once('\t')).map(|(k, n)| (k.to_string(), n.to_string())).collect())
        .unwrap_or_default()
}

/// Notes an app the first time it notifies (at most 200 are kept; the oldest go first).
pub fn remember_app(key: &str, name: &str) {
    let key = key.trim().to_lowercase();
    if key.is_empty() || key.contains(['\t', '\n']) {
        return;
    }
    let mut apps = seen_apps();
    if apps.iter().any(|(k, _)| *k == key) {
        return;
    }
    apps.push((key, name.replace(['\t', '\n'], " ")));
    let start = apps.len().saturating_sub(200);
    let text: String = apps[start..].iter().map(|(k, n)| format!("{k}\t{n}\n")).collect();
    let p = apps_path();
    if let Some(d) = p.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let _ = std::fs::write(p, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_hours_across_midnight_and_within_a_day() {
        let mut r = NotifyRules { quiet: true, ..NotifyRules::default() };
        assert!(r.quiet_at(23 * 60) && r.quiet_at(3 * 60) && !r.quiet_at(7 * 60) && !r.quiet_at(12 * 60) && r.quiet_at(22 * 60));
        (r.quiet_from, r.quiet_to) = (13 * 60, 14 * 60);
        assert!(r.quiet_at(13 * 60 + 30) && !r.quiet_at(14 * 60) && !r.quiet_at(12 * 60));
        r.quiet = false;
        assert!(!r.quiet_at(13 * 60 + 30), "switched off");
    }

    #[test]
    fn critical_always_pops_up_and_muting_is_per_app() {
        let r = NotifyRules { quiet: true, muted: ["spotify".to_string()].into(), ..NotifyRules::default() };
        assert!(!r.pops_up("Spotify", 1, false, 12 * 60), "muted");
        assert!(r.pops_up("vivaldi", 1, false, 12 * 60));
        assert!(!r.pops_up("vivaldi", 1, false, 23 * 60), "quiet hours");
        assert!(!r.pops_up("vivaldi", 1, true, 12 * 60), "do not disturb");
        assert!(r.pops_up("spotify", 2, true, 23 * 60), "critical, whatever else");
    }

    #[test]
    fn the_file_reads_back() {
        let r = NotifyRules {
            quiet: true,
            quiet_from: 21 * 60 + 30,
            quiet_to: 6 * 60,
            muted: ["discord".to_string(), "spotify".to_string()].into(),
        };
        assert_eq!(NotifyRules::parse(&r.to_text()), r);
        assert_eq!(NotifyRules::parse("quiet_from = 25:00\nquiet_to = x\n"), NotifyRules::default(), "bad times keep the defaults");
        assert_eq!((parse_time("7:05"), parse_time("24:00"), show_time(7 * 60 + 5)), (Some(425), None, "07:05".to_string()));
    }
}
