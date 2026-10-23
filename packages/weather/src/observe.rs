//! Measured temperatures from airport weather reports (METAR) via aviationweather.gov:
//! free, no key, worldwide. This is the "ground truth" the accuracy weighting is scored
//! against. Airports are not your yard (nearby microclimate differs), so only stations
//! within a few tens of km are used.

use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct Station {
    pub id: String,
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub obs_epoch: i64,
    pub temp_c: f64,
}

#[derive(Deserialize)]
struct Raw {
    #[serde(rename = "icaoId", default)]
    icao_id: String,
    #[serde(rename = "obsTime", default)]
    obs_time: i64,
    temp: Option<f64>,
    lat: Option<f64>,
    lon: Option<f64>,
    #[serde(default)]
    name: String,
}

/// Reports without a temperature or position are dropped.
pub fn parse_metar(text: &str) -> Result<Vec<Station>, String> {
    let raw: Vec<Raw> = serde_json::from_str(text).map_err(|e| format!("cannot parse METAR reply: {e}"))?;
    Ok(raw
        .into_iter()
        .filter_map(|r| {
            Some(Station { id: r.icao_id, name: r.name, lat: r.lat?, lon: r.lon?, obs_epoch: r.obs_time, temp_c: r.temp? })
        })
        .collect())
}

pub fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let r = 6371.0;
    let (p1, p2) = (lat1.to_radians(), lat2.to_radians());
    let (dp, dl) = ((lat2 - lat1).to_radians(), (lon2 - lon1).to_radians());
    let a = (dp / 2.0).sin().powi(2) + p1.cos() * p2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * a.sqrt().asin()
}

/// Closest station within `max_km`, with its distance.
pub fn nearest(stations: &[Station], lat: f64, lon: f64, max_km: f64) -> Option<(Station, f64)> {
    stations
        .iter()
        .map(|s| (s, haversine_km(lat, lon, s.lat, s.lon)))
        .filter(|(_, d)| *d <= max_km)
        .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .map(|(s, d)| (s.clone(), d))
}

/// A ~1.2 degree box (~65 km each way) around the point, latest 3 hours of reports.
pub fn metar_url(lat: f64, lon: f64) -> String {
    format!(
        "https://aviationweather.gov/api/data/metar?bbox={:.3},{:.3},{:.3},{:.3}&format=json&hours=3",
        lat - 0.6,
        lon - 0.6,
        lat + 0.6,
        lon + 0.6
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const METAR: &str = include_str!("../tests/fixtures/metar.json");

    #[test]
    fn parses_real_metar_reply() {
        let s = parse_metar(METAR).unwrap();
        assert!(!s.is_empty());
        let linden = s.iter().find(|x| x.id == "KLDJ").unwrap();
        assert_eq!(linden.temp_c, 20.9);
        assert_eq!(linden.obs_epoch, 1_791_653_700);
    }

    #[test]
    fn distance_is_sane() {
        // New York to Philadelphia is about 130 km.
        let d = haversine_km(40.7128, -74.0060, 39.9526, -75.1652);
        assert!((120.0..140.0).contains(&d), "{d}");
        assert_eq!(haversine_km(1.0, 2.0, 1.0, 2.0), 0.0);
    }

    #[test]
    fn nearest_respects_the_radius() {
        let s = parse_metar(METAR).unwrap();
        let (st, d) = nearest(&s, 40.6162, -74.2451, 10.0).unwrap();
        assert_eq!(st.id, "KLDJ");
        assert!(d < 0.1);
        assert!(nearest(&s, 0.0, 0.0, 50.0).is_none());
    }

    #[test]
    fn drops_reports_without_temperature() {
        let t = r#"[{"icaoId":"AAAA","obsTime":1,"lat":1.0,"lon":1.0},{"icaoId":"BBBB","obsTime":1,"temp":3.0,"lat":1.0,"lon":1.0}]"#;
        let s = parse_metar(t).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].id, "BBBB");
    }
}
