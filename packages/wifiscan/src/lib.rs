//! What nearby Wi-Fi networks broadcast, for Settings > Wi-Fi & Network.
//!
//! The kernel keeps the last scan's results, with every access point's information elements as they came over the air.
//! `nl80211::scan_dump` reads that list (NL80211_CMD_GET_SCAN, a dump any user may ask for): it sends nothing on the radio and
//! starts no scan. `ie` reads the elements (security, Wi-Fi generation, channel width, WPS and the device details it
//! announces, country, 802.11k/v/r), `oui` names the makers from the IEEE list on disk, and this file groups access points by
//! network name and sums each one up.
//!
//! Everything read from the air is hostile input: anyone in range can send any bytes. The parsers are bounds-checked, capped
//! and tested with random and mutated input (tests/hostile.rs).

pub mod craft;
pub mod ie;
pub mod nl80211;
pub mod oui;

use ie::Parsed;

/// One access point as the kernel last saw it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Bss {
    pub bssid: [u8; 6],
    pub freq_mhz: u32,
    /// Signal in 1/100 dBm.
    pub signal_mbm: Option<i32>,
    pub seen_ms_ago: Option<u32>,
    /// This computer is connected through this access point.
    pub associated: bool,
    /// The capability field (bit 4: privacy, i.e. some encryption).
    pub capability: u16,
    pub beacon_interval: u16,
    /// The elements of its last probe response, or of its beacon when it sent none.
    pub ies: Vec<u8>,
    /// The elements of its last beacon, when the kernel kept them apart.
    pub beacon_ies: Vec<u8>,
}

/// "AA:BB:CC:DD:EE:FF".
pub fn mac_text(m: &[u8; 6]) -> String {
    m.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

pub fn parse_mac(s: &str) -> Option<[u8; 6]> {
    let parts: Vec<&str> = s.trim().split([':', '-']).collect();
    if parts.len() != 6 {
        return None;
    }
    let mut m = [0u8; 6];
    for (i, p) in parts.iter().enumerate() {
        if p.len() != 2 {
            return None;
        }
        m[i] = u8::from_str_radix(p, 16).ok()?;
    }
    Some(m)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Band {
    G2_4,
    G5,
    G6,
    G60,
    Other,
}

impl Band {
    pub fn label(self) -> &'static str {
        match self {
            Band::G2_4 => "2.4 GHz",
            Band::G5 => "5 GHz",
            Band::G6 => "6 GHz",
            Band::G60 => "60 GHz",
            Band::Other => "Other band",
        }
    }
}

pub fn band(freq: u32) -> Band {
    match freq {
        2400..=2500 => Band::G2_4,
        // 5935 is 6 GHz channel 2
        5925..=7125 => Band::G6,
        4900..=5924 => Band::G5,
        57000..=71000 => Band::G60,
        _ => Band::Other,
    }
}

/// The channel number of a frequency (MHz), as the standard numbers them.
pub fn channel(freq: u32) -> Option<u32> {
    Some(match freq {
        2484 => 14,
        2407..=2472 => (freq - 2407) / 5,
        5935 => 2,
        5950..=7125 => (freq - 5950) / 5,
        4910..=4980 => (freq - 4000) / 5,
        5000..=5924 => (freq - 5000) / 5,
        58320..=70200 => (freq - 56160) / 2160,
        _ => return None,
    })
}

/// Signal strength as 0-100, the way NetworkManager reckons it (-40 dBm or better is 100, -100 dBm is 0), so the bars here
/// match the bars in the list.
pub fn quality(dbm: i32) -> u8 {
    let v = (dbm.clamp(-100, -40) + 40).unsigned_abs();
    (100 - (100 * v) / 60) as u8
}

/// The Wi-Fi generation an access point offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Generation {
    B,
    A,
    G,
    Wifi4,
    Wifi5,
    Wifi6,
    Wifi6E,
    Wifi7,
}

impl Generation {
    pub fn label(self) -> &'static str {
        match self {
            Generation::B => "802.11b (before Wi-Fi 4)",
            Generation::A => "802.11a (before Wi-Fi 4)",
            Generation::G => "802.11g (before Wi-Fi 4)",
            Generation::Wifi4 => "Wi-Fi 4 (802.11n)",
            Generation::Wifi5 => "Wi-Fi 5 (802.11ac)",
            Generation::Wifi6 => "Wi-Fi 6 (802.11ax)",
            Generation::Wifi6E => "Wi-Fi 6E (802.11ax on 6 GHz)",
            Generation::Wifi7 => "Wi-Fi 7 (802.11be)",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            Generation::B => "802.11b",
            Generation::A => "802.11a",
            Generation::G => "802.11g",
            Generation::Wifi4 => "Wi-Fi 4",
            Generation::Wifi5 => "Wi-Fi 5",
            Generation::Wifi6 => "Wi-Fi 6",
            Generation::Wifi6E => "Wi-Fi 6E",
            Generation::Wifi7 => "Wi-Fi 7",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SecKind {
    Open,
    /// Enhanced Open: encrypted without a password.
    Owe,
    Wep,
    /// WPA version 1 only.
    Wpa,
    WpaWpa2Personal,
    Wpa2Personal,
    Wpa2Wpa3Personal,
    Wpa3Personal,
    WpaWpa2Enterprise,
    Wpa2Enterprise,
    Wpa2Wpa3Enterprise,
    Wpa3Enterprise,
    Wpa3Enterprise192,
    /// An RSN element with no key management known here (FILS, ...).
    Other,
}

impl SecKind {
    pub fn label(self) -> &'static str {
        match self {
            SecKind::Open => "Open: no password, nothing encrypted",
            SecKind::Owe => "Enhanced Open (OWE): encrypted, no password",
            SecKind::Wep => "WEP: broken encryption, readable by anyone nearby",
            SecKind::Wpa => "WPA (the first version; outdated)",
            SecKind::WpaWpa2Personal => "WPA/WPA2-Personal (mixed; the old WPA is outdated)",
            SecKind::Wpa2Personal => "WPA2-Personal (password)",
            SecKind::Wpa2Wpa3Personal => "WPA2/WPA3-Personal (transition mode)",
            SecKind::Wpa3Personal => "WPA3-Personal (SAE)",
            SecKind::WpaWpa2Enterprise => "WPA/WPA2-Enterprise (802.1X; the old WPA is outdated)",
            SecKind::Wpa2Enterprise => "WPA2-Enterprise (802.1X)",
            SecKind::Wpa2Wpa3Enterprise => "WPA2/WPA3-Enterprise (802.1X, transition mode)",
            SecKind::Wpa3Enterprise => "WPA3-Enterprise (802.1X)",
            SecKind::Wpa3Enterprise192 => "WPA3-Enterprise 192-bit (802.1X)",
            SecKind::Other => "Encrypted (a kind not known here)",
        }
    }
    pub fn short(self) -> &'static str {
        match self {
            SecKind::Open => "Open",
            SecKind::Owe => "Enhanced Open",
            SecKind::Wep => "WEP",
            SecKind::Wpa => "WPA",
            SecKind::WpaWpa2Personal => "WPA/WPA2",
            SecKind::Wpa2Personal => "WPA2",
            SecKind::Wpa2Wpa3Personal => "WPA2/WPA3",
            SecKind::Wpa3Personal => "WPA3",
            SecKind::WpaWpa2Enterprise => "WPA/WPA2 Enterprise",
            SecKind::Wpa2Enterprise => "WPA2 Enterprise",
            SecKind::Wpa2Wpa3Enterprise => "WPA2/WPA3 Enterprise",
            SecKind::Wpa3Enterprise | SecKind::Wpa3Enterprise192 => "WPA3 Enterprise",
            SecKind::Other => "Encrypted",
        }
    }
    /// Needs a password or an account (everything but Open and Enhanced Open).
    pub fn secured(self) -> bool {
        !matches!(self, SecKind::Open | SecKind::Owe)
    }
}

/// Management frame protection (PMF, 802.11w).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Pmf {
    Off,
    Optional,
    Required,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Security {
    pub kind: SecKind,
    pub pmf: Pmf,
    /// Key management, by name ("PSK", "SAE", "802.1X" ...).
    pub akms: Vec<String>,
    pub ciphers: Vec<&'static str>,
    pub group: Option<&'static str>,
    /// An open network that names an Enhanced Open twin (OWE transition mode).
    pub owe_twin: bool,
}

const AKM_PSK: [u8; 5] = [2, 4, 6, 19, 20];
const AKM_SAE: [u8; 4] = [8, 9, 24, 25];
const AKM_8021X: [u8; 6] = [1, 3, 5, 13, 22, 23];

pub fn akm_name(s: &ie::Suite) -> String {
    let n = match (s[..3] == ie::OUI_IEEE, s[..3] == ie::OUI_MICROSOFT, s[3]) {
        (true, _, 1) | (_, true, 1) => "802.1X",
        (true, _, 2) | (_, true, 2) => "PSK",
        (true, _, 3) => "FT-802.1X",
        (true, _, 4) => "FT-PSK",
        (true, _, 5) => "802.1X-SHA256",
        (true, _, 6) => "PSK-SHA256",
        (true, _, 8) => "SAE",
        (true, _, 9) => "FT-SAE",
        (true, _, 11) => "802.1X Suite B",
        (true, _, 12) => "802.1X Suite B 192-bit",
        (true, _, 13) => "FT-802.1X-SHA384",
        (true, _, 14) => "FILS-SHA256",
        (true, _, 15) => "FILS-SHA384",
        (true, _, 16) => "FT-FILS-SHA256",
        (true, _, 17) => "FT-FILS-SHA384",
        (true, _, 18) => "OWE",
        (true, _, 19) => "FT-PSK-SHA384",
        (true, _, 20) => "PSK-SHA384",
        (true, _, 21) => "PASN",
        (true, _, 23) => "802.1X-SHA384",
        (true, _, 24) => "SAE (extended key)",
        (true, _, 25) => "FT-SAE (extended key)",
        _ => return format!("{:02X}-{:02X}-{:02X} type {}", s[0], s[1], s[2], s[3]),
    };
    n.to_string()
}

pub fn cipher_name(s: &ie::Suite) -> &'static str {
    if s[..3] != ie::OUI_IEEE && s[..3] != ie::OUI_MICROSOFT {
        return "a vendor's own";
    }
    match s[3] {
        0 => "none",
        1 => "WEP-40",
        2 => "TKIP",
        4 => "CCMP-128 (AES)",
        5 => "WEP-104",
        6 => "BIP-CMAC-128",
        7 => "no group traffic",
        8 => "GCMP-128",
        9 => "GCMP-256",
        10 => "CCMP-256",
        11 => "BIP-GMAC-128",
        12 => "BIP-GMAC-256",
        13 => "BIP-CMAC-256",
        _ => "other",
    }
}

/// What the elements and the privacy bit say about security.
pub fn security(p: &Parsed, capability: u16) -> Security {
    let types = |r: &Option<ie::Rsn>, oui: [u8; 3]| -> Vec<u8> {
        r.as_ref().map(|r| r.akms.iter().filter(|s| s[..3] == oui).map(|s| s[3]).collect()).unwrap_or_default()
    };
    let rsn_akm = types(&p.rsn, ie::OUI_IEEE);
    let wpa_akm = types(&p.wpa, ie::OUI_MICROSOFT);
    let any = |set: &[u8]| rsn_akm.iter().any(|t| set.contains(t));
    let pmf = match p.rsn.as_ref().and_then(|r| r.caps) {
        Some(c) if c & 0x40 != 0 => Pmf::Required,
        Some(c) if c & 0x80 != 0 => Pmf::Optional,
        _ => Pmf::Off,
    };
    let wpa1 = p.wpa.is_some();
    let kind = if p.rsn.is_some() {
        if rsn_akm.contains(&12) {
            SecKind::Wpa3Enterprise192
        } else if rsn_akm.contains(&11) {
            SecKind::Wpa3Enterprise
        } else if any(&AKM_SAE) && any(&AKM_PSK) {
            SecKind::Wpa2Wpa3Personal
        } else if any(&AKM_SAE) {
            SecKind::Wpa3Personal
        } else if any(&AKM_PSK) {
            if wpa1 { SecKind::WpaWpa2Personal } else { SecKind::Wpa2Personal }
        } else if any(&AKM_8021X) {
            match pmf {
                Pmf::Required => SecKind::Wpa3Enterprise,
                Pmf::Optional if rsn_akm.contains(&5) && rsn_akm.contains(&1) => SecKind::Wpa2Wpa3Enterprise,
                _ if wpa1 => SecKind::WpaWpa2Enterprise,
                _ => SecKind::Wpa2Enterprise,
            }
        } else if rsn_akm.contains(&18) {
            SecKind::Owe
        } else if wpa1 && !wpa_akm.is_empty() {
            if wpa_akm.contains(&2) { SecKind::WpaWpa2Personal } else { SecKind::WpaWpa2Enterprise }
        } else {
            SecKind::Other
        }
    } else if wpa1 {
        SecKind::Wpa
    } else if capability & 0x0010 != 0 {
        SecKind::Wep
    } else {
        SecKind::Open
    };
    let mut akms: Vec<String> = Vec::new();
    let mut ciphers: Vec<&'static str> = Vec::new();
    for r in [&p.rsn, &p.wpa].into_iter().flatten() {
        for a in &r.akms {
            let n = akm_name(a);
            if !akms.contains(&n) {
                akms.push(n);
            }
        }
        for c in &r.pairwise {
            let n = cipher_name(c);
            if !ciphers.contains(&n) {
                ciphers.push(n);
            }
        }
    }
    let group = p.rsn.as_ref().or(p.wpa.as_ref()).and_then(|r| r.group.as_ref()).map(cipher_name);
    let owe_twin = kind == SecKind::Open && p.vendors.iter().any(|v| v.oui == ie::OUI_WFA && v.kind == Some(0x1c));
    Security { kind, pmf, akms, ciphers, group, owe_twin }
}

/// One access point with its elements read.
#[derive(Clone, Debug, PartialEq)]
pub struct Ap {
    pub bss: Bss,
    pub info: Parsed,
    /// Its beacon leaves the name out (a hidden network), even if a probe response gave the name.
    pub beacon_hidden: bool,
}

fn hidden_ssid(s: &Option<Vec<u8>>) -> bool {
    s.as_ref().is_none_or(|s| s.iter().all(|&b| b == 0))
}

impl Ap {
    pub fn new(bss: Bss) -> Ap {
        let info = ie::parse(&bss.ies);
        let beacon_hidden = if bss.beacon_ies.is_empty() { hidden_ssid(&info.ssid) } else { hidden_ssid(&ie::parse(&bss.beacon_ies).ssid) };
        Ap { bss, info, beacon_hidden }
    }

    /// The name's bytes; empty when it is not known (hidden and never answered with it).
    pub fn ssid(&self) -> &[u8] {
        match &self.info.ssid {
            Some(s) if !hidden_ssid(&self.info.ssid) => s,
            _ => &[],
        }
    }

    /// The name as shown (cleaned); empty when it is not known.
    pub fn name(&self) -> String {
        ie::clean_text(self.ssid(), 32)
    }

    pub fn signal_dbm(&self) -> Option<i32> {
        self.bss.signal_mbm.map(|m| m / 100)
    }

    pub fn quality(&self) -> u8 {
        self.signal_dbm().map(quality).unwrap_or(0)
    }

    pub fn band(&self) -> Band {
        band(self.bss.freq_mhz)
    }

    pub fn channel(&self) -> Option<u32> {
        channel(self.bss.freq_mhz)
    }

    /// The channel width in MHz, and whether it is 80+80.
    pub fn width(&self) -> (u16, bool) {
        let i = &self.info;
        let two = self.band() == Band::G2_4;
        let mut best = (20u16, false);
        let ops = [i.ht_op, if two { None } else { i.vht_op }, i.he_op, i.eht_op];
        for op in ops.into_iter().flatten() {
            if let Some(w) = op.width {
                if w > best.0 {
                    best = (w, op.split);
                }
            }
        }
        best
    }

    pub fn generation(&self) -> Generation {
        let i = &self.info;
        let b = self.band();
        if i.eht {
            Generation::Wifi7
        } else if i.he {
            if b == Band::G6 { Generation::Wifi6E } else { Generation::Wifi6 }
        } else if i.vht && b != Band::G2_4 {
            Generation::Wifi5
        } else if i.ht {
            Generation::Wifi4
        } else if b != Band::G2_4 {
            Generation::A
        } else if i.max_rate > 22 {
            Generation::G
        } else {
            Generation::B
        }
    }

    pub fn security(&self) -> Security {
        security(&self.info, self.bss.capability)
    }
}

/// The access points that share one name (several: a mesh, or a router and its extenders). A hidden access point whose name
/// is not known is a network of its own.
#[derive(Clone, Debug, PartialEq)]
pub struct Network {
    pub ssid: Vec<u8>,
    /// Connected one first, then the strongest.
    pub aps: Vec<Ap>,
}

impl Network {
    pub fn name(&self) -> String {
        ie::clean_text(&self.ssid, 32)
    }
    pub fn hidden(&self) -> bool {
        self.ssid.is_empty()
    }
    /// The access point to describe the network by: the connected one, else the strongest.
    pub fn best(&self) -> &Ap {
        &self.aps[0]
    }
    pub fn connected(&self) -> bool {
        self.aps.iter().any(|a| a.bss.associated)
    }
    /// Not every access point with this name uses the same security (one could be an impostor, or a guest radio).
    pub fn mixed_security(&self) -> bool {
        let first = self.aps[0].security().kind;
        self.aps.iter().any(|a| a.security().kind != first)
    }
    /// The highest generation any of its access points offers.
    pub fn generation(&self) -> Generation {
        self.aps.iter().map(Ap::generation).max().unwrap_or(Generation::B)
    }
}

/// Groups access points into networks by name: the connected network first, then by the strongest signal. At most `MAX_BSS`.
pub fn group(list: Vec<Bss>) -> Vec<Network> {
    let mut seen = std::collections::HashSet::new();
    let mut nets: Vec<Network> = Vec::new();
    for bss in list.into_iter().take(MAX_BSS) {
        if !seen.insert(bss.bssid) {
            continue;
        }
        let ap = Ap::new(bss);
        let key = ap.ssid().to_vec();
        match nets.iter_mut().find(|n| !key.is_empty() && n.ssid == key) {
            Some(n) => n.aps.push(ap),
            None => nets.push(Network { ssid: key, aps: vec![ap] }),
        }
    }
    let rank = |a: &Ap| (a.bss.associated, a.bss.signal_mbm.unwrap_or(i32::MIN));
    for n in &mut nets {
        n.aps.sort_by(|a, b| rank(b).cmp(&rank(a)));
    }
    nets.sort_by(|a, b| rank(&b.aps[0]).cmp(&rank(&a.aps[0])));
    nets
}

/// The most access points read from one scan (a busy street has a hundred or two).
pub const MAX_BSS: usize = 512;

/// A made-up scan as text, one access point a line: `BSSID MHz dBm seen-ms connected(0/1) capability(hex) elements(hex)
/// [beacon-elements(hex)]`. For screenshots and tests (debug builds of Settings read it instead of the radio); lines that do
/// not read are skipped.
pub fn parse_fixture(text: &str) -> Vec<Bss> {
    text.lines()
        .filter(|l| !l.trim_start().starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            if f.len() < 7 {
                return None;
            }
            Some(Bss {
                bssid: parse_mac(f[0])?,
                freq_mhz: f[1].parse().ok()?,
                signal_mbm: f[2].parse::<i32>().ok().map(|d| d * 100),
                seen_ms_ago: f[3].parse().ok(),
                associated: f[4] == "1",
                capability: u16::from_str_radix(f[5], 16).ok()?,
                beacon_interval: 100,
                ies: hex(f[6])?,
                beacon_ies: f.get(7).and_then(|h| hex(h)).unwrap_or_default(),
            })
        })
        .collect()
}

pub fn to_fixture(list: &[Bss]) -> String {
    let h = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    list.iter()
        .map(|b| {
            let mut l = format!(
                "{} {} {} {} {} {:04x} {}",
                mac_text(&b.bssid),
                b.freq_mhz,
                b.signal_mbm.unwrap_or(-10_000) / 100,
                b.seen_ms_ago.unwrap_or(0),
                u8::from(b.associated),
                b.capability,
                h(&b.ies)
            );
            if !b.beacon_ies.is_empty() {
                l.push(' ');
                l.push_str(&h(&b.beacon_ies));
            }
            l + "\n"
        })
        .collect()
}

fn hex(s: &str) -> Option<Vec<u8>> {
    if s.len() % 2 != 0 {
        return None;
    }
    (0..s.len()).step_by(2).map(|i| s.get(i..i + 2).and_then(|p| u8::from_str_radix(p, 16).ok())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use craft::{Beacon, Sec};

    fn bss(mac: u8, freq: u32, dbm: i32, ies: Vec<u8>) -> Bss {
        Bss { bssid: [0x02, 0, 0, 0, 0, mac], freq_mhz: freq, signal_mbm: Some(dbm * 100), capability: 0x0011, ies, ..Default::default() }
    }

    #[test]
    fn channels_and_bands() {
        assert_eq!((channel(2412), channel(2484), channel(5180), channel(5955), channel(5935)), (Some(1), Some(14), Some(36), Some(1), Some(2)));
        assert_eq!((band(2437), band(5745), band(6115)), (Band::G2_4, Band::G5, Band::G6));
        assert_eq!((quality(-40), quality(-70), quality(-100), quality(-20), quality(-120)), (100, 50, 0, 100, 0));
    }

    #[test]
    fn security_kinds() {
        let sec = |s: Sec, cap: u16| security(&ie::parse(&Beacon { security: s, ..Beacon::wifi6() }.build()), cap);
        assert_eq!(sec(Sec::Open, 0x01).kind, SecKind::Open);
        assert_eq!(sec(Sec::Open, 0x11).kind, SecKind::Wep, "privacy bit with no RSN or WPA");
        let t = sec(Sec::Wpa2Wpa3, 0x11);
        assert_eq!((t.kind, t.pmf), (SecKind::Wpa2Wpa3Personal, Pmf::Optional));
        assert_eq!(t.akms, vec!["PSK".to_string(), "SAE".to_string()]);
        let w3 = sec(Sec::Wpa3, 0x11);
        assert_eq!((w3.kind, w3.pmf), (SecKind::Wpa3Personal, Pmf::Required));
        assert_eq!(sec(Sec::Wpa2, 0x11).kind, SecKind::Wpa2Personal);
        assert_eq!(sec(Sec::WpaMixed, 0x11).kind, SecKind::WpaWpa2Personal);
        assert_eq!(sec(Sec::Enterprise, 0x11).kind, SecKind::Wpa2Enterprise);
        assert_eq!(sec(Sec::Wpa3Enterprise, 0x11).kind, SecKind::Wpa3Enterprise);
        assert_eq!(sec(Sec::Owe, 0x11).kind, SecKind::Owe);
        assert!(sec(Sec::OpenWithOweTwin, 0x01).owe_twin);
        assert!(!SecKind::Open.secured() && SecKind::Wpa3Personal.secured());
    }

    #[test]
    fn generations_and_widths() {
        let ap = |b: Beacon, f: u32| Ap::new(bss(1, f, -50, b.build()));
        let six = ap(Beacon { channel: 37, six_ghz: true, width: 160, ..Beacon::wifi6() }, 6135);
        assert_eq!(six.generation(), Generation::Wifi6E);
        assert_eq!(six.width(), (160, false));
        let seven = ap(Beacon { eht: true, six_ghz: true, channel: 37, width: 320, ..Beacon::wifi6() }, 6135);
        assert_eq!((seven.generation(), seven.width().0), (Generation::Wifi7, 320));
        let five = ap(Beacon { he: false, ..Beacon::wifi6() }, 5180);
        assert_eq!((five.generation(), five.width().0), (Generation::Wifi5, 80));
        // VHT on 2.4 GHz (a vendor extra) is still Wi-Fi 4 there, at 20 or 40 MHz
        let two = ap(Beacon { he: false, channel: 6, width: 40, ..Beacon::wifi6() }, 2437);
        assert_eq!((two.generation(), two.width().0), (Generation::Wifi4, 40));
        let g = ap(Beacon { ht: false, vht: false, he: false, channel: 6, ..Beacon::wifi6() }, 2437);
        assert_eq!(g.generation(), Generation::G);
    }

    #[test]
    fn networks_grouped_by_name_connected_first() {
        let mesh = |m: u8, dbm: i32| bss(m, 5180, dbm, Beacon { ssid: b"Lantern Hall", ..Beacon::wifi6() }.build());
        let mut near = mesh(1, -70);
        near.associated = true;
        let other = bss(9, 2437, -40, Beacon { ssid: b"Birchwood", channel: 6, ..Beacon::wifi6() }.build());
        let hidden1 = bss(7, 2412, -60, Beacon { ssid: b"", ..Beacon::wifi6() }.build());
        let hidden2 = bss(8, 2412, -61, Beacon { ssid: &[0, 0, 0], ..Beacon::wifi6() }.build());
        let nets = group(vec![other, mesh(2, -45), hidden1, near.clone(), hidden2, near]);
        assert_eq!(nets.len(), 4, "the mesh is one network; each unnamed access point is its own; a repeat is dropped");
        assert_eq!(nets[0].name(), "Lantern Hall");
        assert!(nets[0].connected());
        assert_eq!(nets[0].aps.len(), 2);
        assert!(nets[0].best().bss.associated, "the connected access point describes the network, not the stronger one");
        assert_eq!(nets[1].name(), "Birchwood");
        assert!(nets[2].hidden() && nets[3].hidden());
    }

    #[test]
    fn a_probe_response_names_a_hidden_network() {
        let mut b = bss(3, 5180, -50, Beacon { ssid: b"Quiet Cove", ..Beacon::wifi6() }.build());
        b.beacon_ies = Beacon { ssid: b"", ..Beacon::wifi6() }.build();
        let ap = Ap::new(b);
        assert_eq!(ap.name(), "Quiet Cove");
        assert!(ap.beacon_hidden);
    }

    #[test]
    fn mixed_security_is_noticed() {
        let a = bss(1, 5180, -50, Beacon { ssid: b"Lantern Hall", ..Beacon::wifi6() }.build());
        let b = bss(2, 5180, -60, Beacon { ssid: b"Lantern Hall", security: Sec::Open, ..Beacon::wifi6() }.build());
        let mut b = b;
        b.capability = 0x01;
        let nets = group(vec![a, b]);
        assert!(nets[0].mixed_security());
    }

    #[test]
    fn fixture_text_round_trip() {
        let mut a = bss(1, 5180, -52, Beacon::wifi6().build());
        a.seen_ms_ago = Some(1200);
        a.associated = true;
        a.beacon_interval = 100;
        let text = to_fixture(std::slice::from_ref(&a));
        assert_eq!(parse_fixture(&text), vec![a]);
        assert!(parse_fixture("nonsense\n02:00:00:00:00:01 5180 x\n").is_empty());
        assert_eq!(parse_mac("02-00-00-00-00-0a"), Some([2, 0, 0, 0, 0, 10]));
        assert_eq!(parse_mac("02:00:00:00:00"), None);
    }
}
