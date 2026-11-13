//! Settings > Displays worked out apart from the drawing (displays_ui.rs): the numbers the screens go by, where a screen dragged
//! in the arrangement box lands (edge to edge with the others, never apart, never on top of one), what Extend, Duplicate and
//! Disconnect do to the list, the guard that keeps one screen connected, and the 15 seconds of "Keep these display settings?".
//!
//! Places and sizes are in the space of all screens (logical pixels, what Hyprland's `position` takes): a screen's size is its
//! mode over its scale, turned when it is turned (sys::Output::logical).

use std::time::{Duration, Instant};

use crate::sys::Output;

/// How long a change waits for "Keep changes" before it is put back by itself.
pub const KEEP_SECS: u64 = 15;

/// A screen's place and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Rect {
    fn right(&self) -> i32 {
        self.x + self.w
    }
    fn bottom(&self) -> i32 {
        self.y + self.h
    }

    /// The two cover some of the same room.
    pub fn overlaps(&self, o: &Rect) -> bool {
        self.x < o.right() && o.x < self.right() && self.y < o.bottom() && o.y < self.bottom()
    }

    /// The two share a stretch of an edge (a corner alone is not enough).
    pub fn touches(&self, o: &Rect) -> bool {
        let side = (self.right() == o.x || o.right() == self.x) && self.y < o.bottom() && o.y < self.bottom();
        let top = (self.bottom() == o.y || o.bottom() == self.y) && self.x < o.right() && o.x < self.right();
        side || top
    }
}

/// The screens shown in the space: switched on and not a copy of another.
pub fn placed(o: &Output) -> bool {
    o.enabled && o.mirror_of.is_none()
}

pub fn rect_of(o: &Output) -> Rect {
    let (w, h) = o.logical();
    Rect { x: o.x as i32, y: o.y as i32, w: w.max(1), h: h.max(1) }
}

/// The arrangement box and Extend, Duplicate and Disconnect are there once a second display is attached (a disconnected one
/// counts: it stays in the box to be connected again).
pub fn several(outs: &[Output]) -> bool {
    outs.len() >= 2
}

/// "DP-10" after "DP-2": letters as text, runs of digits as numbers.
fn natural(name: &str) -> Vec<(String, u64)> {
    let mut out = vec![];
    let (mut text, mut num, mut in_num) = (String::new(), String::new(), false);
    for c in name.chars() {
        if c.is_ascii_digit() {
            num.push(c);
            in_num = true;
        } else {
            if in_num {
                out.push((std::mem::take(&mut text), num.parse().unwrap_or(0)));
                num.clear();
                in_num = false;
            }
            text.push(c);
        }
    }
    out.push((text, num.parse().unwrap_or(0)));
    out
}

/// The number each attached screen goes by (1, 2, 3 ...), switched off or not: by connector, so a number stays while screens
/// are dragged, switched off or made main.
pub fn numbers(outs: &[Output]) -> Vec<(String, u32)> {
    let mut names: Vec<&str> = outs.iter().map(|o| o.name.as_str()).collect();
    names.sort_by_key(|n| natural(n));
    names.iter().enumerate().map(|(i, n)| (n.to_string(), i as u32 + 1)).collect()
}

pub fn number_of(outs: &[Output], name: &str) -> u32 {
    numbers(outs).into_iter().find(|(n, _)| n == name).map(|(_, k)| k).unwrap_or(0)
}

/// Where a screen of `size` wanted at `want` goes: against an edge of one of `touch`, with a stretch of edge shared, on top of
/// none of `touch` and `avoid`, as near `want` as that allows. Within `align` of lining up with that screen's top or bottom (left
/// or right above and below), it lines up. None when there is no such place (nothing to touch).
pub fn snap(size: (i32, i32), want: (i32, i32), touch: &[Rect], avoid: &[Rect], align: i32) -> Option<(i32, i32)> {
    let (w, h) = size;
    let near = |v: i32, edges: [i32; 2]| edges.into_iter().filter(|e| (e - v).abs() <= align).min_by_key(|e| (e - v).abs()).unwrap_or(v);
    let mut best: Option<((i32, i32), i64)> = None;
    for o in touch {
        let y = near(want.1.clamp(o.y - h + 1, o.bottom() - 1), [o.y, o.bottom() - h]);
        let x = near(want.0.clamp(o.x - w + 1, o.right() - 1), [o.x, o.right() - w]);
        for p in [(o.right(), y), (o.x - w, y), (x, o.bottom()), (x, o.y - h)] {
            let r = Rect { x: p.0, y: p.1, w, h };
            if touch.iter().chain(avoid).any(|q| q.overlaps(&r)) {
                continue;
            }
            let d = (p.0 - want.0) as i64 * (p.0 - want.0) as i64 + (p.1 - want.1) as i64 * (p.1 - want.1) as i64;
            if best.is_none_or(|(_, bd)| d < bd) {
                best = Some((p, d));
            }
        }
    }
    best.map(|(p, _)| p)
}

/// The screens connected to `anchor` edge to edge (its own place in `rects` among them).
fn joined(rects: &[Rect], anchor: usize) -> Vec<usize> {
    let mut seen = vec![anchor];
    let mut i = 0;
    while i < seen.len() {
        let r = rects[seen[i]];
        for (j, q) in rects.iter().enumerate() {
            if !seen.contains(&j) && r.touches(q) {
                seen.push(j);
            }
        }
        i += 1;
    }
    seen
}

/// Moves the screens not joined to `anchor` against it (the nearest first), so none stands apart.
fn join_up(rects: &mut [Rect], anchor: usize) {
    loop {
        let group = joined(rects, anchor);
        if group.len() == rects.len() {
            return;
        }
        let centre = |r: &Rect| (r.x as i64 * 2 + r.w as i64, r.y as i64 * 2 + r.h as i64);
        let dist = |a: &Rect, b: &Rect| {
            let (p, q) = (centre(a), centre(b));
            (p.0 - q.0).pow(2) + (p.1 - q.1).pow(2)
        };
        let Some(j) = (0..rects.len())
            .filter(|j| !group.contains(j))
            .min_by_key(|&j| group.iter().map(|&g| dist(&rects[j], &rects[g])).min().unwrap_or(0))
        else {
            return;
        };
        let touch: Vec<Rect> = group.iter().map(|&g| rects[g]).collect();
        let avoid: Vec<Rect> = (0..rects.len()).filter(|k| *k != j && !group.contains(k)).map(|k| rects[k]).collect();
        let r = rects[j];
        let (x, y) = snap((r.w, r.h), (r.x, r.y), &touch, &avoid, 0).unwrap_or_else(|| {
            // nowhere near: to the right of them all, level with the rightmost
            let last = touch.iter().max_by_key(|q| q.right()).copied().unwrap_or(r);
            (touch.iter().chain(&avoid).map(|q| q.right()).max().unwrap_or(0), last.y)
        });
        rects[j] = Rect { x, y, ..r };
    }
}

/// The main display at 0,0 (as Windows keeps it), the others where they are next to it; copies sit on what they copy.
fn settle(outs: &mut [Output]) {
    let main = outs
        .iter()
        .position(|o| placed(o) && o.primary)
        .or_else(|| (0..outs.len()).filter(|&i| placed(&outs[i])).min_by_key(|&i| (outs[i].x, outs[i].y)));
    let Some(m) = main else { return };
    let (dx, dy) = (outs[m].x, outs[m].y);
    for o in outs.iter_mut().filter(|o| placed(o)) {
        o.x -= dx;
        o.y -= dy;
    }
    let at: Vec<(String, i64, i64)> = outs.iter().filter(|o| placed(o)).map(|o| (o.name.clone(), o.x, o.y)).collect();
    for o in outs.iter_mut().filter(|o| o.enabled) {
        if let Some((_, x, y)) = o.mirror_of.as_ref().and_then(|of| at.iter().find(|(n, ..)| n == of)) {
            o.x = *x;
            o.y = *y;
        }
    }
}

/// Moves the placed screens to `rects` (in `idx` order) and settles them.
fn place(outs: &mut [Output], idx: &[usize], rects: &[Rect]) {
    for (&i, r) in idx.iter().zip(rects) {
        outs[i].x = r.x as i64;
        outs[i].y = r.y as i64;
    }
    settle(outs);
}

/// A screen dragged in the arrangement box and let go at `want`: it lands against the nearest edge (`align`: how close counts as
/// lined up), the others that it left standing apart join up again, and the main display is at 0,0.
pub fn arrange(outs: &mut [Output], name: &str, want: (i32, i32), align: i32) {
    let idx: Vec<usize> = (0..outs.len()).filter(|&i| placed(&outs[i])).collect();
    let Some(k) = idx.iter().position(|&i| outs[i].name == name) else { return };
    let mut rects: Vec<Rect> = idx.iter().map(|&i| rect_of(&outs[i])).collect();
    let others: Vec<Rect> = (0..rects.len()).filter(|&j| j != k).map(|j| rects[j]).collect();
    if let Some((x, y)) = snap((rects[k].w, rects[k].h), want, &others, &[], align) {
        rects[k].x = x;
        rects[k].y = y;
    }
    join_up(&mut rects, k);
    place(outs, &idx, &rects);
}

/// After a screen changed size (resolution, scale, turned) or one came or went: the others move to stay edge to edge with the
/// main display (one that still fits stays), and the main display is at 0,0.
pub fn tidy(outs: &mut [Output]) {
    let idx: Vec<usize> = (0..outs.len()).filter(|&i| placed(&outs[i])).collect();
    if idx.is_empty() {
        return;
    }
    let main = idx.iter().position(|&i| outs[i].primary).unwrap_or(0);
    let mut rects: Vec<Rect> = idx.iter().map(|&i| rect_of(&outs[i])).collect();
    // a screen another grew over moves out on the side it was on (beside it: sideways; above or below: up or down)
    for j in 0..rects.len() {
        if j == main {
            continue;
        }
        let r = rects[j];
        let Some(a) = (0..rects.len()).find(|&q| q != j && rects[q].overlaps(&r)).map(|q| rects[q]) else { continue };
        let want = if r.y < a.bottom() && a.y < r.bottom() {
            (if r.x >= a.x { a.right() } else { a.x - r.w }, r.y)
        } else {
            (r.x, if r.y >= a.y { a.bottom() } else { a.y - r.h })
        };
        let others: Vec<Rect> = (0..rects.len()).filter(|&q| q != j).map(|q| rects[q]).collect();
        if let Some((x, y)) = snap((r.w, r.h), want, &others, &[], 0) {
            rects[j] = Rect { x, y, ..r };
        }
    }
    join_up(&mut rects, main);
    place(outs, &idx, &rects);
}

/// What a screen does with the others.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenMode {
    /// Its own part of the desktop (the default).
    Extend,
    /// The same picture as that screen.
    Duplicate(String),
    /// Off, kept in the list to be connected again.
    Disconnect,
}

pub fn mode_of(o: &Output) -> ScreenMode {
    match (&o.mirror_of, o.enabled) {
        (_, false) => ScreenMode::Disconnect,
        (Some(of), true) => ScreenMode::Duplicate(of.clone()),
        (None, true) => ScreenMode::Extend,
    }
}

/// Not the last screen showing the desktop: some other screen stays on and not a copy.
pub fn can_disconnect(outs: &[Output], name: &str) -> bool {
    outs.iter().any(|o| o.name != name && placed(o))
}

/// The screens `name` may show a copy of: placed ones, not itself. Only while another one stays placed besides.
pub fn duplicate_targets(outs: &[Output], name: &str) -> Vec<String> {
    if !can_disconnect(outs, name) {
        return vec![];
    }
    outs.iter().filter(|o| o.name != name && placed(o)).map(|o| o.name.clone()).collect()
}

/// Sets what a screen does. The last screen showing the desktop cannot be disconnected or made a copy; the copies of a screen
/// that goes become screens of their own (Hyprland does not copy a copy), and one switched on goes to the right of the others.
pub fn set_mode(outs: &mut [Output], name: &str, mode: &ScreenMode) -> Result<(), String> {
    let Some(i) = outs.iter().position(|o| o.name == name) else { return Err(format!("{name} is not attached")) };
    if mode_of(&outs[i]) == *mode {
        return Ok(());
    }
    let last = "At least one display stays connected and showing the desktop";
    match mode {
        ScreenMode::Disconnect => {
            if !can_disconnect(outs, name) {
                return Err(last.into());
            }
            // its copies stay where it was, as screens of their own
            for o in outs.iter_mut().filter(|o| o.mirror_of.as_deref() == Some(name)) {
                o.mirror_of = None;
            }
            outs[i].enabled = false;
            outs[i].mirror_of = None;
            if outs[i].primary {
                outs[i].primary = false;
            }
        }
        ScreenMode::Duplicate(of) => {
            if !duplicate_targets(outs, name).contains(of) {
                return Err(if can_disconnect(outs, name) { format!("{of} cannot be copied") } else { last.into() });
            }
            for o in outs.iter_mut().filter(|o| o.mirror_of.as_deref() == Some(name)) {
                o.mirror_of = Some(of.clone());
            }
            let o = &mut outs[i];
            o.current = o.mode_to_use();
            o.enabled = true;
            o.mirror_of = Some(of.clone());
        }
        ScreenMode::Extend => {
            let right = outs.iter().filter(|o| placed(o)).map(|o| o.x + rect_of(o).w as i64).max().unwrap_or(0);
            let top = outs.iter().filter(|o| placed(o)).map(|o| o.y).min().unwrap_or(0);
            let o = &mut outs[i];
            o.current = o.mode_to_use();
            o.enabled = true;
            o.mirror_of = None;
            o.x = right;
            o.y = top;
        }
    }
    // the main display is worked out again where it went (displays::settle keeps it at 0,0)
    if !outs.iter().any(|o| o.primary && placed(o)) {
        if let Some(m) = (0..outs.len()).filter(|&k| placed(&outs[k])).min_by_key(|&k| (outs[k].x, outs[k].y)) {
            outs[m].primary = true;
        }
    }
    tidy(outs);
    Ok(())
}

/// One change made on the page, applied to the screens as the compositor reports them when it is made.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Mode(String, ScreenMode),
    /// A mode id ("2560x1440@59.95"): the resolution and the refresh rate.
    Resolution(String, String),
    Scale(String, f64),
    /// Turned (0 to 3: none, 90°, 180°, 270°).
    Turn(String, u32),
    Vrr(String, Option<u8>),
    /// Let go there in the arrangement box (lined up within the last number).
    Arrange(String, (i32, i32), i32),
}

impl Change {
    /// A change that can leave a screen black: it is put back after `KEEP_SECS` unless kept.
    pub fn needs_keep(&self) -> bool {
        matches!(self, Change::Mode(..) | Change::Resolution(..) | Change::Turn(..))
    }

    pub fn apply(&self, outs: &mut [Output]) -> Result<(), String> {
        let find = |outs: &mut [Output], n: &str| outs.iter().position(|o| o.name == n).ok_or(format!("{n} is not attached"));
        match self {
            Change::Mode(n, m) => return set_mode(outs, n, m),
            Change::Arrange(n, want, align) => {
                find(outs, n)?;
                arrange(outs, n, *want, *align);
                return Ok(());
            }
            Change::Resolution(n, id) => {
                let i = find(outs, n)?;
                if !outs[i].modes.iter().any(|m| m.id == *id) {
                    return Err(format!("{n} does not offer {id}"));
                }
                outs[i].current = id.clone();
            }
            Change::Scale(n, sc) => {
                let i = find(outs, n)?;
                outs[i].scale = sc.clamp(0.5, 4.0);
            }
            Change::Turn(n, t) => {
                let i = find(outs, n)?;
                outs[i].transform = *t % 4;
            }
            Change::Vrr(n, v) => {
                let i = find(outs, n)?;
                outs[i].vrr = v.map(|v| v.min(2));
            }
        }
        tidy(outs);
        Ok(())
    }
}

/// "Keep these display settings?": put back by itself once `KEEP_SECS` have gone unless kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Keep {
    pub deadline: Instant,
}

impl Keep {
    pub fn start(now: Instant) -> Keep {
        Keep { deadline: now + Duration::from_secs(KEEP_SECS) }
    }

    /// Whole seconds left, counted up (15 at the start, 1 in the last second).
    pub fn left(&self, now: Instant) -> u64 {
        let ms = self.deadline.saturating_duration_since(now).as_millis() as u64;
        ms.div_ceil(1000)
    }

    pub fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::sys::Mode;

    /// A made-up monitor of this size at this place, at 60 Hz, scale 1.
    pub fn screen(name: &str, (w, h): (u32, u32), (x, y): (i64, i64)) -> Output {
        let id = format!("{w}x{h}@60.00");
        Output {
            name: name.into(),
            make: "Example Display".into(),
            model: format!("M{w}"),
            enabled: true,
            primary: false,
            scale: 1.0,
            current: id.clone(),
            modes: vec![Mode { id, width: w, height: h, refresh: 60.0 }],
            x,
            y,
            transform: 0,
            mirror_of: None,
            vrr: None,
        }
    }

    /// Three made-up screens: a wide main one, a tall one at its right, a small one switched off.
    fn three() -> Vec<Output> {
        let mut wide = screen("DP-1", (3440, 1440), (0, 0));
        wide.primary = true;
        let tall = screen("HDMI-A-1", (2560, 2880), (3440, -720));
        let off = Output { enabled: false, ..screen("HDMI-A-2", (1920, 1080), (0, 0)) };
        vec![wide, tall, off]
    }

    fn at(outs: &[Output], name: &str) -> (i64, i64) {
        outs.iter().find(|o| o.name == name).map(|o| (o.x, o.y)).unwrap()
    }

    fn all_joined_and_apart(outs: &[Output]) -> bool {
        let r: Vec<Rect> = outs.iter().filter(|o| placed(o)).map(rect_of).collect();
        let apart = (0..r.len()).all(|i| (0..r.len()).all(|j| i == j || !r[i].overlaps(&r[j])));
        apart && joined(&r, 0).len() == r.len()
    }

    #[test]
    fn edges_meet_without_gaps_or_overlaps() {
        let wide = Rect { x: 0, y: 0, w: 3440, h: 1440 };
        // let go a little apart at the right: against the right edge, the height kept
        assert_eq!(snap((1920, 1080), (3600, 200), &[wide], &[], 0), Some((3440, 200)));
        // let go overlapping it: pushed out the nearest way
        assert_eq!(snap((1920, 1080), (3000, 200), &[wide], &[], 0), Some((3440, 200)));
        assert_eq!(snap((1920, 1080), (-1500, 100), &[wide], &[], 0), Some((-1920, 100)), "to the left");
        assert_eq!(snap((1920, 1080), (700, 1300), &[wide], &[], 0), Some((700, 1440)), "below");
        assert_eq!(snap((1920, 1080), (700, -900), &[wide], &[], 0), Some((700, -1080)), "above");
        // close to lining up: lines up with the top (or the bottom)
        assert_eq!(snap((1920, 1080), (3500, 30), &[wide], &[], 50), Some((3440, 0)));
        assert_eq!(snap((1920, 1080), (3500, 390), &[wide], &[], 50), Some((3440, 360)), "bottoms level");
        // far down the side: still a stretch of edge shared, never only a corner
        let (x, y) = snap((1920, 1080), (3440, 5000), &[wide], &[], 0).unwrap();
        assert!(Rect { x, y, w: 1920, h: 1080 }.touches(&wide), "{x},{y}");
        // a place taken by a third screen is not chosen
        let right = Rect { x: 3440, y: 0, w: 1920, h: 1080 };
        let p = snap((1920, 1080), (3500, 100), &[wide], &[right], 0).unwrap();
        assert!(!Rect { x: p.0, y: p.1, w: 1920, h: 1080 }.overlaps(&right), "{p:?}");
        assert_eq!(snap((10, 10), (0, 0), &[], &[], 0), None, "nothing to stand against");
    }

    #[test]
    fn a_dragged_screen_lands_edge_to_edge_and_the_main_one_stays_at_0_0() {
        let mut outs = three();
        // the tall screen dragged to the left of the wide one
        arrange(&mut outs, "HDMI-A-1", (-2500, -600), 40);
        assert_eq!(at(&outs, "DP-1"), (0, 0), "the main display stays at 0,0");
        assert_eq!(at(&outs, "HDMI-A-1"), (-2560, -600));
        assert!(all_joined_and_apart(&outs));
        // the main display dragged: everything moves so it is at 0,0 again, the tall one where it was next to it
        arrange(&mut outs, "DP-1", (-2560 - 3440 - 50, -600), 40);
        assert_eq!(at(&outs, "DP-1"), (0, 0));
        assert_eq!(at(&outs, "HDMI-A-1"), (3440, 0), "now at its right, lined up at the top");
        assert!(all_joined_and_apart(&outs));
    }

    #[test]
    fn a_screen_left_apart_by_a_drag_joins_up() {
        // a row: wide, middle, end; the middle one dragged below the wide one leaves the end one apart
        let mut outs =
            vec![screen("DP-1", (1000, 500), (0, 0)), screen("DP-2", (500, 500), (1000, 0)), screen("DP-3", (500, 500), (1500, 0))];
        outs[0].primary = true;
        arrange(&mut outs, "DP-2", (100, 600), 0);
        assert_eq!(at(&outs, "DP-2"), (100, 500));
        assert!(all_joined_and_apart(&outs), "{:?}", outs.iter().map(|o| (o.x, o.y)).collect::<Vec<_>>());
        assert_eq!(at(&outs, "DP-3"), (1000, 0), "moved in against the wide one, where it was nearest");
    }

    #[test]
    fn a_bigger_mode_pushes_the_neighbour_out_not_over() {
        let mut outs = three();
        let tall = outs.iter_mut().find(|o| o.name == "HDMI-A-1").unwrap();
        tall.x = 3440;
        tall.y = 0;
        let big = outs.iter_mut().find(|o| o.name == "DP-1").unwrap();
        big.modes.push(Mode { id: "5120x1440@60.00".into(), width: 5120, height: 1440, refresh: 60.0 });
        big.current = "5120x1440@60.00".into();
        tidy(&mut outs);
        assert_eq!(at(&outs, "HDMI-A-1"), (5120, 0));
        assert!(all_joined_and_apart(&outs));
        // a scale makes it smaller: the neighbour comes back against it
        outs.iter_mut().find(|o| o.name == "DP-1").unwrap().scale = 2.0;
        tidy(&mut outs);
        assert_eq!(at(&outs, "HDMI-A-1"), (2560, 0));
        // turned: tall instead of wide
        outs.iter_mut().find(|o| o.name == "HDMI-A-1").unwrap().transform = 1;
        tidy(&mut outs);
        assert_eq!(rect_of(outs.iter().find(|o| o.name == "HDMI-A-1").unwrap()), Rect { x: 2560, y: 0, w: 2880, h: 2560 });
    }

    #[test]
    fn extend_duplicate_and_disconnect() {
        let mut outs = three();
        // the switched-off one connected again: to the right of the others, at the top
        set_mode(&mut outs, "HDMI-A-2", &ScreenMode::Extend).unwrap();
        assert!(outs[2].enabled);
        assert_eq!(at(&outs, "HDMI-A-2"), (6000, -720));
        assert!(all_joined_and_apart(&outs));
        // a copy of the main one: no place of its own, Hyprland's mirror
        set_mode(&mut outs, "HDMI-A-2", &ScreenMode::Duplicate("DP-1".into())).unwrap();
        assert_eq!(outs[2].mirror_of.as_deref(), Some("DP-1"));
        assert_eq!(at(&outs, "HDMI-A-2"), (0, 0), "it sits on what it copies");
        assert!(all_joined_and_apart(&outs));
        // the main one disconnected: its copy becomes a screen of its own where it was, the top-left one is the main display (as
        // the panel falls back to it) at 0,0, and the tall one joins it
        set_mode(&mut outs, "DP-1", &ScreenMode::Disconnect).unwrap();
        assert!(!outs[0].enabled && outs[2].mirror_of.is_none());
        let main: Vec<&str> = outs.iter().filter(|o| o.primary).map(|o| o.name.as_str()).collect();
        assert_eq!(main, ["HDMI-A-2"]);
        assert_eq!((at(&outs, "HDMI-A-2"), at(&outs, "HDMI-A-1")), ((0, 0), (1920, -720)));
        assert!(all_joined_and_apart(&outs));
        assert_eq!(mode_of(&outs[0]), ScreenMode::Disconnect);
        assert!(set_mode(&mut outs, "DP-9", &ScreenMode::Extend).is_err());
    }

    #[test]
    fn the_last_screen_stays_connected() {
        let mut outs = three();
        set_mode(&mut outs, "HDMI-A-1", &ScreenMode::Disconnect).unwrap();
        assert!(!can_disconnect(&outs, "DP-1"));
        assert!(set_mode(&mut outs, "DP-1", &ScreenMode::Disconnect).is_err(), "the last one");
        assert!(outs[0].enabled, "unchanged");
        // nor made a copy of a screen that is off, or of itself
        assert!(duplicate_targets(&outs, "DP-1").is_empty());
        assert!(set_mode(&mut outs, "DP-1", &ScreenMode::Duplicate("HDMI-A-1".into())).is_err());
        // with a copy beside it, the one it copies is still the last one showing the desktop
        set_mode(&mut outs, "HDMI-A-2", &ScreenMode::Duplicate("DP-1".into())).unwrap();
        assert!(!can_disconnect(&outs, "DP-1"));
        assert!(can_disconnect(&outs, "HDMI-A-2"), "the copy may go");
        assert!(set_mode(&mut outs, "DP-1", &ScreenMode::Duplicate("HDMI-A-2".into())).is_err(), "not a copy of a copy");
    }

    #[test]
    fn one_display_has_no_box_and_numbers_follow_the_connectors() {
        let one = vec![screen("eDP-1", (2880, 1800), (0, 0))];
        assert!(!several(&one), "one display: its settings at once, no box, no Disconnect or Duplicate");
        assert!(!can_disconnect(&one, "eDP-1") && duplicate_targets(&one, "eDP-1").is_empty());
        let mut two = one.clone();
        two.push(Output { enabled: false, ..screen("HDMI-A-1", (1920, 1080), (0, 0)) });
        assert!(several(&two), "a disconnected one counts as attached");
        let outs = vec![screen("DP-10", (10, 10), (0, 0)), screen("HDMI-A-1", (10, 10), (0, 0)), screen("DP-2", (10, 10), (0, 0))];
        assert_eq!(numbers(&outs), [("DP-2".to_string(), 1), ("DP-10".to_string(), 2), ("HDMI-A-1".to_string(), 3)], "DP-2 before DP-10");
        assert_eq!(number_of(&outs, "HDMI-A-1"), 3);
    }

    #[test]
    fn changes_from_the_page() {
        let mut outs = three();
        assert!(Change::Resolution("DP-1".into(), "1x1@60.00".into()).apply(&mut outs).is_err(), "a mode it does not offer");
        Change::Turn("HDMI-A-1".into(), 1).apply(&mut outs).unwrap();
        assert_eq!(rect_of(&outs[1]), Rect { x: 3440, y: -720, w: 2880, h: 2560 }, "turned: wide, still against the main one");
        Change::Scale("DP-1".into(), 2.0).apply(&mut outs).unwrap();
        assert_eq!(at(&outs, "HDMI-A-1"), (1720, -720), "the main one smaller: its neighbour comes in");
        Change::Vrr("DP-1".into(), Some(9)).apply(&mut outs).unwrap();
        assert_eq!(outs[0].vrr, Some(2));
        assert!(Change::Mode("DP-1".into(), ScreenMode::Extend).needs_keep() && !Change::Scale("DP-1".into(), 1.0).needs_keep());
        assert!(Change::Arrange("DP-7".into(), (0, 0), 0).apply(&mut outs).is_err());
    }

    #[test]
    fn keep_counts_down_fifteen_seconds() {
        let t = Instant::now();
        let k = Keep::start(t);
        assert_eq!(k.left(t), 15);
        assert_eq!(k.left(t + Duration::from_millis(14_200)), 1);
        assert!(!k.expired(t + Duration::from_millis(14_999)));
        assert!(k.expired(t + Duration::from_secs(15)));
        assert_eq!(k.left(t + Duration::from_secs(20)), 0);
    }
}
