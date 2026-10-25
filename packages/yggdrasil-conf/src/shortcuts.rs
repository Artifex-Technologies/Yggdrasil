//! The keyboard shortcuts as people read them ("Win + Shift + S", "Screenshots"), shared by Settings > Keyboard Shortcuts, the panel's
//! cheat sheet (Super+/) and docs/SHORTCUTS.md. Live binds come from the compositor (on Hyprland `hyprctl -j binds`, parsed by the
//! caller into `Bind`); the doc is made from the session's own file (`from_lua`), and a test keeps it current.

/// One bind as the compositor reports it: Hyprland's modifier mask (Shift 1, Ctrl 4, Alt 8, Super 64), the key and its description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bind {
    pub modmask: u32,
    pub key: String,
    pub desc: String,
    pub release: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub keys: String,
    pub what: String,
    pub group: &'static str,
}

/// The groups, in the order they are shown.
pub const GROUPS: [&str; 8] = ["Windows", "Desktops", "Apps", "System", "Mouse", "Screenshots", "Media keys", "Other"];

/// A key's name as written on the keyboard ("Esc", "←", "/"); empty for a modifier pressed on its own (Super for the start menu).
pub fn key_name(key: &str) -> String {
    let named = match key.to_ascii_uppercase().as_str() {
        "CODE:10" => "1", "CODE:11" => "2", "CODE:12" => "3", "CODE:13" => "4", "CODE:14" => "5",
        "RETURN" => "Return", "ESCAPE" => "Esc", "PRINT" => "Print Screen", "SPACE" => "Space", "TAB" => "Tab",
        "UP" => "↑", "DOWN" => "↓", "LEFT" => "←", "RIGHT" => "→", "SLASH" => "/", "GRAVE" => "`", "MINUS" => "−", "EQUAL" => "=",
        "SUPER_L" | "SUPER_R" | "CONTROL_L" | "CONTROL_R" | "ALT_L" | "ALT_R" | "SHIFT_L" | "SHIFT_R" => return String::new(),
        "MOUSE:272" => "Left drag", "MOUSE:273" => "Right drag", "MOUSE:274" => "Middle click",
        _ => "",
    };
    if !named.is_empty() {
        return named.to_string();
    }
    if let Some(rest) = key.strip_prefix("XF86") {
        return media_name(rest).unwrap_or(rest).to_string();
    }
    let mut c = key.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + &c.as_str().to_lowercase()).unwrap_or_default()
}

fn media_name(k: &str) -> Option<&'static str> {
    Some(match k {
        "AudioRaiseVolume" => "Volume up",
        "AudioLowerVolume" => "Volume down",
        "AudioMute" => "Mute",
        "AudioMicMute" => "Microphone mute",
        "AudioPlay" => "Play",
        "AudioPause" => "Pause",
        "AudioNext" => "Next track",
        "AudioPrev" => "Previous track",
        "AudioStop" => "Stop",
        "MonBrightnessUp" => "Brightness up",
        "MonBrightnessDown" => "Brightness down",
        _ => return None,
    })
}

/// A bind's keys, as on Windows ("Win + Shift + Q"). A modifier on its own reads as itself ("Win", "Ctrl").
pub fn keys(modmask: u32, key: &str) -> String {
    let mut parts: Vec<String> = vec![];
    for (bit, name) in [(64, "Win"), (4, "Ctrl"), (8, "Alt"), (1, "Shift")] {
        if modmask & bit != 0 {
            parts.push(name.to_string());
        }
    }
    let k = key_name(key);
    if !k.is_empty() {
        parts.push(k);
    }
    parts.join(" + ")
}

/// Which group a shortcut belongs in, from its key and what it does.
pub fn group(key: &str, what: &str) -> &'static str {
    let w = what.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|x| w.contains(x));
    if key.starts_with("XF86") {
        "Media keys"
    } else if key.to_ascii_lowercase().starts_with("mouse:") || has(&["mouse", "pointer"]) {
        "Mouse"
    } else if has(&["screenshot", "screen recording"]) {
        "Screenshots"
    } else if has(&["desktop", "workspace"]) {
        "Desktops"
    } else if has(&["window", "maximize", "minimize", "full screen", "half", "tile", "snap"]) {
        "Windows"
    } else if has(&["lock", "start menu", "notification", "calendar", "settings", "running programs", "shortcuts", "log out", "power"]) {
        "System"
    } else if w.is_empty() {
        "Other"
    } else {
        "Apps"
    }
}

/// The list to show: each shortcut once, the release half of a hold-to-talk pair left out, binds with no description named after
/// their key when it says what it does (the media keys) and listed under Other when it does not. Sorted by group.
pub fn list(binds: &[Bind]) -> Vec<Shortcut> {
    let mut out: Vec<Shortcut> = vec![];
    for b in binds {
        let mut what = b.desc.trim().to_string();
        if b.release && what.contains("let go") {
            continue;
        }
        let k = keys(b.modmask, &b.key);
        if k.is_empty() {
            continue;
        }
        if what.is_empty() {
            // a second action on keys already listed (Alt+Tab also raises the window): not a shortcut of its own
            if out.iter().any(|s| s.keys == k) {
                continue;
            }
            what = b.key.strip_prefix("XF86").and_then(media_name).map(str::to_string).unwrap_or_else(|| "No description".into());
        }
        let group = if what == "No description" { "Other" } else { group(&b.key, &what) };
        if !out.iter().any(|s| s.keys == k && s.what == what) {
            out.push(Shortcut { keys: k, what, group });
        }
    }
    out.sort_by_key(|s| GROUPS.iter().position(|g| *g == s.group).unwrap_or(GROUPS.len()));
    out
}

/// "SUPER + CTRL + LEFT" as Hyprland's mask and key.
fn parse_combo(s: &str) -> (u32, String) {
    let mut mask = 0;
    let mut key = String::new();
    for part in s.split('+').map(str::trim) {
        match part.to_ascii_uppercase().as_str() {
            "SUPER" => mask |= 64,
            "CTRL" | "CONTROL" => mask |= 4,
            "ALT" => mask |= 8,
            "SHIFT" => mask |= 1,
            _ => key = part.to_string(),
        }
    }
    (mask, key)
}

/// The first two "..." strings on a line (the keys and the description), with `" .. n` filled in by `fill`.
fn quoted(line: &str, fill: &str) -> Vec<String> {
    let mut out = vec![];
    let mut rest = line;
    while let Some(a) = rest.find('"') {
        let after = &rest[a + 1..];
        let Some(b) = after.find('"') else { break };
        let mut s = after[..b].to_string();
        let tail = after[b + 1..].trim_start();
        // "SUPER + CTRL + " .. key   /   "Desktop " .. n
        if let Some(t) = tail.strip_prefix("..") {
            let t = t.trim_start();
            if t.starts_with("key") || t.starts_with('n') {
                s.push_str(fill);
            }
        }
        out.push(s);
        rest = &after[b + 1..];
    }
    out
}

/// The shortcuts a session file (yggdrasil.lua) sets: `cmd("KEYS", ..., { desc = "..." })` and `dsp(...)` lines, the numbered
/// desktop loop shown once as "1-4", the media keys from their table.
pub fn from_lua(text: &str) -> Vec<Shortcut> {
    let mut binds: Vec<Bind> = vec![];
    let mut loop_range: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with("--") {
            continue;
        }
        if let Some(r) = t.strip_prefix("for n = ") {
            // for n = 1, 4 do
            let nums: Vec<&str> = r.trim_end_matches("do").split(',').map(str::trim).collect();
            if nums.len() == 2 {
                loop_range = Some(format!("{}-{}", nums[0], nums[1]));
            }
            continue;
        }
        if t == "end" {
            loop_range = None;
            continue;
        }
        // only binds whose keys are written out (the media keys' loop passes a variable: those come from their table below)
        if t.starts_with("cmd(\"") || t.starts_with("dsp(\"") {
            let q = quoted(t, loop_range.as_deref().unwrap_or(""));
            let Some(combo) = q.first() else { continue };
            let desc = t.find("desc = \"").and_then(|i| quoted(&t[i..], loop_range.as_deref().unwrap_or("")).into_iter().next()).unwrap_or_default();
            let (modmask, key) = parse_combo(combo);
            binds.push(Bind { modmask, key, desc, release: t.contains("release = true") });
            continue;
        }
        // the media keys' table: XF86AudioRaiseVolume = "volume-up", ...
        for part in t.split(',') {
            if let Some((k, _)) = part.split_once('=') {
                let k = k.trim();
                if k.starts_with("XF86") && !k.contains(' ') {
                    binds.push(Bind { modmask: 0, key: k.to_string(), desc: String::new(), release: false });
                }
            }
        }
    }
    list(&binds)
}

/// docs/SHORTCUTS.md from the shortcuts.
pub fn markdown(list: &[Shortcut]) -> String {
    let mut s = String::from(
        "# Keyboard shortcuts\n\nYggdrasil's shortcuts on Hyprland, as `packages/session/hyprland/yggdrasil.lua` sets them (this file is made from it:\n\
         `YGGDRASIL_WRITE_DOCS=1 cargo test -p yggdrasil-conf` in `packages/yggdrasil-conf` writes it again; a test fails while it is out\n\
         of date). Settings > Keyboard Shortcuts and the cheat sheet (Win + /) show the live list, your own additions included.\n",
    );
    for g in GROUPS {
        let rows: Vec<&Shortcut> = list.iter().filter(|x| x.group == g).collect();
        if rows.is_empty() {
            continue;
        }
        s.push_str(&format!("\n## {g}\n\n| Keys | What it does |\n| --- | --- |\n"));
        for r in rows {
            s.push_str(&format!("| {} | {} |\n", r.keys.replace('|', "\\|"), r.what.replace('|', "\\|")));
        }
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn b(modmask: u32, key: &str, desc: &str) -> Bind {
        Bind { modmask, key: key.into(), desc: desc.into(), release: false }
    }

    #[test]
    fn keys_read_like_a_keyboard() {
        assert_eq!(keys(65, "M"), "Win + Shift + M");
        assert_eq!(keys(68, "LEFT"), "Win + Ctrl + ←");
        assert_eq!(keys(64, "SUPER_L"), "Win");
        assert_eq!(keys(4, "CONTROL_L"), "Ctrl");
        assert_eq!(keys(64, "mouse:272"), "Win + Left drag");
        assert_eq!(keys(64, "SLASH"), "Win + /");
        assert_eq!(keys(0, "XF86AudioRaiseVolume"), "Volume up");
        assert_eq!(keys(5, "ESCAPE"), "Ctrl + Shift + Esc");
    }

    #[test]
    fn grouped_once_each_with_the_unnamed_under_other() {
        let mut talk = b(64, "S", "Talk to the assistant (let go)");
        talk.release = true;
        let l = list(&[
            b(8, "TAB", "Next window"),
            b(8, "TAB", ""),
            b(68, "1", "Desktop 1"),
            b(64, "E", "Files (Minos)"),
            b(64, "S", "Talk to the assistant (hold)"),
            talk,
            b(0, "XF86AudioMute", ""),
            b(64, "mouse:272", "Move the window"),
            b(65, "S", "Screenshot (area, window or screen)"),
            b(64, "L", "Lock the screen"),
            b(64, "Z", ""),
            b(64, "E", "Files (Minos)"),
        ]);
        let get = |k: &str| l.iter().find(|s| s.keys == k).map(|s| (s.what.as_str(), s.group));
        assert_eq!(get("Alt + Tab"), Some(("Next window", "Windows")));
        assert_eq!(get("Win + Ctrl + 1"), Some(("Desktop 1", "Desktops")));
        assert_eq!(get("Win + E"), Some(("Files (Minos)", "Apps")));
        assert_eq!(get("Mute"), Some(("Mute", "Media keys")));
        assert_eq!(get("Win + Left drag"), Some(("Move the window", "Mouse")));
        assert_eq!(get("Win + Shift + S"), Some(("Screenshot (area, window or screen)", "Screenshots")));
        assert_eq!(get("Win + L"), Some(("Lock the screen", "System")));
        assert_eq!(get("Win + Z"), Some(("No description", "Other")));
        assert_eq!(l.iter().filter(|s| s.keys == "Win + S").count(), 1, "the let-go half is left out");
        assert_eq!(l.iter().filter(|s| s.keys == "Win + E").count(), 1);
        assert_eq!(l.first().map(|s| s.group), Some("Windows"));
        assert_eq!(l.last().map(|s| s.group), Some("Other"));
    }

    #[test]
    fn the_session_file_is_read() {
        let lua = r#"
cmd("SUPER + E", "minos", { desc = "Files (Minos)" })
dsp("SUPER + F", hl.dsp.window.fullscreen({ mode = "fullscreen" }), { desc = "Full screen" })
-- cmd("SUPER + X", "x", { desc = "commented out" })
for n = 1, 4 do
  local key = tostring(n)
  dsp("SUPER + CTRL + " .. key, hl.dsp.focus({ workspace = tostring(n) }), { desc = "Desktop " .. n })
end
for key, what in pairs({
  XF86AudioRaiseVolume = "volume-up", XF86AudioMute = "mute",
}) do
"#;
        let l = from_lua(lua);
        let get = |k: &str| l.iter().find(|s| s.keys == k).map(|s| s.what.clone());
        assert_eq!(get("Win + E").as_deref(), Some("Files (Minos)"));
        assert_eq!(get("Win + F").as_deref(), Some("Full screen"));
        assert_eq!(get("Win + Ctrl + 1-4").as_deref(), Some("Desktop 1-4"));
        assert_eq!(get("Volume up").as_deref(), Some("Volume up"));
        assert!(get("Win + X").is_none());
    }

    /// docs/SHORTCUTS.md is made from the session file; with YGGDRASIL_WRITE_DOCS=1 this writes it, otherwise it fails when stale.
    #[test]
    fn the_shortcuts_doc_is_current() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let Ok(lua) = std::fs::read_to_string(root.join("packages/session/hyprland/yggdrasil.lua")) else { return };
        let want = markdown(&from_lua(&lua));
        let doc = root.join("docs/SHORTCUTS.md");
        if std::env::var_os("YGGDRASIL_WRITE_DOCS").is_some() {
            std::fs::write(&doc, &want).unwrap();
        }
        let have = std::fs::read_to_string(&doc).unwrap_or_default();
        assert!(have == want, "docs/SHORTCUTS.md is out of date: YGGDRASIL_WRITE_DOCS=1 cargo test in packages/yggdrasil-conf");
    }
}
