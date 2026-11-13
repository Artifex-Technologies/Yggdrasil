//! Keyboard and mouse lights off while the screen is dimmed, off or shows the screensaver, and back as they were when you are
//! back (Settings > Peripherals > Keyboard: "Keyboard and mouse lights off while dimmed"):
//!   * Razer keyboards, mice, keypads and mouse mats, through openrazer's daemon on the session bus (if it runs; asking never
//!     starts it): each lighting zone a device has (the whole device, logo, scroll wheel, backlight, left and right strips)
//!   * a laptop's keyboard backlight (/sys/class/leds/*kbd_backlight*), through logind, which lets the signed-in user set it
//!
//! Only what was lit is turned off and remembered, so nothing that was already off is changed. What to put back is kept in
//! ~/.local/state/yggdrasil/lights-off, written in full (and synced) before anything goes dark: a session that ends while the
//! lights are off loses nothing, and "lights-on" at the next start puts them back. A list the old shell hook wrote (the same
//! format) is put back the same way, once.
//!
//! The runs take turns: one worker thread, off the Wayland thread, so a hung daemon never holds up the idle loop; every D-Bus
//! call has a time limit (`TIMEOUT`). The lock file the shell hook used is taken too, so a copy of the old hook somewhere
//! else never runs at the same time. Values read back from the list are checked before they go anywhere: a device's name
//! must be one openrazer could have given, a zone one of the six, a brightness a number from 1 to 100 (a backlight: a whole
//! number up to the light's own maximum).

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use zbus::blocking::Connection;

/// openrazer's daemon on the session bus.
pub const RAZER: &str = "org.razer";
const LOGIN: &str = "org.freedesktop.login1";
const SESSION_PATH: &str = "/org/freedesktop/login1/session/auto";
const SESSION_IFACE: &str = "org.freedesktop.login1.Session";
/// The longest any one D-Bus call may take.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// The kinds of Razer device whose lights go off.
const KINDS: [&str; 4] = ["keyboard", "mouse", "keypad", "mousemat"];
/// A lighting zone: openrazer's interface (razer.device.lighting.<zone>), its getter and its setter.
pub const ZONES: [(&str, &str, &str); 6] = [
    ("brightness", "getBrightness", "setBrightness"),
    ("logo", "getLogoBrightness", "setLogoBrightness"),
    ("scroll", "getScrollBrightness", "setScrollBrightness"),
    ("backlight", "getBacklightBrightness", "setBacklightBrightness"),
    ("left", "getLeftBrightness", "setLeftBrightness"),
    ("right", "getRightBrightness", "setRightBrightness"),
];
/// The list's name in the state folder.
pub const STATE: &str = "lights-off";
/// The list is never read past this (a few lines in real life).
const MAX_STATE: u64 = 64 * 1024;
/// Nor are more entries than this put back (or more devices asked about).
const MAX_ENTRIES: usize = 256;
const MAX_DEVICES: usize = 32;
/// At the start of a session openrazer's daemon may not be up yet: looks for it (one `pause` apart) before giving up.
const WAIT_LOOKS: u32 = 30;
/// How long a run waits for the other's lock before giving up (it never needs more than a few seconds).
const LOCK_WAIT: Duration = Duration::from_secs(60);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    Off,
    On,
}

/// One light that was turned off, and the brightness to put back.
#[derive(Clone, Debug, PartialEq)]
pub enum Entry {
    /// A Razer device's zone (an index into `ZONES`), 1 to 100.
    Razer { id: String, zone: usize, value: f64 },
    /// A keyboard backlight in /sys/class/leds.
    Leds { name: String, value: u32 },
}

impl Entry {
    /// The entry as a line of the list (the shell hook's format: kind, id, zone, setter, value).
    pub fn line(&self) -> String {
        match self {
            Entry::Razer { id, zone, value } => format!("razer {id} {} {} {value}", ZONES[*zone].0, ZONES[*zone].2),
            Entry::Leds { name, value } => format!("leds {name} - - {value}"),
        }
    }
}

/// A device's name on openrazer's bus (its serial number): what may go into an object path.
pub fn razer_id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
}

/// A keyboard backlight's name in /sys/class/leds ("tpacpi::kbd_backlight", "asus::kbd_backlight").
pub fn leds_name_ok(name: &str) -> bool {
    name.len() <= 64
        && name.contains("kbd_backlight")
        && name.bytes().all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b':' | b'-' | b'.'))
}

/// A brightness from the list: digits with at most one point, nothing else (no sign, exponent, "inf" or "nan").
fn number(s: &str) -> Option<f64> {
    let ok = !s.is_empty()
        && s.len() <= 24
        && s.bytes().all(|c| c.is_ascii_digit() || c == b'.')
        && s.bytes().filter(|&c| c == b'.').count() <= 1
        && s.bytes().any(|c| c.is_ascii_digit());
    ok.then(|| s.parse::<f64>().ok()).flatten().filter(|v| v.is_finite())
}

/// One line of the list, or None when anything in it is not what this program (or the shell hook) writes.
pub fn parse_line(line: &str) -> Option<Entry> {
    let f: Vec<&str> = line.split_whitespace().collect();
    let [kind, id, zone, set, value] = f[..] else { return None };
    match kind {
        "razer" => {
            let z = ZONES.iter().position(|(n, _, s)| *n == zone && *s == set)?;
            let v = number(value).filter(|v| *v > 0.0 && *v <= 100.0)?;
            razer_id_ok(id).then(|| Entry::Razer { id: id.into(), zone: z, value: v })
        }
        "leds" => {
            let v = value.bytes().all(|c| c.is_ascii_digit()).then(|| value.parse::<u32>().ok()).flatten().filter(|v| *v > 0)?;
            (zone == "-" && set == "-" && leds_name_ok(id)).then(|| Entry::Leds { name: id.into(), value: v })
        }
        _ => None,
    }
}

/// Every good line of a list, at most `MAX_ENTRIES`; the rest is left out.
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines().filter_map(parse_line).take(MAX_ENTRIES).collect()
}

/// The daemons, reached through D-Bus. Tests put made-up ones here.
pub trait Peers {
    /// openrazer's daemon, when it runs now (asking never starts it).
    fn razer(&mut self) -> Option<&Connection>;
    /// logind (the system bus).
    fn login(&mut self) -> Option<&Connection>;
    /// Between two looks for a daemon that is starting.
    fn pause(&mut self);
}

/// The real buses: connected when first needed, each call bounded by `TIMEOUT`.
pub struct Live {
    session: Option<Option<Connection>>,
    system: Option<Option<Connection>>,
    razer_up: bool,
    /// Both buses at this address instead (the measurements: a private bus with made-up daemons).
    address: Option<String>,
}

impl Live {
    pub fn new() -> Live {
        Live { session: None, system: None, razer_up: false, address: None }
    }

    #[cfg(test)]
    pub fn at(address: &str) -> Live {
        Live { address: Some(address.into()), ..Live::new() }
    }

    fn connect(&self, system: bool) -> Option<Connection> {
        let b = match (&self.address, system) {
            (Some(a), _) => zbus::blocking::connection::Builder::address(a.as_str()),
            (None, false) => zbus::blocking::connection::Builder::session(),
            (None, true) => zbus::blocking::connection::Builder::system(),
        };
        b.map(|b| b.method_timeout(TIMEOUT)).and_then(|b| b.build()).ok()
    }
}

impl Peers for Live {
    fn razer(&mut self) -> Option<&Connection> {
        if self.session.is_none() {
            self.session = Some(self.connect(false));
        }
        let bus = self.session.as_ref()?.as_ref()?;
        if !self.razer_up {
            let dbus = zbus::blocking::fdo::DBusProxy::new(bus).ok()?;
            let name = zbus::names::BusName::try_from(RAZER).ok()?;
            self.razer_up = dbus.name_has_owner(name).unwrap_or(false);
        }
        self.razer_up.then_some(bus)
    }

    fn login(&mut self) -> Option<&Connection> {
        if self.system.is_none() {
            self.system = Some(self.connect(true));
        }
        self.system.as_ref()?.as_ref()
    }

    fn pause(&mut self) {
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// Where the list and the backlights are.
pub struct Places {
    pub state_dir: PathBuf,
    pub leds_dir: PathBuf,
}

impl Places {
    pub fn live() -> Places {
        Places { state_dir: yggdrasil_conf::dirs::state(), leds_dir: PathBuf::from("/sys/class/leds") }
    }
}

/// What a run did, for the log.
#[derive(Debug, Default, PartialEq)]
pub struct Report {
    /// Lights turned off or put back.
    pub done: usize,
    /// Calls that failed (a device unplugged, a daemon that did not answer).
    pub failed: usize,
    /// Lines of the list that were not put back because they were not valid.
    pub refused: usize,
    /// Nothing to do: off when they are off already, on when nothing is off.
    pub idle: bool,
}

fn razer_path(id: &str) -> String {
    format!("/org/razer/device/{id}")
}

/// A call to openrazer's daemon: a getter (no value) or a setter (a brightness).
fn razer_call(bus: &Connection, path: &str, iface: &str, method: &str, value: Option<f64>) -> Option<zbus::Message> {
    match value {
        Some(v) => bus.call_method(Some(RAZER), path, Some(iface), method, &v),
        None => bus.call_method(Some(RAZER), path, Some(iface), method, &()),
    }
    .ok()
}

fn razer_get(bus: &Connection, id: &str, zone: usize) -> Option<f64> {
    let (z, get, _) = ZONES[zone];
    razer_call(bus, &razer_path(id), &format!("razer.device.lighting.{z}"), get, None)?.body().deserialize::<f64>().ok()
}

fn set(peers: &mut dyn Peers, e: &Entry, value: Option<f64>) -> bool {
    match e {
        Entry::Razer { id, zone, value: v } => {
            let Some(bus) = peers.razer() else { return false };
            let (z, _, setter) = ZONES[*zone];
            let b = value.unwrap_or(*v);
            razer_call(bus, &razer_path(id), &format!("razer.device.lighting.{z}"), setter, Some(b)).is_some()
        }
        Entry::Leds { name, value: v } => {
            let Some(bus) = peers.login() else { return false };
            let b = value.map(|x| x as u32).unwrap_or(*v);
            bus.call_method(Some(LOGIN), SESSION_PATH, Some(SESSION_IFACE), "SetBrightness", &("leds", name.as_str(), b)).is_ok()
        }
    }
}

/// The lit Razer zones and keyboard backlights, in the order the shell hook listed them.
fn lit(at: &Places, peers: &mut dyn Peers) -> Vec<Entry> {
    let mut list = vec![];
    if let Some(bus) = peers.razer() {
        let ids: Vec<String> = razer_call(bus, "/org/razer", "razer.devices", "getDevices", None)
            .and_then(|m| m.body().deserialize::<Vec<String>>().ok())
            .unwrap_or_default();
        for id in ids.into_iter().filter(|i| razer_id_ok(i)).take(MAX_DEVICES) {
            let kind = razer_call(bus, &razer_path(&id), "razer.device.misc", "getDeviceType", None)
                .and_then(|m| m.body().deserialize::<String>().ok())
                .unwrap_or_default();
            if !KINDS.contains(&kind.as_str()) {
                continue;
            }
            for zone in 0..ZONES.len() {
                // a zone below 1 counts as off (the hook compared the whole part with 0)
                if let Some(b) = razer_get(bus, &id, zone).filter(|b| b.is_finite() && *b >= 1.0) {
                    list.push(Entry::Razer { id: id.clone(), zone, value: b.min(100.0) });
                }
            }
        }
    }
    let mut names: Vec<String> = fs::read_dir(&at.leds_dir)
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).filter(|n| leds_name_ok(n)).collect())
        .unwrap_or_default();
    names.sort();
    for name in names {
        let b = fs::read_to_string(at.leds_dir.join(&name).join("brightness")).ok().and_then(|s| s.trim().parse::<u32>().ok());
        if let Some(b) = b.filter(|b| *b > 0) {
            list.push(Entry::Leds { name, value: b });
        }
    }
    list.truncate(MAX_ENTRIES);
    list
}

/// A backlight's own maximum; a value above it (or for a light that is gone) is not put back.
fn leds_max(at: &Places, name: &str) -> Option<u32> {
    fs::read_to_string(at.leds_dir.join(name).join("max_brightness")).ok()?.trim().parse().ok()
}

/// The list, written in full and synced before it takes the old one's place.
fn write_state(at: &Places, list: &[Entry]) -> std::io::Result<()> {
    let tmp = at.state_dir.join(format!("{STATE}.new"));
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    let text: String = list.iter().map(|e| e.line() + "\n").collect();
    f.write_all(text.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, at.state_dir.join(STATE))?;
    if let Ok(d) = fs::File::open(&at.state_dir) {
        let _ = d.sync_all();
    }
    Ok(())
}

/// The runs take turns: the shell hook's lock file, held until dropped.
struct Lock {
    _file: fs::File,
}

fn lock(dir: &Path) -> Option<Lock> {
    use std::os::fd::AsRawFd;
    let f = fs::OpenOptions::new().write(true).create(true).truncate(false).mode(0o600).open(dir.join("lights.lock")).ok()?;
    let until = Instant::now() + LOCK_WAIT;
    loop {
        // SAFETY: flock on a descriptor this function owns
        if unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Some(Lock { _file: f });
        }
        if Instant::now() >= until {
            return None;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// One run: lights off (remember what was lit, then turn it off) or back on (put back what the list says, then forget it).
pub fn run(op: Op, at: &Places, peers: &mut dyn Peers) -> Result<Report, String> {
    fs::create_dir_all(&at.state_dir).map_err(|e| format!("no state folder: {e}"))?;
    let _lock = lock(&at.state_dir).ok_or("the lights lock stayed taken")?;
    let state = at.state_dir.join(STATE);
    let mut r = Report::default();
    match op {
        Op::Off => {
            // off already (and the list of what to put back is kept)
            if fs::symlink_metadata(&state).is_ok() {
                r.idle = true;
                return Ok(r);
            }
            let list = lit(at, peers);
            // the list is complete before anything goes dark: a run cut short leaves the lights on, never a list missing what
            // it turned off
            write_state(at, &list).map_err(|e| format!("cannot write {}: {e}", state.display()))?;
            for e in &list {
                if set(peers, e, Some(0.0)) {
                    r.done += 1;
                } else {
                    r.failed += 1;
                }
            }
        }
        Op::On => {
            let Ok(f) = fs::File::open(&state) else {
                r.idle = true;
                return Ok(r);
            };
            let mut bytes = vec![];
            let _ = f.take(MAX_STATE).read_to_end(&mut bytes);
            let text = String::from_utf8_lossy(&bytes);
            let list = parse(&text);
            r.refused = text.lines().filter(|l| !l.trim().is_empty()).count().saturating_sub(list.len());
            // the daemon, looked for once a second for up to 30 s when anything of a Razer's is to be put back
            let razer_up = list.iter().any(|e| matches!(e, Entry::Razer { .. })) && {
                let mut looks = 0;
                loop {
                    if peers.razer().is_some() {
                        break true;
                    }
                    if looks >= WAIT_LOOKS {
                        break false;
                    }
                    peers.pause();
                    looks += 1;
                }
            };
            for e in &list {
                match e {
                    Entry::Leds { name, value } if leds_max(at, name).is_none_or(|m| *value > m) => {
                        r.refused += 1;
                        continue;
                    }
                    Entry::Razer { .. } if !razer_up => {
                        r.failed += 1;
                        continue;
                    }
                    _ => {}
                }
                if set(peers, e, None) {
                    r.done += 1;
                } else {
                    r.failed += 1;
                }
            }
            fs::remove_file(&state).map_err(|e| format!("cannot remove {}: {e}", state.display()))?;
        }
    }
    Ok(r)
}

static WORKER: OnceLock<Sender<Op>> = OnceLock::new();

/// Asks the worker for a run and returns at once. Runs take turns; when several wait, only the last is done (off and on
/// again while a run was busy end where they began).
pub fn request(op: Op) {
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = channel();
        if let Err(e) = std::thread::Builder::new().name("lights".into()).spawn(move || worker(rx)) {
            crate::log(&format!("lights: cannot start the worker: {e}"));
        }
        tx
    });
    let _ = tx.send(op);
}

fn worker(rx: Receiver<Op>) {
    while let Ok(mut op) = rx.recv() {
        while let Ok(next) = rx.try_recv() {
            op = next;
        }
        let what = if op == Op::Off { "off" } else { "on" };
        match run(op, &Places::live(), &mut Live::new()) {
            Ok(r) if r.idle => {}
            Ok(r) => crate::log(&format!("lights {what}: {} done, {} failed, {} refused", r.done, r.failed, r.refused)),
            Err(e) => crate::log(&format!("lights {what}: {e}")),
        }
    }
}

#[cfg(test)]
mod tests;
