//! Window zones (FancyZones-style): drag a window to the notch and a sheet of layouts opens under it; drop the window on a zone
//! and it fills that part of the screen. Super+Shift+Z opens the same sheet for the window in front (then click a zone).
//!
//! Hyprland gives no event while a window is dragged, and the panel gets no pointer events then (the compositor holds the
//! pointer). So the session's Lua watches the left button (a non-consuming press binding arms a timer inside the compositor, a
//! release binding disarms it), calls `yggdrasil-panelctl zones-drag` when a dragged window's pointer reaches the notch and
//! `zones-drop` when the button comes up. Between the two, and only then, the panel asks Hyprland where the pointer is (its
//! socket, no process per question) to light the zone under it. Layouts and the gap come from `zones.conf`
//! (yggdrasil_conf::zones).

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
}

/// How long a drag sheet waits, with the pointer still, for a drop it may never hear.
pub const STALE: std::time::Duration = std::time::Duration::from_secs(8);

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
    let sig = std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok()?;
    let run = std::env::var("XDG_RUNTIME_DIR").ok()?;
    let mut s = UnixStream::connect(format!("{run}/hypr/{sig}/.socket.sock")).ok()?;
    s.set_read_timeout(Some(std::time::Duration::from_millis(200))).ok()?;
    s.write_all(b"j/cursorpos").ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    parse_cursor(&out)
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
    Some(Session { mode, address, floating: win["floating"].as_bool().unwrap_or(true), monitor, layouts: conf.shown(), gap: conf.gap, hover: None, pointer: None, moved: std::time::Instant::now() })
}

/// Where the zone goes on the screen, in global logical pixels.
pub fn target(s: &Session, (li, zi): (usize, usize)) -> Option<Rect> {
    let z = *s.layouts.get(li)?.zones.get(zi)?;
    Some(zones::place(z, s.monitor.free_area(), s.gap))
}

/// Puts the window there: floating first (a tiled window cannot be placed), then its size, then its place (a resize keeps a
/// window centred, so the other order would undo the move).
pub fn apply(s: &Session, hit: (usize, usize)) {
    let Some((x, y, w, h)) = target(s, hit) else { return };
    let win = format!("address:{}", s.address);
    let mut cmds = vec![];
    if !s.floating {
        cmds.push(format!(r#"hl.dsp.window.float({{ action = "enable", window = "{win}" }})"#));
    }
    cmds.push(format!(r#"hl.dsp.window.resize({{ x = {w}, y = {h}, window = "{win}" }})"#));
    cmds.push(format!(r#"hl.dsp.window.move({{ x = {x}, y = {y}, window = "{win}" }})"#));
    for c in cmds {
        let _ = hyprctl(&["dispatch", &c]);
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
        Session { mode: Mode::Drag, address: "0xabc".into(), floating: true, monitor, layouts: zones::builtin(), gap: 8, hover: None, pointer: None, moved: std::time::Instant::now() }
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
