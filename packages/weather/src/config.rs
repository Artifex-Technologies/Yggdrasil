//! Where you are: a place you chose ("manual"), the time zone's main city ("timezone": offline, rough, the default when
//! nothing is set), or nearby Wi-Fi through beaconDB ("auto"), only when you pick it.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Config {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// "metric" (deg C) or "us" (deg F).
    #[serde(default = "metric")]
    pub units: String,
    /// "manual", "timezone" or "auto". Configs from before modes were chosen places.
    #[serde(default = "manual")]
    pub mode: String,
    /// For "auto": when the place was last found (seconds since 1970).
    #[serde(default)]
    pub detected_at: i64,
    /// The last place picked from a search, kept while another mode is used, so "Chosen place" can go back to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chosen: Option<Chosen>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Chosen {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
}

fn manual() -> String {
    "manual".into()
}

fn metric() -> String {
    "metric".into()
}

impl Config {
    pub fn valid(&self) -> bool {
        (-90.0..=90.0).contains(&self.lat)
            && (-180.0..=180.0).contains(&self.lon)
            && (self.units == "metric" || self.units == "us")
            && ["manual", "timezone", "auto"].contains(&self.mode.as_str())
    }

    /// How the place was found, for the card and the settings window.
    pub fn how(&self) -> &'static str {
        match self.mode.as_str() {
            "auto" => "from Wi-Fi nearby",
            "timezone" => "from your time zone (rough)",
            _ => "chosen",
        }
    }
}

/// `YGGDRASIL_WEATHER_CONFIG` overrides the path (used by tests).
pub fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("YGGDRASIL_WEATHER_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    Some(yggdrasil_conf::dirs::config().join("weather.json"))
}

/// `YGGDRASIL_WEATHER_DATA` overrides the history directory.
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("YGGDRASIL_WEATHER_DATA") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    Some(yggdrasil_conf::dirs::data().join("weather"))
}

pub fn load(path: &std::path::Path) -> Option<Config> {
    let c: Config = serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    c.valid().then_some(c)
}

pub fn save(path: &std::path::Path, c: &Config) -> std::io::Result<()> {
    if !c.valid() {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "invalid location"));
    }
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    std::fs::write(path, serde_json::to_vec_pretty(c).map_err(std::io::Error::other)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_validation() {
        let d = std::env::temp_dir().join(format!("lw-config-{}", std::process::id()));
        let f = d.join("w.json");
        let c = Config { name: "Test".into(), lat: 40.7, lon: -74.0, units: "us".into(), mode: "auto".into(), detected_at: 5, chosen: Some(Chosen { name: "Home".into(), lat: 1.0, lon: 2.0 }) };
        save(&f, &c).unwrap();
        assert_eq!(load(&f), Some(c));
        assert!(save(&f, &Config { name: "x".into(), lat: 91.0, lon: 0.0, units: "metric".into(), mode: "manual".into(), detected_at: 0, chosen: None }).is_err());
        std::fs::write(&f, r#"{"name":"x","lat":1.0,"lon":2.0}"#).unwrap();
        assert_eq!((load(&f).unwrap().units.as_str(), load(&f).unwrap().mode.as_str()), ("metric", "manual"));
        std::fs::write(&f, r#"{"name":"x","lat":200.0,"lon":2.0}"#).unwrap();
        assert!(load(&f).is_none());
        std::fs::write(&f, "garbage").unwrap();
        assert!(load(&f).is_none());
    }
}
