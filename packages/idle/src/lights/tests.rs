//! The lights, tested without a bus, a daemon or a device.
//!
//! Differential: the retired shell hook (tests/fixtures/keyboard-lights.sh, byte for byte the hook of commit 840fbc7, kept
//! only for this comparison: it goes after a week's soak, 2026-11-19) and this module run on the same made-up world, and the
//! calls each makes, the list each writes and the brightness each leaves are compared step by step. The shell hook gets a
//! stand-in busctl and sleep on the PATH (canned answers from files, never a bus; the real buses' addresses point nowhere);
//! this module gets a made-up openrazer daemon and logind on private peer-to-peer D-Bus connections (socket pairs in this
//! process). Both sides' backlights are a scratch folder standing in for /sys/class/leds.
//!
//! Then the list's parser on hostile input (property tests): garbage, huge and non-numeric values are never put back.
//!
//! `measure` (ignored; run by hand, see packages/idle/README.md) times both against a made-up daemon on a private bus.

use super::*;
use std::collections::BTreeMap;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use proptest::prelude::*;

const OLD_HOOK: &str = include_str!("../../tests/fixtures/keyboard-lights.sh");

/// openrazer's zone interfaces and methods as the daemon names them (written out here, not taken from `ZONES`).
const OPENRAZER: [(&str, &str, &str); 6] = [
    ("brightness", "getBrightness", "setBrightness"),
    ("logo", "getLogoBrightness", "setLogoBrightness"),
    ("scroll", "getScrollBrightness", "setScrollBrightness"),
    ("backlight", "getBacklightBrightness", "setBacklightBrightness"),
    ("left", "getLeftBrightness", "setLeftBrightness"),
    ("right", "getRightBrightness", "setRightBrightness"),
];

/// A scratch folder, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new() -> Scratch {
        static N: AtomicU32 = AtomicU32::new(0);
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
        let p = std::env::temp_dir().join(format!("ygg-idle-lights-{}-{}-{nanos}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        fs::create_dir_all(&p).unwrap();
        Scratch(p)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Debug)]
struct Device {
    id: &'static str,
    kind: &'static str,
    /// (zone, brightness as the daemon gives it)
    zones: Vec<(&'static str, &'static str)>,
}

#[derive(Clone, Debug)]
struct Led {
    name: &'static str,
    brightness: &'static str,
    max: &'static str,
}

#[derive(Clone, Debug)]
struct World {
    devices: Vec<Device>,
    leds: Vec<Led>,
    /// openrazer's daemon answers after this many looks for it (0: at once); None: it never runs.
    razer_up_after: Option<u32>,
    /// A list already there (from an earlier session, or the old hook).
    state: Option<&'static str>,
}

/// What one side did in one step, and what it left.
#[derive(Debug, PartialEq)]
struct Seen {
    /// "look" (is the daemon running), "pause", "devices", "type ID", "get ID ZONE METHOD", "set ID ZONE METHOD VALUE",
    /// "login SetBrightness leds NAME VALUE": the calls that reached a daemon, in order.
    calls: Vec<String>,
    /// The list after the step.
    state: Option<String>,
    /// "ID ZONE" -> brightness, after the step.
    razer: BTreeMap<String, String>,
    /// backlight -> brightness, after the step.
    leds: BTreeMap<String, String>,
}

fn op_arg(op: Op) -> &'static str {
    if op == Op::Off {
        "lights-off"
    } else {
        "lights-on"
    }
}

fn leds_now(dir: &Path) -> BTreeMap<String, String> {
    let mut m = BTreeMap::new();
    for e in fs::read_dir(dir).unwrap().flatten() {
        let name = e.file_name().into_string().unwrap();
        m.insert(name, fs::read_to_string(e.path().join("brightness")).unwrap().trim().to_string());
    }
    m
}

fn make_leds(dir: &Path, w: &World) {
    for l in &w.leds {
        fs::create_dir_all(dir.join(l.name)).unwrap();
        fs::write(dir.join(l.name).join("brightness"), format!("{}\n", l.brightness)).unwrap();
        fs::write(dir.join(l.name).join("max_brightness"), format!("{}\n", l.max)).unwrap();
    }
}

// ---- the old hook, with stand-ins on the PATH ----

/// busctl as the hook uses it, answering from the made-up world in $FAKE (never a bus). It refuses what the real one would
/// refuse before sending anything (an object path with characters D-Bus does not allow, a double it cannot read), and logs
/// only the calls a daemon would have answered.
const STAND_IN_BUSCTL: &str = r#"#!/bin/sh
w=$FAKE
[ "$1" = --user ] && shift
case $1 in
status)
  [ "$2" = org.razer ] || exit 1
  echo look >>"$w/calls"
  n=$(cat "$w/looks"); echo $((n + 1)) >"$w/looks"
  [ -e "$w/up_after" ] && [ "$n" -ge "$(cat "$w/up_after")" ] || exit 1
  : >"$w/up"
  exit 0 ;;
call) shift ;;
*) exit 1 ;;
esac
dest=$1 path=$2 iface=$3 method=$4
shift 4
case $dest in
org.freedesktop.login1)
  [ "$path $iface $method $1 $2" = "/org/freedesktop/login1/session/auto org.freedesktop.login1.Session SetBrightness ssu leds" ] || exit 1
  case $4 in ''|*[!0-9]*) exit 1 ;; esac
  [ -d "$LEDS/$3" ] || exit 1
  echo "login SetBrightness leds $3 $4" >>"$w/calls"
  echo "$4" >"$LEDS/$3/brightness"
  exit 0 ;;
org.razer) ;;
*) exit 1 ;;
esac
[ -e "$w/up" ] || exit 1
case $path in
/org/razer)
  [ "$iface $method" = "razer.devices getDevices" ] || exit 1
  echo devices >>"$w/calls"
  set -- $(cat "$w/order")
  printf 'as %s' "$#"
  for d; do printf ' "%s"' "$d"; done
  echo
  exit 0 ;;
/org/razer/device/*) id=${path#/org/razer/device/} ;;
*) exit 1 ;;
esac
case $id in ''|*[!A-Za-z0-9_]*) echo "Invalid object path" >&2; exit 1 ;; esac
d=$w/razer/$id
[ -d "$d" ] || exit 1
case $iface in
razer.device.misc)
  [ "$method" = getDeviceType ] || exit 1
  echo "type $id" >>"$w/calls"
  printf 's "%s"\n' "$(cat "$d/type")"
  exit 0 ;;
razer.device.lighting.*) zone=${iface#razer.device.lighting.} ;;
*) exit 1 ;;
esac
case $zone in ''|*[!a-z]*) exit 1 ;; esac
z=$d/$zone
[ -d "$z" ] || exit 1
if [ "$method" = "$(cat "$z/get")" ] && [ $# = 0 ]; then
  echo "get $id $zone $method" >>"$w/calls"
  echo "d $(cat "$z/value")"
  exit 0
fi
if [ "$method" = "$(cat "$z/set")" ] && [ "$1" = d ] && [ $# = 2 ]; then
  case $2 in ''|*[!0-9.]*|*.*.*) echo "Failed to parse" >&2; exit 1 ;; esac
  echo "set $id $zone $method $2" >>"$w/calls"
  echo "$2" >"$z/value"
  exit 0
fi
exit 1
"#;

/// The old hook, reading its backlights from `leds` instead of /sys/class/leds.
fn hook_text(leds: &Path) -> String {
    const GLOB: &str = "for d in /sys/class/leds/*kbd_backlight*; do";
    assert_eq!(OLD_HOOK.matches(GLOB).count(), 1);
    OLD_HOOK.replace(GLOB, &format!("for d in {}/*kbd_backlight*; do", leds.display()))
}

const STAND_IN_SLEEP: &str = "#!/bin/sh\necho pause >>\"$FAKE/calls\"\n";

fn shell_side(world: &World, ops: &[Op]) -> Vec<Seen> {
    let s = Scratch::new();
    let (bin, fake, leds, xdg) = (s.0.join("bin"), s.0.join("fake"), s.0.join("leds"), s.0.join("xdg-state"));
    for d in [&bin, &fake, &leds, &xdg.join("yggdrasil")] {
        fs::create_dir_all(d).unwrap();
    }
    for (name, text) in [("busctl", STAND_IN_BUSCTL), ("sleep", STAND_IN_SLEEP)] {
        fs::write(bin.join(name), text).unwrap();
        fs::set_permissions(bin.join(name), fs::Permissions::from_mode(0o755)).unwrap();
    }
    fs::write(fake.join("calls"), "").unwrap();
    fs::write(fake.join("looks"), "0\n").unwrap();
    if let Some(n) = world.razer_up_after {
        fs::write(fake.join("up_after"), format!("{n}\n")).unwrap();
    }
    fs::write(fake.join("order"), world.devices.iter().map(|d| d.id).collect::<Vec<_>>().join(" ")).unwrap();
    for d in &world.devices {
        let dd = fake.join("razer").join(d.id);
        fs::create_dir_all(&dd).unwrap();
        fs::write(dd.join("type"), d.kind).unwrap();
        for (zone, value) in &d.zones {
            let (_, get, set) = OPENRAZER.iter().find(|z| z.0 == *zone).unwrap();
            let z = dd.join(zone);
            fs::create_dir_all(&z).unwrap();
            fs::write(z.join("value"), value).unwrap();
            fs::write(z.join("get"), get).unwrap();
            fs::write(z.join("set"), set).unwrap();
        }
    }
    make_leds(&leds, world);
    let state = xdg.join("yggdrasil").join(STATE);
    if let Some(t) = world.state {
        fs::write(&state, t).unwrap();
    }
    // the hook, with the scratch backlights in place of /sys/class/leds
    let hook = s.0.join("keyboard-lights");
    fs::write(&hook, hook_text(&leds)).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap_or_default());
    let mut seen = vec![];
    for op in ops {
        let st = Command::new("sh")
            .arg(&hook)
            .arg(op_arg(*op))
            .env("PATH", &path)
            .env("FAKE", &fake)
            .env("LEDS", &leds)
            .env("XDG_STATE_HOME", &xdg)
            .env("HOME", &s.0)
            .env("LC_ALL", "C")
            // were a real busctl ever reached, it would find no bus
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent/ygg-test")
            .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent/ygg-test")
            .status()
            .unwrap();
        assert!(st.success());
        let calls = fs::read_to_string(fake.join("calls")).unwrap().lines().map(String::from).collect();
        fs::write(fake.join("calls"), "").unwrap();
        let mut razer = BTreeMap::new();
        for d in &world.devices {
            for (zone, _) in &d.zones {
                let v = fs::read_to_string(fake.join("razer").join(d.id).join(zone).join("value")).unwrap();
                razer.insert(format!("{} {zone}", d.id), v.trim().to_string());
            }
        }
        seen.push(Seen { calls, state: fs::read_to_string(&state).ok(), razer, leds: leds_now(&leds) });
    }
    seen
}

// ---- this module, with made-up daemons ----

/// The made-up daemons' world.
struct Shared {
    calls: Vec<String>,
    order: Vec<String>,
    kinds: BTreeMap<String, String>,
    /// "ID ZONE" -> brightness
    razer: BTreeMap<String, String>,
    leds_dir: PathBuf,
    looks: u32,
}

impl Shared {
    fn new(world: &World, leds_dir: &Path) -> Shared {
        let mut razer = BTreeMap::new();
        for d in &world.devices {
            for (zone, value) in &d.zones {
                razer.insert(format!("{} {zone}", d.id), value.to_string());
            }
        }
        Shared {
            calls: vec![],
            order: world.devices.iter().map(|d| d.id.to_string()).collect(),
            kinds: world.devices.iter().map(|d| (d.id.to_string(), d.kind.to_string())).collect(),
            razer,
            leds_dir: leds_dir.to_path_buf(),
            looks: 0,
        }
    }
}

enum Answer {
    Unit,
    Double(f64),
    Text(String),
    List(Vec<String>),
    Refused,
}

/// A method call to the made-up openrazer daemon or logind.
fn answer(w: &Mutex<Shared>, msg: &zbus::Message) -> Answer {
    let h = msg.header();
    let path = h.path().map(|p| p.as_str().to_string()).unwrap_or_default();
    let iface = h.interface().map(|i| i.as_str().to_string()).unwrap_or_default();
    let member = h.member().map(|m| m.as_str().to_string()).unwrap_or_default();
    let mut w = w.lock().unwrap();
    if path == "/org/freedesktop/login1/session/auto" && iface == "org.freedesktop.login1.Session" && member == "SetBrightness" {
        let Ok((sub, name, v)) = msg.body().deserialize::<(String, String, u32)>() else { return Answer::Refused };
        let dir = w.leds_dir.join(&name);
        if sub != "leds" || name.contains('/') || !dir.is_dir() {
            return Answer::Refused;
        }
        w.calls.push(format!("login SetBrightness leds {name} {v}"));
        fs::write(dir.join("brightness"), format!("{v}\n")).unwrap();
        return Answer::Unit;
    }
    if path == "/org/razer" && iface == "razer.devices" && member == "getDevices" {
        w.calls.push("devices".into());
        return Answer::List(w.order.clone());
    }
    let Some(id) = path.strip_prefix("/org/razer/device/").map(String::from) else { return Answer::Refused };
    let Some(kind) = w.kinds.get(&id).cloned() else { return Answer::Refused };
    if iface == "razer.device.misc" && member == "getDeviceType" {
        w.calls.push(format!("type {id}"));
        return Answer::Text(kind);
    }
    let Some(zone) = iface.strip_prefix("razer.device.lighting.") else { return Answer::Refused };
    let Some((zone, get, set)) = OPENRAZER.iter().find(|z| z.0 == zone) else { return Answer::Refused };
    let key = format!("{id} {zone}");
    let Some(now) = w.razer.get(&key).cloned() else { return Answer::Refused };
    if member == *get {
        w.calls.push(format!("get {id} {zone} {get}"));
        return Answer::Double(now.parse().unwrap());
    }
    if member == *set {
        let Ok(v) = msg.body().deserialize::<f64>() else { return Answer::Refused };
        w.calls.push(format!("set {id} {zone} {set} {v}"));
        w.razer.insert(key, format!("{v}"));
        return Answer::Unit;
    }
    Answer::Refused
}

/// Answers the method calls arriving on `conn` until it closes.
fn serve(conn: Connection, w: Arc<Mutex<Shared>>) {
    let calls = zbus::blocking::MessageIterator::from(&conn);
    std::thread::spawn(move || {
        for m in calls {
            let Ok(m) = m else { break };
            if m.message_type() != zbus::message::Type::MethodCall {
                continue;
            }
            let h = m.header();
            let _ = match answer(&w, &m) {
                Answer::Unit => conn.reply(&h, &()),
                Answer::Double(v) => conn.reply(&h, &v),
                Answer::Text(t) => conn.reply(&h, &t),
                Answer::List(l) => conn.reply(&h, &l),
                Answer::Refused => conn.reply_error(&h, "org.freedesktop.DBus.Error.UnknownMethod", &"not here"),
            };
        }
    });
}

/// A peer-to-peer connection to the made-up daemons.
// unix_stream: deprecated only because a build with zbus's tokio feature would lack it, and this one has no tokio
#[allow(deprecated)]
fn p2p(w: &Arc<Mutex<Shared>>) -> Connection {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let server = std::thread::spawn(move || {
        zbus::blocking::connection::Builder::unix_stream(a).server(zbus::Guid::generate()).unwrap().p2p().build().unwrap()
    });
    let client = zbus::blocking::connection::Builder::unix_stream(b).p2p().method_timeout(TIMEOUT).build().unwrap();
    serve(server.join().unwrap(), w.clone());
    client
}

struct Fake {
    w: Arc<Mutex<Shared>>,
    razer: Connection,
    login: Connection,
    up_after: Option<u32>,
    up: bool,
}

impl Fake {
    fn new(w: &Arc<Mutex<Shared>>, up_after: Option<u32>) -> Fake {
        Fake { w: w.clone(), razer: p2p(w), login: p2p(w), up_after, up: false }
    }
}

impl Peers for Fake {
    fn razer(&mut self) -> Option<&Connection> {
        if !self.up {
            let mut w = self.w.lock().unwrap();
            w.calls.push("look".into());
            w.looks += 1;
            self.up = self.up_after.is_some_and(|n| w.looks > n);
        }
        self.up.then_some(&self.razer)
    }
    fn login(&mut self) -> Option<&Connection> {
        Some(&self.login)
    }
    fn pause(&mut self) {
        self.w.lock().unwrap().calls.push("pause".into());
    }
}

fn rust_side(world: &World, ops: &[Op]) -> Vec<Seen> {
    let s = Scratch::new();
    let at = Places { state_dir: s.0.join("state"), leds_dir: s.0.join("leds") };
    fs::create_dir_all(&at.state_dir).unwrap();
    fs::create_dir_all(&at.leds_dir).unwrap();
    make_leds(&at.leds_dir, world);
    if let Some(t) = world.state {
        fs::write(at.state_dir.join(STATE), t).unwrap();
    }
    let w = Arc::new(Mutex::new(Shared::new(world, &at.leds_dir)));
    let mut seen = vec![];
    for op in ops {
        // a run of its own each time, as the worker does
        let mut peers = Fake::new(&w, world.razer_up_after);
        run(*op, &at, &mut peers).unwrap();
        let mut sh = w.lock().unwrap();
        let calls = std::mem::take(&mut sh.calls);
        seen.push(Seen {
            calls,
            state: fs::read_to_string(at.state_dir.join(STATE)).ok(),
            razer: sh.razer.clone(),
            leds: leds_now(&at.leds_dir),
        });
    }
    seen
}

// ---- the worlds ----

fn dev(id: &'static str, kind: &'static str, zones: &[(&'static str, &'static str)]) -> Device {
    Device { id, kind, zones: zones.to_vec() }
}

fn led(name: &'static str, brightness: &'static str, max: &'static str) -> Led {
    Led { name, brightness, max }
}

/// The manual test of 2026-11-12: a keyboard at 75, a mouse whose own brightness reads 0 while its logo is at 75 and its
/// wheel at 40; with a keypad, a mouse mat below 1 (counts as off), a headset (not a kind whose lights go off), a name
/// D-Bus would not take, two laptop backlights (one dark) and a caps-lock light (not a keyboard backlight).
fn desk() -> World {
    World {
        devices: vec![
            dev("KBD0001", "keyboard", &[("brightness", "75")]),
            dev("MSE0001", "mouse", &[("brightness", "0"), ("logo", "75"), ("scroll", "40")]),
            dev("PAD0001", "keypad", &[("brightness", "62.5"), ("backlight", "0")]),
            dev("MAT0001", "mousemat", &[("brightness", "0.5"), ("left", "20"), ("right", "100")]),
            dev("HST0001", "headset", &[("brightness", "50")]),
            dev("bad-id", "keyboard", &[("brightness", "60")]),
        ],
        leds: vec![
            led("yggtest_kbd_backlight", "2", "3"),
            led("yggtest_b_kbd_backlight", "1", "1"),
            led("yggtest_capslock", "1", "1"),
            led("yggtest_dark_kbd_backlight", "0", "3"),
        ],
        razer_up_after: Some(0),
        state: None,
    }
}

fn same(world: World, ops: &[Op]) -> Vec<Seen> {
    let old = shell_side(&world, ops);
    let new = rust_side(&world, ops);
    for (i, (o, n)) in old.iter().zip(&new).enumerate() {
        assert_eq!(o, n, "step {i} ({:?}) differs: old hook left, Rust right", ops[i]);
    }
    assert_eq!(old.len(), new.len());
    new
}

#[test]
fn same_as_the_hook_on_the_desk() {
    let seen = same(desk(), &[Op::Off, Op::Off, Op::On, Op::On]);
    // what was lit is listed, then turned off; the second off changes nothing; on puts it all back and forgets the list
    assert_eq!(
        seen[0].state.as_deref(),
        Some(
            "razer KBD0001 brightness setBrightness 75\nrazer MSE0001 logo setLogoBrightness 75\n\
             razer MSE0001 scroll setScrollBrightness 40\nrazer PAD0001 brightness setBrightness 62.5\n\
             razer MAT0001 left setLeftBrightness 20\nrazer MAT0001 right setRightBrightness 100\n\
             leds yggtest_b_kbd_backlight - - 1\nleds yggtest_kbd_backlight - - 2\n"
        )
    );
    assert!(seen[0].razer.iter().all(|(k, v)| v == "0" || k.starts_with("HST") || k.starts_with("bad") || k == "MAT0001 brightness"));
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "0");
    assert_eq!(seen[1].calls, Vec::<String>::new());
    assert_eq!(seen[2].razer["MSE0001 logo"], "75");
    assert_eq!(seen[2].razer["MSE0001 brightness"], "0");
    assert_eq!(seen[2].leds["yggtest_kbd_backlight"], "2");
    assert_eq!(seen[2].state, None);
    assert_eq!(seen[3].calls, Vec::<String>::new());
}

#[test]
fn same_as_the_hook_without_the_daemon() {
    let seen = same(World { razer_up_after: None, ..desk() }, &[Op::Off, Op::On]);
    assert_eq!(seen[0].state.as_deref(), Some("leds yggtest_b_kbd_backlight - - 1\nleds yggtest_kbd_backlight - - 2\n"));
    assert_eq!(seen[1].leds["yggtest_kbd_backlight"], "2");
}

/// A list an earlier session (the old hook) left, put back once; the daemon comes up after three looks a second apart.
#[test]
fn same_as_the_hook_with_a_list_left_over_and_a_late_daemon() {
    let world = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")]), dev("MSE0001", "mouse", &[("brightness", "0"), ("logo", "0")])],
        razer_up_after: Some(3),
        state: Some(
            "razer KBD0001 brightness setBrightness 75\nrazer MSE0001 logo setLogoBrightness 75\nleds yggtest_kbd_backlight - - 2\n",
        ),
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
    };
    let seen = same(world, &[Op::On, Op::On]);
    assert_eq!(seen[0].calls.iter().filter(|c| *c == "pause").count(), 3);
    assert_eq!(seen[0].razer["KBD0001 brightness"], "75");
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "2");
    assert_eq!(seen[0].state, None);
}

/// The daemon never comes: 30 pauses, the backlight is put back, the list goes.
#[test]
fn same_as_the_hook_when_the_daemon_never_comes() {
    let world = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")])],
        razer_up_after: None,
        state: Some("razer KBD0001 brightness setBrightness 75\nleds yggtest_kbd_backlight - - 2\n"),
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
    };
    let seen = same(world, &[Op::On]);
    assert_eq!(seen[0].calls.iter().filter(|c| *c == "pause").count(), 30);
    assert_eq!(seen[0].calls.iter().filter(|c| *c == "look").count(), 31);
    assert_eq!(seen[0].razer["KBD0001 brightness"], "0");
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "2");
}

/// Lines both refuse: not numbers, too few or too many fields, names that are not names. One good line among them.
#[test]
fn same_as_the_hook_on_a_hostile_list() {
    let world = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")]), dev("MSE0001", "mouse", &[("logo", "0")])],
        razer_up_after: Some(0),
        state: Some(
            "razer KBD0001 brightness setBrightness 7x\n\
             razer KBD0001 brightness setBrightness\n\
             razer ../../x brightness setBrightness 5\n\
             razer KBD0001 brightness setBrightness -5\n\
             razer KBD0001 brightness setBrightness 1.2.3\n\
             razer KBD0001 brightness setBrightness 75 extra\n\
             razer KBD0001 brightness setBrightness $(id)\n\
             razer KBD0001 bright;ness setBrightness 5\n\
             leds ../../etc - - 5\n\
             leds yggtest_kbd_backlight - - 1e3\n\
             garbage\n\
             \n\
             razer\tMSE0001\tlogo\tsetLogoBrightness\t20\n\
             razer KBD0001 brightness setBrightness 40\n",
        ),
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
    };
    let seen = same(world, &[Op::On]);
    assert_eq!(seen[0].razer["KBD0001 brightness"], "40");
    assert_eq!(seen[0].razer["MSE0001 logo"], "20");
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "0");
}

/// Where the Rust one deliberately differs, each side checked on its own.
#[test]
fn where_the_rust_one_differs() {
    // 1. A laptop backlight is named "vendor::kbd_backlight": the old hook turned it off but refused the name when putting
    //    it back (its check allowed no ':'), so it stayed dark. The Rust one puts it back.
    let laptop = World { devices: vec![], leds: vec![led("yggtest::kbd_backlight", "2", "3")], razer_up_after: None, state: None };
    let old = shell_side(&laptop, &[Op::Off, Op::On]);
    let new = rust_side(&laptop, &[Op::Off, Op::On]);
    assert_eq!(old[0], new[0], "turning off is the same");
    assert_eq!(old[1].leds["yggtest::kbd_backlight"], "0", "the old hook left it dark");
    assert_eq!(new[1].leds["yggtest::kbd_backlight"], "2", "the Rust one puts it back");
    // 2. Values out of range in a list: the old hook passed any digits on (150 to a Razer zone, 99 to a backlight whose
    //    maximum is 3); the Rust one puts back nothing above 100 or above the light's own maximum.
    let wild = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")])],
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
        razer_up_after: Some(0),
        state: Some("razer KBD0001 brightness setBrightness 150\nleds yggtest_kbd_backlight - - 99\n"),
    };
    let old = shell_side(&wild, &[Op::On]);
    let new = rust_side(&wild, &[Op::On]);
    assert_eq!((old[0].razer["KBD0001 brightness"].as_str(), old[0].leds["yggtest_kbd_backlight"].as_str()), ("150", "99"));
    assert_eq!((new[0].razer["KBD0001 brightness"].as_str(), new[0].leds["yggtest_kbd_backlight"].as_str()), ("0", "0"));
    assert_eq!(new[0].state, None, "the list is gone either way");
}

// ---- the list ----

#[test]
fn a_list_from_the_old_hook_reads_back() {
    let old = "razer KBD0001 brightness setBrightness 75\nrazer MSE0001 logo setLogoBrightness 75\n\
               razer MSE0001 scroll setScrollBrightness 40\nleds asus::kbd_backlight - - 2\n";
    let list = parse(old);
    assert_eq!(list.len(), 4);
    assert_eq!(list[1], Entry::Razer { id: "MSE0001".into(), zone: 1, value: 75.0 });
    assert_eq!(list[3], Entry::Leds { name: "asus::kbd_backlight".into(), value: 2 });
    // and this module writes the same lines
    assert_eq!(list.iter().map(|e| e.line() + "\n").collect::<String>(), old);
}

#[test]
fn a_huge_list_is_read_only_so_far() {
    let s = Scratch::new();
    let at = Places { state_dir: s.0.join("state"), leds_dir: s.0.join("leds") };
    fs::create_dir_all(&at.state_dir).unwrap();
    fs::create_dir_all(&at.leds_dir).unwrap();
    make_leds(&at.leds_dir, &World { leds: vec![led("yggtest_kbd_backlight", "0", "3")], ..desk() });
    // 10 MB of garbage, then a good line: past the 64 KiB read, so not put back; and 1000 good lines before it: only 256 used
    let mut text = "leds yggtest_kbd_backlight - - 1\n".repeat(1000);
    text.push_str(&"x".repeat(10 << 20));
    text.push_str("\nleds yggtest_kbd_backlight - - 3\n");
    fs::write(at.state_dir.join(STATE), &text).unwrap();
    let w = Arc::new(Mutex::new(Shared::new(&World { devices: vec![], ..desk() }, &at.leds_dir)));
    let r = run(Op::On, &at, &mut Fake::new(&w, Some(0))).unwrap();
    assert_eq!(r.done, MAX_ENTRIES);
    assert_eq!(fs::read_to_string(at.leds_dir.join("yggtest_kbd_backlight/brightness")).unwrap(), "1\n");
    assert!(!at.state_dir.join(STATE).exists());
}

#[test]
fn off_writes_nothing_dark_when_the_list_cannot_be_written() {
    let s = Scratch::new();
    let at = Places { state_dir: s.0.join("state"), leds_dir: s.0.join("leds") };
    fs::create_dir_all(&at.leds_dir).unwrap();
    make_leds(&at.leds_dir, &desk());
    // the state folder is a file: no list can be written there, so nothing may go dark
    fs::write(&at.state_dir, "").unwrap();
    let w = Arc::new(Mutex::new(Shared::new(&desk(), &at.leds_dir)));
    assert!(run(Op::Off, &at, &mut Fake::new(&w, Some(0))).is_err());
    assert_eq!(fs::read_to_string(at.leds_dir.join("yggtest_kbd_backlight/brightness")).unwrap(), "2\n");
    assert!(w.lock().unwrap().calls.iter().all(|c| !c.starts_with("set") && !c.starts_with("login")));
}

fn entry() -> impl Strategy<Value = Entry> {
    prop_oneof![
        ("[A-Za-z0-9_]{1,64}", 0..ZONES.len(), 1.0f64..=100.0).prop_map(|(id, zone, value)| Entry::Razer { id, zone, value }),
        ("[a-z0-9_]{0,10}(::)?kbd_backlight[a-z0-9_.-]{0,10}", 1u32..).prop_map(|(name, value)| Entry::Leds { name, value }),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    /// Whatever the list holds, what comes out is only what may go out.
    #[test]
    fn anything_parsed_is_safe(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        for e in parse(&String::from_utf8_lossy(&bytes)) {
            match e {
                Entry::Razer { id, zone, value } => {
                    prop_assert!(razer_id_ok(&id) && zone < ZONES.len());
                    prop_assert!(value.is_finite() && value > 0.0 && value <= 100.0);
                }
                Entry::Leds { name, value } => prop_assert!(leds_name_ok(&name) && value > 0),
            }
        }
    }

    /// Lines shaped like good ones with a value that is not a plain number are never put back.
    #[test]
    fn non_numbers_are_refused(id in "[A-Za-z0-9_]{1,16}", zone in 0..ZONES.len(), v in "\\PC{0,12}") {
        prop_assume!(!v.bytes().all(|c| c.is_ascii_digit() || c == b'.') || v.is_empty() || v.matches('.').count() > 1 || v == ".");
        let (z, _, set) = ZONES[zone];
        prop_assert_eq!(parse_line(&format!("razer {id} {z} {set} {v}")), None);
        prop_assert_eq!(parse_line(&format!("leds {id}_kbd_backlight - - {v}")), None);
    }

    /// Huge values are never put back: above 100 for a Razer zone, beyond a 32-bit number for a backlight.
    #[test]
    fn huge_values_are_refused(id in "[A-Za-z0-9_]{1,16}", zone in 0..ZONES.len(), v in 100.000001f64..1e300, digits in "[1-9][0-9]{10,40}") {
        let (z, _, set) = ZONES[zone];
        prop_assert_eq!(parse_line(&format!("razer {id} {z} {set} {v}")), None);
        prop_assert_eq!(parse_line(&format!("razer {id} {z} {set} {digits}")), None);
        prop_assert_eq!(parse_line(&format!("leds {id}_kbd_backlight - - {digits}")), None);
    }

    /// Names that could leave the device's folder or object path are refused.
    #[test]
    fn bad_names_are_refused(name in "\\PC{0,20}[/$;`\\\\]\\PC{0,20}", zone in 0..ZONES.len()) {
        let (z, _, set) = ZONES[zone];
        prop_assert_eq!(parse_line(&format!("razer {name} {z} {set} 5")), None);
        prop_assert_eq!(parse_line(&format!("leds {name}kbd_backlight - - 5")), None);
    }

    /// What this module writes, it reads back the same.
    #[test]
    fn written_lines_read_back(e in entry()) {
        prop_assert_eq!(parse_line(&e.line()), Some(e));
    }
}

// ---- the measurements (by hand: packages/idle/README.md) ----

/// The made-up daemons, in a process of their own on a private bus (`measure` starts this test binary again for it).
#[test]
#[ignore]
fn fake_daemons_process() {
    let (Ok(addr), Ok(leds)) = (std::env::var("YGG_TEST_FAKE_BUS"), std::env::var("YGG_TEST_FAKE_LEDS")) else { return };
    let w = Arc::new(Mutex::new(Shared::new(&desk(), Path::new(&leds))));
    let conn = zbus::blocking::connection::Builder::address(addr.as_str()).unwrap().build().unwrap();
    conn.request_name(RAZER).unwrap();
    conn.request_name(LOGIN).unwrap();
    serve(conn, w);
    println!("ready");
    std::thread::sleep(Duration::from_secs(3600));
}

fn percentile(v: &mut [f64], p: f64) -> f64 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn rusage(who: i32) -> f64 {
    // SAFETY: getrusage fills the struct it is given
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(who, &mut u) };
    let t = |tv: libc::timeval| tv.tv_sec as f64 * 1000.0 + tv.tv_usec as f64 / 1000.0;
    t(u.ru_utime) + t(u.ru_stime)
}

/// Times the old hook (the real busctl) and this module against the made-up daemons on a private bus with no service files
/// (nothing can be started on it), and counts the processes the hook starts. Never near the real buses: the private bus's
/// address is given to both, and before anything runs it is checked that no org.razer answers there.
#[test]
#[ignore]
fn measure() {
    use std::io::BufRead;
    let s = Scratch::new();
    let leds = s.0.join("leds");
    let xdg = s.0.join("xdg-state");
    fs::create_dir_all(xdg.join("yggdrasil")).unwrap();
    fs::create_dir_all(&leds).unwrap();
    make_leds(&leds, &desk());
    let conf = s.0.join("bus.conf");
    fs::write(
        &conf,
        format!(
            "<!DOCTYPE busconfig PUBLIC \"-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN\" \
             \"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd\">\n<busconfig><type>session</type>\
             <listen>unix:path={}/bus</listen><auth>EXTERNAL</auth><policy context=\"default\">\
             <allow send_destination=\"*\" eavesdrop=\"true\"/><allow eavesdrop=\"true\"/><allow own=\"*\"/></policy></busconfig>\n",
            s.0.display()
        ),
    )
    .unwrap();
    let mut bus = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", conf.display()))
        .args(["--nofork", "--print-address=1"])
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut addr = String::new();
    std::io::BufReader::new(bus.stdout.take().unwrap()).read_line(&mut addr).unwrap();
    let addr = addr.trim().to_string();
    let env = |c: &mut Command| {
        c.env("DBUS_SESSION_BUS_ADDRESS", &addr)
            .env("DBUS_SYSTEM_BUS_ADDRESS", &addr)
            .env("XDG_STATE_HOME", &xdg)
            .env("HOME", &s.0)
            .env("LC_ALL", "C");
    };
    let mut probe = Command::new("busctl");
    probe.args(["--user", "status", "org.razer"]).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    env(&mut probe);
    assert!(!probe.status().unwrap().success(), "an org.razer answers before the made-up one: not the private bus");
    let mut fake = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "lights::tests::fake_daemons_process", "--ignored", "--nocapture", "--test-threads=1"])
        .env("YGG_TEST_FAKE_BUS", &addr)
        .env("YGG_TEST_FAKE_LEDS", &leds)
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut out = std::io::BufReader::new(fake.stdout.take().unwrap());
    let mut line = String::new();
    while !line.contains("ready") {
        line.clear();
        assert!(out.read_line(&mut line).unwrap() > 0, "the made-up daemons did not start");
    }
    let hook = s.0.join("keyboard-lights");
    fs::write(&hook, hook_text(&leds)).unwrap();
    let at = Places { state_dir: xdg.join("yggdrasil"), leds_dir: leds.clone() };
    let runs = 30;
    let (mut sh_off, mut sh_on, mut rs_off, mut rs_on) = (vec![], vec![], vec![], vec![]);
    let (mut sh_cpu_off, mut sh_cpu_on, mut rs_cpu_off, mut rs_cpu_on) = (vec![], vec![], vec![], vec![]);
    let shell = |op: Op| {
        let mut c = Command::new("sh");
        c.arg(&hook).arg(op_arg(op));
        env(&mut c);
        let (t, cpu) = (Instant::now(), rusage(libc::RUSAGE_CHILDREN));
        assert!(c.status().unwrap().success());
        let x = (t.elapsed().as_secs_f64() * 1000.0, rusage(libc::RUSAGE_CHILDREN) - cpu);
        if op == Op::Off {
            // it reached the made-up daemon: the six Razer zones and two backlights are listed
            let listed = fs::read_to_string(at.state_dir.join(STATE)).unwrap_or_default();
            assert_eq!(listed.lines().count(), 8, "the old hook listed {listed:?}");
        }
        x
    };
    let rust = |op: Op| {
        let (t, cpu) = (Instant::now(), rusage(libc::RUSAGE_SELF));
        let r = run(op, &at, &mut Live::at(&addr)).unwrap();
        let x = (t.elapsed().as_secs_f64() * 1000.0, rusage(libc::RUSAGE_SELF) - cpu);
        assert_eq!((r.done, r.failed), (8, 0), "{op:?}");
        x
    };
    // two warm-up rounds, then old and new taking turns
    for i in 0..runs + 2 {
        let order: [bool; 2] = if i % 2 == 0 { [true, false] } else { [false, true] };
        for old in order {
            let (off, on) = if old { (shell(Op::Off), shell(Op::On)) } else { (rust(Op::Off), rust(Op::On)) };
            assert_eq!(leds_now(&leds)["yggtest_kbd_backlight"], "2");
            if i < 2 {
                continue;
            }
            let (a, b, c, d) = if old {
                (&mut sh_off, &mut sh_on, &mut sh_cpu_off, &mut sh_cpu_on)
            } else {
                (&mut rs_off, &mut rs_on, &mut rs_cpu_off, &mut rs_cpu_on)
            };
            a.push(off.0);
            b.push(on.0);
            c.push(off.1);
            d.push(on.1);
        }
    }
    // the processes the hook starts: the last PID given out, before and after (the whole measurement runs in a PID namespace
    // of its own, README; nothing else starts there meanwhile, which the two rounds agreeing shows)
    let last_pid = || -> u32 { fs::read_to_string("/proc/sys/kernel/ns_last_pid").unwrap().trim().parse().unwrap() };
    let count = |op: Op| -> u32 {
        let before = last_pid();
        shell(op);
        last_pid() - before
    };
    let (p_off, p_on) = (count(Op::Off), count(Op::On));
    // and the threads the Rust one starts (a D-Bus connection's own), which take PIDs from the same counter
    let threads = |op: Op| -> u32 {
        let before = last_pid();
        rust(op);
        last_pid() - before
    };
    let (t_off, t_on) = (threads(Op::Off), threads(Op::On));
    assert_eq!((p_off, p_on), (count(Op::Off), count(Op::On)), "something else started processes meanwhile");
    let own_ns = std::process::id() < 100;
    let _ = fake.kill();
    let _ = fake.wait();
    let _ = bus.kill();
    let _ = bus.wait();
    let show = |name: &str, wall: &mut Vec<f64>, cpu: &mut Vec<f64>| {
        println!(
            "{name}: wall median {:.2} ms (p10 {:.2}, p90 {:.2}); CPU median {:.2} ms",
            percentile(wall, 0.5),
            percentile(wall, 0.1),
            percentile(wall, 0.9),
            percentile(cpu, 0.5)
        );
    };
    show("old hook, lights-off", &mut sh_off, &mut sh_cpu_off);
    show("Rust, lights-off", &mut rs_off, &mut rs_cpu_off);
    show("old hook, lights-on", &mut sh_on, &mut sh_cpu_on);
    show("Rust, lights-on", &mut rs_on, &mut rs_cpu_on);
    println!(
        "processes started by the old hook: lights-off {p_off}, lights-on {p_on}; by the Rust one: 0 (threads: {t_off}, {t_on}){}",
        if own_ns { "" } else { " (not in a PID namespace of its own: the hook's counts may include others)" }
    );
}
