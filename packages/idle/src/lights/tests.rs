//! The lights, tested without a bus, a daemon or a device.
//!
//! The made-up worlds: this module runs against a made-up openrazer daemon and logind on private peer-to-peer D-Bus
//! connections (socket pairs in this process), with a scratch folder standing in for /sys/class/leds, and the calls it makes,
//! the list it writes and the brightness it leaves are checked step by step. Until 2026-11-15 these were a differential
//! against the retired shell hook (packages/idle/tests/fixtures/keyboard-lights.sh, the hook of commit 840fbc7), which went
//! after its week's soak; git keeps it, and this file as it was (f9c76a5), with the timing test `measure`.
//!
//! Then the list's parser on hostile input (property tests): garbage, huge and non-numeric values are never put back.

use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use proptest::prelude::*;

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

#[test]
fn the_desk() {
    let seen = rust_side(&desk(), &[Op::Off, Op::Off, Op::On, Op::On]);
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
fn without_the_daemon() {
    let seen = rust_side(&World { razer_up_after: None, ..desk() }, &[Op::Off, Op::On]);
    assert_eq!(seen[0].state.as_deref(), Some("leds yggtest_b_kbd_backlight - - 1\nleds yggtest_kbd_backlight - - 2\n"));
    assert_eq!(seen[1].leds["yggtest_kbd_backlight"], "2");
}

/// A list an earlier session (or the old hook) left, put back once; the daemon comes up after three looks a second apart.
#[test]
fn a_list_left_over_and_a_late_daemon() {
    let world = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")]), dev("MSE0001", "mouse", &[("brightness", "0"), ("logo", "0")])],
        razer_up_after: Some(3),
        state: Some(
            "razer KBD0001 brightness setBrightness 75\nrazer MSE0001 logo setLogoBrightness 75\nleds yggtest_kbd_backlight - - 2\n",
        ),
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
    };
    let seen = rust_side(&world, &[Op::On, Op::On]);
    assert_eq!(seen[0].calls.iter().filter(|c| *c == "pause").count(), 3);
    assert_eq!(seen[0].razer["KBD0001 brightness"], "75");
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "2");
    assert_eq!(seen[0].state, None);
}

/// The daemon never comes: 30 pauses, the backlight is put back, the list goes.
#[test]
fn the_daemon_never_comes() {
    let world = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")])],
        razer_up_after: None,
        state: Some("razer KBD0001 brightness setBrightness 75\nleds yggtest_kbd_backlight - - 2\n"),
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
    };
    let seen = rust_side(&world, &[Op::On]);
    assert_eq!(seen[0].calls.iter().filter(|c| *c == "pause").count(), 30);
    assert_eq!(seen[0].calls.iter().filter(|c| *c == "look").count(), 31);
    assert_eq!(seen[0].razer["KBD0001 brightness"], "0");
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "2");
}

/// Lines the old hook refused too: not numbers, too few or too many fields, names that are not names. One good line among them.
#[test]
fn a_hostile_list() {
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
    let seen = rust_side(&world, &[Op::On]);
    // only the two good lines reach a daemon (the calls the old hook made on this list too)
    assert_eq!(seen[0].calls, ["look", "set MSE0001 logo setLogoBrightness 20", "set KBD0001 brightness setBrightness 40"]);
    assert_eq!(seen[0].razer["KBD0001 brightness"], "40");
    assert_eq!(seen[0].razer["MSE0001 logo"], "20");
    assert_eq!(seen[0].leds["yggtest_kbd_backlight"], "0");
}

/// Where the Rust one deliberately differs from the old hook (its differential showed the hook's side until 2026-11-15).
#[test]
fn where_the_old_hook_was_wrong() {
    // 1. A laptop backlight is named "vendor::kbd_backlight": the old hook turned it off but refused the name when putting
    //    it back (its check allowed no ':'), so it stayed dark. The Rust one puts it back.
    let laptop = World { devices: vec![], leds: vec![led("yggtest::kbd_backlight", "2", "3")], razer_up_after: None, state: None };
    let seen = rust_side(&laptop, &[Op::Off, Op::On]);
    assert_eq!(seen[0].leds["yggtest::kbd_backlight"], "0", "turned off");
    assert_eq!(seen[1].leds["yggtest::kbd_backlight"], "2", "and put back");
    // 2. Values out of range in a list: the old hook passed any digits on (150 to a Razer zone, 99 to a backlight whose
    //    maximum is 3); the Rust one puts back nothing above 100 or above the light's own maximum.
    let wild = World {
        devices: vec![dev("KBD0001", "keyboard", &[("brightness", "0")])],
        leds: vec![led("yggtest_kbd_backlight", "0", "3")],
        razer_up_after: Some(0),
        state: Some("razer KBD0001 brightness setBrightness 150\nleds yggtest_kbd_backlight - - 99\n"),
    };
    let seen = rust_side(&wild, &[Op::On]);
    assert_eq!((seen[0].razer["KBD0001 brightness"].as_str(), seen[0].leds["yggtest_kbd_backlight"].as_str()), ("0", "0"));
    assert_eq!(seen[0].state, None, "the list is gone either way");
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

/// White space around a value is only the space between fields: "1 " (the input `non_numbers_are_refused` once drew, CI run of
/// 89367ed) is the number 1, and white space inside a value makes it two fields, refused.
#[test]
fn white_space_around_a_number() {
    let (z, _, set) = ZONES[0];
    assert_eq!(parse_line(&format!("razer 0 {z} {set} 1 ")), Some(Entry::Razer { id: "0".into(), zone: 0, value: 1.0 }));
    assert_eq!(parse_line(&format!("razer 0 {z} {set}\u{3000}2\t")), Some(Entry::Razer { id: "0".into(), zone: 0, value: 2.0 }));
    assert_eq!(parse_line("leds a_kbd_backlight - - 3 "), Some(Entry::Leds { name: "a_kbd_backlight".into(), value: 3 }));
    assert_eq!(parse_line(&format!("razer 0 {z} {set} 1 2")), None);
    assert_eq!(parse_line("leds a_kbd_backlight - - 1 x"), None);
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

    /// Lines shaped like good ones with a value that is not a plain number are never put back. A line is split at white space,
    /// so the value is what `v` holds between its white space: "1 " is the number 1 (put back, see `white_space_around_a_number`).
    #[test]
    fn non_numbers_are_refused(id in "[A-Za-z0-9_]{1,16}", zone in 0..ZONES.len(), v in "\\PC{0,12}") {
        let t = v.trim();
        prop_assume!(!t.bytes().all(|c| c.is_ascii_digit() || c == b'.') || t.is_empty() || t.matches('.').count() > 1 || t == ".");
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
