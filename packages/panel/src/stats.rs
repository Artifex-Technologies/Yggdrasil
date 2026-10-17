//! Fylgja's Stats section: gauges for the processor, graphics card and memory; the disk, network and
//! uptime; the apps using the most; and an internet speed test. Read once a second, and only while
//! the Stats section is showing. The parsing and number formatting are separate so they can be tested.

use std::collections::HashMap;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use resvg::tiny_skia::Pixmap;

use crate::fylgja::{fit, over, FAct, FHit};
use crate::paint::{self, Fonts, Icons, Rgba};
use crate::shell::Model;

pub const CARD_W: f32 = 486.0;

/// Width of each gauge tile, and the space round the ring (the same above, below and at the sides).
const TILE_W: f32 = 96.0;
const CHIP_FONT: f32 = 8.0;

const WHITE: Rgba = (255, 255, 255, 255);
const DIM: Rgba = (150, 150, 165, 255);
const FAINT: Rgba = (52, 52, 62, 255);
const TILE: Rgba = (26, 26, 32, 255);
const LILAC: Rgba = (206, 150, 232, 255);
const GREEN: Rgba = (138, 204, 150, 255);

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gpu {
    pub name: String,
    pub percent: u32,
    pub clock_mhz: u32,
    pub vram_used_mb: u64,
    pub vram_total_mb: u64,
}

/// An app (all of its processes) and what it uses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopApp {
    /// The program's name as the kernel keeps it (15 letters at most).
    pub comm: String,
    /// Share of the whole processor, in percent.
    pub cpu: f32,
    pub rss_mb: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct StatsSnap {
    pub ready: bool,
    pub cpu_name: String,
    pub cpu_percent: u32,
    pub cpu_ghz: f32,
    pub procs: u32,
    pub gpu: Option<Gpu>,
    pub mem_used_mb: u64,
    pub mem_total_mb: u64,
    pub mem_free_mb: u64,
    pub disk_name: String,
    pub disk_used_gb: u64,
    pub disk_total_gb: u64,
    pub down_bps: u64,
    pub up_bps: u64,
    pub uptime_s: u64,
    pub top_cpu: Vec<TopApp>,
    pub top_ram: Vec<TopApp>,
}

#[derive(Clone, Debug, Default)]
pub enum Speed {
    #[default]
    Idle,
    /// "Download" or "Upload", whichever is being measured.
    Running(&'static str),
    Done { down_mbps: f32, up_mbps: f32, at: Instant },
    Failed(String),
}

// ---------------------------------------------------------------- parsing and formatting

/// "Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz" -> "Intel Core i9-9900K".
pub fn clean_cpu_name(raw: &str) -> String {
    let mut s = raw.replace("(R)", "").replace("(TM)", "").replace("(tm)", "");
    if let Some(i) = s.find(" @ ") {
        s.truncate(i);
    }
    let s = s.replace(" CPU", "").replace(" Processor", "");
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The first "model name" in /proc/cpuinfo.
pub fn cpu_name_from(cpuinfo: &str) -> String {
    cpuinfo.lines().find(|l| l.starts_with("model name")).and_then(|l| l.split_once(':')).map(|(_, v)| clean_cpu_name(v.trim())).unwrap_or_default()
}

/// One `nvidia-smi --query-gpu=name,utilization.gpu,clocks.gr,memory.used,memory.total --format=csv,noheader,nounits` line.
pub fn parse_nvidia(line: &str) -> Option<Gpu> {
    let f: Vec<&str> = line.trim().split(',').map(|x| x.trim()).collect();
    if f.len() < 5 {
        return None;
    }
    Some(Gpu { name: f[0].trim_start_matches("NVIDIA ").to_string(), percent: f[1].parse().ok()?, clock_mhz: f[2].parse().ok()?, vram_used_mb: f[3].parse().ok()?, vram_total_mb: f[4].parse().ok()? })
}

/// Bytes received and sent over the real network interfaces, from /proc/net/dev.
pub fn parse_net_dev(text: &str) -> (u64, u64) {
    let (mut rx, mut tx) = (0u64, 0u64);
    for line in text.lines() {
        let Some((name, rest)) = line.split_once(':') else { continue };
        let name = name.trim();
        if name == "lo" || name.starts_with("veth") || name.starts_with("docker") || name.starts_with("br-") || name.starts_with("virbr") {
            continue;
        }
        let f: Vec<u64> = rest.split_whitespace().filter_map(|n| n.parse().ok()).collect();
        if f.len() >= 9 {
            rx += f[0];
            tx += f[8];
        }
    }
    (rx, tx)
}

/// (name, utime + stime in ticks, resident pages) from a /proc/PID/stat line.
pub fn parse_proc_stat(line: &str) -> Option<(String, u64, u64)> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    let name = line[open + 1..close].to_string();
    let f: Vec<&str> = line[close + 1..].split_whitespace().collect();
    // after the name: state is field 0, utime 11, stime 12, rss 21
    let utime: u64 = f.get(11)?.parse().ok()?;
    let stime: u64 = f.get(12)?.parse().ok()?;
    let rss: i64 = f.get(21)?.parse().ok()?;
    Some((name, utime + stime, rss.max(0) as u64))
}

pub fn fmt_rate(bps: u64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1} MB/s", bps as f32 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{} KB/s", bps / 1_000)
    } else {
        format!("{bps} B/s")
    }
}

/// The same without the space, for narrow tiles ("1.2MB/s").
pub fn fmt_rate_short(bps: u64) -> String {
    fmt_rate(bps).replace(' ', "")
}

/// "1.2M", "300K", "12": for the narrowest tile.
pub fn fmt_rate_tiny(bps: u64) -> String {
    if bps >= 1_000_000 {
        format!("{:.1}M", bps as f32 / 1_000_000.0)
    } else if bps >= 1_000 {
        format!("{}K", bps / 1_000)
    } else {
        format!("{bps}")
    }
}

pub fn fmt_uptime(s: u64) -> String {
    let (d, h, m) = (s / 86_400, s / 3_600 % 24, s / 60 % 60);
    if d > 0 {
        format!("{d} d {h} h")
    } else if h > 0 {
        format!("{h} h {m} m")
    } else {
        format!("{m} m")
    }
}

pub fn fmt_mb(mb: u64) -> String {
    if mb >= 1024 { format!("{:.1} GB", mb as f32 / 1024.0) } else { format!("{mb} MB") }
}

/// "5 min ago" for the speed test's last result.
pub fn fmt_ago(secs: u64) -> String {
    match secs {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{} min ago", secs / 60),
        _ => format!("{} h ago", secs / 3600),
    }
}

/// Mbps from bytes moved over `secs` seconds.
pub fn mbps(bytes: u64, secs: f32) -> f32 {
    if secs <= 0.0 { 0.0 } else { bytes as f32 * 8.0 / 1_000_000.0 / secs }
}

// ---------------------------------------------------------------- reading

fn out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

fn cpu_ghz() -> f32 {
    let mut sum = 0u64;
    let mut n = 0u64;
    if let Ok(rd) = std::fs::read_dir("/sys/devices/system/cpu") {
        for e in rd.flatten() {
            if let Some(khz) = std::fs::read_to_string(e.path().join("cpufreq/scaling_cur_freq")).ok().and_then(|t| t.trim().parse::<u64>().ok()) {
                sum += khz;
                n += 1;
            }
        }
    }
    if n > 0 {
        return sum as f32 / n as f32 / 1_000_000.0;
    }
    let info = std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default();
    info.lines().find(|l| l.starts_with("cpu MHz")).and_then(|l| l.split_once(':')).and_then(|(_, v)| v.trim().parse::<f32>().ok()).map(|m| m / 1000.0).unwrap_or(0.0)
}

fn amd_gpu() -> Option<Gpu> {
    for e in std::fs::read_dir("/sys/class/drm").ok()?.flatten() {
        let n = e.file_name().to_string_lossy().to_string();
        if !n.starts_with("card") || n.contains('-') {
            continue;
        }
        let d = e.path().join("device");
        let read = |f: &str| std::fs::read_to_string(d.join(f)).ok().map(|t| t.trim().to_string());
        let Some(busy) = read("gpu_busy_percent").and_then(|b| b.parse::<u32>().ok()) else { continue };
        let mb = |f: &str| read(f).and_then(|b| b.parse::<u64>().ok()).map(|b| b / 1_048_576).unwrap_or(0);
        let clock = read("pp_dpm_sclk").and_then(|t| t.lines().find(|l| l.contains('*')).and_then(|l| l.split_whitespace().nth(1).map(String::from))).and_then(|m| m.trim_end_matches("Mhz").trim_end_matches("MHz").parse::<u32>().ok()).unwrap_or(0);
        return Some(Gpu { name: read("product_name").unwrap_or_else(|| "AMD Radeon".to_string()), percent: busy, clock_mhz: clock, vram_used_mb: mb("mem_info_vram_used"), vram_total_mb: mb("mem_info_vram_total") });
    }
    None
}

/// The disk the system is on, as "Model (device)" or just "Disk (device)".
fn root_disk_name() -> String {
    let src = out("findmnt", &["-no", "SOURCE", "/"]);
    let dev = src.trim().split('[').next().unwrap_or("").trim().to_string();
    let parent = out("lsblk", &["-no", "PKNAME", &dev]).lines().next().unwrap_or("").trim().to_string();
    let base = if parent.is_empty() { dev.trim_start_matches("/dev/").to_string() } else { parent };
    if base.is_empty() {
        return "Disk".to_string();
    }
    let model = out("lsblk", &["-dno", "MODEL", &format!("/dev/{base}")]).trim().to_string();
    if model.is_empty() { format!("Disk ({base})") } else { format!("{model} ({base})") }
}

/// Reads the numbers once a second into `shared` while `active` is set; the heavy ones only while `wanted` is.
pub fn spawn_poller(active: Arc<AtomicBool>, wanted: Arc<AtomicBool>, shared: Arc<Mutex<StatsSnap>>) {
    std::thread::spawn(move || {
        let mut snap = StatsSnap { cpu_name: cpu_name_from(&std::fs::read_to_string("/proc/cpuinfo").unwrap_or_default()), ..Default::default() };
        let mut disk_name: Option<String> = None;
        let mut prev_cpu: Option<(u64, u64)> = None;
        let mut prev_procs: HashMap<u32, u64> = HashMap::new();
        let mut prev_net: Option<((u64, u64), Instant)> = None;
        let page_mb = unsafe { libc::sysconf(libc::_SC_PAGESIZE) }.max(4096) as u64;
        let mut has_nvidia = true;
        while active.load(Ordering::Relaxed) {
            if !wanted.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(300));
                continue;
            }
            let started = Instant::now();
            // processor
            let stat = std::fs::read_to_string("/proc/stat").unwrap_or_default();
            let cur = crate::fylgja::parse_cpu_stat(&stat);
            let total_delta = match (prev_cpu, cur) {
                (Some(p), Some(c)) if c.1 > p.1 => {
                    snap.cpu_percent = (((c.0 - p.0) * 100) / (c.1 - p.1)) as u32;
                    c.1 - p.1
                }
                _ => 0,
            };
            prev_cpu = cur;
            snap.cpu_ghz = cpu_ghz();
            // processes, and the apps using the most
            let mut now: HashMap<u32, u64> = HashMap::new();
            let mut by_app: HashMap<String, (u64, u64)> = HashMap::new();
            let mut count = 0u32;
            if let Ok(rd) = std::fs::read_dir("/proc") {
                for e in rd.flatten() {
                    let Some(pid) = e.file_name().to_str().and_then(|n| n.parse::<u32>().ok()) else { continue };
                    let Some((name, ticks, rss)) = std::fs::read_to_string(e.path().join("stat")).ok().and_then(|l| parse_proc_stat(&l)) else { continue };
                    count += 1;
                    let delta = prev_procs.get(&pid).map(|p| ticks.saturating_sub(*p)).unwrap_or(0);
                    now.insert(pid, ticks);
                    let ent = by_app.entry(name).or_insert((0, 0));
                    ent.0 += delta;
                    ent.1 += rss * page_mb / 1_048_576;
                }
            }
            prev_procs = now;
            snap.procs = count;
            let apps: Vec<TopApp> = by_app.into_iter().map(|(comm, (d, mb))| TopApp { comm, cpu: if total_delta > 0 { d as f32 * 100.0 / total_delta as f32 } else { 0.0 }, rss_mb: mb }).collect();
            let mut by_cpu = apps.clone();
            by_cpu.sort_by(|a, b| b.cpu.partial_cmp(&a.cpu).unwrap_or(std::cmp::Ordering::Equal));
            snap.top_cpu = by_cpu.into_iter().take(4).collect();
            let mut by_ram = apps;
            by_ram.sort_by(|a, b| b.rss_mb.cmp(&a.rss_mb));
            snap.top_ram = by_ram.into_iter().take(4).collect();
            // memory
            if let Some((used, total)) = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| crate::fylgja::parse_meminfo(&m)) {
                snap.mem_used_mb = used;
                snap.mem_total_mb = total;
                snap.mem_free_mb = total.saturating_sub(used);
            }
            // graphics card
            snap.gpu = if has_nvidia {
                let o = out("nvidia-smi", &["--query-gpu=name,utilization.gpu,clocks.gr,memory.used,memory.total", "--format=csv,noheader,nounits"]);
                if o.trim().is_empty() {
                    has_nvidia = false;
                    amd_gpu()
                } else {
                    o.lines().next().and_then(parse_nvidia)
                }
            } else {
                amd_gpu()
            };
            // disk, network, uptime
            let name = disk_name.get_or_insert_with(root_disk_name).clone();
            snap.disk_name = name;
            let df = out("df", &["-B1", "--output=used,size", "/"]);
            let n: Vec<u64> = df.lines().nth(1).unwrap_or("").split_whitespace().filter_map(|v| v.parse().ok()).collect();
            if n.len() == 2 {
                snap.disk_used_gb = n[0] >> 30;
                snap.disk_total_gb = n[1] >> 30;
            }
            let net = parse_net_dev(&std::fs::read_to_string("/proc/net/dev").unwrap_or_default());
            if let Some((p, at)) = prev_net {
                let dt = at.elapsed().as_secs_f32().max(0.2);
                snap.down_bps = (net.0.saturating_sub(p.0) as f32 / dt) as u64;
                snap.up_bps = (net.1.saturating_sub(p.1) as f32 / dt) as u64;
            }
            prev_net = Some((net, Instant::now()));
            snap.uptime_s = std::fs::read_to_string("/proc/uptime").ok().and_then(|u| u.split_whitespace().next().and_then(|v| v.parse::<f32>().ok())).unwrap_or(0.0) as u64;
            snap.ready = true;
            if let Ok(mut g) = shared.lock() {
                *g = snap.clone();
            }
            let spent = started.elapsed();
            if spent < Duration::from_secs(1) {
                std::thread::sleep(Duration::from_secs(1) - spent);
            }
        }
    });
}

// ---------------------------------------------------------------- the speed test

/// Measures download and upload against Cloudflare's speed-test servers with `curl` (several
/// connections at once). It connects to the internet only when this is called, i.e. when you press the button.
pub fn run_speedtest(state: Arc<Mutex<Speed>>) {
    {
        let Ok(mut g) = state.lock() else { return };
        if matches!(*g, Speed::Running(_)) {
            return;
        }
        *g = Speed::Running("Download");
    }
    std::thread::spawn(move || {
        let set = |s: Speed| {
            if let Ok(mut g) = state.lock() {
                *g = s;
            }
        };
        let down = measure(true);
        let Some(down) = down else { return set(Speed::Failed("Could not reach the speed test server. Check the connection.".into())) };
        set(Speed::Running("Upload"));
        let Some(up) = measure(false) else { return set(Speed::Failed("The upload test failed.".into())) };
        set(Speed::Done { down_mbps: down, up_mbps: up, at: Instant::now() });
    });
}

/// Four downloads of 10 MB, or three uploads of 5 MB, at the same time; the total over the longest.
fn measure(download: bool) -> Option<f32> {
    let n = if download { 4 } else { 3 };
    let started = Instant::now();
    let mut kids = vec![];
    for _ in 0..n {
        let script = if download {
            "curl -s -m 40 -o /dev/null -w '%{size_download}' 'https://speed.cloudflare.com/__down?bytes=10000000'"
        } else {
            "head -c 5000000 /dev/zero | curl -s -m 40 -o /dev/null -w '%{size_upload}' --data-binary @- 'https://speed.cloudflare.com/__up'"
        };
        kids.push(Command::new("sh").args(["-c", script]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?);
    }
    let mut bytes = 0u64;
    for k in kids {
        let o = k.wait_with_output().ok()?;
        bytes += String::from_utf8_lossy(&o.stdout).trim().parse::<f64>().unwrap_or(0.0) as u64;
    }
    if bytes == 0 {
        return None;
    }
    Some(mbps(bytes, started.elapsed().as_secs_f32()))
}

// ---------------------------------------------------------------- drawing

/// A tile's chip: a small rounded label.
/// A chip's width, counted from its letters so that the card's height can be known before drawing.
fn chip_w(text: &str) -> f32 {
    text.chars().count() as f32 * 4.3 + 7.0
}

/// The chips on each gauge tile: the processor, the graphics card, the memory.
fn chip_texts(st: &StatsSnap) -> [Vec<String>; 3] {
    let gpu = match &st.gpu {
        Some(g) => {
            let clock = if g.clock_mhz >= 1000 { format!("{:.1} GHz", g.clock_mhz as f32 / 1000.0) } else { format!("{} MHz", g.clock_mhz) };
            vec![clock, format!("{:.1}/{:.0}GB", g.vram_used_mb as f32 / 1024.0, g.vram_total_mb as f32 / 1024.0)]
        }
        None => vec![],
    };
    [vec![format!("{:.1} GHz", st.cpu_ghz), format!("{} proc", st.procs)], gpu, vec![format!("{} free", fmt_mb(st.mem_free_mb))]]
}

/// The chips laid side by side, starting a new row when the next would not fit.
fn chip_rows(chips: &[String]) -> Vec<Vec<usize>> {
    let avail = TILE_W - 8.0;
    let mut rows: Vec<Vec<usize>> = vec![];
    let mut used = 0.0;
    for (i, c) in chips.iter().enumerate() {
        let w = chip_w(c);
        if rows.is_empty() || used + 3.0 + w > avail {
            rows.push(vec![i]);
            used = w;
        } else {
            rows.last_mut().unwrap().push(i);
            used += 3.0 + w;
        }
    }
    rows
}

/// How many rows of chips the tallest tile needs (at least one).
fn chip_row_count(st: &StatsSnap) -> usize {
    chip_texts(st).iter().map(|c| chip_rows(c).len()).max().unwrap_or(1).max(1)
}

/// The gauge tiles' height for that many rows of chips.
fn tile_h(rows: usize) -> f32 {
    121.0 + 18.0 * (rows as f32 - 1.0) + 16.0 + 12.0
}

/// The card's height: the gauge tiles, the row under them, and the space round and between.
pub fn card_h(m: &Model) -> f32 {
    10.0 + tile_h(chip_row_count(&m.fylgja.stats)) + 6.0 + 44.0 + 10.0
}

fn chip(pix: &mut Pixmap, s: f32, fonts: &Fonts, x: f32, y: f32, text: &str) {
    let w = chip_w(text);
    paint::fill_rounded(pix, x * s, y * s, w * s, 16.0 * s, 4.0 * s, (44, 44, 54, 255));
    let tw = paint::text_width(&fonts.regular, CHIP_FONT * s, text);
    paint::draw_text(pix, &fonts.regular, CHIP_FONT * s, (x + w / 2.0) * s - tw / 2.0, (y + 8.0) * s, WHITE, text);
}

/// A gauge tile: a ring with the percentage, then the name, two lines of detail and chips.
#[allow(clippy::too_many_arguments)]
fn gauge(pix: &mut Pixmap, s: f32, fonts: &Fonts, rect: (f32, f32, f32, f32), frac: f32, label_pct: &str, color: Rgba, title: &str, sub: &str, chips: &[String]) {
    let (x, y, w, h) = rect;
    paint::fill_rounded(pix, x * s, y * s, w * s, h * s, 14.0 * s, TILE);
    let (cx, cy, r) = (x + w / 2.0, y + (w - 58.0) / 2.0 + 29.0, 26.0);
    paint::stroke_arc(pix, cx * s, cy * s, r * s, 6.0 * s, 1.0, FAINT);
    paint::stroke_arc(pix, cx * s, cy * s, r * s, 6.0 * s, frac.clamp(0.0, 1.0), color);
    let tw = paint::text_width(&fonts.bold, 14.0 * s, label_pct);
    paint::draw_text(pix, &fonts.bold, 14.0 * s, cx * s - tw / 2.0, cy * s, WHITE, label_pct);
    let t = paint::text_width(&fonts.bold, 13.0 * s, title);
    paint::draw_text(pix, &fonts.bold, 13.0 * s, cx * s - t / 2.0, (y + 89.0) * s, WHITE, title);
    // the processor or card's name over up to two lines
    let lines = crate::fylgja::wrap(&fonts.regular, 8.5 * s, sub, (w - 8.0) * s);
    for (i, l) in lines.iter().take(2).enumerate() {
        let l = fit(&fonts.regular, 8.5 * s, l, (w - 8.0) * s);
        let lw = paint::text_width(&fonts.regular, 8.5 * s, &l);
        paint::draw_text(pix, &fonts.regular, 8.5 * s, cx * s - lw / 2.0, (y + 101.0 + 11.0 * i as f32) * s, DIM, &l);
    }
    // the chips side by side, centred, a new row when they would not fit
    for (r, row) in chip_rows(chips).iter().enumerate() {
        let total: f32 = row.iter().map(|&i| chip_w(&chips[i])).sum::<f32>() + 3.0 * (row.len() - 1) as f32;
        let mut at = cx - total / 2.0;
        for &i in row {
            chip(pix, s, fonts, at, y + 121.0 + 18.0 * r as f32, &chips[i]);
            at += chip_w(&chips[i]) + 3.0;
        }
    }
}

fn small_tile(pix: &mut Pixmap, s: f32, fonts: &Fonts, rect: (f32, f32, f32, f32), label: &str, value: &str) {
    let (x, y, w, h) = rect;
    paint::fill_rounded(pix, x * s, y * s, w * s, h * s, 12.0 * s, TILE);
    let mut size = 12.0;
    while size > 9.0 && paint::text_width(&fonts.bold, size * s, value) > (w - 16.0) * s {
        size -= 0.5;
    }
    paint::draw_text(pix, &fonts.bold, 9.0 * s, (x + 10.0) * s, (y + 12.0) * s, DIM, &fit(&fonts.bold, 9.0 * s, &label.to_uppercase(), (w - 16.0) * s));
    paint::draw_text(pix, &fonts.bold, size * s, (x + 10.0) * s, (y + 29.0) * s, WHITE, &fit(&fonts.bold, size * s, value, (w - 16.0) * s));
}

/// The name to show for a program: the matching app's, else the program's own.
fn app_name<'a>(m: &'a Model, comm: &'a str) -> (&'a str, Option<&'a str>) {
    for a in &m.apps {
        let prog = crate::apps::command(&a.exec).first().map(|p| p.rsplit('/').next().unwrap_or(p).to_string()).unwrap_or_default();
        let cut: String = prog.chars().take(15).collect();
        if !comm.is_empty() && (cut == comm || (comm.len() >= 6 && cut.starts_with(comm))) {
            return (&a.name, Some(&a.icon));
        }
    }
    (comm, None)
}

/// Draws the Stats section inside `card` and adds its clickable parts to `hits`.
#[allow(clippy::too_many_arguments)]
pub fn paint(pix: &mut Pixmap, s: f32, m: &Model, card: (f32, f32, f32, f32), pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons, hits: &mut Vec<FHit>) {
    let (cx, cy, cw, _ch) = card;
    let d = &m.fylgja;
    let st = &d.stats;
    if !st.ready {
        return;
    }
    let (x0, y0) = (cx + 10.0, cy + 10.0);
    let rw = 160.0;
    let left_w = cw - 20.0 - rw - 6.0;
    let tw = (left_w - 12.0) / 3.0;
    let rows = chip_row_count(st);
    let th = tile_h(rows);
    let chips = chip_texts(st);

    // processor
    gauge(pix, s, fonts, (x0, y0, tw, th), st.cpu_percent as f32 / 100.0, &format!("{}%", st.cpu_percent), paint::accent(255), "CPU", &st.cpu_name, &chips[0]);
    // graphics card
    match &st.gpu {
        Some(g) => gauge(pix, s, fonts, (x0 + tw + 6.0, y0, tw, th), g.percent as f32 / 100.0, &format!("{}%", g.percent), LILAC, "GPU", &g.name, &chips[1]),
        None => gauge(pix, s, fonts, (x0 + tw + 6.0, y0, tw, th), 0.0, "–", LILAC, "GPU", "No graphics data", &[]),
    }
    // memory
    let pct = if st.mem_total_mb > 0 { st.mem_used_mb as f32 / st.mem_total_mb as f32 } else { 0.0 };
    gauge(pix, s, fonts, (x0 + 2.0 * (tw + 6.0), y0, tw, th), pct, &format!("{}%", (pct * 100.0).round() as u32), GREEN, "RAM", &format!("{} / {}", fmt_mb(st.mem_used_mb), fmt_mb(st.mem_total_mb)), &chips[2]);
    // under them: disk, network, uptime
    let by = y0 + th + 6.0;
    small_tile(pix, s, fonts, (x0, by, tw, 44.0), &st.disk_name, &format!("{} / {} GB", st.disk_used_gb, st.disk_total_gb));
    {
        // what comes in and what goes out, side by side (the type shrinks a little if the pair is wide)
        let (nx, ny) = (x0 + tw + 6.0, by);
        paint::fill_rounded(pix, nx * s, ny * s, tw * s, 44.0 * s, 12.0 * s, TILE);
        paint::draw_text(pix, &fonts.bold, 9.0 * s, (nx + 10.0) * s, (ny + 12.0) * s, DIM, "NETWORK");
        let avail = tw - 20.0;
        let (mut down, mut up) = (fmt_rate_short(st.down_bps), fmt_rate_short(st.up_bps));
        let mut size = 10.5;
        let width = |size: f32, a: &str, b: &str| paint::text_width(&fonts.bold, size * s, a) / s + paint::text_width(&fonts.bold, size * s, b) / s + 2.0 * ((size - 3.0).max(5.0) + 2.0) + 8.0;
        while width(size, &down, &up) > avail && size > 8.0 {
            size -= 0.5;
        }
        if width(size, &down, &up) > avail {
            // still too wide: leave off the "B/s"
            (down, up) = (fmt_rate_tiny(st.down_bps), fmt_rate_tiny(st.up_bps));
        }
        let ry = ny + 29.0;
        let mut x = nx + 10.0;
        for (icon, text) in [("go-down-symbolic", &down), ("go-up-symbolic", &up)] {
            // the arrow is smaller than the number beside it
            let isz = (size - 3.0).max(5.0);
            if let Some(ic) = icons.get_tinted(icon, (isz * s) as u32, DIM) {
                paint::draw_pixmap(pix, ic, (x * s) as i32, ((ry - isz / 2.0) * s) as i32);
            }
            x += isz + 2.0;
            paint::draw_text(pix, &fonts.bold, size * s, x * s, ry * s, WHITE, text);
            x += paint::text_width(&fonts.bold, size * s, text) / s + 8.0;
        }
    }
    small_tile(pix, s, fonts, (x0 + 2.0 * (tw + 6.0), by, tw, 44.0), "Uptime", &fmt_uptime(st.uptime_s));

    // right column: the apps using the most
    let rx = x0 + left_w + 6.0;
    paint::fill_rounded(pix, rx * s, y0 * s, rw * s, 112.0 * s, 14.0 * s, TILE);
    paint::draw_text(pix, &fonts.regular, 9.5 * s, (rx + 12.0) * s, (y0 + 13.0) * s, DIM, "Top apps");
    // CPU | RAM: the one in use is lit, a click on either switches
    let seg = (rx + rw - 12.0 - 70.0, y0 + 4.0, 70.0, 18.0);
    paint::fill_rounded(pix, seg.0 * s, seg.1 * s, seg.2 * s, seg.3 * s, 9.0 * s, (40, 40, 50, 255));
    let on_x = if d.top_ram { seg.0 + 35.0 } else { seg.0 };
    paint::fill_rounded(pix, on_x * s, seg.1 * s, 35.0 * s, seg.3 * s, 9.0 * s, paint::accent(255));
    for (i, label) in ["CPU", "RAM"].iter().enumerate() {
        let lw = paint::text_width(&fonts.bold, 9.0 * s, label);
        let lit = (i == 1) == d.top_ram;
        paint::draw_text(pix, &fonts.bold, 9.0 * s, (seg.0 + 35.0 * i as f32 + 17.5) * s - lw / 2.0, (seg.1 + 9.0) * s, if lit { WHITE } else { DIM }, label);
    }
    hits.push(FHit { rect: seg, act: FAct::TopToggle });
    let list = if d.top_ram { &st.top_ram } else { &st.top_cpu };
    let maxv = list.iter().map(|a| if d.top_ram { a.rss_mb as f32 } else { a.cpu }).fold(0.0f32, f32::max).max(0.001);
    for (i, a) in list.iter().enumerate() {
        let ry = y0 + 34.0 + 19.0 * i as f32;
        let (name, icon) = app_name(m, &a.comm);
        let mut tx = rx + 12.0;
        if let Some(ic) = icon.and_then(|i| icons.get(i, (14.0 * s) as u32)) {
            paint::draw_pixmap(pix, ic, (tx * s) as i32, ((ry - 8.0) * s) as i32);
            tx += 20.0;
        } else {
            paint::fill_rounded(pix, tx * s, (ry - 8.0) * s, 14.0 * s, 14.0 * s, 4.0 * s, (52, 52, 62, 255));
            let l: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
            let lw = paint::text_width(&fonts.bold, 8.5 * s, &l);
            paint::draw_text(pix, &fonts.bold, 8.5 * s, (tx + 7.0) * s - lw / 2.0, ry * s - 1.0 * s, WHITE, &l);
            tx += 20.0;
        }
        let value = if d.top_ram { fmt_mb(a.rss_mb) } else { format!("{:.0}%", a.cpu) };
        let vw = paint::text_width(&fonts.regular, 9.5 * s, &value);
        paint::draw_text(pix, &fonts.regular, 9.5 * s, (rx + rw - 12.0) * s - vw, ry * s, DIM, &value);
        paint::draw_text(pix, &fonts.bold, 10.0 * s, tx * s, ry * s, WHITE, &fit(&fonts.bold, 10.0 * s, name, (rx + rw - 12.0 - tx) * s - vw - 8.0 * s));
        let frac = (if d.top_ram { a.rss_mb as f32 } else { a.cpu }) / maxv;
        paint::fill_rounded(pix, tx * s, (ry + 6.0) * s, (rx + rw - 12.0 - tx) * s, 2.0 * s, 1.0 * s, FAINT);
        paint::fill_rounded(pix, tx * s, (ry + 6.0) * s, ((rx + rw - 12.0 - tx) * frac).max(2.0) * s, 2.0 * s, 1.0 * s, if d.top_ram { GREEN } else { paint::accent(255) });
    }

    // the speed test
    let sy = y0 + 118.0;
    paint::fill_rounded(pix, rx * s, sy * s, rw * s, (th + 50.0 - 118.0) * s, 14.0 * s, TILE);
    let speed = d.speed.lock().map(|g| g.clone()).unwrap_or_default();
    let head = match &speed {
        Speed::Done { at, .. } => format!("Internet speed · {}", fmt_ago(at.elapsed().as_secs())),
        Speed::Running(phase) => format!("Internet speed · {phase}…"),
        _ => "Internet speed".to_string(),
    };
    paint::draw_text(pix, &fonts.regular, 9.5 * s, (rx + 12.0) * s, (sy + 12.0) * s, DIM, &head);
    match &speed {
        Speed::Done { down_mbps, up_mbps, .. } => {
            paint::draw_text(pix, &fonts.bold, 17.0 * s, (rx + 12.0) * s, (sy + 34.0) * s, WHITE, &format!("{down_mbps:.0}"));
            let w1 = paint::text_width(&fonts.bold, 17.0 * s, &format!("{down_mbps:.0}")) / s;
            paint::draw_text(pix, &fonts.regular, 9.0 * s, (rx + 14.0 + w1) * s, (sy + 34.0) * s, DIM, "Mbps down");
            paint::draw_text(pix, &fonts.bold, 17.0 * s, (rx + 90.0) * s, (sy + 34.0) * s, WHITE, &format!("{up_mbps:.0}"));
            let w2 = paint::text_width(&fonts.bold, 17.0 * s, &format!("{up_mbps:.0}")) / s;
            paint::draw_text(pix, &fonts.regular, 9.0 * s, (rx + 92.0 + w2) * s, (sy + 34.0) * s, DIM, "Mbps up");
        }
        Speed::Failed(why) => {
            paint::draw_text(pix, &fonts.regular, 9.5 * s, (rx + 12.0) * s, (sy + 33.0) * s, (232, 120, 120, 255), &fit(&fonts.regular, 9.5 * s, why, (rw - 24.0) * s));
        }
        Speed::Running(_) => {
            paint::draw_text(pix, &fonts.regular, 11.0 * s, (rx + 12.0) * s, (sy + 33.0) * s, WHITE, "Measuring…");
        }
        Speed::Idle => {
            paint::draw_text(pix, &fonts.regular, 10.5 * s, (rx + 12.0) * s, (sy + 33.0) * s, DIM, "Not measured yet");
        }
    }
    let b = (rx + 12.0, sy + 50.0, rw - 24.0, 28.0);
    let running = matches!(speed, Speed::Running(_));
    paint::fill_rounded(pix, b.0 * s, b.1 * s, b.2 * s, b.3 * s, 10.0 * s, if running { (60, 50, 90, 255) } else if over(pointer, b) { (150, 110, 232, 255) } else { paint::accent(255) });
    let label = if running { "Testing…" } else { "Run speed test" };
    let lw = paint::text_width(&fonts.bold, 11.0 * s, label);
    paint::draw_text(pix, &fonts.bold, 11.0 * s, (b.0 + b.2 / 2.0) * s - lw / 2.0, (b.1 + b.3 / 2.0) * s, WHITE, label);
    if !running {
        hits.push(FHit { rect: b, act: FAct::SpeedTest });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_numbers() {
        assert_eq!(clean_cpu_name("Intel(R) Core(TM) i9-9900K CPU @ 3.60GHz"), "Intel Core i9-9900K");
        assert_eq!(clean_cpu_name("AMD Ryzen 7 7840HS w/ Radeon 780M Graphics"), "AMD Ryzen 7 7840HS w/ Radeon 780M Graphics");
        assert_eq!(cpu_name_from("processor\t: 0\nmodel name\t: AMD Ryzen 9 5900X 12-Core Processor\n"), "AMD Ryzen 9 5900X 12-Core");
        assert_eq!(fmt_rate(1_234_567), "1.2 MB/s");
        assert_eq!(fmt_rate(300_400), "300 KB/s");
        assert_eq!(fmt_rate(12), "12 B/s");
        assert_eq!(fmt_uptime(5 * 3600 + 12 * 60 + 9), "5 h 12 m");
        assert_eq!(fmt_uptime(3 * 86_400 + 4 * 3600), "3 d 4 h");
        assert_eq!(fmt_uptime(125), "2 m");
        assert_eq!(fmt_mb(19_866), "19.4 GB");
        assert_eq!(fmt_mb(512), "512 MB");
        assert_eq!(fmt_ago(30), "just now");
        assert_eq!(fmt_ago(300), "5 min ago");
        assert!((mbps(10_000_000, 1.0) - 80.0).abs() < 0.01);
        assert_eq!(mbps(5, 0.0), 0.0);
    }

    #[test]
    fn readings() {
        let g = parse_nvidia("NVIDIA GeForce RTX 2080 Ti, 12, 1950, 1024, 11264\n").unwrap();
        assert_eq!((g.name.as_str(), g.percent, g.clock_mhz, g.vram_used_mb, g.vram_total_mb), ("GeForce RTX 2080 Ti", 12, 1950, 1024, 11264));
        assert!(parse_nvidia("").is_none());
        assert!(parse_nvidia("No devices were found").is_none());
        let net = "Inter-|   Receive\n face |bytes packets errs drop fifo frame compressed multicast|bytes packets\n    lo: 1000 1 0 0 0 0 0 0 1000 1 0 0 0 0 0 0\n enp0s1: 5000 10 0 0 0 0 0 0 700 5 0 0 0 0 0 0\n veth1: 9999 1 0 0 0 0 0 0 9999 1 0 0 0 0 0 0\n";
        assert_eq!(parse_net_dev(net), (5000, 700));
        let line = "1234 (Web Content) S 1 1234 1234 0 -1 4194560 100 0 0 0 150 50 0 0 20 0 5 0 100 1000000 2500 18446744073709551615";
        assert_eq!(parse_proc_stat(line), Some(("Web Content".to_string(), 200, 2500)));
        assert!(parse_proc_stat("garbage").is_none());
    }
}
