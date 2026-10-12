//! Small formatting helpers shared by the app and the preview.

use std::time::{SystemTime, UNIX_EPOCH};

/// "2026-10-10 13:21" in UTC (no date crate needed for one column).
pub fn datetime(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn formats_known_instants() {
        assert_eq!(datetime(UNIX_EPOCH), "1970-01-01 00:00");
        assert_eq!(datetime(UNIX_EPOCH + Duration::from_secs(1_791_638_519)), "2026-10-10 13:21");
        assert_eq!(datetime(UNIX_EPOCH + Duration::from_secs(951_782_400)), "2000-02-29 00:00"); // leap day
    }
}
