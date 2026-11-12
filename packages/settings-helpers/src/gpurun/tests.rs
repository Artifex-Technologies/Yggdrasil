//! The Python script's tests (helpers/tests/test_gpu_run.py), carried over: a pretend /sys with one card, and an NVIDIA card
//! next to an Intel Arc (the RTX 2070 + B580 setup Lossless Scaling's dual-GPU mode is for). And more: card numbers as int()
//! reads them, refusals.

use super::*;
use crate::testutil::{env_lock, TempDir};
use std::sync::MutexGuard;

const VULKANINFO: &str = "
Devices:
========
GPU0:
\tapiVersion         = 1.4.312
\tdriverVersion      = 580.95.5.0
\tvendorID           = 0x10de
\tdeviceID           = 0x1f07
\tdeviceType         = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU
\tdeviceName         = NVIDIA GeForce RTX 2070
GPU1:
\tapiVersion         = 1.4.318
\tvendorID           = 0x8086
\tdeviceID           = 0xe20b
\tdeviceType         = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU
\tdeviceName         = Intel(R) Arc(tm) B580 Graphics (BMG G21)
";

/// A pretend /sys with the given cards: (cardN, slot, vendor, device, driver, boot, connected outputs).
struct Sys {
    _root: TempDir,
    _lock: MutexGuard<'static, ()>,
}

/// A made-up card: (drm name, PCI slot, vendor, device, driver, boot VGA, each connector connected or not).
type FakeCard<'a> = (&'a str, &'a str, &'a str, &'a str, &'a str, bool, &'a [bool]);

fn sys(cards: &[FakeCard]) -> Sys {
    let lock = env_lock();
    let root = TempDir::new();
    let drm = root.0.join("class/drm");
    fs_mk(&drm);
    for (card, slot, vendor, device, driver, boot, outputs) in cards {
        let dev = root.0.join("devices/pci0000:00").join(slot);
        fs_mk(&dev);
        for (k, v) in [("vendor", *vendor), ("device", *device), ("boot_vga", if *boot { "1" } else { "0" })] {
            std::fs::write(dev.join(k), format!("{v}\n")).unwrap();
        }
        let drv = root.0.join("bus/pci/drivers").join(driver);
        fs_mk(&drv);
        std::os::unix::fs::symlink(&drv, dev.join("driver")).unwrap();
        fs_mk(&drm.join(card));
        std::os::unix::fs::symlink(&dev, drm.join(card).join("device")).unwrap();
        for (i, connected) in outputs.iter().enumerate() {
            let o = drm.join(format!("{card}-DP-{}", i + 1));
            fs_mk(&o);
            std::fs::write(o.join("status"), if *connected { "connected\n" } else { "disconnected\n" }).unwrap();
        }
    }
    std::env::set_var("YGGDRASIL_SYSFS", &root.0);
    Sys { _root: root, _lock: lock }
}

fn fs_mk(p: &std::path::Path) {
    std::fs::create_dir_all(p).unwrap();
}

fn names(slot: &str) -> String {
    match slot {
        "0000:01:00.0" => "GeForce RTX 2070".into(),
        "0000:03:00.0" => "Arc B580".into(),
        _ => String::new(),
    }
}

fn two() -> Sys {
    sys(&[
        ("card1", "0000:01:00.0", "0x10de", "0x1f07", "nvidia", true, &[true]),
        ("card2", "0000:03:00.0", "0x8086", "0xe20b", "xe", false, &[false, false]),
    ])
}

fn cs(vk: Vec<VkDevice>) -> Vec<Card> {
    cards_with(Some(vk), &names).unwrap()
}

fn vk() -> Vec<VkDevice> {
    vulkan_devices(Some(VULKANINFO)).unwrap()
}

fn get<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
    env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
}

#[test]
fn test_vulkaninfo_is_read() {
    assert_eq!(
        vk(),
        vec![
            (0x10DE, 0x1F07, "NVIDIA GeForce RTX 2070".to_string()),
            (0x8086, 0xE20B, "Intel(R) Arc(tm) B580 Graphics (BMG G21)".to_string())
        ]
    );
}

#[test]
fn test_cards_are_numbered_in_pci_order_with_their_screens() {
    let _s = two();
    let c = cs(vk());
    let got: Vec<(i64, &str, &str, bool, i64)> =
        c.iter().map(|c| (c.number, c.name.as_str(), c.driver.as_str(), c.boot, c.screens)).collect();
    assert_eq!(got, vec![(1, "GeForce RTX 2070", "nvidia", true, 1), (2, "Arc B580", "xe", false, 0)]);
    assert_eq!(c[1].vulkan_name, "Intel(R) Arc(tm) B580 Graphics (BMG G21)");
}

#[test]
fn test_one_card_needs_nothing() {
    let _s = sys(&[("card0", "0000:01:00.0", "0x10de", "0x1e07", "nvidia", true, &[true])]);
    let c = cs(vec![]);
    assert_eq!(environment(&c[0], &c), vec![]);
}

#[test]
fn test_the_arc_is_picked_and_the_nvidia_card_hidden() {
    let _s = two();
    let c = cs(vk());
    let env = environment(&c[1], &c);
    assert_eq!(get(&env, "DRI_PRIME"), Some("pci-0000_03_00_0"));
    assert_eq!(get(&env, "MESA_VK_DEVICE_SELECT"), Some("8086:e20b"));
    assert_eq!(get(&env, "__VK_LAYER_NV_optimus"), Some("non_NVIDIA_only"));
    assert_eq!(get(&env, "DXVK_FILTER_DEVICE_NAME"), Some("Intel(R) Arc(tm) B580 Graphics (BMG G21)"));
    assert_eq!(get(&env, "VKD3D_FILTER_DEVICE_NAME"), get(&env, "DXVK_FILTER_DEVICE_NAME"));
    assert_eq!(get(&env, "__NV_PRIME_RENDER_OFFLOAD"), None);
}

#[test]
fn test_the_nvidia_card_is_picked_by_prime_offload() {
    let _s = two();
    let c = cs(vk());
    let env = environment(&c[0], &c);
    assert_eq!(
        (get(&env, "__NV_PRIME_RENDER_OFFLOAD"), get(&env, "__GLX_VENDOR_LIBRARY_NAME"), get(&env, "__VK_LAYER_NV_optimus")),
        (Some("1"), Some("nvidia"), Some("NVIDIA_only"))
    );
    assert_eq!(get(&env, "DXVK_FILTER_DEVICE_NAME"), Some("NVIDIA GeForce RTX 2070"));
    assert_eq!(get(&env, "DRI_PRIME"), None);
}

#[test]
fn test_without_vulkaninfo_the_name_filters_are_left_out() {
    let _s = two();
    let c = cs(vec![]);
    let env = environment(&c[1], &c);
    assert_eq!(get(&env, "DXVK_FILTER_DEVICE_NAME"), None);
    assert_eq!(get(&env, "DRI_PRIME"), Some("pci-0000_03_00_0"));
}

#[test]
fn test_frames_on_the_arc_while_the_2070_draws() {
    let _s = two();
    let c = cs(vk());
    let argv = frames_command(&c[0], &c[1], &c, &["%command%".to_string(), "-arg".to_string()], 3);
    let g = argv.iter().position(|a| a == "gamescope").unwrap();
    let dd = argv.iter().position(|a| a == "--").unwrap();
    let (outer, gs, inner) = (&argv[..g], &argv[g..dd + 1], &argv[dd + 1..]);
    assert!(outer.contains(&"LSFGVK_ENV=1".to_string()));
    assert!(outer.contains(&"LSFGVK_MULTIPLIER=3".to_string()));
    assert!(outer.contains(&"DRI_PRIME=pci-0000_03_00_0".to_string()), "gamescope composites on the Arc");
    assert_eq!(gs[gs.iter().position(|a| a == "--prefer-vk-device").unwrap() + 1], "8086:e20b");
    assert_eq!(gs[1..3], ["--backend".to_string(), "sdl".to_string()], "a Vulkan swapchain lsfg-vk can work on");
    assert_eq!(inner[inner.len() - 2..], ["%command%".to_string(), "-arg".to_string()]);
    assert!(inner.contains(&"DISABLE_LSFGVK=1".to_string()), "no frame generation inside the game itself");
    let u = inner.iter().position(|a| a == "-u").unwrap();
    for k in ["LSFGVK_ENV", "LSFGVK_MULTIPLIER", "DRI_PRIME", "MESA_VK_DEVICE_SELECT"] {
        assert!(inner[u - 1..].contains(&k.to_string()), "{k} is taken away from the game");
    }
    assert!(inner.contains(&"__NV_PRIME_RENDER_OFFLOAD=1".to_string()), "the game draws on the 2070");
    assert!(inner.contains(&"__VK_LAYER_NV_optimus=NVIDIA_only".to_string()), "the game's value replaces gamescope's");
}

#[test]
fn test_a_card_number_that_does_not_exist_is_refused() {
    let _s = two();
    let c = cs(vec![]);
    assert!(pick("3", &c).is_none());
    assert!(pick("x", &c).is_none());
    assert_eq!(pick("2", &c).unwrap().slot, "0000:03:00.0");
}

// ---- more

#[test]
fn card_numbers_are_read_as_int_read_them() {
    let _s = two();
    let c = cs(vec![]);
    for ok in [" 2", "+2", "02", "2\n", "\u{ff12}", "0_2"] {
        assert_eq!(pick(ok, &c).map(|c| c.number), Some(2), "{ok:?}");
    }
    for bad in ["", "2.0", "0x2", "two", "-2", "2 1", "\u{b2}", "1;id", "$(id)"] {
        assert!(pick(bad, &c).map(|c| c.number) != Some(2) || bad.is_empty(), "{bad:?}");
        assert!(pick(bad, &c).is_none(), "{bad:?}");
    }
}

#[test]
fn a_card_without_a_vendor_is_left_out_and_odd_names_are_not_cards() {
    let _s = sys(&[
        ("card0", "0000:01:00.0", "0x1002", "0x73bf", "amdgpu", true, &[true, true]),
        ("card10", "0000:0a:00.0", "0x8086", "0x56a0", "i915", false, &[]),
    ]);
    let root = std::env::var("YGGDRASIL_SYSFS").unwrap();
    // a render node and a card whose device has no vendor file
    std::fs::create_dir_all(format!("{root}/class/drm/renderD128/device")).unwrap();
    std::fs::create_dir_all(format!("{root}/class/drm/card7/device")).unwrap();
    let c = cs(vec![]);
    let got: Vec<(i64, &str, i64)> = c.iter().map(|c| (c.number, c.slot.as_str(), c.screens)).collect();
    assert_eq!(got, vec![(1, "0000:01:00.0", 2), (2, "0000:0a:00.0", 0)]);
    assert_eq!(c[1].name, "Intel card", "no lspci name: the vendor's");
    let env = environment(&c[0], &c);
    assert_eq!(
        env,
        vec![("DRI_PRIME".to_string(), "pci-0000_01_00_0".to_string()), ("MESA_VK_DEVICE_SELECT".to_string(), "1002:73bf".to_string())]
    );
}

#[test]
fn json_as_settings_reads_it() {
    let _s = two();
    let c = cs(vk());
    let text = json::dumps_indent(&Value::from(c.iter().map(Card::to_json).collect::<Vec<_>>()), 1);
    assert!(text.starts_with("[\n {\n  \"slot\": \"0000:01:00.0\",\n  \"vendor\": 4318,\n  \"device\": 7943,\n  \"driver\": \"nvidia\",\n  \"boot\": true,\n  \"screens\": 1,\n  \"number\": 1,\n"), "{text}");
}
