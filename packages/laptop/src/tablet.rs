//! Tablet mode and screen rotation on a 2-in-1 (`/usr/lib/yggdrasil/tablet watch`, started with the session).
//!
//! - Tablet mode comes from the laptop's switch (SW_TABLET_MODE). Hyprland gets the switch's events and the session's binds
//!   (yggdrasil.lua) run `tablet mode on` / `tablet mode off`, which tell the watcher over its socket; at start a ThinkPad's
//!   thinkpad_acpi says where it is. Folded into a tablet, the built-in screen follows the accelerometer; opened as a laptop it
//!   goes back upright. A screen with an accelerometer and no switch (a tablet) always follows it.
//! - Rotation: the accelerometer through iio-sensor-proxy, claimed only while it is followed (so the sensor sleeps otherwise);
//!   the turn is given to Hyprland for the built-in screen (eDP/DSI) with its mode, place and scale as they are, and the same
//!   turn to its touchscreen and its built-in pen, so a touch lands under the finger. Hyprland rereads its config on a reload
//!   (the turn would be lost), so the watcher turns it again after `configreloaded` and `monitoradded`.
//! - The lock (Settings > Displays, `tablet lock on`): the screen stays as it is now. Kept in ~/.config/yggdrasil/tablet.conf.
//! - The on-screen keyboard (wvkbd, an online component in Agora: "On-screen keyboard"): shown in tablet mode while no keyboard
//!   is plugged in by USB or Bluetooth, closed otherwise. `keyboard = off` in tablet.conf never shows it.
//!
//! Yggdrasil's own compositor cannot turn a screen yet (its wlr-output-management refuses a transform), so there the watcher
//! does the keyboard only.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::sync::mpsc::Sender;
use std::time::Duration;

use yggdrasil_wm::hyprland::{plain_word, Hyprland};
use yggdrasil_wm::Screen;

/// The on-screen keyboard: wvkbd's international layout (from the AUR, through Agora's "On-screen keyboard").
pub const OSK: &str = "/usr/bin/wvkbd-mobintl";

/// Which way up the screen is held, as iio-sensor-proxy names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Normal,
    LeftUp,
    BottomUp,
    RightUp,
}

impl Orientation {
    /// iio-sensor-proxy's AccelerometerOrientation; "undefined" (lying flat) and anything else is None: the screen stays.
    pub fn parse(s: &str) -> Option<Orientation> {
        match s {
            "normal" => Some(Orientation::Normal),
            "left-up" => Some(Orientation::LeftUp),
            "bottom-up" => Some(Orientation::BottomUp),
            "right-up" => Some(Orientation::RightUp),
            _ => None,
        }
    }

    /// The screen's transform (wl_output's numbering, which Hyprland uses): the picture turned 90° counter-clockwise for each
    /// step, as GNOME does for the same readings.
    pub fn transform(self) -> u32 {
        match self {
            Orientation::Normal => 0,
            Orientation::LeftUp => 1,
            Orientation::BottomUp => 2,
            Orientation::RightUp => 3,
        }
    }
}

/// ~/.config/yggdrasil/tablet.conf.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Conf {
    /// The screen stays as it is (Settings > Displays > Lock rotation).
    pub rotation_lock: bool,
    /// The on-screen keyboard shows in tablet mode.
    pub keyboard: bool,
}

impl Default for Conf {
    fn default() -> Conf {
        Conf { rotation_lock: false, keyboard: true }
    }
}

impl Conf {
    /// `rotation-lock = on`, `keyboard = off`; anything else is left at its default.
    pub fn parse(text: &str) -> Conf {
        let mut c = Conf::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else { continue };
            let on = match v.trim() {
                "on" | "true" | "yes" | "1" => true,
                "off" | "false" | "no" | "0" => false,
                _ => continue,
            };
            match k.trim() {
                "rotation-lock" => c.rotation_lock = on,
                "keyboard" => c.keyboard = on,
                _ => {}
            }
        }
        c
    }

    pub fn text(&self) -> String {
        let w = |b: bool| if b { "on" } else { "off" };
        format!(
            "# Written by Settings > Displays (tablet mode and rotation on a 2-in-1).\nrotation-lock = {}\nkeyboard = {}\n",
            w(self.rotation_lock),
            w(self.keyboard)
        )
    }

    pub fn path() -> PathBuf {
        yggdrasil_conf::dirs::config().join("tablet.conf")
    }

    pub fn load(path: &Path) -> Conf {
        std::fs::read_to_string(path).map(|t| Conf::parse(&t)).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, self.text()).map_err(|e| e.to_string())
    }
}

/// What the watcher knows, and what it wants from it.
#[derive(Clone, Debug, PartialEq)]
pub struct State {
    /// The laptop has a tablet-mode switch (without one, a screen with an accelerometer is a tablet).
    pub has_switch: bool,
    /// Folded into a tablet, as the switch last said.
    pub tablet: bool,
    /// The accelerometer's last reading (None: lying flat, or not read yet).
    pub orientation: Option<Orientation>,
    pub conf: Conf,
    /// A keyboard plugged in by USB or Bluetooth.
    pub plugged_keyboard: bool,
    /// The turn the screen should have now (kept while locked).
    pub rotation: u32,
}

impl State {
    /// Used as a tablet: folded, or a tablet without a switch.
    pub fn as_tablet(&self) -> bool {
        !self.has_switch || self.tablet
    }

    /// The accelerometer is read (and so claimed from iio-sensor-proxy).
    pub fn follows(&self) -> bool {
        self.as_tablet() && !self.conf.rotation_lock
    }

    /// The turn wanted: upright as a laptop; as a tablet the accelerometer's, unless locked (then it stays) or lying flat.
    pub fn wanted(&self) -> u32 {
        if !self.as_tablet() {
            0
        } else if self.conf.rotation_lock {
            self.rotation
        } else {
            self.orientation.map(Orientation::transform).unwrap_or(self.rotation)
        }
    }

    /// The on-screen keyboard is shown: folded into a tablet (a switch said so), with no keyboard plugged in, not turned off.
    pub fn keyboard(&self) -> bool {
        self.has_switch && self.tablet && !self.plugged_keyboard && self.conf.keyboard
    }
}

/// The built-in screen: a laptop's panel (eDP) or a tablet's (DSI), switched on.
pub fn panel(screens: &[Screen]) -> Option<&Screen> {
    screens.iter().find(|s| !s.disabled && (s.name.starts_with("eDP") || s.name.starts_with("DSI")))
}

/// The touchscreens and the pens built into a screen, from Hyprland's `devices` answer: the same ones yggdrasil.lua maps to the
/// built-in screen (Wacom's HID digitisers, ELAN, Goodix, stylus panels; never a drawing tablet on a cable).
pub fn panel_inputs(devices_json: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(devices_json).unwrap_or_default();
    let names = |k: &str| -> Vec<String> {
        v[k].as_array().map(|a| a.iter().filter_map(|d| d["name"].as_str().map(str::to_string)).collect()).unwrap_or_default()
    };
    let builtin_pen = |n: &String| n.starts_with("wacom-hid-") || ["elan", "goodix", "stylus"].iter().any(|w| n.contains(w));
    let mut out: Vec<String> = names("touch");
    out.extend(names("tablets").into_iter().filter(builtin_pen));
    out.retain(|n| plain_word(n));
    out.dedup();
    out
}

/// A number as Lua reads it ("1.5", "2").
fn num(x: f64) -> String {
    let s = format!("{x:.4}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The Lua that turns the built-in screen (mode, place and scale as they are) and its touchscreen and pen with it. None for a
/// screen whose name could not go into Lua as it is.
pub fn rotate_lua(s: &Screen, transform: u32, inputs: &[String]) -> Option<String> {
    if !plain_word(&s.name) || transform > 7 {
        return None;
    }
    let mut lua = format!(
        "hl.monitor({{ output = \"{}\", mode = \"{}x{}@{}\", position = \"{}x{}\", scale = {}, transform = {transform} }})",
        s.name,
        s.width,
        s.height,
        num(s.refresh.unwrap_or(60.0)),
        s.x,
        s.y,
        num(s.scale.unwrap_or(1.0)),
    );
    for i in inputs.iter().filter(|i| plain_word(i)) {
        lua.push_str(&format!("; hl.device({{ name = \"{i}\", transform = {transform} }})"));
    }
    Some(lua)
}

/// Turns the built-in screen in Hyprland. Ok when Hyprland took it.
pub fn rotate(h: &Hyprland, transform: u32) -> Result<(), String> {
    let screens = h.screens_all();
    let s = panel(&screens).ok_or("no built-in screen")?;
    let inputs = h.query("devices").map(|j| panel_inputs(&j)).unwrap_or_default();
    let lua = rotate_lua(s, transform, &inputs).ok_or("the screen's name is not plain")?;
    match h.eval(&lua).map(|a| a.trim().to_string()) {
        Some(a) if a.is_empty() || a == "ok" => Ok(()),
        Some(a) => Err(a),
        None => Err("Hyprland did not answer".into()),
    }
}

/// The built-in screen's turn in Hyprland now.
pub fn current(h: &Hyprland) -> Option<u32> {
    panel(&h.screens_all()).map(|s| s.transform)
}

/// What the watcher hears.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The switch: folded into a tablet (true) or opened (false).
    Mode(bool),
    /// The accelerometer's new reading.
    Orientation(Option<Orientation>),
    /// tablet.conf changed (Settings).
    Conf,
    /// A device came or went in /dev/input (a keyboard plugged in?).
    Inputs,
    /// Hyprland read its config again or a screen came: turn the screen again.
    Reapply,
}

/// A line on the watcher's socket.
pub fn parse_command(line: &str) -> Option<Event> {
    match line.trim() {
        "mode on" => Some(Event::Mode(true)),
        "mode off" => Some(Event::Mode(false)),
        "conf" => Some(Event::Conf),
        _ => None,
    }
}

/// A line of Hyprland's event socket that undoes or needs the turn.
pub fn needs_reapply(line: &str) -> bool {
    line.starts_with("configreloaded>>") || line.starts_with("monitoradded>>") || line.starts_with("monitoraddedv2>>")
}

/// The watcher's socket in the runtime folder.
pub fn socket_path(runtime: &Path) -> PathBuf {
    runtime.join("yggdrasil-tablet.sock")
}

/// Tells a running watcher (`tablet mode on`, `tablet lock on`). Ok(false) when none runs.
pub fn tell(runtime: &Path, line: &str) -> Result<bool, String> {
    let p = socket_path(runtime);
    if !yggdrasil_wm::sock::ours(&p) {
        return Ok(false);
    }
    match yggdrasil_wm::sock::request(&p, format!("{line}\n").as_bytes(), Duration::from_secs(2)) {
        Some(a) if a.trim() == "ok" => Ok(true),
        Some(a) => Err(a.trim().to_string()),
        None => Ok(false),
    }
}

/// Listens on the socket; each line it understands is passed on.
pub fn listen(path: &Path, tx: Sender<Event>) -> std::io::Result<()> {
    let _ = std::fs::remove_file(path);
    let l = UnixListener::bind(path)?;
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            let _ = c.set_read_timeout(Some(Duration::from_secs(2)));
            let mut line = String::new();
            let _ = BufReader::new(std::io::Read::take(&c, 256)).read_line(&mut line);
            let answer = match parse_command(&line) {
                Some(e) => {
                    if tx.send(e).is_err() {
                        return;
                    }
                    "ok\n"
                }
                None => "unknown command\n",
            };
            let _ = c.write_all(answer.as_bytes());
        }
    });
    Ok(())
}

/// Hyprland's events: a turn is needed again after `configreloaded` and `monitoradded`.
pub fn watch_hyprland(h: &Hyprland, tx: Sender<Event>) {
    let path = h.events_socket();
    std::thread::spawn(move || {
        let Ok(s) = UnixStream::connect(&path) else { return };
        for line in BufReader::new(s).lines() {
            let Ok(line) = line else { return };
            if needs_reapply(&line) && tx.send(Event::Reapply).is_err() {
                return;
            }
        }
    });
}

/// Devices coming and going in /dev/input (inotify: no polling).
pub fn watch_inputs(dir: &Path, tx: Sender<Event>) {
    let Ok(mut ino) = inotify::Inotify::init() else { return };
    if ino.watches().add(dir, inotify::WatchMask::CREATE | inotify::WatchMask::DELETE).is_err() {
        return;
    }
    std::thread::spawn(move || {
        let mut buf = [0u8; 4096];
        while let Ok(events) = ino.read_events_blocking(&mut buf) {
            if events.into_iter().any(|e| e.name.is_some_and(|n| n.to_string_lossy().starts_with("event")))
                && tx.send(Event::Inputs).is_err()
            {
                return;
            }
        }
    });
}

/// iio-sensor-proxy's accelerometer on the system bus (or a stand-in on another connection, in tests).
pub struct Accel {
    proxy: zbus::blocking::Proxy<'static>,
    claimed: bool,
}

impl Accel {
    pub fn new(conn: &zbus::blocking::Connection) -> zbus::Result<Accel> {
        let proxy = zbus::blocking::Proxy::new(conn, "net.hadess.SensorProxy", "/net/hadess/SensorProxy", "net.hadess.SensorProxy")?;
        Ok(Accel { proxy, claimed: false })
    }

    pub fn present(&self) -> bool {
        self.proxy.get_property::<bool>("HasAccelerometer").unwrap_or(false)
    }

    /// Readings start (true) or stop; iio-sensor-proxy stops reading the sensor when nobody holds a claim.
    pub fn claim(&mut self, on: bool) -> Result<(), String> {
        if on != self.claimed {
            self.proxy.call_method(if on { "ClaimAccelerometer" } else { "ReleaseAccelerometer" }, &()).map_err(|e| e.to_string())?;
            self.claimed = on;
        }
        Ok(())
    }

    pub fn orientation(&self) -> Option<Orientation> {
        self.proxy.get_property::<String>("AccelerometerOrientation").ok().and_then(|o| Orientation::parse(&o))
    }

    /// Each new reading, passed on.
    pub fn watch(&self, tx: Sender<Event>) {
        let p = self.proxy.clone();
        std::thread::spawn(move || {
            for change in p.receive_property_changed::<String>("AccelerometerOrientation") {
                if let Ok(o) = change.get() {
                    if tx.send(Event::Orientation(Orientation::parse(&o))).is_err() {
                        return;
                    }
                }
            }
        });
    }
}

/// The on-screen keyboard's process: started to show it, ended to hide it (it uses nothing while hidden).
pub struct Osk {
    pub command: Vec<String>,
    child: Option<Child>,
}

impl Osk {
    pub fn new(command: Vec<String>) -> Osk {
        Osk { command, child: None }
    }

    /// It is shown now (the process runs).
    pub fn shown(&mut self) -> bool {
        match &mut self.child {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    pub fn set(&mut self, show: bool) -> Result<(), String> {
        if show == self.shown() {
            return Ok(());
        }
        if show {
            let (prog, args) = self.command.split_first().ok_or("no keyboard program")?;
            if !Path::new(prog).exists() {
                return Err(format!("{prog} is not installed (Agora: On-screen keyboard)"));
            }
            self.child = Some(Command::new(prog).args(args).spawn().map_err(|e| e.to_string())?);
        } else if let Some(mut c) = self.child.take() {
            unsafe { libc::kill(c.id() as libc::pid_t, libc::SIGTERM) };
            let _ = c.wait();
        }
        Ok(())
    }
}

impl Drop for Osk {
    fn drop(&mut self) {
        let _ = self.set(false);
    }
}

/// The watcher: the state, and the compositor, accelerometer and keyboard it acts through.
pub struct Watcher {
    pub state: State,
    pub hypr: Option<Hyprland>,
    pub accel: Option<Accel>,
    pub osk: Osk,
    /// /sys's root (the keyboards are looked for there again after each /dev/input change).
    pub root: PathBuf,
    pub conf_path: PathBuf,
    /// The turn Hyprland has (None: unknown, turn it again).
    pub applied: Option<u32>,
    /// What went wrong last, said once.
    last_error: Option<String>,
}

impl Watcher {
    pub fn new(state: State, hypr: Option<Hyprland>, accel: Option<Accel>, osk: Osk, root: PathBuf, conf_path: PathBuf) -> Watcher {
        let applied = hypr.as_ref().and_then(current);
        let mut state = state;
        if let Some(a) = applied {
            state.rotation = a;
        }
        Watcher { state, hypr, accel, osk, root, conf_path, applied, last_error: None }
    }

    fn note(&mut self, r: Result<(), String>) {
        match r {
            Ok(()) => self.last_error = None,
            Err(e) => {
                if self.last_error.as_ref() != Some(&e) {
                    eprintln!("tablet: {e}");
                }
                self.last_error = Some(e);
            }
        }
    }

    pub fn handle(&mut self, e: Event) {
        match e {
            Event::Mode(on) => self.state.tablet = on,
            Event::Orientation(o) => self.state.orientation = o,
            Event::Conf => self.state.conf = Conf::load(&self.conf_path),
            Event::Inputs => self.state.plugged_keyboard = crate::sysfs::plugged_keyboard(&self.root),
            Event::Reapply => self.applied = None,
        }
    }

    /// Brings the screen, the accelerometer's claim and the keyboard in line with the state.
    pub fn step(&mut self) {
        let follows = self.state.follows();
        if let Some(a) = &mut self.accel {
            let r = a.claim(follows);
            if r.is_ok() && follows {
                // the reading at the moment of the claim (the changes follow as signals)
                if let Some(o) = a.orientation() {
                    self.state.orientation = Some(o);
                }
            }
            self.note(r);
        }
        let want = self.state.wanted();
        self.state.rotation = want;
        if self.applied != Some(want) {
            if let Some(h) = &self.hypr {
                let r = rotate(h, want);
                if r.is_ok() {
                    self.applied = Some(want);
                }
                self.note(r);
            }
        }
        let show = self.state.keyboard();
        let r = self.osk.set(show);
        self.note(r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(name: &str, transform: u32) -> Screen {
        Screen {
            id: Some(0),
            name: name.into(),
            make: String::new(),
            model: String::new(),
            x: 0,
            y: 0,
            width: 2880,
            height: 1800,
            scale: Some(1.75),
            transform,
            reserved: [0; 4],
            focused: true,
            disabled: false,
            active_workspace: Some(1),
            special_workspace: None,
            special_workspace_id: None,
            refresh: Some(60.001),
            modes: vec![],
            mirror_of: None,
        }
    }

    fn yoga() -> State {
        State { has_switch: true, tablet: false, orientation: None, conf: Conf::default(), plugged_keyboard: false, rotation: 0 }
    }

    #[test]
    fn readings_turn_the_screen_as_gnome_does() {
        assert_eq!(
            ["normal", "left-up", "bottom-up", "right-up"].map(|s| Orientation::parse(s).map(Orientation::transform)),
            [Some(0), Some(1), Some(2), Some(3)]
        );
        assert_eq!(Orientation::parse("undefined"), None);
        assert_eq!(Orientation::parse(""), None);
    }

    #[test]
    fn a_laptop_stays_upright_and_a_tablet_follows_unless_locked() {
        let mut s = yoga();
        s.orientation = Some(Orientation::LeftUp);
        assert!(!s.follows() && s.wanted() == 0 && !s.keyboard(), "opened as a laptop: upright, no keyboard, sensor asleep");
        s.tablet = true;
        assert!(s.follows() && s.wanted() == 1 && s.keyboard(), "folded: follows, and the keyboard shows");
        s.rotation = 1;
        s.orientation = None;
        assert_eq!(s.wanted(), 1, "lying flat: stays as it was");
        s.conf.rotation_lock = true;
        s.orientation = Some(Orientation::BottomUp);
        assert!(!s.follows() && s.wanted() == 1, "locked: stays, and the sensor is let go");
        s.plugged_keyboard = true;
        assert!(!s.keyboard(), "a keyboard is plugged in: none on the screen");
        s.plugged_keyboard = false;
        s.conf.keyboard = false;
        assert!(!s.keyboard(), "turned off in tablet.conf");
        s.tablet = false;
        assert_eq!(s.wanted(), 0, "opened again: upright even while locked");
        let tablet = State { has_switch: false, orientation: Some(Orientation::RightUp), ..yoga() };
        assert!(tablet.follows() && tablet.wanted() == 3 && !tablet.keyboard(), "no switch: a tablet; no switch to say a keyboard is gone");
    }

    #[test]
    fn the_settings_file_is_read_and_written() {
        assert_eq!(Conf::parse(""), Conf::default());
        let c = Conf::parse("# x\nrotation-lock = on\nkeyboard=off\nrotation-lock: maybe\nother = 1\n");
        assert_eq!(c, Conf { rotation_lock: true, keyboard: false });
        assert_eq!(Conf::parse(&c.text()), c);
        let d = std::env::temp_dir().join(format!("ygg-tablet-conf-{}", std::process::id()));
        let p = d.join("yggdrasil/tablet.conf");
        assert_eq!(Conf::load(&p), Conf::default(), "no file: the defaults");
        c.save(&p).unwrap();
        assert_eq!(Conf::load(&p), c);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_built_in_screen_and_its_touch_and_pen_are_turned_together() {
        let screens = vec![Screen { name: "DP-1".into(), ..screen("DP-1", 0) }, screen("eDP-1", 0)];
        assert_eq!(panel(&screens).map(|s| s.name.as_str()), Some("eDP-1"));
        assert!(panel(&[Screen { disabled: true, ..screen("eDP-1", 0) }]).is_none(), "lid closed and the panel off: nothing to turn");
        let devices = r#"{"mice":[{"name":"tpps/2-elan-trackpoint"}],"keyboards":[],"touch":[{"address":"0x1","name":"wacom-hid-5349-finger"}],
            "tablets":[{"address":"0x2","name":"wacom-hid-5349-pen"},{"address":"0x3","type":"tabletTool"},{"name":"wacom-intuos-pro-m-pen"},{"name":"odd name"}],
            "switches":[{"name":"lid-switch"}]}"#;
        let inputs = panel_inputs(devices);
        assert_eq!(inputs, ["wacom-hid-5349-finger", "wacom-hid-5349-pen"], "a drawing tablet on a cable is not turned");
        let lua = rotate_lua(&screen("eDP-1", 0), 1, &inputs).unwrap();
        assert_eq!(
            lua,
            "hl.monitor({ output = \"eDP-1\", mode = \"2880x1800@60.001\", position = \"0x0\", scale = 1.75, transform = 1 }); \
             hl.device({ name = \"wacom-hid-5349-finger\", transform = 1 }); hl.device({ name = \"wacom-hid-5349-pen\", transform = 1 })"
        );
        assert!(rotate_lua(&screen("eDP-1\"); os.exit(", 0), 1, &[]).is_none(), "nothing odd goes into Lua");
        assert!(rotate_lua(&screen("eDP-1", 0), 9, &[]).is_none());
        assert_eq!(num(2.0), "2");
        assert_eq!(num(1.25), "1.25");
    }

    #[test]
    fn socket_lines_and_hyprland_events() {
        assert_eq!(parse_command("mode on\n"), Some(Event::Mode(true)));
        assert_eq!(parse_command("mode off"), Some(Event::Mode(false)));
        assert_eq!(parse_command("conf"), Some(Event::Conf));
        assert_eq!(parse_command("rm -rf"), None);
        assert!(needs_reapply("configreloaded>>") && needs_reapply("monitoraddedv2>>1,eDP-1,x") && !needs_reapply("activewindow>>a,b"));
    }

    #[test]
    fn the_keyboard_process_starts_and_ends_with_tablet_mode() {
        let mut osk = Osk::new(vec!["/usr/bin/sleep".into(), "30".into()]);
        assert!(!osk.shown());
        osk.set(true).unwrap();
        let pid = osk.child.as_ref().unwrap().id();
        osk.set(true).unwrap();
        assert_eq!(osk.child.as_ref().unwrap().id(), pid, "shown once, not twice");
        osk.set(false).unwrap();
        assert!(!osk.shown() && osk.child.is_none());
        let mut missing = Osk::new(vec!["/nonexistent/wvkbd-mobintl".into()]);
        assert!(missing.set(true).unwrap_err().contains("Agora"), "not installed: said so, nothing started");
        assert!(missing.set(false).is_ok());
    }
}
