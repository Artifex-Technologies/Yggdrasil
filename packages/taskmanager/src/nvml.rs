//! A thin, read-only binding to NVIDIA's management library (NVML, libnvidia-ml.so.1), opened with dlopen the first time an
//! NVIDIA card is looked at, so the Task Manager needs no NVIDIA package to build or start and loads nothing on a machine
//! without one. Only queries are bound: nothing here can change a clock, a power limit, persistence or accounting mode.
//!
//! The structures follow nvml.h (the _v2/_v3 entry points every driver since 2021 has).

use std::ffi::{c_char, c_int, c_uint, c_ulonglong, c_void, CStr};

type Ret = c_int;
type Dev = *mut c_void;

const SUCCESS: Ret = 0;
const ERROR_NOT_FOUND: Ret = 6;
const ERROR_INSUFFICIENT_SIZE: Ret = 7;
/// usedGpuMemory when the driver cannot say (NVML_VALUE_NOT_AVAILABLE).
const NOT_AVAILABLE: u64 = u64::MAX;

pub const CLOCK_GRAPHICS: c_uint = 0;
pub const CLOCK_MEM: c_uint = 2;

#[repr(C)]
#[derive(Default)]
struct Utilization {
    gpu: c_uint,
    memory: c_uint,
}

#[repr(C)]
#[derive(Default)]
struct Memory {
    total: c_ulonglong,
    free: c_ulonglong,
    used: c_ulonglong,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct ProcessInfo {
    pid: c_uint,
    used_gpu_memory: c_ulonglong,
    gpu_instance_id: c_uint,
    compute_instance_id: c_uint,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct UtilSample {
    pid: c_uint,
    time_stamp: c_ulonglong,
    sm: c_uint,
    mem: c_uint,
    enc: c_uint,
    dec: c_uint,
}

#[repr(C)]
struct PciInfo {
    bus_id_legacy: [c_char; 16],
    domain: c_uint,
    bus: c_uint,
    device: c_uint,
    pci_device_id: c_uint,
    pci_subsystem_id: c_uint,
    bus_id: [c_char; 32],
}

type FnInit = unsafe extern "C" fn() -> Ret;
type FnCount = unsafe extern "C" fn(*mut c_uint) -> Ret;
type FnHandle = unsafe extern "C" fn(c_uint, *mut Dev) -> Ret;
type FnStr = unsafe extern "C" fn(Dev, *mut c_char, c_uint) -> Ret;
type FnSysStr = unsafe extern "C" fn(*mut c_char, c_uint) -> Ret;
type FnSysInt = unsafe extern "C" fn(*mut c_int) -> Ret;
type FnUtil = unsafe extern "C" fn(Dev, *mut Utilization) -> Ret;
type FnMem = unsafe extern "C" fn(Dev, *mut Memory) -> Ret;
type FnUint = unsafe extern "C" fn(Dev, *mut c_uint) -> Ret;
type FnUintArg = unsafe extern "C" fn(Dev, c_uint, *mut c_uint) -> Ret;
type FnCodec = unsafe extern "C" fn(Dev, *mut c_uint, *mut c_uint) -> Ret;
type FnProcs = unsafe extern "C" fn(Dev, *mut c_uint, *mut ProcessInfo) -> Ret;
type FnSamples = unsafe extern "C" fn(Dev, *mut UtilSample, *mut c_uint, c_ulonglong) -> Ret;
type FnPci = unsafe extern "C" fn(Dev, *mut PciInfo) -> Ret;

/// The library's entry points that were found (an older driver may lack some: those numbers are then not shown).
pub struct Nvml {
    count: FnCount,
    handle: FnHandle,
    name: Option<FnStr>,
    driver_version: Option<FnSysStr>,
    cuda_version: Option<FnSysInt>,
    utilization: Option<FnUtil>,
    memory: Option<FnMem>,
    temperature: Option<FnUintArg>,
    power: Option<FnUint>,
    power_limit: Option<FnUint>,
    clock: Option<FnUintArg>,
    max_clock: Option<FnUintArg>,
    fan: Option<FnUint>,
    encoder: Option<FnCodec>,
    decoder: Option<FnCodec>,
    pcie_throughput: Option<FnUintArg>,
    link_gen: Option<FnUint>,
    link_width: Option<FnUint>,
    max_link_gen: Option<FnUint>,
    max_link_width: Option<FnUint>,
    compute_procs: Option<FnProcs>,
    graphics_procs: Option<FnProcs>,
    samples: Option<FnSamples>,
    pci: Option<FnPci>,
}

// The library is thread safe (nvml.h); the handles are plain pointers it owns for the life of the process.
unsafe impl Send for Nvml {}

/// A card, by its NVML handle.
#[derive(Clone, Copy)]
pub struct Device(Dev);
unsafe impl Send for Device {}

/// What one process did on a card since the last look (percent of the sampling period).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcUtil {
    pub pid: u32,
    pub sm: f32,
    pub mem: f32,
    pub enc: f32,
    pub dec: f32,
}

fn sym<T: Copy>(lib: *mut c_void, name: &CStr) -> Option<T> {
    let p = unsafe { libc::dlsym(lib, name.as_ptr()) };
    if p.is_null() {
        None
    } else {
        // SAFETY: T is the function pointer type nvml.h declares for this symbol
        Some(unsafe { std::mem::transmute_copy::<*mut c_void, T>(&p) })
    }
}

fn text(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

impl Nvml {
    /// Opens and initialises the library; None when there is no NVIDIA driver (or no card).
    pub fn load() -> Option<Nvml> {
        let lib = unsafe { libc::dlopen(c"libnvidia-ml.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if lib.is_null() {
            return None;
        }
        let init: FnInit = sym(lib, c"nvmlInit_v2")?;
        if unsafe { init() } != SUCCESS {
            return None;
        }
        Some(Nvml {
            count: sym(lib, c"nvmlDeviceGetCount_v2")?,
            handle: sym(lib, c"nvmlDeviceGetHandleByIndex_v2")?,
            name: sym(lib, c"nvmlDeviceGetName"),
            driver_version: sym(lib, c"nvmlSystemGetDriverVersion"),
            cuda_version: sym(lib, c"nvmlSystemGetCudaDriverVersion_v2"),
            utilization: sym(lib, c"nvmlDeviceGetUtilizationRates"),
            memory: sym(lib, c"nvmlDeviceGetMemoryInfo"),
            temperature: sym(lib, c"nvmlDeviceGetTemperature"),
            power: sym(lib, c"nvmlDeviceGetPowerUsage"),
            power_limit: sym(lib, c"nvmlDeviceGetEnforcedPowerLimit"),
            clock: sym(lib, c"nvmlDeviceGetClockInfo"),
            max_clock: sym(lib, c"nvmlDeviceGetMaxClockInfo"),
            fan: sym(lib, c"nvmlDeviceGetFanSpeed"),
            encoder: sym(lib, c"nvmlDeviceGetEncoderUtilization"),
            decoder: sym(lib, c"nvmlDeviceGetDecoderUtilization"),
            pcie_throughput: sym(lib, c"nvmlDeviceGetPcieThroughput"),
            link_gen: sym(lib, c"nvmlDeviceGetCurrPcieLinkGeneration"),
            link_width: sym(lib, c"nvmlDeviceGetCurrPcieLinkWidth"),
            max_link_gen: sym(lib, c"nvmlDeviceGetMaxPcieLinkGeneration"),
            max_link_width: sym(lib, c"nvmlDeviceGetMaxPcieLinkWidth"),
            compute_procs: sym(lib, c"nvmlDeviceGetComputeRunningProcesses_v3"),
            graphics_procs: sym(lib, c"nvmlDeviceGetGraphicsRunningProcesses_v3"),
            samples: sym(lib, c"nvmlDeviceGetProcessUtilization"),
            pci: sym(lib, c"nvmlDeviceGetPciInfo_v3"),
        })
    }

    pub fn devices(&self) -> Vec<Device> {
        let mut n: c_uint = 0;
        if unsafe { (self.count)(&mut n) } != SUCCESS {
            return vec![];
        }
        (0..n)
            .filter_map(|i| {
                let mut d: Dev = std::ptr::null_mut();
                (unsafe { (self.handle)(i, &mut d) } == SUCCESS && !d.is_null()).then_some(Device(d))
            })
            .collect()
    }

    fn uint(&self, f: Option<FnUint>, d: Device) -> Option<u32> {
        let mut v: c_uint = 0;
        (unsafe { f?(d.0, &mut v) } == SUCCESS).then_some(v)
    }

    fn uint_arg(&self, f: Option<FnUintArg>, d: Device, arg: c_uint) -> Option<u32> {
        let mut v: c_uint = 0;
        (unsafe { f?(d.0, arg, &mut v) } == SUCCESS).then_some(v)
    }

    pub fn name(&self, d: Device) -> Option<String> {
        let mut buf = [0 as c_char; 96];
        (unsafe { self.name?(d.0, buf.as_mut_ptr(), buf.len() as c_uint) } == SUCCESS).then(|| text(&buf))
    }

    pub fn driver_version(&self) -> Option<String> {
        let mut buf = [0 as c_char; 80];
        (unsafe { self.driver_version?(buf.as_mut_ptr(), buf.len() as c_uint) } == SUCCESS).then(|| text(&buf))
    }

    /// The CUDA version the driver supports, e.g. (13, 0).
    pub fn cuda_version(&self) -> Option<(i32, i32)> {
        let mut v: c_int = 0;
        (unsafe { self.cuda_version?(&mut v) } == SUCCESS && v > 0).then_some((v / 1000, v % 1000 / 10))
    }

    /// (graphics/compute busy %, memory controller busy %).
    pub fn utilization(&self, d: Device) -> Option<(u32, u32)> {
        let mut u = Utilization::default();
        (unsafe { self.utilization?(d.0, &mut u) } == SUCCESS).then_some((u.gpu, u.memory))
    }

    /// (used, total) bytes of the card's own memory.
    pub fn memory(&self, d: Device) -> Option<(u64, u64)> {
        let mut m = Memory::default();
        (unsafe { self.memory?(d.0, &mut m) } == SUCCESS).then_some((m.used, m.total))
    }

    /// Degrees Celsius of the GPU's own sensor.
    pub fn temperature(&self, d: Device) -> Option<u32> {
        self.uint_arg(self.temperature, d, 0)
    }

    /// Milliwatts.
    pub fn power(&self, d: Device) -> Option<u32> {
        self.uint(self.power, d)
    }

    /// Milliwatts.
    pub fn power_limit(&self, d: Device) -> Option<u32> {
        self.uint(self.power_limit, d)
    }

    /// MHz now, for CLOCK_GRAPHICS or CLOCK_MEM.
    pub fn clock(&self, d: Device, which: c_uint) -> Option<u32> {
        self.uint_arg(self.clock, d, which)
    }

    /// MHz at most.
    pub fn max_clock(&self, d: Device, which: c_uint) -> Option<u32> {
        self.uint_arg(self.max_clock, d, which)
    }

    /// Percent of the fan's top speed (cards without a fan say not supported).
    pub fn fan(&self, d: Device) -> Option<u32> {
        self.uint(self.fan, d)
    }

    /// (video encoder %, video decoder %).
    pub fn codecs(&self, d: Device) -> (Option<u32>, Option<u32>) {
        let get = |f: Option<FnCodec>| {
            let (mut u, mut period) = (0 as c_uint, 0 as c_uint);
            (unsafe { f?(d.0, &mut u, &mut period) } == SUCCESS).then_some(u)
        };
        (get(self.encoder), get(self.decoder))
    }

    /// (sent, received) over PCIe in bytes per second. The driver measures for about 20 ms per call, so this is asked only
    /// while the card's own page is open.
    pub fn pcie_throughput(&self, d: Device) -> (Option<f64>, Option<f64>) {
        let kb = |counter| self.uint_arg(self.pcie_throughput, d, counter).map(|k| k as f64 * 1024.0);
        (kb(0), kb(1))
    }

    /// (generation, width) now: a card drops to a slower link when idle.
    pub fn pcie_link(&self, d: Device) -> Option<(u32, u32)> {
        self.uint(self.link_gen, d).zip(self.uint(self.link_width, d))
    }

    /// (generation, width) at most.
    pub fn max_pcie_link(&self, d: Device) -> Option<(u32, u32)> {
        self.uint(self.max_link_gen, d).zip(self.uint(self.max_link_width, d))
    }

    /// The PCI address, as the kernel writes it (0000:01:00.0).
    pub fn pci_address(&self, d: Device) -> Option<String> {
        let mut p =
            PciInfo { bus_id_legacy: [0; 16], domain: 0, bus: 0, device: 0, pci_device_id: 0, pci_subsystem_id: 0, bus_id: [0; 32] };
        (unsafe { self.pci?(d.0, &mut p) } == SUCCESS).then(|| pci_address(&text(&p.bus_id)))
    }

    /// Every process holding the card (compute and graphics lists together) with the card memory it uses, when the
    /// driver says. The open kernel driver lists every program that holds the card as a compute process, so the list
    /// says only "holds the card", not what for.
    pub fn processes(&self, d: Device) -> Vec<(u32, Option<u64>)> {
        let mut out: Vec<(u32, Option<u64>)> = vec![];
        for f in [self.compute_procs, self.graphics_procs].into_iter().flatten() {
            let mut n: c_uint = 0;
            let r = unsafe { f(d.0, &mut n, std::ptr::null_mut()) };
            if r == SUCCESS || r != ERROR_INSUFFICIENT_SIZE {
                continue; // none, or not supported
            }
            // room for a few that start between the two calls
            let mut buf = vec![ProcessInfo::default(); n as usize + 8];
            let mut n = buf.len() as c_uint;
            if unsafe { f(d.0, &mut n, buf.as_mut_ptr()) } != SUCCESS {
                continue;
            }
            for p in buf.iter().take(n as usize) {
                let mem = (p.used_gpu_memory != NOT_AVAILABLE).then_some(p.used_gpu_memory);
                match out.iter_mut().find(|(pid, _)| *pid == p.pid) {
                    Some(e) => e.1 = e.1.max(mem),
                    None => out.push((p.pid, mem)),
                }
            }
        }
        out
    }

    /// What each process did since `since` (the driver's microsecond time stamp; 0 = whatever it still has), averaged per
    /// process, and the newest time stamp seen. None when the driver cannot say.
    pub fn process_utilization(&self, d: Device, since: u64) -> Option<(Vec<ProcUtil>, u64)> {
        let f = self.samples?;
        let mut n: c_uint = 0;
        let r = unsafe { f(d.0, std::ptr::null_mut(), &mut n, since) };
        if r == ERROR_NOT_FOUND {
            return Some((vec![], since)); // nothing ran since
        }
        if r != ERROR_INSUFFICIENT_SIZE && r != SUCCESS {
            return None;
        }
        if n == 0 {
            return Some((vec![], since));
        }
        let mut buf = vec![UtilSample::default(); n as usize + 8];
        let mut n = buf.len() as c_uint;
        match unsafe { f(d.0, buf.as_mut_ptr(), &mut n, since) } {
            SUCCESS => {}
            ERROR_NOT_FOUND => return Some((vec![], since)),
            _ => return None,
        }
        let samples: Vec<(u32, u64, [u32; 4])> =
            buf.iter().take(n as usize).map(|s| (s.pid, s.time_stamp, [s.sm, s.mem, s.enc, s.dec])).collect();
        Some(average_samples(&samples, since))
    }
}

/// Averages the samples of each process (pid, time stamp, [sm, mem, enc, dec]); returns them and the newest time stamp.
pub fn average_samples(samples: &[(u32, u64, [u32; 4])], since: u64) -> (Vec<ProcUtil>, u64) {
    let mut sums: Vec<(u32, [u64; 4], u32)> = vec![];
    let mut newest = since;
    for &(pid, ts, v) in samples {
        newest = newest.max(ts);
        // a percentage above 100 is a driver's placeholder for "unknown": left out
        let v = v.map(|x| x.min(100) as u64);
        match sums.iter_mut().find(|s| s.0 == pid) {
            Some(s) => {
                for (sum, x) in s.1.iter_mut().zip(v) {
                    *sum += x;
                }
                s.2 += 1;
            }
            None => sums.push((pid, v, 1)),
        }
    }
    let out = sums
        .into_iter()
        .map(|(pid, s, n)| {
            let avg = |i: usize| s[i] as f32 / n as f32;
            ProcUtil { pid, sm: avg(0), mem: avg(1), enc: avg(2), dec: avg(3) }
        })
        .collect();
    (out, newest)
}

/// NVML writes the PCI domain with eight digits ("00000000:01:00.0"); the kernel's sysfs with four, in lower case.
pub fn pci_address(bus_id: &str) -> String {
    let s = bus_id.trim().to_lowercase();
    match s.split_once(':') {
        Some((dom, rest)) if dom.len() > 4 => format!("{}:{rest}", &dom[dom.len() - 4..]),
        _ => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_match_nvml_h() {
        assert_eq!(std::mem::size_of::<ProcessInfo>(), 24);
        assert_eq!(std::mem::size_of::<UtilSample>(), 32);
        assert_eq!(std::mem::size_of::<PciInfo>(), 68);
        assert_eq!(std::mem::size_of::<Memory>(), 24);
    }

    #[test]
    fn samples_average_per_process() {
        let (v, newest) = average_samples(&[(10, 5, [20, 4, 0, 0]), (10, 9, [40, 6, 0, 0]), (11, 7, [0, 0, 50, 999])], 3);
        assert_eq!(newest, 9);
        let a = v.iter().find(|p| p.pid == 10).unwrap();
        assert_eq!((a.sm, a.mem), (30.0, 5.0));
        let b = v.iter().find(|p| p.pid == 11).unwrap();
        assert_eq!((b.enc, b.dec), (50.0, 100.0), "out-of-range values are capped");
        assert_eq!(average_samples(&[], 42).1, 42);
    }

    #[test]
    fn bus_ids() {
        assert_eq!(pci_address("00000000:01:00.0"), "0000:01:00.0");
        assert_eq!(pci_address("0000:0A:00.0"), "0000:0a:00.0");
    }

    #[test]
    fn loads_or_says_none() {
        // on a machine without the NVIDIA driver this is None; with it, every card has a name
        if let Some(n) = Nvml::load() {
            for d in n.devices() {
                assert!(n.name(d).is_some_and(|s| !s.is_empty()));
            }
        }
    }
}
