//! A made-up NetworkManager on a private peer-to-peer D-Bus connection (a socket pair in this process), for the tests: no bus,
//! no real NetworkManager, nothing on the computer is changed. The addresses are made up (locally administered, 02:...).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

use crate::join::Offer;

pub const NM_PATH: &str = "/org/freedesktop/NetworkManager";
pub const WIFI: u32 = 2;
pub const ACTIVATED: u32 = 2;

pub type Asked = Arc<Mutex<Vec<(HashMap<String, HashMap<String, OwnedValue>>, String, String)>>>;

struct Nm {
    asked: Asked,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Nm {
    fn get_devices(&self) -> Vec<OwnedObjectPath> {
        ["/d/eth", "/d/wlan", "/d/usb"].iter().map(|p| OwnedObjectPath::try_from(*p).unwrap()).collect()
    }

    fn add_and_activate_connection(
        &self,
        connection: HashMap<String, HashMap<String, OwnedValue>>,
        device: OwnedObjectPath,
        specific_object: OwnedObjectPath,
    ) -> (OwnedObjectPath, OwnedObjectPath) {
        self.asked.lock().unwrap().push((connection, device.to_string(), specific_object.to_string()));
        (OwnedObjectPath::try_from("/s/new").unwrap(), OwnedObjectPath::try_from("/a/new").unwrap())
    }
}

/// An adapter: its type, last state reason, interface name, current address, driver and udev path.
struct Device(u32, (u32, u32), &'static str, &'static str, &'static str, &'static str);

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl Device {
    #[zbus(property)]
    fn device_type(&self) -> u32 {
        self.0
    }
    #[zbus(property)]
    fn state_reason(&self) -> (u32, u32) {
        self.1
    }
    #[zbus(property)]
    fn interface(&self) -> String {
        self.2.into()
    }
    #[zbus(property)]
    fn hw_address(&self) -> String {
        self.3.into()
    }
    #[zbus(property)]
    fn driver(&self) -> String {
        self.4.into()
    }
    #[zbus(property)]
    fn path(&self) -> String {
        self.5.into()
    }
}

/// A Wi-Fi adapter's access points and its permanent address.
struct Wireless(Vec<&'static str>, &'static str);

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl Wireless {
    fn get_all_access_points(&self) -> Vec<OwnedObjectPath> {
        self.0.iter().map(|p| OwnedObjectPath::try_from(*p).unwrap()).collect()
    }
    #[zbus(property)]
    fn perm_hw_address(&self) -> String {
        self.1.into()
    }
}

struct Wired(&'static str);

#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wired")]
impl Wired {
    #[zbus(property)]
    fn perm_hw_address(&self) -> String {
        self.0.into()
    }
}

struct Ap(&'static [u8], u8, Offer);

#[zbus::interface(name = "org.freedesktop.NetworkManager.AccessPoint")]
impl Ap {
    #[zbus(property)]
    fn ssid(&self) -> Vec<u8> {
        self.0.to_vec()
    }
    #[zbus(property)]
    fn strength(&self) -> u8 {
        self.1
    }
    #[zbus(property)]
    fn flags(&self) -> u32 {
        self.2.flags
    }
    #[zbus(property)]
    fn wpa_flags(&self) -> u32 {
        self.2.wpa_flags
    }
    #[zbus(property)]
    fn rsn_flags(&self) -> u32 {
        self.2.rsn_flags
    }
}

struct Active(Arc<Mutex<Vec<u32>>>);

#[zbus::interface(name = "org.freedesktop.NetworkManager.Connection.Active")]
impl Active {
    /// each read gives the next state of the list (the last one stays)
    #[zbus(property)]
    fn state(&self) -> u32 {
        let mut s = self.0.lock().unwrap();
        if s.len() > 1 {
            s.remove(0)
        } else {
            s[0]
        }
    }
}

struct Profile(Arc<Mutex<bool>>);

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl Profile {
    fn delete(&self) {
        *self.0.lock().unwrap() = true;
    }
}

pub const WPA2: Offer = Offer { flags: 1, wpa_flags: 0, rsn_flags: 0x188 };

/// A NetworkManager with a wired adapter and two Wi-Fi adapters (a built-in one, and a USB one plugged in after it). The
/// built-in one hears two access points named "Home" (the second stronger), one "Other" and a weak "Far"; the USB one a
/// weaker "Home", "Far" strongly and "Usb-Only". The new connection goes through `states`, and the adapters' last reason is
/// `reason`.
pub struct World {
    pub bus: Connection,
    _server: Connection,
    pub asked: Asked,
    pub deleted: Arc<Mutex<bool>>,
}

// unix_stream: deprecated only because a build with zbus's tokio feature would lack it, and this one has no tokio
#[allow(deprecated)]
pub fn world(states: Vec<u32>, reason: u32, home: Offer) -> World {
    let (a, b) = std::os::unix::net::UnixStream::pair().unwrap();
    let asked: Asked = Arc::default();
    let deleted = Arc::new(Mutex::new(false));
    let (asked2, deleted2) = (asked.clone(), deleted.clone());
    let server = std::thread::spawn(move || {
        zbus::blocking::connection::Builder::unix_stream(a)
            .server(zbus::Guid::generate())
            .unwrap()
            .p2p()
            .serve_at(NM_PATH, Nm { asked: asked2 })
            .unwrap()
            .serve_at("/d/eth", Device(1, (100, 0), "eth0", "02:00:00:00:00:10", "e1000e", "pci-0000:00:1f.6"))
            .unwrap()
            .serve_at("/d/eth", Wired("02:00:00:00:00:10"))
            .unwrap()
            .serve_at("/d/wlan", Device(WIFI, (30, reason), "wlan0", "02:00:00:00:00:99", "iwlwifi", "pci-0000:00:14.3"))
            .unwrap()
            .serve_at("/d/wlan", Wireless(vec!["/ap/1", "/ap/2", "/ap/3", "/ap/4"], "02:00:00:00:00:01"))
            .unwrap()
            .serve_at("/d/usb", Device(WIFI, (30, reason), "wlan1", "02:00:00:00:00:98", "rtw88_8812au", "pci-0000:00:14.0-usb-0:2:1.0"))
            .unwrap()
            .serve_at("/d/usb", Wireless(vec!["/ap/5", "/ap/6", "/ap/7"], "02:00:00:00:00:02"))
            .unwrap()
            .serve_at("/ap/4", Ap(b"Far", 20, WPA2))
            .unwrap()
            .serve_at("/ap/5", Ap(b"Home", 55, home))
            .unwrap()
            .serve_at("/ap/6", Ap(b"Far", 80, WPA2))
            .unwrap()
            .serve_at("/ap/7", Ap(b"Usb-Only", 50, WPA2))
            .unwrap()
            .serve_at("/ap/1", Ap(b"Home", 40, home))
            .unwrap()
            .serve_at("/ap/2", Ap(b"Home", 70, home))
            .unwrap()
            .serve_at("/ap/3", Ap(b"Other", 90, WPA2))
            .unwrap()
            .serve_at("/a/new", Active(Arc::new(Mutex::new(states))))
            .unwrap()
            .serve_at("/s/new", Profile(deleted2))
            .unwrap()
            .build()
            .unwrap()
    });
    let bus = zbus::blocking::connection::Builder::unix_stream(b).p2p().build().unwrap();
    World { bus, _server: server.join().unwrap(), asked, deleted }
}
