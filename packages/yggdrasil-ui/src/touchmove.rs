//! Moving and resizing a window with a finger. The title bar and the edges ask the compositor to move or resize the window with
//! the pointer (xdg_toplevel move/resize, through winit's drag_window), which needs a pointer button's serial: a finger has
//! none, so on a touchscreen nothing happened (an X1 Yoga in tablet mode, 2026-11-13). A finger's drag is done here instead:
//! about 60 times a second the window is placed where the finger has taken it, through the compositor (yggdrasil-wm: Hyprland
//! or Yggdrasil's own), from where the finger is in the window and where the window really is.

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

/// What the finger does: move the window, or resize it from these edges.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Move,
    Resize { left: bool, right: bool, top: bool, bottom: bool },
}

/// The smallest a window is made by a finger (logical pixels).
pub const MIN: (i32, i32) = (240, 160);

/// The window's rectangle (x, y, width, height) after the finger moved `d` from where it started, for `kind`.
pub fn target(start: (i32, i32, i32, i32), kind: Kind, d: (i32, i32)) -> (i32, i32, i32, i32) {
    let (x, y, w, h) = start;
    match kind {
        Kind::Move => (x + d.0, y + d.1, w, h),
        Kind::Resize { left, right, top, bottom } => {
            let (mut nx, mut nw) = (x, w);
            if right {
                nw = (w + d.0).max(MIN.0);
            } else if left {
                nw = (w - d.0).max(MIN.0);
                nx = x + w - nw;
            }
            let (mut ny, mut nh) = (y, h);
            if bottom {
                nh = (h + d.1).max(MIN.1);
            } else if top {
                nh = (h - d.1).max(MIN.1);
                ny = y + h - nh;
            }
            (nx, ny, nw, nh)
        }
    }
}

struct Grab {
    kind: Kind,
    /// Where the finger is in the window (logical pixels): when it touched, and now.
    start: (f64, f64),
    now: (f64, f64),
}

fn slot() -> &'static Arc<Mutex<Option<Grab>>> {
    static S: OnceLock<Arc<Mutex<Option<Grab>>>> = OnceLock::new();
    S.get_or_init(|| Arc::new(Mutex::new(None)))
}

pub fn active() -> bool {
    slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

/// A finger started dragging at `local` (logical pixels in the window): move or resize until `end`.
pub fn begin(kind: Kind, local: (f64, f64)) {
    let s = slot().clone();
    {
        let Ok(mut g) = s.lock() else { return };
        if g.is_some() {
            return;
        }
        *g = Some(Grab { kind, start: local, now: local });
    }
    std::thread::spawn(move || follow(s));
}

pub fn update(local: (f64, f64)) {
    if let Ok(mut g) = slot().lock() {
        if let Some(g) = g.as_mut() {
            g.now = local;
        }
    }
}

pub fn end() {
    if let Ok(mut g) = slot().lock() {
        *g = None;
    }
}

/// The window under the finger: the one in front (a touch focuses the window it lands on).
fn own_window(desk: &yggdrasil_wm::Desk) -> Option<(String, (i32, i32, i32, i32))> {
    let w = desk.active_window(true)?;
    if w.fullscreen {
        return None;
    }
    let (at, size) = (w.at?, w.size?);
    Some((w.id, (at.0 as i32, at.1 as i32, size.0 as i32, size.1 as i32)))
}

fn follow(s: Arc<Mutex<Option<Grab>>>) {
    let Some(desk) = yggdrasil_wm::Desk::detect() else {
        end();
        return;
    };
    let Some((id, start_rect)) = own_window(&desk) else {
        end();
        return;
    };
    let first = s.lock().ok().and_then(|g| g.as_ref().map(|g| g.start));
    let Some(first) = first else { return };
    // where the finger was on the screen when it touched
    let finger0 = (start_rect.0 as f64 + first.0, start_rect.1 as f64 + first.1);
    let mut last = start_rect;
    loop {
        std::thread::sleep(Duration::from_millis(16));
        let (kind, now) = match s.lock().ok().and_then(|g| g.as_ref().map(|g| (g.kind, g.now))) {
            Some(v) => v,
            None => return,
        };
        // where the window really is now (the compositor may still be catching up), and so where the finger is on the screen
        let at = desk.windows().into_iter().find(|w| w.id == id).and_then(|w| w.at).unwrap_or((last.0 as f64, last.1 as f64));
        let finger = (at.0 + now.0, at.1 + now.1);
        let d = ((finger.0 - finger0.0).round() as i32, (finger.1 - finger0.1).round() as i32);
        let t = target(start_rect, kind, d);
        if t != last {
            desk.place(&id, true, t);
            last = t;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_finger_moves_and_resizes_from_the_edge_it_holds() {
        let r = (100, 100, 800, 600);
        assert_eq!(target(r, Kind::Move, (50, -20)), (150, 80, 800, 600));
        let se = Kind::Resize { left: false, right: true, top: false, bottom: true };
        assert_eq!(target(r, se, (100, 50)), (100, 100, 900, 650));
        let nw = Kind::Resize { left: true, right: false, top: true, bottom: false };
        assert_eq!(target(r, nw, (100, 50)), (200, 150, 700, 550), "the far corner stays put");
        // never smaller than MIN, and the far edge still stays put
        assert_eq!(target(r, nw, (700, 500)), (660, 540, 240, 160));
        let e = Kind::Resize { left: false, right: true, top: false, bottom: false };
        assert_eq!(target(r, e, (-900, 30)), (100, 100, 240, 600));
    }
}
