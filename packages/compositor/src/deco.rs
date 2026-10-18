//! Window controls drawn by the compositor for windows that ask for server-side decoration (Qt programs, for example): no title bar of its
//! own, only a small dark pill at the top right of the window with a grip and three round buttons (minimize, maximize, close), level with
//! the program's own menu or tool bar, so the program keeps the whole height of its window. Programs that draw their own controls
//! (Minos, Settings, the Marketplace) never get one. Move a window by the grip, or with Super and the left mouse button anywhere on it
//! (Super and the right button resizes).

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;
use tiny_skia::Pixmap;

use crate::draw::{fill_circle, fill_rounded};

pub const PILL_W: i32 = 112;
pub const PILL_H: i32 = 28;
/// Distance from the window's top and right edge.
pub const MARGIN: i32 = 6;
/// How far outside a window its resize grips reach.
pub const GRIP: i32 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Minimize,
    Maximize,
    Close,
    /// The grip (and the gaps): drag the window, double click to maximize.
    Drag,
}

/// Where the button circles are: the centre x, measured from the pill's right edge.
const BUTTONS: [(Hit, f64); 3] = [(Hit::Close, 18.0), (Hit::Maximize, 42.0), (Hit::Minimize, 66.0)];

/// What a point `x` along the pill (`PILL_W` wide) is on.
pub fn hit(x: f64) -> Hit {
    for (h, from_right) in BUTTONS {
        if (x - (PILL_W as f64 - from_right)).abs() <= 10.0 {
            return h;
        }
    }
    Hit::Drag
}

/// What the pill was last drawn from; it is drawn again only when one of these changes.
#[derive(Clone, PartialEq)]
pub struct Key {
    pub active: bool,
    pub hover: Option<Hit>,
}

pub struct Bar {
    pub key: Key,
    pub buffer: MemoryRenderBuffer,
}

pub fn make(key: Key) -> Bar {
    let (w, h) = (PILL_W as u32, PILL_H as u32);
    let mut pix = Pixmap::new(w, h).expect("pill size");
    fill_rounded(&mut pix, 0.0, 0.0, w as f32, h as f32, h as f32 / 2.0, if key.active { (20, 20, 28, 225) } else { (20, 20, 28, 170) });
    // the grip: two columns of three dots
    for col in 0..2 {
        for row in 0..3 {
            fill_circle(&mut pix, 14.0 + col as f32 * 7.0, h as f32 / 2.0 - 6.0 + row as f32 * 6.0, 1.4, (200, 200, 214, if key.hover == Some(Hit::Drag) { 230 } else { 130 }));
        }
    }
    let colors = [(Hit::Close, (255, 95, 87, 255)), (Hit::Maximize, (39, 201, 63, 255)), (Hit::Minimize, (255, 189, 46, 255))];
    for (b, c) in colors {
        let from_right = BUTTONS.iter().find(|x| x.0 == b).map(|x| x.1).unwrap_or(18.0);
        let shown = if key.active || key.hover == Some(b) { c } else { (90, 90, 104, 255) };
        let r = if key.hover == Some(b) { 7.5 } else { 6.0 };
        fill_circle(&mut pix, w as f32 - from_right as f32, h as f32 / 2.0, r, shown);
    }
    // tiny-skia is premultiplied RGBA; the buffer wants Argb8888 (BGRA bytes)
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for p in pix.data().chunks_exact(4) {
        data.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    let buffer = MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (w as i32, h as i32), 1, Transform::Normal, None);
    Bar { key, buffer }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_buttons_are_where_they_are_drawn() {
        assert_eq!(hit(PILL_W as f64 - 18.0), Hit::Close);
        assert_eq!(hit(PILL_W as f64 - 42.0), Hit::Maximize);
        assert_eq!(hit(PILL_W as f64 - 66.0), Hit::Minimize);
        assert_eq!(hit(14.0), Hit::Drag);
    }
}
