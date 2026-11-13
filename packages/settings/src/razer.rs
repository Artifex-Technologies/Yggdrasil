//! Razer keyboards, mice and the rest through OpenRazer's daemon on the session bus (`org.razer`, the user's own
//! openrazer-daemon): each device's lighting zones (brightness, effect, colour), and a mouse's DPI, polling rate and battery.
//!
//! What a device offers is read from its introspection (OpenRazer gives each device only the interfaces and methods it has),
//! so a zone or an effect shows only where the device has it: the whole device (`razer.device.lighting.brightness` and
//! `.chroma`), the logo, the scroll wheel, the backlight and the left and right strips (`.logo`, `.scroll`, `.backlight`,
//! `.left`, `.right`, their methods named with the zone: getLogoBrightness, setScrollStatic ...).
//!
//! Every call has a time limit (the connection's method timeout), and all of it runs on a background thread (peripherals.rs):
//! a daemon that hangs holds up nothing. The daemon is never started from here: when it is not running, nothing is asked.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use serde::Deserialize;
use zbus::blocking::{proxy, Connection, Proxy};
use zbus::proxy::CacheProperties;

pub const BUS: &str = "org.razer";
const ROOT: &str = "/org/razer";
/// The longest any one call may take.
pub const TIMEOUT: Duration = Duration::from_secs(2);
/// The polling rates offered when the device does not list its own.
const POLL_RATES: [u32; 3] = [125, 500, 1000];

/// A lighting zone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ZoneKind {
    Main,
    Logo,
    Scroll,
    Backlight,
    Left,
    Right,
}

impl ZoneKind {
    pub const ALL: [ZoneKind; 6] = [ZoneKind::Main, ZoneKind::Logo, ZoneKind::Scroll, ZoneKind::Backlight, ZoneKind::Left, ZoneKind::Right];

    /// The word in the zone's method names (setLogoStatic); none for the whole device (setStatic).
    fn word(self) -> &'static str {
        match self {
            ZoneKind::Main => "",
            ZoneKind::Logo => "Logo",
            ZoneKind::Scroll => "Scroll",
            ZoneKind::Backlight => "Backlight",
            ZoneKind::Left => "Left",
            ZoneKind::Right => "Right",
        }
    }

    /// The interface of its effects.
    fn effects_iface(self) -> &'static str {
        match self {
            ZoneKind::Main => "razer.device.lighting.chroma",
            ZoneKind::Logo => "razer.device.lighting.logo",
            ZoneKind::Scroll => "razer.device.lighting.scroll",
            ZoneKind::Backlight => "razer.device.lighting.backlight",
            ZoneKind::Left => "razer.device.lighting.left",
            ZoneKind::Right => "razer.device.lighting.right",
        }
    }

    /// The interface of its brightness.
    fn brightness_iface(self) -> &'static str {
        match self {
            ZoneKind::Main => "razer.device.lighting.brightness",
            z => z.effects_iface(),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ZoneKind::Main => "Lighting",
            ZoneKind::Logo => "Logo",
            ZoneKind::Scroll => "Scroll wheel",
            ZoneKind::Backlight => "Backlight",
            ZoneKind::Left => "Left side",
            ZoneKind::Right => "Right side",
        }
    }

    fn method(self, what: &str) -> String {
        // getBrightness / getLogoBrightness, setStatic / setLogoStatic, getEffect / getLogoEffect
        let (verb, rest) = what.split_at(3);
        format!("{verb}{}{rest}", self.word())
    }
}

/// The effects offered: those most devices have, as OpenRazer names them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effect {
    Off,
    Static,
    Spectrum,
    Breathing,
    Wave,
}

impl Effect {
    pub const ALL: [Effect; 5] = [Effect::Off, Effect::Static, Effect::Spectrum, Effect::Breathing, Effect::Wave];

    pub fn label(self) -> &'static str {
        match self {
            Effect::Off => "Off",
            Effect::Static => "One colour",
            Effect::Spectrum => "Spectrum",
            Effect::Breathing => "Breathing",
            Effect::Wave => "Wave",
        }
    }

    /// The setter without the zone's word (`ZoneKind::method` adds it).
    fn setter(self) -> &'static str {
        match self {
            Effect::Off => "setNone",
            Effect::Static => "setStatic",
            Effect::Spectrum => "setSpectrum",
            Effect::Breathing => "setBreathSingle",
            Effect::Wave => "setWave",
        }
    }

    /// Takes a colour.
    pub fn coloured(self) -> bool {
        matches!(self, Effect::Static | Effect::Breathing)
    }

    /// OpenRazer's name for the effect running (getEffect); None for one not offered here (reactive, starlight, custom ...).
    fn from_name(name: &str) -> Option<Effect> {
        match name {
            "none" => Some(Effect::Off),
            "static" => Some(Effect::Static),
            "spectrum" => Some(Effect::Spectrum),
            "breathSingle" | "breathDual" | "breathTriple" | "breathRandom" => Some(Effect::Breathing),
            "wave" => Some(Effect::Wave),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Zone {
    pub kind: ZoneKind,
    /// 0 to 100; None when the zone has no brightness of its own.
    #[serde(default)]
    pub brightness: Option<f64>,
    /// What it can do, in the order offered.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// What it does now (None: something not offered here, or not known).
    #[serde(default)]
    pub effect: Option<Effect>,
    #[serde(default)]
    pub color: [u8; 3],
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Dpi {
    pub x: u32,
    pub y: u32,
    pub max: u32,
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Device {
    /// The device's name on the bus (its serial number): used only to address it, never shown.
    pub id: String,
    pub name: String,
    /// keyboard, mouse, mousemat, keypad, headset, ...
    pub kind: String,
    #[serde(default)]
    pub zones: Vec<Zone>,
    #[serde(default)]
    pub dpi: Option<Dpi>,
    /// The polling rate now and the rates offered.
    #[serde(default)]
    pub poll: Option<(u32, Vec<u32>)>,
    /// Percent, and whether it is charging.
    #[serde(default)]
    pub battery: Option<(f64, bool)>,
}

impl Device {
    /// A mouse, or what lies under one (a mouse mat): shown on the Mouse page; keyboards and keypads on the Keyboard page.
    pub fn is_mouse_side(&self) -> bool {
        matches!(self.kind.as_str(), "mouse" | "mousemat" | "mousedock")
    }

    pub fn is_keyboard_side(&self) -> bool {
        matches!(self.kind.as_str(), "keyboard" | "keypad")
    }
}

/// A change asked for.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Brightness(ZoneKind, f64),
    Effect(ZoneKind, Effect, [u8; 3]),
    Dpi(u32),
    Poll(u32),
}

/// A device's name on the bus: what may go into an object path.
fn id_ok(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn path(id: &str) -> String {
    format!("{ROOT}/device/{id}")
}

/// The session bus, every call on it bounded by `TIMEOUT`.
pub fn session() -> Result<Connection, String> {
    zbus::blocking::connection::Builder::session()
        .map(|b| b.method_timeout(TIMEOUT))
        .and_then(|b| b.build())
        .map_err(|e| format!("The session bus could not be reached: {e}"))
}

/// Whether the daemon runs (asked of the bus, so a daemon that is not running is not started by the asking).
pub fn running(bus: &Connection) -> bool {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(bus) else { return false };
    zbus::names::BusName::try_from(BUS).ok().and_then(|n| dbus.name_has_owner(n).ok()).unwrap_or(false)
}

fn object<'a>(bus: &'a Connection, path: &'a str, iface: &'a str) -> Result<Proxy<'a>, String> {
    proxy::Builder::new(bus)
        .destination(BUS)
        .and_then(|b| b.path(path))
        .and_then(|b| b.interface(iface))
        .map(|b| b.cache_properties(CacheProperties::No))
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())
}

fn call<R>(bus: &Connection, id: &str, iface: &str, method: &str) -> Option<R>
where
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    let p = path(id);
    let o = object(bus, &p, iface).ok()?;
    o.call(method, &()).ok()
}

/// The interfaces in an introspection, each with its methods. OpenRazer's introspection is python-dbus's plain XML: the names
/// are read from `<interface name="...">` and `<method name="...">`, nothing else.
pub fn interfaces(xml: &str) -> BTreeMap<String, BTreeSet<String>> {
    let mut out: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let name = |tag: &str| tag.split("name=\"").nth(1).and_then(|r| r.split('"').next()).map(str::to_string);
    for tag in xml.split('<').skip(1) {
        if tag.starts_with("interface ") {
            current = name(tag);
            if let Some(i) = &current {
                out.entry(i.clone()).or_default();
            }
        } else if tag.starts_with("/interface") {
            current = None;
        } else if tag.starts_with("method ") {
            if let (Some(i), Some(m)) = (&current, name(tag)) {
                out.entry(i.clone()).or_default().insert(m);
            }
        }
    }
    out
}

/// Every device the daemon lists, with what each offers and its state now. A device that does not answer is left out.
pub fn list_on(bus: &Connection) -> Result<Vec<Device>, String> {
    let ids: Vec<String> = object(bus, ROOT, "razer.devices")?.call("getDevices", &()).map_err(|e| e.to_string())?;
    Ok(ids.iter().filter(|id| id_ok(id)).filter_map(|id| read_device(bus, id)).collect())
}

fn read_device(bus: &Connection, id: &str) -> Option<Device> {
    let xml: String = call(bus, id, "org.freedesktop.DBus.Introspectable", "Introspect")?;
    let ifaces = interfaces(&xml);
    let has = |iface: &str, method: &str| ifaces.get(iface).is_some_and(|m| m.contains(method));
    let misc = "razer.device.misc";
    let name: String = call(bus, id, misc, "getDeviceName").unwrap_or_else(|| "Razer device".into());
    let kind: String = call(bus, id, misc, "getDeviceType").unwrap_or_default();
    let mut zones = vec![];
    for z in ZoneKind::ALL {
        let (bi, ei) = (z.brightness_iface(), z.effects_iface());
        let brightness = if has(bi, &z.method("getBrightness")) { call::<f64>(bus, id, bi, &z.method("getBrightness")) } else { None };
        let effects: Vec<Effect> = Effect::ALL.into_iter().filter(|e| has(ei, &z.method(e.setter()))).collect();
        if brightness.is_none() && effects.is_empty() {
            continue;
        }
        let effect = if has(ei, &z.method("getEffect")) {
            call::<String>(bus, id, ei, &z.method("getEffect")).and_then(|n| Effect::from_name(&n))
        } else {
            None
        };
        let colors: Vec<u8> = if has(ei, &z.method("getEffectColors")) {
            call(bus, id, ei, &z.method("getEffectColors")).unwrap_or_default()
        } else {
            vec![]
        };
        let color = match colors.as_slice() {
            [r, g, b, ..] => [*r, *g, *b],
            _ => [0, 255, 0],
        };
        zones.push(Zone { kind: z, brightness: brightness.map(|b| b.clamp(0.0, 100.0)), effects, effect, color });
    }
    let dpi = if has("razer.device.dpi", "getDPI") && has("razer.device.dpi", "setDPI") {
        let now: Vec<i32> = call(bus, id, "razer.device.dpi", "getDPI").unwrap_or_default();
        let max: i32 = if has("razer.device.dpi", "maxDPI") { call(bus, id, "razer.device.dpi", "maxDPI").unwrap_or(16000) } else { 16000 };
        match now.as_slice() {
            [x, y, ..] if *x > 0 => Some(Dpi { x: *x as u32, y: (*y).max(1) as u32, max: max.max(*x) as u32 }),
            [x] if *x > 0 => Some(Dpi { x: *x as u32, y: *x as u32, max: max.max(*x) as u32 }),
            _ => None,
        }
    } else {
        None
    };
    let poll = if has(misc, "getPollRate") && has(misc, "setPollRate") {
        let now: Option<i32> = call(bus, id, misc, "getPollRate");
        let mut rates: Vec<u32> = if has(misc, "getSupportedPollRates") {
            call::<Vec<u16>>(bus, id, misc, "getSupportedPollRates").unwrap_or_default().into_iter().map(u32::from).collect()
        } else {
            vec![]
        };
        if rates.is_empty() {
            rates = POLL_RATES.to_vec();
        }
        now.filter(|n| *n > 0).map(|n| {
            if !rates.contains(&(n as u32)) {
                rates.push(n as u32);
                rates.sort_unstable();
            }
            (n as u32, rates)
        })
    } else {
        None
    };
    let battery = if has("razer.device.power", "getBattery") {
        let pct: Option<f64> = call(bus, id, "razer.device.power", "getBattery");
        let charging: bool = if has("razer.device.power", "isCharging") {
            call(bus, id, "razer.device.power", "isCharging").unwrap_or(false)
        } else {
            false
        };
        // 0 is what a wired device without a battery reading says
        pct.filter(|p| *p > 0.0).map(|p| (p.clamp(0.0, 100.0), charging))
    } else {
        None
    };
    Some(Device { id: id.to_string(), name, kind, zones, dpi, poll, battery })
}

/// Makes one change on one device.
pub fn apply_on(bus: &Connection, id: &str, change: &Change) -> Result<(), String> {
    if !id_ok(id) {
        return Err("Not a Razer device's name.".into());
    }
    let p = path(id);
    let r = match change {
        Change::Brightness(z, b) => {
            let b = b.clamp(0.0, 100.0);
            object(bus, &p, z.brightness_iface())?.call::<_, _, ()>(z.method("setBrightness").as_str(), &(b,))
        }
        Change::Effect(z, e, [r, g, b]) => {
            let o = object(bus, &p, z.effects_iface())?;
            let m = z.method(e.setter());
            match e {
                Effect::Static | Effect::Breathing => o.call::<_, _, ()>(m.as_str(), &(*r, *g, *b)),
                // 1: the wave runs left to right
                Effect::Wave => o.call::<_, _, ()>(m.as_str(), &(1i32,)),
                Effect::Off | Effect::Spectrum => o.call::<_, _, ()>(m.as_str(), &()),
            }
        }
        Change::Dpi(d) => {
            let d = (*d).clamp(100, u16::MAX as u32) as u16;
            object(bus, &p, "razer.device.dpi")?.call::<_, _, ()>("setDPI", &(d, d))
        }
        Change::Poll(r) => {
            let r = (*r).clamp(1, u16::MAX as u32) as u16;
            object(bus, &p, "razer.device.misc")?.call::<_, _, ()>("setPollRate", &(r,))
        }
    };
    r.map_err(|e| format!("The device did not take it: {e}"))
}

#[cfg(test)]
mod tests {
    //! A made-up OpenRazer daemon on a private peer-to-peer D-Bus connection (a socket pair in this process): no bus, no real
    //! daemon, no device is touched. A keyboard (the whole device: brightness and five effects) and a mouse (logo and scroll
    //! wheel zones without breathing, DPI, polling rate, battery), as the daemon offers them.
    use super::*;
    use std::sync::{Arc, Mutex};

    type Log = Arc<Mutex<Vec<String>>>;

    struct Devices;

    #[zbus::interface(name = "razer.devices")]
    impl Devices {
        #[zbus(name = "getDevices")]
        fn get_devices(&self) -> Vec<String> {
            vec!["KBD0001".into(), "MSE0001".into(), "bad/../id".into()]
        }
    }

    struct Misc {
        name: &'static str,
        kind: &'static str,
        log: Log,
    }

    #[zbus::interface(name = "razer.device.misc")]
    impl Misc {
        #[zbus(name = "getDeviceName")]
        fn get_device_name(&self) -> String {
            self.name.into()
        }
        #[zbus(name = "getDeviceType")]
        fn get_device_type(&self) -> String {
            self.kind.into()
        }
        #[zbus(name = "getPollRate")]
        fn get_poll_rate(&self) -> i32 {
            500
        }
        #[zbus(name = "setPollRate")]
        fn set_poll_rate(&self, rate: u16) {
            self.log.lock().unwrap().push(format!("setPollRate {rate}"));
        }
    }

    /// The keyboard's own misc: no polling rate.
    struct KbdMisc;

    #[zbus::interface(name = "razer.device.misc")]
    impl KbdMisc {
        #[zbus(name = "getDeviceName")]
        fn get_device_name(&self) -> String {
            "Example Keyboard".into()
        }
        #[zbus(name = "getDeviceType")]
        fn get_device_type(&self) -> String {
            "keyboard".into()
        }
    }

    struct Brightness(Log);

    #[zbus::interface(name = "razer.device.lighting.brightness")]
    impl Brightness {
        #[zbus(name = "getBrightness")]
        fn get_brightness(&self) -> f64 {
            80.0
        }
        #[zbus(name = "setBrightness")]
        fn set_brightness(&self, b: f64) {
            self.0.lock().unwrap().push(format!("setBrightness {b}"));
        }
    }

    struct Chroma(Log);

    #[zbus::interface(name = "razer.device.lighting.chroma")]
    impl Chroma {
        #[zbus(name = "getEffect")]
        fn get_effect(&self) -> String {
            "breathSingle".into()
        }
        #[zbus(name = "getEffectColors")]
        fn get_effect_colors(&self) -> Vec<u8> {
            vec![255, 0, 128, 0, 0, 0, 0, 0, 0]
        }
        #[zbus(name = "setNone")]
        fn set_none(&self) {
            self.0.lock().unwrap().push("setNone".into());
        }
        #[zbus(name = "setStatic")]
        fn set_static(&self, r: u8, g: u8, b: u8) {
            self.0.lock().unwrap().push(format!("setStatic {r} {g} {b}"));
        }
        #[zbus(name = "setSpectrum")]
        fn set_spectrum(&self) {
            self.0.lock().unwrap().push("setSpectrum".into());
        }
        #[zbus(name = "setBreathSingle")]
        fn set_breath_single(&self, r: u8, g: u8, b: u8) {
            self.0.lock().unwrap().push(format!("setBreathSingle {r} {g} {b}"));
        }
        #[zbus(name = "setWave")]
        fn set_wave(&self, d: i32) {
            self.0.lock().unwrap().push(format!("setWave {d}"));
        }
    }

    struct Logo(Log);

    #[zbus::interface(name = "razer.device.lighting.logo")]
    impl Logo {
        #[zbus(name = "getLogoBrightness")]
        fn get_logo_brightness(&self) -> f64 {
            40.0
        }
        #[zbus(name = "setLogoBrightness")]
        fn set_logo_brightness(&self, b: f64) {
            self.0.lock().unwrap().push(format!("setLogoBrightness {b}"));
        }
        #[zbus(name = "getLogoEffect")]
        fn get_logo_effect(&self) -> String {
            "static".into()
        }
        #[zbus(name = "getLogoEffectColors")]
        fn get_logo_effect_colors(&self) -> Vec<u8> {
            vec![0, 200, 255]
        }
        #[zbus(name = "setLogoStatic")]
        fn set_logo_static(&self, r: u8, g: u8, b: u8) {
            self.0.lock().unwrap().push(format!("setLogoStatic {r} {g} {b}"));
        }
        #[zbus(name = "setLogoNone")]
        fn set_logo_none(&self) {
            self.0.lock().unwrap().push("setLogoNone".into());
        }
        #[zbus(name = "setLogoSpectrum")]
        fn set_logo_spectrum(&self) {}
    }

    struct Scroll;

    #[zbus::interface(name = "razer.device.lighting.scroll")]
    impl Scroll {
        #[zbus(name = "getScrollBrightness")]
        fn get_scroll_brightness(&self) -> f64 {
            0.0
        }
        #[zbus(name = "setScrollBrightness")]
        fn set_scroll_brightness(&self, _b: f64) {}
        #[zbus(name = "getScrollEffect")]
        fn get_scroll_effect(&self) -> String {
            "reactive".into()
        }
        #[zbus(name = "setScrollWave")]
        fn set_scroll_wave(&self, _d: i32) {}
    }

    struct DpiIface(Log);

    #[zbus::interface(name = "razer.device.dpi")]
    impl DpiIface {
        #[zbus(name = "getDPI")]
        fn get_dpi(&self) -> Vec<i32> {
            vec![1600, 1600]
        }
        #[zbus(name = "setDPI")]
        fn set_dpi(&self, x: u16, y: u16) {
            self.0.lock().unwrap().push(format!("setDPI {x} {y}"));
        }
        #[zbus(name = "maxDPI")]
        fn max_dpi(&self) -> i32 {
            30000
        }
    }

    struct Power;

    #[zbus::interface(name = "razer.device.power")]
    impl Power {
        #[zbus(name = "getBattery")]
        fn get_battery(&self) -> f64 {
            73.0
        }
        #[zbus(name = "isCharging")]
        fn is_charging(&self) -> bool {
            true
        }
    }

    /// A daemon that never answers in time: its getDevices sleeps longer than the time limit.
    struct Slow;

    #[zbus::interface(name = "razer.devices")]
    impl Slow {
        #[zbus(name = "getDevices")]
        async fn get_devices(&self) -> Vec<String> {
            std::thread::sleep(Duration::from_millis(600));
            vec![]
        }
    }

    struct World {
        bus: Connection,
        _server: Connection,
        log: Log,
    }

    // unix_stream: deprecated only because a build with zbus's tokio feature would lack it, and this one has no tokio
    #[allow(deprecated)]
    fn world() -> World {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let log: Log = Arc::default();
        let l = log.clone();
        let server = std::thread::spawn(move || {
            let kbd = "/org/razer/device/KBD0001";
            let mse = "/org/razer/device/MSE0001";
            zbus::blocking::connection::Builder::unix_stream(a)
                .server(zbus::Guid::generate())
                .unwrap()
                .p2p()
                .serve_at(ROOT, Devices)
                .unwrap()
                .serve_at(kbd, KbdMisc)
                .unwrap()
                .serve_at(kbd, Brightness(l.clone()))
                .unwrap()
                .serve_at(kbd, Chroma(l.clone()))
                .unwrap()
                .serve_at(mse, Misc { name: "Example Mouse", kind: "mouse", log: l.clone() })
                .unwrap()
                .serve_at(mse, Logo(l.clone()))
                .unwrap()
                .serve_at(mse, Scroll)
                .unwrap()
                .serve_at(mse, DpiIface(l.clone()))
                .unwrap()
                .serve_at(mse, Power)
                .unwrap()
                .build()
                .unwrap()
        });
        let bus = zbus::blocking::connection::Builder::unix_stream(b).p2p().method_timeout(TIMEOUT).build().unwrap();
        World { bus, _server: server.join().unwrap(), log }
    }

    #[test]
    fn devices_and_what_each_offers_are_read() {
        let w = world();
        let list = list_on(&w.bus).unwrap();
        assert_eq!(list.len(), 2, "a name that may not go into a path is never asked");
        let kbd = &list[0];
        assert_eq!(
            (kbd.name.as_str(), kbd.kind.as_str(), kbd.dpi.as_ref(), kbd.poll.as_ref()),
            ("Example Keyboard", "keyboard", None, None)
        );
        assert!(kbd.is_keyboard_side());
        assert_eq!(kbd.zones.len(), 1);
        let z = &kbd.zones[0];
        assert_eq!((z.kind, z.brightness, z.effect, z.color), (ZoneKind::Main, Some(80.0), Some(Effect::Breathing), [255, 0, 128]));
        assert_eq!(z.effects, Effect::ALL, "every effect the device has, in order");

        let mouse = &list[1];
        assert!(mouse.is_mouse_side());
        assert_eq!(mouse.dpi, Some(Dpi { x: 1600, y: 1600, max: 30000 }));
        assert_eq!(mouse.poll, Some((500, vec![125, 500, 1000])), "the usual rates when the device lists none");
        assert_eq!(mouse.battery, Some((73.0, true)));
        let kinds: Vec<ZoneKind> = mouse.zones.iter().map(|z| z.kind).collect();
        assert_eq!(kinds, [ZoneKind::Logo, ZoneKind::Scroll]);
        let logo = &mouse.zones[0];
        assert_eq!(logo.effects, [Effect::Off, Effect::Static, Effect::Spectrum], "no breathing where the device has none");
        assert_eq!((logo.effect, logo.color, logo.brightness), (Some(Effect::Static), [0, 200, 255], Some(40.0)));
        let scroll = &mouse.zones[1];
        assert_eq!((scroll.effects.as_slice(), scroll.effect, scroll.brightness), ([Effect::Wave].as_slice(), None, Some(0.0)));
    }

    #[test]
    fn changes_reach_the_right_zone() {
        let w = world();
        apply_on(&w.bus, "KBD0001", &Change::Brightness(ZoneKind::Main, 55.0)).unwrap();
        apply_on(&w.bus, "KBD0001", &Change::Effect(ZoneKind::Main, Effect::Static, [1, 2, 3])).unwrap();
        apply_on(&w.bus, "KBD0001", &Change::Effect(ZoneKind::Main, Effect::Breathing, [4, 5, 6])).unwrap();
        apply_on(&w.bus, "KBD0001", &Change::Effect(ZoneKind::Main, Effect::Wave, [0, 0, 0])).unwrap();
        apply_on(&w.bus, "KBD0001", &Change::Effect(ZoneKind::Main, Effect::Off, [0, 0, 0])).unwrap();
        apply_on(&w.bus, "MSE0001", &Change::Brightness(ZoneKind::Logo, 250.0)).unwrap();
        apply_on(&w.bus, "MSE0001", &Change::Effect(ZoneKind::Logo, Effect::Static, [9, 8, 7])).unwrap();
        apply_on(&w.bus, "MSE0001", &Change::Dpi(3200)).unwrap();
        apply_on(&w.bus, "MSE0001", &Change::Poll(1000)).unwrap();
        assert_eq!(
            *w.log.lock().unwrap(),
            [
                "setBrightness 55",
                "setStatic 1 2 3",
                "setBreathSingle 4 5 6",
                "setWave 1",
                "setNone",
                "setLogoBrightness 100",
                "setLogoStatic 9 8 7",
                "setDPI 3200 3200",
                "setPollRate 1000",
            ]
        );
        assert!(apply_on(&w.bus, "MSE0001", &Change::Effect(ZoneKind::Logo, Effect::Breathing, [1, 1, 1])).is_err(), "not offered");
        assert!(apply_on(&w.bus, "../x", &Change::Dpi(800)).is_err());
    }

    #[test]
    #[allow(deprecated)]
    fn a_daemon_that_hangs_holds_nothing_up() {
        let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            zbus::blocking::connection::Builder::unix_stream(a)
                .server(zbus::Guid::generate())
                .unwrap()
                .p2p()
                .serve_at(ROOT, Slow)
                .unwrap()
                .build()
                .unwrap()
        });
        let bus = zbus::blocking::connection::Builder::unix_stream(b).p2p().method_timeout(Duration::from_millis(150)).build().unwrap();
        let _server = server.join().unwrap();
        let start = std::time::Instant::now();
        assert!(list_on(&bus).is_err());
        assert!(start.elapsed() < Duration::from_millis(550), "the call gave up at its time limit");
    }

    #[test]
    fn introspection_is_read_for_names_only() {
        let xml = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN" "x">
<node name="/org/razer/device/X"><interface name="razer.device.lighting.logo"><method name="setLogoStatic"><arg direction="in" type="y" name="red" /></method>
<method name="getLogoBrightness"><arg direction="out" type="d" /></method></interface>
<interface name="razer.device.misc"><method name="getDeviceName"><arg direction="out" type="s" /></method></interface></node>"#;
        let i = interfaces(xml);
        assert_eq!(i["razer.device.lighting.logo"].iter().collect::<Vec<_>>(), ["getLogoBrightness", "setLogoStatic"]);
        assert!(i["razer.device.misc"].contains("getDeviceName"));
        assert_eq!(ZoneKind::Scroll.method("setStatic"), "setScrollStatic");
        assert_eq!(ZoneKind::Main.method("getBrightness"), "getBrightness");
        assert_eq!(ZoneKind::Logo.method("getEffectColors"), "getLogoEffectColors");
    }
}
