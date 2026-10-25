//! Window zones (FancyZones-style): drag a window to the notch and a sheet of layouts opens under it; drop the window on a zone
//! and it fills that part of the screen. Super+Shift+Z opens the same sheet for the window in front (then click a zone).
//!
//! Hyprland gives no event while a window is dragged, and the panel gets no pointer events then (the compositor holds the
//! pointer). So the session's Lua watches the left button (a non-consuming press binding arms a timer inside the compositor) and
//! calls `yggdrasil-panelctl zones-drag` when a dragged window's pointer reaches the notch. Between then and the drop, and only
//! then, the panel asks Hyprland where the pointer and the window are (its socket, no process per question) to light the zone
//! under it. Layouts and the gap come from `zones.conf` (yggdrasil_conf::zones).
//!
//! The drop: Hyprland's release binding does not fire at the end of a window move (tried 2026-10-25), so the end of the drag is
//! read from several signs, whichever comes first, and acted on once (`DropWatch`): the window stops following the pointer (the
//! grip, the pointer's offset from the window's corner, changes), the pointer comes back to the sheet (an enter after the sheet
//! settled, or a click), or the session's Lua sees the same and calls `zones-drop`.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::process::{Command, Stdio};

use resvg::tiny_skia::Pixmap;
use yggdrasil_conf::zones::{self, Layout, Monitor, Rect, ZonesConf};

use crate::paint::{self, Fonts};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A window is being dragged: the pointer is polled, the drop places it.
    Drag,
    /// Opened from the keyboard for the window in front: a click places it.
    Pick,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    /// The window to place (Hyprland's address).
    pub address: String,
    pub floating: bool,
    pub monitor: Monitor,
    pub layouts: Vec<Layout>,
    pub gap: u32,
    /// The zone under the pointer: layout and zone index.
    pub hover: Option<(usize, usize)>,
    /// Where the pointer is on the sheet's surface (logical pixels), while dragging.
    pub pointer: Option<(f32, f32)>,
    /// When the pointer last moved: a drag sheet that hears no drop (the release went unseen) closes after a while.
    pub moved: std::time::Instant,
    /// Watches for the end of the drag.
    pub watch: DropWatch,
}

/// How the end of a drag shows. Every sign ends it once; later ones are ignored.
///
/// While the button is held the window follows the pointer, moving whenever it does (a frame behind at most, which on a fast drag
/// is more than a few pixels, so the window's offset from the pointer is no test). Let go, and the window stays where it is while the
/// pointer moves on: the drop is the pointer moving more than `SLACK` since the window last moved.
#[derive(Clone, Debug)]
pub struct DropWatch {
    /// The pointer's offset from the dragged window's top-left corner when the sheet opened (the window was being dragged).
    pub grip: Option<(f64, f64)>,
    /// Where the window was last seen, and where the pointer was when it last moved.
    pub last_at: Option<(f64, f64)>,
    pub anchor: Option<(f64, f64)>,
    pub opened: std::time::Instant,
    /// The zone under the pointer the last time the window was seen following it (where it was let go).
    pub held: Option<(usize, usize)>,
    pub done: bool,
}

/// How far (logical pixels) the pointer may move away from its grip before the window is taken as let go.
pub const SLACK: f64 = 6.0;
/// Pointer focus coming back to the sheet means the compositor let go of the pointer; right after the sheet opens it can be the
/// sheet appearing under the pointer instead, so not before this.
pub const SETTLE: std::time::Duration = std::time::Duration::from_millis(250);

impl DropWatch {
    pub fn new(grip: Option<(f64, f64)>) -> DropWatch {
        DropWatch { grip, last_at: None, anchor: None, opened: std::time::Instant::now(), held: None, done: false }
    }

    /// The pointer and the window, read while following: Some(zone or none) once the window no longer follows the pointer (the
    /// drop, at the zone held last); otherwise remembers `hover` as where it is held.
    pub fn follow(&mut self, cursor: (f64, f64), window_at: Option<(f64, f64)>, hover: Option<(usize, usize)>) -> Option<Option<(usize, usize)>> {
        if self.done {
            return None;
        }
        let Some(at) = window_at.filter(|_| self.grip.is_some()) else {
            // the window cannot be judged: just remember where the pointer is
            self.held = hover;
            return None;
        };
        if self.last_at != Some(at) {
            // the window moved with the pointer: still held, over this zone
            self.last_at = Some(at);
            self.anchor = Some(cursor);
            self.held = hover;
            return None;
        }
        let (ax, ay) = self.anchor.unwrap_or(cursor);
        if (cursor.0 - ax).abs() + (cursor.1 - ay).abs() > SLACK {
            self.done = true;
            return Some(self.held);
        }
        None
    }

    /// The pointer entered the sheet (or clicked it) at a zone or not: the drop, unless the sheet has only just appeared.
    pub fn pointer_back(&mut self, clicked: bool, at: Option<(usize, usize)>) -> Option<Option<(usize, usize)>> {
        if self.done || (!clicked && self.opened.elapsed() < SETTLE) {
            return None;
        }
        self.done = true;
        Some(at.or(self.held))
    }

    /// The session said the drag ended.
    pub fn told(&mut self) -> Option<Option<(usize, usize)>> {
        if self.done {
            return None;
        }
        self.done = true;
        Some(self.held)
    }
}

/// How long a drag sheet waits, with the pointer still, for a drop it may never hear.
pub const STALE: std::time::Duration = std::time::Duration::from_secs(8);

/// The left mouse button as the kernel sees it, from the pointer devices this user may read (Hyprland reports no mouse buttons, and
/// no button event reaches the panel while the compositor moves a window). Only the button state is asked for; no events are read.
pub struct Buttons {
    devices: Vec<std::fs::File>,
}

const BTN_LEFT: usize = 0x110;
const KEY_A: usize = 30;

fn ioc_read(nr: u64, len: usize) -> u64 {
    (2u64 << 30) | ((len as u64) << 16) | ((b'E' as u64) << 8) | nr
}

fn bit(buf: &[u8], n: usize) -> bool {
    buf.get(n / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

impl Buttons {
    /// The readable devices that have a left button and are not keyboards (a mouse's macro-key half shows up as a keyboard).
    pub fn open() -> Buttons {
        use std::os::fd::AsRawFd;
        let mut devices = vec![];
        let Ok(rd) = std::fs::read_dir("/dev/input") else { return Buttons { devices } };
        for e in rd.flatten() {
            if !e.file_name().to_string_lossy().starts_with("event") {
                continue;
            }
            let Ok(f) = std::fs::File::open(e.path()) else { continue };
            let mut caps = [0u8; 96];
            // EVIOCGBIT(EV_KEY): which keys and buttons the device has
            let n = unsafe { libc::ioctl(f.as_raw_fd(), ioc_read(0x20 + 1, caps.len()) as _, caps.as_mut_ptr()) };
            if n > 0 && bit(&caps, BTN_LEFT) && !bit(&caps, KEY_A) {
                devices.push(f);
            }
        }
        Buttons { devices }
    }

    /// Whether the left button is down on any of them; None when no pointer device can be read.
    pub fn left_down(&self) -> Option<bool> {
        use std::os::fd::AsRawFd;
        if self.devices.is_empty() {
            return None;
        }
        Some(self.devices.iter().any(|f| {
            let mut state = [0u8; 96];
            // EVIOCGKEY: which keys and buttons are held now
            let n = unsafe { libc::ioctl(f.as_raw_fd(), ioc_read(0x18, state.len()) as _, state.as_mut_ptr()) };
            n >= 0 && bit(&state, BTN_LEFT)
        }))
    }
}

/// Notices a window being dragged to the notch, with no binding on the mouse button (a binding on the left button kept Hyprland
/// 0.56 from ending its own window move when the button came up). Each tick asks the kernel whether the left button is held (one
/// ioctl per mouse); only while it is held does it ask Hyprland where the window in front is and where the pointer is.
pub struct Spotter {
    buttons: Buttons,
    opened: std::time::Instant,
    start: Option<(String, (f64, f64))>,
    told: bool,
}

impl Default for Spotter {
    fn default() -> Self {
        Spotter { buttons: Buttons::open(), opened: std::time::Instant::now(), start: None, told: false }
    }
}

impl Spotter {
    /// Call every tick. True once per drag, when a floating window being dragged has reached the notch of its screen.
    pub fn tick(&mut self) -> bool {
        // mice come and go: look again now and then while no button is held
        if self.start.is_none() && self.opened.elapsed() > std::time::Duration::from_secs(10) {
            *self = Spotter::default();
        }
        if self.buttons.left_down() != Some(true) {
            self.start = None;
            self.told = false;
            return false;
        }
        let Some(raw) = ask("j/activewindow") else { return false };
        let Some((address, at)) = parse_window(&raw) else { return false };
        let floating = serde_json::from_str::<serde_json::Value>(&raw).ok().and_then(|v| v["floating"].as_bool()).unwrap_or(false);
        match &self.start {
            Some((a, from)) if *a == address => {
                if self.told || !floating || (at.0 - from.0).abs() + (at.1 - from.1).abs() <= 6.0 {
                    return false;
                }
                let Some(c) = cursor() else { return false };
                let monitors = ask("j/monitors").map(|j| parse_monitors(&j)).unwrap_or_default();
                if near_notch(&monitors, c) {
                    self.told = true;
                    return true;
                }
                false
            }
            _ => {
                // the press: the window it went to, and where it was
                self.start = Some((address, at));
                self.told = false;
                false
            }
        }
    }
}

/// The pointer is at the notch: within 180 px of the middle of its screen, no lower than 30 px under the bar.
fn near_notch(monitors: &[Monitor], (cx, cy): (f64, f64)) -> bool {
    monitors.iter().any(|m| {
        let (x, y, w, h) = m.logical();
        let inside = cx >= x as f64 && cx < (x + w) as f64 && cy >= y as f64 && cy < (y + h) as f64;
        inside && (cx - (x as f64 + w as f64 / 2.0)).abs() < 180.0 && cy < (y + m.reserved[1]) as f64 + 30.0
    })
}

/// A line in the drag trail (~/.local/state/yggdrasil/zones.log, shared with the session's Lua), to find why a drop is missed.
pub fn note(msg: &str) {
    use std::io::Write;
    let Some(dir) = std::env::var_os("XDG_STATE_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state"))) else { return };
    let path = dir.join("yggdrasil/zones.log");
    let big = std::fs::metadata(&path).map(|m| m.len() > 200_000).unwrap_or(false);
    let f = if big { std::fs::File::create(&path) } else { std::fs::OpenOptions::new().create(true).append(true).open(&path) };
    if let Ok(mut f) = f {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "{} panel: {msg}", t % 100_000_000);
    }
}

fn hyprctl(args: &[&str]) -> Option<String> {
    let out = Command::new("hyprctl").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Hyprland's `monitors -j` list.
pub fn parse_monitors(json: &str) -> Vec<Monitor> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let Some(a) = v.as_array() else { return vec![] };
    a.iter()
        .filter_map(|m| {
            let r = m["reserved"].as_array();
            let rv = |i: usize| r.and_then(|r| r.get(i)).and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            Some(Monitor {
                name: m["name"].as_str()?.to_string(),
                x: m["x"].as_i64()? as i32,
                y: m["y"].as_i64()? as i32,
                width: m["width"].as_u64()? as u32,
                height: m["height"].as_u64()? as u32,
                scale: m["scale"].as_f64().unwrap_or(1.0) as f32,
                transform: m["transform"].as_u64().unwrap_or(0) as u32,
                reserved: [rv(0), rv(1), rv(2), rv(3)],
            })
        })
        .collect()
}

/// Where the pointer is in the global space, from Hyprland's socket (cheap enough to ask every few tens of milliseconds while a
/// window is dragged; nothing asks otherwise).
pub fn cursor() -> Option<(f64, f64)> {
    parse_cursor(&ask("j/cursorpos")?)
}

/// The window in front: its address and top-left corner (global logical pixels).
pub fn front_window() -> Option<(String, (f64, f64))> {
    parse_window(&ask("j/activewindow")?)
}

/// One question to Hyprland's socket.
fn ask(q: &str) -> Option<String> {
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let run = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let mut s = UnixStream::connect(format!("{run}/hypr/{sig}/.socket.sock")).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_millis(200))).ok()?;
    s.write_all(q.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

fn parse_window(json: &str) -> Option<(String, (f64, f64))> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let at = v["at"].as_array()?;
    Some((v["address"].as_str()?.to_string(), (at.first()?.as_f64()?, at.get(1)?.as_f64()?)))
}

fn parse_cursor(json: &str) -> Option<(f64, f64)> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    Some((v["x"].as_f64()?, v["y"].as_f64()?))
}

/// Starts placing the window in front: on the screen under the pointer when dragging, the window's own screen from the keyboard.
pub fn begin(mode: Mode) -> Option<Session> {
    let conf = ZonesConf::load();
    if !conf.enabled || conf.layouts.is_empty() {
        return None;
    }
    let win: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "activewindow"])?).ok()?;
    let address = win["address"].as_str().filter(|a| a.starts_with("0x"))?.to_string();
    if win["fullscreen"].as_i64().unwrap_or(0) != 0 {
        return None;
    }
    let monitors = parse_monitors(&hyprctl(&["-j", "monitors"])?);
    let monitor = match mode {
        Mode::Drag => cursor().and_then(|(x, y)| monitors.iter().find(|m| m.contains(x, y)).cloned()),
        Mode::Pick => win["monitor"].as_i64().and_then(|id| {
            let names: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "monitors"])?).ok()?;
            let name = names.as_array()?.iter().find(|m| m["id"].as_i64() == Some(id))?["name"].as_str()?.to_string();
            monitors.iter().find(|m| m.name == name).cloned()
        }),
    }
    .or_else(|| monitors.first().cloned())?;
    // where the pointer holds the window: while it does, the window follows the pointer
    let at = win["at"].as_array().and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
    let grip = match (mode, cursor(), at) {
        (Mode::Drag, Some((cx, cy)), Some((wx, wy))) => Some((cx - wx, cy - wy)),
        _ => None,
    };
    Some(Session { mode, address, floating: win["floating"].as_bool().unwrap_or(true), monitor, layouts: conf.shown(), gap: conf.gap, hover: None, pointer: None, moved: std::time::Instant::now(), watch: DropWatch::new(grip) })
}

/// Where the zone goes on the screen, in global logical pixels.
pub fn target(s: &Session, (li, zi): (usize, usize)) -> Option<Rect> {
    let z = *s.layouts.get(li)?.zones.get(zi)?;
    Some(zones::place(z, s.monitor.free_area(), s.gap))
}

/// Puts the window there: floating first (a tiled window cannot be placed), then its size, then its place (a resize keeps a
/// window centred, so the other order would undo the move).
pub fn apply(s: &Session, hit: (usize, usize)) {
    let Some(r) = target(s, hit) else { return };
    place(&s.address, s.floating, r);
}

/// Puts a window (Hyprland's address) at a rectangle (global logical pixels).
pub fn place(address: &str, floating: bool, (x, y, w, h): Rect) {
    let win = format!("address:{address}");
    let mut cmds = vec![];
    if !floating {
        cmds.push(format!(r#"hl.dsp.window.float({{ action = "enable", window = "{win}" }})"#));
    }
    cmds.push(format!(r#"hl.dsp.window.resize({{ x = {w}, y = {h}, window = "{win}" }})"#));
    cmds.push(format!(r#"hl.dsp.window.move({{ x = {x}, y = {y}, window = "{win}" }})"#));
    for c in cmds {
        let answer = hyprctl(&["dispatch", &c]);
        note(&format!("{c} -> {answer:?}"));
    }
}

/// The sheet's surface starts under the bar on the left edge of the screen: global pointer to surface coordinates.
pub fn to_surface(s: &Session, (gx, gy): (f64, f64)) -> (f32, f32) {
    let (mx, my, _, _) = s.monitor.logical();
    ((gx - mx as f64) as f32, (gy - my as f64 - s.monitor.reserved[1] as f64) as f32)
}

const THUMB_W: f32 = 132.0;
const PAD: f32 = 14.0;

/// The sheet under the notch: a card with one small picture per layout, its zones drawn in it. Returns each zone's rectangle on
/// the surface (logical pixels) with its layout and zone index. Also lights, at its real size and place, the zone under the
/// pointer.
pub fn paint(pix: &mut Pixmap, scale: f32, s: &Session, fonts: &Fonts, dark: bool) -> (Rect4, Vec<(Rect4, (usize, usize))>) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let w = pix.width() as f32 / scale;
    let (_, _, sw, sh) = s.monitor.free_area();
    let aspect = if sw > 0 { sh as f32 / sw as f32 } else { 0.5625 };
    let th = (THUMB_W * aspect).clamp(48.0, 110.0);
    let n = s.layouts.len().max(1) as f32;
    let cw = n * THUMB_W + (n - 1.0) * 10.0 + 2.0 * PAD;
    let ch = PAD + 20.0 + th + 22.0 + PAD;
    let (cx, cy) = ((w - cw) / 2.0, 8.0);
    let (bg, text, dim, tile, zone, edge) = if dark {
        ((24, 24, 30, 240), (240, 240, 245, 255), (150, 150, 165, 255), (255, 255, 255, 14), (255, 255, 255, 34), (255, 255, 255, 60))
    } else {
        ((250, 250, 252, 242), (20, 20, 28, 255), (100, 100, 115, 255), (0, 0, 0, 10), (0, 0, 0, 26), (0, 0, 0, 50))
    };
    // the zone under the pointer, where the window will go
    if let Some(hit) = s.hover {
        if let Some((gx, gy, gw, gh)) = target(s, hit) {
            let (lx, ly) = to_surface(s, (gx as f64, gy as f64));
            paint::fill_rounded(pix, lx * scale, ly * scale, gw as f32 * scale, gh as f32 * scale, 12.0 * scale, paint::accent(70));
            paint::stroke_rounded(pix, lx * scale, ly * scale, gw as f32 * scale, gh as f32 * scale, 12.0 * scale, 2.0 * scale, paint::accent(220));
        }
    }
    paint::fill_rounded(pix, cx * scale, cy * scale, cw * scale, ch * scale, 16.0 * scale, bg);
    let title = if s.mode == Mode::Drag { "Drop on a zone" } else { "Pick a zone (Esc to cancel)" };
    paint::draw_text(pix, &fonts.bold, 12.5 * scale, (cx + PAD) * scale, (cy + PAD + 7.0) * scale, text, title);
    let mut hits = vec![];
    for (li, l) in s.layouts.iter().enumerate() {
        let tx = cx + PAD + li as f32 * (THUMB_W + 10.0);
        let ty = cy + PAD + 20.0;
        paint::fill_rounded(pix, tx * scale, ty * scale, THUMB_W * scale, th * scale, 8.0 * scale, tile);
        for (zi, z) in l.zones.iter().enumerate() {
            let g = 3.0;
            let r = (tx + z.0 * THUMB_W + g, ty + z.1 * th + g, z.2 * THUMB_W - 2.0 * g, z.3 * th - 2.0 * g);
            let on = s.hover == Some((li, zi));
            paint::fill_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 5.0 * scale, if on { paint::accent(255) } else { zone });
            if !on {
                paint::stroke_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 5.0 * scale, 1.0 * scale, edge);
            }
            hits.push((r, (li, zi)));
        }
        let nw = paint::text_width(&fonts.regular, 11.0 * scale, l.name) / scale;
        paint::draw_text(pix, &fonts.regular, 11.0 * scale, (tx + (THUMB_W - nw) / 2.0) * scale, (ty + th + 12.0) * scale, dim, l.name);
    }
    ((cx, cy, cw, ch), hits)
}

/// A rectangle on the surface in logical pixels (x, y, width, height).
pub type Rect4 = (f32, f32, f32, f32);

/// The zone under a point of the surface.
pub fn hit_at(hits: &[(Rect4, (usize, usize))], (px, py): (f32, f32)) -> Option<(usize, usize)> {
    hits.iter().find(|(r, _)| px >= r.0 && px < r.0 + r.2 && py >= r.1 && py < r.1 + r.3).map(|(_, h)| *h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> Session {
        let monitor = Monitor { name: "DP-1".into(), x: 0, y: 0, width: 3440, height: 1440, scale: 1.0, transform: 0, reserved: [0, 28, 0, 70] };
        Session { mode: Mode::Drag, address: "0xabc".into(), floating: true, monitor, layouts: zones::builtin(), gap: 8, hover: None, pointer: None, moved: std::time::Instant::now(), watch: DropWatch::new(Some((40.0, 10.0))) }
    }

    #[test]
    fn the_drop_is_seen_once_whichever_sign_comes_first() {
        // the window follows the pointer: no drop, and the zone under it is remembered
        let mut w = DropWatch::new(Some((40.0, 10.0)));
        assert_eq!(w.follow((140.0, 30.0), Some((100.0, 20.0)), Some((0, 1))), None);
        assert_eq!(w.follow((300.0, 31.0), Some((260.0, 21.0)), Some((0, 0))), None);
        // a fast drag: the window a frame behind the pointer by far more than the slack, but it moved: still held
        assert_eq!(w.follow((360.0, 31.0), Some((290.0, 21.0)), Some((0, 0))), None);
        // the pointer still for a moment, the window caught up: still held
        assert_eq!(w.follow((360.0, 31.0), Some((320.0, 21.0)), Some((0, 0))), None);
        assert_eq!(w.follow((362.0, 31.0), Some((320.0, 21.0)), Some((0, 0))), None, "within the slack");
        // let go: the pointer moves on, the window stays; the drop is at the zone it was held over, not where the pointer went
        assert_eq!(w.follow((390.0, 40.0), Some((320.0, 21.0)), Some((1, 0))), Some(Some((0, 0))));
        // every later sign is ignored
        assert_eq!(w.follow((400.0, 40.0), Some((260.0, 21.0)), None), None);
        assert_eq!(w.pointer_back(true, Some((2, 0))), None);
        assert_eq!(w.told(), None);

        // the pointer coming back: an enter right as the sheet opens is the sheet appearing, not a drop; a click always is
        let mut w = DropWatch::new(Some((40.0, 10.0)));
        assert_eq!(w.pointer_back(false, Some((1, 1))), None);
        assert_eq!(w.pointer_back(true, Some((1, 1))), Some(Some((1, 1))));
        let mut w = DropWatch::new(None);
        w.opened -= SETTLE;
        assert_eq!(w.follow((10.0, 10.0), None, Some((3, 0))), None, "without a grip the window is not judged");
        assert_eq!(w.pointer_back(false, None), Some(Some((3, 0))), "an enter later is the drop, at the zone held last");

        // the session's word
        let mut w = DropWatch::new(None);
        w.held = Some((0, 2));
        assert_eq!(w.told(), Some(Some((0, 2))));
        assert_eq!(w.told(), None);
    }

    #[test]
    fn the_window_in_front_is_read() {
        assert_eq!(parse_window(r#"{"address":"0x55","at":[120,48],"size":[800,600]}"#), Some(("0x55".into(), (120.0, 48.0))));
        assert_eq!(parse_window("{}"), None);
    }

    #[test]
    fn hyprland_answers_are_read() {
        let j = r#"[{"id":0,"name":"DP-1","x":0,"y":0,"width":3440,"height":1440,"scale":1.00,"transform":0,"reserved":[0,28,0,70]}]"#;
        let m = parse_monitors(j);
        assert_eq!(m[0].free_area(), (0, 28, 3440, 1342));
        assert_eq!(parse_cursor(r#"{"x": 1720.5, "y": 12}"#), Some((1720.5, 12.0)));
        assert_eq!(parse_cursor("garbage"), None);
    }

    #[test]
    fn the_target_leaves_the_bar_and_dock_free() {
        let s = session();
        let (x, y, w, h) = target(&s, (0, 0)).unwrap(); // the left half
        assert_eq!((x, y), (8, 36));
        assert_eq!(y + h, 1440 - 70 - 8);
        assert_eq!(x + w, 1720 - 4);
        assert_eq!(target(&s, (0, 9)), None);
    }

    #[test]
    fn the_pointer_is_mapped_onto_the_sheet_and_its_zones() {
        let mut s = session();
        // a second screen to the right: its surface starts at its own left edge, under its bar
        s.monitor.x = 3440;
        assert_eq!(to_surface(&s, (3440.0 + 100.0, 28.0 + 50.0)), (100.0, 50.0));
        let hits = vec![((10.0, 10.0, 20.0, 20.0), (1, 2))];
        assert_eq!(hit_at(&hits, (15.0, 15.0)), Some((1, 2)));
        assert_eq!(hit_at(&hits, (35.0, 15.0)), None);
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;

    #[test]
    fn ioctl_numbers_and_bits() {
        // EVIOCGKEY(96) and EVIOCGBIT(EV_KEY, 96) as the kernel headers spell them
        assert_eq!(ioc_read(0x18, 96), 0x80604518);
        assert_eq!(ioc_read(0x21, 96), 0x80604521);
        let mut b = [0u8; 96];
        b[BTN_LEFT / 8] = 1 << (BTN_LEFT % 8);
        assert!(bit(&b, BTN_LEFT) && !bit(&b, KEY_A));
    }
}
