//! What Linux tells about a graphics card without root and without a vendor library: a DRM client's fdinfo
//! (/proc/PID/fdinfo/FD of an open /dev/dri node, in the format of the kernel's Documentation/gpu/drm-usage-stats.rst, which
//! amdgpu, i915, xe, panfrost, panthor, v3d and others write), the driver's files in /sys (amdgpu's busy percentages and
//! memory, i915/xe frequencies, hwmon sensors, the PCIe link) and pci.ids for a card's name.
//!
//! Plain functions over text, so the parts that cannot be tried on this machine (it has only an NVIDIA card, whose open
//! driver writes no usage stats) are tested with the kernel's own examples.

use std::collections::HashMap;

/// One open DRM file of a process, as its fdinfo describes it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Client {
    pub driver: String,
    /// The kernel's id for the open file: the same id under two descriptors (dup, or shared with a child) is one client.
    pub id: Option<u64>,
    /// Busy time per engine, nanoseconds since the client opened the card (a name given twice is added up).
    pub engine_ns: Vec<(String, u64)>,
    /// Engines of the same kind counted together (drm-engine-capacity-*), 1 when not given.
    pub capacity: HashMap<String, u64>,
    /// (busy cycles, total cycles) per engine, for drivers that count in GPU cycles (xe).
    pub cycles: Vec<(String, u64, u64)>,
    /// Bytes per memory region: resident where the driver says, else the older drm-memory-*, else the total requested.
    pub memory: Vec<(String, u64)>,
}

/// "1234", "1234 ns", "36496 KiB", "290 MiB" -> the number in bytes (or the plain number).
fn number(v: &str) -> Option<u64> {
    let mut it = v.split_whitespace();
    let n: u64 = it.next()?.parse().ok()?;
    Some(match it.next() {
        Some("KiB") => n * 1024,
        Some("MiB") => n * 1024 * 1024,
        Some("GiB") => n * 1024 * 1024 * 1024,
        _ => n,
    })
}

/// Parses one fdinfo file; None when it is not a DRM client with usage stats (no drm-driver line).
pub fn parse_fdinfo(s: &str) -> Option<Client> {
    let mut c = Client::default();
    let mut resident: Vec<(String, u64)> = vec![];
    let mut legacy: Vec<(String, u64)> = vec![];
    let mut total: Vec<(String, u64)> = vec![];
    let mut cycles: HashMap<String, u64> = HashMap::new();
    let mut total_cycles: HashMap<String, u64> = HashMap::new();
    let mut have_driver = false;
    for line in s.lines() {
        let Some((k, v)) = line.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let Some(key) = k.strip_prefix("drm-") else { continue };
        if key == "driver" {
            c.driver = v.to_string();
            have_driver = true;
        } else if key == "client-id" {
            c.id = v.parse().ok();
        } else if let Some(e) = key.strip_prefix("engine-capacity-") {
            if let Some(n) = number(v).filter(|&n| n > 0) {
                c.capacity.insert(e.to_string(), n);
            }
        } else if let Some(e) = key.strip_prefix("engine-") {
            if let Some(n) = number(v) {
                match c.engine_ns.iter_mut().find(|(name, _)| name == e) {
                    Some(x) => x.1 += n,
                    None => c.engine_ns.push((e.to_string(), n)),
                }
            }
        } else if let Some(e) = key.strip_prefix("total-cycles-") {
            if let Some(n) = number(v) {
                total_cycles.insert(e.to_string(), n);
            }
        } else if let Some(e) = key.strip_prefix("cycles-") {
            if let Some(n) = number(v) {
                cycles.insert(e.to_string(), n);
            }
        } else if let Some(r) = key.strip_prefix("resident-") {
            resident.extend(number(v).map(|n| (r.to_string(), n)));
        } else if let Some(r) = key.strip_prefix("memory-") {
            legacy.extend(number(v).map(|n| (r.to_string(), n)));
        } else if let Some(r) = key.strip_prefix("total-") {
            total.extend(number(v).map(|n| (r.to_string(), n)));
        }
    }
    if !have_driver {
        return None;
    }
    let mut regions: Vec<String> = vec![];
    for (r, _) in resident.iter().chain(&legacy).chain(&total) {
        if !regions.contains(r) {
            regions.push(r.clone());
        }
    }
    let find = |v: &[(String, u64)], r: &str| v.iter().find(|(x, _)| x == r).map(|x| x.1);
    c.memory =
        regions.into_iter().map(|r| (r.clone(), find(&resident, &r).or(find(&legacy, &r)).or(find(&total, &r)).unwrap_or(0))).collect();
    let mut names: Vec<&String> = cycles.keys().filter(|k| total_cycles.contains_key(*k)).collect();
    names.sort();
    c.cycles = names.into_iter().map(|k| (k.clone(), cycles[k], total_cycles[k])).collect();
    Some(c)
}

/// Where a memory region lives, as Windows' Task Manager splits it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    /// The card's own memory (VRAM; on an integrated chip the part of RAM set aside for it).
    Dedicated,
    /// System memory the card uses (GTT, system).
    Shared,
}

/// Sorts a driver's region names; None for bookkeeping regions that are not memory a program fills (amdgpu's gds, gws, oa,
/// doorbell, mmioremap; its "cpu" region is buffers the card cannot reach right now).
pub fn region(name: &str) -> Option<Region> {
    if name.starts_with("vram") || name.starts_with("local") || name.starts_with("stolen") {
        Some(Region::Dedicated)
    } else if name == "gtt" || name.starts_with("system") || name == "memory" {
        Some(Region::Shared)
    } else {
        None
    }
}

/// The name Windows would give an engine, from the driver's key (Documentation/gpu/*: amdgpu, i915, xe, panfrost, v3d).
pub fn engine_label(driver: &str, key: &str) -> String {
    let l = match (driver, key) {
        ("amdgpu", "gfx") => "3D",
        ("amdgpu", "compute") => "Compute",
        ("amdgpu", "dma") => "Copy",
        ("amdgpu", "dec" | "jpeg") => "Video decode",
        ("amdgpu", "enc" | "enc_1") => "Video encode",
        ("amdgpu", "vpe") => "Video processing",
        ("i915", "render") | ("xe", "rcs") => "3D",
        ("i915", "copy") | ("xe", "bcs") => "Copy",
        ("i915", "video") | ("xe", "vcs") => "Video codec",
        ("i915", "video-enhance") | ("xe", "vecs") => "Video processing",
        ("i915", "compute") | ("xe", "ccs") => "Compute",
        ("panfrost", "fragment") => "3D (fragment)",
        ("panfrost", "vertex-tiler") => "3D (vertex)",
        ("v3d", "render") => "3D (render)",
        ("v3d", "bin") => "3D (binning)",
        ("v3d", "csd") => "Compute",
        ("v3d", "tfu") => "Texture upload",
        _ => {
            let mut s = key.replace(['-', '_'], " ");
            if let Some(f) = s.get(..1) {
                s = f.to_uppercase() + &s[1..];
            }
            return s;
        }
    };
    l.to_string()
}

/// Windows' order: 3D first, then compute, copy and video.
pub fn engine_rank(label: &str) -> u8 {
    match label {
        l if l.starts_with("3D") => 0,
        "Compute" => 1,
        "Copy" => 2,
        "Video encode" => 3,
        "Video decode" => 4,
        "Video codec" => 5,
        "Video processing" => 6,
        _ => 7,
    }
}

/// Busy share of one engine between two looks, 0-100. `prev`/`now` are nanoseconds busy; `wall_ns` the time between the
/// looks. A value that went backwards counts as no change (the kernel allows that briefly).
pub fn ns_util(prev: u64, now: u64, wall_ns: f64, capacity: u64) -> f32 {
    if wall_ns <= 0.0 || now <= prev {
        return 0.0;
    }
    ((now - prev) as f64 / wall_ns / capacity.max(1) as f64 * 100.0).clamp(0.0, 100.0) as f32
}

/// Busy share from GPU cycles: (busy, total) before and now.
pub fn cycles_util(prev: (u64, u64), now: (u64, u64), capacity: u64) -> f32 {
    let busy = now.0.saturating_sub(prev.0) as f64;
    let total = now.1.saturating_sub(prev.1) as f64;
    if total <= 0.0 {
        return 0.0;
    }
    (busy / total / capacity.max(1) as f64 * 100.0).clamp(0.0, 100.0) as f32
}

/// "16.0 GT/s PCIe" (sysfs current_link_speed) -> the PCIe generation, 4.
pub fn link_generation(speed: &str) -> Option<u32> {
    let gts: f32 = speed.split_whitespace().next()?.parse().ok()?;
    Some(match gts {
        x if x < 3.0 => 1,
        x if x < 6.0 => 2,
        x if x < 10.0 => 3,
        x if x < 20.0 => 4,
        x if x < 40.0 => 5,
        x if x < 80.0 => 6,
        _ => 7,
    })
}

/// The top level of an amdgpu clock table (pp_dpm_sclk, pp_dpm_mclk: "0: 500Mhz\n1: 1800Mhz *"), MHz.
pub fn dpm_max_mhz(s: &str) -> Option<u32> {
    s.lines()
        .filter_map(|l| {
            let v = l.split_once(':')?.1.trim();
            let digits: String = v.chars().take_while(|c| c.is_ascii_digit()).collect();
            let n: u32 = digits.parse().ok()?;
            Some(if v[digits.len()..].to_lowercase().starts_with("ghz") { n * 1000 } else { n })
        })
        .max()
}

/// A device's name from pci.ids ("Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]"): the vendor's line, then the device's
/// line under it (one tab in).
pub fn pci_name(ids: &str, vendor: u16, device: u16) -> Option<String> {
    let v = format!("{vendor:04x}");
    let d = format!("{device:04x}");
    let mut in_vendor = false;
    for line in ids.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        if !line.starts_with('\t') {
            if in_vendor {
                return None; // the next vendor: not listed
            }
            in_vendor = line.starts_with(&v) && line[4..].starts_with(' ');
        } else if in_vendor && !line.starts_with("\t\t") {
            let rest = &line[1..];
            if rest.starts_with(&d) && rest[4..].starts_with(' ') {
                return Some(rest[4..].trim().to_string());
            }
        }
    }
    None
}

/// A friendlier card name: "Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]" -> "AMD Radeon RX 6800/6800 XT / 6900 XT";
/// "Alder Lake-P GT2 [Iris Xe Graphics]" -> "Intel Iris Xe Graphics".
pub fn marketing_name(vendor: u16, pci: &str) -> String {
    let inner = match (pci.find('['), pci.rfind(']')) {
        (Some(a), Some(b)) if b > a => &pci[a + 1..b],
        _ => pci,
    };
    let maker = match vendor {
        0x1002 => "AMD ",
        0x8086 => "Intel ",
        0x10de => "NVIDIA ",
        _ => "",
    };
    if inner.starts_with(maker.trim()) || maker.is_empty() {
        inner.to_string()
    } else {
        format!("{maker}{inner}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The examples in the kernel's documentation (Documentation/gpu/i915.rst, panfrost.rst, xe_drm_client.c), as written there
    // with the leading spaces removed.
    const I915: &str = "pos:    0\nflags:  0100002\nmnt_id: 21\ndrm-driver: i915\ndrm-pdev:   0000:00:02.0\ndrm-client-id:      7\ndrm-engine-render:  9288864723 ns\ndrm-engine-copy:    2035071108 ns\ndrm-engine-video:   0 ns\ndrm-engine-capacity-video:   2\ndrm-engine-video-enhance:   0 ns\n";
    const PANFROST: &str = "pos:    0\nflags:  02400002\nmnt_id: 27\nino:    531\ndrm-driver:     panfrost\ndrm-client-id:  14\ndrm-engine-fragment:    1846584880 ns\ndrm-cycles-fragment:    1424359409\ndrm-maxfreq-fragment:   799999987 Hz\ndrm-curfreq-fragment:   799999987 Hz\ndrm-engine-vertex-tiler:        71932239 ns\ndrm-cycles-vertex-tiler:        52617357\ndrm-maxfreq-vertex-tiler:       799999987 Hz\ndrm-curfreq-vertex-tiler:       799999987 Hz\ndrm-total-memory:       290 MiB\ndrm-shared-memory:      0 MiB\ndrm-active-memory:      226 MiB\ndrm-resident-memory:    36496 KiB\ndrm-purgeable-memory:   128 KiB\n";
    const XE: &str = "pos:    0\nflags:  0100002\nmnt_id: 26\nino:    685\ndrm-driver:     xe\ndrm-client-id:  3\ndrm-pdev:       0000:03:00.0\ndrm-total-system:       0\ndrm-shared-system:      0\ndrm-active-system:      0\ndrm-resident-system:    0\ndrm-purgeable-system:   0\ndrm-total-gtt:  192 KiB\ndrm-shared-gtt: 0\ndrm-active-gtt: 0\ndrm-resident-gtt:       192 KiB\ndrm-total-vram0:        23992 KiB\ndrm-shared-vram0:       16 MiB\ndrm-active-vram0:       0\ndrm-resident-vram0:     23992 KiB\ndrm-total-stolen:       0\ndrm-shared-stolen:      0\ndrm-active-stolen:      0\ndrm-resident-stolen:    0\ndrm-cycles-rcs: 28257900\ndrm-total-cycles-rcs:   7655183225\ndrm-cycles-bcs: 0\ndrm-total-cycles-bcs:   7655183225\ndrm-cycles-vcs: 0\ndrm-total-cycles-vcs:   7655183225\ndrm-engine-capacity-vcs:        2\ndrm-cycles-vecs:        0\ndrm-total-cycles-vecs:  7655183225\ndrm-engine-capacity-vecs:       2\ndrm-cycles-ccs: 0\ndrm-total-cycles-ccs:   7655183225\ndrm-engine-capacity-ccs:        4\n";
    // amdgpu as amdgpu_fdinfo.c prints it (drm_print_memory_stats, the legacy drm-memory-* aliases, engines only when used;
    // the UVD and VCN decoders share the name "dec")
    const AMDGPU: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t26\nino:\t1121\ndrm-driver:\tamdgpu\ndrm-client-id:\t17\ndrm-pdev:\t0000:03:00.0\npasid:\t32771\ndrm-total-vram:\t145664 KiB\ndrm-shared-vram:\t0\ndrm-resident-vram:\t145664 KiB\ndrm-purgeable-vram:\t0\ndrm-total-gtt:\t2048 KiB\ndrm-shared-gtt:\t0\ndrm-resident-gtt:\t2048 KiB\ndrm-total-cpu:\t0\ndrm-resident-cpu:\t0\ndrm-memory-vram:\t145664 KiB\ndrm-memory-gtt: \t2048 KiB\ndrm-memory-cpu: \t0 KiB\namd-evicted-vram:\t0 KiB\namd-requested-vram:\t145664 KiB\namd-requested-gtt:\t2048 KiB\ndrm-engine-gfx:\t1234567 ns\ndrm-engine-dma:\t55 ns\ndrm-engine-dec:\t1000 ns\ndrm-engine-dec:\t500 ns\n";
    // an older amdgpu (before drm_print_memory_stats): only the drm-memory-* keys
    const AMDGPU_OLD: &str = "drm-driver:\tamdgpu\ndrm-pdev:\t0000:0a:00.0\ndrm-client-id:\t4\ndrm-memory-vram:\t8192 KiB\ndrm-memory-gtt: \t1024 KiB\ndrm-memory-cpu: \t0 KiB\ndrm-engine-gfx:\t42 ns\n";
    // what the NVIDIA open driver writes for /dev/dri/renderD128 here (no usage stats)
    const NVIDIA_DRM: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t30\nino:\t282\n";

    #[test]
    fn i915_example() {
        let c = parse_fdinfo(I915).unwrap();
        assert_eq!((c.driver.as_str(), c.id), ("i915", Some(7)));
        assert_eq!(c.engine_ns[0], ("render".to_string(), 9288864723));
        assert_eq!(c.engine_ns.len(), 4, "render, copy, video, video-enhance");
        assert_eq!(c.capacity.get("video"), Some(&2));
        assert!(!c.engine_ns.iter().any(|(e, _)| e.starts_with("capacity")), "capacity is not an engine");
        assert!(c.memory.is_empty());
        assert_eq!(engine_label("i915", "render"), "3D");
        assert_eq!(engine_label("i915", "video-enhance"), "Video processing");
    }

    #[test]
    fn panfrost_example() {
        let c = parse_fdinfo(PANFROST).unwrap();
        assert_eq!(c.engine_ns, vec![("fragment".into(), 1846584880), ("vertex-tiler".into(), 71932239)]);
        assert!(c.cycles.is_empty(), "cycles without total cycles are not a ratio");
        assert_eq!(c.memory, vec![("memory".to_string(), 36496 * 1024)], "resident wins over total");
        assert_eq!(region("memory"), Some(Region::Shared));
    }

    #[test]
    fn xe_example() {
        let c = parse_fdinfo(XE).unwrap();
        assert!(c.engine_ns.is_empty());
        let rcs = c.cycles.iter().find(|e| e.0 == "rcs").unwrap();
        assert_eq!((rcs.1, rcs.2), (28257900, 7655183225));
        assert_eq!(c.cycles.len(), 5);
        assert_eq!(c.capacity.get("ccs"), Some(&4));
        let mem: HashMap<_, _> = c.memory.iter().cloned().collect();
        assert_eq!((mem["gtt"], mem["vram0"], mem["system"]), (192 * 1024, 23992 * 1024, 0));
        assert_eq!(
            (region("vram0"), region("gtt"), region("stolen")),
            (Some(Region::Dedicated), Some(Region::Shared), Some(Region::Dedicated))
        );
        assert_eq!(engine_label("xe", "ccs"), "Compute");
    }

    #[test]
    fn amdgpu_examples() {
        let c = parse_fdinfo(AMDGPU).unwrap();
        assert_eq!(c.id, Some(17));
        assert!(c.engine_ns.contains(&("dec".to_string(), 1500)), "both decoders added up");
        let mem: HashMap<_, _> = c.memory.iter().cloned().collect();
        assert_eq!((mem["vram"], mem["gtt"], mem["cpu"]), (145664 * 1024, 2048 * 1024, 0));
        assert_eq!(region("cpu"), None);
        assert_eq!(region("doorbell"), None);
        let old = parse_fdinfo(AMDGPU_OLD).unwrap();
        assert_eq!(old.memory[0], ("vram".to_string(), 8192 * 1024), "the deprecated key still counts");
        assert_eq!(engine_label("amdgpu", "gfx"), "3D");
        assert_eq!(engine_label("amdgpu", "enc_1"), "Video encode");
        assert_eq!(engine_label("amdgpu", "jpeg"), "Video decode");
    }

    #[test]
    fn not_a_client_with_stats() {
        assert!(parse_fdinfo(NVIDIA_DRM).is_none());
        assert!(parse_fdinfo("").is_none());
        assert_eq!(engine_label("future", "neural_net"), "Neural net");
    }

    #[test]
    fn utilisation() {
        // 0.5 s busy in 1 s
        assert_eq!(ns_util(1_000_000_000, 1_500_000_000, 1e9, 1), 50.0);
        // two video engines, one busy all the time
        assert_eq!(ns_util(0, 1_000_000_000, 1e9, 2), 50.0);
        assert_eq!(ns_util(5, 4, 1e9, 1), 0.0, "a value that went back counts as nothing");
        assert_eq!(ns_util(0, 5_000_000_000, 1e9, 1), 100.0, "capped");
        assert_eq!(cycles_util((28257900, 7655183225), (28257900 + 500, 7655183225 + 1000), 1), 50.0);
        assert_eq!(cycles_util((0, 0), (100, 100), 4), 25.0);
        assert_eq!(cycles_util((0, 5), (0, 5), 1), 0.0);
        let mut v = vec!["Video decode", "Copy", "3D", "Compute"];
        v.sort_by_key(|l| engine_rank(l));
        assert_eq!(v, vec!["3D", "Compute", "Copy", "Video decode"]);
    }

    #[test]
    fn sysfs_texts() {
        assert_eq!(link_generation("16.0 GT/s PCIe"), Some(4));
        assert_eq!(link_generation("8.0 GT/s PCIe"), Some(3));
        assert_eq!(link_generation("2.5 GT/s PCIe"), Some(1));
        assert_eq!(link_generation("Unknown"), None);
        assert_eq!(dpm_max_mhz("0: 500Mhz \n1: 1800Mhz *\n2: 2575Mhz \n"), Some(2575));
        assert_eq!(dpm_max_mhz("0: 96Mhz *\n"), Some(96));
        assert_eq!(dpm_max_mhz(""), None);
    }

    #[test]
    fn pci_ids() {
        let ids = "# comment\n1002  Advanced Micro Devices, Inc. [AMD/ATI]\n\t73bf  Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]\n\t\t1002 0e3a  Radeon RX 6900 XT\n10de  NVIDIA Corporation\n\t1e07  TU102 [GeForce RTX 2080 Ti Rev. A]\n8086  Intel Corporation\n\t46a6  Alder Lake-P GT2 [Iris Xe Graphics]\n";
        assert_eq!(pci_name(ids, 0x1002, 0x73bf).as_deref(), Some("Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]"));
        assert_eq!(pci_name(ids, 0x8086, 0x46a6).as_deref(), Some("Alder Lake-P GT2 [Iris Xe Graphics]"));
        assert_eq!(pci_name(ids, 0x1002, 0x0e3a), None, "a subsystem line is not a device");
        assert_eq!(pci_name(ids, 0x1234, 0x0001), None);
        assert_eq!(marketing_name(0x1002, "Navi 21 [Radeon RX 6800/6800 XT / 6900 XT]"), "AMD Radeon RX 6800/6800 XT / 6900 XT");
        assert_eq!(marketing_name(0x8086, "Alder Lake-P GT2 [Iris Xe Graphics]"), "Intel Iris Xe Graphics");
        assert_eq!(marketing_name(0x14e4, "VideoCore"), "VideoCore");
    }
}
