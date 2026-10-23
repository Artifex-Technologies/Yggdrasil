//! Small UTC/civil-date helpers so we do not need a date crate.
//! Open-Meteo returns local times as "YYYY-MM-DDTHH:MM" plus a UTC offset in seconds.

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm).
pub fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// "YYYY-MM-DDTHH:MM" local time at `utc_offset_s` -> Unix epoch seconds.
pub fn parse_local(iso: &str, utc_offset_s: i64) -> Option<i64> {
    let (date, time) = iso.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (d.next()?.parse().ok()?, d.next()?.parse().ok()?, d.next()?.parse().ok()?);
    let mut t = time.split(':');
    let (hh, mm): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || hh > 23 || mm > 59 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86_400 + hh * 3600 + mm * 60 - utc_offset_s)
}

/// Epoch seconds -> local "YYYY-MM-DDTHH:MM" at `utc_offset_s`.
pub fn format_local(epoch: i64, utc_offset_s: i64) -> String {
    let t = epoch + utc_offset_s;
    let (y, m, d) = civil_from_days(t.div_euclid(86_400));
    let s = t.rem_euclid(86_400);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}", s / 3600, s % 3600 / 60)
}

pub fn now_epoch() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(11_017), (2000, 3, 1));
        assert_eq!(civil_from_days(days_from_civil(2026, 10, 10)), (2026, 10, 10));
    }

    #[test]
    fn local_round_trip_with_offset() {
        // 2026-10-10 14:00 at UTC-4 is 18:00 UTC.
        let e = parse_local("2026-10-10T14:00", -4 * 3600).unwrap();
        assert_eq!(format_local(e, 0), "2026-10-10T18:00");
        assert_eq!(format_local(e, -4 * 3600), "2026-10-10T14:00");
    }

    #[test]
    fn rejects_garbage() {
        assert!(parse_local("not a time", 0).is_none());
        assert!(parse_local("2026-13-01T00:00", 0).is_none());
        assert!(parse_local("2026-01-01T25:00", 0).is_none());
    }
}
