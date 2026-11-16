//! Asking NetworkManager to look for networks: RequestScan on each Wi-Fi adapter (what `nmcli device wifi rescan` does inside,
//! without starting a program). Settings asks while its Wi-Fi page is on screen. NetworkManager decides whether a scan really
//! runs: it refuses one asked for too soon after the last (about every 10 s) or while it is joining a network, and Settings
//! never asks more often than that anyway.

use std::collections::HashMap;

use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};

use crate::join::{object, DEVICE, NM, NM_PATH, WIFI, WIRELESS};

/// Asks every Wi-Fi adapter on the system bus to scan; how many took the request.
pub fn request() -> Result<usize, String> {
    let bus = Connection::system().map_err(|e| format!("NetworkManager could not be reached: {e}"))?;
    request_on(&bus)
}

/// `request` on a given bus. An adapter that refuses (a scan already running, one just done, a network being joined) is not
/// an error: the scan it already has is read all the same.
pub fn request_on(bus: &Connection) -> Result<usize, String> {
    let devices: Vec<OwnedObjectPath> = object(bus, NM_PATH, NM)?.call("GetDevices", &()).map_err(|e| e.to_string())?;
    let mut took = 0;
    for d in devices {
        if object(bus, d.as_str(), DEVICE)?.get_property::<u32>("DeviceType").ok() != Some(WIFI) {
            continue;
        }
        let options: HashMap<&str, Value> = HashMap::new();
        if object(bus, d.as_str(), WIRELESS)?.call::<_, _, ()>("RequestScan", &(options,)).is_ok() {
            took += 1;
        }
    }
    Ok(took)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake_nm::{world, ACTIVATED, WPA2};

    /// The made-up NetworkManager (no bus, nothing on the computer asked): both Wi-Fi adapters are asked, the wired one is
    /// not; the USB adapter refuses (as NetworkManager does right after a scan), which is not an error.
    #[test]
    fn each_wifi_adapter_is_asked_and_a_refusal_is_not_an_error() {
        let w = world(vec![ACTIVATED], 0, WPA2);
        assert_eq!(request_on(&w.bus), Ok(1), "the built-in adapter took it, the USB one refused");
        assert_eq!(*w.scans.lock().unwrap(), ["/d/wlan", "/d/usb"], "every Wi-Fi adapter asked, the wired one not");
    }
}
