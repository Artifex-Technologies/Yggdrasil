//! The privacy pack's switches (Security > Privacy, under the private microphone), all on by default:
//!
//! - **Check downloads**: Heimdall's downloads guard (`heimdall-fileguard`, a user service) tells you when a finished
//!   download would contact a server as soon as it is opened. Pref `downloads_guard`; the running guard follows the file.
//! - **Check new files in my folders**: the same guard on Desktop, Documents, Pictures, Music and Videos, and a scan offered
//!   when a drive is connected. Pref `folders_guard`; it needs Check downloads on (that switch turns the whole guard off).
//! - **Remove hidden info in Minos**: Minos' right-click entry that writes a copy without the hidden information
//!   (`heimdall-strip`). Pref `remove_hidden_info`.
//! - **Viewer apps ask before going online**: the operating system's per-app network rules start PDF viewers, office suites,
//!   image viewers and media players on "Ask" (`labyrinth-appnet viewers status --json` / `pkexec labyrinth-appnet viewers
//!   on|off`, through the apps integration's `network` command). A system's own setting per person, so it asks for the password.
//! - **Warn about look-alike names**: a warning beside app names that imitate others (letters of another alphabet, hidden
//!   characters) in Settings > Apps and Agora. Pref `lookalike_warnings`.
//!
//! The prefs are the person's file `~/.config/heimdall/privacy.json` (yggdrasil_ui::privacy_prefs). A made-up system (fixture)
//! reads none of them: every switch shows on, the tools are its `tools`, and the viewers' answer is its `run` entry.

use yggdrasil_ui::privacy_prefs as prefs;

/// Heimdall's downloads guard and its tool for clean copies.
pub const GUARD_TOOL: &str = "heimdall-fileguard";
pub const STRIP_TOOL: &str = "heimdall-strip";
/// The command a made-up system answers for the viewers' rule (the real one is the apps integration's `network` path).
const FIXTURE_NET: &str = "labyrinth-appnet";

/// The viewers' rule: whether it is on for this person, and how many apps it covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Viewers {
    pub on: bool,
    pub apps: usize,
}

/// What the rows show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pack {
    pub guard_installed: bool,
    pub guard_on: bool,
    pub folders_on: bool,
    pub strip_installed: bool,
    pub strip_on: bool,
    /// None: this system's app rules have no viewers' rule (or it could not be read).
    pub viewers: Option<Viewers>,
    pub lookalike_on: bool,
}

/// `labyrinth-appnet viewers status --json`: `{"on":true,"apps":["org.gnome.Evince", ...]}`. Anything else is None.
pub fn parse_viewers(text: &str) -> Option<Viewers> {
    let v: serde_json::Value = serde_json::from_str(text.trim()).ok()?;
    Some(Viewers { on: v.get("on")?.as_bool()?, apps: v.get("apps")?.as_array()?.len() })
}

/// The program is on PATH (no process started). A made-up system answers from its fixture's `tools`.
pub fn installed(tool: &str) -> bool {
    if let Some(f) = crate::fixture::get() {
        return f.tools.iter().any(|t| t == tool);
    }
    let Some(path) = std::env::var_os("PATH") else { return false };
    std::env::split_paths(&path).any(|d| {
        std::fs::metadata(d.join(tool)).is_ok_and(|m| m.is_file() && std::os::unix::fs::PermissionsExt::mode(&m.permissions()) & 0o111 != 0)
    })
}

/// One of the person's switches (always on for a made-up system).
pub fn pref(key: &str) -> bool {
    crate::fixture::get().is_some() || prefs::get(key)
}

/// Set one of the person's switches (refused for a made-up system, like every change there).
pub fn set_pref(key: &'static str, on: bool) -> Result<(), String> {
    if crate::fixture::get().is_some() {
        return Err("A made-up system (a fixture): nothing is changed.".into());
    }
    prefs::set(key, on)
}

/// The command of the apps' network rules: the integration's `network` path (a made-up system: the fixture's name).
fn network_command() -> Option<String> {
    if crate::fixture::get().is_some() {
        return installed(FIXTURE_NET).then(|| FIXTURE_NET.to_string());
    }
    crate::appmgr::rules().and_then(|r| r.path("network")).map(|p| p.to_string_lossy().into_owned())
}

/// Read every switch (on a background thread: it asks the app rules' command).
pub fn load() -> Pack {
    let viewers = network_command().and_then(|net| crate::sys::run(&net, &["viewers", "status", "--json"])).and_then(|t| parse_viewers(&t));
    Pack {
        guard_installed: installed(GUARD_TOOL),
        guard_on: pref(prefs::DOWNLOADS_GUARD),
        folders_on: pref(prefs::FOLDERS_GUARD),
        strip_installed: installed(STRIP_TOOL),
        strip_on: pref(prefs::REMOVE_HIDDEN_INFO),
        viewers,
        lookalike_on: pref(prefs::LOOKALIKE_WARNINGS),
    }
}

/// Switch the viewers' rule for this person (asks for the password).
pub fn set_viewers(on: bool) -> Result<(), String> {
    let net = network_command().ok_or("this system has no per-app network rules")?;
    crate::sys::act("pkexec", &[&net, "viewers", if on { "on" } else { "off" }])
}

/// The rows' titles and grey lines.
pub const GUARD_TITLE: &str = "Check downloads";
pub const GUARD_ABOUT: &str = "Tells you when a finished download would contact a server as soon as it is opened (remote templates, \
     PDF actions, playlists and shortcuts that point online). The file itself is never changed";
pub const FOLDERS_TITLE: &str = "Check new files in my folders";
pub const FOLDERS_ABOUT: &str = "Also checks files that appear in Desktop, Documents, Pictures, Music and Videos, and offers a scan \
     when a drive is connected";
pub const STRIP_TITLE: &str = "Remove hidden info in Minos";
pub const STRIP_ABOUT: &str = "Right-click a photo, video, song or document to save a copy without where it was made, the device, \
     the author and other hidden details";
pub const VIEWERS_TITLE: &str = "Viewer apps ask before going online";
pub const VIEWERS_ABOUT: &str = "PDF viewers, office apps, image viewers and media players ask you the first time they try to connect, \
     so opening a file cannot quietly tell a server you did";
pub const LOOKALIKE_TITLE: &str = "Warn about look-alike names";
pub const LOOKALIKE_ABOUT: &str = "Marks app names that imitate others with letters from another alphabet or hidden characters, \
     and shows the program they really are";

/// The words on the right of a row that cannot be switched, or None when the switch shows.
pub fn note(installed: bool) -> Option<&'static str> {
    (!installed).then_some("Not installed")
}

/// The viewers' row note: None when it can be switched.
/// The folders' row note: None when it can be switched (the guard installed and Check downloads on).
pub fn folders_note(installed: bool, guard_on: bool) -> Option<&'static str> {
    if !installed {
        Some("Not installed")
    } else if !guard_on {
        Some("Needs Check downloads")
    } else {
        None
    }
}

pub fn viewers_note(v: Option<Viewers>) -> Option<&'static str> {
    v.is_none().then_some("Not available")
}

/// The program an app's desktop entry starts: the Exec line's first word (after `env` and its VAR=value words), unquoted, and
/// found on PATH when it is not a path already. A program that is not found is shown as written.
pub fn program_of(exec: &str) -> String {
    let all = words(exec);
    let mut it = all.iter().map(String::as_str);
    let mut first = it.next().unwrap_or_default();
    if first == "env" || first.ends_with("/env") {
        first = it.find(|w| !w.contains('=') && !w.starts_with('-')).unwrap_or_default();
    }
    if first.is_empty() || first.contains('/') {
        return first.to_string();
    }
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).map(|d| d.join(first)).find(|c| c.is_file()))
        .map(|c| c.to_string_lossy().into_owned())
        .unwrap_or_else(|| first.to_string())
}

/// An Exec line's words: split at spaces, a double-quoted part kept whole (with \" and \\ inside), as the desktop entry
/// specification has it.
fn words(exec: &str) -> Vec<String> {
    let (mut out, mut cur, mut quoted, mut any) = (vec![], String::new(), false, false);
    let mut chars = exec.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any = true;
            }
            '\\' if quoted => cur.extend(chars.next()),
            c if c.is_whitespace() && !quoted => {
                if any || !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
                any = false;
            }
            c => cur.push(c),
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// What Settings > Apps shows beside a name that imitates another: the reasons in plain words, the name with its hidden
/// characters shown, and the program it really starts. None for an ordinary name (or with the warnings off).
pub fn name_warning(name: &str, exec: &str, warnings_on: bool) -> Option<(String, String)> {
    if !warnings_on {
        return None;
    }
    let r = yggdrasil_ui::lookalike::check(name);
    if !r.suspicious() {
        return None;
    }
    let real = program_of(exec);
    let what = if real.is_empty() { String::new() } else { format!(" It starts {real}.") };
    Some((r.words(), format!("Its name as written: {}.{what}", yggdrasil_ui::lookalike::visible(name))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_viewers_answer_parses() {
        assert_eq!(parse_viewers(r#"{"on":true,"apps":["a","b"]}"#), Some(Viewers { on: true, apps: 2 }));
        assert_eq!(parse_viewers("{\"on\": false, \"apps\": []}\n"), Some(Viewers { on: false, apps: 0 }));
        assert_eq!(parse_viewers(r#"{"on":"yes","apps":[]}"#), None);
        assert_eq!(parse_viewers(r#"{"on":true}"#), None);
        assert_eq!(parse_viewers("labyrinth-appnet: unknown command (see --help)"), None, "an older program");
        assert_eq!(viewers_note(None), Some("Not available"));
        assert_eq!(viewers_note(Some(Viewers::default())), None);
        assert_eq!(note(false), Some("Not installed"));
        assert_eq!(note(true), None);
    }

    #[test]
    fn the_program_of_an_exec_line() {
        assert_eq!(program_of("/usr/bin/vivaldi-stable %U"), "/usr/bin/vivaldi-stable");
        assert_eq!(program_of("\"/opt/My App/app\" --x"), "/opt/My App/app");
        assert_eq!(program_of("env GDK_BACKEND=x11 /usr/bin/foo %f"), "/usr/bin/foo");
        assert_eq!(program_of("surely-not-a-program-here-xyz %u"), "surely-not-a-program-here-xyz");
        assert_eq!(program_of(""), "");
        assert!(program_of("sh -c true").ends_with("/sh"), "found on PATH");
    }

    #[test]
    fn a_look_alike_name_is_explained() {
        assert_eq!(name_warning("Vivaldi", "/usr/bin/vivaldi-stable", true), None);
        let (why, what) = name_warning("Viv\u{0430}ldi", "/home/x/.local/bin/v %U", true).unwrap();
        assert!(why.contains("Latin and Cyrillic"), "{why}");
        assert!(why.contains("\"Vivaldi\""), "{why}");
        assert_eq!(what, "Its name as written: Viv\u{0430}ldi. It starts /home/x/.local/bin/v.");
        let (_, what) = name_warning("Fire\u{200B}fox", "", true).unwrap();
        assert_eq!(what, "Its name as written: Fire<U+200B>fox.");
        assert_eq!(name_warning("Viv\u{0430}ldi", "/usr/bin/v", false), None, "warnings switched off");
    }

    /// The desktop fixture (screenshots): both tools installed, the viewers' rule on, every switch on.
    #[test]
    fn the_folders_switch_needs_the_guard() {
        assert_eq!(folders_note(true, true), None);
        assert_eq!(folders_note(true, false), Some("Needs Check downloads"));
        assert_eq!(folders_note(false, true), Some("Not installed"));
    }

    #[test]
    fn the_desktop_fixture_answers_the_rows() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/system.json")).unwrap();
        let f: crate::fixture::Fixture = serde_json::from_str(&text).unwrap();
        for t in [GUARD_TOOL, STRIP_TOOL, FIXTURE_NET] {
            assert!(f.tools.iter().any(|x| x == t), "{t}");
        }
        let v = parse_viewers(&f.answer(FIXTURE_NET, &["viewers", "status", "--json"]).unwrap()).unwrap();
        assert!(v.on && v.apps > 0);
    }
}
