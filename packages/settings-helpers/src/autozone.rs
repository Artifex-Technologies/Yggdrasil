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

/// Open-Meteo's answer to `timezone=auto`: {"timezone": "America/Denver", ...}. Only an Area/Location name (it goes on
/// timedatectl's command line: never one starting with "-") that the system's tz database lists (`known`, see [`in_tzdata`]);
/// never "Factory", "posixrules", "localtime" or the "right/" and "posix/" copies, whatever a server names.
pub fn parse_zone(text: &str, known: impl Fn(&str) -> bool) -> Result<String, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|_| "Open-Meteo gave an answer that is not JSON".to_string())?;
    let z = v["timezone"].as_str().unwrap_or_default();
    let area_location = z.len() <= 64
        && z.starts_with(|c: char| c.is_ascii_uppercase())
        && z.contains('/')
        && !z.contains("..")
        && !z.contains("//")
        && !z.ends_with('/')
        && !z.starts_with("right/")
        && !z.starts_with("posix/")
        && z.chars().all(|c| c.is_ascii_alphanumeric() || "/_+-".contains(c));
    if area_location && known(z) {
        Ok(z.to_string())
    } else if z.is_empty() {
        Err("Open-Meteo did not say which time zone that is".into())
    } else {
        Err(format!("Open-Meteo named a time zone this computer does not know: {z}"))
    }
}

/// Whether tzdata.zi (the tz database in one file, /usr/share/zoneinfo/tzdata.zi) names `zone`: a zone ("Z NAME ...") or a
/// link to one ("L TARGET NAME").
pub fn in_tzdata(zi: &str, zone: &str) -> bool {
    zi.lines().any(|l| {
        let w: Vec<&str> = l.split_whitespace().collect();
        matches!(w.as_slice(), ["Z", name, ..] if *name == zone) || matches!(w.as_slice(), ["L", _, name] if *name == zone)
    })
}

/// The live check: listed in tzdata.zi, and its file is there for timedated to use.
fn known_zone(zone: &str) -> bool {
    let dir = std::path::Path::new("/usr/share/zoneinfo");
    std::fs::read_to_string(dir.join("tzdata.zi")).is_ok_and(|zi| in_tzdata(&zi, zone)) && dir.join(zone).is_file()
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
    parse_zone(&text, known_zone).map_err(failed)
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
        // names that are files under /usr/share/zoneinfo but not Area/Location zones, or would read as an option to
        // timedatectl: refused even when the system says it has them
        for z in [
            "-America/Denver",
            "--help",
            "Factory",
            "posixrules",
            "localtime",
            "right/America/Denver",
            "posix/America/Denver",
            "America//Denver",
            "America/",
            "/etc/localtime",
            "america/denver",
        ] {
            let answer = serde_json::json!({ "timezone": z }).to_string();
            assert!(parse_zone(&answer, |_| true).is_err(), "{z} was taken");
        }
    }

    #[test]
    fn tzdata_lists_zones_and_links() {
        let zi = "# version 2026a\nR US 1967 2006 - O lastSu 2 0 S\nZ America/Denver -6:59:56 - LMT 1883 N 18 19u\nZ Factory 0 - -00\nL America/Denver America/Shiprock\nL Etc/GMT GMT\n";
        assert!(in_tzdata(zi, "America/Denver") && in_tzdata(zi, "America/Shiprock"));
        assert!(!in_tzdata(zi, "America/Boise") && !in_tzdata(zi, "Etc/GMT") && !in_tzdata(zi, "US"));
        // tzdata names Factory, but it is no Area/Location zone
        assert!(in_tzdata(zi, "Factory"));
        assert!(parse_zone(r#"{"timezone":"Factory"}"#, |z| in_tzdata(zi, z)).is_err());
        assert_eq!(parse_zone(r#"{"timezone":"America/Shiprock"}"#, |z| in_tzdata(zi, z)), Ok("America/Shiprock".into()));
    }
}
