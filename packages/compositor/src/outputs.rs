//! The screens (outputs) and what they are set to: mode, scale, place, turn (transform) and on or off. Four ways set them, all
//! through `App::configure_outputs`:
//! - **Settings > Displays' file**, `~/.config/yggdrasil/monitors.lua`, the same file the Hyprland session loads (Settings writes
//!   it and asks the compositor to reload): `hl.monitor({ output = "DP-1", mode = "2560x1440@143.97", position = "0x0",
//!   scale = 1.25 })` and `hl.monitor({ output = "HDMI-A-1", disabled = true })` lines. Read at start, when it changes (the
//!   config files' watch, configs.rs, and `reload`), and for a screen that appears. A screen it does not name gets its own
//!   size at the fastest rate it has for it (`own_mode`: an ultrawide at 3440x1440@180, not the 60 Hz its EDID names first),
//!   its own scale (headless: the command line's) and the next place to the right, as the Hyprland session does for a screen
//!   nothing names (packages/session/hyprland/yggdrasil.lua).
//! - **wlr-output-management** (outputmgmt.rs), the standard protocol (wlr-randr, kanshi): changes for as long as the session runs,
//!   as on Hyprland.
//! - the headless backend's own commands: `resize` and `scale`, and screens added and removed (`add-output`, `remove-output`).
//! - the control socket's `transform NAME N` on every backend (the tablet watcher turns a 2-in-1's screen with it).
//!
//! A turn is wl_output's numbering, as Hyprland's and Settings' `transform`: 0 upright, 1 to 3 the picture turned 90° counter-
//! clockwise for each step, 4 to 7 the same mirrored. A screen keeps its turn until something sets another (monitors.lua's
//! `transform`, when it says one; so reading Settings' file again does not undo the tablet watcher's turn).
//!
//! A screen that goes (removed or switched off) takes its panels with it (they are told `closed`) and its lock surface; the windows
//! on it move to the first screen, keeping their place on it as far as it fits, and windows on a screen that moves go with it.
//! At least one screen always stays on. A screen whose link goes (unplugged, or a monitor hunting between its inputs, which
//! drops and brings back its link within a second) is kept for `UNPLUG_GRACE` with its windows and panels, and goes only when
//! it has not come back by then (`output_unplugged`, `output_back`).
//!
//! A mode asked for (monitors.lua, wlr-output-management) is matched to the screen's own modes within half a hertz
//! (`closest_mode`): Settings and Hyprland write 60 for a panel's 60.003 Hz.

use std::path::PathBuf;
use std::time::Duration;

use smithay::output::{Mode, Output, Scale};
use smithay::reexports::calloop::RegistrationToken;
use smithay::reexports::wayland_server::backend::GlobalId;
use smithay::utils::{Logical, Point, Rectangle, Transform};

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
    /// None: the turn it has now
    pub transform: Option<Transform>,
}

/// One `hl.monitor({ ... })` of monitors.lua. None: what the screen has by itself (its preferred mode, its default scale, the
/// next place to the right).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    /// empty: every screen no other rule names
    pub output: String,
    pub mode: Option<Mode>,
    /// Hyprland's words for a mode ("highrr", "highres"): chosen from the screen's own modes
    pub pick: Option<Pick>,
    pub at: Option<Point<i32, Logical>>,
    pub scale: Option<f64>,
    pub transform: Option<Transform>,
    pub disabled: bool,
}

/// Hyprland's words for a mode: the fastest (then the largest), or the largest (then the fastest), both of the screen's own
/// shape (`pick_mode`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    HighRr,
    HighRes,
}

/// Two sizes of the same shape (aspect ratio) within 1 %: 1366x768 and 1920x1080 are, and so are 2560x1080 and 3440x1440 (both
/// sold as 21:9); DCI's 4096x2160 and 3840x2160 are not, nor 800x600 and 2560x2880.
fn same_shape(a: Mode, b: Mode) -> bool {
    let (aw, ah, bw, bh) = (i64::from(a.size.w), i64::from(a.size.h), i64::from(b.size.w), i64::from(b.size.h));
    aw > 0 && ah > 0 && bw > 0 && bh > 0 && (aw * bh - bw * ah).abs() * 100 <= aw * bh
}

/// The mode of `modes` that `pick` asks for, among the modes of the shape of the screen's own (`preferred`) mode: Hyprland's
/// `highrr` is the fastest mode of any size and `highres` the largest, which picked a monitor's 800x600 at 60.32 Hz over its
/// own 2560x2880 at 59.98 and would pick the cinema 4096x2160 a monitor accepts, stretched to its own shape.
pub fn pick_mode(modes: &[Mode], preferred: Mode, pick: Pick) -> Option<Mode> {
    let area = |m: &Mode| i64::from(m.size.w) * i64::from(m.size.h);
    let shaped = modes.iter().copied().filter(|m| same_shape(*m, preferred));
    match pick {
        Pick::HighRr => shaped.max_by_key(|m| (m.refresh, area(m))),
        Pick::HighRes => shaped.max_by_key(|m| (area(m), m.refresh)),
    }
}

/// The mode of a screen nothing names: its own (`preferred`) size at the fastest rate it has for it, the preferred mode itself
/// when nothing at that size is faster. The Hyprland session's rule (yggdrasil.lua's `fastest_own`): a monitor's EDID names
/// 60 Hz first even when it runs at 144 or 180.
pub fn own_mode(modes: &[Mode], preferred: Mode) -> Mode {
    modes
        .iter()
        .copied()
        .filter(|m| m.size == preferred.size && m.refresh > preferred.refresh)
        .max_by_key(|m| m.refresh)
        .unwrap_or(preferred)
}

/// A turn by wl_output's number (0 to 7).
pub fn transform_from(n: u32) -> Option<Transform> {
    use Transform::*;
    [Normal, _90, _180, _270, Flipped, Flipped90, Flipped180, Flipped270].get(n as usize).copied()
}

/// A turn's wl_output number.
pub fn transform_number(t: Transform) -> u32 {
    (0..8).find(|&n| transform_from(n) == Some(t)).unwrap_or(0)
}

#[derive(Default)]
pub struct Heads {
    pub list: Vec<Head>,
    /// the rules of monitors.lua as last read, and its stamp then (configs.rs; None inside: no file)
    pub rules: Vec<Rule>,
    rules_stamp: Option<crate::configs::Stamp>,
    /// wlr-output-management's managers (outputmgmt.rs)
    pub mgmt: crate::outputmgmt::Mgmt,
    /// screens whose link went a moment ago, kept until UNPLUG_GRACE has passed: name, and the timer that then removes it
    pub leaving: Vec<(String, RegistrationToken)>,
}

/// How long a screen whose link went may stay away and come back as if it had never gone (its windows, panels and wl_output
/// kept): an HDMI monitor hunting between its inputs bounced three times in 26 s on the laptop (2026-11-19).
pub const UNPLUG_GRACE: Duration = Duration::from_secs(2);

/// The mode of `modes` that is `want`: the same size and the closest refresh within half a hertz (monitors.lua's 60 for a
/// panel's 60.003 Hz, as Hyprland matches it). None when the screen has no such mode.
pub fn closest_mode(modes: &[Mode], want: Mode) -> Option<Mode> {
    modes
        .iter()
        .filter(|m| m.size == want.size && (m.refresh - want.refresh).abs() <= 500)
        .min_by_key(|m| (m.refresh - want.refresh).abs())
        .copied()
}

/// Settings > Displays' file; YGGDRASIL_COMPOSITOR_MONITORS names another (the bake-off's session gives its own).
pub fn monitors_path() -> PathBuf {
    std::env::var_os("YGGDRASIL_COMPOSITOR_MONITORS")
        .map(PathBuf::from)
        .unwrap_or_else(|| yggdrasil_conf::dirs::config().join("monitors.lua"))
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
                "mode" => {
                    r.mode = parse_mode(&v);
                    r.pick = match v.trim() {
                        "highrr" => Some(Pick::HighRr),
                        "highres" => Some(Pick::HighRes),
                        _ => None,
                    };
                }
                "position" => r.at = parse_at(&v),
                "scale" => r.scale = crate::headless::parse_scale(&v),
                "transform" => r.transform = v.trim().parse().ok().and_then(transform_from),
                "disabled" => r.disabled = v == "true",
                _ => {}
            }
        }
        rest = &rest[close + 1..];
        rules.push(r);
    }
    rules
}

/// What the rules say a screen is to be: the last rule naming it, else the last for every screen, else what it has by itself
/// (its own size at its fastest rate there, `own_mode`). `modes` are the screen's own (for that and "highrr" and "highres").
/// A rule without a mode keeps the preferred one, as on Hyprland (whose session works out the fastest rate only for a screen
/// no rule names), so monitors.lua means the same on both.
pub fn want_from_rules(rules: &[Rule], name: &str, preferred: Mode, modes: &[Mode], default_scale: f64) -> Want {
    let rule = rules.iter().rev().find(|r| r.output == name).or_else(|| rules.iter().rev().find(|r| r.output.is_empty()));
    match rule {
        None => Want { enabled: true, mode: own_mode(modes, preferred), scale: default_scale, at: None, transform: None },
        Some(r) => Want {
            enabled: !r.disabled,
            mode: r.mode.or_else(|| r.pick.and_then(|p| pick_mode(modes, preferred, p))).unwrap_or(preferred),
            scale: r.scale.unwrap_or(default_scale),
            at: r.at,
            transform: r.transform,
        },
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
            transform: Some(self.output.current_transform()),
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
    /// A laptop's own screen is listed before the others (Settings > Displays numbers it 1) and, unless placed elsewhere, laid
    /// out first, at the left: also when it comes back after the lid.
    pub fn add_output(&mut self, output: Output, default_scale: f64) {
        let name = output.name();
        let at = match crate::laptop::is_internal(&name) {
            true => self.heads.list.iter().position(|h| !crate::laptop::is_internal(&h.output.name())).unwrap_or(self.heads.list.len()),
            false => self.heads.list.len(),
        };
        self.heads.list.insert(at, Head { output, enabled: false, global: None, default_scale, fixed_at: None });
        let preferred = self.heads.list[at].preferred();
        let modes = self.heads.list[at].output.modes();
        let want = want_from_rules(&self.heads.rules, &name, preferred, &modes, default_scale);
        // the rules may not switch the only screen off
        let want = if self.heads.list.iter().any(|h| h.enabled) { want } else { Want { enabled: true, ..want } };
        if let Err(e) = self.configure_outputs(vec![(name.clone(), want.clone())], false) {
            eprintln!("yggdrasil-compositor: {name}: {e}; it keeps its own mode and scale");
            let plain = Want { enabled: true, mode: preferred, scale: 1.0, at: None, transform: None };
            let _ = self.configure_outputs(vec![(name, plain)], false);
        }
    }

    /// A screen goes (unplugged): as switched off, then forgotten. The last one stays.
    pub fn remove_output(&mut self, name: &str) -> Result<(), String> {
        let i = self.heads.list.iter().position(|h| h.output.name() == name).ok_or_else(|| format!("no output {name}"))?;
        self.output_back(name);
        let mut off = self.heads.list[i].now();
        off.enabled = false;
        self.configure_outputs(vec![(name.to_string(), off)], false)?;
        self.heads.list.remove(i);
        self.output_list_changed();
        Ok(())
    }

    /// A screen's link went (its connector disconnected): it stays, with its windows and panels, for UNPLUG_GRACE, and is
    /// removed then unless it came back (`output_back`). Nothing changes for programs meanwhile.
    pub fn output_unplugged(&mut self, name: &str) {
        use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
        if self.head(name).is_none() || self.heads.leaving.iter().any(|(n, _)| n == name) {
            return;
        }
        let n = name.to_string();
        let timer = self.loop_handle.insert_source(Timer::from_duration(UNPLUG_GRACE), move |_, _, app| {
            app.heads.leaving.retain(|(x, _)| *x != n);
            eprintln!("yggdrasil-compositor: {n} did not come back: removed");
            if let Err(e) = app.remove_output(&n) {
                eprintln!("yggdrasil-compositor: {n}: {e}");
            }
            TimeoutAction::Drop
        });
        match timer {
            Ok(t) => self.heads.leaving.push((name.to_string(), t)),
            Err(_) => {
                let _ = self.remove_output(name);
            }
        }
    }

    /// A screen whose link went is back within UNPLUG_GRACE: it is kept as it was (true). False when it was not leaving.
    pub fn output_back(&mut self, name: &str) -> bool {
        let Some(i) = self.heads.leaving.iter().position(|(n, _)| n == name) else { return false };
        let (_, t) = self.heads.leaving.remove(i);
        self.loop_handle.remove(t);
        self.wake();
        true
    }

    /// The screens whose link went and that are still kept (the hardware backend gives one back its CRTC when it returns).
    pub fn leaving_outputs(&self) -> Vec<Output> {
        self.heads.leaving.iter().filter_map(|(n, _)| self.head(n)).map(|h| h.output.clone()).collect()
    }

    /// The hardware backend could not set the mode asked for a screen (its GPU refused it, or the screen has no such mode): the
    /// screen is put back at `kept`,
    /// the mode its CRTC still has, as a change like any other (wl_output, Settings through wlr-output-management, X11 programs,
    /// the layout and the windows on it), so nothing goes on showing a mode it does not run.
    pub fn mode_kept(&mut self, name: &str, kept: Mode) {
        let Some(h) = self.head(name) else { return };
        let w = Want { mode: kept, ..h.now() };
        if let Err(e) = self.configure_outputs(vec![(name.to_string(), w)], false) {
            eprintln!("yggdrasil-compositor: {name}: not put back at the mode it has: {e}");
        }
    }

    /// Whether every screen that is on and stays on can have the mode `changes` asks for it (matched as `configure_outputs`
    /// matches it), as its backend says (`mode_check`: the hardware backend's test commit). A program applying or testing a
    /// configuration through wlr-output-management is answered 'failed' when one cannot, and nothing changes; before, it was
    /// told 'succeeded' and the screen put back a frame later (`mode_kept`).
    pub fn modes_taken(&mut self, changes: &[(String, Want)]) -> Result<(), String> {
        for (name, w) in changes {
            let Some(h) = self.heads.list.iter().find(|h| h.output.name() == *name) else { continue };
            if !w.enabled || !h.enabled {
                continue;
            }
            let mode = closest_mode(&h.output.modes(), w.mode).unwrap_or(w.mode);
            if h.output.current_mode() == Some(mode) {
                continue;
            }
            if let Some(check) = self.mode_check.as_mut() {
                check(name, mode)
                    .map_err(|e| format!("{name}: {}x{} at {} Hz: {e}", mode.size.w, mode.size.h, mode.refresh as f64 / 1000.0))?;
            }
        }
        Ok(())
    }

    /// Set screens to what `changes` says (the others stay as they are). With `test_only` nothing changes: only whether it could.
    /// A mode is matched to the screen's own within half a hertz (`closest_mode`).
    pub fn configure_outputs(&mut self, changes: Vec<(String, Want)>, test_only: bool) -> Result<(), String> {
        let mut changes = changes;
        for (name, w) in &mut changes {
            let Some(h) = self.heads.list.iter().find(|h| h.output.name() == *name) else {
                return Err(format!("no output {name}"));
            };
            if let Some(m) = closest_mode(&h.output.modes(), w.mode) {
                w.mode = m;
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
            if let Some(t) = w.transform.filter(|t| *t != h.output.current_transform()) {
                h.output.change_current_state(None, Some(t), None, None);
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
        self.keep_pointer_on_screens(&before);
        self.output_resized();
        self.keep_clear();
        self.output_list_changed();
        // X11 programs follow the screens' scale (xwayland.rs)
        self.follow_scale();
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
        self.choose_main();
    }

    /// The main screen (windows maximize and the switcher shows there; X11 programs are told it is the primary one), as the
    /// panel's dock and Settings have it (yggdrasil_conf::display::pick): the monitor chosen in Settings > Displays when it is
    /// on, else a laptop's own screen (so it is main again when the lid opens), else the one at the top left.
    pub fn choose_main(&mut self) {
        let on: Vec<Output> = self.heads.list.iter().filter(|h| h.enabled).map(|h| h.output.clone()).collect();
        let props: Vec<(String, smithay::output::PhysicalProperties, Point<i32, Logical>)> =
            on.iter().map(|o| (o.name(), o.physical_properties(), o.current_location())).collect();
        let seen: Vec<yggdrasil_conf::display::Seen> = props
            .iter()
            .map(|(name, p, at)| yggdrasil_conf::display::Seen { connector: name, make: &p.make, model: &p.model, x: at.x, y: at.y })
            .collect();
        let main = yggdrasil_conf::display::pick(self.main_choice.as_ref(), &seen).map(|i| on[i].clone());
        let Some(main) = main.filter(|m| *m != self.output) else { return };
        self.output = main;
        if let Some(wm) = self.xwm.as_mut() {
            let _ = wm.set_randr_primary_output(Some(&self.output));
        }
    }

    /// Read monitors.lua again when it changed (or appeared, or went), and set the screens to it: at start, on `reload` and when
    /// the config files' watch saw it change (configs.rs).
    pub fn reload_monitors(&mut self) {
        if self.heads.list.is_empty() {
            return;
        }
        let path = monitors_path();
        let t = crate::configs::stamp(&path);
        if self.heads.rules_stamp == Some(t) {
            return;
        }
        self.heads.rules_stamp = Some(t);
        self.heads.rules = std::fs::read_to_string(&path).map(|s| parse_rules(&s)).unwrap_or_default();
        let changes: Vec<(String, Want)> = self
            .heads
            .list
            .iter()
            .map(|h| {
                (h.output.name(), want_from_rules(&self.heads.rules, &h.output.name(), h.preferred(), &h.output.modes(), h.default_scale))
            })
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

    /// Turns a screen (`transform NAME N`): its picture, the layout (a turned screen is as wide as it was high), its panels and
    /// windows, and where a touch or the pen lands on it (touch.rs).
    pub fn set_transform(&mut self, name: &str, t: Transform) -> Result<(), String> {
        let mut w = self.head(name).ok_or_else(|| format!("no output {name}"))?.now();
        w.transform = Some(t);
        self.configure_outputs(vec![(name.to_string(), w)], false)
    }

    /// Each screen's name and turn (`transform`).
    pub fn transforms(&self) -> Vec<String> {
        self.heads.list.iter().map(|h| format!("{}\t{}", h.output.name(), transform_number(h.output.current_transform()))).collect()
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
                pick: None,
                at: Some((0, 0).into()),
                scale: Some(1.0),
                transform: None,
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
        // a turn (Settings' and Hyprland's `transform`, wl_output's numbers); anything else is no turn
        let r = parse_rules(
            "hl.monitor({ output = \"eDP-1\", mode = \"1920x1200@60\", position = \"0x0\", scale = 1.25, transform = 1 })\n\
            hl.monitor({ output = \"DP-1\", transform = 7 })\nhl.monitor({ output = \"DP-2\", transform = 9 })\n",
        );
        assert_eq!(r.iter().map(|r| r.transform).collect::<Vec<_>>(), [Some(Transform::_90), Some(Transform::Flipped270), None]);
        assert_eq!(
            (0..9).map(|n| transform_from(n).map(transform_number)).collect::<Vec<_>>()[..8],
            (0..8).map(Some).collect::<Vec<_>>()[..]
        );
    }

    #[test]
    fn a_mode_asked_for_is_matched_within_half_a_hertz() {
        // the laptop's panel offers 60.003 Hz; Settings (as Hyprland lists it) writes 60
        let modes = [mode(1920, 1200, 60_003), mode(1920, 1200, 48_002), mode(1280, 800, 59_910), mode(1280, 800, 60_000)];
        let want = parse_mode("1920x1200@60").unwrap();
        assert_eq!(closest_mode(&modes, want), Some(mode(1920, 1200, 60_003)));
        assert_eq!(closest_mode(&modes, parse_mode("1920x1200@60.00").unwrap()), Some(mode(1920, 1200, 60_003)));
        assert_eq!(closest_mode(&modes, mode(1280, 800, 59_950)), Some(mode(1280, 800, 59_910)), "the closer of two");
        assert_eq!(closest_mode(&modes, mode(1280, 800, 60_000)), Some(mode(1280, 800, 60_000)), "an exact one first");
        assert_eq!(closest_mode(&modes, mode(1920, 1200, 59_400)), None, "more than half a hertz off");
        assert_eq!(closest_mode(&modes, mode(1920, 1080, 60_000)), None, "another size");
    }

    #[test]
    fn the_last_rule_naming_a_screen_wins_and_others_keep_their_own() {
        let rules =
            parse_rules("hl.monitor({ output = \"A\", scale = 2 })\nhl.monitor({ output = \"A\", scale = 1.5, position = \"100x0\" })\n");
        let pref = mode(1600, 900, 60_000);
        assert_eq!(
            want_from_rules(&rules, "A", pref, &[], 1.0),
            Want { enabled: true, mode: pref, scale: 1.5, at: Some((100, 0).into()), transform: None }
        );
        assert_eq!(
            want_from_rules(&rules, "B", pref, &[], 1.25),
            Want { enabled: true, mode: pref, scale: 1.25, at: None, transform: None }
        );
        let all = parse_rules("hl.monitor({ output = \"\", disabled = true })\nhl.monitor({ output = \"B\", scale = 2 })\n");
        assert!(!want_from_rules(&all, "A", pref, &[], 1.0).enabled, "the rule for every screen");
        assert!(want_from_rules(&all, "B", pref, &[], 1.0).enabled, "a rule of its own comes first");
        assert_eq!(parse_mode("2560x1440@143.97Hz"), Some(mode(2560, 1440, 143_970)));
        assert_eq!((parse_mode("preferred"), parse_mode("0x10"), parse_mode("10x10@-1")), (None, None, None));
    }

    /// "WxH@Hz" into modes, the first the monitor's own (preferred), as `hyprctl monitors all` lists them
    fn modes(list: &[&str]) -> Vec<Mode> {
        list.iter().map(|s| parse_mode(s).unwrap()).collect()
    }

    /// An ultrawide's modes (an X34, as Hyprland listed them on 2026-11-13; packages/session/tests/monitor-modes.lua): 60 Hz
    /// first, the fast modes at its own size later; and two added for the test, the cinema (DCI) 4096x2160 a TV-minded EDID
    /// lists and a 2560x1080 of the same shape within 1 %.
    fn ultrawide() -> Vec<Mode> {
        modes(&[
            "3440x1440@60.00",
            "3840x2160@59.94",
            "3840x2160@29.97",
            "2560x1440@59.95",
            "1920x1080@60.00",
            "3440x1440@180.00",
            "3440x1440@143.85",
            "3440x1440@100.00",
            "3440x1440@85.00",
            "3440x1440@75.00",
            "4096x2160@60.00",
            "2560x1080@119.88",
        ])
    }

    /// A 2560x2880 monitor's (an LG DualUp, the same day): its own mode at 59.98 Hz, and an 800x600 at 60.32 Hz.
    fn tall() -> Vec<Mode> {
        modes(&[
            "2560x2880@59.98",
            "1920x1080@60.00",
            "1920x1080@59.94",
            "1920x1080@50.00",
            "1920x1080@29.97",
            "1280x720@59.94",
            "1280x720@50.00",
            "1024x768@60.00",
            "800x600@60.32",
            "720x576@50.00",
            "720x480@59.94",
        ])
    }

    #[test]
    fn a_screen_nothing_names_runs_at_its_own_size_at_its_fastest_rate() {
        // an empty monitors.lua (or none): the Hyprland session's rule, not the 60 Hz the EDID names first
        let (uw, t) = (ultrawide(), tall());
        assert_eq!(want_from_rules(&[], "DP-1", uw[0], &uw, 1.0).mode, mode(3440, 1440, 180_000));
        assert_eq!(want_from_rules(&[], "DP-3", t[0], &t, 1.0).mode, mode(2560, 2880, 59_980), "not 800x600@60.32");
        // a rule for another screen leaves this one to its own too
        let other = parse_rules("hl.monitor({ output = \"HDMI-A-1\", mode = \"1920x1080@60\" })\n");
        assert_eq!(want_from_rules(&other, "DP-1", uw[0], &uw, 1.0).mode, mode(3440, 1440, 180_000));
        // a laptop's panel whose own mode is already its fastest, and one listing a slower rate at its size: its own
        let panel = modes(&["1920x1200@60.003", "1920x1200@48.002", "1280x800@60.00"]);
        assert_eq!(want_from_rules(&[], "eDP-1", panel[0], &panel, 1.25).mode, panel[0]);
        // headless (one mode, or none known): the command line's
        assert_eq!(want_from_rules(&[], "HEADLESS-1", mode(1600, 900, 60_000), &[], 1.0).mode, mode(1600, 900, 60_000));
        // a rule naming the screen without a mode keeps the preferred one, as Hyprland does with it
        let named = parse_rules("hl.monitor({ output = \"DP-1\", scale = 1.25 })\n");
        assert_eq!(want_from_rules(&named, "DP-1", uw[0], &uw, 1.0).mode, uw[0]);
        let words = parse_rules("hl.monitor({ output = \"DP-1\", mode = \"preferred\" })\n");
        assert_eq!(want_from_rules(&words, "DP-1", uw[0], &uw, 1.0).mode, uw[0]);
    }

    #[test]
    fn highrr_and_highres_stay_at_the_screens_own_shape() {
        let (uw, t) = (ultrawide(), tall());
        let rules = parse_rules("hl.monitor({ output = \"\", mode = \"highrr\", position = \"auto\", scale = 1 })\n");
        assert_eq!(rules[0].pick, Some(Pick::HighRr));
        assert_eq!(want_from_rules(&rules, "DP-1", uw[0], &uw, 1.0).mode, mode(3440, 1440, 180_000));
        assert_eq!(want_from_rules(&rules, "DP-3", t[0], &t, 1.0).mode, mode(2560, 2880, 59_980), "not 800x600@60.32");
        let res = parse_rules("hl.monitor({ output = \"\", mode = \"highres\" })\n");
        assert_eq!(res[0].pick, Some(Pick::HighRes));
        // the largest of its own shape: not the 3840x2160 it scales down, nor DCI's 4096x2160; the fastest of that size
        assert_eq!(want_from_rules(&res, "DP-1", uw[0], &uw, 1.0).mode, mode(3440, 1440, 180_000));
        assert_eq!(want_from_rules(&res, "DP-3", t[0], &t, 1.0).mode, mode(2560, 2880, 59_980));
        // a 4K TV's own shape takes in 3840x2160 and 1920x1080, never the cinema 4096x2160
        let tv = modes(&["3840x2160@60.00", "4096x2160@60.00", "1920x1080@120.00", "3840x2160@30.00"]);
        assert_eq!(pick_mode(&tv, tv[0], Pick::HighRes), Some(mode(3840, 2160, 60_000)));
        assert_eq!(pick_mode(&tv, tv[0], Pick::HighRr), Some(mode(1920, 1080, 120_000)), "the fastest of the same shape");
        // no modes known (headless): the screen's own
        assert_eq!(want_from_rules(&rules, "DP-1", uw[0], &[], 1.0).mode, uw[0]);
        // the shapes, within 1 %
        let s = |a: &str, b: &str| same_shape(parse_mode(a).unwrap(), parse_mode(b).unwrap());
        assert!(s("1920x1080", "1366x768") && s("3440x1440", "2560x1080") && s("2560x2880", "2560x2880"));
        assert!(!s("3840x2160", "4096x2160") && !s("2560x2880", "800x600") && !s("3440x1440", "5120x1440"));
        assert!(!s("1920x1080", "1920x1200"), "16:9 is not 16:10");
    }
}
