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
//!
//! "Disguise my voice" (off by default): the private microphone also shifts the voice's pitch and formants, so it is not
//! recognised at a casual listen (Heimdall's docs/VOICE_DISGUISE.md; it can be partly undone, and the row says so). A switch
//! and, while it is on, the preset (Lower or Higher): `heimdall-private-mic disguise on|off|lower|higher`. Shown only while
//! the private microphone is on and only when Heimdall's program reports it (an older one leaves the field out).

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
    /// The voice disguise, when Heimdall's program knows it: on or off, and the preset.
    pub disguise: Option<(bool, Preset)>,
}

/// The voice disguise's presets, as Heimdall's program names them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Preset {
    #[default]
    Lower,
    Higher,
}

impl Preset {
    pub const ALL: [Preset; 2] = [Preset::Lower, Preset::Higher];
    /// The segmented control's labels, in `ALL`'s order.
    pub const LABELS: [&'static str; 2] = ["Lower", "Higher"];

    /// The word `heimdall-private-mic disguise` takes.
    pub fn arg(self) -> &'static str {
        match self {
            Preset::Lower => "lower",
            Preset::Higher => "higher",
        }
    }

    pub fn index(self) -> usize {
        Preset::ALL.iter().position(|p| *p == self).unwrap_or(0)
    }
}

/// What `heimdall-private-mic status --json` says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status {
    pub enforce: bool,
    pub node: bool,
    pub lykanos: bool,
    pub sarina_raw: bool,
    pub disguise: Option<(bool, Preset)>,
}

/// One line of `heimdall-private-mic status --json`: `{"enforce":true,"node":true,"lykanos":false,"sarina_raw":false}`.
/// `enforce` and `node` must be there; the two later fields count as false when an older program leaves them out.
/// `"disguise":false,"disguise_preset":"lower"` (newer still) give `disguise`; without them it is None and the disguise row
/// is not shown. A preset name this build does not know counts as Lower. Anything else is None (the row then says the
/// state could not be read).
pub fn parse_status(text: &str) -> Option<Status> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    let later = |k: &str| match v.get(k) {
        None => Some(false),
        Some(x) => x.as_bool(),
    };
    let disguise = match v.get("disguise") {
        None => None,
        Some(d) => {
            let preset = match v.get("disguise_preset").and_then(|p| p.as_str()) {
                Some("higher") => Preset::Higher,
                _ => Preset::Lower,
            };
            Some((d.as_bool()?, preset))
        }
    };
    Some(Status {
        enforce: v.get("enforce")?.as_bool()?,
        node: v.get("node")?.as_bool()?,
        lykanos: later("lykanos")?,
        sarina_raw: later("sarina_raw")?,
        disguise,
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
    Some(Mic { installed: true, enforce: s.enforce, node: s.node, lykanos, sarina_raw: lykanos && s.sarina_raw, disguise: s.disguise })
}

/// Switch it on or off (takes effect at once, kept across restarts).
pub fn set(on: bool) -> Result<(), String> {
    sys::act(TOOL, &[if on { "on" } else { "off" }])
}

/// LykanOS only: let Sarina's voice input record the real microphone, or not.
pub fn set_sarina_raw(on: bool) -> Result<(), String> {
    sys::act(TOOL, &["sarina-raw", if on { "on" } else { "off" }])
}

/// "Disguise my voice" on or off (takes effect at once, kept across restarts).
pub fn set_disguise(on: bool) -> Result<(), String> {
    sys::act(TOOL, &["disguise", if on { "on" } else { "off" }])
}

/// Another preset (also switches the disguise on).
pub fn set_disguise_preset(p: Preset) -> Result<(), String> {
    sys::act(TOOL, &["disguise", p.arg()])
}

/// The disguise rows' titles and grey line: honest about what it does not do.
pub const DISGUISE_TITLE: &str = "Disguise my voice";
pub const DISGUISE_ABOUT: &str = "Shifts your voice's pitch and formants so it is not recognised at a casual listen. A determined \
     listener can partly undo it";
pub const DISGUISE_PRESET_TITLE: &str = "Voice";

/// The disguise rows are shown: the program knows the disguise and the private microphone is on (with it off, apps record
/// the real microphone).
pub fn disguise_row(m: Option<Mic>) -> bool {
    m.is_some_and(|m| m.installed && m.enforce && m.disguise.is_some())
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
            Some(Status { enforce: true, node: true, lykanos: true, sarina_raw: true, disguise: None })
        );
        assert_eq!(parse_status(r#"{"enforce":true,"node":true,"lykanos":"yes"}"#), None, "a later field not a boolean");
        let d =
            parse_status(r#"{"enforce":true,"node":true,"lykanos":false,"sarina_raw":false,"disguise":true,"disguise_preset":"higher"}"#);
        assert_eq!(d.map(|s| s.disguise), Some(Some((true, Preset::Higher))));
        let d = parse_status(r#"{"enforce":true,"node":true,"disguise":false,"disguise_preset":"something new"}"#);
        assert_eq!(d.map(|s| s.disguise), Some(Some((false, Preset::Lower))), "an unknown preset is Lower");
        assert_eq!(parse_status(r#"{"enforce":true,"node":true}"#).map(|s| s.disguise), Some(None), "an older program");
        assert_eq!(parse_status(r#"{"enforce":true,"node":true,"disguise":"on"}"#), None, "not a boolean");
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
    fn the_disguise_rows_need_the_program_to_know_it_and_the_microphone_on() {
        let m = Mic { installed: true, enforce: true, node: true, disguise: Some((false, Preset::Lower)), ..Mic::default() };
        assert!(disguise_row(Some(m)));
        assert!(!disguise_row(Some(Mic { disguise: None, ..m })), "an older Heimdall");
        assert!(!disguise_row(Some(Mic { enforce: false, ..m })), "the private microphone off");
        assert!(!disguise_row(Some(Mic { installed: false, ..m })));
        assert!(!disguise_row(None));
        assert_eq!(Preset::LABELS.len(), Preset::ALL.len());
        assert_eq!(Preset::Higher.index(), 1);
        assert_eq!(Preset::Higher.arg(), "higher");
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
        assert_eq!(s.disguise, Some((false, Preset::Lower)), "the screenshots show the disguise row, off");
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
