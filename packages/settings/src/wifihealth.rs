//! Wi-Fi & Network > Wi-Fi checks: why the Wi-Fi is slow, from the security monitor's `wifi_check` command (the `security`
//! integration, docs/INTEGRATIONS.md): a weak signal, a crowded channel, other devices, the link to the router, or past the router.

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct Finding {
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

fn monitor() -> Option<(&'static yggdrasil_conf::integrations::Integration, Vec<String>)> {
    let i = yggdrasil_conf::integrations::security()?;
    Some((i, i.command("wifi_check")?))
}

/// Whether a security monitor offers the check (the page is shown only then).
pub fn available() -> bool {
    monitor().is_some()
}

/// Runs the check (about 10 seconds; with the download test about 20). Blocks: call it on a thread.
pub fn run(speed: bool) -> Result<Report, String> {
    let (i, cmd) = monitor().ok_or("There is no Wi-Fi check on this computer.")?;
    let name = i.get("name").unwrap_or("The security monitor");
    let mut c = std::process::Command::new(&cmd[0]);
    c.args(&cmd[1..]).stdin(std::process::Stdio::null());
    if speed {
        c.arg("--speed");
    }
    let out = c.output().map_err(|_| format!("{name} could not be started."))?;
    if !out.status.success() {
        let e = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if e.is_empty() { format!("{name} could not run the check.") } else { e });
    }
    parse(&String::from_utf8_lossy(&out.stdout))
}

pub fn parse(text: &str) -> Result<Report, String> {
    serde_json::from_str(text.trim()).map_err(|_| "The answer could not be read.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_report() {
        let r = parse(
            r#"{"connected":true,"summary":"A weak signal is the likely cause","verdict":"bad",
            "link":{"ssid":"Home","band":"5 GHz","channel":157,"width":80,"signal_dbm":-80,"rx_mbps":6.5,"tx_mbps":13},
            "findings":[{"check":"signal","status":"bad","title":"Weak signal","detail":"Move closer."}],"devices_here":4}"#,
        )
        .unwrap();
        assert!(r.connected);
        assert_eq!(r.link.channel, 157);
        assert_eq!(r.findings[0].title, "Weak signal");
        assert!(parse("nonsense").is_err());
    }
}
