//! The private microphone (Settings > Security, and a row on Peripherals > Microphone): Heimdall's "Microphone (private)",
//! which every app records through, removes the sounds that can place or date a recording (aircraft overhead, mains hum).
//! Settings only asks and switches: `heimdall-private-mic status --json` when the page opens (and while it stays open, with
//! the page's own reading), `heimdall-private-mic on|off` when the switch moves. Both are argv, never a shell. Without the
//! program the row says "Not installed" and cannot be switched.

use crate::sys;

/// The program, from Heimdall's package.
pub const TOOL: &str = "heimdall-private-mic";

/// What the row shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mic {
    /// The program is installed.
    pub installed: bool,
    /// Every app is held to the private microphone (WirePlumber's heimdall.private-mic.enforce; on by default).
    pub enforce: bool,
    /// "Microphone (private)" exists in PipeWire now.
    pub node: bool,
}

/// One line of `heimdall-private-mic status --json`: `{"enforce":true,"node":true}`. Anything else is None (the row then
/// says the state could not be read).
pub fn parse_status(text: &str) -> Option<(bool, bool)> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    Some((v.get("enforce")?.as_bool()?, v.get("node")?.as_bool()?))
}

/// The program is on PATH (no process started to find out). A made-up system answers from its fixture's `tools`.
fn installed() -> bool {
    if let Some(f) = crate::fixture::get() {
        return f.tools.iter().any(|t| t == TOOL);
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|d| {
        let p = d.join(TOOL);
        std::fs::metadata(&p).is_ok_and(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0)
    })
}

/// Read the state (on a background thread: it asks WirePlumber).
pub fn load() -> Option<Mic> {
    if !installed() {
        return Some(Mic::default());
    }
    let (enforce, node) = parse_status(&sys::run(TOOL, &["status", "--json"])?)?;
    Some(Mic { installed: true, enforce, node })
}

/// Switch it on or off (takes effect at once, kept across restarts).
pub fn set(on: bool) -> Result<(), String> {
    sys::act(TOOL, &[if on { "on" } else { "off" }])
}

/// The row's grey line under the title.
pub const ABOUT: &str =
    "Removes the sounds that can place or date a call or recording (aircraft overhead, mains hum). Turn off for full bass, e.g. music";

/// The words on the right of the row when it cannot be switched, or None when the switch shows.
pub fn note(m: Option<Mic>) -> Option<&'static str> {
    match m {
        None => Some("Could not be read"),
        Some(m) if !m.installed => Some("Not installed"),
        Some(_) => None,
    }
}

/// The line under the switch while it is on but the node is missing (PipeWire not restarted since the install, say).
pub fn warning(m: Mic) -> Option<&'static str> {
    (m.installed && m.enforce && !m.node).then_some("On, but \"Microphone (private)\" is not running: log out and in again")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_line_parses() {
        assert_eq!(parse_status("{\"enforce\":true,\"node\":true}\n"), Some((true, true)));
        assert_eq!(parse_status(r#"{"node": false, "enforce": false}"#), Some((false, false)));
        assert_eq!(parse_status(r#"{"enforce":"yes","node":true}"#), None, "not a boolean");
        assert_eq!(parse_status(r#"{"enforce":true}"#), None, "a field missing");
        assert_eq!(parse_status("enforce: on"), None);
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn the_row_says_why_it_cannot_be_switched() {
        assert_eq!(note(None), Some("Could not be read"));
        assert_eq!(note(Some(Mic::default())), Some("Not installed"));
        assert_eq!(note(Some(Mic { installed: true, enforce: false, node: true })), None);
    }

    /// The desktop fixture (screenshots) has it installed and on.
    #[test]
    fn the_desktop_fixture_shows_it_on() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/system.json")).unwrap();
        let f: crate::fixture::Fixture = serde_json::from_str(&text).unwrap();
        assert!(f.tools.iter().any(|t| t == TOOL));
        assert_eq!(parse_status(&f.answer(TOOL, &["status", "--json"]).unwrap()), Some((true, true)));
    }

    #[test]
    fn a_missing_node_is_said_only_while_on() {
        assert!(warning(Mic { installed: true, enforce: true, node: false }).is_some());
        assert!(warning(Mic { installed: true, enforce: false, node: false }).is_none());
        assert!(warning(Mic { installed: true, enforce: true, node: true }).is_none());
        assert!(warning(Mic::default()).is_none());
    }
}
