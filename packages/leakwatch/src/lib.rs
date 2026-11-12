//! The leak watch: samples memory, keeps a day of history per program, and finds the ones that keep growing.
//!
//! Two kinds of series are kept:
//! - a **process**: its resident memory plus what of it is in swap (`VmRSS` + `VmSwap` from `/proc/PID/status`, readable for
//!   every process), so a leak that is pushed out to swap still counts;
//! - **buffers**: the shared memory a compositor (Hyprland) holds for one program's images, from the compositor's
//!   `/proc/PID/smaps`, grouped by the memfd name the program gave (`memfd:yggdrasil-panel`). This is where the panel's leak
//!   of 2026-11-09 hid: the panel itself stayed small while Hyprland held 819 of its old images, about 4 GB.
//!
//! A series is suspect when, over at least four hours, its hourly lows climb steadily and the last hour's low is well above
//! the first one's. Hourly lows ignore short peaks (a program that loads something and lets it go again is not leaking).
//! Nothing here writes outside the state file; nothing is ever stopped or restarted.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Programs below this are not followed until they grow past it (a leak gets there soon enough).
pub const FLOOR_KB: u64 = 20 * 1024;
/// A compositor's buffers for one program below this are not followed.
pub const BUFFER_FLOOR_KB: u64 = 8 * 1024;
/// How much history is kept.
pub const KEEP_SECS: u64 = 26 * 3600;
/// The shortest history a verdict needs.
pub const MIN_SPAN_SECS: u64 = 4 * 3600;
/// Compositors whose shared buffers are looked at (their smaps must be readable, so the person's own).
pub const COMPOSITORS: &[&str] = &["Hyprland", "sway", "yggdrasil-compo"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Process,
    Buffers,
}

/// One program's (or one program's buffers') history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Series {
    pub name: String,
    pub kind: Kind,
    pub pid: u32,
    /// For buffers: the program that made them (the memfd name); the compositor is `name`'s holder.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub holder: String,
    /// (unix seconds, kB)
    pub points: Vec<(u64, u64)>,
    /// When a warning went out for this series (unix seconds), so it is not repeated within a day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warned: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    pub version: u32,
    pub series: BTreeMap<String, Series>,
}

/// One reading of one series.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    pub key: String,
    pub name: String,
    pub kind: Kind,
    pub pid: u32,
    pub holder: String,
    pub kb: u64,
}

/// `VmRSS` + `VmSwap` and the name, from a `/proc/PID/status` text. None for kernel threads (no VmRSS).
pub fn parse_status(text: &str) -> Option<(String, u64)> {
    let mut name = None;
    let mut rss = None;
    let mut swap = 0;
    for line in text.lines() {
        let (k, v) = match line.split_once(':') {
            Some(x) => x,
            None => continue,
        };
        let kb = || v.trim().trim_end_matches("kB").trim().parse::<u64>().ok();
        match k {
            "Name" => name = Some(v.trim().to_string()),
            "VmRSS" => rss = kb(),
            "VmSwap" => swap = kb().unwrap_or(0),
            _ => {}
        }
    }
    Some((name?, rss? + swap))
}

/// The start time (clock ticks since boot) from a `/proc/PID/stat` text: field 22, counted after the name in parentheses
/// (which may itself hold spaces and parentheses). With the pid it tells a restarted program from the old one.
pub fn parse_start(stat: &str) -> Option<u64> {
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// The shared memory a compositor holds per memfd name, in kB, from its `/proc/PID/smaps` text.
pub fn parse_smaps_memfd(smaps: &str) -> BTreeMap<String, u64> {
    let mut out = BTreeMap::new();
    let mut current: Option<String> = None;
    for line in smaps.lines() {
        let first = line.split_whitespace().next().unwrap_or("");
        if first.contains('-') && first.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            // a mapping header: "start-end perms offset dev inode   path"
            current = line.split_once("/memfd:").map(|(_, n)| n.trim_end_matches(" (deleted)").trim().to_string());
        } else if let (Some(name), Some(v)) = (&current, line.strip_prefix("Rss:")) {
            let kb: u64 = v.trim().trim_end_matches("kB").trim().parse().unwrap_or(0);
            *out.entry(name.clone()).or_insert(0) += kb;
        }
    }
    out
}

/// Every reading under a proc root (`/proc`, or a folder laid out the same way in tests).
pub fn scan(root: &Path) -> Vec<Reading> {
    let mut out = vec![];
    let Ok(dir) = std::fs::read_dir(root) else { return out };
    for e in dir.flatten() {
        let file_name = e.file_name();
        let Some(pid) = file_name.to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let p = e.path();
        let Ok(status) = std::fs::read_to_string(p.join("status")) else { continue };
        let Some((name, kb)) = parse_status(&status) else { continue };
        let start = std::fs::read_to_string(p.join("stat")).ok().and_then(|s| parse_start(&s)).unwrap_or(0);
        out.push(Reading { key: format!("p:{pid}:{start}"), name: name.clone(), kind: Kind::Process, pid, holder: String::new(), kb });
        if COMPOSITORS.contains(&name.as_str()) {
            if let Ok(smaps) = std::fs::read_to_string(p.join("smaps")) {
                for (prog, kb) in parse_smaps_memfd(&smaps) {
                    out.push(Reading {
                        key: format!("b:{pid}:{start}:{prog}"),
                        name: prog,
                        kind: Kind::Buffers,
                        pid,
                        holder: name.clone(),
                        kb,
                    });
                }
            }
        }
    }
    out
}

impl State {
    /// Adds one sample taken at `now`: followed series get a point, new ones start when they pass their floor, and series
    /// whose program is gone (or restarted: a new key) are dropped with their history.
    pub fn record(&mut self, now: u64, readings: &[Reading]) {
        self.version = 1;
        let seen: std::collections::BTreeSet<&str> = readings.iter().map(|r| r.key.as_str()).collect();
        self.series.retain(|k, _| seen.contains(k.as_str()));
        for r in readings {
            let floor = if r.kind == Kind::Buffers { BUFFER_FLOOR_KB } else { FLOOR_KB };
            match self.series.get_mut(&r.key) {
                Some(s) => s.points.push((now, r.kb)),
                None if r.kb >= floor => {
                    self.series.insert(
                        r.key.clone(),
                        Series {
                            name: r.name.clone(),
                            kind: r.kind,
                            pid: r.pid,
                            holder: r.holder.clone(),
                            points: vec![(now, r.kb)],
                            warned: None,
                        },
                    );
                }
                None => {}
            }
        }
        for s in self.series.values_mut() {
            s.points.retain(|(t, _)| now.saturating_sub(*t) <= KEEP_SECS);
        }
    }
}

/// What the history of one series says.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub first_kb: u64,
    pub last_kb: u64,
    pub hours: f64,
    pub per_hour_kb: f64,
    pub suspect: bool,
}

/// The lowest value in each hour from the first point, in order (hours with no point are skipped).
pub fn hourly_lows(points: &[(u64, u64)]) -> Vec<u64> {
    let Some(&(t0, _)) = points.first() else { return vec![] };
    let mut lows: BTreeMap<u64, u64> = BTreeMap::new();
    for &(t, kb) in points {
        let h = (t - t0) / 3600;
        lows.entry(h).and_modify(|v| *v = (*v).min(kb)).or_insert(kb);
    }
    lows.into_values().collect()
}

/// Suspect: at least four hours of history; the last hour's low at least `min_growth_kb` and half again above the first
/// hour's low; and the hourly lows climbing (or holding) in at least three steps out of four.
pub fn judge(s: &Series) -> Option<Verdict> {
    let (&(t0, _), &(t1, _)) = (s.points.first()?, s.points.last()?);
    let lows = hourly_lows(&s.points);
    let (first, last) = (*lows.first()?, *lows.last()?);
    let hours = (t1 - t0) as f64 / 3600.0;
    let growth = last.saturating_sub(first);
    let per_hour_kb = if hours > 0.0 { growth as f64 / hours } else { 0.0 };
    let min_growth_kb = if s.kind == Kind::Buffers { 128 * 1024 } else { 256 * 1024 };
    let steps = lows.windows(2).count();
    // "holding": within 1 % or 4 MB of the hour before
    let climbing = lows.windows(2).filter(|w| w[1] + (w[0] / 100).max(4096) >= w[0]).count();
    let suspect = t1 - t0 >= MIN_SPAN_SECS
        && lows.len() >= 5
        && growth >= min_growth_kb
        && last as f64 >= first as f64 * 1.5
        && climbing * 4 >= steps * 3;
    Some(Verdict { first_kb: first, last_kb: last, hours, per_hour_kb, suspect })
}

/// Megabytes or gigabytes, for people.
pub fn human(kb: u64) -> String {
    if kb >= 1024 * 1024 {
        format!("{:.1} GB", kb as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} MB", (kb + 512) / 1024)
    }
}

/// The warning for a suspect series: (title, body).
pub fn warning(s: &Series, v: &Verdict) -> (String, String) {
    let span = format!("{:.0} hours", v.hours.max(1.0));
    match s.kind {
        Kind::Process => (
            format!("Memory keeps growing: {}", s.name),
            format!(
                "{} went from {} to {} over {span} and has not come back down. Restarting it gives the memory back. Details: yggdrasil-leakwatch report",
                s.name,
                human(v.first_kb),
                human(v.last_kb)
            ),
        ),
        Kind::Buffers => (
            format!("{} is not giving back its images", s.name),
            format!(
                "{} holds {} of images from {} ({} {span} ago). A program that drops images without saying so leaves them there; restarting {} gives the memory back. Details: yggdrasil-leakwatch report",
                s.holder,
                human(v.last_kb),
                s.name,
                human(v.first_kb),
                s.name
            ),
        ),
    }
}

/// Names never warned about, from `ignore = a, b` lines of the settings file.
pub fn ignored(conf: &str) -> Vec<String> {
    conf.lines()
        .filter_map(|l| l.split('#').next()?.split_once('='))
        .filter(|(k, _)| k.trim() == "ignore")
        .flat_map(|(_, v)| v.split(',').map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).collect::<Vec<_>>())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str =
        "Name:\tyggdrasil-panel\nUmask:\t0022\nState:\tS (sleeping)\nVmRSS:\t  121360 kB\nRssAnon:\t   40000 kB\nVmSwap:\t     512 kB\n";

    #[test]
    fn status_gives_name_and_rss_plus_swap() {
        assert_eq!(parse_status(STATUS), Some(("yggdrasil-panel".into(), 121872)));
        assert_eq!(parse_status("Name:\tkworker/0:1\nState:\tI (idle)\n"), None, "a kernel thread has no VmRSS");
    }

    #[test]
    fn start_time_survives_a_name_with_spaces_and_parentheses() {
        let stat = "597693 (my (odd) prog) S 1 597693 597693 0 -1 4194560 1 0 0 0 5 3 0 0 20 0 9 0 3141592 100000 2000";
        assert_eq!(parse_start(stat), Some(3141592));
    }

    #[test]
    fn smaps_groups_the_compositors_memfd_memory_by_program() {
        let smaps = "\
7f00-7f10 rw-s 00000000 00:01 1   /memfd:yggdrasil-panel (deleted)
Size:   19800 kB
Rss:    19800 kB
7f10-7f20 rw-s 00000000 00:01 2   /memfd:yggdrasil-panel (deleted)
Rss:     4096 kB
7f20-7f30 rw-s 00000000 00:01 3   /memfd:wayland-shm (deleted)
Rss:      100 kB
7f30-7f40 r-xp 00000000 08:01 4   /usr/bin/Hyprland
Rss:     8000 kB
7f40-7f50 rw-p 00000000 00:00 0
Rss:      300 kB
";
        let m = parse_smaps_memfd(smaps);
        assert_eq!(m.get("yggdrasil-panel"), Some(&23896));
        assert_eq!(m.get("wayland-shm"), Some(&100));
        assert_eq!(m.len(), 2, "files and anonymous memory are not buffers");
    }

    fn series(kind: Kind, pts: impl IntoIterator<Item = (u64, u64)>) -> Series {
        Series { name: "x".into(), kind, pid: 1, holder: "Hyprland".into(), points: pts.into_iter().collect(), warned: None }
    }

    #[test]
    fn the_panel_leak_of_2026_11_09_is_caught() {
        // 819 buffers, about 4 GB, in 35 hours: about 115 MB an hour, steadily; sampled every 5 minutes for 6 hours
        let pts = (0..=72).map(|i| (i * 300, 150 * 1024 + i * 300 * 115 * 1024 / 3600));
        let v = judge(&series(Kind::Buffers, pts)).unwrap();
        assert!(v.suspect, "{v:?}");
        assert!(v.per_hour_kb > 100.0 * 1024.0);
    }

    #[test]
    fn a_busy_but_steady_program_is_not_a_leak() {
        // swings between 300 MB and 1.2 GB (a game loading levels), lows flat
        let pts = (0..=96).map(|i| (i * 300, if i % 6 == 0 { 300 * 1024 } else { 1200 * 1024 }));
        assert!(!judge(&series(Kind::Process, pts)).unwrap().suspect);
    }

    #[test]
    fn slow_growth_below_the_bar_is_not_a_leak() {
        // 20 MB an hour for 6 hours: 120 MB, under 256 MB
        let pts = (0..=72).map(|i| (i * 300, 200 * 1024 + i * 300 * 20 * 1024 / 3600));
        assert!(!judge(&series(Kind::Process, pts)).unwrap().suspect);
    }

    #[test]
    fn too_little_history_gives_no_verdict_yet() {
        let pts = (0..=24).map(|i| (i * 300, 100 * 1024 + i * 50 * 1024));
        assert!(!judge(&series(Kind::Process, pts)).unwrap().suspect, "two hours is not enough");
    }

    #[test]
    fn growth_that_went_back_down_is_not_a_leak() {
        // up for four hours, then back to where it started
        let mut pts: Vec<(u64, u64)> = (0..=48).map(|i| (i * 300, 200 * 1024 + i * 20 * 1024)).collect();
        pts.extend((49..=84).map(|i| (i * 300, 200 * 1024)));
        assert!(!judge(&series(Kind::Process, pts)).unwrap().suspect);
    }

    fn reading(key: &str, kind: Kind, kb: u64) -> Reading {
        Reading { key: key.into(), name: key.into(), kind, pid: 1, holder: String::new(), kb }
    }

    #[test]
    fn record_follows_big_ones_drops_the_gone_and_restarts_fresh() {
        let mut st = State::default();
        st.record(
            0,
            &[
                reading("p:1:10", Kind::Process, 30 * 1024),
                reading("p:2:10", Kind::Process, 1024),
                reading("b:9:1:panel", Kind::Buffers, 9 * 1024),
            ],
        );
        assert!(st.series.contains_key("p:1:10") && st.series.contains_key("b:9:1:panel"));
        assert!(!st.series.contains_key("p:2:10"), "small programs are not followed");
        // pid 1 restarted (new start time): the old history goes, the new one starts
        st.record(300, &[reading("p:1:99", Kind::Process, 30 * 1024), reading("b:9:1:panel", Kind::Buffers, 9 * 1024)]);
        assert!(!st.series.contains_key("p:1:10") && st.series.contains_key("p:1:99"));
        assert_eq!(st.series["b:9:1:panel"].points.len(), 2);
        // history older than a day and two hours is let go
        st.record(KEEP_SECS + 300, &[reading("b:9:1:panel", Kind::Buffers, 9 * 1024)]);
        assert_eq!(st.series["b:9:1:panel"].points, vec![(300, 9 * 1024), (KEEP_SECS + 300, 9 * 1024)]);
    }

    #[test]
    fn scan_reads_a_proc_tree() {
        let root = std::env::temp_dir().join(format!("leakwatch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (pid, status, stat, smaps) in [
            ("100", STATUS, "100 (yggdrasil-panel) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 555 0 0", None),
            (
                "200",
                "Name:\tHyprland\nVmRSS:\t500000 kB\n",
                "200 (Hyprland) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 777 0 0",
                Some("7f-80 rw-s 0 00:01 1 /memfd:yggdrasil-panel (deleted)\nRss:\t9000 kB\n"),
            ),
            ("self", STATUS, "", None),
        ] {
            let d = root.join(pid);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("status"), status).unwrap();
            std::fs::write(d.join("stat"), stat).unwrap();
            if let Some(s) = smaps {
                std::fs::write(d.join("smaps"), s).unwrap();
            }
        }
        let mut r = scan(&root);
        r.sort_by(|a, b| a.key.cmp(&b.key));
        let keys: Vec<&str> = r.iter().map(|r| r.key.as_str()).collect();
        assert_eq!(keys, vec!["b:200:777:yggdrasil-panel", "p:100:555", "p:200:777"], "non-numeric entries are skipped");
        assert_eq!(r[0].holder, "Hyprland");
        assert_eq!(r[0].kb, 9000);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_warning_names_who_holds_what() {
        let mut s = series(Kind::Buffers, [(0, 150 * 1024), (6 * 3600, 900 * 1024)]);
        s.name = "yggdrasil-panel".into();
        let v = judge(&s).unwrap();
        let (title, body) = warning(&s, &v);
        assert_eq!(title, "yggdrasil-panel is not giving back its images");
        assert!(body.starts_with("Hyprland holds 900 MB of images from yggdrasil-panel"), "{body}");
    }

    #[test]
    fn ignore_lines() {
        assert_eq!(
            ignored("# comment\nignore = vivaldi-bin, code\nignore=ollama # big model\nother = x"),
            vec!["vivaldi-bin", "code", "ollama"]
        );
    }

    #[test]
    fn sizes_for_people() {
        assert_eq!(human(121360), "119 MB");
        assert_eq!(human(4 * 1024 * 1024), "4.0 GB");
    }
}
