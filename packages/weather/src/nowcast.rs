//! Rain for the next two hours, in 15-minute steps: Open-Meteo's `minutely_15` precipitation
//! (in the US that is NOAA's HRRR, in Europe the regional models; elsewhere it is filled from the
//! hourly global models). Read as text so tests use fixtures.

use crate::openmeteo::FORECAST_URL;
use crate::timeutil::parse_local;
use serde::{Deserialize, Serialize};

/// Below this many mm/h it is not called rain (drizzle that wets nothing).
pub const WET_MM_H: f64 = 0.2;
/// How far ahead the nowcast looks.
pub const STEPS: usize = 8;
const STEP_S: i64 = 900;

pub fn url(lat: f64, lon: f64) -> String {
    format!("{FORECAST_URL}?latitude={lat:.4}&longitude={lon:.4}&minutely_15=precipitation&forecast_minutely_15=12&past_minutely_15=1&timezone=auto")
}

/// One 15-minute step: when it starts (epoch seconds) and the rate in mm/h.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    pub start: i64,
    pub mm_h: f64,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    utc_offset_seconds: i64,
    minutely_15: Option<Minutely>,
}

#[derive(Deserialize)]
struct Minutely {
    time: Vec<String>,
    precipitation: Vec<Option<f64>>,
}

/// The steps of a `minutely_15` reply. Open-Meteo gives millimetres per 15 minutes; they are
/// returned as mm/h. Missing values are dropped.
pub fn parse(text: &str) -> Result<Vec<Step>, String> {
    let r: Raw = serde_json::from_str(text).map_err(|e| format!("cannot parse Open-Meteo nowcast: {e}"))?;
    let m = r.minutely_15.ok_or("reply has no minutely_15 data")?;
    Ok(m.time
        .iter()
        .zip(m.precipitation.iter())
        .filter_map(|(t, p)| Some(Step { start: parse_local(t, r.utc_offset_seconds)?, mm_h: (*p)? * 4.0 }))
        .collect())
}

/// What the next two hours hold, seen from `now`.
#[derive(Debug, Clone, PartialEq)]
pub struct Outlook {
    /// From the step that contains `now`, at most `STEPS` of them.
    pub steps: Vec<Step>,
    pub raining_now: bool,
    pub now_mm_h: f64,
    /// Not raining now: minutes until the first wet step.
    pub starts_in: Option<i64>,
    /// Raining now: minutes until the first dry step.
    pub stops_in: Option<i64>,
    /// The heaviest rate in the window.
    pub peak_mm_h: f64,
}

pub fn outlook(steps: &[Step], now: i64) -> Option<Outlook> {
    let first = steps.iter().position(|s| s.start <= now && now < s.start + STEP_S).or_else(|| steps.iter().position(|s| s.start > now))?;
    let steps: Vec<Step> = steps[first..].iter().take(STEPS).copied().collect();
    let now_mm_h = steps.first()?.mm_h;
    let raining_now = now_mm_h >= WET_MM_H;
    let mins = |s: &Step| ((s.start - now).max(0) + 59) / 60;
    let starts_in = if raining_now { None } else { steps.iter().find(|s| s.mm_h >= WET_MM_H).map(mins) };
    let stops_in = if raining_now { steps.iter().find(|s| s.mm_h < WET_MM_H).map(mins) } else { None };
    let peak_mm_h = steps.iter().map(|s| s.mm_h).fold(0.0, f64::max);
    Some(Outlook { steps, raining_now, now_mm_h, starts_in, stops_in, peak_mm_h })
}

/// The usual meteorological bands for rain rate.
pub fn intensity(mm_h: f64) -> &'static str {
    match mm_h {
        x if x < WET_MM_H => "dry",
        x if x < 2.5 => "light rain",
        x if x < 7.6 => "moderate rain",
        x if x < 50.0 => "heavy rain",
        _ => "violent rain",
    }
}

/// "20 min", "1 h 05 min".
pub fn minutes(m: i64) -> String {
    if m < 60 { format!("{m} min") } else { format!("{} h {:02} min", m / 60, m % 60) }
}

/// What `watch` remembers between runs so one shower is announced once.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct WatchState {
    /// When the last announced rain was expected to start (epoch seconds).
    #[serde(default)]
    pub announced_onset: Option<i64>,
}

/// Announce rain starting within this many minutes.
pub const WARN_MIN: i64 = 30;

/// The notification to send now, if any: (title, body). Updates `st`.
/// A shower is the same one while its expected start stays within 45 minutes of the announced
/// one, so forecasts that shift by a step or two do not announce it again; once that start has
/// passed by an hour, the next rain is a new shower.
pub fn decide(o: &Outlook, now: i64, st: &mut WatchState) -> Option<(String, String)> {
    if let Some(a) = st.announced_onset {
        if now > a + 3600 && !o.raining_now {
            st.announced_onset = None;
        }
    }
    let m = o.starts_in?;
    if m > WARN_MIN {
        return None;
    }
    let onset = now + m * 60;
    if st.announced_onset.is_some_and(|a| (onset - a).abs() <= 45 * 60) {
        return None;
    }
    st.announced_onset = Some(onset);
    let title = if m <= 5 { "Rain starting now".to_string() } else { format!("Rain in about {}", minutes(m)) };
    let mut body = format!("{} expected, up to {:.1} mm/h", capital(intensity(o.peak_mm_h)), o.peak_mm_h);
    if let Some(dry) = o.steps.iter().skip_while(|s| s.mm_h < WET_MM_H).find(|s| s.mm_h < WET_MM_H) {
        body.push_str(&format!("; dry again in about {}", minutes(((dry.start - now).max(0) + 59) / 60)));
    }
    body.push('.');
    Some((title, body))
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIX: &str = include_str!("../tests/fixtures/minutely15.json");
    // 2026-10-25T14:00 at UTC-4 (the fixture's offset)
    fn t(hh: i64, mm: i64) -> i64 {
        parse_local(&format!("2026-10-25T{hh:02}:{mm:02}"), -4 * 3600).unwrap()
    }

    #[test]
    fn parses_mm_per_15_min_as_mm_per_hour() {
        let s = parse(FIX).unwrap();
        assert_eq!(s.len(), 12, "the null step is dropped");
        assert_eq!(s[0], Step { start: t(13, 45), mm_h: 0.0 });
        assert!((s[3].mm_h - 0.8).abs() < 1e-9);
        assert!(parse(r#"{"utc_offset_seconds":0}"#).is_err());
    }

    #[test]
    fn rain_starting_soon_is_found() {
        let s = parse(FIX).unwrap();
        let o = outlook(&s, t(14, 5)).unwrap();
        assert!(!o.raining_now);
        assert_eq!(o.steps[0].start, t(14, 0));
        assert_eq!(o.starts_in, Some(25)); // the 14:30 step
        assert_eq!(o.stops_in, None);
        assert!((o.peak_mm_h - 6.0).abs() < 1e-9);
        assert_eq!(o.steps.len(), STEPS);
    }

    #[test]
    fn raining_now_reports_when_it_stops() {
        let s = parse(FIX).unwrap();
        let o = outlook(&s, t(14, 50)).unwrap();
        assert!(o.raining_now);
        assert_eq!(o.starts_in, None);
        assert_eq!(o.stops_in, Some(55)); // the 15:45 step is dry
    }

    #[test]
    fn one_shower_is_announced_once() {
        let s = parse(FIX).unwrap();
        let mut st = WatchState::default();
        let (title, body) = decide(&outlook(&s, t(14, 5)).unwrap(), t(14, 5), &mut st).unwrap();
        assert_eq!(title, "Rain in about 25 min");
        assert!(body.starts_with("Moderate rain expected, up to 6.0 mm/h; dry again in about 1 h 40 min"), "{body}");
        // ten minutes later the same shower is still coming: no second notice
        assert_eq!(decide(&outlook(&s, t(14, 15)).unwrap(), t(14, 15), &mut st), None);
        // a dry outlook far later forgets it
        let dry = vec![Step { start: t(18, 0), mm_h: 0.0 }];
        assert_eq!(decide(&outlook(&dry, t(18, 5)).unwrap(), t(18, 5), &mut st), None);
        assert_eq!(st.announced_onset, None);
    }

    #[test]
    fn far_or_no_rain_is_quiet() {
        let s = parse(FIX).unwrap();
        let mut st = WatchState::default();
        // from 13:45 the rain is 45 minutes away: more than WARN_MIN
        assert_eq!(decide(&outlook(&s, t(13, 45)).unwrap(), t(13, 45), &mut st), None);
        assert_eq!(intensity(0.1), "dry");
        assert_eq!(intensity(1.0), "light rain");
        assert_eq!(minutes(65), "1 h 05 min");
    }
}
