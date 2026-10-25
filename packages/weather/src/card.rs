//! Weather's card in the Yggdrasil panel: the weather, and behind its Settings button the place (automatic, from the time zone,
//! or chosen from a search) and the units. The panel draws the blocks and runs `yggdrasil-weather press ID [TEXT]` for a press
//! (docs/INTEGRATIONS.md, "Cards you can use"). What the card is showing (settings or not, the search) is kept in the runtime
//! folder only, and forgotten after ten minutes.

use crate::config::{Chosen, Config};
use crate::locate::Found;
use crate::openmeteo::Place;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;

/// After this long the card opens on the weather again.
pub const STALE_S: i64 = 600;
const MAX_RESULTS: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CardState {
    /// "settings", or empty for the weather.
    #[serde(default)]
    pub view: String,
    #[serde(default)]
    pub query: String,
    #[serde(default)]
    pub results: Vec<Place>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub updated: i64,
}

/// `YGGDRASIL_WEATHER_RUNTIME` overrides the folder (tests).
pub fn state_path() -> Option<PathBuf> {
    let dir = std::env::var_os("YGGDRASIL_WEATHER_RUNTIME").filter(|d| !d.is_empty()).map(PathBuf::from).or_else(|| Some(PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("yggdrasil-weather")))?;
    Some(dir.join("card.json"))
}

pub fn load_state(now: i64) -> CardState {
    let st: CardState = state_path().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    if now - st.updated > STALE_S {
        CardState::default()
    } else {
        st
    }
}

pub fn save_state(st: &CardState) {
    let Some(p) = state_path() else { return };
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_vec(st).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// The card's blocks. `cfg` is the place in use (None when there is none at all).
pub fn blocks(cfg: Option<&Config>, st: &CardState) -> Value {
    if st.view != "settings" {
        return json!([{"buttons": [{"id": "view:settings", "label": "Settings"}]}]);
    }
    let mode = cfg.map(|c| c.mode.as_str()).unwrap_or("");
    let chosen = cfg.and_then(|c| c.chosen.as_ref().map(|x| x.name.clone()).or_else(|| (c.mode == "manual").then(|| c.name.clone())));
    let place_note = |m: &str, text: &str| match cfg {
        Some(c) if c.mode == m => format!("{text} \u{00B7} now {}", c.name),
        _ => text.to_string(),
    };
    let mut b = vec![
        json!({"section": "PLACE"}),
        json!({"list": [
            {"id": "mode:auto", "title": "Automatic", "note": place_note("auto", "nearby Wi-Fi through beaconDB; network IDs and your IP go to beaconDB"), "on": mode == "auto"},
            {"id": "mode:timezone", "title": "From time zone", "note": place_note("timezone", "rough, offline"), "on": mode == "timezone"},
            {"id": "mode:manual", "title": "Chosen place", "note": chosen.unwrap_or_else(|| "search below".into()), "on": mode == "manual"},
        ]}),
        json!({"field": {"id": "search", "hint": "Search for a city", "value": st.query, "submit": "search-go"}}),
    ];
    if !st.results.is_empty() {
        let rows: Vec<Value> = st.results.iter().enumerate().map(|(i, p)| json!({"id": format!("pick:{i}"), "title": p.label(), "note": format!("{:.2}, {:.2}", p.latitude, p.longitude)})).collect();
        b.push(json!({"list": rows}));
    }
    let us = cfg.map(|c| c.units == "us").unwrap_or(false);
    b.push(json!({"section": "UNITS"}));
    b.push(json!({"buttons": [{"id": "units:metric", "label": "Celsius", "accent": !us}, {"id": "units:us", "label": "Fahrenheit", "accent": us}], "columns": 2}));
    if !st.status.is_empty() {
        b.push(json!({"text": st.status, "dim": true}));
    }
    b.push(json!({"buttons": [{"id": "view:normal", "label": "Done"}]}));
    Value::Array(b)
}

/// What a press does. `current` is the place in use (saved, or the time zone's when nothing is saved); the lookups are passed in
/// so nothing reaches the network but the one the press asks for (beaconDB only for "Automatic", Open-Meteo's search only when
/// the search is sent). Returns the config to save, if it changed.
pub fn press(
    id: &str,
    text: Option<&str>,
    st: &mut CardState,
    current: Option<Config>,
    now: i64,
    timezone: impl Fn() -> Result<Found, String>,
    wifi: impl Fn() -> Result<Found, String>,
    search: impl Fn(&str) -> Result<Vec<Place>, String>,
) -> Option<Config> {
    st.updated = now;
    let base = current.clone().unwrap_or(Config { name: String::new(), lat: 0.0, lon: 0.0, units: "metric".into(), mode: "timezone".into(), detected_at: 0, chosen: None });
    let at = |f: Found, mode: &str| Config { name: f.name, lat: f.lat, lon: f.lon, mode: mode.into(), detected_at: if mode == "auto" { now } else { 0 }, ..base.clone() };
    match id {
        "view:settings" => {
            st.view = "settings".into();
            st.status.clear();
            None
        }
        "view:normal" => {
            *st = CardState { updated: now, ..Default::default() };
            None
        }
        "mode:auto" => match wifi() {
            Ok(f) => {
                st.status = format!("Found {}.", f.name);
                Some(at(f, "auto"))
            }
            Err(e) => {
                st.status = format!("Could not find your place: {e}");
                None
            }
        },
        "mode:timezone" => match timezone() {
            Ok(f) => {
                st.status = format!("Using {}, from your time zone.", f.name);
                Some(at(f, "timezone"))
            }
            Err(e) => {
                st.status = e;
                None
            }
        },
        "mode:manual" => match base.chosen.clone() {
            Some(c) => {
                st.status = format!("Using {}.", c.name);
                Some(Config { name: c.name, lat: c.lat, lon: c.lon, mode: "manual".into(), detected_at: 0, ..base })
            }
            None if base.mode == "manual" && current.is_some() => None,
            None => {
                st.status = "Search for a city below and pick it.".into();
                None
            }
        },
        "search" => {
            st.query = text.unwrap_or("").chars().take(100).collect();
            None
        }
        "search-go" => {
            let q = st.query.trim().to_string();
            if q.is_empty() {
                st.status = "Type a city first.".into();
                return None;
            }
            match search(&q) {
                Ok(v) => {
                    st.status = if v.is_empty() { format!("No place found for \u{201C}{q}\u{201D}.") } else { String::new() };
                    st.results = v.into_iter().take(MAX_RESULTS).collect();
                }
                Err(e) => st.status = e,
            }
            None
        }
        "units:metric" | "units:us" => {
            let units = &id[6..];
            (current.is_some() && base.units != units).then(|| Config { units: units.into(), ..base })
        }
        _ => {
            if let Some(p) = id.strip_prefix("pick:").and_then(|n| n.parse::<usize>().ok()).and_then(|n| st.results.get(n).cloned()) {
                st.status = format!("Using {}.", p.label());
                st.results.clear();
                let chosen = Chosen { name: p.label(), lat: p.latitude, lon: p.longitude };
                return Some(Config { name: chosen.name.clone(), lat: chosen.lat, lon: chosen.lon, mode: "manual".into(), detected_at: 0, chosen: Some(chosen), ..base });
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tz() -> Result<Found, String> {
        Ok(Found { name: "Denver".into(), lat: 39.74, lon: -104.98, accuracy_m: None })
    }
    fn no_wifi() -> Result<Found, String> {
        Err("offline".into())
    }
    fn wifi() -> Result<Found, String> {
        Ok(Found { name: "Current location".into(), lat: 45.68, lon: -111.04, accuracy_m: Some(80.0) })
    }
    fn no_search(_: &str) -> Result<Vec<Place>, String> {
        panic!("searched")
    }
    fn place(name: &str) -> Place {
        Place { name: name.into(), latitude: 45.68, longitude: -111.04, country: "United States".into(), admin1: "Montana".into(), timezone: String::new() }
    }
    fn cfg(mode: &str) -> Config {
        Config { name: "Denver".into(), lat: 39.74, lon: -104.98, units: "metric".into(), mode: mode.into(), detected_at: 0, chosen: None }
    }

    #[test]
    fn the_card_opens_on_the_weather_and_settings_list_the_choices() {
        let st = CardState::default();
        assert_eq!(blocks(Some(&cfg("timezone")), &st)[0]["buttons"][0]["id"], "view:settings");
        let st = CardState { view: "settings".into(), query: "boz".into(), results: vec![place("Bozeman")], ..Default::default() };
        let b = blocks(Some(&cfg("timezone")), &st);
        let rows = &b[1]["list"];
        assert_eq!((rows[0]["on"].as_bool(), rows[1]["on"].as_bool()), (Some(false), Some(true)));
        assert!(rows[1]["note"].as_str().unwrap().contains("now Denver"));
        assert_eq!(rows[2]["note"], "search below");
        assert_eq!(b[2]["field"]["value"], "boz");
        assert_eq!(b[2]["field"]["submit"], "search-go");
        assert_eq!(b[3]["list"][0]["id"], "pick:0");
        assert_eq!(b[3]["list"][0]["title"], "Bozeman, Montana, United States");
        let last = b.as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["buttons"][0]["id"], "view:normal");
        // the units show which is in use
        assert!(b.as_array().unwrap().iter().any(|x| x["buttons"][0]["id"] == "units:metric" && x["buttons"][0]["accent"] == true));
    }

    #[test]
    fn typing_only_stores_the_search_and_enter_searches() {
        let mut st = CardState { view: "settings".into(), ..Default::default() };
        assert_eq!(press("search", Some("Bozeman"), &mut st, Some(cfg("timezone")), 10, tz, no_wifi, no_search), None);
        assert_eq!(st.query, "Bozeman");
        let found = press("search-go", None, &mut st, Some(cfg("timezone")), 11, tz, no_wifi, |q| {
            assert_eq!(q, "Bozeman");
            Ok(vec![place("Bozeman"), place("Belgrade")])
        });
        assert_eq!((found, st.results.len()), (None, 2));
        let c = press("pick:0", None, &mut st, Some(cfg("timezone")), 12, tz, no_wifi, no_search).unwrap();
        assert_eq!((c.mode.as_str(), c.name.as_str()), ("manual", "Bozeman, Montana, United States"));
        assert_eq!(c.chosen.as_ref().unwrap().lat, 45.68);
        assert!(st.results.is_empty());
        // back to the time zone, then to the chosen place again, without searching
        let tzc = press("mode:timezone", None, &mut st, Some(c.clone()), 13, tz, no_wifi, no_search).unwrap();
        assert_eq!((tzc.mode.as_str(), tzc.name.as_str(), tzc.chosen.is_some()), ("timezone", "Denver", true));
        let back = press("mode:manual", None, &mut st, Some(tzc), 14, tz, no_wifi, no_search).unwrap();
        assert_eq!((back.mode.as_str(), back.lat), ("manual", 45.68));
    }

    #[test]
    fn automatic_saves_what_beacondb_found_and_a_failure_keeps_the_place() {
        let mut st = CardState::default();
        assert_eq!(press("mode:auto", None, &mut st, Some(cfg("timezone")), 20, tz, no_wifi, no_search), None);
        assert!(st.status.contains("offline"));
        let c = press("mode:auto", None, &mut st, Some(cfg("timezone")), 21, tz, wifi, no_search).unwrap();
        assert_eq!((c.mode.as_str(), c.detected_at, c.lat), ("auto", 21, 45.68));
    }

    #[test]
    fn units_and_views() {
        let mut st = CardState::default();
        let c = press("units:us", None, &mut st, Some(cfg("manual")), 1, tz, no_wifi, no_search).unwrap();
        assert_eq!(c.units, "us");
        assert_eq!(press("units:metric", None, &mut st, Some(cfg("manual")), 1, tz, no_wifi, no_search), None, "already metric");
        press("view:settings", None, &mut st, None, 5, tz, no_wifi, no_search);
        assert_eq!(st.view, "settings");
        press("view:normal", None, &mut st, None, 6, tz, no_wifi, no_search);
        assert_eq!((st.view.as_str(), st.updated), ("", 6));
        assert_eq!(press("pick:9", None, &mut st, None, 7, tz, no_wifi, no_search), None);
        assert_eq!(press("nonsense", None, &mut st, None, 7, tz, no_wifi, no_search), None);
    }

    #[test]
    fn the_card_state_is_forgotten_after_ten_minutes() {
        let dir = std::env::temp_dir().join(format!("weather-card-{}", std::process::id()));
        std::env::set_var("YGGDRASIL_WEATHER_RUNTIME", &dir);
        save_state(&CardState { view: "settings".into(), updated: 1000, ..Default::default() });
        assert_eq!(load_state(1000 + STALE_S).view, "settings");
        assert_eq!(load_state(1001 + STALE_S).view, "");
        std::fs::remove_dir_all(&dir).ok();
    }
}
