//! Information elements: what an access point says about itself in its beacons and probe responses (IEEE 802.11-2020 9.4.2).
//! Anyone in radio range can send these bytes, so they are hostile input: every read is bounds-checked, nothing here panics or
//! loops on any input, every list is capped, and text is cleaned before anyone sees it (`clean_text`).

/// Element IDs read here.
pub mod id {
    pub const SSID: u8 = 0;
    pub const RATES: u8 = 1;
    pub const DS_PARAMS: u8 = 3;
    pub const COUNTRY: u8 = 7;
    pub const BSS_LOAD: u8 = 11;
    pub const HT_CAPS: u8 = 45;
    pub const RSN: u8 = 48;
    pub const EXT_RATES: u8 = 50;
    pub const MOBILITY_DOMAIN: u8 = 54;
    pub const HT_OPERATION: u8 = 61;
    pub const RM_CAPS: u8 = 70;
    pub const EXT_CAPS: u8 = 127;
    pub const VHT_CAPS: u8 = 191;
    pub const VHT_OPERATION: u8 = 192;
    pub const VENDOR: u8 = 221;
    pub const EXTENSION: u8 = 255;
    /// Element ID Extension values (the first byte of an EXTENSION element's body).
    pub const EXT_HE_CAPS: u8 = 35;
    pub const EXT_HE_OPERATION: u8 = 36;
    pub const EXT_EHT_OPERATION: u8 = 106;
    pub const EXT_MULTI_LINK: u8 = 107;
    pub const EXT_EHT_CAPS: u8 = 108;
}

/// The longest lists kept from one access point (a real one sends a handful; a hostile one could send hundreds).
const MAX_SUITES: usize = 16;
const MAX_VENDORS: usize = 32;
/// WPS data may be split over several vendor elements; at most this much is joined.
const MAX_WPS: usize = 2048;

/// Walks the elements of `data`: (element ID, body). Stops at the end, or at an element whose length runs past the end (the
/// rest is not trusted).
pub fn elements(data: &[u8]) -> Elements<'_> {
    Elements { data, truncated: false }
}

pub struct Elements<'a> {
    data: &'a [u8],
    /// The list ended in the middle of an element.
    pub truncated: bool,
}

impl<'a> Iterator for Elements<'a> {
    type Item = (u8, &'a [u8]);
    fn next(&mut self) -> Option<Self::Item> {
        if self.data.is_empty() {
            return None;
        }
        if self.data.len() < 2 {
            self.truncated = true;
            self.data = &[];
            return None;
        }
        let (eid, len) = (self.data[0], self.data[1] as usize);
        let Some(body) = self.data.get(2..2 + len) else {
            self.truncated = true;
            self.data = &[];
            return None;
        };
        self.data = &self.data[2 + len..];
        Some((eid, body))
    }
}

/// A cipher or key-management suite: an OUI and a type.
pub type Suite = [u8; 4];

pub const OUI_IEEE: [u8; 3] = [0x00, 0x0f, 0xac];
pub const OUI_MICROSOFT: [u8; 3] = [0x00, 0x50, 0xf2];
pub const OUI_WFA: [u8; 3] = [0x50, 0x6f, 0x9a];

/// An RSN element (WPA2/WPA3), or the older WPA element (laid out the same after its header).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rsn {
    pub version: u16,
    pub group: Option<Suite>,
    pub pairwise: Vec<Suite>,
    pub akms: Vec<Suite>,
    /// RSN capabilities (bit 6: management frame protection required, bit 7: capable). None on WPA, or when left out.
    pub caps: Option<u16>,
    pub group_mgmt: Option<Suite>,
    /// The element ended in the middle of a field.
    pub malformed: bool,
}

/// Reads an RSN element's body, as far as it goes (every field after the version is optional).
pub fn parse_rsn(b: &[u8]) -> Option<Rsn> {
    let mut r = Reader::new(b);
    let mut out = Rsn { version: r.u16_le()?, ..Default::default() };
    suites_after_version(&mut r, &mut out);
    if out.malformed || r.left() < 2 {
        return Some(out);
    }
    out.caps = r.u16_le();
    // PMKIDs (only in association requests, skipped), then the group management cipher
    if let Some(n) = r.u16_le() {
        if r.skip(n as usize * 16).is_none() {
            out.malformed = true;
            return Some(out);
        }
        if r.left() >= 4 {
            out.group_mgmt = r.suite();
        }
    }
    Some(out)
}

/// Reads the WPA (version 1) vendor element's body after its OUI and type.
pub fn parse_wpa(b: &[u8]) -> Option<Rsn> {
    let mut r = Reader::new(b);
    let mut out = Rsn { version: r.u16_le()?, ..Default::default() };
    suites_after_version(&mut r, &mut out);
    Some(out)
}

fn suites_after_version(r: &mut Reader, out: &mut Rsn) {
    if r.left() == 0 {
        return;
    }
    let Some(g) = r.suite() else {
        out.malformed = true;
        return;
    };
    out.group = Some(g);
    match suite_list(r) {
        Ok(Some(v)) => out.pairwise = v,
        Ok(None) => return,
        Err(()) => {
            out.malformed = true;
            return;
        }
    }
    match suite_list(r) {
        Ok(Some(v)) => out.akms = v,
        Ok(None) => {}
        Err(()) => out.malformed = true,
    }
}

/// A count and that many suites: Ok(None) when the field is left out (nothing left), Err when it runs past the end. A body is
/// at most 255 bytes, so a huge count fails within 64 reads.
fn suite_list(r: &mut Reader) -> Result<Option<Vec<Suite>>, ()> {
    if r.left() == 0 {
        return Ok(None);
    }
    let n = r.u16_le().ok_or(())?;
    let mut v = Vec::new();
    for _ in 0..n {
        let s = r.suite().ok_or(())?;
        if v.len() < MAX_SUITES {
            v.push(s);
        }
    }
    Ok(Some(v))
}

/// The 2.4 GHz HT operation, or the channel width of a VHT, HE (6 GHz) or EHT operation element.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Operation {
    pub primary: Option<u8>,
    /// 20, 40, 80, 160 or 320; None when the element says nothing about it.
    pub width: Option<u16>,
    /// 80+80 (two separate 80 MHz pieces), counted as 160.
    pub split: bool,
}

/// HT operation: the primary channel and whether a second 20 MHz channel is in use.
pub fn parse_ht_operation(b: &[u8]) -> Option<Operation> {
    let (&primary, info) = (b.first()?, b.get(1)?);
    let forty = info & 0x04 != 0 && matches!(info & 0x03, 1 | 3);
    Some(Operation { primary: Some(primary), width: Some(if forty { 40 } else { 20 }), split: false })
}

/// VHT operation (also carried inside HE operation): width 0 leaves it to HT; 1 is 80, or 160 / 80+80 by the second centre.
pub fn vht_width(b: &[u8]) -> Option<Operation> {
    let (&w, &c0, &c1) = (b.first()?, b.get(1)?, b.get(2)?);
    let (width, split) = match w {
        0 => return Some(Operation::default()),
        1 if c1 == 0 => (80, false),
        1 => {
            let d = c1.abs_diff(c0);
            if d == 8 { (160, false) } else if d > 16 { (160, true) } else { (80, false) }
        }
        2 => (160, false),
        3 => (160, true),
        _ => return Some(Operation::default()),
    };
    Some(Operation { primary: None, width: Some(width), split })
}

/// HE operation (the body after its extension ID): its VHT and 6 GHz parts, when present.
pub fn parse_he_operation(b: &[u8]) -> Option<Operation> {
    if b.len() < 6 {
        return None;
    }
    let params = u32::from(b[0]) | u32::from(b[1]) << 8 | u32::from(b[2]) << 16;
    let mut at = 6;
    let mut op = Operation::default();
    if params & (1 << 14) != 0 {
        op = vht_width(b.get(at..at + 3)?)?;
        at += 3;
    }
    if params & (1 << 15) != 0 {
        at += 1;
    }
    if params & (1 << 17) != 0 {
        let six = b.get(at..at + 5)?;
        let (width, split) = match six[1] & 0x03 {
            0 => (20, false),
            1 => (40, false),
            2 => (80, false),
            _ => (160, six[3] != 0 && six[3].abs_diff(six[2]) > 16),
        };
        op = Operation { primary: Some(six[0]), width: Some(width), split };
    }
    Some(op)
}

/// EHT operation (the body after its extension ID): the width, up to 320 MHz, when the operation information is present.
pub fn parse_eht_operation(b: &[u8]) -> Option<Operation> {
    let params = *b.first()?;
    if params & 0x01 == 0 {
        return Some(Operation::default());
    }
    let control = *b.get(5)?;
    let width = match control & 0x07 {
        0 => 20,
        1 => 40,
        2 => 80,
        3 => 160,
        4 => 320,
        _ => return Some(Operation::default()),
    };
    Some(Operation { primary: None, width: Some(width), split: false })
}

/// The Country element: two letters and where the rules apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Country {
    /// Two capital letters, or empty when the bytes were not letters.
    pub code: String,
    pub environment: Environment,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Environment {
    Any,
    Indoor,
    Outdoor,
    /// "X": not a country (an operating class table).
    NonCountry,
    Unknown,
}

pub fn parse_country(b: &[u8]) -> Option<Country> {
    let c = b.get(..2)?;
    let code = if c.iter().all(u8::is_ascii_uppercase) { String::from_utf8_lossy(c).into_owned() } else { String::new() };
    let environment = match b.get(2) {
        Some(b' ') => Environment::Any,
        Some(b'I') => Environment::Indoor,
        Some(b'O') => Environment::Outdoor,
        Some(b'X') => Environment::NonCountry,
        _ => Environment::Unknown,
    };
    Some(Country { code, environment })
}

/// The BSS Load element: how many devices the access point has and how busy its channel is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BssLoad {
    pub stations: u16,
    /// 0-255: the share of time the channel was busy.
    pub utilization: u8,
}

/// A vendor-specific element: whose it is and its type byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Vendor {
    pub oui: [u8; 3],
    pub kind: Option<u8>,
    pub len: usize,
}

/// What Wi-Fi Protected Setup announces.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Wps {
    /// 0x10 is 1.0; Version2 from the Wi-Fi Alliance extension (0x20 is 2.0).
    pub version: Option<u8>,
    pub version2: Option<u8>,
    /// 1: not configured, 2: configured.
    pub state: Option<u8>,
    pub locked: bool,
    /// A pairing window is open right now (someone pressed the button or started a PIN pairing).
    pub selected_registrar: bool,
    /// 0: PIN, 4: push button, ...
    pub password_id: Option<u16>,
    pub config_methods: Option<u16>,
    pub manufacturer: String,
    pub model: String,
    pub model_number: String,
    pub serial: String,
    pub device_name: String,
    /// The primary device type: category and subcategory.
    pub device_type: Option<(u16, u16)>,
    pub uuid: Option<[u8; 16]>,
    /// 1: 2.4 GHz, 2: 5 GHz, 4: 60 GHz (bits).
    pub rf_bands: Option<u8>,
    pub malformed: bool,
}

/// Reads WPS attributes (type, length, value; big-endian) from the joined bodies of its vendor elements.
pub fn parse_wps(b: &[u8]) -> Wps {
    let mut w = Wps::default();
    let mut r = Reader::new(b);
    while r.left() > 0 {
        let (Some(t), Some(n)) = (r.u16_be(), r.u16_be()) else {
            w.malformed = true;
            break;
        };
        let Some(v) = r.take(n as usize) else {
            w.malformed = true;
            break;
        };
        let byte = v.first().copied();
        let word = (v.len() >= 2).then(|| u16::from_be_bytes([v[0], v[1]]));
        match t {
            0x104a => w.version = byte,
            0x1044 => w.state = byte,
            0x1057 => w.locked = byte.is_some_and(|x| x != 0),
            0x1041 => w.selected_registrar = byte.is_some_and(|x| x != 0),
            0x1012 => w.password_id = word,
            0x1008 => w.config_methods = word,
            0x1021 => w.manufacturer = clean_text(v, 64),
            0x1023 => w.model = clean_text(v, 32),
            0x1024 => w.model_number = clean_text(v, 32),
            0x1042 => w.serial = clean_text(v, 32),
            0x1011 => w.device_name = clean_text(v, 32),
            0x1054 if v.len() >= 8 => w.device_type = Some((u16::from_be_bytes([v[0], v[1]]), u16::from_be_bytes([v[6], v[7]]))),
            0x1047 if v.len() == 16 => w.uuid = v.try_into().ok(),
            0x103c => w.rf_bands = byte,
            // the Wi-Fi Alliance's extension: sub-elements (ID, length, value); 0 is Version2
            0x1049 if v.len() >= 3 && v[..3] == [0x00, 0x37, 0x2a] => {
                let mut s = Reader::new(&v[3..]);
                while let (Some(id), Some(len)) = (s.u8(), s.u8()) {
                    let Some(val) = s.take(len as usize) else { break };
                    if id == 0 {
                        w.version2 = val.first().copied();
                    }
                }
            }
            _ => {}
        }
    }
    w
}

/// Everything read from one access point's elements.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Parsed {
    /// The SSID element's bytes (None: there was none). Empty or all zeros: a hidden network.
    pub ssid: Option<Vec<u8>>,
    pub ds_channel: Option<u8>,
    pub country: Option<Country>,
    pub rsn: Option<Rsn>,
    /// The older WPA element (Microsoft's vendor element, type 1).
    pub wpa: Option<Rsn>,
    pub ht: bool,
    pub vht: bool,
    pub he: bool,
    pub eht: bool,
    /// Wi-Fi 7 multi-link: one device on several bands at once.
    pub multi_link: bool,
    pub ht_op: Option<Operation>,
    pub vht_op: Option<Operation>,
    pub he_op: Option<Operation>,
    pub eht_op: Option<Operation>,
    /// The fastest basic or supported legacy rate, in 500 kb/s (to tell 802.11b from g).
    pub max_rate: u8,
    pub bss_load: Option<BssLoad>,
    /// Radio Measurement capabilities present (802.11k), and whether neighbour reports are among them.
    pub rrm: bool,
    pub neighbor_report: bool,
    /// Extended Capabilities bit 19: BSS transition management (802.11v).
    pub bss_transition: bool,
    /// The Mobility Domain element (802.11r fast roaming): its ID.
    pub mobility_domain: Option<u16>,
    pub wps: Option<Wps>,
    pub vendors: Vec<Vendor>,
    /// The list ended in the middle of an element (the rest was not read).
    pub truncated: bool,
}

/// Reads every element of `ies`.
pub fn parse(ies: &[u8]) -> Parsed {
    let mut p = Parsed::default();
    let mut wps: Option<Vec<u8>> = None;
    let mut it = elements(ies);
    for (eid, b) in it.by_ref() {
        match eid {
            id::SSID if p.ssid.is_none() => p.ssid = Some(b.iter().take(32).copied().collect()),
            id::RATES | id::EXT_RATES => {
                // rates in 500 kb/s with the "basic" bit; values above 54 Mb/s (108) are membership selectors, not rates
                for &r in b {
                    let r = r & 0x7f;
                    if r <= 108 && r > p.max_rate {
                        p.max_rate = r;
                    }
                }
            }
            id::DS_PARAMS => p.ds_channel = b.first().copied(),
            id::COUNTRY if p.country.is_none() => p.country = parse_country(b),
            id::BSS_LOAD if b.len() >= 3 => p.bss_load = Some(BssLoad { stations: u16::from_le_bytes([b[0], b[1]]), utilization: b[2] }),
            id::HT_CAPS => p.ht = true,
            id::HT_OPERATION => {
                p.ht = true;
                p.ht_op = parse_ht_operation(b);
            }
            id::RSN if p.rsn.is_none() => p.rsn = parse_rsn(b),
            id::MOBILITY_DOMAIN if b.len() >= 2 => p.mobility_domain = Some(u16::from_le_bytes([b[0], b[1]])),
            id::RM_CAPS => {
                p.rrm = true;
                p.neighbor_report = b.first().is_some_and(|x| x & 0x02 != 0);
            }
            id::EXT_CAPS => p.bss_transition = b.get(2).is_some_and(|x| x & 0x08 != 0),
            id::VHT_CAPS => p.vht = true,
            id::VHT_OPERATION => {
                p.vht = true;
                p.vht_op = vht_width(b);
            }
            id::EXTENSION => match b.first() {
                Some(&id::EXT_HE_CAPS) => p.he = true,
                Some(&id::EXT_HE_OPERATION) => {
                    p.he = true;
                    p.he_op = parse_he_operation(&b[1..]);
                }
                Some(&id::EXT_EHT_CAPS) => p.eht = true,
                Some(&id::EXT_EHT_OPERATION) => {
                    p.eht = true;
                    p.eht_op = parse_eht_operation(&b[1..]);
                }
                Some(&id::EXT_MULTI_LINK) => p.multi_link = true,
                _ => {}
            },
            id::VENDOR if b.len() >= 3 => {
                let oui = [b[0], b[1], b[2]];
                let kind = b.get(3).copied();
                if p.vendors.len() < MAX_VENDORS {
                    p.vendors.push(Vendor { oui, kind, len: b.len() });
                }
                match (oui, kind) {
                    (OUI_MICROSOFT, Some(1)) if p.wpa.is_none() => p.wpa = parse_wpa(&b[4..]),
                    (OUI_MICROSOFT, Some(4)) => {
                        let w = wps.get_or_insert_with(Vec::new);
                        let room = MAX_WPS.saturating_sub(w.len());
                        w.extend(b[4..].iter().take(room));
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    p.truncated = it.truncated;
    p.wps = wps.map(|w| parse_wps(&w));
    p
}

/// Text from the air, made safe to show: invalid UTF-8 becomes U+FFFD, control characters, bidirectional overrides and
/// zero-width characters (which can make one name look like another) are dropped, and it is cut to `max` characters.
pub fn clean_text(b: &[u8], max: usize) -> String {
    let s = String::from_utf8_lossy(b);
    let kept: String = s.chars().filter(|c| !invisible(*c)).take(max).collect();
    kept.trim().to_string()
}

/// A character that changes how text looks without being seen: controls, bidi overrides and isolates, zero-width ones.
pub fn invisible(c: char) -> bool {
    c.is_control() || matches!(c as u32, 0x200b..=0x200f | 0x202a..=0x202e | 0x2060..=0x2069 | 0xfeff | 0xad)
}

/// A bounds-checked cursor; every read returns None past the end.
struct Reader<'a> {
    b: &'a [u8],
}

impl<'a> Reader<'a> {
    fn new(b: &'a [u8]) -> Self {
        Reader { b }
    }
    fn left(&self) -> usize {
        self.b.len()
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let v = self.b.get(..n)?;
        self.b = &self.b[n..];
        Some(v)
    }
    fn skip(&mut self, n: usize) -> Option<()> {
        self.take(n).map(|_| ())
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|v| v[0])
    }
    fn u16_le(&mut self) -> Option<u16> {
        self.take(2).map(|v| u16::from_le_bytes([v[0], v[1]]))
    }
    fn u16_be(&mut self) -> Option<u16> {
        self.take(2).map(|v| u16::from_be_bytes([v[0], v[1]]))
    }
    fn suite(&mut self) -> Option<Suite> {
        self.take(4).map(|v| [v[0], v[1], v[2], v[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::craft;

    #[test]
    fn elements_stop_at_a_short_one() {
        let v: Vec<_> = elements(&[0, 3, b'a', b'b', b'c', 3, 1, 6]).collect();
        assert_eq!(v, vec![(0u8, &b"abc"[..]), (3, &[6u8][..])]);
        let mut it = elements(&[0, 9, b'a']);
        assert_eq!(it.next(), None);
        assert!(it.truncated, "a length past the end");
        let mut it = elements(&[0]);
        assert_eq!(it.next(), None);
        assert!(it.truncated);
    }

    #[test]
    fn rsn_personal_transition_with_pmf() {
        let b = craft::rsn_body(&[craft::akm(2), craft::akm(8)], Some(0x0080));
        let r = parse_rsn(&b).unwrap();
        assert_eq!(r.version, 1);
        assert_eq!(r.group, Some([0, 0x0f, 0xac, 4]));
        assert_eq!(r.pairwise, vec![[0, 0x0f, 0xac, 4]]);
        assert_eq!(r.akms, vec![[0, 0x0f, 0xac, 2], [0, 0x0f, 0xac, 8]]);
        assert_eq!(r.caps, Some(0x0080));
        assert!(!r.malformed);
        // cut anywhere: still read as far as it goes, never a panic
        for n in 0..b.len() {
            let _ = parse_rsn(&b[..n]);
        }
        // a count that runs past the end is marked
        let mut bad = b.clone();
        bad[6] = 0xff;
        assert!(parse_rsn(&bad).unwrap().malformed);
    }

    #[test]
    fn widths_from_each_generation() {
        assert_eq!(parse_ht_operation(&[6, 0x05]).unwrap().width, Some(40));
        assert_eq!(parse_ht_operation(&[6, 0x00]).unwrap().width, Some(20));
        assert_eq!(vht_width(&[1, 42, 0]).unwrap().width, Some(80));
        assert_eq!(vht_width(&[1, 42, 50]).unwrap().width, Some(160));
        let split = vht_width(&[1, 42, 155]).unwrap();
        assert_eq!((split.width, split.split), (Some(160), true));
        // HE on 6 GHz: parameters with the 6 GHz part present, then primary 37, width 160
        let he = parse_he_operation(&[0, 0, 0x02, 0, 0, 0, 37, 0x03, 39, 47, 0]).unwrap();
        assert_eq!((he.primary, he.width), (Some(37), Some(160)));
        assert_eq!(parse_eht_operation(&[1, 0, 0, 0, 0, 4, 31, 63]).unwrap().width, Some(320));
        assert_eq!(parse_eht_operation(&[0]).unwrap().width, None);
        assert_eq!(parse_eht_operation(&[1, 0, 0]), None, "cut short");
    }

    #[test]
    fn wps_device_details_and_text_cleaned() {
        let body = craft::wps_body(&craft::WpsFields {
            manufacturer: "Example Networks",
            model: "Beacon X\u{202e}9",
            model_number: "EX-1000",
            serial: "SN0001",
            device_name: "Example Router\u{0007}",
            locked: true,
            selected: false,
        });
        let w = parse_wps(&body);
        assert_eq!(w.state, Some(2));
        assert_eq!(w.manufacturer, "Example Networks");
        assert_eq!(w.model, "Beacon X9", "a right-to-left override is dropped");
        assert_eq!(w.device_name, "Example Router", "a control character is dropped");
        assert_eq!((w.model_number.as_str(), w.serial.as_str()), ("EX-1000", "SN0001"));
        assert!(w.locked);
        assert_eq!(w.version2, Some(0x20));
        assert_eq!(w.device_type, Some((6, 1)));
        assert!(!w.malformed);
        assert!(parse_wps(&[0x10, 0x21, 0xff, 0xff, b'x']).malformed);
    }

    #[test]
    fn a_whole_beacon() {
        let ies = craft::Beacon { ssid: b"Lantern Hall", channel: 36, ..craft::Beacon::wifi6() }.build();
        let p = parse(&ies);
        assert_eq!(p.ssid.as_deref(), Some(&b"Lantern Hall"[..]));
        assert!(p.ht && p.vht && p.he && !p.eht);
        assert_eq!(p.vht_op.unwrap().width, Some(80));
        assert!(p.rrm && p.neighbor_report && p.bss_transition);
        assert_eq!(p.mobility_domain, Some(0x1234));
        assert_eq!(p.country.as_ref().map(|c| c.code.as_str()), Some("US"));
        assert!(p.wps.is_some());
        assert!(!p.truncated);
        assert!(p.vendors.iter().any(|v| v.oui == OUI_MICROSOFT && v.kind == Some(2)), "WMM");
    }

    #[test]
    fn text_from_the_air() {
        assert_eq!(clean_text(b"Home\xe2\x80\x8bNet", 32), "HomeNet", "zero-width space");
        assert_eq!(clean_text(b"\xff\xfeAB", 32), "\u{fffd}\u{fffd}AB");
        assert_eq!(clean_text(&[b'a'; 100], 10).len(), 10);
    }
}
