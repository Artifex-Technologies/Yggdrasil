//! What a vendor-specific element is, where it is recognised. Who made an access point, or the company an element belongs to,
//! comes from Heimdall's maker lookup (the crate `heimdall-macvendor`, all four IEEE registries), which Settings uses directly.

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

    #[test]
    fn recognised_elements() {
        assert_eq!(element_name([0x00, 0x50, 0xf2], Some(2)), Some("WMM (traffic priorities)"));
        assert_eq!(element_name([0x50, 0x6f, 0x9a], Some(0x77)), Some("Wi-Fi Alliance"));
        assert_eq!(element_name([0x12, 0x34, 0x56], Some(1)), None);
    }
}
