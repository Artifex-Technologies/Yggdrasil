//! Who made an access point, from the first three bytes of its address (the IEEE's list, from the hwdata package, read offline),
//! and what a vendor-specific element is, where it is recognised. The short names and the one-pass lookup follow Heimdall's
//! `internal/lan/oui.go` (the same author's, MPL-2.0).

use std::collections::HashMap;
use std::io::BufRead;

/// The IEEE's list of address prefixes and the companies they belong to (Arch's hwdata package).
pub const OUI_PATH: &str = "/usr/share/hwdata/oui.txt";

/// A locally administered address: made up by the device (a second network on the same router, a phone's hotspot, a private
/// address), so its first bytes name no maker.
pub fn is_local(mac: &[u8; 6]) -> bool {
    mac[0] & 0x02 != 0
}

/// Looks up the makers of the given prefixes in one pass over the list (it is a few megabytes; only wanted ones are kept).
pub fn makers(path: &std::path::Path, wanted: &[[u8; 3]]) -> HashMap<[u8; 3], String> {
    let Ok(f) = std::fs::File::open(path) else { return HashMap::new() };
    makers_from(std::io::BufReader::new(f), wanted)
}

pub fn makers_from(r: impl BufRead, wanted: &[[u8; 3]]) -> HashMap<[u8; 3], String> {
    let mut out = HashMap::new();
    if wanted.is_empty() {
        return out;
    }
    let mut r = r;
    let mut line = Vec::new();
    while out.len() < wanted.len() {
        line.clear();
        match r.read_until(b'\n', &mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        // "28-6F-B9   (hex)\t\tNokia Shanghai Bell Co., Ltd."
        if line.len() < 20 || line[2] != b'-' || line[5] != b'-' {
            continue;
        }
        let Some(at) = line.windows(5).take(20).position(|w| w == b"(hex)") else { continue };
        let Some(prefix) = std::str::from_utf8(&line[..8]).ok().and_then(|p| {
            let b: Vec<u8> = p.split('-').filter_map(|x| u8::from_str_radix(x, 16).ok()).collect();
            <[u8; 3]>::try_from(b).ok()
        }) else {
            continue;
        };
        if wanted.contains(&prefix) && !out.contains_key(&prefix) {
            let name = String::from_utf8_lossy(&line[at + 5..]);
            out.insert(prefix, short_vendor(name.trim()));
        }
    }
    out
}

/// A registered company name as people say it ("ASKEY COMPUTER CORP" -> "Askey Computer", "Intel Corporate" -> "Intel").
pub fn short_vendor(s: &str) -> String {
    let mut s: String = s.trim().chars().filter(|c| !crate::ie::invisible(*c)).take(64).collect();
    if s.to_uppercase() == s && s.chars().count() > 4 {
        s = s
            .to_lowercase()
            .split_whitespace()
            .map(|w| {
                let mut c = w.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" ");
    }
    // company endings, in any case ("eero inc." -> "eero")
    for cut in [
        " co.,ltd",
        " co., ltd",
        ",",
        " inc.",
        " inc",
        " corporation",
        " corporate",
        " corp.",
        " corp",
        " ltd.",
        " ltd",
        " llc",
        " gmbh",
        " limited",
        " technologies",
        " technology",
        " international",
    ] {
        // (a lower-cased copy is searched only when it lines up byte for byte with the name)
        let lower = s.to_lowercase();
        let at = if lower.len() == s.len() { lower.find(cut) } else { s.find(cut) };
        if let Some(i) = at.filter(|&i| i > 0 && s.is_char_boundary(i)) {
            s.truncate(i);
        }
    }
    s.trim().trim_end_matches(['.', ' ']).to_string()
}

/// What a vendor-specific element is, for the ones recognised here (others are named by their maker from the list).
pub fn element_name(oui: [u8; 3], kind: Option<u8>) -> Option<&'static str> {
    Some(match (oui, kind) {
        ([0x00, 0x50, 0xf2], Some(1)) => "WPA (the first version)",
        ([0x00, 0x50, 0xf2], Some(2)) => "WMM (traffic priorities)",
        ([0x00, 0x50, 0xf2], Some(4)) => "WPS (Wi-Fi Protected Setup)",
        ([0x50, 0x6f, 0x9a], Some(0x09)) => "Wi-Fi Direct",
        ([0x50, 0x6f, 0x9a], Some(0x0a)) => "Wi-Fi Display (Miracast)",
        ([0x50, 0x6f, 0x9a], Some(0x10)) => "Hotspot 2.0 (Passpoint)",
        ([0x50, 0x6f, 0x9a], Some(0x12)) => "OSEN (Passpoint sign-up)",
        ([0x50, 0x6f, 0x9a], Some(0x13)) => "Wi-Fi Aware (NAN)",
        ([0x50, 0x6f, 0x9a], Some(0x16)) => "Agile Multiband (MBO/OCE)",
        ([0x50, 0x6f, 0x9a], Some(0x1a)) => "Wi-Fi Easy Connect (DPP)",
        ([0x50, 0x6f, 0x9a], Some(0x1b)) => "EasyMesh (Multi-AP)",
        ([0x50, 0x6f, 0x9a], Some(0x1c)) => "Enhanced Open twin (OWE transition)",
        ([0x50, 0x6f, 0x9a], _) => "Wi-Fi Alliance",
        ([0x00, 0x90, 0x4c], _) => "Broadcom (early 802.11n)",
        ([0x00, 0x10, 0x18], _) => "Broadcom",
        ([0x00, 0x03, 0x7f], _) => "Qualcomm Atheros",
        ([0x8c, 0xfd, 0xf0], _) => "Qualcomm",
        ([0x00, 0x0c, 0xe7], _) => "MediaTek",
        ([0x00, 0x0c, 0x43], _) => "Ralink (MediaTek)",
        ([0x00, 0xe0, 0x4c], _) => "Realtek",
        ([0x00, 0x17, 0xf2], _) => "Apple",
        ([0x00, 0x40, 0x96], _) => "Cisco",
        ([0x00, 0x0b, 0x86], _) => "Aruba",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // a made-up excerpt in the list's layout
    const LIST: &str = "OUI/MA-L\t\t\tOrganization\n\
        02-AA-01   (hex)\t\tEXAMPLE NETWORKS CORP\n\
        02AA01     (base 16)\t\tEXAMPLE NETWORKS CORP\n\
        \t\t\t\tSomewhere\n\n\
        0A-BB-02   (hex)\t\tSample Radio Co.,Ltd\n\
        0A-BB-03   (hex)\t\tNot Wanted Inc.\n";

    #[test]
    fn makers_from_the_list() {
        let m = makers_from(LIST.as_bytes(), &[[0x02, 0xaa, 0x01], [0x0a, 0xbb, 0x02], [0x0c, 0, 0]]);
        assert_eq!(m.len(), 2);
        assert_eq!(m[&[0x02, 0xaa, 0x01]], "Example Networks");
        assert_eq!(m[&[0x0a, 0xbb, 0x02]], "Sample Radio");
        assert!(makers(std::path::Path::new("/nonexistent/oui.txt"), &[[0, 0, 0]]).is_empty());
        assert!(makers_from(&b"\xff\xfe-\x00-garbage (hex) \n"[..], &[[0, 0, 0]]).is_empty());
    }

    #[test]
    fn short_names() {
        assert_eq!(short_vendor("ASKEY COMPUTER CORP"), "Askey Computer");
        assert_eq!(short_vendor("Intel Corporate"), "Intel");
        assert_eq!(short_vendor("Amazon Technologies Inc."), "Amazon");
        assert_eq!(short_vendor("Apple, Inc."), "Apple");
        assert_eq!(short_vendor("eero inc."), "eero");
        assert_eq!(short_vendor("Ubiquiti Inc"), "Ubiquiti");
        assert!(is_local(&[0x02, 0, 0, 0, 0, 1]) && !is_local(&[0x00, 0x11, 0, 0, 0, 1]));
        assert_eq!(element_name([0x00, 0x50, 0xf2], Some(2)), Some("WMM (traffic priorities)"));
        assert_eq!(element_name([0x12, 0x34, 0x56], Some(1)), None);
    }
}
