//! Joining a Wi-Fi network with a password: NetworkManager's own D-Bus call (AddAndActivateConnection, what nmcli does inside),
//! so the password travels in a message on the system bus to NetworkManager alone. On nmcli's command line it would be in
//! /proc/PID/cmdline, which every program on the computer can read while nmcli waits for the network.
//!
//! As with nmcli: the strongest access point with that name is chosen, NetworkManager works out the rest (WPA2, WPA3, the
//! mixed modes) from what the access point offers, the caller waits until it is connected, and a new profile that did not
//! connect is deleted again (a mistyped password is not kept). An open network is joined the same way, with no password given.
//!
//! Used by Settings (Wi-Fi & Network) and the installer's Network step.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use zbus::blocking::{proxy, Connection, Proxy};
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ACTIVE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const PROFILE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
/// NM_DEVICE_TYPE_WIFI
const WIFI: u32 = 2;
/// NM_802_11_AP_FLAGS_PRIVACY: an access point that asks for a key (with no WPA flags: WEP)
const PRIVACY: u32 = 0x1;
/// NM_ACTIVE_CONNECTION_STATE_ACTIVATED and _DEACTIVATED
const ACTIVATED: u32 = 2;
const DEACTIVATED: u32 = 4;
/// NM_DEVICE_STATE_REASON_NO_SECRETS and _SUPPLICANT_DISCONNECT: how a refused password ends
const NO_SECRETS: u32 = 7;
const SUPPLICANT_DISCONNECT: u32 = 8;

/// What an access point offers (its flags as NetworkManager reports them).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Offer {
    pub flags: u32,
    pub wpa_flags: u32,
    pub rsn_flags: u32,
}

/// What to join.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Request {
    pub ssid: String,
    /// Nothing for an open network.
    pub password: String,
    /// A network that does not broadcast its name: the profile says so (802-11-wireless.hidden), so NetworkManager asks for it
    /// by name, and it is activated without an access point in hand.
    pub hidden: bool,
    /// For a hidden network out of the scan list: whether it asks for a password (WPA, WPA2 or WPA3 Personal). A network in the
    /// scan list says so itself.
    pub secured: bool,
    /// This computer's real hardware address on this network (cloned-mac-address "permanent"), for networks that reserve or
    /// allow addresses; otherwise the system's default (which may be a private address of its own for each network).
    pub real_address: bool,
}

impl Request {
    /// A network by name, with its password (empty for an open one).
    pub fn new(ssid: &str, password: &str) -> Request {
        Request { ssid: ssid.into(), password: password.into(), ..Request::default() }
    }
}

/// Joins through NetworkManager on the system bus, and waits (up to 90 s) until it is connected.
pub fn join(r: &Request) -> Result<(), String> {
    let bus = Connection::system().map_err(|e| format!("NetworkManager could not be reached: {e}"))?;
    join_on(&bus, r, Duration::from_secs(90), Duration::from_millis(250))
}

fn object<'a>(bus: &'a Connection, path: &'a str, iface: &'a str) -> Result<Proxy<'a>, String> {
    proxy::Builder::new(bus)
        .destination(NM)
        .and_then(|b| b.path(path))
        .and_then(|b| b.interface(iface))
        .map(|b| b.cache_properties(CacheProperties::No))
        .and_then(|b| b.build())
        .map_err(|e| e.to_string())
}

/// The new profile NetworkManager is asked to complete and connect: the name, and the password where the access point wants
/// one, as nmcli gives it (a WPA/WPA2/WPA3 passphrase as the PSK; a WEP key or passphrase as key 0). `offer` None: the access
/// point is not in the scan list (yet), so WPA is assumed.
pub fn settings(ssid: &str, password: &str, offer: Option<Offer>) -> HashMap<&'static str, HashMap<&'static str, Value<'static>>> {
    let mut wifi: HashMap<&'static str, Value<'static>> = HashMap::new();
    wifi.insert("ssid", Value::from(ssid.as_bytes().to_vec()));
    let mut out = HashMap::from([("802-11-wireless", wifi)]);
    let mut sec: HashMap<&'static str, Value<'static>> = HashMap::new();
    match offer {
        Some(o) if o.wpa_flags != 0 || o.rsn_flags != 0 => {
            sec.insert("psk", Value::from(password.to_string()));
        }
        Some(o) if o.flags & PRIVACY != 0 => {
            // NM_WEP_KEY_TYPE_KEY (1) for 5 or 13 characters or 10 or 26 hex digits, else NM_WEP_KEY_TYPE_PASSPHRASE (2)
            let hex = password.chars().all(|c| c.is_ascii_hexdigit());
            let key = matches!(password.len(), 5 | 13) || (hex && matches!(password.len(), 10 | 26));
            sec.insert("key-mgmt", Value::from("none"));
            sec.insert("wep-key0", Value::from(password.to_string()));
            sec.insert("wep-key-type", Value::from(if key { 1u32 } else { 2u32 }));
        }
        Some(_) => {} // an open network: nothing to give
        None => {
            sec.insert("key-mgmt", Value::from("wpa-psk"));
            sec.insert("psk", Value::from(password.to_string()));
        }
    }
    if !sec.is_empty() {
        out.insert("802-11-wireless-security", sec);
    }
    out
}

/// The whole profile for a request: `settings`, and whether the network is hidden and which hardware address it gets.
/// ("assigned-mac-address" is nmcli's cloned-mac-address on D-Bus.)
pub fn profile(r: &Request, offer: Option<Offer>) -> HashMap<&'static str, HashMap<&'static str, Value<'static>>> {
    // a hidden network out of sight: open, or WPA/WPA2/WPA3 Personal as the person said
    let offer = offer.or(if r.hidden && !r.secured { Some(Offer::default()) } else { None });
    let mut out = settings(&r.ssid, &r.password, offer);
    let wifi = out.entry("802-11-wireless").or_default();
    if r.hidden {
        wifi.insert("hidden", Value::from(true));
    }
    if r.real_address {
        wifi.insert("assigned-mac-address", Value::from("permanent"));
    }
    out
}

/// `join` on a given bus, waiting up to `limit` and looking every `every`.
pub fn join_on(bus: &Connection, r: &Request, limit: Duration, every: Duration) -> Result<(), String> {
    let ssid = r.ssid.as_str();
    let nm = object(bus, NM_PATH, NM)?;
    let devices: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).map_err(|e| e.to_string())?;
    // the Wi-Fi adapters, and on them the strongest access point with this name
    let mut first: Option<OwnedObjectPath> = None;
    let mut best: Option<(u8, OwnedObjectPath, OwnedObjectPath, Offer)> = None;
    for d in devices {
        if object(bus, d.as_str(), DEVICE)?.get_property::<u32>("DeviceType").ok() != Some(WIFI) {
            continue;
        }
        first.get_or_insert_with(|| d.clone());
        let aps: Vec<OwnedObjectPath> = object(bus, d.as_str(), WIRELESS)?.call("GetAllAccessPoints", &()).unwrap_or_default();
        for a in aps {
            let ap = object(bus, a.as_str(), ACCESS_POINT)?;
            if ap.get_property::<Vec<u8>>("Ssid").ok().as_deref() != Some(ssid.as_bytes()) {
                continue;
            }
            let strength = ap.get_property::<u8>("Strength").unwrap_or(0);
            if best.as_ref().is_none_or(|b| strength > b.0) {
                let flag = |name: &str| ap.get_property::<u32>(name).unwrap_or(0);
                let offer = Offer { flags: flag("Flags"), wpa_flags: flag("WpaFlags"), rsn_flags: flag("RsnFlags") };
                best = Some((strength, d.clone(), a.clone(), offer));
            }
        }
    }
    let (device, ap, offer) = match best {
        Some((_, d, a, o)) => (d, a, Some(o)),
        None => (first.ok_or("No Wi-Fi adapter found.")?, OwnedObjectPath::from(ObjectPath::from_static_str_unchecked("/")), None),
    };
    let (profile, active): (OwnedObjectPath, OwnedObjectPath) =
        nm.call("AddAndActivateConnection", &(profile(r, offer), &device, &ap)).map_err(|e| e.to_string())?;
    let until = Instant::now() + limit;
    let timed_out = loop {
        // a connection that failed is soon gone from the bus: that is a failure too
        match object(bus, active.as_str(), ACTIVE).and_then(|p| p.get_property::<u32>("State").map_err(|e| e.to_string())) {
            Ok(ACTIVATED) => return Ok(()),
            Ok(DEACTIVATED) | Err(_) => break false,
            Ok(_) => {}
        }
        if Instant::now() >= until {
            break true;
        }
        std::thread::sleep(every);
    };
    let reason = object(bus, device.as_str(), DEVICE).ok().and_then(|p| p.get_property::<(u32, u32)>("StateReason").ok()).map(|r| r.1);
    // as nmcli does: the profile just made is not kept when it did not connect (a mistyped password is not saved)
    if let Ok(p) = object(bus, profile.as_str(), PROFILE) {
        let _: Result<(), _> = p.call("Delete", &());
    }
    Err(if timed_out {
        format!("“{ssid}” did not answer in time.")
    } else if matches!(reason, Some(NO_SECRETS | SUPPLICANT_DISCONNECT)) {
        "The password was not accepted.".to_string()
    } else {
        format!("Could not connect to “{ssid}”.")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_nm::{world, WPA2};
    use zbus::zvariant::OwnedValue;

    fn text(v: &OwnedValue) -> String {
        String::try_from(v.try_clone().unwrap()).unwrap()
    }

    const FAST: Duration = Duration::from_millis(1);

    #[test]
    fn the_password_goes_to_networkmanager_in_a_message() {
        let w = world(vec![1, 1, ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &Request::new("Home", "hunter22"), Duration::from_secs(10), FAST), Ok(()));
        let asked = w.asked.lock().unwrap();
        let (conn, device, ap) = &asked[0];
        assert_eq!((device.as_str(), ap.as_str()), ("/d/wlan", "/ap/2"), "the Wi-Fi adapter, the strongest access point named so");
        assert_eq!(Vec::<u8>::try_from(conn["802-11-wireless"]["ssid"].try_clone().unwrap()).unwrap(), b"Home");
        assert_eq!(text(&conn["802-11-wireless-security"]["psk"]), "hunter22");
        assert!(!conn["802-11-wireless-security"].contains_key("key-mgmt"), "NetworkManager picks WPA2 or WPA3 from the access point");
        assert!(!*w.deleted.lock().unwrap(), "a profile that connected is kept");
    }

    #[test]
    fn with_two_wifi_adapters_the_one_that_hears_the_network_best_joins_it() {
        // heard by both: the USB adapter hears it better
        let w = world(vec![ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &Request::new("Far", "pw"), Duration::from_secs(10), FAST), Ok(()));
        let (_, device, ap) = &w.asked.lock().unwrap()[0];
        assert_eq!((device.as_str(), ap.as_str()), ("/d/usb", "/ap/6"));
        // heard by the USB adapter alone
        let w = world(vec![ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &Request::new("Usb-Only", "pw"), Duration::from_secs(10), FAST), Ok(()));
        let (_, device, ap) = &w.asked.lock().unwrap()[0];
        assert_eq!((device.as_str(), ap.as_str()), ("/d/usb", "/ap/7"));
        // heard by both, the built-in one better
        let w = world(vec![ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &Request::new("Home", "pw"), Duration::from_secs(10), FAST), Ok(()));
        assert_eq!(w.asked.lock().unwrap()[0].1, "/d/wlan");
    }

    #[test]
    fn a_hidden_network_is_asked_for_by_name() {
        let hidden = |secured| Request { ssid: "Quiet".into(), password: "hunter22".into(), hidden: true, secured, ..Request::default() };
        let w = world(vec![1, ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &hidden(true), Duration::from_secs(10), FAST), Ok(()));
        let (conn, device, ap) = &w.asked.lock().unwrap()[0];
        assert_eq!((device.as_str(), ap.as_str()), ("/d/wlan", "/"), "no access point in hand: activated by the profile alone");
        assert!(bool::try_from(conn["802-11-wireless"]["hidden"].try_clone().unwrap()).unwrap());
        assert_eq!(text(&conn["802-11-wireless-security"]["key-mgmt"]), "wpa-psk", "WPA/WPA2/WPA3 Personal");
        assert_eq!(text(&conn["802-11-wireless-security"]["psk"]), "hunter22");
        assert!(!conn["802-11-wireless"].contains_key("assigned-mac-address"), "the system's own address choice");
        let w = world(vec![ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &Request { password: String::new(), ..hidden(false) }, Duration::from_secs(10), FAST), Ok(()));
        let (conn, _, _) = &w.asked.lock().unwrap()[0];
        assert!(!conn.contains_key("802-11-wireless-security"), "an open hidden network: no password");
        // a refused hidden network is not kept either
        let w = world(vec![1, DEACTIVATED], NO_SECRETS, WPA2);
        assert_eq!(join_on(&w.bus, &hidden(true), Duration::from_secs(10), FAST), Err("The password was not accepted.".into()));
        assert!(*w.deleted.lock().unwrap());
    }

    #[test]
    fn the_real_address_for_networks_that_reserve_one() {
        let w = world(vec![ACTIVATED], 0, WPA2);
        let r = Request { real_address: true, ..Request::new("Home", "hunter22") };
        assert_eq!(join_on(&w.bus, &r, Duration::from_secs(10), FAST), Ok(()));
        let (conn, _, _) = &w.asked.lock().unwrap()[0];
        assert_eq!(text(&conn["802-11-wireless"]["assigned-mac-address"]), "permanent");
        assert!(!conn["802-11-wireless"].contains_key("hidden"));
        let p = profile(&Request { real_address: true, hidden: true, ..Request::new("Quiet", "") }, None);
        assert_eq!(p["802-11-wireless"]["assigned-mac-address"], Value::from("permanent"));
        assert_eq!(p["802-11-wireless"]["hidden"], Value::from(true));
    }

    #[test]
    fn a_refused_password_is_not_kept() {
        let w = world(vec![1, DEACTIVATED], NO_SECRETS, WPA2);
        assert_eq!(
            join_on(&w.bus, &Request::new("Home", "wrong"), Duration::from_secs(10), FAST),
            Err("The password was not accepted.".into())
        );
        assert!(*w.deleted.lock().unwrap());
        let w = world(vec![1], 0, WPA2);
        assert_eq!(
            join_on(&w.bus, &Request::new("Home", "x"), Duration::from_millis(20), FAST),
            Err("“Home” did not answer in time.".into())
        );
        assert!(*w.deleted.lock().unwrap());
    }

    #[test]
    fn a_network_out_of_sight_and_wep() {
        let w = world(vec![ACTIVATED], 0, WPA2);
        assert_eq!(join_on(&w.bus, &Request::new("Hidden", "hunter22"), Duration::from_secs(10), FAST), Ok(()));
        let (conn, device, ap) = &w.asked.lock().unwrap()[0];
        assert_eq!((device.as_str(), ap.as_str()), ("/d/wlan", "/"));
        assert_eq!(text(&conn["802-11-wireless-security"]["key-mgmt"]), "wpa-psk");
        let w = world(vec![ACTIVATED], 0, Offer { flags: PRIVACY, wpa_flags: 0, rsn_flags: 0 });
        assert_eq!(join_on(&w.bus, &Request::new("Home", "abcde"), Duration::from_secs(10), FAST), Ok(()));
        let (conn, _, _) = &w.asked.lock().unwrap()[0];
        assert_eq!(text(&conn["802-11-wireless-security"]["wep-key0"]), "abcde");
        assert_eq!(u32::try_from(conn["802-11-wireless-security"]["wep-key-type"].try_clone().unwrap()).unwrap(), 1);
    }

    #[test]
    fn what_each_kind_of_network_is_given() {
        let s = |pw: &str, o| settings("N", pw, o);
        assert!(!s("x", Some(Offer::default())).contains_key("802-11-wireless-security"), "open: no password sent");
        let wep = s("a long wep passphrase", Some(Offer { flags: PRIVACY, ..Offer::default() }));
        assert_eq!(wep["802-11-wireless-security"]["wep-key-type"], Value::from(2u32));
        let hex = s("0123456789", Some(Offer { flags: PRIVACY, ..Offer::default() }));
        assert_eq!(hex["802-11-wireless-security"]["wep-key-type"], Value::from(1u32));
        let wpa3 = s("pw", Some(Offer { flags: PRIVACY, wpa_flags: 0, rsn_flags: 0x400 }));
        assert_eq!(wpa3["802-11-wireless-security"]["psk"], Value::from("pw"));
    }
}
