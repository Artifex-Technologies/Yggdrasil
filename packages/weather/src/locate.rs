//! Finding the place without a terminal: from the time zone (offline, rough) or from nearby Wi-Fi through beaconDB
//! (an open, non-profit location database; Ichnaea API). Nothing here runs unless the user picked that mode.

use std::time::Duration;

pub const BEACONDB_URL: &str = "https://api.beacondb.net/v1/geolocate";
const UA: &str = "yggdrasil-weather/0.1 (+https://github.com/Artifex-Technologies/Yggdrasil)";
/// How close the time zone's city must be to name a detected place after it.
const NAME_KM: f64 = 80.0;

#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// Metres, when the service said.
    pub accuracy_m: Option<f64>,
}

/// Coordinates rounded to two decimals (about 1 km): all that leaves this computer afterwards.
pub fn round2(x: f64) -> f64 {
    (x * 100.0).round() / 100.0
}

/// ISO 6709 as zone1970.tab writes it: "+394421-1045903" or "+2232+08822".
pub fn parse_iso6709(s: &str) -> Option<(f64, f64)> {
    let split = s[1..].find(['+', '-'])? + 1;
    let (a, b) = s.split_at(split);
    fn part(p: &str, deg_digits: usize) -> Option<f64> {
        let sign = if p.starts_with('-') { -1.0 } else { 1.0 };
        let d = &p[1..];
        let num = |r: std::ops::Range<usize>| d.get(r).and_then(|x| x.parse::<f64>().ok());
        let deg = num(0..deg_digits)?;
        let min = num(deg_digits..deg_digits + 2)?;
        let sec = if d.len() >= deg_digits + 4 { num(deg_digits + 2..deg_digits + 4)? } else { 0.0 };
        Some(sign * (deg + min / 60.0 + sec / 3600.0))
    }
    Some((part(a, 2)?, part(b, 3)?))
}

/// The coordinates zone1970.tab gives `zone` (its main city).
pub fn zone_coords(tab: &str, zone: &str) -> Option<(f64, f64)> {
    tab.lines().filter(|l| !l.starts_with('#')).find_map(|l| {
        let f: Vec<&str> = l.split('\t').collect();
        (f.get(2) == Some(&zone)).then(|| parse_iso6709(f.get(1)?)).flatten()
    })
}

/// "America/Argentina/Buenos_Aires" -> "Buenos Aires".
pub fn zone_city(zone: &str) -> String {
    zone.rsplit('/').next().unwrap_or(zone).replace('_', " ")
}

/// The system's time zone name ("America/Denver").
pub fn system_zone() -> Option<String> {
    if let Ok(t) = std::env::var("TZ") {
        let t = t.trim_start_matches(':');
        if t.contains('/') && !t.starts_with('/') {
            return Some(t.to_string());
        }
    }
    if let Ok(p) = std::fs::read_link("/etc/localtime") {
        let s = p.to_string_lossy().to_string();
        if let Some(i) = s.find("zoneinfo/") {
            return Some(s[i + 9..].to_string());
        }
    }
    let o = std::process::Command::new("timedatectl").args(["show", "-p", "Timezone", "--value"]).output().ok()?;
    let z = String::from_utf8_lossy(&o.stdout).trim().to_string();
    (!z.is_empty()).then_some(z)
}

/// Offline and rough: the time zone's main city.
pub fn from_timezone() -> Result<Found, String> {
    let zone = system_zone().ok_or("the time zone is not set")?;
    let tab = std::fs::read_to_string("/usr/share/zoneinfo/zone1970.tab").map_err(|e| format!("no time zone table: {e}"))?;
    let (lat, lon) = zone_coords(&tab, &zone).ok_or_else(|| format!("{zone} has no place in the time zone table"))?;
    Ok(Found { name: zone_city(&zone), lat: round2(lat), lon: round2(lon), accuracy_m: None })
}

/// `nmcli -t -f BSSID,SIGNAL dev wifi list` lines ("AA\:BB\:CC\:DD\:EE\:FF:84") -> (mac, dBm), strongest first, at most 30.
pub fn parse_nmcli(out: &str) -> Vec<(String, i32)> {
    let mut v: Vec<(String, i32)> = out
        .lines()
        .filter_map(|l| {
            let (mac, sig) = l.trim().rsplit_once(':')?;
            let mac = mac.replace("\\:", ":").to_lowercase();
            let ok = mac.len() == 17 && mac.split(':').all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
            let sig: i32 = sig.parse().ok()?;
            ok.then(|| (mac, sig / 2 - 100))
        })
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    v.dedup_by(|a, b| a.0 == b.0);
    v.truncate(30);
    v
}

/// The request beaconDB gets: the networks' IDs and signal (no names); with none, it places the IP address.
pub fn request_body(aps: &[(String, i32)]) -> serde_json::Value {
    if aps.is_empty() {
        return serde_json::json!({"considerIp": true});
    }
    let list: Vec<_> = aps.iter().map(|(m, s)| serde_json::json!({"macAddress": m, "signalStrength": s})).collect();
    serde_json::json!({"considerIp": true, "wifiAccessPoints": list})
}

/// beaconDB's answer: {"location": {"lat", "lng"}, "accuracy"} or {"error": {...}}.
pub fn parse_answer(text: &str) -> Result<(f64, f64, Option<f64>), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "beaconDB gave an answer that is not JSON".to_string())?;
    if let Some(m) = v["error"]["message"].as_str() {
        return Err(format!("beaconDB: {m}"));
    }
    match (v["location"]["lat"].as_f64(), v["location"]["lng"].as_f64()) {
        (Some(lat), Some(lon)) if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) => Ok((lat, lon, v["accuracy"].as_f64())),
        _ => Err("beaconDB did not know where these networks are".into()),
    }
}

pub fn km(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (la1, lo1, la2, lo2) = (a.0.to_radians(), a.1.to_radians(), b.0.to_radians(), b.1.to_radians());
    let h = ((la2 - la1) / 2.0).sin().powi(2) + la1.cos() * la2.cos() * ((lo2 - lo1) / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().asin()
}

/// Precise: nearby Wi-Fi (as NetworkManager last saw it, no new scan) through beaconDB.
pub fn from_wifi() -> Result<Found, String> {
    let scan = std::process::Command::new("nmcli")
        .args(["-t", "-f", "BSSID,SIGNAL", "dev", "wifi", "list", "--rescan", "no"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();
    let body = request_body(&parse_nmcli(&scan)).to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(false).build().into();
    let mut resp = agent
        .post(BEACONDB_URL)
        .header("User-Agent", UA)
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("beaconDB could not be reached: {e}"))?;
    let text = resp.body_mut().read_to_string().map_err(|e| e.to_string())?;
    let (lat, lon, acc) = parse_answer(&text)?;
    let (lat, lon) = (round2(lat), round2(lon));
    let name = from_timezone().ok().filter(|t| km((t.lat, t.lon), (lat, lon)) <= NAME_KM).map(|t| t.name).unwrap_or_else(|| "Current location".into());
    Ok(Found { name, lat, lon, accuracy_m: acc })
}

/// Places named like `query` (Open-Meteo's geocoding: only the typed name is sent).
pub fn search_places(query: &str) -> Result<Vec<crate::openmeteo::Place>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(12))).build().into();
    let mut resp = agent.get(&crate::openmeteo::geocode_url(query)).header("User-Agent", UA).call().map_err(|e| format!("the place search could not be reached: {e}"))?;
    crate::openmeteo::parse_places(&resp.body_mut().read_to_string().map_err(|e| e.to_string())?)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TAB: &str = "# comment\nAU\t-3133+15905\tAustralia/Lord_Howe\tLord Howe Island\nIN\t+2232+08822\tAsia/Kolkata\nUS\t+394421-1045903\tAmerica/Denver\tMountain (most areas)\nAR\t-3436-05827\tAmerica/Argentina/Buenos_Aires\n";

    #[test]
    fn time_zones_become_places() {
        let (lat, lon) = zone_coords(TAB, "America/Denver").unwrap();
        assert!((lat - 39.7392).abs() < 0.001 && (lon + 104.9842).abs() < 0.001, "{lat} {lon}");
        let (lat, lon) = zone_coords(TAB, "Asia/Kolkata").unwrap();
        assert!((lat - 22.5333).abs() < 0.001 && (lon - 88.3667).abs() < 0.001);
        let (lat, lon) = zone_coords(TAB, "Australia/Lord_Howe").unwrap();
        assert!(lat < -31.0 && lon > 159.0);
        assert_eq!(zone_coords(TAB, "Mars/Olympus"), None);
        assert_eq!(zone_city("America/Argentina/Buenos_Aires"), "Buenos Aires");
        assert_eq!(round2(39.73921), 39.74);
    }

    #[test]
    fn nmcli_lines_become_networks() {
        let out = "AA\\:BB\\:CC\\:00\\:11\\:22:40\nAA\\:BB\\:CC\\:00\\:11\\:23:84\n\nnot a line\nAA\\:BB\\:CC\\:00\\:11\\:23:84\n";
        let v = parse_nmcli(out);
        assert_eq!(v, vec![("aa:bb:cc:00:11:23".to_string(), -58), ("aa:bb:cc:00:11:22".to_string(), -80)]);
        assert_eq!(request_body(&[]), serde_json::json!({"considerIp": true}));
        assert_eq!(request_body(&v)["wifiAccessPoints"][0]["macAddress"], "aa:bb:cc:00:11:23");
        let many: String = (0..40).map(|i| format!("AA\\:BB\\:CC\\:00\\:11\\:{i:02X}:{}\n", 50 + i)).collect();
        assert_eq!(parse_nmcli(&many).len(), 30);
    }

    #[test]
    fn beacondb_answers_are_read() {
        assert_eq!(parse_answer(r#"{"location":{"lat":39.7392,"lng":-104.9903},"accuracy":120.5}"#), Ok((39.7392, -104.9903, Some(120.5))));
        assert!(parse_answer(r#"{"error":{"code":404,"message":"Not found"}}"#).unwrap_err().contains("Not found"));
        assert!(parse_answer("<html>").is_err());
        assert!(parse_answer(r#"{"location":{"lat":999,"lng":0}}"#).is_err());
        assert!(km((39.74, -104.98), (40.01, -105.27)) < 50.0);
    }
}
