//! Month grids for the date popover. Weeks start on Sunday, like the picture it follows.

pub const MONTHS: [&str; 12] = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
pub const DAYS: [&str; 7] = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

/// Day of the week, 0 = Sunday (Sakamoto's method).
pub fn weekday(y: i32, m: u32, d: u32) -> u32 {
    const T: [i32; 12] = [0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4];
    let y = if m < 3 { y - 1 } else { y };
    ((y + y / 4 - y / 100 + y / 400 + T[(m - 1) as usize] + d as i32).rem_euclid(7)) as u32
}

pub fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

/// The month `delta` months from (y, m).
pub fn shift_month(y: i32, m: u32, delta: i32) -> (i32, u32) {
    let idx = y * 12 + (m as i32 - 1) + delta;
    (idx.div_euclid(12), (idx.rem_euclid(12) + 1) as u32)
}

/// Six rows of seven days: (day of the month, belongs to this month). The first and last rows
/// show the end of the month before and the start of the next, which the popover draws dimmer.
pub fn grid(y: i32, m: u32) -> [[(u32, bool); 7]; 6] {
    let lead = weekday(y, m, 1) as i32;
    let (py, pm) = shift_month(y, m, -1);
    let prev_days = days_in_month(py, pm) as i32;
    let this = days_in_month(y, m) as i32;
    let mut g = [[(0u32, false); 7]; 6];
    for (i, cell) in g.iter_mut().flatten().enumerate() {
        let n = i as i32 - lead + 1;
        *cell = if n < 1 {
            ((prev_days + n) as u32, false)
        } else if n > this {
            ((n - this) as u32, false)
        } else {
            (n as u32, true)
        };
    }
    g
}

/// "Sunday, December 19, 2026"; with `short`, "Sunday, Dec 19, 2026" (12-hour times are longer, so the month gives way).
pub fn full_date(y: i32, m: u32, d: u32, short: bool) -> String {
    let month = MONTHS[(m - 1) as usize];
    format!("{}, {} {}, {}", DAYS[weekday(y, m, d) as usize], if short { &month[..3] } else { month }, d, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekdays_are_right() {
        assert_eq!(weekday(2020, 11, 6), 5); // Friday, the picture's date
        assert_eq!(weekday(1993, 12, 19), 0); // a Sunday
        assert_eq!(weekday(2000, 2, 29), 2);
    }

    #[test]
    fn month_lengths_and_shifts() {
        assert_eq!(days_in_month(2024, 2), 29);
        assert_eq!(days_in_month(1900, 2), 28);
        assert_eq!(days_in_month(2000, 2), 29);
        assert_eq!(shift_month(2026, 1, -1), (2025, 12));
        assert_eq!(shift_month(2026, 12, 1), (2027, 1));
        assert_eq!(shift_month(2026, 10, 14), (2027, 12));
    }

    #[test]
    fn the_grid_for_november_2020_matches_the_picture() {
        let g = grid(2020, 11);
        // 1 Nov 2020 was a Sunday: the first row starts with 1 and runs to 7.
        assert_eq!(g[0].map(|c| c.0), [1, 2, 3, 4, 5, 6, 7]);
        assert!(g[0].iter().all(|c| c.1));
        // The last week runs on into December.
        assert_eq!(g[4].map(|c| c.0), [29, 30, 1, 2, 3, 4, 5]);
        assert_eq!(g[4].map(|c| c.1), [true, true, false, false, false, false, false]);
    }

    #[test]
    fn a_month_starting_midweek_shows_the_end_of_the_one_before() {
        let g = grid(2026, 10); // 1 Oct 2026 is a Thursday
        assert_eq!(g[0].map(|c| (c.0, c.1)), [(27, false), (28, false), (29, false), (30, false), (1, true), (2, true), (3, true)]);
    }

    #[test]
    fn full_date_reads_like_the_header() {
        assert_eq!(full_date(2020, 11, 6, false), "Friday, November 6, 2020");
        assert_eq!(full_date(1993, 12, 19, false), "Sunday, December 19, 1993");
        assert_eq!(full_date(1993, 12, 19, true), "Sunday, Dec 19, 1993");
    }
}
