//! Title bars drawn by the compositor for windows that ask for them (xdg-decoration "server side"): a dark bar with the window's title and
//! three round buttons (minimize, maximize, close) at the right, in the same look as Settings, Minos and the Marketplace. Programs that draw
//! their own bar (those three, and most modern apps) never get one.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;
use tiny_skia::Pixmap;

use crate::draw::{draw_text, fill_circle, fill_rounded, text_width, Fonts};

pub const BAR_H: i32 = 34;
/// How far outside a window its resize grips reach.
pub const GRIP: i32 = 6;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Minimize,
    Maximize,
    Close,
    /// Anywhere else on the bar: drag the window, double click to maximize.
    Drag,
}

/// Where the button circles are: the centre x, measured from the bar's right edge.
const BUTTONS: [(Hit, f64); 3] = [(Hit::Close, 20.0), (Hit::Maximize, 44.0), (Hit::Minimize, 68.0)];

/// What a point `x` along a bar `width` wide is on.
pub fn hit(x: f64, width: i32) -> Hit {
    for (h, from_right) in BUTTONS {
        if (x - (width as f64 - from_right)).abs() <= 10.0 {
            return h;
        }
    }
    Hit::Drag
}

/// What the bar was last drawn from; it is drawn again only when one of these changes.
#[derive(Clone, PartialEq)]
pub struct Key {
    pub title: String,
    pub width: i32,
    pub active: bool,
    pub hover: Option<Hit>,
    pub maximized: bool,
}

pub struct Bar {
    pub key: Key,
    pub buffer: MemoryRenderBuffer,
}

pub fn make(fonts: &Fonts, key: Key) -> Bar {
    let (w, h) = (key.width.max(1) as u32, BAR_H as u32);
    let mut pix = Pixmap::new(w, h).expect("bar size");
    let bg = if key.active { (24, 24, 33, 255) } else { (17, 17, 24, 255) };
    // rounded at the top, straight below: the program's own picture starts right under the bar
    fill_rounded(&mut pix, 0.0, 0.0, w as f32, h as f32 + 12.0, 10.0, bg);
    fill_rounded(&mut pix, 0.0, h as f32 - 1.0, w as f32, 1.0, 0.0, (255, 255, 255, 22));
    let (fg, dim) = ((236, 236, 244, 255), (140, 140, 156, 255));
    let max_w = (w as f32 - 100.0).max(20.0);
    let mut title = key.title.clone();
    while title.chars().count() > 1 && text_width(&fonts.regular, 13.0, &title) > max_w {
        title.pop();
        if text_width(&fonts.regular, 13.0, &format!("{title}…")) <= max_w {
            title.push('…');
            break;
        }
    }
    draw_text(&mut pix, &fonts.regular, 13.0, 14.0, h as f32 / 2.0, if key.active { fg } else { dim }, &title);
    let colors = [(Hit::Close, (255, 95, 87, 255)), (Hit::Maximize, (39, 201, 63, 255)), (Hit::Minimize, (255, 189, 46, 255))];
    for (b, c) in colors {
        let from_right = BUTTONS.iter().find(|x| x.0 == b).map(|x| x.1).unwrap_or(20.0);
        let faded = if key.active || key.hover == Some(b) { c } else { (90, 90, 104, 255) };
        let r = if key.hover == Some(b) { 7.5 } else { 6.0 };
        fill_circle(&mut pix, w as f32 - from_right as f32, h as f32 / 2.0, r, faded);
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
        assert_eq!(hit(300.0 - 20.0, 300), Hit::Close);
        assert_eq!(hit(300.0 - 44.0, 300), Hit::Maximize);
        assert_eq!(hit(300.0 - 68.0, 300), Hit::Minimize);
        assert_eq!(hit(40.0, 300), Hit::Drag);
    }
}
