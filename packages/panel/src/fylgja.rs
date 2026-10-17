//! Fylgja: the notch. It hangs from the top bar's black tab. Resting the pointer on the tab opens a
//! small card with what is playing; the wheel opens the full notch and moves through its sections;
//! moving the pointer away closes it. This file reads what the sections show (what is playing,
//! volumes, load, what was copied, the wallpapers) with the system's own tools, records a spoken
//! question for Sarina, and draws all of it. The parsing is separate from the commands so it can be
//! tested.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use resvg::tiny_skia::Pixmap;
use serde_json::Value;

use crate::paint::{self, Fonts, Icons, Rgba};
use crate::shell::{Model, PowerAct, StartAct};

pub const SECTIONS: [&str; 7] = ["Controls", "Today", "Stats", "Clipboard", "Notifications", "Wallpaper", "Power"];
const CLIPBOARD: usize = 3;
pub const NOTIFS: usize = 4;
const WALLPAPER: usize = 5;
const WALL_PAGE: usize = 8;
const MAX_CLIPS: usize = 25;

/// Where the notch is: peeking (the small music card), open on a section, or showing Sarina's voice card.
#[derive(Clone, Debug, Default)]
pub struct FyState {
    pub section: usize,
    /// The full notch (the wheel, a click or a shortcut opened it); otherwise just the small card.
    pub open: bool,
    /// Opened by a shortcut: it stays until Esc or a click outside, instead of closing when the pointer leaves.
    pub pinned: bool,
    /// Sarina's voice card is showing (a question is being recorded, answered or has failed).
    pub voice_ui: bool,
    /// Controls: the list of sound outputs is dropped down from the Audio tile.
    pub picker: bool,
    /// Controls: the volume's mixer (every app playing sound) is dropped down under the volume bar.
    pub mixer: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum Voice {
    #[default]
    Idle,
    /// Recording a question, this many seconds so far.
    Listening(u64),
    /// The recording is with Sarina; what she heard once the words are known (empty before).
    Thinking(String),
    Reply(String),
    Failed(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    /// The player's D-Bus name (org.mpris.MediaPlayer2.…).
    pub bus: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// The cover picture, a file on this computer.
    pub art: Option<String>,
    /// How far into the song, and how long it is, in microseconds (0 when the player does not say).
    pub pos_us: u64,
    pub len_us: u64,
}

/// One app that is playing sound, with its own volume.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppVol {
    pub id: u32,
    pub name: String,
    pub percent: u32,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// The first reading has been done (before that the card has nothing true to say).
    pub ready: bool,
    /// The slower readings (networks, Bluetooth, outputs, Night Light) have been done too.
    pub slow_ready: bool,
    pub media: Option<Media>,
    /// The main output's volume in percent, and whether it is muted.
    pub master: Option<(u32, bool)>,
    pub apps: Vec<AppVol>,
    /// Wi-Fi: None without an adapter, else (radio on, name of the network joined).
    pub wifi: Option<(bool, String)>,
    /// Bluetooth: None without an adapter, else (powered, name of a connected device).
    pub bt: Option<(bool, String)>,
    /// The screen's warm tint (Night Light) is on.
    pub night: bool,
    /// Sound outputs as (name, description), and the one in use.
    pub sinks: Vec<(String, String)>,
    pub default_sink: String,
    pub cpu: u32,
    /// Memory used and total, in MiB.
    pub mem: (u64, u64),
    /// The root disk, used and total, in GiB.
    pub disk: (u64, u64),
    pub temp: Option<u32>,
}

/// Everything the notch draws from, kept in the panel's model.
#[derive(Default)]
pub struct FyData {
    pub snap: Snapshot,
    pub clips: Vec<String>,
    pub history_on: bool,
    pub voice: Voice,
    /// The cover of what is playing, ready to draw.
    pub art: Option<Pixmap>,
    pub walls: Vec<PathBuf>,
    pub wall_page: usize,
    pub thumbs: Option<Arc<Mutex<HashMap<PathBuf, Pixmap>>>>,
    /// The Stats section: its readings, which list of apps it shows, and the speed test.
    pub stats: crate::stats::StatsSnap,
    pub top_ram: bool,
    pub speed: Arc<Mutex<crate::stats::Speed>>,
}

// ---------------------------------------------------------------- parsing

/// `wpctl get-volume` prints "Volume: 0.45" or "Volume: 0.45 [MUTED]".
pub fn parse_wpctl(s: &str) -> Option<(u32, bool)> {
    let v: f32 = s.split_whitespace().nth(1)?.parse().ok()?;
    Some(((v * 100.0).round() as u32, s.contains("MUTED")))
}

/// `pactl -f json list sink-inputs`: the apps playing sound.
pub fn parse_sink_inputs(json: &str) -> Vec<AppVol> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else { return vec![] };
    list.iter()
        .filter_map(|i| {
            let id = i.get("index")?.as_u64()? as u32;
            let props = i.get("properties")?;
            let name = ["application.name", "media.name"].iter().find_map(|k| props.get(k).and_then(|v| v.as_str())).unwrap_or("Sound").to_string();
            let vol = i.get("volume")?.as_object()?.values().next()?.get("value_percent")?.as_str()?;
            Some(AppVol { id, name, percent: vol.trim_end_matches('%').parse().ok()? })
        })
        .collect()
}

/// A property printed by `busctl --json=short get-property`.
pub fn parse_busctl(json: &str) -> Option<Value> {
    serde_json::from_str::<Value>(json).ok()?.get("data").cloned()
}

/// The sound outputs from `pactl -f json list sinks`, as (name, description).
pub fn parse_sinks(json: &str) -> Vec<(String, String)> {
    let Ok(Value::Array(list)) = serde_json::from_str::<Value>(json) else { return vec![] };
    list.iter()
        .filter_map(|i| {
            let name = i.get("name")?.as_str()?.to_string();
            let desc = i.get("description").and_then(|d| d.as_str()).unwrap_or(&name).to_string();
            Some((name, desc))
        })
        .collect()
}

/// Wi-Fi from `nmcli radio wifi`, `nmcli -t -f TYPE device` and `nmcli -t -f NAME,TYPE connection show --active`.
pub fn parse_wifi(radio: &str, devices: &str, active: &str) -> Option<(bool, String)> {
    if !devices.lines().any(|l| l.trim() == "wifi") {
        return None;
    }
    let name = active.lines().find_map(|l| l.strip_suffix(":802-11-wireless")).unwrap_or("").replace("\\:", ":");
    Some((radio.trim() == "enabled", name))
}

/// "Device AA:BB:CC Headphones" -> "Headphones" (the first connected device).
pub fn parse_connected_name(devices: &str) -> String {
    devices.lines().next().map(|l| l.splitn(3, ' ').nth(2).unwrap_or("").trim().to_string()).unwrap_or_default()
}

/// (title, artist, cover address, length in microseconds) from a player's Metadata.
pub fn parse_metadata(data: &Value) -> (String, String, String, u64) {
    let text = |k: &str| data.get(k).and_then(|v| v.get("data")).cloned();
    let title = text("xesam:title").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let artist = match text("xesam:artist") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "),
        Some(Value::String(s)) => s,
        _ => String::new(),
    };
    let art = text("mpris:artUrl").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let len = text("mpris:length").and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n.max(0) as u64))).unwrap_or(0);
    (title, artist, art, len)
}

/// (busy, total) jiffies from the first line of /proc/stat.
pub fn parse_cpu_stat(stat: &str) -> Option<(u64, u64)> {
    let nums: Vec<u64> = stat.lines().next()?.split_whitespace().skip(1).filter_map(|n| n.parse().ok()).collect();
    if nums.len() < 5 {
        return None;
    }
    let total: u64 = nums.iter().take(8).sum();
    Some((total - nums[3] - nums[4], total))
}

/// (used, total) in MiB from /proc/meminfo.
pub fn parse_meminfo(m: &str) -> Option<(u64, u64)> {
    let kb = |k: &str| m.lines().find(|l| l.starts_with(k))?.split_whitespace().nth(1)?.parse::<u64>().ok();
    let total = kb("MemTotal:")?;
    Some(((total - kb("MemAvailable:")?) / 1024, total / 1024))
}

/// Standard base64 (what `base64` prints) back to bytes; None on anything else.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.trim().bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => return None,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// Puts a copied text first in the history: no repeats, no blanks, only the newest few.
pub fn push_clip(list: &mut Vec<String>, text: String) {
    let t = text.trim_end_matches(['\n', '\r']).to_string();
    if t.trim().is_empty() {
        return;
    }
    list.retain(|x| *x != t);
    list.insert(0, t);
    list.truncate(MAX_CLIPS);
}

/// The text cut to one line that fits `max` pixels.
fn one_line(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> String {
    let line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    fit(font, size, line, max)
}

/// `text` broken into lines of at most `max` pixels, at spaces where it can.
pub fn wrap(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> Vec<String> {
    let mut lines = vec![];
    for para in text.lines() {
        let mut cur = String::new();
        for word in para.split_whitespace() {
            let try_it = if cur.is_empty() { word.to_string() } else { format!("{cur} {word}") };
            if cur.is_empty() || paint::text_width(font, size, &try_it) <= max {
                cur = try_it;
            } else {
                lines.push(std::mem::take(&mut cur));
                cur = word.to_string();
            }
        }
        lines.push(cur);
    }
    lines
}

// ---------------------------------------------------------------- reading

fn out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

fn cache_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
    let d = base.join("labyrinth").join("fylgja");
    std::fs::create_dir_all(&d).ok()?;
    Some(d)
}

/// The cover as a file here: a file:// address is used as it is, a web address is fetched once and kept.
fn cover_file(url: &str) -> Option<String> {
    if let Some(p) = url.strip_prefix("file://") {
        let p = p.replace("%20", " ");
        // a browser's cover is often a bare temp file: the decoder needs an extension, so keep a copy that has one
        let known = Path::new(&p).extension().and_then(|e| e.to_str()).map(|e| matches!(e.to_lowercase().as_str(), "jpg" | "jpeg" | "png" | "webp" | "gif")).unwrap_or(false);
        if known {
            return Some(p);
        }
        let data = std::fs::read(&p).ok()?;
        let ext = image_ext(&data)?;
        let mut h: u64 = 0xcbf29ce484222325;
        for b in p.bytes().chain(data.len().to_string().bytes()) {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        let copy = cache_dir()?.join(format!("cover-{h:016x}.{ext}"));
        if !copy.is_file() {
            std::fs::write(&copy, &data).ok()?;
        }
        return Some(copy.display().to_string());
    }
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return None;
    }
    let mut h: u64 = 0xcbf29ce484222325; // FNV-1a, to name the file
    for b in url.bytes() {
        h = (h ^ b as u64).wrapping_mul(0x100000001b3);
    }
    // The picture decoder goes by the file's extension, and a web cover has none: keep it under one that matches
    // what it really is (told by its first bytes).
    let base = cache_dir()?.join(format!("cover-{h:016x}"));
    for ext in ["jpg", "png", "webp", "gif"] {
        let p = base.with_extension(ext);
        if p.is_file() {
            return Some(p.display().to_string());
        }
    }
    let ok = Command::new("curl").args(["-sfL", "--max-time", "6", "-o"]).arg(&base).arg(url).status().map(|s| s.success()).unwrap_or(false);
    if !ok {
        let _ = std::fs::remove_file(&base);
        return None;
    }
    let ext = std::fs::read(&base).ok().and_then(|d| image_ext(&d));
    let Some(ext) = ext else {
        let _ = std::fs::remove_file(&base);
        return None;
    };
    let path = base.with_extension(ext);
    std::fs::rename(&base, &path).ok()?;
    Some(path.display().to_string())
}

/// The extension for a picture, from its first bytes.
fn image_ext(d: &[u8]) -> Option<&'static str> {
    if d.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("jpg")
    } else if d.starts_with(&[0x89, b'P', b'N', b'G']) {
        Some("png")
    } else if d.len() > 12 && &d[0..4] == b"RIFF" && &d[8..12] == b"WEBP" {
        Some("webp")
    } else if d.starts_with(b"GIF8") {
        Some("gif")
    } else {
        None
    }
}

fn media() -> Option<Media> {
    let list = out("busctl", &["--user", "list", "--no-legend", "--no-pager"]);
    let mut best: Option<Media> = None;
    for bus in list.lines().filter_map(|l| l.split_whitespace().next()).filter(|n| n.starts_with("org.mpris.MediaPlayer2.")) {
        let get = |prop: &str| parse_busctl(&out("busctl", &["--user", "--json=short", "get-property", bus, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", prop]));
        let status = get("PlaybackStatus").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let (title, artist, art, len_us) = get("Metadata").map(|d| parse_metadata(&d)).unwrap_or_default();
        let pos_us = get("Position").and_then(|v| v.as_u64().or_else(|| v.as_i64().map(|n| n.max(0) as u64))).unwrap_or(0);
        if title.is_empty() && status != "Playing" {
            continue;
        }
        let m = Media { bus: bus.to_string(), title, artist, playing: status == "Playing", art: if art.is_empty() { None } else { cover_file(&art) }, pos_us, len_us };
        // one that is playing wins over one that is paused
        if best.as_ref().map(|b| !b.playing && m.playing).unwrap_or(true) {
            best = Some(m);
        }
    }
    best
}

fn temperature() -> Option<u32> {
    let mut best: Option<u32> = None;
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let name = std::fs::read_to_string(e.path().join("name")).unwrap_or_default();
        if !matches!(name.trim(), "coretemp" | "k10temp" | "zenpower") {
            continue;
        }
        if let Some(t) = std::fs::read_to_string(e.path().join("temp1_input")).ok().and_then(|t| t.trim().parse::<u32>().ok()) {
            best = best.max(Some(t / 1000));
        }
    }
    best
}

fn disk() -> (u64, u64) {
    let o = out("df", &["-B1", "--output=used,size", "/"]);
    let n: Vec<u64> = o.lines().nth(1).unwrap_or("").split_whitespace().filter_map(|v| v.parse().ok()).collect();
    if n.len() == 2 { (n[0] >> 30, n[1] >> 30) } else { (0, 0) }
}

/// Reads what the notch shows while `active` is set, into `shared`: what is playing, volumes and load
/// every second; networks, Bluetooth, outputs and Night Light (slower, and several programs) every third.
pub fn spawn_poller(active: Arc<AtomicBool>, shared: Arc<Mutex<Snapshot>>) {
    std::thread::spawn(move || {
        let mut prev: Option<(u64, u64)> = None;
        let mut snap = Snapshot::default();
        let mut round = 0u32;
        while active.load(Ordering::Relaxed) {
            let cur = std::fs::read_to_string("/proc/stat").ok().and_then(|s| parse_cpu_stat(&s));
            if let (Some(p), Some(c)) = (prev, cur) {
                if c.1 > p.1 {
                    snap.cpu = (((c.0 - p.0) * 100) / (c.1 - p.1)) as u32;
                }
            }
            prev = cur;
            snap.media = media();
            snap.master = parse_wpctl(&out("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]));
            snap.apps = parse_sink_inputs(&out("pactl", &["-f", "json", "list", "sink-inputs"]));
            snap.mem = std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| parse_meminfo(&m)).unwrap_or_default();
            snap.disk = disk();
            snap.temp = temperature();
            snap.ready = true;
            if let Ok(mut g) = shared.lock() {
                *g = snap.clone();
            }
            if round % 3 == 0 {
                snap.wifi = parse_wifi(&out("nmcli", &["radio", "wifi"]), &out("nmcli", &["-t", "-f", "TYPE", "device"]), &out("nmcli", &["-t", "-f", "NAME,TYPE", "connection", "show", "--active"]));
                snap.bt = {
                    let show = out("timeout", &["2", "bluetoothctl", "show"]);
                    match crate::status::parse_bt(&show, "") {
                        crate::status::Bt::Missing => None,
                        crate::status::Bt::Off => Some((false, String::new())),
                        _ => Some((true, parse_connected_name(&out("timeout", &["2", "bluetoothctl", "devices", "Connected"])))),
                    }
                };
                snap.night = out("kreadconfig6", &["--file", "kwinrc", "--group", "NightColor", "--key", "Active"]).trim() == "true";
                snap.sinks = parse_sinks(&out("pactl", &["-f", "json", "list", "sinks"]));
                snap.default_sink = out("pactl", &["get-default-sink"]).trim().to_string();
                snap.slow_ready = true;
                if let Ok(mut g) = shared.lock() {
                    *g = snap.clone();
                }
            }
            round += 1;
            std::thread::sleep(Duration::from_secs(1));
        }
    });
}

// ---------------------------------------------------------------- the clipboard

/// Run by `wl-paste --watch` with the new clipboard on stdin: one base64 line per copied text, and
/// nothing at all when the copy came from a password manager.
const CLIP_SCRIPT: &str = "wl-paste --list-types 2>/dev/null | grep -q x-kde-passwordManagerHint && exit 0; head -c 65536 | base64 -w0; echo";

/// Starts watching the clipboard for text; what is copied goes into `clips`, in memory only.
pub fn start_clip_watch(clips: Arc<Mutex<Vec<String>>>) -> Option<Child> {
    // a watcher left by an earlier run of the panel
    let _ = Command::new("pkill").args(["-u", &unsafe { libc::getuid() }.to_string(), "-f", "wl-paste --type text --watch"]).status();
    let mut child = Command::new("wl-paste").args(["--type", "text", "--watch", "sh", "-c", CLIP_SCRIPT]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let stdout = child.stdout.take()?;
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(text) = b64_decode(&line).and_then(|b| String::from_utf8(b).ok()) {
                if let Ok(mut g) = clips.lock() {
                    push_clip(&mut g, text);
                }
            }
        }
    });
    Some(child)
}

/// Puts `text` on the clipboard again.
pub fn copy_text(text: &str) {
    if let Ok(mut c) = Command::new("wl-copy").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
        if let Some(mut i) = c.stdin.take() {
            let _ = i.write_all(text.as_bytes());
        }
        // wl-copy stays running to serve the clipboard; the system reaps it when something else is copied
        std::mem::forget(c);
    }
}

// ---------------------------------------------------------------- pictures

fn xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('"', "&quot;")
}

/// A picture file drawn to fill `w` x `h` (cropped to fit) with round corners; None if it will not load.
pub fn load_image(path: &str, w: u32, h: u32, radius: f32) -> Option<Pixmap> {
    // Drawn several times larger and then averaged down: a cover shrunk straight to 58 px loses its thin lines
    // (frames, small print, hairline strokes), which looked like the picture was cut off.
    let k = if w.max(h) <= 160 { 6 } else { 2 };
    load_image_k(path, w, h, radius, k)
}

/// A picture scaled to fill the whole of a screen (cropped to fit), drawn once at that size: for the wallpaper.
pub fn load_cover(path: &str, w: u32, h: u32) -> Option<Pixmap> {
    load_image_k(path, w, h, 0.0, 1)
}

fn load_image_k(path: &str, w: u32, h: u32, radius: f32, k: u32) -> Option<Pixmap> {
    let (bw, bh) = (w * k, h * k);
    let svg = format!(
        "<svg xmlns='http://www.w3.org/2000/svg' xmlns:xlink='http://www.w3.org/1999/xlink' width='{bw}' height='{bh}'><image width='{bw}' height='{bh}' preserveAspectRatio='xMidYMid slice' xlink:href=\"{}\"/></svg>",
        xml(path)
    );
    let tree = resvg::usvg::Tree::from_data(svg.as_bytes(), &resvg::usvg::Options::default()).ok()?;
    let mut big = Pixmap::new(bw, bh)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::identity(), &mut big.as_mut());
    // an unreadable picture leaves it empty
    if !big.data().iter().any(|b| *b != 0) {
        return None;
    }
    let mut pm = Pixmap::new(w, h)?;
    average_down(&big, &mut pm, k);
    round_corners(&mut pm, radius);
    Some(pm)
}

/// Each pixel of `dst` is the average of the k x k block of `src` behind it (premultiplied, so edges stay right).
fn average_down(src: &Pixmap, dst: &mut Pixmap, k: u32) {
    let (sw, dw, dh) = (src.width() as usize, dst.width() as usize, dst.height() as usize);
    let k = k as usize;
    let s = src.data();
    let d = dst.data_mut();
    for y in 0..dh {
        for x in 0..dw {
            let mut acc = [0u32; 4];
            for yy in 0..k {
                let row = ((y * k + yy) * sw + x * k) * 4;
                for xx in 0..k {
                    for c in 0..4 {
                        acc[c] += s[row + xx * 4 + c] as u32;
                    }
                }
            }
            let n = (k * k) as u32;
            for c in 0..4 {
                d[(y * dw + x) * 4 + c] = (acc[c] / n) as u8;
            }
        }
    }
}

/// Fade the corners to transparent along a circle of `radius` (about a pixel of smoothing).
fn round_corners(pm: &mut Pixmap, radius: f32) {
    let (w, h) = (pm.width() as f32, pm.height() as f32);
    let r = radius.min(w.min(h) / 2.0);
    if r <= 0.0 {
        return;
    }
    let stride = pm.width() as usize;
    let rows = pm.height() as usize;
    let data = pm.data_mut();
    for y in 0..rows {
        for x in 0..stride {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let cx = if px < r { r } else if px > w - r { w - r } else { continue };
            let cy = if py < r { r } else if py > h - r { h - r } else { continue };
            let dist = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
            let cover = (r - dist + 0.5).clamp(0.0, 1.0);
            if cover < 1.0 {
                for c in 0..4 {
                    let i = (y * stride + x) * 4 + c;
                    data[i] = (data[i] as f32 * cover) as u8;
                }
            }
        }
    }
}

/// The pictures Settings offers as wallpapers (the same folders).
pub fn wallpaper_list() -> Vec<PathBuf> {
    fn best_image(dir: &Path) -> Option<PathBuf> {
        let mut best: Option<(u64, PathBuf)> = None;
        for e in std::fs::read_dir(dir).ok()?.flatten() {
            let p = e.path();
            let stem = p.file_stem()?.to_string_lossy().to_string();
            let px = stem.split_once('x').and_then(|(w, h)| Some(w.parse::<u64>().ok()? * h.parse::<u64>().ok()?)).unwrap_or(0);
            if best.as_ref().map(|b| px > b.0).unwrap_or(true) {
                best = Some((px, p));
            }
        }
        best.map(|b| b.1)
    }
    let mut list = vec![];
    for dir in ["/usr/share/labyrinth/wallpapers", "/usr/share/wallpapers"] {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let p = if p.is_dir() { best_image(&p.join("contents/images")).unwrap_or(p) } else { p };
                if p.extension().map(|x| matches!(x.to_str(), Some("png" | "jpg" | "jpeg" | "webp"))).unwrap_or(false) {
                    list.push(p);
                }
            }
        }
    }
    list.sort();
    list
}

/// Makes the wallpapers' small pictures in the background; the notch draws what is ready.
pub fn spawn_thumbs(list: Vec<PathBuf>, size: (u32, u32), into: Arc<Mutex<HashMap<PathBuf, Pixmap>>>) {
    std::thread::spawn(move || {
        for p in list {
            if let Some(pm) = load_image(&p.display().to_string(), size.0, size.1, 6.0 * size.1 as f32 / 56.0) {
                if let Ok(mut g) = into.lock() {
                    g.insert(p, pm);
                }
            }
        }
    });
}

// ---------------------------------------------------------------- Sarina's voice

/// Where a recorded question is kept until Sarina has heard it: private to this user, and gone after.
fn voice_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let d = base.join("labyrinth-sarina");
    std::fs::create_dir_all(&d).ok()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
    }
    Some(d)
}

/// Starts recording the microphone (mono, 16 kHz, what speech tools want) to a new file.
pub fn start_recording() -> Result<(Child, PathBuf), String> {
    let dir = voice_dir().ok_or("Could not make a private folder for the recording.")?;
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("ask-{stamp}.wav"));
    let child = Command::new("pw-record").args(["--rate", "16000", "--channels", "1", "--format", "s16"]).arg(&path).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map_err(|_| "Recording needs PipeWire (pw-record), which is not installed.".to_string())?;
    // the model loads while the question is being said, so the answer does not wait for it
    let _ = Command::new("labyrinth-sarina-warm").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    Ok((child, path))
}

/// Ends the recording cleanly (pw-record finishes the file on SIGINT).
pub fn stop_recording(child: &mut Child) {
    let _ = Command::new("kill").args(["-INT", &child.id().to_string()]).status();
    let _ = child.wait();
}

/// What Sarina says aloud for a reply: the answer without the `Heard: "..."` line the card shows above it.
pub fn spoken_part(reply: &str) -> String {
    let mut lines = reply.lines();
    let first = lines.clone().next().unwrap_or("");
    if first.starts_with("Heard: ") {
        lines.next();
    }
    lines.collect::<Vec<_>>().join("\n").trim().to_string()
}

/// Says the answer aloud (`labyrinth-sarina-speak`: a local Piper voice), in the background. A missing voice just means silence.
pub fn speak(text: &str) {
    if text.is_empty() {
        return;
    }
    let text = text.to_string();
    std::thread::spawn(move || {
        use std::io::Write;
        if let Ok(mut c) = Command::new("labyrinth-sarina-speak").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            if let Some(mut si) = c.stdin.take() {
                let _ = si.write_all(text.as_bytes());
            }
            let _ = c.wait();
        }
    });
}

/// Stops whatever Sarina is saying.
pub fn stop_speaking() {
    let _ = Command::new("labyrinth-sarina-speak").arg("--stop").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|mut c| std::thread::spawn(move || { let _ = c.wait(); }));
}

/// Gives the recording to Sarina (`labyrinth-sarina-ask`) and reports her reply or why there is none.
pub fn ask_sarina(wav: PathBuf, conversation: Option<String>, heard: calloop::channel::Sender<String>, done: calloop::channel::Sender<Result<String, String>>) {
    std::thread::spawn(move || {
        let r = run_ask(&wav, conversation.as_deref(), &heard);
        let _ = std::fs::remove_file(&wav); // the question is not kept
        let _ = done.send(r);
    });
}

/// Runs `labyrinth-sarina-ask`, passing on its first line (`Heard: "..."`) as soon as it is printed, so the card can show
/// what was understood while Sarina is still working on the answer.
fn run_ask(wav: &PathBuf, conversation: Option<&str>, heard: &calloop::channel::Sender<String>) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    let mut cmd = Command::new("labyrinth-sarina-ask");
    cmd.arg(wav);
    cmd.env("LABYRINTH_TONE", if labyrinth_conf::PanelConf::load().sarina_tone { "1" } else { "0" });
    if let Some(id) = conversation {
        cmd.env("LABYRINTH_CONVERSATION", id); // her session continues across the turns of a conversation
    }
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|_| "labyrinth-sarina-ask is missing from this system.".to_string())?;
    let mut text = String::new();
    if let Some(out) = child.stdout.take() {
        for (i, line) in BufReader::new(out).lines().map_while(Result::ok).enumerate() {
            if i == 0 {
                if let Some(h) = line.strip_prefix("Heard: ") {
                    let _ = heard.send(h.to_string());
                }
            }
            text.push_str(&line);
            text.push('\n');
        }
    }
    let status = child.wait().map_err(|_| "Sarina could not answer.".to_string())?;
    let text = text.trim().to_string();
    if status.success() {
        return Ok(text);
    }
    let mut err = String::new();
    if let Some(mut e) = child.stderr.take() {
        use std::io::Read;
        let _ = e.read_to_string(&mut err);
    }
    let msg = if text.is_empty() { err.trim().to_string() } else { text };
    Err(if msg.is_empty() { "Sarina could not answer.".to_string() } else { msg })
}

// ---------------------------------------------------------------- acting

#[derive(Clone, Debug, PartialEq)]
pub enum Slider {
    Master,
    App(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum FAct {
    Section(usize),
    Prev,
    PlayPause,
    Next,
    /// A volume slider: the click's place along `rect` gives the level.
    Slide(Slider),
    Start(StartAct),
    /// Copy this item of the clipboard history again.
    Clip(usize),
    ClipClear,
    /// Something in the notification list (shared with the right-hand pane).
    Notif(crate::notify::NAct),
    Wall(PathBuf),
    /// The wallpaper page: -1 back, +1 on.
    WallPage(i32),
    /// Open the full notch from the small card.
    Expand,
    VoiceStop,
    /// The Controls tiles.
    Wifi,
    WifiMore,
    Bt,
    BtMore,
    /// Mute or unmute the sound output.
    Mute,
    /// Show or hide the list of outputs and app volumes.
    Picker,
    Night,
    Dark,
    Sink(String),
    /// Opens or closes the volume's mixer.
    Mixer,
    /// A click away from the open list of outputs just closes it.
    PickerClose,
    /// Stats: show the apps using the most RAM instead of CPU, or the other way.
    TopToggle,
    SpeedTest,
}

#[derive(Clone, Debug)]
pub struct FHit {
    pub rect: (f32, f32, f32, f32),
    pub act: FAct,
}

fn run(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd).args(args).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
}

pub fn media_control(bus: &str, method: &str) {
    run("busctl", &["--user", "call", bus, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", method]);
}

/// Warm tint for the screen (Night Light): constant at 4000 K when on, since a schedule needs a location.
pub fn set_night(on: bool) {
    std::thread::spawn(move || {
        let w = |k: &str, v: &str| {
            let _ = Command::new("kwriteconfig6").args(["--file", "kwinrc", "--group", "NightColor", "--key", k, v]).status();
        };
        w("Active", if on { "true" } else { "false" });
        if on {
            w("Mode", "Constant");
            w("NightTemperature", "4000");
        }
        let _ = Command::new("busctl").args(["--user", "call", "org.kde.KWin", "/KWin", "org.kde.KWin", "reconfigure"]).status();
    });
}

/// Dark or light, the way Settings > Appearance does it (colour scheme, and Kvantum for KDE apps).
pub fn set_dark(dark: bool) {
    std::thread::spawn(move || {
        let scheme = if dark { "WhiteSurDark" } else { "WhiteSur" };
        let _ = Command::new("plasma-apply-colorscheme").arg(scheme).status();
        let _ = Command::new("labyrinth-minos-icon").args(["-", if dark { "dark" } else { "light" }]).status();
        if let Some(home) = std::env::var_os("HOME") {
            let d = PathBuf::from(home).join(".config/Kvantum");
            let _ = std::fs::create_dir_all(&d);
            let _ = std::fs::write(d.join("kvantum.kvconfig"), format!("[General]\ntheme={scheme}\n"));
        }
    });
}

pub fn set_volume(target: &Slider, percent: u32) {
    let p = percent.min(100);
    match target {
        Slider::Master => run("wpctl", &["set-volume", "@DEFAULT_AUDIO_SINK@", &format!("{:.2}", p as f32 / 100.0)]),
        Slider::App(id) => run("pactl", &["set-sink-input-volume", &id.to_string(), &format!("{p}%")]),
    }
}

// ---------------------------------------------------------------- drawing

const WHITE: Rgba = (255, 255, 255, 255);
const DIM: Rgba = (150, 150, 165, 255);
const FAINT: Rgba = (60, 60, 72, 255);
const RED: Rgba = (232, 120, 120, 255);

/// Whether there is anything to show: the small card only appears when something is playing.
pub fn visible(m: &Model, st: &FyState) -> bool {
    st.open || st.voice_ui || m.fylgja.snap.media.is_some()
}

/// The card's size in logical pixels for what it is showing.
pub fn card_size(m: &Model, st: &FyState) -> (f32, f32) {
    if st.voice_ui && m.fylgja.voice != Voice::Idle {
        let h = match &m.fylgja.voice {
            Voice::Listening(_) => 128.0,
            Voice::Thinking(h) if !h.is_empty() => 118.0,
            Voice::Thinking(_) => 96.0,
            Voice::Reply(_) => 230.0,
            _ => 130.0,
        };
        return (440.0, h);
    }
    if st.open {
        return match st.section {
            0 => (440.0, controls_height(m, st)),
            2 => (crate::stats::CARD_W, crate::stats::card_h(m)),
            NOTIFS => (440.0, 372.0),
            _ => (440.0, 192.0),
        };
    }
    (380.0, 96.0)
}

/// The card's place; `slide` is 0 when it is tucked up out of sight and 1 when it hangs fully down.
pub fn card_rect(screen_w: f32, size: (f32, f32), slide: f32) -> (f32, f32, f32, f32) {
    ((screen_w - size.0) / 2.0, -size.1 * (1.0 - slide), size.0, size.1)
}

/// How far an open or close has got after `ms`, eased so it slows as it arrives.
pub fn ease(ms: u128) -> f32 {
    let t = (ms as f32 / SLIDE_MS as f32).clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t)
}

pub const SLIDE_MS: u128 = 170;

pub fn fit(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> String {
    if paint::text_width(font, size, text) <= max {
        return text.to_string();
    }
    let mut t: String = text.to_string();
    while !t.is_empty() && paint::text_width(font, size, &format!("{t}…")) > max {
        t.pop();
    }
    format!("{t}…")
}

fn slider(pix: &mut Pixmap, s: f32, x: f32, cy: f32, w: f32, percent: u32, c: Rgba) {
    paint::fill_rounded(pix, x * s, (cy - 3.0) * s, w * s, 6.0 * s, 3.0 * s, FAINT);
    let fill = w * percent.min(100) as f32 / 100.0;
    if fill > 1.0 {
        paint::fill_rounded(pix, x * s, (cy - 3.0) * s, fill * s, 6.0 * s, 3.0 * s, c);
    }
}

fn bar(pix: &mut Pixmap, s: f32, x: f32, cy: f32, w: f32, frac: f32, c: Rgba) {
    paint::fill_rounded(pix, x * s, (cy - 4.0) * s, w * s, 8.0 * s, 4.0 * s, FAINT);
    let fill = w * frac.clamp(0.0, 1.0);
    if fill > 1.0 {
        paint::fill_rounded(pix, x * s, (cy - 4.0) * s, fill * s, 8.0 * s, 4.0 * s, c);
    }
}

pub fn over(p: (f32, f32), r: (f32, f32, f32, f32)) -> bool {
    p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3
}

/// A round-cornered button with a symbolic icon; a hit is added for it.
fn icon_button(pix: &mut Pixmap, s: f32, icons: &mut Icons, hits: &mut Vec<FHit>, pointer: (f32, f32), rect: (f32, f32, f32, f32), icon: &str, act: FAct) {
    if over(pointer, rect) {
        paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 10.0 * s, (255, 255, 255, 28));
    }
    let px = (20.0 * s) as u32;
    if let Some(ic) = icons.get_tinted(icon, px, WHITE) {
        paint::draw_pixmap(pix, ic, ((rect.0 + (rect.2 - 20.0) / 2.0) * s) as i32, ((rect.1 + (rect.3 - 20.0) / 2.0) * s) as i32);
    }
    hits.push(FHit { rect, act });
}

fn media_buttons(pix: &mut Pixmap, s: f32, icons: &mut Icons, hits: &mut Vec<FHit>, pointer: (f32, f32), x: f32, y: f32, playing: bool) {
    let play = if playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" };
    for (i, (icon, act)) in [("media-skip-backward-symbolic", FAct::Prev), (play, FAct::PlayPause), ("media-skip-forward-symbolic", FAct::Next)].into_iter().enumerate() {
        icon_button(pix, s, icons, hits, pointer, (x + 36.0 * i as f32, y, 34.0, 34.0), icon, act);
    }
}

/// One tile of the Controls grid: a round icon, a label and a line under it; lit when on. A tile may
/// have a chevron at its right with an action of its own.
#[allow(clippy::too_many_arguments)]
fn tile(pix: &mut Pixmap, s: f32, fonts: &Fonts, icons: &mut Icons, hits: &mut Vec<FHit>, pointer: (f32, f32), rect: (f32, f32, f32, f32), on: bool, icon: &str, label: &str, sub: &str, act: FAct, more: Option<FAct>) {
    let hot = over(pointer, rect);
    let fill = match (on, hot) {
        (true, true) => (140, 100, 226, 255),
        (true, false) => paint::accent(255),
        (false, true) => (44, 44, 54, 255),
        (false, false) => (30, 30, 38, 255),
    };
    paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 14.0 * s, fill);
    let (x, y, w, h) = rect;
    let chev = if more.is_some() { 26.0 } else { 0.0 };
    paint::fill_circle(pix, (x + 24.0) * s, (y + h / 2.0) * s, 14.0 * s, (255, 255, 255, if on { 40 } else { 20 }));
    if let Some(ic) = icons.get_tinted(icon, (16.0 * s) as u32, WHITE) {
        paint::draw_pixmap(pix, ic, ((x + 24.0 - 8.0) * s) as i32, ((y + h / 2.0 - 8.0) * s) as i32);
    }
    let room = (w - 46.0 - chev - 4.0) * s;
    paint::draw_text(pix, &fonts.bold, 12.0 * s, (x + 44.0) * s, (y + h / 2.0 - 7.0) * s, WHITE, &fit(&fonts.bold, 12.0 * s, label, room));
    paint::draw_text(pix, &fonts.regular, 10.0 * s, (x + 44.0) * s, (y + h / 2.0 + 8.0) * s, if on { (232, 224, 255, 255) } else { DIM }, &fit(&fonts.regular, 10.0 * s, sub, room));
    match more {
        Some(m) => {
            hits.push(FHit { rect: (x, y, w - chev, h), act });
            let c = (x + w - chev, y, chev, h);
            paint::fill_rounded(pix, (c.0) * s, (y + 10.0) * s, 1.0 * s, (h - 20.0) * s, 0.5 * s, (255, 255, 255, 40));
            if let Some(ic) = icons.get_tinted("pan-down-symbolic", (14.0 * s) as u32, WHITE) {
                paint::draw_pixmap(pix, ic, ((c.0 + 6.0) * s) as i32, ((y + h / 2.0 - 7.0) * s) as i32);
            }
            hits.push(FHit { rect: c, act: m });
        }
        None => hits.push(FHit { rect, act }),
    }
}

/// How tall the Controls card is: shorter when nothing is playing, taller with the mixer open.
pub fn controls_height(m: &Model, st: &FyState) -> f32 {
    let snap = &m.fylgja.snap;
    let mixer = if st.mixer && !snap.apps.is_empty() { snap.apps.len().min(4) as f32 * 26.0 + 8.0 } else { 0.0 };
    let media = if snap.media.is_some() { 116.0 } else { 0.0 };
    174.0 + media + mixer
}

fn clock(us: u64) -> String {
    format!("{}:{:02}", us / 60_000_000, us / 1_000_000 % 60)
}

/// Controls: Wi-Fi, Bluetooth, sound output, Night Light, dark mode, the volume, and what is playing.
#[allow(clippy::too_many_arguments)]
fn paint_controls(pix: &mut Pixmap, s: f32, m: &Model, st: &FyState, card: (f32, f32, f32, f32), pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons, hits: &mut Vec<FHit>) {
    let (cx, cy, cw, _ch) = card;
    let (x0, inner) = (cx + 20.0, cw - 40.0);
    let snap = &m.fylgja.snap;
    let y0 = cy + 22.0;
    let tw = (inner - 16.0) / 3.0;

    // row 1: Wi-Fi, Bluetooth, sound output
    let wait = !snap.slow_ready;
    let (wifi_on, wifi_sub) = match &snap.wifi {
        Some((true, name)) => (true, if name.is_empty() { "On".to_string() } else { name.clone() }),
        Some((false, _)) => (false, "Off".to_string()),
        None => (false, if matches!(m.status.net, crate::status::Net::Wired) { "Wired".to_string() } else { "No adapter".to_string() }),
    };
    let wifi_sub = if wait { "…".to_string() } else { wifi_sub };
    tile(pix, s, fonts, icons, hits, pointer, (x0, y0, tw, 46.0), wifi_on, "network-wireless-symbolic", "Wi-Fi", &wifi_sub, FAct::Wifi, Some(FAct::WifiMore));
    let (bt_on, bt_sub) = match &snap.bt {
        Some((true, name)) => (true, if name.is_empty() { "On".to_string() } else { name.clone() }),
        Some((false, _)) => (false, "Off".to_string()),
        None => (false, "No adapter".to_string()),
    };
    let bt_sub = if wait { "…".to_string() } else { bt_sub };
    tile(pix, s, fonts, icons, hits, pointer, (x0 + tw + 8.0, y0, tw, 46.0), bt_on, if bt_on { "bluetooth-active-symbolic" } else { "bluetooth-disabled-symbolic" }, "Bluetooth", &bt_sub, FAct::Bt, Some(FAct::BtMore));
    let muted = matches!(snap.master, Some((_, true)));
    let sink = if wait { "…".to_string() } else { snap.sinks.iter().find(|(n, _)| *n == snap.default_sink).map(|(_, d)| d.clone()).unwrap_or_else(|| "No output".to_string()) };
    tile(pix, s, fonts, icons, hits, pointer, (x0 + 2.0 * (tw + 8.0), y0, tw, 46.0), !muted && snap.master.is_some() || st.picker, "audio-volume-high-symbolic", "Audio", &sink, FAct::Mute, if snap.sinks.is_empty() { None } else { Some(FAct::Picker) });

    // row 2: Night Light, dark mode
    let hw = (inner - 8.0) / 2.0;
    tile(pix, s, fonts, icons, hits, pointer, (x0, y0 + 54.0, hw, 46.0), snap.night, "night-light-symbolic", "Night Light", if wait { "…" } else if snap.night { "On" } else { "Off" }, FAct::Night, None);
    tile(pix, s, fonts, icons, hits, pointer, (x0 + hw + 8.0, y0 + 54.0, hw, 46.0), m.dark, "weather-clear-night-symbolic", "Dark mode", if m.dark { "On" } else { "Off" }, FAct::Dark, None);

    // the master volume, with a caret that drops down the mixer (only when an app is playing sound)
    let has_apps_early = !snap.apps.is_empty();
    let vy = y0 + 108.0;
    let vr = (x0, vy, inner, 30.0);
    paint::fill_rounded(pix, vr.0 * s, vr.1 * s, vr.2 * s, vr.3 * s, 15.0 * s, (30, 30, 38, 255));
    let pct = snap.master.map(|(p, _)| p).unwrap_or(0);
    let fillw = (inner * pct.min(100) as f32 / 100.0).max(30.0);
    if snap.master.is_some() {
        paint::fill_rounded(pix, vr.0 * s, vr.1 * s, fillw * s, vr.3 * s, 15.0 * s, if muted { (90, 90, 104, 255) } else { paint::accent(255) });
    }
    if let Some(ic) = icons.get_tinted(if muted { "audio-volume-muted-symbolic" } else { "audio-volume-high-symbolic" }, (16.0 * s) as u32, WHITE) {
        paint::draw_pixmap(pix, ic, ((x0 + 10.0) * s) as i32, ((vy + 7.0) * s) as i32);
    }
    paint::draw_text(pix, &fonts.bold, 11.0 * s, (x0 + 34.0) * s, (vy + 15.0) * s, WHITE, "Master");
    let label = if snap.master.is_some() { format!("{pct}%") } else if snap.ready { "No sound output".to_string() } else { String::new() };
    let lw = paint::text_width(&fonts.bold, 12.0 * s, &label);
    let label_right = if has_apps_early { x0 + inner - 38.0 } else { x0 + inner - 14.0 };
    paint::draw_text(pix, &fonts.bold, 12.0 * s, label_right * s - lw, (vy + 15.0) * s, WHITE, &label);
    let has_apps = !snap.apps.is_empty();
    let caret = (x0 + inner - 34.0, vy, 34.0, 30.0);
    if has_apps && over(pointer, caret) {
        paint::fill_rounded(pix, (caret.0 + 4.0) * s, (vy + 3.0) * s, 26.0 * s, 24.0 * s, 12.0 * s, (255, 255, 255, 30));
    }
    if has_apps {
        if let Some(ic) = icons.get_tinted(if st.mixer { "pan-up-symbolic" } else { "pan-down-symbolic" }, (14.0 * s) as u32, WHITE) {
            paint::draw_pixmap(pix, ic, ((caret.0 + 10.0) * s) as i32, ((vy + 8.0) * s) as i32);
        }
    }
    if snap.master.is_some() {
        // the whole bar is the slider: a click sets the level, and so does dragging along it
        hits.push(FHit { rect: (vr.0, vr.1, if has_apps { vr.2 - 38.0 } else { vr.2 }, vr.3), act: FAct::Slide(Slider::Master) });
    }
    if has_apps {
        hits.push(FHit { rect: caret, act: FAct::Mixer });
    }

    // the mixer: every app that is playing sound, each with its own volume
    let mut next = vy + 40.0;
    if st.mixer && has_apps {
        paint::fill_rounded(pix, x0 * s, (vy + 36.0) * s, inner * s, (snap.apps.len().min(4) as f32 * 26.0 + 4.0) * s, 12.0 * s, (24, 24, 30, 255));
        for (i, a) in snap.apps.iter().take(4).enumerate() {
            let y = vy + 52.0 + 26.0 * i as f32;
            paint::draw_text(pix, &fonts.regular, 11.5 * s, (x0 + 14.0) * s, y * s, WHITE, &fit(&fonts.regular, 11.5 * s, &a.name, 96.0 * s));
            let (sx, sw) = (x0 + 120.0, inner - 120.0 - 56.0);
            slider(pix, s, sx, y, sw, a.percent, (176, 150, 232, 255));
            let p = format!("{}%", a.percent);
            let pw = paint::text_width(&fonts.regular, 11.0 * s, &p);
            paint::draw_text(pix, &fonts.regular, 11.0 * s, (x0 + inner - 14.0) * s - pw, y * s, DIM, &p);
            hits.push(FHit { rect: (sx, y - 10.0, sw, 20.0), act: FAct::Slide(Slider::App(a.id)) });
        }
        next += snap.apps.len().min(4) as f32 * 26.0 + 8.0;
    }

    // what is playing (left out when nothing is)
    let (my, mh) = (next + 0.0, 106.0);
    if let Some(md) = &snap.media {
        paint::fill_rounded(pix, x0 * s, my * s, inner * s, mh * s, 14.0 * s, (24, 24, 30, 255));
        paint::draw_text(pix, &fonts.regular, 10.0 * s, (x0 + 14.0) * s, (my + 14.0) * s, DIM, &fit(&fonts.regular, 10.0 * s, &sink, 200.0 * s));
        let art = (x0 + 14.0, my + 26.0, 58.0, 58.0);
        match &m.fylgja.art {
            Some(a) => paint::draw_pixmap(pix, a, (art.0 * s) as i32, (art.1 * s) as i32),
            None => paint::fill_rounded(pix, art.0 * s, art.1 * s, art.2 * s, art.3 * s, 8.0 * s, (44, 44, 54, 255)),
        }
        let tx = x0 + 84.0;
        let room = (inner - 84.0 - 124.0) * s;
        let title = if md.title.is_empty() { "Playing".to_string() } else { fit(&fonts.bold, 14.0 * s, &md.title, room) };
        paint::draw_text(pix, &fonts.bold, 14.0 * s, tx * s, (my + 46.0) * s, WHITE, &title);
        paint::draw_text(pix, &fonts.regular, 11.0 * s, tx * s, (my + 64.0) * s, DIM, &fit(&fonts.regular, 11.0 * s, &md.artist, room));
        media_buttons(pix, s, icons, hits, pointer, x0 + inner - 120.0, my + 34.0, md.playing);
        if md.len_us > 0 {
            let (bx, bw, by) = (x0 + 14.0 + 40.0, inner - 28.0 - 80.0, my + mh - 12.0);
            paint::draw_text(pix, &fonts.regular, 10.0 * s, (x0 + 14.0) * s, by * s, DIM, &clock(md.pos_us));
            let end = clock(md.len_us);
            let ew = paint::text_width(&fonts.regular, 10.0 * s, &end);
            paint::draw_text(pix, &fonts.regular, 10.0 * s, (x0 + inner - 14.0) * s - ew, by * s, DIM, &end);
            slider(pix, s, bx, by, bw, ((md.pos_us as f32 / md.len_us as f32) * 100.0).min(100.0) as u32, paint::accent(255));
        }
    }

    // the list of outputs drops down from the Audio tile over whatever is below it
    if st.picker {
        hits.push(FHit { rect: (cx, cy, cw, 400.0), act: FAct::PickerClose });
        let rows = snap.sinks.len().clamp(1, 5) as f32;
        let (lw, lh) = (220.0, rows * 30.0 + 10.0);
        let lx = x0 + inner - lw;
        let ly = y0 + 50.0;
        paint::fill_rounded(pix, lx * s, ly * s, lw * s, lh * s, 12.0 * s, (38, 38, 48, 255));
        paint::stroke_rounded(pix, (lx + 0.5) * s, (ly + 0.5) * s, (lw - 1.0) * s, (lh - 1.0) * s, 12.0 * s, s, (255, 255, 255, 36));
        if snap.sinks.is_empty() {
            paint::draw_text(pix, &fonts.regular, 11.5 * s, (lx + 14.0) * s, (ly + 22.0) * s, DIM, "No sound outputs found");
        }
        for (i, (name, desc)) in snap.sinks.iter().take(5).enumerate() {
            let rect = (lx + 5.0, ly + 5.0 + 30.0 * i as f32, lw - 10.0, 28.0);
            let current = *name == snap.default_sink;
            if current || over(pointer, rect) {
                paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 8.0 * s, if current { paint::accent(90) } else { (255, 255, 255, 24) });
            }
            paint::draw_text(pix, &fonts.regular, 11.5 * s, (rect.0 + 10.0) * s, (rect.1 + 14.0) * s, WHITE, &fit(&fonts.regular, 11.5 * s, desc, (rect.2 - 20.0) * s));
            hits.push(FHit { rect, act: FAct::Sink(name.clone()) });
        }
    }
}

/// Draws the notch and returns its card and what can be clicked, in logical pixels.
pub fn paint(pix: &mut Pixmap, s: f32, m: &Model, st: &FyState, slide: f32, pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons) -> ((f32, f32, f32, f32), Vec<FHit>) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let w = pix.width() as f32 / s;
    if !visible(m, st) {
        return ((w / 2.0, 0.0, 0.0, 0.0), vec![]);
    }
    let card = card_rect(w, card_size(m, st), slide);
    let (cx, cy, cw, ch) = card;
    // pure black, square at the top where it joins the bar, round at the bottom
    paint::fill_rounded(pix, cx * s, (cy - 20.0) * s, cw * s, (ch + 20.0) * s, 20.0 * s, (0, 0, 0, 255));
    let mut hits: Vec<FHit> = vec![];
    let pad = 20.0;
    let (x0, inner) = (cx + pad, cw - 2.0 * pad);
    let d = &m.fylgja;
    let snap = &d.snap;
    let centred = |pix: &mut Pixmap, font: &ab_glyph::FontVec, size: f32, y: f32, c: Rgba, text: &str| {
        let tw = paint::text_width(font, size * s, text);
        paint::draw_text(pix, font, size * s, (cx + cw / 2.0) * s - tw / 2.0, y * s, c, text);
    };

    // ---- Sarina's voice card
    if st.voice_ui && d.voice != Voice::Idle {
        match &d.voice {
            Voice::Listening(secs) => {
                // a pulse that breathes with the seconds
                let r = if secs % 2 == 0 { 9.0 } else { 7.0 };
                paint::fill_circle(pix, (x0 + 14.0) * s, (cy + 34.0) * s, (r + 7.0) * s, paint::accent(70));
                paint::fill_circle(pix, (x0 + 14.0) * s, (cy + 34.0) * s, r * s, paint::accent(255));
                paint::draw_text(pix, &fonts.bold, 15.0 * s, (x0 + 42.0) * s, (cy + 34.0) * s, WHITE, &format!("Listening…  {}:{:02}", secs / 60, secs % 60));
                paint::draw_text(pix, &fonts.regular, 12.0 * s, x0 * s, (cy + 72.0) * s, DIM, "Ask your question. Press the shortcut again to send it, Esc to cancel.");
                let b = (x0 + inner - 74.0, cy + ch - 40.0, 74.0, 28.0);
                paint::fill_rounded(pix, b.0 * s, b.1 * s, b.2 * s, b.3 * s, 10.0 * s, if over(pointer, b) { (150, 110, 232, 255) } else { paint::accent(255) });
                let lw = paint::text_width(&fonts.bold, 12.5 * s, "Send");
                paint::draw_text(pix, &fonts.bold, 12.5 * s, (b.0 + b.2 / 2.0) * s - lw / 2.0, (b.1 + b.3 / 2.0) * s, WHITE, "Send");
                hits.push(FHit { rect: b, act: FAct::VoiceStop });
            }
            Voice::Thinking(heard) => {
                paint::fill_circle(pix, (x0 + 14.0) * s, (cy + 40.0) * s, 7.0 * s, paint::accent(255));
                paint::draw_text(pix, &fonts.bold, 15.0 * s, (x0 + 42.0) * s, (cy + 40.0) * s, WHITE, "Sarina is thinking…");
                if !heard.is_empty() {
                    paint::draw_text(pix, &fonts.regular, 12.5 * s, x0 * s, (cy + 76.0) * s, DIM, &fit(&fonts.regular, 12.5 * s, heard, inner * s));
                }
            }
            Voice::Reply(text) => {
                paint::draw_text(pix, &fonts.bold, 13.0 * s, x0 * s, (cy + 26.0) * s, paint::accent(255), "Sarina");
                let lines = wrap(&fonts.regular, 13.0 * s, text, inner * s);
                let rows = 8;
                for (i, l) in lines.iter().take(rows).enumerate() {
                    let l = if i == rows - 1 && lines.len() > rows { format!("{l}…") } else { l.clone() };
                    paint::draw_text(pix, &fonts.regular, 13.0 * s, x0 * s, (cy + 52.0 + 20.0 * i as f32) * s, WHITE, &l);
                }
            }
            Voice::Failed(msg) => {
                paint::draw_text(pix, &fonts.bold, 13.0 * s, x0 * s, (cy + 26.0) * s, RED, "Sarina could not be asked");
                for (i, l) in wrap(&fonts.regular, 12.5 * s, msg, inner * s).iter().take(4).enumerate() {
                    paint::draw_text(pix, &fonts.regular, 12.5 * s, x0 * s, (cy + 52.0 + 19.0 * i as f32) * s, DIM, l);
                }
            }
            Voice::Idle => {}
        }
        return (card, hits);
    }

    // ---- the small card: what is playing
    if !st.open {
        if !snap.ready {
            return (card, hits);
        }
        match &snap.media {
            Some(md) => {
                let mut tx = x0;
                if let Some(art) = &d.art {
                    paint::draw_pixmap(pix, art, (x0 * s) as i32, ((cy + 14.0) * s) as i32);
                    tx = x0 + art.width() as f32 / s + 12.0;
                } else {
                    paint::fill_rounded(pix, x0 * s, (cy + 14.0) * s, 58.0 * s, 58.0 * s, 8.0 * s, (30, 30, 38, 255));
                    tx = tx.max(x0 + 70.0);
                }
                let room = (cx + cw - pad - 112.0) - tx;
                let title = if md.title.is_empty() { "Playing".to_string() } else { fit(&fonts.bold, 14.0 * s, &md.title, room * s) };
                paint::draw_text(pix, &fonts.bold, 14.0 * s, tx * s, (cy + 34.0) * s, WHITE, &title);
                paint::draw_text(pix, &fonts.regular, 12.0 * s, tx * s, (cy + 54.0) * s, DIM, &fit(&fonts.regular, 12.0 * s, &md.artist, room * s));
                media_buttons(pix, s, icons, &mut hits, pointer, cx + cw - pad - 108.0, cy + 26.0, md.playing);
                // a click anywhere else on the card opens the full notch
                hits.insert(0, FHit { rect: (cx, cy, cw, ch), act: FAct::Expand });
            }
            None => {}
        }
        return (card, hits);
    }

    // ---- the full notch: the section's name, then the content, then the dots
    // (the section titles are drawn by the bar, in the black band above this card)
    let section = st.section;
    let top = cy + 22.0;
    let note = |pix: &mut Pixmap, text: &str| centred(pix, &fonts.regular, 13.0, top + 70.0, DIM, text);

    match section {
        0 => paint_controls(pix, s, m, st, card, pointer, fonts, icons, &mut hits),
        1 => {
            let tw = paint::text_width(&fonts.bold, 34.0 * s, &m.clock_secs);
            paint::draw_text(pix, &fonts.bold, 34.0 * s, (cx + cw / 2.0) * s - tw / 2.0, (top + 36.0) * s, WHITE, &m.clock_secs);
            let (y, mo, dd) = m.today;
            centred(pix, &fonts.regular, 14.0, top + 78.0, DIM, &crate::calendar::full_date(y, mo, dd, false));
            let open = m.wins.iter().filter(|w| w.listed()).count();
            centred(pix, &fonts.regular, 12.5, top + 108.0, DIM, &format!("{open} window{} open", if open == 1 { "" } else { "s" }));
        }
        2 => crate::stats::paint(pix, s, m, card, pointer, fonts, icons, &mut hits),
        CLIPBOARD => {
            if !d.history_on {
                note(pix, "Clipboard history is off. Turn it on in Settings > Appearance.");
            } else if d.clips.is_empty() {
                note(pix, "Nothing copied yet");
            } else {
                for (i, c) in d.clips.iter().take(5).enumerate() {
                    let rect = (x0, top + 34.0 * i as f32 - 6.0, inner, 30.0);
                    if over(pointer, rect) {
                        paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 8.0 * s, (255, 255, 255, 26));
                    }
                    paint::draw_text(pix, &fonts.regular, 12.5 * s, (rect.0 + 10.0) * s, (rect.1 + 15.0) * s, WHITE, &one_line(&fonts.regular, 12.5 * s, c, (inner - 20.0) * s));
                    hits.push(FHit { rect, act: FAct::Clip(i) });
                }
                let b = (cx + cw - pad - 44.0, cy + ch - 32.0, 44.0, 24.0);
                let lw = paint::text_width(&fonts.regular, 11.5 * s, "Clear");
                paint::draw_text(pix, &fonts.regular, 11.5 * s, (b.0 + b.2 / 2.0) * s - lw / 2.0, (b.1 + b.3 / 2.0) * s, if over(pointer, b) { WHITE } else { DIM }, "Clear");
                hits.push(FHit { rect: b, act: FAct::ClipClear });
            }
        }
        NOTIFS => {
            let area = (x0, cy + 14.0, inner, ch - 22.0);
            for (rect, act) in crate::notify::paint_list(pix, s, area, &m.notifs, pointer, crate::notify::now_secs(), fonts, icons) {
                hits.push(FHit { rect, act: FAct::Notif(act) });
            }
        }
        WALLPAPER => {
            let n = d.walls.len();
            if n == 0 {
                note(pix, "No wallpapers found");
            } else {
                let pages = n.div_ceil(WALL_PAGE);
                let page = d.wall_page.min(pages - 1);
                let thumbs = d.thumbs.as_ref().and_then(|t| t.lock().ok());
                let (tw, th, gap) = ((inner - 3.0 * 8.0) / 4.0, ((inner - 24.0) / 4.0) * 9.0 / 16.0, 8.0);
                for (i, p) in d.walls.iter().skip(page * WALL_PAGE).take(WALL_PAGE).enumerate() {
                    let (c, r) = (i % 4, i / 4);
                    let rect = (x0 + c as f32 * (tw + gap), top - 6.0 + r as f32 * (th + gap), tw, th);
                    match thumbs.as_ref().and_then(|t| t.get(p)) {
                        Some(pm) => paint::draw_pixmap(pix, pm, (rect.0 * s) as i32, (rect.1 * s) as i32),
                        None => paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 6.0 * s, (30, 30, 38, 255)),
                    }
                    if over(pointer, rect) {
                        paint::stroke_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 6.0 * s, 2.0 * s, paint::accent(255));
                    }
                    hits.push(FHit { rect, act: FAct::Wall(p.clone()) });
                }
                if pages > 1 {
                    for (label, by, rect) in [("‹", -1, (cx + pad, cy + ch - 32.0, 28.0, 24.0)), ("›", 1, (cx + cw - pad - 28.0, cy + ch - 32.0, 28.0, 24.0))] {
                        let lw = paint::text_width(&fonts.bold, 16.0 * s, label);
                        paint::draw_text(pix, &fonts.bold, 16.0 * s, (rect.0 + rect.2 / 2.0) * s - lw / 2.0, (rect.1 + rect.3 / 2.0) * s, if over(pointer, rect) { WHITE } else { DIM }, label);
                        hits.push(FHit { rect, act: FAct::WallPage(by) });
                    }
                }
            }
        }
        6 => {
            let mut acts = vec![("system-lock-screen-symbolic", "Lock", PowerAct::Lock), ("system-suspend-symbolic", "Sleep", PowerAct::Sleep)];
            if m.hibernate {
                acts.push(("system-hibernate-symbolic", "Hibernate", PowerAct::Hibernate));
            }
            acts.push(("system-log-out-symbolic", "Log out", PowerAct::Logout));
            acts.push(("system-reboot-symbolic", "Restart", PowerAct::Restart));
            acts.push(("system-shutdown-symbolic", "Shut down", PowerAct::Shutdown));
            let cell = inner / acts.len() as f32;
            for (i, (icon, label, act)) in acts.into_iter().enumerate() {
                let rect = (x0 + cell * i as f32 + 3.0, top + 14.0, cell - 6.0, 84.0);
                paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 14.0 * s, if over(pointer, rect) { (255, 255, 255, 34) } else { (255, 255, 255, 14) });
                if let Some(ic) = icons.get_tinted(icon, (26.0 * s) as u32, WHITE) {
                    paint::draw_pixmap(pix, ic, ((rect.0 + (rect.2 - 26.0) / 2.0) * s) as i32, ((rect.1 + 16.0) * s) as i32);
                }
                let lw = paint::text_width(&fonts.regular, 11.5 * s, label);
                paint::draw_text(pix, &fonts.regular, 11.5 * s, (rect.0 + rect.2 / 2.0) * s - lw / 2.0, (rect.1 + 64.0) * s, DIM, label);
                hits.push(FHit { rect, act: FAct::Start(StartAct::Power(act)) });
            }
        }
        _ => {}
    }

    (card, hits)
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_spoken_part_drops_the_heard_line() {
        assert_eq!(spoken_part("Heard: \"what time is it\"\n\nIt is noon."), "It is noon.");
        assert_eq!(spoken_part("It is noon."), "It is noon.");
        assert_eq!(spoken_part("Heard: \"x\""), "");
    }

    use super::*;

    #[test]
    fn volume_lines() {
        assert_eq!(parse_wpctl("Volume: 0.45\n"), Some((45, false)));
        assert_eq!(parse_wpctl("Volume: 1.00 [MUTED]\n"), Some((100, true)));
        assert_eq!(parse_wpctl(""), None);
    }

    #[test]
    fn apps_playing_sound() {
        let j = r#"[{"index":42,"volume":{"front-left":{"value":29491,"value_percent":"45%"},"front-right":{"value":29491,"value_percent":"45%"}},"properties":{"application.name":"Spotify","media.name":"Song"}}]"#;
        assert_eq!(parse_sink_inputs(j), vec![AppVol { id: 42, name: "Spotify".into(), percent: 45 }]);
        assert!(parse_sink_inputs("not json").is_empty());
    }

    #[test]
    fn what_is_playing() {
        let j = r#"{"type":"a{sv}","data":{"xesam:title":{"type":"s","data":"Gimme Shelter"},"xesam:artist":{"type":"as","data":["The Rolling Stones","Merry Clayton"]},"mpris:artUrl":{"type":"s","data":"file:///tmp/a%20b.jpg"}}}"#;
        let d = parse_busctl(j).unwrap();
        assert_eq!(parse_metadata(&d), ("Gimme Shelter".to_string(), "The Rolling Stones, Merry Clayton".to_string(), "file:///tmp/a%20b.jpg".to_string(), 0));
        let long = parse_busctl(r#"{"type":"a{sv}","data":{"mpris:length":{"type":"x","data":242000000}}}"#).unwrap();
        assert_eq!(parse_metadata(&long).3, 242_000_000);
        assert_eq!(clock(242_000_000), "4:02");
        assert_eq!(cover_file("file:///tmp/a%20b.jpg").as_deref(), Some("/tmp/a b.jpg"));
        assert_eq!(cover_file("ftp://x/y"), None);
        assert_eq!(parse_busctl(r#"{"type":"s","data":"Playing"}"#).unwrap().as_str(), Some("Playing"));
    }

    #[test]
    fn controls_readings() {
        let sinks = r#"[{"index":1,"name":"alsa_output.pci.analog","description":"Speakers"},{"name":"bluez_sink.X","description":"Headphones"}]"#;
        assert_eq!(parse_sinks(sinks), vec![("alsa_output.pci.analog".to_string(), "Speakers".to_string()), ("bluez_sink.X".to_string(), "Headphones".to_string())]);
        assert!(parse_sinks("").is_empty());
        assert_eq!(parse_wifi("enabled\n", "ethernet\nwifi\nloopback\n", "Wired connection 1:802-3-ethernet\nHome 5G:802-11-wireless\n"), Some((true, "Home 5G".to_string())));
        assert_eq!(parse_wifi("disabled\n", "wifi\n", ""), Some((false, String::new())));
        assert_eq!(parse_wifi("enabled\n", "ethernet\nloopback\n", ""), None);
        assert_eq!(parse_connected_name("Device AA:BB:CC:DD:EE:FF WH-1000XM4\n"), "WH-1000XM4");
        assert_eq!(parse_connected_name(""), "");
    }

    #[test]
    fn load_numbers() {
        // user nice system idle iowait irq softirq steal
        let (busy, total) = parse_cpu_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4 5\n").unwrap();
        assert_eq!((busy, total), (150, 1000));
        let mem = "MemTotal:       16384000 kB\nMemFree:  1 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(parse_meminfo(mem), Some((8000, 16000)));
    }

    #[test]
    fn covers_are_named_by_what_they_are() {
        assert_eq!(image_ext(&[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]), Some("jpg"));
        assert_eq!(image_ext(b"\x89PNG\r\n\x1a\n"), Some("png"));
        assert_eq!(image_ext(b"RIFF\x00\x00\x00\x00WEBPVP8 "), Some("webp"));
        assert_eq!(image_ext(b"<html>not a picture"), None);
    }

    #[test]
    fn base64_and_history() {
        assert_eq!(b64_decode("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(b64_decode("aGk=\n").unwrap(), b"hi");
        assert_eq!(b64_decode("").unwrap(), b"");
        assert!(b64_decode("a*b").is_none());
        let mut l = vec![];
        push_clip(&mut l, "one\n".into());
        push_clip(&mut l, "two".into());
        push_clip(&mut l, "   ".into());
        push_clip(&mut l, "one".into());
        assert_eq!(l, vec!["one".to_string(), "two".to_string()]);
        for i in 0..40 {
            push_clip(&mut l, format!("item {i}"));
        }
        assert_eq!(l.len(), MAX_CLIPS);
        assert_eq!(l[0], "item 39");
    }
}
