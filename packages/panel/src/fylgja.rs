//! Fylgja: the notch. It hangs from the top bar's black tab; hovering opens it and the wheel moves
//! through its sections. This file reads what the sections show (what is playing, volumes, load) with
//! the system's own tools while the notch is open, and draws it. The parsing is separate from the
//! commands so it can be tested.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use resvg::tiny_skia::Pixmap;
use serde_json::Value;

use crate::paint::{self, Fonts, Icons, Rgba};
use crate::shell::{Model, PowerAct, StartAct};

pub const SECTIONS: [&str; 7] = ["Controls", "Today", "Stats", "Clipboard", "Apps", "Wallpaper", "Power"];

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Media {
    /// The player's D-Bus name (org.mpris.MediaPlayer2.…).
    pub bus: String,
    pub title: String,
    pub artist: String,
    pub playing: bool,
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
    pub media: Option<Media>,
    /// The main output's volume in percent, and whether it is muted.
    pub master: Option<(u32, bool)>,
    pub apps: Vec<AppVol>,
    pub cpu: u32,
    /// Memory used and total, in MiB.
    pub mem: (u64, u64),
    /// The root disk, used and total, in GiB.
    pub disk: (u64, u64),
    pub temp: Option<u32>,
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

pub fn parse_metadata(data: &Value) -> (String, String) {
    let text = |k: &str| data.get(k).and_then(|v| v.get("data")).cloned();
    let title = text("xesam:title").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
    let artist = match text("xesam:artist") {
        Some(Value::Array(a)) => a.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(", "),
        Some(Value::String(s)) => s,
        _ => String::new(),
    };
    (title, artist)
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

// ---------------------------------------------------------------- reading

fn out(cmd: &str, args: &[&str]) -> String {
    Command::new(cmd).args(args).output().map(|o| String::from_utf8_lossy(&o.stdout).to_string()).unwrap_or_default()
}

fn media() -> Option<Media> {
    let list = out("busctl", &["--user", "list", "--no-legend", "--no-pager"]);
    let mut best: Option<Media> = None;
    for bus in list.lines().filter_map(|l| l.split_whitespace().next()).filter(|n| n.starts_with("org.mpris.MediaPlayer2.")) {
        let get = |prop: &str| parse_busctl(&out("busctl", &["--user", "--json=short", "get-property", bus, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", prop]));
        let status = get("PlaybackStatus").and_then(|v| v.as_str().map(String::from)).unwrap_or_default();
        let (title, artist) = get("Metadata").map(|d| parse_metadata(&d)).unwrap_or_default();
        if title.is_empty() && status != "Playing" {
            continue;
        }
        let m = Media { bus: bus.to_string(), title, artist, playing: status == "Playing" };
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

/// Reads everything once a second while `active` is set, into `shared`.
pub fn spawn_poller(active: Arc<AtomicBool>, shared: Arc<Mutex<Snapshot>>) {
    std::thread::spawn(move || {
        let mut prev: Option<(u64, u64)> = None;
        while active.load(Ordering::Relaxed) {
            let cur = std::fs::read_to_string("/proc/stat").ok().and_then(|s| parse_cpu_stat(&s));
            let cpu = match (prev, cur) {
                (Some(p), Some(c)) if c.1 > p.1 => (((c.0 - p.0) * 100) / (c.1 - p.1)) as u32,
                _ => shared.lock().map(|s| s.cpu).unwrap_or(0),
            };
            prev = cur;
            let snap = Snapshot {
                media: media(),
                master: parse_wpctl(&out("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"])),
                apps: parse_sink_inputs(&out("pactl", &["-f", "json", "list", "sink-inputs"])),
                cpu,
                mem: std::fs::read_to_string("/proc/meminfo").ok().and_then(|m| parse_meminfo(&m)).unwrap_or_default(),
                disk: disk(),
                temp: temperature(),
            };
            if let Ok(mut g) = shared.lock() {
                *g = snap;
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    });
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
}

#[derive(Clone, Debug)]
pub struct FHit {
    pub rect: (f32, f32, f32, f32),
    pub act: FAct,
}

fn run(cmd: &str, args: &[&str]) {
    let _ = Command::new(cmd).args(args).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn();
}

pub fn media_control(bus: &str, method: &str) {
    run("busctl", &["--user", "call", bus, "/org/mpris/MediaPlayer2", "org.mpris.MediaPlayer2.Player", method]);
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
const ACCENT: Rgba = (124, 84, 214, 255);
const CARD_W: f32 = 440.0;
const CARD_H: f32 = 262.0;

pub fn card_rect(screen_w: f32) -> (f32, f32, f32, f32) {
    ((screen_w - CARD_W) / 2.0, 0.0, CARD_W, CARD_H)
}

fn fit(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> String {
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

fn over(p: (f32, f32), r: (f32, f32, f32, f32)) -> bool {
    p.0 >= r.0 && p.0 < r.0 + r.2 && p.1 >= r.1 && p.1 < r.1 + r.3
}

/// Draws the notch for `section` and returns its card and what can be clicked, in logical pixels.
pub fn paint(pix: &mut Pixmap, s: f32, m: &Model, section: usize, pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons) -> ((f32, f32, f32, f32), Vec<FHit>) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let w = pix.width() as f32 / s;
    let card = card_rect(w);
    let (cx, cy, cw, ch) = card;
    // pure black, square at the top where it joins the bar's tab, round at the bottom
    paint::fill_rounded(pix, cx * s, (cy - 20.0) * s, cw * s, (ch + 20.0) * s, 20.0 * s, (0, 0, 0, 255));
    let mut hits: Vec<FHit> = vec![];
    let pad = 20.0;
    let (x0, inner) = (cx + pad, cw - 2.0 * pad);
    let snap = &m.fylgja;

    // the section's name, then the content, then the dots
    let name = SECTIONS[section];
    let nw = paint::text_width(&fonts.bold, 13.0 * s, name);
    paint::draw_text(pix, &fonts.bold, 13.0 * s, (cx + cw / 2.0) * s - nw / 2.0, (cy + 20.0) * s, WHITE, name);
    let top = cy + 44.0;
    let note = |pix: &mut Pixmap, text: &str| {
        let tw = paint::text_width(&fonts.regular, 13.0 * s, text);
        paint::draw_text(pix, &fonts.regular, 13.0 * s, (cx + cw / 2.0) * s - tw / 2.0, (top + 70.0) * s, DIM, text);
    };

    match section {
        0 => {
            // what is playing
            match &snap.media {
                Some(md) => {
                    let title = if md.title.is_empty() { "Playing".to_string() } else { fit(&fonts.bold, 15.0 * s, &md.title, (inner - 130.0) * s) };
                    paint::draw_text(pix, &fonts.bold, 15.0 * s, x0 * s, (top + 10.0) * s, WHITE, &title);
                    paint::draw_text(pix, &fonts.regular, 12.5 * s, x0 * s, (top + 30.0) * s, DIM, &fit(&fonts.regular, 12.5 * s, &md.artist, (inner - 130.0) * s));
                    let bx = x0 + inner - 120.0;
                    let play = if md.playing { "media-playback-pause-symbolic" } else { "media-playback-start-symbolic" };
                    for (i, (icon, act)) in [("media-skip-backward-symbolic", FAct::Prev), (play, FAct::PlayPause), ("media-skip-forward-symbolic", FAct::Next)].into_iter().enumerate() {
                        let rect = (bx + 40.0 * i as f32, top - 6.0, 36.0, 36.0);
                        if over(pointer, rect) {
                            paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 10.0 * s, (255, 255, 255, 28));
                        }
                        if let Some(ic) = icons.get_tinted(icon, (20.0 * s) as u32, WHITE) {
                            paint::draw_pixmap(pix, ic, ((rect.0 + 8.0) * s) as i32, ((rect.1 + 8.0) * s) as i32);
                        }
                        hits.push(FHit { rect, act });
                    }
                }
                None => {
                    paint::draw_text(pix, &fonts.regular, 13.0 * s, x0 * s, (top + 10.0) * s, DIM, "Nothing is playing");
                }
            }
            // volumes: the main one, then the apps that are making sound
            let y = top + 58.0;
            let row = |pix: &mut Pixmap, hits: &mut Vec<FHit>, y: f32, label: &str, percent: u32, who: Slider, c: Rgba| {
                paint::draw_text(pix, &fonts.regular, 12.5 * s, x0 * s, y * s, WHITE, &fit(&fonts.regular, 12.5 * s, label, 100.0 * s));
                let sx = x0 + 110.0;
                let sw = inner - 110.0 - 44.0;
                slider(pix, s, sx, y, sw, percent, c);
                let p = format!("{percent}%");
                let pw = paint::text_width(&fonts.regular, 12.0 * s, &p);
                paint::draw_text(pix, &fonts.regular, 12.0 * s, (x0 + inner) * s - pw, y * s, DIM, &p);
                hits.push(FHit { rect: (sx, y - 10.0, sw, 20.0), act: FAct::Slide(who) });
            };
            match snap.master {
                Some((p, muted)) => row(pix, &mut hits, y, "Volume", p, Slider::Master, if muted { DIM } else { ACCENT }),
                None => {
                    paint::draw_text(pix, &fonts.regular, 12.5 * s, x0 * s, y * s, DIM, "No sound output found");
                }
            }
            for (i, a) in snap.apps.iter().take(3).enumerate() {
                row(pix, &mut hits, y + 30.0 * (i as f32 + 1.0), &a.name, a.percent, Slider::App(a.id), (176, 150, 232, 255));
            }
        }
        1 => {
            let tw = paint::text_width(&fonts.bold, 34.0 * s, &m.clock_secs);
            paint::draw_text(pix, &fonts.bold, 34.0 * s, (cx + cw / 2.0) * s - tw / 2.0, (top + 36.0) * s, WHITE, &m.clock_secs);
            let (y, mo, d) = m.today;
            let date = crate::calendar::full_date(y, mo, d, false);
            let dw = paint::text_width(&fonts.regular, 14.0 * s, &date);
            paint::draw_text(pix, &fonts.regular, 14.0 * s, (cx + cw / 2.0) * s - dw / 2.0, (top + 78.0) * s, DIM, &date);
            let open = m.wins.iter().filter(|w| w.listed()).count();
            let line = format!("{open} window{} open", if open == 1 { "" } else { "s" });
            let lw = paint::text_width(&fonts.regular, 12.5 * s, &line);
            paint::draw_text(pix, &fonts.regular, 12.5 * s, (cx + cw / 2.0) * s - lw / 2.0, (top + 108.0) * s, DIM, &line);
        }
        2 => {
            let mut y = top + 14.0;
            let mut stat = |pix: &mut Pixmap, label: &str, frac: f32, text: String| {
                paint::draw_text(pix, &fonts.regular, 12.5 * s, x0 * s, y * s, WHITE, label);
                bar(pix, s, x0 + 80.0, y, inner - 80.0 - 120.0, frac, if frac > 0.85 { (232, 100, 100, 255) } else { ACCENT });
                let tw = paint::text_width(&fonts.regular, 12.0 * s, &text);
                paint::draw_text(pix, &fonts.regular, 12.0 * s, (x0 + inner) * s - tw, y * s, DIM, &text);
                y += 34.0;
            };
            stat(pix, "Processor", snap.cpu as f32 / 100.0, format!("{}%", snap.cpu));
            let (mu, mt) = snap.mem;
            stat(pix, "Memory", if mt > 0 { mu as f32 / mt as f32 } else { 0.0 }, format!("{:.1} / {:.0} GB", mu as f32 / 1024.0, mt as f32 / 1024.0));
            let (du, dt) = snap.disk;
            stat(pix, "Disk", if dt > 0 { du as f32 / dt as f32 } else { 0.0 }, format!("{du} / {dt} GB"));
            if let Some(t) = snap.temp {
                stat(pix, "Temperature", t as f32 / 100.0, format!("{t} °C"));
            }
        }
        4 => {
            let apps = crate::shell::pinned_apps(m);
            let (cell, cols) = (62.0, 6usize);
            for (i, a) in apps.iter().take(cols * 2).enumerate() {
                let (c, r) = (i % cols, i / cols);
                let rect = (x0 + 4.0 + c as f32 * cell, top + r as f32 * 78.0, cell - 4.0, 70.0);
                if over(pointer, rect) {
                    paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 10.0 * s, (255, 255, 255, 24));
                }
                if let Some(ic) = icons.get(&a.icon, (36.0 * s) as u32) {
                    paint::draw_pixmap(pix, ic, ((rect.0 + (rect.2 - 36.0) / 2.0) * s) as i32, ((rect.1 + 6.0) * s) as i32);
                }
                let label = fit(&fonts.regular, 10.5 * s, &a.name, (rect.2 - 2.0) * s);
                let lw = paint::text_width(&fonts.regular, 10.5 * s, &label);
                paint::draw_text(pix, &fonts.regular, 10.5 * s, (rect.0 + rect.2 / 2.0) * s - lw / 2.0, (rect.1 + 56.0) * s, DIM, &label);
                hits.push(FHit { rect, act: FAct::Start(StartAct::Launch(a.id.clone())) });
            }
        }
        6 => {
            let mut acts = vec![("system-lock-screen-symbolic", "Lock", PowerAct::Lock), ("system-suspend-symbolic", "Sleep", PowerAct::Sleep)];
            if m.hibernate {
                acts.push(("system-hibernate-symbolic", "Hibernate", PowerAct::Hibernate));
            }
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
        3 => note(pix, "Clipboard history is not built yet"),
        _ => note(pix, "Wallpapers are not built yet"),
    }

    // the dots: where you are, and a click goes there
    let dots_y = cy + ch - 16.0;
    let total = SECTIONS.len() as f32 * 16.0;
    for i in 0..SECTIONS.len() {
        let dx = cx + cw / 2.0 - total / 2.0 + 8.0 + 16.0 * i as f32;
        paint::fill_circle(pix, dx * s, dots_y * s, if i == section { 3.5 } else { 2.5 } * s, if i == section { WHITE } else { FAINT });
        hits.push(FHit { rect: (dx - 8.0, dots_y - 9.0, 16.0, 18.0), act: FAct::Section(i) });
    }
    (card, hits)
}

#[cfg(test)]
mod tests {
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
        let j = r#"{"type":"a{sv}","data":{"xesam:title":{"type":"s","data":"Gimme Shelter"},"xesam:artist":{"type":"as","data":["The Rolling Stones","Merry Clayton"]}}}"#;
        let d = parse_busctl(j).unwrap();
        assert_eq!(parse_metadata(&d), ("Gimme Shelter".to_string(), "The Rolling Stones, Merry Clayton".to_string()));
        assert_eq!(parse_busctl(r#"{"type":"s","data":"Playing"}"#).unwrap().as_str(), Some("Playing"));
    }

    #[test]
    fn load_numbers() {
        // user nice system idle iowait irq softirq steal
        let (busy, total) = parse_cpu_stat("cpu  100 0 50 800 50 0 0 0 0 0\ncpu0 1 2 3 4 5\n").unwrap();
        assert_eq!((busy, total), (150, 1000));
        let mem = "MemTotal:       16384000 kB\nMemFree:  1 kB\nMemAvailable:    8192000 kB\n";
        assert_eq!(parse_meminfo(mem), Some((8000, 16000)));
    }
}
