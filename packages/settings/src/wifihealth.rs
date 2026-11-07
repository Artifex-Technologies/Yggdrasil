//! Wi-Fi & Network > Wi-Fi checks: why the Wi-Fi is slow, from Heimdall's `heimdalld wifi-health --json` (Heimdall's
//! docs/WIFI_HEALTH.md): a weak signal, a crowded channel, other devices, the link to the router, or past the router.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Finding {
    #[serde(default)]
    pub check: String,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: String,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Link {
    #[serde(default)]
    pub ssid: String,
    #[serde(default)]
    pub band: String,
    #[serde(default)]
    pub channel: i32,
    #[serde(default)]
    pub width: i32,
    #[serde(default)]
    pub signal_dbm: i32,
    #[serde(default)]
    pub rx_mbps: f64,
    #[serde(default)]
    pub tx_mbps: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Report {
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub verdict: String,
    #[serde(default)]
    pub link: Link,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// Runs the check (about 10 seconds; with the download test about 20). Blocks: call it on a thread.
pub fn run(speed: bool) -> Result<Report, String> {
    let mut args = vec!["wifi-health", "--json"];
    if speed {
        args.push("--speed");
    }
    let out = std::process::Command::new("heimdalld").args(&args).output().map_err(|_| "Heimdall is not installed.".to_string())?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr).trim().trim_start_matches("wifi-health: ").to_string();
        return Err(if e.contains("unknown command") { "Heimdall is too old for Wi-Fi checks; update it.".into() } else { e });
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse(text: &str) -> Result<Report, String> {
    serde_json::from_str(text.trim()).map_err(|_| "Heimdall's answer could not be read.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_report() {
        let r = parse(r#"{"connected":true,"summary":"A weak signal is the likely cause","verdict":"bad",
            "link":{"ssid":"Home","band":"5 GHz","channel":157,"width":80,"signal_dbm":-80,"rx_mbps":6.5,"tx_mbps":13},
            "findings":[{"check":"signal","status":"bad","title":"Weak signal","detail":"Move closer."}],"devices_here":4}"#)
        .unwrap();
        assert!(r.connected);
        assert_eq!(r.link.channel, 157);
        assert_eq!(r.findings[0].title, "Weak signal");
        assert!(parse("nonsense").is_err());
    }
}
