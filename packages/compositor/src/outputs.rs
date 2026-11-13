//! The screens (outputs) and what they are set to: mode, scale, place and on or off. Three ways set them, all through
//! `App::configure_outputs`:
//! - **Settings > Displays' file**, `~/.config/yggdrasil/monitors.lua`, the same file the Hyprland session loads (Settings writes
//!   it and asks the compositor to reload): `hl.monitor({ output = "DP-1", mode = "2560x1440@143.97", position = "0x0",
//!   scale = 1.25 })` and `hl.monitor({ output = "HDMI-A-1", disabled = true })` lines. Read at start, when it changes (the
//!   2-second look and `reload`), and for a screen that appears. A screen it does not name gets its own mode and scale (headless:
//!   the command line's) and the next place to the right, as the session's `mode = "highrr", position = "auto"` rule.
//! - **wlr-output-management** (outputmgmt.rs), the standard protocol (wlr-randr, kanshi): changes for as long as the session runs,
//!   as on Hyprland.
//! - the headless backend's own commands: `resize` and `scale`, and screens added and removed (`add-output`, `remove-output`).
//!
//! A screen that goes (removed or switched off) takes its panels with it (they are told `closed`) and its lock surface; the windows
//! on it move to the first screen, keeping their place on it as far as it fits, and windows on a screen that moves go with it.
//! At least one screen always stays on.

use std::path::PathBuf;
use std::time::Duration;

use smithay::output::{Mode, Output, Scale};
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{Logical, Point, Rectangle};

use crate::state::{self, App};

/// One screen the compositor knows, on or off.
pub struct Head {
    pub output: Output,
    pub enabled: bool,
    /// its wl_output while it is on
    global: Option<GlobalId>,
    /// the scale it has when nothing says otherwise (headless: the command line's)
    pub default_scale: f64,
    /// placed where Settings' file or a program said; None: to the right of the others
    pub fixed_at: Option<Point<i32, Logical>>,
}

/// What one screen is to be.
#[derive(Clone, Debug, PartialEq)]
pub struct Want {
    pub enabled: bool,
    pub mode: Mode,
    pub scale: f64,
    pub at: Option<Point<i32, Logical>>,
}

/// One `hl.monitor({ ... })` of monitors.lua. None: what the screen has by itself (its preferred mode, its default scale, the
/// next place to the right).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    /// empty: every screen no other rule names
    pub output: String,
    pub mode: Option<Mode>,
    pub at: Option<Point<i32, Logical>>,
    pub scale: Option<f64>,
    pub disabled: bool,
}

#[derive(Default)]
pub struct Heads {
    pub list: Vec<Head>,
    /// the rules of monitors.lua as last read, and when it was changed then (None inside: no file)
    pub rules: Vec<Rule>,
    rules_time: Option<Option<std::time::SystemTime>>,
    /// wlr-output-management's managers (outputmgmt.rs)
    pub mgmt: crate::outputmgmt::Mgmt,
}

pub fn monitors_path() -> PathBuf {
    yggdrasil_conf::dirs::config().join("monitors.lua")
}

/// A Lua line without its `--` comment (one inside a string stays).
fn code(line: &str) -> &str {
    let mut quote = None;
    let b = line.as_bytes();
    for i in 0..b.len() {
        match (quote, b[i]) {
            (None, b'"' | b'\'') => quote = Some(b[i]),
            (Some(q), c) if c == q => quote = None,
            (None, b'-') if b.get(i + 1) == Some(&b'-') => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The `key = value` pairs of a table's inside, split at the commas outside strings; strings unquoted.
fn pairs(inside: &str) -> Vec<(String, String)> {
    let mut parts = vec![];
    let (mut cur, mut quote) = (String::new(), None);
    for c in inside.chars() {
        match (quote, c) {
            (None, '"' | '\'') => {
                quote = Some(c);
                cur.push(c);
            }
            (Some(q), c) if c == q => {
                quote = None;
                cur.push(c);
            }
            (None, ',') => parts.push(std::mem::take(&mut cur)),
            _ => cur.push(c),
        }
    }
    parts.push(cur);
    parts
        .into_iter()
        .filter_map(|p| {
            let (k, v) = p.split_once('=')?;
            let v = v.trim();
            let v = v
                .strip_prefix('"')
                .and_then(|v| v.strip_suffix('"'))
                .or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
                .unwrap_or(v);
            Some((k.trim().to_string(), v.to_string()))
        })
        .collect()
}

/// "2560x1440@143.97" (Hz, "Hz" after it allowed) or "2560x1440" (60 Hz); the words (preferred, highrr, highres ...): None.
pub fn parse_mode(s: &str) -> Option<Mode> {
    let s = s.trim().trim_end_matches("Hz");
    let (size, rate) = match s.split_once('@') {
        Some((a, b)) => (a, b.parse::<f64>().ok().filter(|r| r.is_finite() && *r > 0.0 && *r < 1000.0)?),
        None => (s, 60.0),
    };
    let (w, h) = crate::headless::parse_size(size)?;
    Some(Mode { size: (w, h).into(), refresh: (rate * 1000.0).round() as i32 })
}

/// "1920x0", "-1920x0"; "auto" and its kin: None.
fn parse_at(s: &str) -> Option<Point<i32, Logical>> {
    let (x, y) = s.trim().split_once('x')?;
    let (x, y) = (x.parse::<i32>().ok()?, y.parse::<i32>().ok()?);
    (x.abs() <= 65536 && y.abs() <= 65536).then(|| (x, y).into())
}

/// The `hl.monitor({ ... })` calls of monitors.lua, in order (the file Settings > Displays writes, or one written by hand in
/// the same form). Anything else in it is Lua for Hyprland alone and left out.
pub fn parse_rules(text: &str) -> Vec<Rule> {
    let text: String = text.lines().map(|l| code(l).to_string() + "\n").collect();
    let mut rules = vec![];
    let mut rest = text.as_str();
    while let Some(i) = rest.find("hl.monitor(") {
        rest = &rest[i + "hl.monitor(".len()..];
        let (Some(open), Some(close)) = (rest.find('{'), rest.find('}')) else { break };
        if close < open {
            continue;
        }
        let mut r = Rule::default();
        for (k, v) in pairs(&rest[open + 1..close]) {
            match k.as_str() {
                "output" => r.output = v,
                "mode" => r.mode = parse_mode(&v),
                "position" => r.at = parse_at(&v),
                "scale" => r.scale = crate::headless::parse_scale(&v),
                "disabled" => r.disabled = v == "true",
                _ => {}
            }
        }
        rest = &rest[close + 1..];
        rules.push(r);
    }
    rules
}

/// What the rules say a screen is to be: the last rule naming it, else the last for every screen, else what it has by itself.
pub fn want_from_rules(rules: &[Rule], name: &str, preferred: Mode, default_scale: f64) -> Want {
    let rule = rules.iter().rev().find(|r| r.output == name).or_else(|| rules.iter().rev().find(|r| r.output.is_empty()));
    match rule {
        None => Want { enabled: true, mode: preferred, scale: default_scale, at: None },
        Some(r) => Want { enabled: !r.disabled, mode: r.mode.unwrap_or(preferred), scale: r.scale.unwrap_or(default_scale), at: r.at },
    }
}

impl Head {
    fn preferred(&self) -> Mode {
        self.output.preferred_mode().or_else(|| self.output.current_mode()).unwrap_or(Mode { size: (1920, 1080).into(), refresh: 60_000 })
    }

    /// What it is now.
    pub fn now(&self) -> Want {
        Want {
            enabled: self.enabled,
            mode: self.output.current_mode().unwrap_or_else(|| self.preferred()),
            scale: self.output.current_scale().fractional_scale(),
            at: self.fixed_at,
        }
    }
}

fn check(w: &Want) -> Result<(), String> {
    let (mw, mh) = (w.mode.size.w, w.mode.size.h);
    if !(1..=16384).contains(&mw) || !(1..=16384).contains(&mh) || w.mode.refresh < 0 {
        return Err(format!("{mw}x{mh}: not a mode"));
    }
    if !w.scale.is_finite() || !(0.25..=8.0).contains(&w.scale) {
        return Err(format!("scale {}: 0.25 to 8", w.scale));
    }
    if w.at.is_some_and(|p| p.x.abs() > 65536 || p.y.abs() > 65536) {
        return Err("a place within 65536 of the origin".into());
    }
    Ok(())
}

impl App {
    /// A screen appears (at start, or plugged in): its wl_output, and the mode, scale and place Settings' file gives it.
    pub fn add_output(&mut self, output: Output, default_scale: f64) {
        let name = output.name();
        self.heads.list.push(Head { output, enabled: false, global: None, default_scale, fixed_at: None });
        let want = {
            let h = self.heads.list.last().expect("just added");
            want_from_rules(&self.heads.rules, &name, h.preferred(), default_scale)
        };
        // the rules may not switch the only screen off
        let want = if self.heads.list.iter().any(|h| h.enabled) { want } else { Want { enabled: true, ..want } };
        if let Err(e) = self.configure_outputs(vec![(name.clone(), want.clone())], false) {
            eprintln!("yggdrasil-compositor: {name}: {e}; it keeps its own mode and scale");
            let plain = Want { enabled: true, mode: self.heads.list.last().expect("just added").preferred(), scale: 1.0, at: None };
            let _ = self.configure_outputs(vec![(name, plain)], false);
        }
    }

    /// A screen goes (unplugged): as switched off, then forgotten. The last one stays.
    pub fn remove_output(&mut self, name: &str) -> Result<(), String> {
        let i = self.heads.list.iter().position(|h| h.output.name() == name).ok_or_else(|| format!("no output {name}"))?;
        let mut off = self.heads.list[i].now();
        off.enabled = false;
        self.configure_outputs(vec![(name.to_string(), off)], false)?;
        self.heads.list.remove(i);
        self.output_list_changed();
        Ok(())
    }

    /// Set screens to what `changes` says (the others stay as they are). With `test_only` nothing changes: only whether it could.
    pub fn configure_outputs(&mut self, changes: Vec<(String, Want)>, test_only: bool) -> Result<(), String> {
        for (name, w) in &changes {
            if !self.heads.list.iter().any(|h| h.output.name() == *name) {
                return Err(format!("no output {name}"));
            }
            check(w).map_err(|e| format!("{name}: {e}"))?;
        }
        let wants: Vec<Want> = self
            .heads
            .list
            .iter()
            .map(|h| changes.iter().rev().find(|(n, _)| *n == h.output.name()).map(|(_, w)| w.clone()).unwrap_or_else(|| h.now()))
            .collect();
        if !wants.iter().any(|w| w.enabled) {
            return Err("at least one screen stays on".into());
        }
        if test_only {
            return Ok(());
        }
        self.wake();
        // where each screen was, for the windows on it
        let before: Vec<(String, Rectangle<i32, Logical>)> =
            self.heads.list.iter().filter(|h| h.enabled).map(|h| (h.output.name(), state::output_rect(&h.output))).collect();
        let dh = self.display_handle.clone();
        let mut gone = vec![];
        for (h, w) in self.heads.list.iter_mut().zip(wants) {
            if h.output.current_mode() != Some(w.mode) || h.output.current_scale().fractional_scale() != w.scale {
                h.output.change_current_state(Some(w.mode), None, Some(Scale::Fractional(w.scale)), None);
            }
            h.fixed_at = w.at;
            if w.enabled && !h.enabled {
                h.global = Some(h.output.create_global::<App>(&dh));
                h.enabled = true;
            } else if !w.enabled && h.enabled {
                h.enabled = false;
                gone.push(h.output.clone());
                if let Some(id) = h.global.take() {
                    // programs are told it went; the global itself goes a little later, so one binding it just now does not fail
                    dh.disable_global::<App>(id.clone());
                    let _ = self.loop_handle.insert_source(
                        smithay::reexports::calloop::timer::Timer::from_duration(Duration::from_secs(10)),
                        move |_, _, app| {
                            app.display_handle.remove_global::<App>(id.clone());
                            smithay::reexports::calloop::timer::TimeoutAction::Drop
                        },
                    );
                }
            }
        }
        for o in &gone {
            self.output_gone(o);
        }
        self.layout_outputs();
        self.windows_follow_outputs(&before);
        self.output_resized();
        self.keep_clear();
        self.output_list_changed();
        Ok(())
    }

    /// The screens that are on, in the space: each where it was put, the others side by side to the right of them all.
    fn layout_outputs(&mut self) {
        let on: Vec<(Output, Option<Point<i32, Logical>>)> =
            self.heads.list.iter().filter(|h| h.enabled).map(|h| (h.output.clone(), h.fixed_at)).collect();
        let mut right = on.iter().filter_map(|(o, at)| at.map(|p| p.x + state::logical_size(o).w)).max().unwrap_or(0);
        for (o, at) in &on {
            let p = at.unwrap_or_else(|| {
                let p = Point::from((right, 0));
                right += state::logical_size(o).w;
                p
            });
            if o.current_location() != p {
                o.change_current_state(None, None, None, Some(p));
            }
            self.space.map_output(o, p);
        }
        // the first screen that is on is the main one (windows maximize and the switcher shows there)
        if !on.iter().any(|(o, _)| *o == self.output) {
            if let Some((o, _)) = on.first() {
                self.output = o.clone();
            }
        }
    }

    /// Read monitors.lua again when it changed (or appeared, or went), and set the screens to it. `reload` and the 2-second look.
    pub fn reload_monitors(&mut self) {
        if self.heads.list.is_empty() {
            return;
        }
        let path = monitors_path();
        let t = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
        if self.heads.rules_time == Some(t) {
            return;
        }
        self.heads.rules_time = Some(t);
        self.heads.rules = std::fs::read_to_string(&path).map(|s| parse_rules(&s)).unwrap_or_default();
        let changes: Vec<(String, Want)> = self
            .heads
            .list
            .iter()
            .map(|h| (h.output.name(), want_from_rules(&self.heads.rules, &h.output.name(), h.preferred(), h.default_scale)))
            .collect();
        if let Err(e) = self.configure_outputs(changes, false) {
            eprintln!("yggdrasil-compositor: monitors.lua: {e}; the screens stay as they are");
        }
    }

    /// The screens' list or settings changed: wlr-output-management's programs are told.
    fn output_list_changed(&mut self) {
        let heads: Vec<crate::outputmgmt::HeadState> = self.heads.list.iter().map(crate::outputmgmt::HeadState::of).collect();
        let dh = self.display_handle.clone();
        self.heads.mgmt.changed(&dh, &heads);
    }

    /// A screen by name, on or off.
    pub fn head(&self, name: &str) -> Option<&Head> {
        self.heads.list.iter().find(|h| h.output.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32, mhz: i32) -> Mode {
        Mode { size: (w, h).into(), refresh: mhz }
    }

    #[test]
    fn settings_monitors_file_is_read() {
        // as packages/settings (hypr_monitors_lua) writes it
        let text = "-- Written by Settings > Displays. Loaded by Yggdrasil's Hyprland config.\n\
            hl.monitor({ output = \"DP-1\", mode = \"3440x1440@174.96\", position = \"0x0\", scale = 1 })\n\
            hl.monitor({ output = \"HDMI-A-1\", disabled = true })\n\
            hl.monitor({ output = \"DP-2\", mode = \"1920x1080@60.00\", position = \"-1920x200\", scale = 1.25 })\n";
        let r = parse_rules(text);
        assert_eq!(r.len(), 3);
        assert_eq!(
            r[0],
            Rule {
                output: "DP-1".into(),
                mode: Some(mode(3440, 1440, 174_960)),
                at: Some((0, 0).into()),
                scale: Some(1.0),
                disabled: false
            }
        );
        assert!(r[1].disabled && r[1].output == "HDMI-A-1" && r[1].mode.is_none());
        assert_eq!((r[2].at, r[2].scale), (Some((-1920, 200).into()), Some(1.25)));
        // the session's own default rule and one written by hand
        let r = parse_rules(
            "hl.monitor({ output = \"\", mode = \"highrr\", position = \"auto\", scale = 1 }) -- every screen\n\
            hl.monitor({output='X-1',mode='800x600'}) -- hl.monitor({ output = \"Y\" })\n",
        );
        assert_eq!(r.len(), 2, "a call in a comment is not one");
        assert_eq!((r[0].output.as_str(), r[0].mode, r[0].at, r[0].scale), ("", None, None, Some(1.0)));
        assert_eq!(r[1].mode, Some(mode(800, 600, 60_000)));
        assert!(parse_rules("print('hl.monitor(')").is_empty());
    }

    #[test]
    fn the_last_rule_naming_a_screen_wins_and_others_keep_their_own() {
        let rules =
            parse_rules("hl.monitor({ output = \"A\", scale = 2 })\nhl.monitor({ output = \"A\", scale = 1.5, position = \"100x0\" })\n");
        let pref = mode(1600, 900, 60_000);
        assert_eq!(want_from_rules(&rules, "A", pref, 1.0), Want { enabled: true, mode: pref, scale: 1.5, at: Some((100, 0).into()) });
        assert_eq!(want_from_rules(&rules, "B", pref, 1.25), Want { enabled: true, mode: pref, scale: 1.25, at: None });
        let all = parse_rules("hl.monitor({ output = \"\", disabled = true })\nhl.monitor({ output = \"B\", scale = 2 })\n");
        assert!(!want_from_rules(&all, "A", pref, 1.0).enabled, "the rule for every screen");
        assert!(want_from_rules(&all, "B", pref, 1.0).enabled, "a rule of its own comes first");
        assert_eq!(parse_mode("2560x1440@143.97Hz"), Some(mode(2560, 1440, 143_970)));
        assert_eq!((parse_mode("preferred"), parse_mode("0x10"), parse_mode("10x10@-1")), (None, None, None));
    }
}
