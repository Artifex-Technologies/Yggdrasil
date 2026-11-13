//! autozone: Settings > Date & Time > "Set time zone automatically". Run by `yggdrasil-autozone.timer` (a user timer that the
//! switch enables): finds where this computer is from the Wi-Fi networks NetworkManager last saw, through beaconDB (an open,
//! non-profit location database, as the weather's "Automatic" place does), asks Open-Meteo which time zone those coordinates
//! are in (real zone borders, not the nearest city), and sets that zone when it differs.
//!
//! What leaves the computer: to beaconDB, the networks' IDs (BSSIDs, not names), their signal and the IP address; to
//! Open-Meteo, the coordinates rounded to two decimals (about 1 km). Nothing when the switch is off (the timer is not enabled).
//!
//! The zone is set through timedated without a password: Yggdrasil's polkit rule
//! (`/usr/share/polkit-1/rules.d/50-yggdrasil-timezone.rules`) lets an administrator signed in at this computer set the time
//! zone, and only that (the owner approved this rule on 2026-11-13).

use std::time::Duration;

pub const BEACONDB_URL: &str = "https://api.beacondb.net/v1/geolocate";
pub const OPENMETEO_URL: &str = "https://api.open-meteo.com/v1/forecast";

/// `nmcli -t -f BSSID,SIGNAL dev wifi list` lines ("AA\:BB\:CC\:DD\:EE\:FF:84") -> (mac, dBm), strongest first, at most 30
/// (as yggdrasil-weather's locate.rs reads them).
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
    v.sort_by_key(|a| std::cmp::Reverse(a.1));
    v.dedup_by(|a, b| a.0 == b.0);
    v.truncate(30);
    v
}

/// The request beaconDB gets: the networks' IDs and signal (no names); with none, it places the IP address.
pub fn request_body(aps: &[(String, i32)]) -> String {
    let list: Vec<_> = aps.iter().map(|(m, s)| serde_json::json!({"macAddress": m, "signalStrength": s})).collect();
    serde_json::json!({"considerIp": true, "wifiAccessPoints": list}).to_string()
}

/// beaconDB's answer: {"location": {"lat", "lng"}, "accuracy"} or {"error": {...}} -> coordinates rounded to about 1 km.
pub fn parse_location(text: &str) -> Result<(f64, f64), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "beaconDB gave an answer that is not JSON".to_string())?;
    if let Some(m) = v["error"]["message"].as_str() {
        return Err(format!("beaconDB: {m}"));
    }
    let round2 = |x: f64| (x * 100.0).round() / 100.0;
    match (v["location"]["lat"].as_f64(), v["location"]["lng"].as_f64()) {
        (Some(lat), Some(lon)) if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) => Ok((round2(lat), round2(lon))),
        _ => Err("beaconDB did not know where these networks are".into()),
    }
}

/// Open-Meteo's answer to `timezone=auto`: {"timezone": "America/Denver", ...}. Only a name the system's tz database has.
pub fn parse_zone(text: &str, known: impl Fn(&str) -> bool) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "Open-Meteo gave an answer that is not JSON".to_string())?;
    let z = v["timezone"].as_str().unwrap_or_default();
    let plain =
        !z.is_empty() && !z.starts_with('/') && !z.contains("..") && z.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c));
    if plain && known(z) {
        Ok(z.to_string())
    } else if z.is_empty() {
        Err("Open-Meteo did not say which time zone that is".into())
    } else {
        Err(format!("Open-Meteo named a time zone this computer does not know: {z}"))
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(15))).http_status_as_error(false).build().into()
}

fn ua() -> String {
    format!("yggdrasil-autozone/{} (+https://github.com/Artifex-Technologies/Yggdrasil)", env!("CARGO_PKG_VERSION"))
}

fn out(cmd: &str, args: &[&str]) -> String {
    std::process::Command::new(cmd)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// The zone this computer is in now.
pub fn locate_zone() -> Result<String, String> {
    let scan = out("nmcli", &["-t", "-f", "BSSID,SIGNAL", "dev", "wifi", "list", "--rescan", "no"]);
    let body = request_body(&parse_nmcli(&scan));
    let mut resp = agent()
        .post(BEACONDB_URL)
        .header("User-Agent", &ua())
        .header("Content-Type", "application/json")
        .send(body.as_bytes())
        .map_err(|e| format!("beaconDB could not be reached: {e}"))?;
    let (lat, lon) = parse_location(&resp.body_mut().read_to_string().map_err(|e| e.to_string())?)?;
    let url = format!("{OPENMETEO_URL}?latitude={lat:.2}&longitude={lon:.2}&timezone=auto&forecast_days=1");
    let mut resp = agent().get(&url).header("User-Agent", &ua()).call().map_err(|e| format!("Open-Meteo could not be reached: {e}"))?;
    parse_zone(&resp.body_mut().read_to_string().map_err(|e| e.to_string())?, |z| {
        std::path::Path::new("/usr/share/zoneinfo").join(z).is_file()
    })
}

/// `autozone [--dry-run]`: 0 when the zone is right (set now or already), 1 when it could not be found or set.
pub fn main(args: &[String]) -> i32 {
    let dry = args.iter().any(|a| a == "--dry-run");
    let zone = match locate_zone() {
        Ok(z) => z,
        Err(e) => {
            eprintln!("autozone: {e}");
            return 1;
        }
    };
    let now = out("timedatectl", &["show", "-p", "Timezone", "--value"]);
    if now == zone {
        println!("autozone: the time zone is already {zone}");
        return 0;
    }
    if dry {
        println!("autozone: would change the time zone from {now} to {zone}");
        return 0;
    }
    match std::process::Command::new("timedatectl").args(["--no-ask-password", "set-timezone", &zone]).status() {
        Ok(s) if s.success() => {
            println!("autozone: time zone changed from {now} to {zone}");
            0
        }
        _ => {
            eprintln!("autozone: timedated did not change the time zone to {zone}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn networks_go_strongest_first_without_names() {
        let aps = parse_nmcli("AA\\:BB\\:CC\\:DD\\:EE\\:01:40\nAA\\:BB\\:CC\\:DD\\:EE\\:02:90\nnot a line\n");
        assert_eq!(aps, [("aa:bb:cc:dd:ee:02".to_string(), -55), ("aa:bb:cc:dd:ee:01".to_string(), -80)]);
        let body = request_body(&aps);
        assert!(body.contains("\"macAddress\":\"aa:bb:cc:dd:ee:02\"") && body.contains("\"considerIp\":true"));
    }

    #[test]
    fn the_location_is_rounded_to_about_a_kilometre() {
        assert_eq!(parse_location(r#"{"location":{"lat":45.67891,"lng":-111.04321},"accuracy":40}"#), Ok((45.68, -111.04)));
        assert!(parse_location(r#"{"error":{"code":404,"message":"Not found"}}"#).unwrap_err().contains("Not found"));
        assert!(parse_location("<html>").is_err());
    }

    #[test]
    fn only_a_zone_the_system_knows_is_taken() {
        let known = |z: &str| z == "America/Denver";
        assert_eq!(parse_zone(r#"{"timezone":"America/Denver","utc_offset_seconds":-25200}"#, known), Ok("America/Denver".into()));
        assert!(parse_zone(r#"{"timezone":"Mars/Olympus"}"#, known).is_err());
        assert!(parse_zone(r#"{"timezone":"../../etc/passwd"}"#, |_| true).is_err());
        assert!(parse_zone(r#"{"latitude":1}"#, known).is_err());
    }
}
