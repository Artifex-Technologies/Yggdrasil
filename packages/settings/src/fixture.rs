//! Debug builds only: a made-up system, for screenshots of pages without the real one behind them. With
//! `$YGGDRASIL_SETTINGS_FIXTURE` naming a JSON file
//!
//!     {"run": {"nmcli -t -f ... device": "its output", "curl *": "..."}, "scan": "made-up access points"}
//!
//! every command `sys::run` would start is answered from `run` instead (a key ending in `*` answers every command line it
//! starts; a command not listed gets no answer) and none is started, every change (`sys::act`) is refused, and the Wi-Fi
//! scan is `scan` (yggdrasil-wifiscan's fixture lines) instead of the radio's. Release builds never read it.
//! Example: packages/settings/tests/fixtures/wifi.json.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

#[derive(Debug, Default, Deserialize)]
pub struct Fixture {
    #[serde(default)]
    pub run: BTreeMap<String, String>,
    #[serde(default)]
    pub scan: String,
}

/// The fixture, when a debug build was started with one. A file that cannot be read gives an empty one (nothing real is run
/// either way).
pub fn get() -> Option<&'static Fixture> {
    static F: OnceLock<Option<Fixture>> = OnceLock::new();
    F.get_or_init(|| {
        if !cfg!(debug_assertions) {
            return None;
        }
        let p = std::env::var_os("YGGDRASIL_SETTINGS_FIXTURE")?;
        let f = std::fs::read_to_string(&p).map_err(|e| e.to_string()).and_then(|t| serde_json::from_str(&t).map_err(|e| e.to_string()));
        Some(f.unwrap_or_else(|e| {
            eprintln!("yggdrasil-settings: the fixture {} could not be read ({e}); answering nothing", p.to_string_lossy());
            Fixture::default()
        }))
    })
    .as_ref()
}

impl Fixture {
    /// The made-up output of a command line.
    pub fn answer(&self, cmd: &str, args: &[&str]) -> Option<String> {
        let line = std::iter::once(cmd).chain(args.iter().copied()).collect::<Vec<_>>().join(" ");
        if let Some(v) = self.run.get(&line) {
            return Some(v.clone());
        }
        self.run.iter().find(|(k, _)| k.strip_suffix('*').is_some_and(|p| line.starts_with(p))).map(|(_, v)| v.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_by_line_or_prefix() {
        let f: Fixture = serde_json::from_str(r#"{"run":{"nmcli radio wifi":"enabled\n","curl *":"{}"}}"#).unwrap();
        assert_eq!(f.answer("nmcli", &["radio", "wifi"]).as_deref(), Some("enabled\n"));
        assert_eq!(f.answer("curl", &["-s", "http://127.0.0.1:1/x"]).as_deref(), Some("{}"));
        assert_eq!(f.answer("nmcli", &["device"]), None, "not listed: no answer, and nothing is run");
    }

    /// The committed fixture for the Wi-Fi screenshots reads, and every name and address in it is a made-up one.
    #[test]
    fn the_wifi_fixture_is_made_up() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/wifi.json")).unwrap();
        let f: Fixture = serde_json::from_str(&text).unwrap();
        let scan = yggdrasil_wifiscan::parse_fixture(&f.scan);
        assert!(scan.len() >= 6);
        for b in &scan {
            // locally administered addresses, or the documentation-style ones below: none from a real place
            assert!(yggdrasil_wifiscan::oui::is_local(&b.bssid) || b.bssid[3..5] == [0xd0, 0xc5], "{}", yggdrasil_wifiscan::mac_text(&b.bssid));
        }
    }
}
