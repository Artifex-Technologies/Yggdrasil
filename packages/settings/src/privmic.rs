//! The private microphone (Settings > Security, and a row on Peripherals > Microphone): Heimdall's "Microphone (private)",
//! which every app records through, removes the sounds that can place or date a recording (aircraft overhead, mains hum).
//! Settings only asks and switches: `heimdall-private-mic status --json` when the page opens (and while it stays open, with
//! the page's own reading), `heimdall-private-mic on|off` when the switch moves. Both are argv, never a shell. Without the
//! program the row says "Not installed" and cannot be switched.
//!
//! On LykanOS only (/etc/os-release ID=lykanos, and Heimdall's program agrees) a second switch lets Sarina's voice input
//! record the real microphone (`heimdall-private-mic sarina-raw on|off`, off by default); every other app stays on the
//! private one. On Labyrinth OS and anywhere else that switch is never shown: Sarina records the private microphone like
//! every app.

use crate::sys;

/// The program, from Heimdall's package.
pub const TOOL: &str = "heimdall-private-mic";

/// What the rows show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mic {
    /// The program is installed.
    pub installed: bool,
    /// Every app is held to the private microphone (WirePlumber's heimdall.private-mic.enforce; on by default).
    pub enforce: bool,
    /// "Microphone (private)" exists in PipeWire now.
    pub node: bool,
    /// This is LykanOS (os-release says so and so does the program): the Sarina switch is shown.
    pub lykanos: bool,
    /// LykanOS only: Sarina's voice input records the real microphone (off by default).
    pub sarina_raw: bool,
}

/// What `heimdall-private-mic status --json` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub enforce: bool,
    pub node: bool,
    pub lykanos: bool,
    pub sarina_raw: bool,
}

/// One line of `heimdall-private-mic status --json`: `{"enforce":true,"node":true,"lykanos":false,"sarina_raw":false}`.
/// `enforce` and `node` must be there; the two later fields count as false when an older program leaves them out.
/// Anything else is None (the row then says the state could not be read).
pub fn parse_status(text: &str) -> Option<Status> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let later = |k: &str| match v.get(k) {
        None => Some(false),
        Some(x) => x.as_bool(),
    };
    Some(Status {
        enforce: v.get("enforce")?.as_bool()?,
        node: v.get("node")?.as_bool()?,
        lykanos: later("lykanos")?,
        sarina_raw: later("sarina_raw")?,
    })
}

/// The ID line of an os-release file is `lykanos` (quoted or not).
pub fn is_lykanos(os_release: &str) -> bool {
    os_release.lines().filter_map(|l| l.trim().strip_prefix("ID=")).any(|v| v.trim().trim_matches(|c| c == '"' || c == '\'') == "lykanos")
}

/// This system is LykanOS. A made-up system (fixture) is not.
fn this_is_lykanos() -> bool {
    crate::fixture::get().is_none() && is_lykanos(&std::fs::read_to_string("/etc/os-release").unwrap_or_default())
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
    let s = parse_status(&sys::run(TOOL, &["status", "--json"])?)?;
    // both must say LykanOS: the file here, and Heimdall's program (which honours the setting only there)
    let lykanos = s.lykanos && this_is_lykanos();
    Some(Mic { installed: true, enforce: s.enforce, node: s.node, lykanos, sarina_raw: lykanos && s.sarina_raw })
}

/// Switch it on or off (takes effect at once, kept across restarts).
pub fn set(on: bool) -> Result<(), String> {
    sys::act(TOOL, &[if on { "on" } else { "off" }])
}

/// LykanOS only: let Sarina's voice input record the real microphone, or not.
pub fn set_sarina_raw(on: bool) -> Result<(), String> {
    sys::act(TOOL, &["sarina-raw", if on { "on" } else { "off" }])
}

/// The row's grey line under the title.
pub const ABOUT: &str =
    "Removes the sounds that can place or date a call or recording (aircraft overhead, mains hum). Turn off for full bass, e.g. music";

/// The Sarina row's title and grey line (LykanOS only).
pub const SARINA_TITLE: &str = "Sarina's voice input uses the real microphone";
pub const SARINA_ABOUT: &str = "Off: Sarina hears the private microphone like every app. On: only her voice input records the real \
     microphone; every other app stays private";

/// The Sarina row is shown: LykanOS, the program installed and the private microphone on (with it off there is nothing to
/// make an exception from).
pub fn sarina_row(m: Option<Mic>) -> bool {
    m.is_some_and(|m| m.installed && m.lykanos && m.enforce)
}

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
        let on = Status { enforce: true, node: true, ..Status::default() };
        assert_eq!(parse_status("{\"enforce\":true,\"node\":true}\n"), Some(on), "the older two-field form");
        assert_eq!(parse_status(r#"{"node": false, "enforce": false}"#), Some(Status::default()));
        assert_eq!(
            parse_status(r#"{"enforce":true,"node":true,"lykanos":true,"sarina_raw":true}"#),
            Some(Status { enforce: true, node: true, lykanos: true, sarina_raw: true })
        );
        assert_eq!(parse_status(r#"{"enforce":true,"node":true,"lykanos":"yes"}"#), None, "a later field not a boolean");
        assert_eq!(parse_status(r#"{"enforce":"yes","node":true}"#), None, "not a boolean");
        assert_eq!(parse_status(r#"{"enforce":true}"#), None, "a field missing");
        assert_eq!(parse_status("enforce: on"), None);
        assert_eq!(parse_status(""), None);
    }

    #[test]
    fn os_release_says_lykanos_or_not() {
        assert!(is_lykanos("NAME=\"LykanOS\"\nID=lykanos\nID_LIKE=arch\n"));
        assert!(is_lykanos("ID=\"lykanos\"\n"));
        assert!(!is_lykanos("NAME=\"Labyrinth OS\"\nID=labyrinth\nID_LIKE=arch\n"));
        assert!(!is_lykanos("ID=arch\nID_LIKE=lykanos\n"), "only ID counts");
        assert!(!is_lykanos(""));
    }

    #[test]
    fn the_sarina_row_is_lykanos_only() {
        let m = Mic { installed: true, enforce: true, node: true, ..Mic::default() };
        assert!(!sarina_row(Some(m)), "Labyrinth and anywhere else: never");
        assert!(sarina_row(Some(Mic { lykanos: true, ..m })));
        assert!(!sarina_row(Some(Mic { lykanos: true, enforce: false, ..m })), "nothing to except while it is off");
        assert!(!sarina_row(Some(Mic { lykanos: true, installed: false, ..m })));
        assert!(!sarina_row(None));
    }

    #[test]
    fn the_row_says_why_it_cannot_be_switched() {
        assert_eq!(note(None), Some("Could not be read"));
        assert_eq!(note(Some(Mic::default())), Some("Not installed"));
        assert_eq!(note(Some(Mic { installed: true, node: true, ..Mic::default() })), None);
    }

    /// The desktop fixture (screenshots) has it installed and on, and is not LykanOS.
    #[test]
    fn the_desktop_fixture_shows_it_on() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/system.json")).unwrap();
        let f: crate::fixture::Fixture = serde_json::from_str(&text).unwrap();
        assert!(f.tools.iter().any(|t| t == TOOL));
        let s = parse_status(&f.answer(TOOL, &["status", "--json"]).unwrap()).unwrap();
        assert!(s.enforce && s.node && !s.lykanos && !s.sarina_raw);
    }

    #[test]
    fn a_missing_node_is_said_only_while_on() {
        let m = Mic { installed: true, ..Mic::default() };
        assert!(warning(Mic { enforce: true, ..m }).is_some());
        assert!(warning(m).is_none());
        assert!(warning(Mic { enforce: true, node: true, ..m }).is_none());
        assert!(warning(Mic::default()).is_none());
    }
}
