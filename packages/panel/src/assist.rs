//! Snap Assist: after a window goes into a zone (zones.rs), the layout's other zones are offered to the other windows open on that
//! screen's desktop. The first empty zone shows those windows as tiles; a click (or Enter) puts that window there and the next empty
//! zone takes its turn, until every zone is filled or no window is left. A click anywhere else, or Escape, stops (what was placed
//! stays). One `hyprctl clients` when it opens; nothing runs while it waits.

use std::process::{Command, Stdio};

use resvg::tiny_skia::Pixmap;
use yggdrasil_conf::zones::{self, Layout, Monitor, Rect};

use crate::apps::{self, App};
use crate::paint::{self, Fonts, Icons};
use crate::zones::{Rect4, Session};

/// A window that can fill a zone.
#[derive(Clone, Debug, PartialEq)]
pub struct Win {
    pub address: String,
    pub class: String,
    pub title: String,
    pub floating: bool,
}

#[derive(Clone, Debug)]
pub struct Assist {
    pub monitor: Monitor,
    pub layout: Layout,
    pub gap: u32,
    /// The zones still empty, in the order they are offered.
    pub empty: Vec<usize>,
    pub wins: Vec<Win>,
    /// The tile chosen with the keyboard.
    pub sel: usize,
    /// How far the tiles are scrolled (rows).
    pub scroll: usize,
}

/// Hyprland's `clients -j`: the windows on desktop `workspace` of screen `monitor`, not hidden or full-screen, other than `placed`,
/// in the order it lists them (most recently focused first when it is asked with that order).
pub fn parse_clients(json: &str, monitor: i64, workspace: i64, placed: &str) -> Vec<Win> {
    let v: serde_json::Value = serde_json::from_str(json).unwrap_or_default();
    let Some(a) = v.as_array() else { return vec![] };
    let mut wins: Vec<(i64, Win)> = a
        .iter()
        .filter(|c| c["monitor"].as_i64() == Some(monitor) && c["workspace"]["id"].as_i64() == Some(workspace))
        .filter(|c| c["mapped"].as_bool().unwrap_or(true) && !c["hidden"].as_bool().unwrap_or(false))
        .filter(|c| c["fullscreen"].as_i64().unwrap_or(0) == 0 && !c["workspace"]["name"].as_str().unwrap_or("").starts_with("special:"))
        .filter_map(|c| {
            let address = c["address"].as_str()?.to_string();
            (address != placed).then(|| {
                let order = c["focusHistoryID"].as_i64().unwrap_or(i64::MAX);
                (order, Win { address, class: c["class"].as_str().unwrap_or("").to_string(), title: c["title"].as_str().unwrap_or("").to_string(), floating: c["floating"].as_bool().unwrap_or(true) })
            })
        })
        .collect();
    wins.sort_by_key(|(o, _)| *o);
    wins.into_iter().map(|(_, w)| w).collect()
}

fn hyprctl(args: &[&str]) -> Option<String> {
    let out = Command::new("hyprctl").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// After a window went into zone `hit` of the session's layouts: the other zones and windows, if there is anything to offer.
pub fn begin(s: &Session, (li, zi): (usize, usize)) -> Option<Assist> {
    let layout = s.layouts.get(li)?.clone();
    let empty: Vec<usize> = (0..layout.zones.len()).filter(|z| *z != zi).collect();
    if empty.is_empty() {
        return None;
    }
    let mons: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "monitors"])?).ok()?;
    let m = mons.as_array()?.iter().find(|m| m["name"].as_str() == Some(s.monitor.name.as_str()))?;
    let (mid, ws) = (m["id"].as_i64()?, m["activeWorkspace"]["id"].as_i64()?);
    let wins = parse_clients(&hyprctl(&["-j", "clients"])?, mid, ws, &s.address);
    if wins.is_empty() {
        return None;
    }
    Some(Assist { monitor: s.monitor.clone(), layout, gap: s.gap, empty, wins, sel: 0, scroll: 0 })
}

impl Assist {
    /// Where a zone is on the screen (global logical pixels).
    pub fn zone_rect(&self, z: usize) -> Option<Rect> {
        Some(zones::place(*self.layout.zones.get(z)?, self.monitor.free_area(), self.gap))
    }

    /// Window `w` goes into the zone whose turn it is: returns where (address, floating, rectangle) and whether anything is left to
    /// offer. The window leaves the choices; the next empty zone takes its turn.
    pub fn take(&mut self, w: usize) -> Option<((String, bool, Rect), bool)> {
        let z = *self.empty.first()?;
        let rect = self.zone_rect(z)?;
        let win = (w < self.wins.len()).then(|| self.wins.remove(w))?;
        self.empty.remove(0);
        self.sel = 0;
        self.scroll = 0;
        Some(((win.address, win.floating, rect), !self.empty.is_empty() && !self.wins.is_empty()))
    }

    /// Arrow keys and Tab: the next or previous tile (wrapping).
    pub fn step(&mut self, by: i32) {
        let n = self.wins.len() as i32;
        if n > 0 {
            self.sel = (self.sel as i32 + by).rem_euclid(n) as usize;
        }
    }
}

const TILE_W: f32 = 168.0;
const TILE_H: f32 = 128.0;
const GAP: f32 = 12.0;

/// The tiles' grid inside the zone whose turn it is: columns, rows that fit, and the grid's top-left (surface pixels).
fn grid(area: Rect4, n: usize) -> (usize, usize, f32, f32) {
    let inner_w = (area.2 - 2.0 * 24.0).max(TILE_W);
    let cols = (((inner_w + GAP) / (TILE_W + GAP)).floor() as usize).clamp(1, n.max(1));
    let rows_fit = (((area.3 - 72.0 + GAP) / (TILE_H + GAP)).floor() as usize).max(1);
    let gw = cols as f32 * TILE_W + (cols as f32 - 1.0) * GAP;
    (cols, rows_fit, area.0 + (area.2 - gw) / 2.0, area.1 + 56.0)
}

/// Draws the empty zones (the one whose turn it is with the windows as tiles) over a dimmed screen. `to_surface` maps global
/// pixels onto the surface. Returns each tile's rectangle and window index.
#[allow(clippy::too_many_arguments)]
pub fn paint(pix: &mut Pixmap, scale: f32, a: &Assist, origin: (f32, f32), apps_list: &[App], pointer: Option<(f32, f32)>, fonts: &Fonts, icons: &mut Icons) -> Vec<(Rect4, usize)> {
    pix.fill(paint::color((0, 0, 0, 0)));
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    paint::fill_rounded(pix, 0.0, 0.0, w, h, 0.0, (0, 0, 0, 110));
    let surf = |r: Rect| -> Rect4 { (r.0 as f32 - origin.0, r.1 as f32 - origin.1, r.2 as f32, r.3 as f32) };
    let mut hits = vec![];
    for (i, z) in a.empty.iter().enumerate() {
        let Some(r) = a.zone_rect(*z).map(surf) else { continue };
        if i > 0 {
            // a later turn: just where it is
            paint::fill_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 14.0 * scale, (255, 255, 255, 14));
            paint::stroke_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 14.0 * scale, 1.0 * scale, (255, 255, 255, 50));
            continue;
        }
        paint::fill_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 14.0 * scale, (24, 24, 30, 200));
        paint::stroke_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 14.0 * scale, 2.0 * scale, paint::accent(220));
        let head = "Choose a window for this space";
        paint::draw_text(pix, &fonts.bold, 14.0 * scale, (r.0 + 24.0) * scale, (r.1 + 26.0) * scale, (240, 240, 245, 255), head);
        let hint = "Click elsewhere or Esc to stop";
        let hw = paint::text_width(&fonts.regular, 11.5 * scale, hint) / scale;
        paint::draw_text(pix, &fonts.regular, 11.5 * scale, (r.0 + r.2 - 24.0 - hw) * scale, (r.1 + 26.0) * scale, (160, 160, 175, 255), hint);
        let (cols, rows_fit, gx, gy) = grid(r, a.wins.len());
        let first = a.scroll * cols;
        for (k, win) in a.wins.iter().enumerate().skip(first).take(cols * rows_fit) {
            let slot = k - first;
            let t = (gx + (slot % cols) as f32 * (TILE_W + GAP), gy + (slot / cols) as f32 * (TILE_H + GAP), TILE_W, TILE_H);
            let over = pointer.is_some_and(|(px, py)| px >= t.0 && px < t.0 + t.2 && py >= t.1 && py < t.1 + t.3);
            let fill = if over || k == a.sel { (60, 56, 76, 245) } else { (36, 36, 44, 240) };
            paint::fill_rounded(pix, t.0 * scale, t.1 * scale, t.2 * scale, t.3 * scale, 10.0 * scale, fill);
            if k == a.sel {
                paint::stroke_rounded(pix, t.0 * scale, t.1 * scale, t.2 * scale, t.3 * scale, 10.0 * scale, 2.0 * scale, paint::accent(255));
            }
            let app = apps::for_app_id(apps_list, &win.class);
            let (name, icon) = app.map(|a| (a.name.clone(), a.icon.clone())).unwrap_or_else(|| (win.class.clone(), win.class.to_lowercase()));
            crate::notify::draw_app_icon(pix, scale, icons, fonts, &name, &icon, t.0 + (TILE_W - 52.0) / 2.0, t.1 + 18.0, 52.0, false);
            let label = if win.title.is_empty() { name.clone() } else { win.title.clone() };
            let line = crate::fylgja::fit(&fonts.regular, 12.0 * scale, &label, (TILE_W - 20.0) * scale);
            let lw = paint::text_width(&fonts.regular, 12.0 * scale, &line) / scale;
            paint::draw_text(pix, &fonts.regular, 12.0 * scale, (t.0 + (TILE_W - lw) / 2.0) * scale, (t.1 + 96.0) * scale, (235, 235, 242, 255), &line);
            hits.push((t, k));
        }
        let shown = cols * rows_fit;
        if a.wins.len() > first + shown || first > 0 {
            let more = format!("{} of {} · scroll for more", (a.wins.len() - first).min(shown), a.wins.len());
            paint::draw_text(pix, &fonts.regular, 11.0 * scale, (r.0 + 24.0) * scale, (r.1 + r.3 - 16.0) * scale, (160, 160, 175, 255), &more);
        }
    }
    hits
}

/// Scrolls the tiles a row at a time, within what there is.
pub fn scroll(a: &mut Assist, by: i32, area: Rect4) {
    let (cols, rows_fit, _, _) = grid(area, a.wins.len());
    let rows = a.wins.len().div_ceil(cols);
    let max = rows.saturating_sub(rows_fit) as i32;
    a.scroll = (a.scroll as i32 + by).clamp(0, max) as usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assist(n: usize) -> Assist {
        let monitor = Monitor { name: "DP-1".into(), x: 0, y: 0, width: 3440, height: 1440, scale: 1.0, transform: 0, reserved: [0, 28, 0, 70] };
        let layout = zones::builtin().into_iter().find(|l| l.zones.len() == 3).unwrap();
        let wins = (0..n).map(|i| Win { address: format!("0x{i}"), class: "foot".into(), title: format!("w{i}"), floating: i % 2 == 0 }).collect();
        Assist { monitor, layout, gap: 8, empty: vec![1, 2], wins, sel: 0, scroll: 0 }
    }

    #[test]
    fn the_clients_on_this_desktop_are_offered() {
        let j = r#"[
            {"address":"0x1","class":"vivaldi","title":"Mail","monitor":0,"workspace":{"id":1,"name":"1"},"floating":true,"fullscreen":0,"mapped":true,"hidden":false,"focusHistoryID":2},
            {"address":"0x2","class":"foot","title":"Shell","monitor":0,"workspace":{"id":1,"name":"1"},"floating":false,"fullscreen":0,"mapped":true,"hidden":false,"focusHistoryID":1},
            {"address":"0x3","class":"x","title":"placed","monitor":0,"workspace":{"id":1,"name":"1"},"floating":true,"fullscreen":0,"mapped":true,"hidden":false,"focusHistoryID":0},
            {"address":"0x4","class":"y","title":"other desk","monitor":0,"workspace":{"id":2,"name":"2"},"floating":true,"fullscreen":0,"mapped":true,"hidden":false,"focusHistoryID":3},
            {"address":"0x5","class":"z","title":"min","monitor":0,"workspace":{"id":1,"name":"1"},"floating":true,"fullscreen":0,"mapped":true,"hidden":true,"focusHistoryID":4},
            {"address":"0x6","class":"z","title":"full","monitor":0,"workspace":{"id":1,"name":"1"},"floating":true,"fullscreen":2,"mapped":true,"hidden":false,"focusHistoryID":5},
            {"address":"0x7","class":"z","title":"other screen","monitor":1,"workspace":{"id":1,"name":"1"},"floating":true,"fullscreen":0,"mapped":true,"hidden":false,"focusHistoryID":6}]"#;
        let w = parse_clients(j, 0, 1, "0x3");
        assert_eq!(w.iter().map(|w| w.address.as_str()).collect::<Vec<_>>(), vec!["0x2", "0x1"], "most recently used first");
        assert!(!w[0].floating);
    }

    #[test]
    fn zones_fill_in_turn_until_none_or_no_window_is_left() {
        let mut a = assist(3);
        let first = a.zone_rect(1).unwrap();
        let ((addr, floating, rect), more) = a.take(1).unwrap();
        assert_eq!((addr.as_str(), floating, rect, more), ("0x1", false, first, true));
        assert_eq!(a.empty, vec![2]);
        assert_eq!(a.wins.len(), 2);
        let (_, more) = a.take(0).unwrap();
        assert!(!more, "every zone is filled");
        assert_eq!(a.take(0), None);

        let mut a = assist(1);
        let (_, more) = a.take(0).unwrap();
        assert!(!more, "no window left for the third zone");
        assert_eq!(assist(2).take(5), None, "a tile that is not there");
    }

    #[test]
    fn keys_and_scrolling_stay_in_range() {
        let mut a = assist(3);
        a.step(-1);
        assert_eq!(a.sel, 2);
        a.step(1);
        assert_eq!(a.sel, 0);
        let mut a = assist(40);
        let area = (0.0, 0.0, 600.0, 400.0);
        scroll(&mut a, 100, area);
        let (cols, rows_fit, _, _) = grid(area, 40);
        assert_eq!(a.scroll, 40usize.div_ceil(cols) - rows_fit);
        scroll(&mut a, -100, area);
        assert_eq!(a.scroll, 0);
    }

    /// On a live Hyprland session (`cargo test -- --ignored live`): opens a foot window of its own, lists this desktop's windows as
    /// Snap Assist does, puts the foot window into a zone and reads back where Hyprland put it, then closes it.
    #[test]
    #[ignore]
    fn live_assist_lists_and_places() {
        let mut foot = std::process::Command::new("foot").args(["-a", "assist-test", "sleep", "30"]).spawn().expect("foot");
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let mons: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "monitors"]).unwrap()).unwrap();
        let m = &mons[0];
        let (mid, ws) = (m["id"].as_i64().unwrap(), m["activeWorkspace"]["id"].as_i64().unwrap());
        let wins = parse_clients(&hyprctl(&["-j", "clients"]).unwrap(), mid, ws, "none");
        let w = wins.iter().find(|w| w.class == "assist-test").expect("the test window is offered").clone();
        let mut a = assist(0);
        a.monitor = crate::zones::parse_monitors(&hyprctl(&["-j", "monitors"]).unwrap()).remove(0);
        a.wins = vec![w.clone()];
        let ((addr, floating, rect), more) = a.take(0).unwrap();
        crate::zones::place(&addr, floating, rect);
        std::thread::sleep(std::time::Duration::from_millis(600));
        let c: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "clients"]).unwrap()).unwrap();
        let got = c.as_array().unwrap().iter().find(|c| c["address"].as_str() == Some(addr.as_str())).unwrap().clone();
        let _ = foot.kill();
        let at = (got["at"][0].as_i64().unwrap() as i32, got["at"][1].as_i64().unwrap() as i32);
        let size = (got["size"][0].as_i64().unwrap() as i32, got["size"][1].as_i64().unwrap() as i32);
        eprintln!("zone {rect:?} -> at {at:?} size {size:?}, more {more}");
        assert_eq!((at.0, at.1, size.0, size.1), rect);
    }
}
