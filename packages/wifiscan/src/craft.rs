//! Elements built from parts, for tests and for made-up scans (Settings' screenshots). Nothing here is sent anywhere; the
//! names used with it are made up.

use crate::ie::{self, Suite};

/// One element: ID, length, body (a body longer than 255 bytes is cut).
pub fn element(eid: u8, body: &[u8]) -> Vec<u8> {
    let body = &body[..body.len().min(255)];
    let mut v = vec![eid, body.len() as u8];
    v.extend_from_slice(body);
    v
}

pub fn ext_element(ext: u8, body: &[u8]) -> Vec<u8> {
    let mut b = vec![ext];
    b.extend_from_slice(body);
    element(ie::id::EXTENSION, &b)
}

pub fn akm(t: u8) -> Suite {
    [0x00, 0x0f, 0xac, t]
}

/// An RSN body: version 1, CCMP group and pairwise ciphers, the key managements given, and the capabilities (with a BIP
/// group management cipher when protection is on).
pub fn rsn_body(akms: &[Suite], caps: Option<u16>) -> Vec<u8> {
    let mut b = vec![1, 0];
    b.extend_from_slice(&[0x00, 0x0f, 0xac, 4]);
    b.extend_from_slice(&[1, 0, 0x00, 0x0f, 0xac, 4]);
    b.extend_from_slice(&(akms.len() as u16).to_le_bytes());
    for a in akms {
        b.extend_from_slice(a);
    }
    if let Some(c) = caps {
        b.extend_from_slice(&c.to_le_bytes());
        if c & 0xc0 != 0 {
            b.extend_from_slice(&[0, 0, 0x00, 0x0f, 0xac, 6]);
        }
    }
    b
}

/// The old WPA vendor element with TKIP and the given key management (1: 802.1X, 2: PSK).
pub fn wpa_element(akm: u8) -> Vec<u8> {
    let mut b = vec![0x00, 0x50, 0xf2, 1, 1, 0, 0x00, 0x50, 0xf2, 2, 1, 0, 0x00, 0x50, 0xf2, 2, 1, 0, 0x00, 0x50, 0xf2];
    b.push(akm);
    element(ie::id::VENDOR, &b)
}

/// What a router's WPS announces.
#[derive(Clone, Copy, Debug)]
pub struct WpsFields<'a> {
    pub manufacturer: &'a str,
    pub model: &'a str,
    pub model_number: &'a str,
    pub serial: &'a str,
    pub device_name: &'a str,
    pub locked: bool,
    pub selected: bool,
}

/// The WPS attributes (without the vendor element's header): version 1.0, configured, the device details, a router's device
/// type, push button and label PIN, and Version2 2.0.
pub fn wps_body(f: &WpsFields) -> Vec<u8> {
    let mut b = Vec::new();
    let mut attr = |t: u16, v: &[u8]| {
        b.extend_from_slice(&t.to_be_bytes());
        b.extend_from_slice(&(v.len() as u16).to_be_bytes());
        b.extend_from_slice(v);
    };
    attr(0x104a, &[0x10]);
    attr(0x1044, &[2]);
    if f.locked {
        attr(0x1057, &[1]);
    }
    if f.selected {
        attr(0x1041, &[1]);
        attr(0x1012, &[0, 4]);
    }
    attr(0x103b, &[3]);
    attr(0x1047, &[0x5a; 16]);
    attr(0x1021, f.manufacturer.as_bytes());
    attr(0x1023, f.model.as_bytes());
    attr(0x1024, f.model_number.as_bytes());
    if !f.serial.is_empty() {
        attr(0x1042, f.serial.as_bytes());
    }
    attr(0x1054, &[0, 6, 0x00, 0x50, 0xf2, 0x04, 0, 1]);
    attr(0x1011, f.device_name.as_bytes());
    attr(0x1008, &[0x02, 0x84]);
    attr(0x103c, &[3]);
    attr(0x1049, &[0x00, 0x37, 0x2a, 0x00, 0x01, 0x20]);
    b
}

/// WPS as vendor elements (split in two when it does not fit one, as routers do).
pub fn wps_elements(f: &WpsFields) -> Vec<u8> {
    let body = wps_body(f);
    let mut out = Vec::new();
    for chunk in body.chunks(251) {
        let mut b = vec![0x00, 0x50, 0xf2, 4];
        b.extend_from_slice(chunk);
        out.extend(element(ie::id::VENDOR, &b));
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sec {
    Open,
    OpenWithOweTwin,
    Owe,
    Wpa2,
    WpaMixed,
    Wpa2Wpa3,
    Wpa3,
    Enterprise,
    Wpa3Enterprise,
}

/// A beacon, described by what is in it.
#[derive(Clone, Debug)]
pub struct Beacon<'a> {
    pub ssid: &'a [u8],
    pub channel: u8,
    /// On 6 GHz (the channel is a 6 GHz one; no VHT there, HE carries the width).
    pub six_ghz: bool,
    /// 20, 40, 80, 160 or 320.
    pub width: u16,
    pub ht: bool,
    pub vht: bool,
    pub he: bool,
    pub eht: bool,
    pub security: Sec,
    pub wps: Option<WpsFields<'a>>,
    pub country: Option<&'a str>,
    pub rrm: bool,
    pub btm: bool,
    pub mobility_domain: Option<u16>,
    pub bss_load: Option<(u16, u8)>,
    /// Extra vendor elements: OUI and type.
    pub vendors: Vec<[u8; 4]>,
}

impl Beacon<'static> {
    /// A Wi-Fi 6 router on 5 GHz channel 36 at 80 MHz, WPA2/WPA3, WPS on and locked, 802.11k/v/r, WMM.
    pub fn wifi6() -> Self {
        Beacon {
            ssid: b"Lantern Hall",
            channel: 36,
            six_ghz: false,
            width: 80,
            ht: true,
            vht: true,
            he: true,
            eht: false,
            security: Sec::Wpa2Wpa3,
            wps: Some(WpsFields {
                manufacturer: "Example Networks",
                model: "Beacon AX",
                model_number: "EX-1000",
                serial: "",
                device_name: "Example Router",
                locked: true,
                selected: false,
            }),
            country: Some("US"),
            rrm: true,
            btm: true,
            mobility_domain: Some(0x1234),
            bss_load: Some((4, 40)),
            vendors: vec![[0x00, 0x50, 0xf2, 2]],
        }
    }
}

impl Beacon<'_> {
    pub fn build(&self) -> Vec<u8> {
        let six = self.six_ghz;
        let two = !six && self.channel <= 14;
        let mut v = element(ie::id::SSID, self.ssid);
        v.extend(element(ie::id::RATES, if two { &[0x82, 0x84, 0x8b, 0x96, 0x0c, 0x12, 0x18, 0x24][..] } else { &[0x8c, 0x12, 0x98, 0x24, 0xb0, 0x48, 0x60, 0x6c][..] }));
        if two {
            v.extend(element(ie::id::DS_PARAMS, &[self.channel]));
        }
        if let Some(c) = self.country {
            let mut b = c.as_bytes().to_vec();
            b.extend_from_slice(b" \x01\x0b\x1e");
            v.extend(element(ie::id::COUNTRY, &b));
        }
        if let Some((n, u)) = self.bss_load {
            let mut b = n.to_le_bytes().to_vec();
            b.extend_from_slice(&[u, 0, 0]);
            v.extend(element(ie::id::BSS_LOAD, &b));
        }
        let ccmp_psk = |a: &[u8], caps: u16| element(ie::id::RSN, &rsn_body(&a.iter().map(|&t| akm(t)).collect::<Vec<_>>(), Some(caps)));
        match self.security {
            Sec::Open => {}
            Sec::OpenWithOweTwin => v.extend(element(ie::id::VENDOR, &[0x50, 0x6f, 0x9a, 0x1c, 0x02, 0, 0, 0, 0, 1, 4, b't', b'w', b'i', b'n'])),
            Sec::Owe => v.extend(ccmp_psk(&[18], 0x00c0)),
            Sec::Wpa2 => v.extend(ccmp_psk(&[2], 0x000c)),
            Sec::WpaMixed => {
                v.extend(ccmp_psk(&[2], 0x000c));
                v.extend(wpa_element(2));
            }
            Sec::Wpa2Wpa3 => v.extend(ccmp_psk(&[2, 8], 0x0080)),
            Sec::Wpa3 => v.extend(ccmp_psk(&[8], 0x00c0)),
            Sec::Enterprise => v.extend(ccmp_psk(&[1], 0x0000)),
            Sec::Wpa3Enterprise => v.extend(ccmp_psk(&[5], 0x00c0)),
        }
        if let Some(md) = self.mobility_domain {
            let mut b = md.to_le_bytes().to_vec();
            b.push(1);
            v.extend(element(ie::id::MOBILITY_DOMAIN, &b));
        }
        if self.rrm {
            v.extend(element(ie::id::RM_CAPS, &[0x73, 0, 0, 0, 0]));
        }
        if self.ht {
            v.extend(element(ie::id::HT_CAPS, &[0xef, 0x09, 0x1b, 0xff, 0xff, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]));
            let info = if self.width >= 40 { 0x05 } else { 0x00 };
            let mut b = vec![self.channel, info];
            b.extend_from_slice(&[0; 20]);
            v.extend(element(ie::id::HT_OPERATION, &b));
        }
        let mut ext = vec![0u8; 3];
        if self.btm {
            ext[2] |= 0x08;
        }
        v.extend(element(ie::id::EXT_CAPS, &ext));
        if self.vht && !six {
            v.extend(element(ie::id::VHT_CAPS, &[0x91, 0x59, 0x82, 0x0f, 0xea, 0xff, 0, 0, 0xea, 0xff, 0, 0]));
            let (w, c0, c1) = match self.width {
                80 => (1, self.channel.saturating_add(6), 0),
                160 => (1, self.channel.saturating_add(6), self.channel.saturating_add(14)),
                _ => (0, 0, 0),
            };
            v.extend(element(ie::id::VHT_OPERATION, &[w, c0, c1, 0xfc, 0xff]));
        }
        if self.he {
            v.extend(ext_element(ie::id::EXT_HE_CAPS, &[0x0d, 0x1a, 0x08, 0, 0, 0, 0x22, 0x20, 0x02, 0xc0, 0x0f, 0x03, 0x95, 0x18, 0, 0xcc, 0, 0xfa, 0xff, 0xfa, 0xff]));
            if six {
                let w = match self.width {
                    20 => 0,
                    40 => 1,
                    80 => 2,
                    _ => 3,
                };
                v.extend(ext_element(ie::id::EXT_HE_OPERATION, &[0, 0, 0x02, 0x01, 0xfc, 0xff, self.channel, w, self.channel.saturating_add(2), self.channel.saturating_add(10), 6]));
            } else {
                v.extend(ext_element(ie::id::EXT_HE_OPERATION, &[0xf4, 0x01, 0x00, 0x01, 0xfc, 0xff]));
            }
        }
        if self.eht {
            v.extend(ext_element(ie::id::EXT_EHT_CAPS, &[0, 0, 0, 0, 0, 0, 0, 0, 0]));
            let w = match self.width {
                20 => 0,
                40 => 1,
                80 => 2,
                160 => 3,
                _ => 4,
            };
            v.extend(ext_element(ie::id::EXT_EHT_OPERATION, &[0x01, 0x44, 0x44, 0x44, 0x44, w, self.channel.saturating_add(14), self.channel.saturating_add(30)]));
        }
        for x in &self.vendors {
            let mut b = x.to_vec();
            b.extend_from_slice(&[1, 0, 0, 0]);
            v.extend(element(ie::id::VENDOR, &b));
        }
        if let Some(w) = &self.wps {
            v.extend(wps_elements(w));
        }
        v
    }
}
