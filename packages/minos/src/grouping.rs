//! Group by (the command bar's Group menu): the list cut into headed groups by type, by when it was changed, or by size, as
//! Windows Explorer and the Files app group a folder. The items keep the order they are sorted in within each group.

use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::listing::{self, Entry};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum GroupBy {
    #[default]
    None,
    Type,
    Modified,
    Size,
}

impl GroupBy {
    pub const ALL: [GroupBy; 4] = [GroupBy::None, GroupBy::Type, GroupBy::Modified, GroupBy::Size];

    pub fn label(self) -> &'static str {
        match self {
            GroupBy::None => "None",
            GroupBy::Type => "Type",
            GroupBy::Modified => "Date modified",
            GroupBy::Size => "Size",
        }
    }
}

/// The list in its groups: `order` (indices into the entries) rearranged group after group, and where each group starts in it
/// with its heading. Not grouped: `order` as it was and no headings.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Arranged {
    pub order: Vec<usize>,
    pub heads: Vec<(usize, String)>,
}

impl Arranged {
    /// The heading of the group starting at position `pos` of `order`, if one does.
    pub fn head_at(&self, pos: usize) -> Option<&str> {
        self.heads.iter().find(|(p, _)| *p == pos).map(|(_, h)| h.as_str())
    }
}

/// Days since 1970-01-01 in the computer's time zone (`offset`: seconds east of UTC).
fn local_day(t: SystemTime, offset: i64) -> Option<i64> {
    let secs = match t.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(e) => -(e.duration().as_secs() as i64),
    };
    Some((secs + offset).div_euclid(86_400))
}

/// (year, month) of a day number (the civil calendar).
fn year_month(day: i64) -> (i64, i64) {
    let z = day + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (yoe + era * 400 + i64::from(m <= 2), m)
}

/// The group of a change time, as Windows names them; the number orders the groups, newest first.
fn when(modified: Option<SystemTime>, now: SystemTime, offset: i64) -> (u8, &'static str) {
    let (Some(day), Some(today)) = (modified.and_then(|m| local_day(m, offset)), local_day(now, offset)) else {
        return (9, "No date");
    };
    // Monday is a week's first day; 1970-01-01 was a Thursday
    let week_start = |d: i64| d - (d + 3).rem_euclid(7);
    let ((y, m), (ty, tm)) = (year_month(day), year_month(today));
    let months = (ty * 12 + tm) - (y * 12 + m);
    if day >= today {
        (0, "Today")
    } else if day == today - 1 {
        (1, "Yesterday")
    } else if day >= week_start(today) {
        (2, "Earlier this week")
    } else if day >= week_start(today) - 7 {
        (3, "Last week")
    } else if months == 0 {
        (4, "Earlier this month")
    } else if months == 1 {
        (5, "Last month")
    } else if y == ty {
        (6, "Earlier this year")
    } else {
        (7, "A long time ago")
    }
}

/// A file's size group, as Windows names them (folders are a group of their own, first).
fn size_group(e: &Entry) -> (u8, &'static str) {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    match e.size {
        _ if e.is_dir => (0, "Folders"),
        0 => (1, "Empty"),
        s if s <= 16 * KB => (2, "Tiny (up to 16 KB)"),
        s if s <= MB => (3, "Small (16 KB to 1 MB)"),
        s if s <= 128 * MB => (4, "Medium (1 to 128 MB)"),
        s if s <= GB => (5, "Large (128 MB to 1 GB)"),
        s if s <= 4 * GB => (6, "Huge (1 to 4 GB)"),
        _ => (7, "Gigantic (over 4 GB)"),
    }
}

/// `order` cut into groups by `by`. `now` and `offset` (seconds east of UTC) place Today and the other dates.
pub fn arrange(entries: &[Entry], order: &[usize], by: GroupBy, now: SystemTime, offset: i64) -> Arranged {
    if by == GroupBy::None || order.is_empty() {
        return Arranged { order: order.to_vec(), heads: vec![] };
    }
    // (rank, heading) for each item, then a stable sort by it: the sorted order stays within each group
    let keyed: Vec<((u8, String), usize)> = order
        .iter()
        .map(|&i| {
            let e = &entries[i];
            let key = match by {
                GroupBy::Type if e.is_dir => (0, "Folders".to_string()),
                GroupBy::Type => (1, listing::type_name(e)),
                GroupBy::Modified => {
                    let (r, h) = when(e.modified, now, offset);
                    (r, h.to_string())
                }
                GroupBy::Size => {
                    let (r, h) = size_group(e);
                    (r, h.to_string())
                }
                GroupBy::None => unreachable!(),
            };
            (key, i)
        })
        .collect();
    let mut sorted = keyed;
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = Arranged::default();
    for (pos, (key, i)) in sorted.into_iter().enumerate() {
        if out.heads.last().is_none_or(|(_, h)| *h != key.1) {
            out.heads.push((pos, key.1));
        }
        out.order.push(i);
    }
    out
}

/// The computer's time zone now, in seconds east of UTC (0 when it cannot be read).
pub fn utc_offset() -> i64 {
    let now = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs() as libc::time_t).unwrap_or(0);
    // SAFETY: localtime_r fills the tm given; both live on this stack
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return 0;
    }
    tm.tm_gmtoff as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    const DAY: u64 = 86_400;

    fn file(name: &str, size: u64, days_ago: Option<u64>, now: SystemTime) -> Entry {
        Entry {
            name: name.into(),
            path: PathBuf::from("/d").join(name),
            is_dir: false,
            size,
            modified: days_ago.map(|d| now - Duration::from_secs(d * DAY)),
            hidden: false,
            usage: None,
        }
    }

    fn folder(name: &str, now: SystemTime) -> Entry {
        Entry { is_dir: true, size: 0, ..file(name, 0, Some(0), now) }
    }

    fn heads(a: &Arranged) -> Vec<&str> {
        a.heads.iter().map(|(_, h)| h.as_str()).collect()
    }

    // Wednesday 2026-11-25 12:00 UTC
    fn wednesday() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_608_000)
    }

    #[test]
    fn none_keeps_the_order_and_has_no_headings() {
        let now = wednesday();
        let e = vec![file("b", 1, Some(0), now), file("a", 1, Some(0), now)];
        let a = arrange(&e, &[1, 0], GroupBy::None, now, 0);
        assert_eq!(a, Arranged { order: vec![1, 0], heads: vec![] });
    }

    #[test]
    fn by_type_folders_first_then_each_kind_keeping_the_sort() {
        let now = wednesday();
        let e = vec![
            file("z.png", 1, Some(0), now),
            folder("Music", now),
            file("a.txt", 1, Some(0), now),
            file("b.png", 1, Some(0), now),
            file("README", 1, Some(0), now),
        ];
        // sorted by name descending, say: the groups keep that order inside them
        let a = arrange(&e, &[0, 3, 2, 4, 1], GroupBy::Type, now, 0);
        assert_eq!(heads(&a), ["Folders", "File", "PNG file", "TXT file"]);
        assert_eq!(a.order, [1, 4, 0, 3, 2]);
        assert_eq!(a.heads.iter().map(|(p, _)| *p).collect::<Vec<_>>(), [0, 1, 2, 4]);
        assert_eq!(a.head_at(2), Some("PNG file"));
        assert_eq!(a.head_at(3), None);
    }

    #[test]
    fn by_date_like_windows_newest_first() {
        let now = wednesday();
        let ages = [0, 1, 2, 5, 9, 20, 40, 120, 400];
        let mut e: Vec<Entry> = ages.iter().map(|&d| file(&format!("f{d}"), 1, Some(d), now)).collect();
        e.push(file("undated", 1, None, now));
        let order: Vec<usize> = (0..e.len()).rev().collect();
        let a = arrange(&e, &order, GroupBy::Modified, now, 0);
        assert_eq!(
            heads(&a),
            [
                "Today",
                "Yesterday",
                "Earlier this week", // Monday (2 days ago)
                "Last week",         // 5 and 9 days ago: the week from Monday 2026-11-16
                "Earlier this month",
                "Last month",
                "Earlier this year",
                "A long time ago",
                "No date"
            ]
        );
        let last_week: Vec<&str> = a.order[a.heads[3].0..a.heads[4].0].iter().map(|&i| e[i].name.as_str()).collect();
        assert_eq!(last_week, ["f9", "f5"], "within a group the given order stays");
    }

    #[test]
    fn the_time_zone_decides_where_today_begins() {
        // 2026-11-25 00:30 UTC is still the 24th six hours west of UTC
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_795_566_600);
        let late = file("late", 1, None, now);
        let e = vec![Entry { modified: Some(now - Duration::from_secs(3600)), ..late }];
        assert_eq!(heads(&arrange(&e, &[0], GroupBy::Modified, now, 0)), ["Yesterday"]);
        assert_eq!(heads(&arrange(&e, &[0], GroupBy::Modified, now, -6 * 3600)), ["Today"]);
    }

    #[test]
    fn by_size_in_windows_bands() {
        let now = wednesday();
        let e = vec![
            file("big", 5 << 30, Some(0), now),
            file("empty", 0, Some(0), now),
            folder("dir", now),
            file("tiny", 100, Some(0), now),
            file("song", 4 << 20, Some(0), now),
            file("small", 200 << 10, Some(0), now),
        ];
        let a = arrange(&e, &[0, 1, 2, 3, 4, 5], GroupBy::Size, now, 0);
        assert_eq!(
            heads(&a),
            ["Folders", "Empty", "Tiny (up to 16 KB)", "Small (16 KB to 1 MB)", "Medium (1 to 128 MB)", "Gigantic (over 4 GB)"]
        );
    }
}
