//! Open-Meteo: multi-model forecast, ensemble members, and place search.
//! Free for non-commercial use with no key (https://open-meteo.com/en/docs); the
//! terms are theirs, not ours. Responses are parsed from text so tests use fixtures.

use serde::Deserialize;
use std::collections::BTreeMap;

pub const FORECAST_URL: &str = "https://api.open-meteo.com/v1/forecast";
pub const ENSEMBLE_URL: &str = "https://ensemble-api.open-meteo.com/v1/ensemble";
pub const GEOCODE_URL: &str = "https://geocoding-api.open-meteo.com/v1/search";

/// Models requested by default: three independent global systems (NOAA GFS, ECMWF IFS,
/// DWD ICON). More agreeing independent models means more trustworthy; disagreement is
/// the uncertainty signal.
pub const DEFAULT_MODELS: &[&str] = &["gfs_seamless", "ecmwf_ifs025", "icon_seamless"];

#[derive(Debug, Clone, PartialEq)]
pub struct Series {
    pub source: String,
    pub temp_c: Vec<Option<f64>>,
    pub precip_prob: Vec<Option<f64>>,
}

#[derive(Debug, Clone, Default)]
pub struct Forecast {
    /// Local times, "YYYY-MM-DDTHH:MM".
    pub times: Vec<String>,
    pub utc_offset_s: i64,
    pub models: Vec<Series>,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(default)]
    utc_offset_seconds: i64,
    hourly: BTreeMap<String, serde_json::Value>,
}

fn nums(v: &serde_json::Value) -> Vec<Option<f64>> {
    v.as_array().map(|a| a.iter().map(|x| x.as_f64()).collect()).unwrap_or_default()
}

fn raw(text: &str) -> Result<Raw, String> {
    serde_json::from_str(text).map_err(|e| format!("cannot parse Open-Meteo reply: {e}"))
}

fn times_of(r: &Raw) -> Result<Vec<String>, String> {
    r.hourly
        .get("time")
        .and_then(|t| t.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .ok_or_else(|| "reply has no hourly times".to_string())
}

/// Parse a `models=a,b,c` forecast. With several models Open-Meteo suffixes each
/// variable (`temperature_2m_gfs_seamless`); with one it does not (`temperature_2m`).
pub fn parse_multi(text: &str) -> Result<Forecast, String> {
    let r = raw(text)?;
    let times = times_of(&r)?;
    let mut by_source: BTreeMap<String, Series> = BTreeMap::new();
    for (key, val) in &r.hourly {
        let (var, source) = if let Some(s) = key.strip_prefix("temperature_2m_") {
            ("t", s)
        } else if key == "temperature_2m" {
            ("t", "best_match")
        } else if let Some(s) = key.strip_prefix("precipitation_probability_") {
            ("p", s)
        } else if key == "precipitation_probability" {
            ("p", "best_match")
        } else {
            continue;
        };
        let s = by_source.entry(source.to_string()).or_insert_with(|| Series {
            source: source.to_string(),
            temp_c: vec![],
            precip_prob: vec![],
        });
        if var == "t" { s.temp_c = nums(val) } else { s.precip_prob = nums(val) }
    }
    let models: Vec<Series> = by_source.into_values().filter(|s| !s.temp_c.is_empty()).collect();
    if models.is_empty() {
        return Err("reply contains no temperature series".into());
    }
    Ok(Forecast { times, utc_offset_s: r.utc_offset_seconds, models })
}

/// Parse an ensemble reply into one temperature series per member (the unsuffixed
/// `temperature_2m` is the control run and is included).
pub fn parse_ensemble(text: &str) -> Result<(Vec<String>, Vec<Vec<Option<f64>>>), String> {
    let r = raw(text)?;
    let times = times_of(&r)?;
    let members: Vec<Vec<Option<f64>>> = r
        .hourly
        .iter()
        .filter(|(k, _)| *k == "temperature_2m" || k.starts_with("temperature_2m_member"))
        .map(|(_, v)| nums(v))
        .collect();
    if members.is_empty() {
        return Err("reply contains no ensemble members".into());
    }
    Ok((times, members))
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Place {
    pub name: String,
    pub latitude: f64,
    pub longitude: f64,
    #[serde(default)]
    pub country: String,
    #[serde(default)]
    pub admin1: String,
    #[serde(default)]
    pub timezone: String,
}

impl Place {
    pub fn label(&self) -> String {
        [self.name.as_str(), self.admin1.as_str(), self.country.as_str()]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn parse_places(text: &str) -> Result<Vec<Place>, String> {
    #[derive(Deserialize)]
    struct R {
        #[serde(default)]
        results: Vec<Place>,
    }
    serde_json::from_str::<R>(text).map(|r| r.results).map_err(|e| format!("cannot parse place search: {e}"))
}

pub fn forecast_url(lat: f64, lon: f64, models: &[&str], days: u32) -> String {
    format!(
        "{FORECAST_URL}?latitude={lat:.4}&longitude={lon:.4}&hourly=temperature_2m,precipitation_probability&models={}&forecast_days={days}&timezone=auto",
        models.join(",")
    )
}

pub fn ensemble_url(lat: f64, lon: f64, days: u32) -> String {
    format!("{ENSEMBLE_URL}?latitude={lat:.4}&longitude={lon:.4}&hourly=temperature_2m&models=gfs025&forecast_days={days}&timezone=auto")
}

pub fn geocode_url(name: &str) -> String {
    let enc: String = name
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() || b"-._~".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") })
        .collect();
    format!("{GEOCODE_URL}?name={enc}&count=5")
}

#[cfg(test)]
mod tests {
    use super::*;

    const MULTI: &str = include_str!("../tests/fixtures/multi.json");
    const ENS: &str = include_str!("../tests/fixtures/ens.json");
    const GEO: &str = include_str!("../tests/fixtures/geocode.json");

    #[test]
    fn parses_real_multi_model_reply() {
        let f = parse_multi(MULTI).unwrap();
        assert_eq!(f.models.len(), 3);
        let names: Vec<_> = f.models.iter().map(|m| m.source.as_str()).collect();
        assert_eq!(names, ["ecmwf_ifs025", "gfs_seamless", "icon_seamless"]);
        assert_eq!(f.times.len(), 6);
        assert!(f.models.iter().all(|m| m.temp_c.len() == 6 && m.precip_prob.len() == 6));
        assert_eq!(f.utc_offset_s, -4 * 3600);
    }

    #[test]
    fn single_model_reply_has_unsuffixed_keys() {
        let t = r#"{"utc_offset_seconds":0,"hourly":{"time":["2026-01-01T00:00"],"temperature_2m":[3.5],"precipitation_probability":[10]}}"#;
        let f = parse_multi(t).unwrap();
        assert_eq!(f.models[0].source, "best_match");
        assert_eq!(f.models[0].temp_c, vec![Some(3.5)]);
    }

    #[test]
    fn nulls_become_none() {
        let t = r#"{"hourly":{"time":["a","b"],"temperature_2m_x":[1.0,null]}}"#;
        assert_eq!(parse_multi(t).unwrap().models[0].temp_c, vec![Some(1.0), None]);
    }

    #[test]
    fn parses_ensemble_members_including_control() {
        let (times, members) = parse_ensemble(ENS).unwrap();
        assert_eq!(times.len(), 6);
        assert_eq!(members.len(), 5); // control + member01..04 in the trimmed fixture
        assert!(members.iter().all(|m| m.len() == 6));
    }

    #[test]
    fn errors_are_reported_not_panicked() {
        assert!(parse_multi("{}").is_err());
        assert!(parse_multi("not json").is_err());
        assert!(parse_ensemble(r#"{"hourly":{"time":[]}}"#).is_err());
    }

    #[test]
    fn places() {
        let p = parse_places(GEO).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[1].label(), "Berlin, New Hampshire, United States");
        assert!(parse_places(r#"{"generationtime_ms":1}"#).unwrap().is_empty());
    }

    #[test]
    fn urls() {
        assert!(forecast_url(40.7, -74.0, DEFAULT_MODELS, 3).contains("models=gfs_seamless,ecmwf_ifs025,icon_seamless"));
        assert!(geocode_url("São Paulo & co").contains("name=S%C3%A3o%20Paulo%20%26%20co"));
    }
}
