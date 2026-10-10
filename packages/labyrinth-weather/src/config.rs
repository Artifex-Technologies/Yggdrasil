//! Where you are. Never guessed from your IP: you set a place (or coordinates) once.

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
}

fn metric() -> String {
    "metric".into()
}

impl Config {
    pub fn valid(&self) -> bool {
        (-90.0..=90.0).contains(&self.lat) && (-180.0..=180.0).contains(&self.lon) && (self.units == "metric" || self.units == "us")
    }
}

/// `LABYRINTH_WEATHER_CONFIG` overrides the path (used by tests).
pub fn config_path() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LABYRINTH_WEATHER_CONFIG") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    dirs::config_dir().map(|d| d.join("labyrinth").join("weather.json"))
}

/// `LABYRINTH_WEATHER_DATA` overrides the history directory.
pub fn data_dir() -> Option<PathBuf> {
    if let Ok(p) = std::env::var("LABYRINTH_WEATHER_DATA") {
        if !p.is_empty() {
            return Some(PathBuf::from(p));
        }
    }
    dirs::data_dir().map(|d| d.join("labyrinth").join("weather"))
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
        let c = Config { name: "Test".into(), lat: 40.7, lon: -74.0, units: "us".into() };
        save(&f, &c).unwrap();
        assert_eq!(load(&f), Some(c));
        assert!(save(&f, &Config { name: "x".into(), lat: 91.0, lon: 0.0, units: "metric".into() }).is_err());
        std::fs::write(&f, r#"{"name":"x","lat":1.0,"lon":2.0}"#).unwrap();
        assert_eq!(load(&f).unwrap().units, "metric");
        std::fs::write(&f, r#"{"name":"x","lat":200.0,"lon":2.0}"#).unwrap();
        assert!(load(&f).is_none());
        std::fs::write(&f, "garbage").unwrap();
        assert!(load(&f).is_none());
    }
}
