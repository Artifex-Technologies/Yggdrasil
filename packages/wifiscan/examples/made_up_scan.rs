//! Prints a made-up scan in the fixture format (`yggdrasil_wifiscan::parse_fixture`), for Settings' screenshots
//! (packages/settings/tests/fixtures/wifi.json). Every name, address and device detail here is invented: the addresses are
//! made-up ones (D0:C5 in the middle) after real makers' prefixes, so the maker lookup has something to show, or locally
//! administered ones.
//!
//!     cargo run --example made_up_scan

use yggdrasil_wifiscan::craft::{Beacon, Sec, WpsFields};
use yggdrasil_wifiscan::{to_fixture, Bss};

fn ap(mac: [u8; 6], freq: u32, dbm: i32, seen: u32, assoc: bool, privacy: bool, ies: Vec<u8>) -> Bss {
    Bss {
        bssid: mac,
        freq_mhz: freq,
        signal_mbm: Some(dbm * 100),
        seen_ms_ago: Some(seen),
        associated: assoc,
        capability: if privacy { 0x0411 } else { 0x0401 },
        beacon_interval: 100,
        ies,
        beacon_ies: Vec::new(),
    }
}

fn main() {
    // a three-piece mesh, Wi-Fi 6, WPA2/WPA3; this computer is on the nearest one
    let lantern = |ch: u8, w: u16| Beacon { ssid: b"Lantern Hall", channel: ch, width: w, wps: None, bss_load: Some((7, 51)), ..Beacon::wifi6() }.build();
    let eero = |last: u8| [0x08, 0xf0, 0x1e, 0xd0, 0xc5, last];
    // a library's Wi-Fi 5 router with WPS on, announcing its details
    let birchwood = Beacon {
        ssid: b"Birchwood Library",
        channel: 44,
        width: 80,
        he: false,
        security: Sec::Wpa2,
        rrm: false,
        btm: false,
        mobility_domain: None,
        bss_load: None,
        wps: Some(WpsFields {
            manufacturer: "Example Networks",
            model: "Beacon AC",
            model_number: "EX-750",
            serial: "EXN0042-7781",
            device_name: "Birchwood Router",
            locked: true,
            selected: false,
        }),
        vendors: vec![[0x00, 0x50, 0xf2, 2], [0x00, 0x10, 0x18, 2]],
        ..Beacon::wifi6()
    }
    .build();
    let coffee = Beacon {
        ssid: b"CoffeeCat Guest",
        channel: 11,
        width: 20,
        vht: false,
        he: false,
        security: Sec::OpenWithOweTwin,
        wps: None,
        rrm: false,
        btm: false,
        mobility_domain: None,
        country: None,
        bss_load: Some((14, 160)),
        ..Beacon::wifi6()
    }
    .build();
    let harbor = Beacon {
        ssid: b"Harbor Lights 7",
        channel: 37,
        six_ghz: true,
        width: 320,
        eht: true,
        security: Sec::Wpa3,
        wps: None,
        vendors: vec![[0x00, 0x50, 0xf2, 2], [0x50, 0x6f, 0x9a, 0x16]],
        ..Beacon::wifi6()
    }
    .build();
    let northgate = Beacon { ssid: b"Northgate Office", channel: 100, width: 160, security: Sec::Enterprise, wps: None, ..Beacon::wifi6() }.build();
    let hidden = Beacon { ssid: b"", channel: 149, width: 80, security: Sec::Wpa2, wps: None, ..Beacon::wifi6() }.build();
    let list = vec![
        ap(eero(0x01), 5180, -48, 800, true, true, lantern(36, 80)),
        ap(eero(0x02), 5745, -63, 1400, false, true, lantern(149, 80)),
        ap(eero(0x03), 2412, -71, 1600, false, true, Beacon { channel: 1, width: 20, ..Beacon::wifi6() }.build()),
        ap([0x34, 0xf7, 0x16, 0xd0, 0xc5, 0x10], 5220, -57, 2100, false, true, birchwood),
        ap([0x40, 0x5d, 0x82, 0xd0, 0xc5, 0x20], 2462, -64, 900, false, false, coffee),
        ap([0xf0, 0x9f, 0xc2, 0xd0, 0xc5, 0x30], 6135, -66, 1100, false, true, harbor),
        ap([0x00, 0x26, 0x18, 0xd0, 0xc5, 0x40], 5500, -74, 3000, false, true, northgate),
        ap([0x0a, 0x12, 0x34, 0xd0, 0xc5, 0x50], 5745, -58, 1200, false, true, hidden),
    ];
    print!("{}", to_fixture(&list));
}
