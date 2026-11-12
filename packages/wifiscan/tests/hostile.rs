//! Information elements come from anyone in radio range, and the netlink messages carry them: every reader here is run on
//! random bytes and on real-looking beacons that were cut, bit-flipped and spliced. None may panic, loop, or let an invisible
//! or control character through to the screen.

use proptest::prelude::*;
use yggdrasil_wifiscan::{craft, group, ie, nl80211, oui, parse_fixture, Ap, Bss};

fn beacons() -> Vec<Vec<u8>> {
    use craft::{Beacon, Sec};
    let mut v = vec![Beacon::wifi6().build()];
    for (s, six, eht) in [
        (Sec::Open, false, false),
        (Sec::Wpa3, true, true),
        (Sec::WpaMixed, false, false),
        (Sec::Enterprise, false, false),
        (Sec::Owe, true, false),
    ] {
        v.push(
            Beacon {
                security: s,
                six_ghz: six,
                eht,
                channel: if six { 37 } else { 6 },
                width: if six { 160 } else { 40 },
                ..Beacon::wifi6()
            }
            .build(),
        );
    }
    v
}

/// Everything a page would ask of an access point.
fn describe(ap: &Ap) {
    let _ = (ap.name(), ap.ssid(), ap.quality(), ap.band(), ap.channel(), ap.width(), ap.generation(), ap.security());
    if let Some(w) = &ap.info.wps {
        for t in [&w.manufacturer, &w.model, &w.model_number, &w.serial, &w.device_name] {
            assert!(!t.chars().any(ie::invisible), "{t:?}");
            assert!(t.chars().count() <= 64);
        }
    }
    assert!(!ap.name().chars().any(ie::invisible));
    assert!(ap.name().chars().count() <= 32);
    assert!(ap.info.vendors.len() <= 32);
    for r in [&ap.info.rsn, &ap.info.wpa].into_iter().flatten() {
        assert!(r.pairwise.len() <= 16 && r.akms.len() <= 16);
    }
}

fn ap(ies: Vec<u8>) -> Ap {
    Ap::new(Bss { bssid: [2, 0, 0, 0, 0, 1], freq_mhz: 5180, signal_mbm: Some(-5000), capability: 0x11, ies, ..Default::default() })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 3000, ..ProptestConfig::default() })]

    #[test]
    fn random_elements_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2400)) {
        describe(&ap(bytes.clone()));
        // the walk ends: never more elements than bytes / 2
        prop_assert!(ie::elements(&bytes).count() <= bytes.len() / 2 + 1);
    }

    #[test]
    fn damaged_beacons_never_panic(pick in 0usize..6, cut in any::<prop::sample::Index>(), flips in proptest::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..12), splice in proptest::collection::vec(any::<u8>(), 0..40)) {
        let mut b = beacons()[pick].clone();
        for (at, x) in &flips {
            let i = at.index(b.len());
            b[i] ^= x;
        }
        let at = cut.index(b.len() + 1);
        let mut spliced = b[..at].to_vec();
        spliced.extend(&splice);
        spliced.extend_from_slice(&b[at..]);
        describe(&ap(b[..at].to_vec()));
        describe(&ap(spliced));
        describe(&ap(b));
    }

    #[test]
    fn each_element_reader_on_random_bodies(body in proptest::collection::vec(any::<u8>(), 0..300)) {
        let _ = ie::parse_rsn(&body);
        let _ = ie::parse_wpa(&body);
        let _ = ie::parse_ht_operation(&body);
        let _ = ie::vht_width(&body);
        let _ = ie::parse_he_operation(&body);
        let _ = ie::parse_eht_operation(&body);
        let _ = ie::parse_country(&body);
        let w = ie::parse_wps(&body);
        prop_assert!(w.manufacturer.chars().count() <= 64 && w.model.chars().count() <= 32);
    }

    #[test]
    fn names_survive_the_round_trip(ssid in proptest::collection::vec(any::<u8>(), 1..33)) {
        let ies = craft::Beacon { ssid: &ssid, ..craft::Beacon::wifi6() }.build();
        let p = ie::parse(&ies);
        prop_assert_eq!(p.ssid.as_deref(), Some(&ssid[..]));
        let a = ap(ies);
        if ssid.iter().any(|&b| b != 0) {
            prop_assert_eq!(a.ssid(), &ssid[..]);
        } else {
            prop_assert!(a.ssid().is_empty(), "all zeros is a hidden name");
        }
    }

    #[test]
    fn wps_text_round_trip(name in "[ -~]{0,32}", maker in "[ -~]{0,64}") {
        let f = craft::WpsFields { manufacturer: &maker, model: "M", model_number: "1", serial: "", device_name: &name, locked: false, selected: true };
        let w = ie::parse_wps(&craft::wps_body(&f));
        prop_assert_eq!(w.device_name, name.trim());
        prop_assert_eq!(w.manufacturer, maker.trim());
        prop_assert!(w.selected_registrar);
        // split over several vendor elements, as long WPS data is, it reads the same
        let p = ie::parse(&craft::wps_elements(&f));
        prop_assert_eq!(p.wps.map(|w| w.manufacturer), Some(maker.trim().to_string()));
    }

    #[test]
    fn netlink_readers_on_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let n = nl80211::messages(&bytes).count();
        prop_assert!(n <= bytes.len() / 16 + 1);
        for (_, _, _, payload) in nl80211::messages(&bytes) {
            for (_, v) in nl80211::attrs(payload) {
                if let Some(b) = nl80211::parse_bss(v) {
                    prop_assert!(b.ies.len() <= 4096 && b.beacon_ies.len() <= 4096);
                    describe(&Ap::new(b));
                }
            }
        }
        prop_assert!(nl80211::attrs(&bytes).count() <= bytes.len() / 4 + 1);
        if let Some(b) = nl80211::parse_bss(&bytes) {
            describe(&Ap::new(b));
        }
    }

    #[test]
    fn grouping_random_scans(list in proptest::collection::vec((0u8..6, any::<bool>(), -95i32..-20, 0usize..6), 0..40)) {
        let b = beacons();
        let bss: Vec<Bss> = list.iter().map(|&(m, assoc, dbm, pick)| Bss {
            bssid: [2, 0, 0, 0, 0, m], freq_mhz: 5180, signal_mbm: Some(dbm * 100), associated: assoc, capability: 0x11, ies: b[pick].clone(), ..Default::default()
        }).collect();
        let unique: std::collections::HashSet<_> = bss.iter().map(|b| b.bssid).collect();
        let nets = group(bss);
        prop_assert_eq!(nets.iter().map(|n| n.aps.len()).sum::<usize>(), unique.len());
        for n in &nets {
            prop_assert!(!n.aps.is_empty());
            let _ = (n.name(), n.mixed_security(), n.generation(), n.best());
        }
    }

    #[test]
    fn fixture_text_and_maker_list_on_random_input(text in "\\PC{0,400}", raw in proptest::collection::vec(any::<u8>(), 0..400)) {
        let _ = parse_fixture(&text);
        let _ = oui::makers_from(&raw[..], &[[0, 0, 0], [2, 0, 0]]);
        let s = oui::short_vendor(&text);
        prop_assert!(!s.chars().any(ie::invisible));
    }

    #[test]
    fn cleaned_text_shows_nothing_hidden(raw in proptest::collection::vec(any::<u8>(), 0..200), max in 0usize..70) {
        let t = ie::clean_text(&raw, max);
        prop_assert!(t.chars().count() <= max);
        prop_assert!(!t.chars().any(ie::invisible));
    }
}
