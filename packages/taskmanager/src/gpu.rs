//! Every graphics card and what each process does on it, for the Processes, Details and Performance tabs.
//!
//! - NVIDIA: NVML (nvml.rs), opened the first time a tab that shows the card is open; the card's numbers and, per
//!   process, the card memory it holds and its share of the 3D/compute, video encode and video decode engines.
//! - AMD (amdgpu), Intel (i915, xe) and the others that write DRM usage stats (panfrost, panthor, v3d, ...): the busy time
//!   in each open /dev/dri file's fdinfo (drm.rs), read only for processes that hold such a file (found once per process
//!   and looked for again only when its number of open files changes), plus the driver's files in /sys and hwmon.
//! - The graphics APIs a process uses (CUDA, Vulkan, OpenGL, ggml, ONNX Runtime, ...) from the libraries it has mapped,
//!   read for processes on a card only, at most every 30 s.
//!
//! Nothing is written anywhere; nothing needs root. What cannot be read is None and is not shown (a Raspberry Pi, a virtual
//! machine, another account's processes).

use crate::drm::{self, Region};
use crate::nvml::{self, Nvml};
use crate::ollama;
use crate::procs::Proc;
use std::collections::{HashMap, HashSet};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Vendor {
    Nvidia,
    Amd,
    Intel,
    #[default]
    Other,
}

/// One graphics card (or integrated graphics) now. A None is a number this card or driver does not give.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Device {
    /// Stays the same while the program runs: the PCI address, or the device's path in /sys.
    pub key: String,
    pub name: String,
    pub vendor: Vendor,
    pub driver: String,
    pub integrated: Option<bool>,
    /// Busy share of the whole card, 0-100.
    pub util: Option<f32>,
    /// Each engine's busy share, Windows' order.
    pub engines: Vec<(String, f32)>,
    /// The engines are what your own programs use (from fdinfo), not the whole card.
    pub engines_yours: bool,
    /// Memory controller busy share.
    pub mem_ctrl: Option<f32>,
    pub dedicated: Option<u64>,
    pub dedicated_total: Option<u64>,
    pub shared: Option<u64>,
    pub shared_total: Option<u64>,
    /// The memory numbers are what your own programs hold (from fdinfo), not the whole card's.
    pub mem_yours: bool,
    pub temp: Option<f32>,
    /// Watts.
    pub power: Option<f32>,
    pub power_limit: Option<f32>,
    /// MHz now and at most.
    pub clock: Option<u32>,
    pub clock_max: Option<u32>,
    pub mem_clock: Option<u32>,
    pub mem_clock_max: Option<u32>,
    pub fan_pct: Option<f32>,
    pub fan_rpm: Option<u32>,
    /// PCIe (generation, lanes) now and at most.
    pub link: Option<(u32, u32)>,
    pub link_max: Option<(u32, u32)>,
    /// PCIe bytes per second sent and received (NVIDIA only, and only while the card's page is open).
    pub pcie_tx: Option<f64>,
    pub pcie_rx: Option<f64>,
    pub driver_version: Option<String>,
    pub cuda: Option<String>,
    pub location: Option<String>,
}

impl Device {
    /// Anything worth a page: a virtual machine's display adapter or a display-only driver gives none of these.
    fn readable(&self) -> bool {
        self.util.is_some()
            || !self.engines.is_empty()
            || self.dedicated.is_some()
            || self.shared.is_some()
            || self.temp.is_some()
            || self.clock.is_some()
    }
}

/// What one process does on the graphics cards.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ProcGpu {
    /// Its busiest engine's share, 0-100 (Windows' "GPU" column).
    pub util: f32,
    /// "GPU 0 - 3D" (Windows' "GPU engine"); empty while it has used none.
    pub engine: String,
    /// Card memory it holds, bytes; None when the driver does not say.
    pub dedicated: Option<u64>,
    /// System memory the card uses for it.
    pub shared: Option<u64>,
    /// Graphics and compute libraries it has loaded.
    pub apis: Vec<&'static str>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub devices: Vec<Device>,
    pub procs: HashMap<u32, ProcGpu>,
    /// Per-process numbers can be read on this machine (some card gives them).
    pub per_process: bool,
    /// Ollama's loaded models; None when Ollama did not answer or was not asked.
    pub models: Option<Vec<ollama::Model>>,
}

/// What the open tab needs.
#[derive(Clone, Copy, Debug, Default)]
pub struct Want {
    /// The cards' own numbers.
    pub devices: bool,
    /// Each process's.
    pub procs: bool,
    /// A card's own page is open: also the slower numbers (PCIe throughput) and Ollama's models.
    pub page: bool,
}

/// Library names in /proc/PID/maps and the API each one means.
const APIS: [(&str, &str); 14] = [
    ("libcuda.so", "CUDA"),
    ("libcudart.so", "CUDA"),
    ("libvulkan.so", "Vulkan"),
    ("libGLX", "OpenGL"),
    ("libGL.so", "OpenGL"),
    ("libEGL", "OpenGL"),
    ("libGLESv2", "OpenGL"),
    ("libOpenCL.so", "OpenCL"),
    ("libggml", "ggml"),
    ("libllama", "ggml"),
    ("libonnxruntime", "ONNX Runtime"),
    ("libtorch_cuda", "PyTorch"),
    ("libnvidia-encode", "NVENC"),
    ("libva.so", "VA-API"),
];

/// The APIs a process has loaded, from its /proc/PID/maps, in the order of APIS.
pub fn apis_from_maps(maps: &str) -> Vec<&'static str> {
    let mut seen: HashSet<&'static str> = HashSet::new();
    for line in maps.lines() {
        let Some(path) = line.split_whitespace().nth(5) else { continue };
        let base = path.rsplit('/').next().unwrap_or(path);
        for (lib, api) in APIS {
            if base.starts_with(lib) {
                seen.insert(api);
            }
        }
    }
    let mut out: Vec<&'static str> = vec![];
    for (_, api) in APIS {
        if seen.contains(api) && !out.contains(&api) {
            out.push(api);
        }
    }
    out
}

/// A DRM device other than an NVIDIA card (those are NVML's).
#[derive(Clone, Debug, Default)]
struct DrmDev {
    key: String,
    dev_dir: PathBuf,
    /// /sys/class/drm/cardN, when it has a card node.
    card_dir: Option<PathBuf>,
    /// The device numbers of its /dev/dri nodes.
    rdevs: Vec<u64>,
    driver: String,
    name: String,
    vendor: Vendor,
    integrated: Option<bool>,
    location: Option<String>,
}

/// Drivers that write no usage stats (NVIDIA's own, display-only and virtual adapters): their open files are not read.
const NO_STATS: [&str; 9] = ["nvidia", "simpledrm", "efidrm", "vesadrm", "vkms", "bochs-drm", "cirrus", "vboxvideo", "hyperv_drm"];

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn read_u64(p: &Path) -> Option<u64> {
    read_trim(p)?.parse().ok()
}

fn hex(p: &Path) -> Option<u16> {
    u16::from_str_radix(read_trim(p)?.trim_start_matches("0x"), 16).ok()
}

fn platform_name(driver: &str) -> String {
    match driver {
        "v3d" => "Broadcom V3D".into(),
        "vc4" => "Broadcom VideoCore".into(),
        "panfrost" => "Arm Mali (Panfrost)".into(),
        "panthor" => "Arm Mali (Panthor)".into(),
        "lima" => "Arm Mali (Lima)".into(),
        "msm" => "Qualcomm Adreno".into(),
        "etnaviv" => "Vivante".into(),
        "asahi" => "Apple GPU".into(),
        d => format!("Graphics ({d})"),
    }
}

/// The DRM devices under `class` (/sys/class/drm), one per device with all its nodes.
fn drm_devices(class: &Path, pci_ids: &mut Option<String>) -> Vec<DrmDev> {
    let mut out: Vec<DrmDev> = vec![];
    let Ok(dir) = std::fs::read_dir(class) else { return out };
    let mut entries: Vec<(String, PathBuf)> = dir.flatten().map(|e| (e.file_name().to_string_lossy().to_string(), e.path())).collect();
    entries.sort();
    for (name, path) in entries {
        let is_card = name.strip_prefix("card").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        let is_render = name.strip_prefix("renderD").is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
        if !is_card && !is_render {
            continue;
        }
        let Ok(dev_dir) = std::fs::canonicalize(path.join("device")) else { continue };
        let driver = std::fs::read_link(dev_dir.join("driver"))
            .ok()
            .and_then(|p| p.file_name().map(|f| f.to_string_lossy().to_string()))
            .unwrap_or_default();
        if driver.is_empty() || NO_STATS.contains(&driver.as_str()) {
            continue;
        }
        let rdev = read_trim(&path.join("dev")).and_then(|s| {
            let (a, b) = s.split_once(':')?;
            Some(libc::makedev(a.parse().ok()?, b.parse().ok()?))
        });
        let key = dev_dir.to_string_lossy().to_string();
        if let Some(d) = out.iter_mut().find(|d| d.key == key) {
            d.rdevs.extend(rdev);
            if is_card && d.card_dir.is_none() {
                d.card_dir = Some(path);
            }
            continue;
        }
        let slot = dev_dir.file_name().map(|f| f.to_string_lossy().to_string()).unwrap_or_default();
        let pci = slot.contains(':') && dev_dir.join("vendor").exists();
        let (vendor_id, device_id) = if pci { (hex(&dev_dir.join("vendor")), hex(&dev_dir.join("device"))) } else { (None, None) };
        let vendor = match vendor_id {
            Some(0x1002) => Vendor::Amd,
            Some(0x8086) => Vendor::Intel,
            Some(0x10de) => Vendor::Nvidia,
            _ => Vendor::Other,
        };
        let name = match (vendor_id, device_id) {
            (Some(v), Some(d)) => {
                let ids = pci_ids.get_or_insert_with(|| {
                    ["/usr/share/hwdata/pci.ids", "/usr/share/misc/pci.ids"]
                        .iter()
                        .find_map(|p| std::fs::read_to_string(p).ok())
                        .unwrap_or_default()
                });
                drm::pci_name(ids, v, d).map(|n| drm::marketing_name(v, &n)).unwrap_or_else(|| {
                    format!(
                        "{} graphics ({v:04x}:{d:04x})",
                        match vendor {
                            Vendor::Amd => "AMD",
                            Vendor::Intel => "Intel",
                            _ => "PCI",
                        }
                    )
                })
            }
            _ => platform_name(&driver),
        };
        // Intel's integrated graphics sit at 00:02.0 and have no memory of their own
        let integrated = match vendor {
            Vendor::Intel => Some(slot.ends_with("00:02.0")),
            _ => None,
        };
        out.push(DrmDev {
            key,
            dev_dir,
            card_dir: is_card.then_some(path),
            rdevs: rdev.into_iter().collect(),
            driver,
            name,
            vendor,
            integrated,
            location: pci.then_some(slot),
        });
    }
    out
}

fn first_hwmon(dev_dir: &Path) -> Option<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dev_dir.join("hwmon")).ok()?.flatten().map(|e| e.path()).collect();
    v.sort();
    v.into_iter().next()
}

/// A DRM device's own numbers from its driver's files and hwmon. `energy` keeps the last energy reading (µJ) for drivers
/// that give energy instead of power.
fn read_drm(d: &DrmDev, energy: &mut HashMap<String, (u64, Instant)>, rapl: Option<&Path>) -> Device {
    let dd = &d.dev_dir;
    let mut g = Device {
        key: d.key.clone(),
        name: d.name.clone(),
        vendor: d.vendor,
        driver: d.driver.clone(),
        integrated: d.integrated,
        location: d.location.clone(),
        ..Default::default()
    };
    let pct = |f: &str| read_u64(&dd.join(f)).map(|v| v.min(100) as f32);
    match d.driver.as_str() {
        "amdgpu" => {
            g.util = pct("gpu_busy_percent");
            g.mem_ctrl = pct("mem_busy_percent");
            g.dedicated = read_u64(&dd.join("mem_info_vram_used"));
            g.dedicated_total = read_u64(&dd.join("mem_info_vram_total"));
            g.shared = read_u64(&dd.join("mem_info_gtt_used"));
            g.shared_total = read_u64(&dd.join("mem_info_gtt_total"));
            g.clock_max = read_trim(&dd.join("pp_dpm_sclk")).and_then(|s| drm::dpm_max_mhz(&s));
            g.mem_clock_max = read_trim(&dd.join("pp_dpm_mclk")).and_then(|s| drm::dpm_max_mhz(&s));
        }
        "i915" => {
            if let Some(c) = &d.card_dir {
                let mhz = |a: &str, b: &str| read_u64(&c.join(a)).or_else(|| read_u64(&c.join(b))).map(|v| v as u32);
                g.clock = mhz("gt_act_freq_mhz", "gt/gt0/rps_act_freq_mhz");
                g.clock_max = mhz("gt_RP0_freq_mhz", "gt/gt0/rps_RP0_freq_mhz");
                if let Some(total) = read_u64(&c.join("lmem_total_bytes")).filter(|&t| t > 0) {
                    g.dedicated_total = Some(total);
                    g.dedicated = read_u64(&c.join("lmem_avail_bytes")).map(|a| total.saturating_sub(a));
                }
            }
        }
        "xe" => {
            let f = dd.join("tile0/gt0/freq0");
            g.clock = read_u64(&f.join("act_freq")).map(|v| v as u32);
            g.clock_max = read_u64(&f.join("rp0_freq")).map(|v| v as u32);
        }
        _ => {}
    }
    if let Some(h) = first_hwmon(dd) {
        g.temp = read_u64(&h.join("temp1_input")).map(|m| m as f32 / 1000.0);
        let uw = |f: &str| read_u64(&h.join(f)).map(|v| v as f32 / 1e6);
        g.power = uw("power1_average").or_else(|| uw("power1_input"));
        g.power_limit = uw("power1_cap").or_else(|| uw("power1_max")).filter(|&w| w > 0.0);
        let hz = |f: &str| read_u64(&h.join(f)).map(|v| (v / 1_000_000) as u32);
        if g.clock.is_none() {
            g.clock = hz("freq1_input");
        }
        g.mem_clock = hz("freq2_input");
        g.fan_rpm = read_u64(&h.join("fan1_input")).map(|v| v as u32);
        g.fan_pct = read_u64(&h.join("pwm1")).map(|v| (v.min(255) as f32 / 255.0 * 100.0).round());
        if g.power.is_none() {
            g.power = energy_power(energy, &d.key, read_u64(&h.join("energy1_input")));
        }
    }
    // Intel's integrated graphics: the processor's graphics power domain, when the system lets everyone read it
    if g.power.is_none() && g.integrated == Some(true) {
        if let Some(r) = rapl {
            if read_trim(&r.join("name")).as_deref() == Some("uncore") {
                g.power = energy_power(energy, &format!("{}:rapl", d.key), read_u64(&r.join("energy_uj")));
            }
        }
    }
    let link = |s: &str, w: &str| {
        let gen = read_trim(&dd.join(s)).and_then(|v| drm::link_generation(&v))?;
        let width = read_u64(&dd.join(w)).filter(|&x| x > 0)? as u32;
        Some((gen, width))
    };
    g.link = link("current_link_speed", "current_link_width");
    g.link_max = link("max_link_speed", "max_link_width");
    g
}

/// Watts from two energy readings (µJ) a moment apart; None on the first.
fn energy_power(prev: &mut HashMap<String, (u64, Instant)>, key: &str, uj: Option<u64>) -> Option<f32> {
    let uj = uj?;
    let now = Instant::now();
    let out = prev.get(key).and_then(|&(e0, t0)| {
        let dt = now.duration_since(t0).as_secs_f64();
        (dt > 0.05 && uj >= e0).then(|| ((uj - e0) as f64 / 1e6 / dt) as f32)
    });
    prev.insert(key.to_string(), (uj, now));
    out
}

struct NvCard {
    dev: nvml::Device,
    key: String,
    name: String,
    location: Option<String>,
    /// The driver's time stamp of the newest process sample seen.
    last_sample: u64,
    /// The numbers that change slowly and cost the driver a few milliseconds each (power, clocks, temperature, fan),
    /// read every 2 seconds, and when.
    slow: Option<(Instant, NvSlow)>,
    /// What does not change: top clocks and the widest link.
    fixed: Option<NvFixed>,
}

/// The top core and memory clocks and the widest PCIe link.
type NvFixed = (Option<u32>, Option<u32>, Option<(u32, u32)>);

/// A process's part on one card: (card, engine, share, card memory, shared memory).
type ProcRow = (String, String, f32, Option<u64>, Option<u64>);

#[derive(Clone, Copy, Default)]
struct NvSlow {
    temp: Option<f32>,
    power: Option<f32>,
    power_limit: Option<f32>,
    clock: Option<u32>,
    mem_clock: Option<u32>,
    fan: Option<f32>,
    link: Option<(u32, u32)>,
}

struct NvState {
    lib: Nvml,
    cards: Vec<NvCard>,
    driver: Option<String>,
    cuda: Option<String>,
}

/// The DRM files a process holds: found once, looked for again when its number of open files changes.
struct FdScan {
    count: u64,
    at: Instant,
    /// (descriptor, device number)
    fds: Vec<(u32, u64)>,
}

/// A client's counters at the last look.
#[derive(Clone, Default)]
struct ClientPrev {
    engine_ns: HashMap<String, u64>,
    cycles: HashMap<String, (u64, u64)>,
}

#[derive(Default)]
pub struct GpuSampler {
    nvml: Option<Option<NvState>>,
    drm: Option<Vec<DrmDev>>,
    rdev_key: HashMap<u64, String>,
    fds: HashMap<(u32, u64), FdScan>,
    clients: HashMap<(u32, u64, String, u64), ClientPrev>,
    walked_at: Option<Instant>,
    maps: HashMap<(u32, u64), (Instant, Vec<&'static str>)>,
    energy: HashMap<String, (u64, Instant)>,
    models_at: Option<Instant>,
    models: Option<Vec<ollama::Model>>,
    rapl: Option<PathBuf>,
}

/// One process in the fdinfo walk: (device, engine label, %) per engine it used, its memory, the device it holds.
#[derive(Default)]
struct WalkProc {
    engines: Vec<(String, String, f32)>,
    dedicated: Option<u64>,
    shared: Option<u64>,
    dev: String,
}

/// What the fdinfo walk found: per process, and per device the engines and memory summed.
#[derive(Default)]
struct Walk {
    procs: HashMap<u32, WalkProc>,
    dev_engines: HashMap<String, HashMap<String, f32>>,
    dev_shared: HashMap<String, u64>,
    dev_dedicated: HashMap<String, u64>,
}

impl GpuSampler {
    fn nv(&mut self) -> Option<&mut NvState> {
        self.nvml
            .get_or_insert_with(|| {
                let lib = Nvml::load()?;
                let cards: Vec<NvCard> = lib
                    .devices()
                    .into_iter()
                    .map(|dev| {
                        let location = lib.pci_address(dev);
                        NvCard {
                            dev,
                            key: location.clone().unwrap_or_else(|| format!("nvml:{}", lib.name(dev).unwrap_or_default())),
                            name: lib.name(dev).unwrap_or_else(|| "NVIDIA graphics".into()),
                            location,
                            last_sample: 0,
                            slow: None,
                            fixed: None,
                        }
                    })
                    .collect();
                if cards.is_empty() {
                    return None;
                }
                let driver = lib.driver_version();
                let cuda = lib.cuda_version().map(|(a, b)| format!("{a}.{b}"));
                Some(NvState { lib, cards, driver, cuda })
            })
            .as_mut()
    }

    fn drm_devs(&mut self) -> &Vec<DrmDev> {
        if self.drm.is_none() {
            let mut ids = None;
            let devs = drm_devices(Path::new("/sys/class/drm"), &mut ids);
            for d in &devs {
                for r in &d.rdevs {
                    self.rdev_key.insert(*r, d.key.clone());
                }
            }
            self.rapl = Some(PathBuf::from("/sys/class/powercap/intel-rapl:0:1")).filter(|p| p.exists());
            self.drm = Some(devs);
        }
        self.drm.as_ref().unwrap()
    }

    /// Reads the DRM clients of your own processes that hold a card with usage stats.
    fn walk(&mut self, procs: &[Proc], me: u32) -> Walk {
        let mut w = Walk::default();
        if self.rdev_key.is_empty() {
            return w;
        }
        let now = Instant::now();
        let wall_ns = self.walked_at.map(|t| now.duration_since(t).as_nanos() as f64).unwrap_or(0.0);
        // a share over the minutes another tab was open would not be "now": after a gap the next look starts afresh
        let fresh = wall_ns > 0.0 && wall_ns < 5e9;
        let labels: HashMap<String, String> = self.drm.iter().flatten().map(|d| (d.key.clone(), d.driver.clone())).collect();
        let mut live_fds = HashSet::new();
        let mut live_clients = HashSet::new();
        for p in procs.iter().filter(|p| !p.kernel && (p.uid == me || me == 0)) {
            let key = (p.pid, p.started.to_bits());
            live_fds.insert(key);
            let fd_dir = format!("/proc/{}/fd", p.pid);
            // the size of /proc/PID/fd is its number of open files (Linux 6.2 and later; 0 before, then a slower recheck)
            let count = std::fs::metadata(&fd_dir).map(|m| m.len()).unwrap_or(0);
            let rescan = match self.fds.get(&key) {
                None => true,
                Some(s) => {
                    let age = now.duration_since(s.at).as_secs();
                    (count != s.count && age >= 5) || (count == 0 && age >= 30)
                }
            };
            if rescan {
                let mut fds = vec![];
                if let Ok(dir) = std::fs::read_dir(&fd_dir) {
                    for e in dir.flatten() {
                        let Ok(target) = std::fs::read_link(e.path()) else { continue };
                        if !target.starts_with("/dev/dri/") {
                            continue;
                        }
                        let (Some(fd), Ok(meta)) =
                            (e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()), std::fs::metadata(e.path()))
                        else {
                            continue;
                        };
                        if self.rdev_key.contains_key(&meta.rdev()) {
                            fds.push((fd, meta.rdev()));
                        }
                    }
                }
                self.fds.insert(key, FdScan { count, at: now, fds });
            }
            let scan = &self.fds[&key];
            if scan.fds.is_empty() {
                continue;
            }
            let mut seen: HashSet<(String, u64)> = HashSet::new();
            let entry = w.procs.entry(p.pid).or_default();
            for &(fd, rdev) in &scan.fds {
                let Some(c) = std::fs::read_to_string(format!("/proc/{}/fdinfo/{fd}", p.pid)).ok().and_then(|s| drm::parse_fdinfo(&s))
                else {
                    continue;
                };
                let dev = self.rdev_key[&rdev].clone();
                let cid = c.id.unwrap_or(u64::MAX - fd as u64);
                if entry.dev.is_empty() {
                    entry.dev = dev.clone();
                }
                if !seen.insert((dev.clone(), cid)) {
                    continue; // the same client under another descriptor
                }
                for (name, bytes) in &c.memory {
                    match drm::region(name) {
                        Some(Region::Dedicated) => {
                            *entry.dedicated.get_or_insert(0) += bytes;
                            *w.dev_dedicated.entry(dev.clone()).or_default() += bytes;
                        }
                        Some(Region::Shared) => {
                            *entry.shared.get_or_insert(0) += bytes;
                            *w.dev_shared.entry(dev.clone()).or_default() += bytes;
                        }
                        None => {}
                    }
                }
                let ckey = (p.pid, p.started.to_bits(), dev.clone(), cid);
                live_clients.insert(ckey.clone());
                let prev = self.clients.get(&ckey).cloned();
                let mut next = ClientPrev::default();
                let driver = labels.get(&dev).cloned().unwrap_or_default();
                let mut utils: HashMap<String, f32> = HashMap::new();
                for (e, ns) in &c.engine_ns {
                    let before = prev.as_ref().and_then(|pv| pv.engine_ns.get(e).copied()).filter(|_| fresh);
                    // a counter that went back keeps the larger value until it catches up (drm-usage-stats.rst)
                    next.engine_ns.insert(e.clone(), before.map_or(*ns, |b| b.max(*ns)));
                    if let Some(b) = before {
                        let cap = c.capacity.get(e).copied().unwrap_or(1);
                        *utils.entry(drm::engine_label(&driver, e)).or_default() += drm::ns_util(b, *ns, wall_ns, cap);
                    }
                }
                for (e, busy, total) in &c.cycles {
                    let before = prev.as_ref().and_then(|pv| pv.cycles.get(e).copied()).filter(|_| fresh);
                    next.cycles.insert(e.clone(), (*busy, *total));
                    if c.engine_ns.iter().any(|(n, _)| n == e) {
                        continue; // counted by its busy time already
                    }
                    if let Some(b) = before {
                        let cap = c.capacity.get(e).copied().unwrap_or(1);
                        *utils.entry(drm::engine_label(&driver, e)).or_default() += drm::cycles_util(b, (*busy, *total), cap);
                    }
                }
                self.clients.insert(ckey, next);
                for (label, u) in utils {
                    let u = u.min(100.0);
                    *w.dev_engines.entry(dev.clone()).or_default().entry(label.clone()).or_default() += u;
                    entry.engines.push((dev.clone(), label, u));
                }
            }
        }
        self.fds.retain(|k, _| live_fds.contains(k));
        self.clients.retain(|k, _| live_clients.contains(k));
        w.procs.retain(|_, v| !v.engines.is_empty() || v.dedicated.is_some() || v.shared.is_some());
        self.walked_at = Some(now);
        w
    }

    /// The graphics cards and processes now, as much as `want` asks for.
    pub fn sample(&mut self, procs: &[Proc], me: u32, want: Want) -> Snapshot {
        let mut snap = Snapshot::default();
        if !want.devices && !want.procs {
            return snap;
        }
        let have_drm = !self.drm_devs().is_empty();
        // engine shares need two looks: until then an engine is unknown, not idle
        let second_look = self.walked_at.is_some_and(|t| t.elapsed().as_secs_f64() < 5.0);
        let walk = if have_drm { self.walk(procs, me) } else { Walk::default() };
        let mut devices: Vec<Device> = vec![];
        let mut per_proc: HashMap<u32, Vec<ProcRow>> = HashMap::new();

        // NVIDIA, through NVML
        if let Some(nv) = self.nv() {
            let (driver, cuda) = (nv.driver.clone(), nv.cuda.clone());
            for card in nv.cards.iter_mut() {
                let lib = &nv.lib;
                let d = card.dev;
                let mut g = Device {
                    key: card.key.clone(),
                    name: card.name.clone(),
                    vendor: Vendor::Nvidia,
                    driver: "nvidia".into(),
                    integrated: Some(false),
                    driver_version: driver.clone(),
                    cuda: cuda.clone(),
                    location: card.location.clone(),
                    ..Default::default()
                };
                if let Some((u, m)) = lib.utilization(d) {
                    g.util = Some(u.min(100) as f32);
                    g.mem_ctrl = Some(m.min(100) as f32);
                    g.engines.push(("3D/Compute".into(), u.min(100) as f32));
                }
                if let Some((used, total)) = lib.memory(d) {
                    g.dedicated = Some(used);
                    g.dedicated_total = Some(total);
                }
                if want.devices {
                    let (enc, dec) = lib.codecs(d);
                    g.engines.extend(enc.map(|v| ("Video encode".to_string(), v.min(100) as f32)));
                    g.engines.extend(dec.map(|v| ("Video decode".to_string(), v.min(100) as f32)));
                    let fixed = *card.fixed.get_or_insert_with(|| {
                        (lib.max_clock(d, nvml::CLOCK_GRAPHICS), lib.max_clock(d, nvml::CLOCK_MEM), lib.max_pcie_link(d))
                    });
                    (g.clock_max, g.mem_clock_max, g.link_max) = fixed;
                    let slow = match card.slow {
                        Some((at, s)) if at.elapsed().as_millis() < 1900 => s,
                        _ => {
                            let s = NvSlow {
                                temp: lib.temperature(d).map(|t| t as f32),
                                power: lib.power(d).map(|mw| mw as f32 / 1000.0),
                                power_limit: lib.power_limit(d).map(|mw| mw as f32 / 1000.0),
                                clock: lib.clock(d, nvml::CLOCK_GRAPHICS),
                                mem_clock: lib.clock(d, nvml::CLOCK_MEM),
                                fan: lib.fan(d).map(|f| f.min(100) as f32),
                                link: lib.pcie_link(d),
                            };
                            card.slow = Some((Instant::now(), s));
                            s
                        }
                    };
                    g.temp = slow.temp;
                    g.power = slow.power;
                    g.power_limit = slow.power_limit;
                    g.clock = slow.clock;
                    g.mem_clock = slow.mem_clock;
                    g.fan_pct = slow.fan;
                    g.link = slow.link;
                    if want.page {
                        (g.pcie_tx, g.pcie_rx) = lib.pcie_throughput(d);
                    }
                }
                if want.procs {
                    let held = lib.processes(d);
                    let utils = lib.process_utilization(d, card.last_sample);
                    if let Some((_, newest)) = &utils {
                        card.last_sample = *newest;
                    }
                    let utils: HashMap<u32, nvml::ProcUtil> =
                        utils.map(|u| u.0.into_iter().map(|x| (x.pid, x)).collect()).unwrap_or_default();
                    let mut pids: Vec<u32> = held.iter().map(|h| h.0).collect();
                    pids.extend(utils.keys().copied().filter(|p| !held.iter().any(|h| h.0 == *p)));
                    for pid in pids {
                        let mem = held.iter().find(|h| h.0 == pid).and_then(|h| h.1);
                        let (label, u) = match utils.get(&pid) {
                            Some(x) => [("3D/Compute", x.sm), ("Video encode", x.enc), ("Video decode", x.dec)]
                                .into_iter()
                                .fold(("3D/Compute", 0.0f32), |a, b| if b.1 > a.1 { b } else { a }),
                            None => ("3D/Compute", 0.0),
                        };
                        per_proc.entry(pid).or_default().push((g.key.clone(), label.into(), u, mem, None));
                    }
                }
                devices.push(g);
            }
        }

        // the others, through /sys and fdinfo
        let drm_devs: Vec<DrmDev> = self.drm.clone().unwrap_or_default();
        let rapl = self.rapl.clone();
        for d in &drm_devs {
            if devices.iter().any(|g| g.location.is_some() && g.location == d.location) {
                continue; // an NVIDIA card NVML already gave
            }
            let mut g = read_drm(d, &mut self.energy, rapl.as_deref());
            let mut engines: HashMap<String, f32> = walk.dev_engines.get(&d.key).cloned().unwrap_or_default();
            if second_look {
                // the engines this kind of card has, so an idle one shows as idle rather than missing
                let known: &[&str] = match d.driver.as_str() {
                    "amdgpu" => &["3D", "Compute", "Copy", "Video encode", "Video decode"],
                    "i915" => &["3D", "Copy", "Video codec", "Video processing"],
                    "xe" => &["3D", "Compute", "Copy", "Video codec", "Video processing"],
                    _ => &[],
                };
                for k in known {
                    engines.entry(k.to_string()).or_default();
                }
            }
            if !engines.is_empty() {
                let mut v: Vec<(String, f32)> = engines.into_iter().map(|(k, u)| (k, u.min(100.0))).collect();
                v.sort_by(|a, b| drm::engine_rank(&a.0).cmp(&drm::engine_rank(&b.0)).then(a.0.cmp(&b.0)));
                g.engines = v;
                g.engines_yours = true;
                if g.util.is_none() {
                    g.util = g.engines.iter().map(|e| e.1).reduce(f32::max);
                }
            }
            // without the driver's own counters, what your programs hold
            if g.shared.is_none() && g.dedicated.is_none() {
                g.shared = walk.dev_shared.get(&d.key).copied();
                g.dedicated = walk.dev_dedicated.get(&d.key).copied();
                g.mem_yours = g.shared.is_some() || g.dedicated.is_some();
            }
            if g.readable() {
                devices.push(g);
            }
        }
        for (pid, wp) in walk.procs {
            let e = per_proc.entry(pid).or_default();
            // memory once per process, on its own row
            e.push((wp.dev, String::new(), 0.0, wp.dedicated, wp.shared));
            for (dev, label, u) in wp.engines {
                e.push((dev, label, u, None, None));
            }
        }

        // number the cards as Windows does: by where they sit (an integrated chip at 00:02.0 comes first)
        devices.sort_by(|a, b| match (&a.location, &b.location) {
            (Some(x), Some(y)) => x.cmp(y),
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            _ => a.key.cmp(&b.key),
        });
        snap.per_process = devices.iter().any(|g| g.vendor == Vendor::Nvidia || g.engines_yours);

        if want.procs {
            let index: HashMap<&str, usize> = devices.iter().enumerate().map(|(i, g)| (g.key.as_str(), i)).collect();
            let started: HashMap<u32, (u64, u32)> = procs.iter().map(|p| (p.pid, (p.started.to_bits(), p.uid))).collect();
            let now = Instant::now();
            let mut live = HashSet::new();
            for (pid, rows) in per_proc {
                let mut pg = ProcGpu::default();
                for (dev, label, u, ded, shared) in rows {
                    if ded.is_some() {
                        pg.dedicated = Some(pg.dedicated.unwrap_or(0) + ded.unwrap_or(0));
                    }
                    if shared.is_some() {
                        pg.shared = Some(pg.shared.unwrap_or(0) + shared.unwrap_or(0));
                    }
                    if !label.is_empty() && (pg.engine.is_empty() || u > pg.util) {
                        pg.util = u;
                        if let Some(i) = index.get(dev.as_str()) {
                            pg.engine = format!("GPU {i} - {label}");
                        }
                    }
                }
                // the APIs, for your own processes (another account's maps are not readable)
                if let Some(&(start, uid)) = started.get(&pid) {
                    if uid == me || me == 0 {
                        let key = (pid, start);
                        live.insert(key);
                        let stale = self.maps.get(&key).is_none_or(|(at, _)| now.duration_since(*at).as_secs() >= 30);
                        if stale {
                            let apis = std::fs::read_to_string(format!("/proc/{pid}/maps")).map(|m| apis_from_maps(&m)).unwrap_or_default();
                            self.maps.insert(key, (now, apis));
                        }
                        pg.apis = self.maps[&key].1.clone();
                    }
                }
                snap.procs.insert(pid, pg);
            }
            self.maps.retain(|k, _| live.contains(k));
        }

        if want.page {
            let due = self.models_at.is_none_or(|t| t.elapsed().as_secs() >= 5);
            if due {
                self.models = ollama::loaded();
                self.models_at = Some(Instant::now());
            }
            snap.models = self.models.clone();
        }
        snap.devices = devices;
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(files: &[(&str, &str)]) -> PathBuf {
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "ygg-tm-gpu-{}-{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        for (p, content) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, content).unwrap();
        }
        root
    }

    #[test]
    fn amdgpu_sysfs_fixture() {
        // the files amdgpu and its hwmon write (Documentation/gpu/amdgpu/thermal.rst, driver-misc.rst), values made up
        let root = tree(&[
            ("dev/gpu_busy_percent", "37\n"),
            ("dev/mem_busy_percent", "12\n"),
            ("dev/mem_info_vram_used", "2147483648\n"),
            ("dev/mem_info_vram_total", "17163091968\n"),
            ("dev/mem_info_gtt_used", "104857600\n"),
            ("dev/mem_info_gtt_total", "33554432000\n"),
            ("dev/pp_dpm_sclk", "0: 500Mhz \n1: 2105Mhz *\n2: 2575Mhz \n"),
            ("dev/pp_dpm_mclk", "0: 96Mhz \n3: 1000Mhz *\n"),
            ("dev/current_link_speed", "16.0 GT/s PCIe\n"),
            ("dev/current_link_width", "16\n"),
            ("dev/max_link_speed", "16.0 GT/s PCIe\n"),
            ("dev/max_link_width", "16\n"),
            ("dev/hwmon/hwmon3/temp1_input", "54000\n"),
            ("dev/hwmon/hwmon3/power1_average", "187000000\n"),
            ("dev/hwmon/hwmon3/power1_cap", "255000000\n"),
            ("dev/hwmon/hwmon3/freq1_input", "2105000000\n"),
            ("dev/hwmon/hwmon3/freq2_input", "1000000000\n"),
            ("dev/hwmon/hwmon3/fan1_input", "1450\n"),
            ("dev/hwmon/hwmon3/pwm1", "102\n"),
        ]);
        let d = DrmDev { key: "k".into(), dev_dir: root.join("dev"), driver: "amdgpu".into(), vendor: Vendor::Amd, ..Default::default() };
        let g = read_drm(&d, &mut HashMap::new(), None);
        assert_eq!((g.util, g.mem_ctrl), (Some(37.0), Some(12.0)));
        assert_eq!((g.dedicated, g.dedicated_total), (Some(2 << 30), Some(17163091968)));
        assert_eq!((g.shared, g.shared_total), (Some(100 << 20), Some(33554432000)));
        assert_eq!((g.temp, g.power, g.power_limit), (Some(54.0), Some(187.0), Some(255.0)));
        assert_eq!((g.clock, g.clock_max, g.mem_clock, g.mem_clock_max), (Some(2105), Some(2575), Some(1000), Some(1000)));
        assert_eq!((g.fan_rpm, g.fan_pct), (Some(1450), Some(40.0)));
        assert_eq!((g.link, g.link_max), (Some((4, 16)), Some((4, 16))));
        assert!(g.readable());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn intel_sysfs_fixtures() {
        // i915 integrated: frequencies in the card's folder, no memory of its own, no sensors, no PCIe link widths
        let root = tree(&[("card/gt_act_freq_mhz", "350\n"), ("card/gt_RP0_freq_mhz", "1300\n"), ("dev/vendor", "0x8086\n")]);
        let d = DrmDev {
            key: "k".into(),
            dev_dir: root.join("dev"),
            card_dir: Some(root.join("card")),
            driver: "i915".into(),
            vendor: Vendor::Intel,
            integrated: Some(true),
            ..Default::default()
        };
        let g = read_drm(&d, &mut HashMap::new(), None);
        assert_eq!((g.clock, g.clock_max), (Some(350), Some(1300)));
        assert_eq!((g.util, g.dedicated, g.temp, g.power, g.link), (None, None, None, None, None), "nothing made up");
        // xe: frequencies per tile and GT; energy instead of power (two readings needed)
        let root2 = tree(&[
            ("dev/tile0/gt0/freq0/act_freq", "1950\n"),
            ("dev/tile0/gt0/freq0/rp0_freq", "2400\n"),
            ("dev/hwmon/hwmon5/energy1_input", "1000000\n"),
        ]);
        let d2 = DrmDev { key: "x".into(), dev_dir: root2.join("dev"), driver: "xe".into(), vendor: Vendor::Intel, ..Default::default() };
        let mut energy = HashMap::new();
        let g2 = read_drm(&d2, &mut energy, None);
        assert_eq!((g2.clock, g2.clock_max, g2.power), (Some(1950), Some(2400), None));
        // a second reading 1 J later, a moment after the first
        energy.insert("x".into(), (0, Instant::now() - std::time::Duration::from_millis(500)));
        let g3 = read_drm(&d2, &mut energy, None);
        assert!(g3.power.is_some_and(|w| w > 1.0 && w < 3.0), "about 2 W: {:?}", g3.power);
        let _ = std::fs::remove_dir_all(root);
        let _ = std::fs::remove_dir_all(root2);
    }

    #[test]
    fn a_device_with_nothing_readable_is_hidden() {
        let root = tree(&[("dev/vendor", "0x1af4\n")]);
        let d = DrmDev { key: "v".into(), dev_dir: root.join("dev"), driver: "virtio_gpu".into(), ..Default::default() };
        assert!(!read_drm(&d, &mut HashMap::new(), None).readable());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn drm_class_fixture() {
        // /sys/class/drm with an Intel card (card0 + renderD128 of one device) and an NVIDIA one (left to NVML)
        let root = tree(&[
            ("devices/0000:00:02.0/vendor", "0x8086\n"),
            ("devices/0000:00:02.0/device", "0x46a6\n"),
            ("devices/0000:01:00.0/vendor", "0x10de\n"),
            ("devices/0000:01:00.0/device", "0x1e07\n"),
            ("drivers/i915/.keep", ""),
            ("drivers/nvidia/.keep", ""),
            ("class/card0/dev", "226:0\n"),
            ("class/renderD128/dev", "226:128\n"),
            ("class/card1/dev", "226:1\n"),
            ("class/card0-eDP-1/dev", "226:9\n"),
        ]);
        let link = |from: &str, to: &str| std::os::unix::fs::symlink(root.join(to), root.join(from)).unwrap();
        link("class/card0/device", "devices/0000:00:02.0");
        link("class/renderD128/device", "devices/0000:00:02.0");
        link("class/card1/device", "devices/0000:01:00.0");
        link("devices/0000:00:02.0/driver", "drivers/i915");
        link("devices/0000:01:00.0/driver", "drivers/nvidia");
        let mut ids = Some("8086  Intel Corporation\n\t46a6  Alder Lake-P GT2 [Iris Xe Graphics]\n".to_string());
        let devs = drm_devices(&root.join("class"), &mut ids);
        assert_eq!(devs.len(), 1, "the NVIDIA card is NVML's; a connector is not a device");
        let d = &devs[0];
        assert_eq!((d.driver.as_str(), d.name.as_str(), d.vendor), ("i915", "Intel Iris Xe Graphics", Vendor::Intel));
        assert_eq!((d.integrated, d.location.as_deref()), (Some(true), Some("0000:00:02.0")));
        assert_eq!(d.rdevs.len(), 2, "card and render node");
        assert!(d.card_dir.as_ref().is_some_and(|c| c.ends_with("card0")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn apis_from_libraries() {
        let maps = "7f00-7f01 r-xp 00000000 103:02 1 /usr/lib/libcuda.so.620.70.06\n\
                    7f02-7f03 r-xp 00000000 103:02 2 /usr/lib/libvulkan.so.1.4.300\n\
                    7f04-7f05 r--p 00000000 103:02 3 /usr/lib/libEGL_nvidia.so.0\n\
                    7f06-7f07 r-xp 00000000 103:02 4 /opt/app/lib/libggml-cuda.so\n\
                    7f08-7f09 rw-p 00000000 00:00 0 \n\
                    7f0a-7f0b r-xp 00000000 103:02 5 /usr/lib/libc.so.6\n";
        assert_eq!(apis_from_maps(maps), vec!["CUDA", "Vulkan", "OpenGL", "ggml"]);
        assert!(apis_from_maps("").is_empty());
    }

    #[test]
    fn a_real_look() {
        // whatever this machine has: never a card with nothing readable, and nothing when no tab asks
        let me = unsafe { libc::getuid() };
        let mut ps = crate::procs::Sampler::new();
        let procs = ps.sample();
        let mut s = GpuSampler::default();
        assert!(s.sample(&procs, me, Want::default()).devices.is_empty());
        let snap = s.sample(&procs, me, Want { devices: true, procs: true, page: false });
        assert!(snap.devices.iter().all(|d| d.readable()));
        for p in snap.procs.values() {
            assert!(p.util <= 100.0);
        }
    }
}
