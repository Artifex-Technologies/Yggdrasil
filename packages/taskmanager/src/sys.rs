//! The whole machine: processor (and each core), memory, disks, network adapters and the graphics card, for the Performance
//! tab. Everything comes from /proc and /sys except an NVIDIA card's numbers, which only its driver's nvidia-smi gives (read
//! only while the Performance tab is open: it costs a few hundredths of a second each time).

use std::collections::HashMap;
use std::time::Instant;

fn read(path: &str) -> Option<String> {
    std::fs::read_to_string(path).ok()
}

/// (busy, total) jiffies of one `cpu` line of /proc/stat.
pub fn cpu_line(line: &str) -> Option<(u64, u64)> {
    let v: Vec<u64> = line.split_whitespace().skip(1).filter_map(|x| x.parse().ok()).collect();
    if v.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal (guest time is already inside user)
    let total: u64 = v.iter().take(8).sum();
    let idle = v[3] + v.get(4).copied().unwrap_or(0);
    Some((total - idle, total))
}

/// The whole processor first, then each core.
pub fn parse_proc_stat(s: &str) -> Vec<(u64, u64)> {
    s.lines().filter(|l| l.starts_with("cpu")).filter_map(cpu_line).collect()
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mem {
    pub total: u64,
    pub available: u64,
    pub cached: u64,
    pub swap_total: u64,
    pub swap_free: u64,
    pub committed: u64,
    pub commit_limit: u64,
}

impl Mem {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }
}

pub fn parse_meminfo(s: &str) -> Mem {
    let kb: HashMap<&str, u64> = s
        .lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            Some((k, v.split_whitespace().next()?.parse::<u64>().ok()? * 1024))
        })
        .collect();
    let g = |k: &str| kb.get(k).copied().unwrap_or(0);
    Mem {
        total: g("MemTotal"),
        available: g("MemAvailable"),
        cached: g("Cached") + g("Buffers"),
        swap_total: g("SwapTotal"),
        swap_free: g("SwapFree"),
        committed: g("Committed_AS"),
        commit_limit: g("CommitLimit"),
    }
}

/// (sectors read, sectors written, milliseconds busy) per disk from /proc/diskstats.
pub fn parse_diskstats(s: &str) -> HashMap<String, (u64, u64, u64)> {
    s.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            let n = |i: usize| f.get(i).and_then(|x| x.parse::<u64>().ok());
            Some((f.get(2)?.to_string(), (n(5)?, n(9)?, n(12)?)))
        })
        .collect()
}

/// (bytes received, bytes sent) per adapter from /proc/net/dev.
pub fn parse_net_dev(s: &str) -> HashMap<String, (u64, u64)> {
    s.lines()
        .filter_map(|l| {
            let (name, rest) = l.split_once(':')?;
            let f: Vec<u64> = rest.split_whitespace().filter_map(|x| x.parse().ok()).collect();
            Some((name.trim().to_string(), (*f.first()?, *f.get(8)?)))
        })
        .collect()
}

/// A disk worth showing: a whole drive, not a partition, a loop file, a RAM disk or compressed swap.
pub fn shown_disk(name: &str) -> bool {
    !["loop", "ram", "zram", "dm-", "sr", "fd", "md"].iter().any(|p| name.starts_with(p))
}

/// An adapter worth showing: not the loopback, a container's or a virtual machine's private link.
pub fn shown_adapter(name: &str) -> bool {
    name != "lo" && !["veth", "virbr", "docker", "br-", "vl-", "lb-", "vnet"].iter().any(|p| name.starts_with(p))
}

#[derive(Clone, Debug, Default)]
pub struct Disk {
    pub name: String,
    pub model: String,
    pub kind: String,
    pub size: u64,
    /// Share of the time it was busy, 0-100.
    pub active: f32,
    pub read_rate: f64,
    pub write_rate: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Adapter {
    pub name: String,
    pub kind: String,
    /// Link speed in Mbit/s, when the driver says.
    pub speed: Option<u64>,
    pub recv_rate: f64,
    pub send_rate: f64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub util: f32,
    pub mem_used: u64,
    pub mem_total: u64,
    pub temp: Option<f32>,
    pub power: Option<f32>,
    pub clock: Option<u32>,
}

/// One line of `nvidia-smi --query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,clocks.gr
/// --format=csv,noheader,nounits` (memory in MiB).
pub fn parse_nvidia(line: &str) -> Option<Gpu> {
    let f: Vec<&str> = line.split(',').map(str::trim).collect();
    if f.len() < 4 {
        return None;
    }
    let num = |i: usize| f.get(i).and_then(|x| x.parse::<f32>().ok());
    Some(Gpu {
        name: f[0].to_string(),
        util: num(1)?,
        mem_used: (num(2)? as u64) << 20,
        mem_total: (num(3)? as u64) << 20,
        temp: num(4),
        power: num(5),
        clock: num(6).map(|c| c as u32),
    })
}

#[derive(Clone, Debug, Default)]
pub struct CpuInfo {
    pub model: String,
    pub sockets: usize,
    pub cores: usize,
    pub logical: usize,
    pub base_mhz: Option<u32>,
    pub l1: u64,
    pub l2: u64,
    pub l3: u64,
    pub virtualization: bool,
}

pub fn parse_cpuinfo(s: &str) -> CpuInfo {
    let mut info = CpuInfo::default();
    let mut cores = std::collections::HashSet::new();
    let mut sockets = std::collections::HashSet::new();
    let mut phys = String::new();
    for l in s.lines() {
        let Some((k, v)) = l.split_once(':') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match k {
            "processor" => info.logical += 1,
            "model name" if info.model.is_empty() => info.model = v.split_whitespace().collect::<Vec<_>>().join(" "),
            "physical id" => {
                phys = v.to_string();
                sockets.insert(v.to_string());
            }
            "core id" => {
                cores.insert((phys.clone(), v.to_string()));
            }
            "flags" if !info.virtualization => info.virtualization = v.split(' ').any(|f| f == "vmx" || f == "svm"),
            _ => {}
        }
    }
    info.sockets = sockets.len().max(1);
    info.cores = if cores.is_empty() { info.logical } else { cores.len() };
    info
}

/// "48K" or "2048K" or "32M" from /sys's cache size files, in bytes.
pub fn parse_size(s: &str) -> u64 {
    let s = s.trim();
    let (n, mul) = match s.chars().last() {
        Some('K') => (&s[..s.len() - 1], 1024),
        Some('M') => (&s[..s.len() - 1], 1 << 20),
        _ => (s, 1),
    };
    n.parse::<u64>().unwrap_or(0) * mul
}

pub fn cpu_info() -> CpuInfo {
    let mut info = read("/proc/cpuinfo").map(|s| parse_cpuinfo(&s)).unwrap_or_default();
    info.base_mhz = read("/sys/devices/system/cpu/cpu0/cpufreq/base_frequency")
        .or_else(|| read("/sys/devices/system/cpu/cpu0/cpufreq/cpuinfo_max_freq"))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|khz| khz / 1000);
    // caches: count each one once (shared_cpu_list tells which cores share it)
    let mut seen = std::collections::HashSet::new();
    if let Ok(cpus) = std::fs::read_dir("/sys/devices/system/cpu") {
        for c in cpus.flatten() {
            let name = c.file_name().to_string_lossy().to_string();
            if !name.starts_with("cpu") || !name[3..].chars().all(|x| x.is_ascii_digit()) || name.len() == 3 {
                continue;
            }
            let Ok(idx) = std::fs::read_dir(c.path().join("cache")) else { continue };
            for i in idx.flatten() {
                let p = i.path();
                let get = |f: &str| std::fs::read_to_string(p.join(f)).ok().map(|s| s.trim().to_string());
                let (Some(level), Some(size), Some(shared), Some(kind)) = (get("level"), get("size"), get("shared_cpu_list"), get("type"))
                else {
                    continue;
                };
                if !seen.insert((level.clone(), kind.clone(), shared)) {
                    continue;
                }
                let bytes = parse_size(&size);
                match level.as_str() {
                    "1" => info.l1 += bytes,
                    "2" => info.l2 += bytes,
                    "3" => info.l3 += bytes,
                    _ => {}
                }
            }
        }
    }
    info
}

/// Average current speed of the cores, MHz.
pub fn cpu_speed() -> Option<u32> {
    let mut sum = 0u64;
    let mut n = 0u64;
    for i in 0..1024 {
        match read(&format!("/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq")) {
            Some(s) => {
                if let Ok(k) = s.trim().parse::<u64>() {
                    sum += k;
                    n += 1;
                }
            }
            None if i > 0 => break,
            None => return None,
        }
    }
    (n > 0).then(|| (sum / n / 1000) as u32)
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub cpu: f32,
    pub cores: Vec<f32>,
    pub speed: Option<u32>,
    pub mem: Mem,
    pub disks: Vec<Disk>,
    pub adapters: Vec<Adapter>,
    pub gpu: Option<Gpu>,
    pub uptime: f64,
}

#[derive(Default)]
pub struct SysSampler {
    cpu_prev: Vec<(u64, u64)>,
    disk_prev: HashMap<String, (u64, u64, u64)>,
    net_prev: HashMap<String, (u64, u64)>,
    at: Option<Instant>,
    gpu_at: Option<Instant>,
    gpu: Option<Gpu>,
}

fn sysfs(path: String) -> Option<String> {
    std::fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

/// An AMD card's numbers from its driver's files (no program to run).
fn amd_gpu() -> Option<Gpu> {
    for i in 0..4 {
        let base = format!("/sys/class/drm/card{i}/device");
        let Some(util) = sysfs(format!("{base}/gpu_busy_percent")).and_then(|s| s.parse::<f32>().ok()) else { continue };
        let num = |f: &str| sysfs(format!("{base}/{f}")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
        return Some(Gpu {
            name: "AMD graphics".into(),
            util,
            mem_used: num("mem_info_vram_used"),
            mem_total: num("mem_info_vram_total"),
            temp: None,
            power: None,
            clock: None,
        });
    }
    None
}

fn nvidia_gpu() -> Option<Gpu> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total,temperature.gpu,power.draw,clocks.gr",
            "--format=csv,noheader,nounits",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    parse_nvidia(String::from_utf8_lossy(&out.stdout).lines().next()?)
}

impl SysSampler {
    /// The machine now. `with_gpu`: also the graphics card (NVIDIA: at most every 2 s).
    pub fn sample(&mut self, with_gpu: bool) -> Snapshot {
        let now = Instant::now();
        let dt = self.at.map(|t| now.duration_since(t).as_secs_f64()).unwrap_or(0.0);
        let mut snap = Snapshot::default();
        let cpu = read("/proc/stat").map(|s| parse_proc_stat(&s)).unwrap_or_default();
        let pct =
            |(b, t): (u64, u64), (b0, t0): (u64, u64)| if t > t0 { (b.saturating_sub(b0)) as f32 / (t - t0) as f32 * 100.0 } else { 0.0 };
        if cpu.len() == self.cpu_prev.len() && !cpu.is_empty() {
            snap.cpu = pct(cpu[0], self.cpu_prev[0]);
            snap.cores = cpu.iter().zip(&self.cpu_prev).skip(1).map(|(a, b)| pct(*a, *b)).collect();
        } else {
            snap.cores = vec![0.0; cpu.len().saturating_sub(1)];
        }
        self.cpu_prev = cpu;
        snap.speed = cpu_speed();
        snap.mem = read("/proc/meminfo").map(|s| parse_meminfo(&s)).unwrap_or_default();
        snap.uptime = read("/proc/uptime").and_then(|s| s.split_whitespace().next()?.parse().ok()).unwrap_or(0.0);

        let disks = read("/proc/diskstats").map(|s| parse_diskstats(&s)).unwrap_or_default();
        let mut names: Vec<String> = std::fs::read_dir("/sys/block")
            .map(|d| d.flatten().map(|e| e.file_name().to_string_lossy().to_string()).filter(|n| shown_disk(n)).collect())
            .unwrap_or_default();
        names.sort();
        for n in names {
            let Some(&(r, w, busy)) = disks.get(&n) else { continue };
            let mut d = Disk { name: n.clone(), ..Default::default() };
            d.model = sysfs(format!("/sys/block/{n}/device/model")).unwrap_or_default();
            d.size = sysfs(format!("/sys/block/{n}/size")).and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 512;
            let rot = sysfs(format!("/sys/block/{n}/queue/rotational")).as_deref() == Some("1");
            d.kind = if n.starts_with("nvme") {
                "SSD (NVMe)".into()
            } else if rot {
                "HDD".into()
            } else {
                "SSD".into()
            };
            if let (Some(&(r0, w0, b0)), true) = (self.disk_prev.get(&n), dt > 0.0) {
                d.read_rate = r.saturating_sub(r0) as f64 * 512.0 / dt;
                d.write_rate = w.saturating_sub(w0) as f64 * 512.0 / dt;
                d.active = (busy.saturating_sub(b0) as f64 / (dt * 1000.0) * 100.0).min(100.0) as f32;
            }
            snap.disks.push(d);
        }
        self.disk_prev = disks;

        let net = read("/proc/net/dev").map(|s| parse_net_dev(&s)).unwrap_or_default();
        let mut names: Vec<&String> = net.keys().filter(|n| shown_adapter(n)).collect();
        names.sort();
        for n in names {
            let up = sysfs(format!("/sys/class/net/{n}/operstate")).map(|s| s != "down").unwrap_or(true);
            if !up {
                continue;
            }
            let (rx, tx) = net[n];
            let wireless = std::path::Path::new(&format!("/sys/class/net/{n}/wireless")).exists();
            let tun = sysfs(format!("/sys/class/net/{n}/tun_flags")).is_some()
                || n.starts_with("tailscale")
                || n.starts_with("wg")
                || n.starts_with("tun");
            let mut a = Adapter {
                name: n.clone(),
                kind: if wireless {
                    "Wi-Fi".into()
                } else if tun {
                    "VPN".into()
                } else {
                    "Ethernet".into()
                },
                ..Default::default()
            };
            a.speed = sysfs(format!("/sys/class/net/{n}/speed")).and_then(|s| s.parse::<i64>().ok()).filter(|&s| s > 0).map(|s| s as u64);
            if let (Some(&(rx0, tx0)), true) = (self.net_prev.get(n), dt > 0.0) {
                a.recv_rate = rx.saturating_sub(rx0) as f64 / dt;
                a.send_rate = tx.saturating_sub(tx0) as f64 / dt;
            }
            snap.adapters.push(a);
        }
        self.net_prev = net;

        if with_gpu {
            let due = self.gpu_at.map(|t| now.duration_since(t).as_secs_f64() >= 2.0).unwrap_or(true);
            if due {
                self.gpu = amd_gpu().or_else(nvidia_gpu);
                self.gpu_at = Some(now);
            }
            snap.gpu = self.gpu.clone();
        }
        self.at = Some(now);
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processor_lines() {
        let s = "cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 50 0 25 400 25 0 0 0 0 0\ncpu1 50 0 25 400 25 0 0 0 0 0\nintr 1 2 3\n";
        let v = parse_proc_stat(s);
        assert_eq!(v, vec![(150, 1000), (75, 500), (75, 500)]);
        assert!(cpu_line("cpu 1 2").is_none());
    }

    #[test]
    fn memory() {
        let m = parse_meminfo("MemTotal:       32000000 kB\nMemAvailable:   20000000 kB\nCached: 1000 kB\nBuffers: 24 kB\nSwapTotal: 0 kB\nCommitted_AS: 5 kB\n");
        assert_eq!(m.total, 32_000_000 * 1024);
        assert_eq!(m.used(), 12_000_000 * 1024);
        assert_eq!(m.cached, 1024 * 1024);
        assert_eq!(m.committed, 5 * 1024);
    }

    #[test]
    fn disks_and_adapters() {
        let d = parse_diskstats(
            " 259       0 nvme0n1 1000 0 80000 500 2000 0 160000 900 0 1500 1400 0 0 0 0\n   7  0 loop0 1 0 2 0 0 0 0 0 0 0 0\n",
        );
        assert_eq!(d["nvme0n1"], (80000, 160000, 1500));
        assert!(shown_disk("nvme0n1") && shown_disk("sda") && !shown_disk("zram0") && !shown_disk("loop3"));
        let n = parse_net_dev("Inter-|   Receive\n face |bytes\n    lo: 10 1 0 0 0 0 0 0 10 1 0 0 0 0 0 0\n enp5s0: 5000 10 0 0 0 0 0 0 7000 20 0 0 0 0 0 0\n");
        assert_eq!(n["enp5s0"], (5000, 7000));
        assert!(!shown_adapter("lo") && shown_adapter("tailscale0") && !shown_adapter("vl-limbo") && shown_adapter("wlan0"));
    }

    #[test]
    fn nvidia_line() {
        let g = parse_nvidia("NVIDIA GeForce RTX 2080 Ti, 22, 1416, 11264, 25.72, 34.53, 420").unwrap();
        assert_eq!((g.util, g.mem_used, g.mem_total, g.clock), (22.0, 1416 << 20, 11264 << 20, Some(420)));
        let g = parse_nvidia("Some Card, 5, 100, 200, [N/A], [N/A], [N/A]").unwrap();
        assert_eq!((g.temp, g.power), (None, None));
        assert!(parse_nvidia("garbage").is_none());
    }

    #[test]
    fn cpuinfo_and_sizes() {
        let s = "processor : 0\nmodel name : Intel(R)  Core(TM) i9\nphysical id : 0\ncore id : 0\nflags : fpu vmx sse\n\nprocessor : 1\nphysical id : 0\ncore id : 0\n\nprocessor : 2\nphysical id : 0\ncore id : 1\n";
        let c = parse_cpuinfo(s);
        assert_eq!((c.model.as_str(), c.logical, c.cores, c.sockets, c.virtualization), ("Intel(R) Core(TM) i9", 3, 2, 1, true));
        assert_eq!((parse_size("48K"), parse_size("32M"), parse_size("512")), (48 * 1024, 32 << 20, 512));
    }

    #[test]
    fn a_real_look() {
        let mut s = SysSampler::default();
        let a = s.sample(false);
        assert!(a.mem.total > 0 && !a.cores.is_empty());
        assert!(a.gpu.is_none(), "no graphics card query unless asked");
    }
}
