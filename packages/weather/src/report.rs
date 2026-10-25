//! Turn the consensus into what the bar and terminal show.

use crate::consensus::Hour;
use crate::observe::Station;
use crate::nowcast::Outlook;
use crate::timeutil::{format_local, now_epoch};
use serde_json::json;

pub fn to_units(c: f64, units: &str) -> f64 {
    if units == "us" { c * 9.0 / 5.0 + 32.0 } else { c }
}

fn deg(c: f64, units: &str) -> String {
    format!("{:.0}\u{00B0}", to_units(c, units))
}

/// Index of the hour to treat as "now": the first forecast hour not before this local hour.
pub fn current_index(hours: &[Hour], utc_offset_s: i64, now: i64) -> Option<usize> {
    let key = format_local(now, utc_offset_s);
    let key = &key[..13]; // "YYYY-MM-DDTHH"
    hours.iter().position(|h| &h.time[..13.min(h.time.len())] >= key)
}

/// Rough icon from rain chance and time of day. A heuristic: we have no cloud-cover data.
pub fn icon(precip: Option<f64>, hour_of_day: u32) -> &'static str {
    match precip {
        Some(p) if p >= 60.0 => "\u{1F327}",
        Some(p) if p >= 30.0 => "\u{26C5}",
        _ if !(6..20).contains(&hour_of_day) => "\u{1F319}",
        _ => "\u{2600}",
    }
}

pub struct BarInput<'a> {
    pub hours: &'a [Hour],
    pub utc_offset_s: i64,
    pub units: &'a str,
    pub place: &'a str,
    /// How the place was found ("chosen", "from your time zone (rough)", "from Wi-Fi nearby").
    pub how: &'a str,
    pub observed: Option<(&'a Station, f64)>,
    pub weighted: bool,
}

/// One line of Waybar JSON.
pub fn bar_json(i: &BarInput) -> String {
    let Some(idx) = current_index(i.hours, i.utc_offset_s, now_epoch()) else {
        return json!({"text": "", "tooltip": "no forecast data"}).to_string();
    };
    let h = &i.hours[idx];
    let hod: u32 = h.time.get(11..13).and_then(|s| s.parse().ok()).unwrap_or(12);
    let text = format!("{} {}", icon(h.precip_prob, hod), deg(h.mean, i.units));

    let mut tip = vec![format!("{}", i.place)];
    tip.push(format!(
        "Now: {} (models {} to {}, {} of them)",
        deg(h.mean, i.units),
        deg(h.low, i.units),
        deg(h.high, i.units),
        h.n_models
    ));
    // Only the WIDTH of the ensemble is shown: it is a different model run than the three
    // above, so its absolute range can sit off to one side of their average.
    if let Some((p10, _, p90)) = h.ensemble {
        let half = (p90 - p10) / 2.0 * if i.units == "us" { 9.0 / 5.0 } else { 1.0 };
        tip.push(format!("Ensemble spread: about \u{00B1}{half:.0}\u{00B0}"));
    }
    if let Some(p) = h.precip_prob {
        tip.push(format!("Rain chance: {:.0}%", p));
    }
    tip.push(format!("Agreement: {}", h.confidence.label()));
    if let Some((s, km)) = i.observed {
        tip.push(format!("Measured at {} ({:.0} km away): {}", s.id, km, deg(s.temp_c, i.units)));
    }
    tip.push(if i.weighted { "Models weighted by measured accuracy".into() } else { "Models weighted equally (not enough measured history yet)".into() });
    // Next few hours.
    let later: Vec<String> = i.hours.iter().skip(idx + 3).step_by(3).take(4).map(|x| format!("{} {}", &x.time[11..13.min(x.time.len())], deg(x.mean, i.units))).collect();
    if !later.is_empty() {
        tip.push(format!("Later: {}", later.join("  ")));
    }
    json!({"text": text, "tooltip": tip.join("\n")}).to_string()
}

/// The Yggdrasil panel's bar add-on JSON (`yggdrasil-weather panel`): the temperature now and,
/// from the nowcast, rain in the next two hours. Either half may be missing.
pub fn panel_json(i: Option<&BarInput>, o: Option<&Outlook>, now: i64) -> serde_json::Value {
    use crate::nowcast::{intensity, minutes, WARN_MIN, WET_MM_H};
    let mut parts = vec![];
    let mut lines = vec![];
    let mut state = "ok";
    let mut icon_name = "weather-few-clouds-symbolic";
    if let Some(i) = i {
        if let Some(idx) = current_index(i.hours, i.utc_offset_s, now) {
            let h = &i.hours[idx];
            parts.push(deg(h.mean, i.units));
            lines.push(json!({"title": "Now", "value": deg(h.mean, i.units), "note": format!("models {} to {}; agreement {}", deg(h.low, i.units), deg(h.high, i.units), h.confidence.label())}));
            if let Some(p) = h.precip_prob {
                lines.push(json!({"title": "Rain chance this hour", "value": format!("{p:.0}%"), "percent": p}));
            }
            if let Some((s, km)) = i.observed {
                lines.push(json!({"title": "Measured", "value": deg(s.temp_c, i.units), "note": format!("{} ({:.0} km away)", s.id, km)}));
            }
            let hod: u32 = h.time.get(11..13).and_then(|s| s.parse().ok()).unwrap_or(12);
            if !(6..20).contains(&hod) {
                icon_name = "weather-clear-night-symbolic";
            }
        }
    }
    if let Some(o) = o {
        if o.raining_now {
            parts.push(format!("{} mm/h", short(o.now_mm_h)));
            icon_name = if o.now_mm_h >= 7.6 { "weather-storm-symbolic" } else { "weather-showers-symbolic" };
            let until = o.stops_in.map(|m| format!("dry in about {}", minutes(m))).unwrap_or_else(|| "for the next two hours".into());
            lines.push(json!({"title": "Rain", "value": intensity(o.now_mm_h), "note": until}));
        } else if let Some(m) = o.starts_in {
            parts.push(format!("rain in {}", minutes(m)));
            icon_name = "weather-showers-scattered-symbolic";
            if m <= WARN_MIN {
                state = "warn";
            }
            lines.push(json!({"title": "Rain", "value": format!("in about {}", minutes(m)), "note": format!("{}, up to {:.1} mm/h", intensity(o.peak_mm_h), o.peak_mm_h)}));
        } else {
            lines.push(json!({"title": "Rain", "value": "none in the next two hours"}));
        }
        if o.peak_mm_h >= WET_MM_H {
            let off = i.map(|i| i.utc_offset_s).unwrap_or(0);
            for s in &o.steps {
                // the meter is full at 10 mm/h (heavy rain)
                lines.push(json!({"title": format_local(s.start, off)[11..].to_string(), "value": format!("{} mm/h", short(s.mm_h)), "percent": (s.mm_h * 10.0).min(100.0), "note": intensity(s.mm_h)}));
            }
        }
    }
    if let Some(i) = i {
        lines.push(json!({"title": "Place", "value": i.place, "note": i.how}));
    }
    if parts.is_empty() {
        // just the icon, so the card (and its settings) can still be opened
        return json!({"text": "", "state": "ok", "icon": icon_name, "help": "No weather data right now (offline, or the services did not answer)."});
    }
    json!({
        "text": parts.join(" \u{00B7} "),
        "state": state,
        "icon": icon_name,
        "lines": lines,
        "help": "Temperature: GFS, ECMWF and ICON combined (Open-Meteo). Rain: Open-Meteo 15-minute precipitation for the next two hours. Change the place or units with Settings below.",
    })
}

fn short(mm_h: f64) -> String {
    if mm_h < 1.0 { format!("{mm_h:.2}") } else { format!("{mm_h:.1}") }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::Confidence;
    use crate::timeutil::parse_local;

    fn hour(time: &str, mean: f64) -> Hour {
        Hour { time: time.into(), mean, low: mean - 1.0, high: mean + 1.0, n_models: 3, ensemble: Some((mean - 2.0, mean, mean + 2.0)), precip_prob: Some(10.0), confidence: Confidence::High }
    }

    #[test]
    fn units() {
        assert_eq!(to_units(0.0, "us"), 32.0);
        assert_eq!(to_units(100.0, "us"), 212.0);
        assert_eq!(to_units(21.5, "metric"), 21.5);
        assert_eq!(deg(21.6, "metric"), "22\u{00B0}");
    }

    #[test]
    fn picks_the_current_hour() {
        let hs = vec![hour("2026-10-10T13:00", 1.0), hour("2026-10-10T14:00", 2.0), hour("2026-10-10T15:00", 3.0)];
        let now = parse_local("2026-10-10T14:20", -4 * 3600).unwrap();
        assert_eq!(current_index(&hs, -4 * 3600, now), Some(1));
        let before = parse_local("2026-10-10T09:00", -4 * 3600).unwrap();
        assert_eq!(current_index(&hs, -4 * 3600, before), Some(0));
        let after = parse_local("2026-10-11T09:00", -4 * 3600).unwrap();
        assert_eq!(current_index(&hs, -4 * 3600, after), None);
    }

    #[test]
    fn icons() {
        assert_eq!(icon(Some(80.0), 12), "\u{1F327}");
        assert_eq!(icon(Some(40.0), 12), "\u{26C5}");
        assert_eq!(icon(Some(5.0), 12), "\u{2600}");
        assert_eq!(icon(Some(5.0), 23), "\u{1F319}");
    }

    #[test]
    fn bar_json_is_valid_and_honest_about_weighting() {
        let now = now_epoch();
        let key = format_local(now, 0);
        let hs = vec![hour(&format!("{}:00", &key[..13]), 18.4), hour(&format!("{}:00", &key[..13]), 19.0)];
        let s = Station { id: "KXYZ".into(), name: "x".into(), lat: 0.0, lon: 0.0, obs_epoch: 0, temp_c: 17.8 };
        let input = BarInput { hours: &hs, utc_offset_s: 0, units: "metric", place: "Test, Place", how: "chosen", observed: Some((&s, 12.3)), weighted: false };
        let v: serde_json::Value = serde_json::from_str(&bar_json(&input)).unwrap();
        assert!(v["text"].as_str().unwrap().contains("18\u{00B0}"));
        let tip = v["tooltip"].as_str().unwrap();
        assert!(tip.contains("KXYZ") && tip.contains("12 km"));
        assert!(tip.contains("weighted equally"));
        assert!(tip.contains("Ensemble spread: about \u{00B1}2\u{00B0}")); // (p90 - p10) / 2
        let input = BarInput { weighted: true, ..input };
        assert!(serde_json::from_str::<serde_json::Value>(&bar_json(&input)).unwrap()["tooltip"].as_str().unwrap().contains("measured accuracy"));
    }

    #[test]
    fn no_data_gives_empty_bar_text() {
        let input = BarInput { hours: &[], utc_offset_s: 0, units: "metric", place: "x", how: "chosen", observed: None, weighted: false };
        let v: serde_json::Value = serde_json::from_str(&bar_json(&input)).unwrap();
        assert_eq!(v["text"], "");
    }

    #[test]
    fn panel_json_shows_temperature_and_rain() {
        let text = include_str!("../tests/fixtures/minutely15.json");
        let steps = crate::nowcast::parse(text).unwrap();
        let now = parse_local("2026-10-25T14:05", -4 * 3600).unwrap();
        let o = crate::nowcast::outlook(&steps, now).unwrap();
        let hs = vec![hour("2026-10-25T14:00", 12.2)];
        let input = BarInput { hours: &hs, utc_offset_s: -4 * 3600, units: "metric", place: "New York", how: "chosen", observed: None, weighted: false };
        let v = panel_json(Some(&input), Some(&o), now);
        assert_eq!(v["text"], "12\u{00B0} \u{00B7} rain in 25 min");
        assert_eq!(v["state"], "warn");
        let lines = v["lines"].as_array().unwrap();
        assert!(lines.iter().any(|l| l["title"] == "15:00" && l["value"] == "6.0 mm/h" && l["percent"] == 60.0));
        assert_eq!(lines.last().unwrap()["value"], "New York");
        // nothing at all: no pill
        let none = panel_json(None, None, now);
        assert_eq!((none["text"].as_str(), none["state"].as_str(), none["icon"].is_string()), (Some(""), Some("ok"), true), "no data: the icon alone, so the card still opens");
        // only the temperature (the nowcast failed)
        assert_eq!(panel_json(Some(&input), None, now)["text"], "12\u{00B0}");
    }
}
