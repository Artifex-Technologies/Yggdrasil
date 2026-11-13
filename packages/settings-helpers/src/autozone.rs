//! autozone: Settings > Date & Time > "Set time zone automatically". Run by `yggdrasil-autozone.timer` (a user timer that the
//! switch enables): finds roughly where this computer is from its IP address, through beaconDB (an open, non-profit location
//! service, as the weather's "Automatic" place does; packages/geolocate), asks Open-Meteo which time zone those coordinates
//! are in (real zone borders, not the nearest city), and sets that zone when it differs.
//!
//! What leaves the computer: to beaconDB, the IP address only (the request is `{"considerIp":true}`; no Wi-Fi networks, no
//! hardware IDs); to Open-Meteo, the rough coordinates rounded to two decimals (about 1 km), for the time zone. Nothing when the
//! switch is off (the timer is not enabled), and nothing while a VPN is on: the IP address would be the VPN server's, so the
//! run is skipped and says why in the journal.
//!
//! The zone is set through timedated without a password: Yggdrasil's polkit rule
//! (`/usr/share/polkit-1/rules.d/50-yggdrasil-timezone.rules`) lets an administrator signed in at this computer set the time
//! zone, and only that (the owner approved this rule on 2026-11-13).

use geolocate::{Error, Place};
use std::time::Duration;

pub const OPENMETEO_URL: &str = "https://api.open-meteo.com/v1/forecast";

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

/// Open-Meteo's forecast address for `place`, asking only for its time zone; the coordinates go out with two decimals.
pub fn zone_url(place: Place) -> String {
    let (lat, lon) = (geolocate::round2(place.lat), geolocate::round2(place.lon));
    format!("{OPENMETEO_URL}?latitude={lat:.2}&longitude={lon:.2}&timezone=auto&forecast_days=1")
}

/// The zone this computer is in now (nothing is asked while a VPN is on).
pub fn locate_zone() -> Result<String, Error> {
    let place = geolocate::locate(&ua())?;
    let failed = |e: String| Error::Failed(e);
    let mut resp = agent()
        .get(&zone_url(place))
        .header("User-Agent", &ua())
        .call()
        .map_err(|e| failed(format!("Open-Meteo could not be reached: {e}")))?;
    let text = resp.body_mut().read_to_string().map_err(|e| failed(e.to_string()))?;
    parse_zone(&text, |z| std::path::Path::new("/usr/share/zoneinfo").join(z).is_file()).map_err(failed)
}

/// `autozone [--dry-run]`: 0 when the zone is right (set now or already), 1 when it could not be found or set.
/// While a VPN is on, 0 too: the run is skipped (the place would be the VPN server's), not failed.
pub fn main(args: &[String]) -> i32 {
    run(args, locate_zone)
}

fn run(args: &[String], locate: impl FnOnce() -> Result<String, Error>) -> i32 {
    let dry = args.iter().any(|a| a == "--dry-run");
    let zone = match locate() {
        Ok(z) => z,
        Err(Error::Vpn(why)) => {
            println!("autozone: skipped, {why}: the IP address would be the VPN server's, and the zone wherever that server is");
            return 0;
        }
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
    fn open_meteo_gets_two_decimals() {
        let url = zone_url(Place { lat: 48.85661, lon: 2.35222 });
        assert!(url.contains("?latitude=48.86&longitude=2.35&timezone=auto"), "{url}");
    }

    #[test]
    fn a_vpn_skips_the_run_and_a_failure_fails_it() {
        // both return before timedatectl is asked anything
        assert_eq!(run(&[], || Err(Error::Vpn("a VPN connection is active".into()))), 0);
        assert_eq!(run(&[], || Err(Error::Failed("beaconDB could not be reached".into()))), 1);
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
