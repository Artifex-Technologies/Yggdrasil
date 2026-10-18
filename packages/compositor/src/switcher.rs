//! The window switcher shown while Alt is held after Tab: a row of cards, one per window (most recently used first), the chosen one
//! highlighted. Letting go of Alt activates it; Escape cancels.

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::utils::Transform;
use tiny_skia::Pixmap;

use crate::draw::{draw_text, fill_circle, fill_rounded, text_width, Fonts};

pub const CARD_W: i32 = 200;
pub const CARD_H: i32 = 96;
const GAP: i32 = 12;
const PAD: i32 = 18;
/// At most this many cards in a row; more windows scroll the selection out of view, so only the ones around it are drawn.
pub const MAX_CARDS: usize = 7;

/// The windows to draw: the chosen one and as many around it as fit.
pub fn visible(total: usize, index: usize) -> std::ops::Range<usize> {
    if total <= MAX_CARDS {
        return 0..total;
    }
    let start = index.saturating_sub(MAX_CARDS / 2).min(total - MAX_CARDS);
    start..start + MAX_CARDS
}

pub fn size(cards: usize) -> (i32, i32) {
    let n = cards.max(1) as i32;
    (PAD * 2 + n * CARD_W + (n - 1) * GAP, PAD * 2 + CARD_H)
}

fn clip(fonts: &Fonts, size: f32, s: &str, max: f32) -> String {
    let mut t = s.to_string();
    if text_width(&fonts.regular, size, &t) <= max {
        return t;
    }
    while t.chars().count() > 1 {
        t.pop();
        let c = format!("{t}…");
        if text_width(&fonts.regular, size, &c) <= max {
            return c;
        }
    }
    t
}

/// `entries` are (title, app id); `index` is the chosen one (an index into `entries`).
pub fn make(fonts: &Fonts, entries: &[(String, String)], index: usize, accent: (u8, u8, u8)) -> MemoryRenderBuffer {
    let shown = visible(entries.len(), index);
    let (w, h) = size(shown.len());
    let mut pix = Pixmap::new(w as u32, h as u32).expect("switcher size");
    fill_rounded(&mut pix, 0.0, 0.0, w as f32, h as f32, 18.0, (18, 18, 26, 238));
    fill_rounded(&mut pix, 0.0, 0.0, w as f32, 1.0, 0.0, (255, 255, 255, 20));
    for (slot, i) in shown.enumerate() {
        let (title, app_id) = &entries[i];
        let x = (PAD + slot as i32 * (CARD_W + GAP)) as f32;
        let y = PAD as f32;
        let chosen = i == index;
        fill_rounded(&mut pix, x, y, CARD_W as f32, CARD_H as f32, 12.0, if chosen { (accent.0, accent.1, accent.2, 255) } else { (40, 40, 54, 255) });
        // the first letter in a circle stands in for an icon
        let letter = app_id.chars().chain(title.chars()).find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
        fill_circle(&mut pix, x + 30.0, y + 32.0, 18.0, if chosen { (255, 255, 255, 60) } else { (accent.0, accent.1, accent.2, 255) });
        draw_text(&mut pix, &fonts.bold, 18.0, x + 30.0 - text_width(&fonts.bold, 18.0, &letter) / 2.0, y + 32.0, (255, 255, 255, 255), &letter);
        draw_text(&mut pix, &fonts.regular, 13.0, x + 14.0, y + 70.0, (255, 255, 255, 255), &clip(fonts, 13.0, title, CARD_W as f32 - 28.0));
        if !app_id.is_empty() {
            draw_text(&mut pix, &fonts.regular, 11.0, x + 62.0, y + 32.0, (255, 255, 255, if chosen { 200 } else { 130 }), &clip(fonts, 11.0, app_id, CARD_W as f32 - 76.0));
        }
    }
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for p in pix.data().chunks_exact(4) {
        data.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (w, h), 1, Transform::Normal, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn few_windows_all_show() {
        assert_eq!(visible(3, 1), 0..3);
    }

    #[test]
    fn many_windows_show_the_ones_around_the_choice() {
        assert_eq!(visible(20, 0), 0..7);
        assert_eq!(visible(20, 10), 7..14);
        assert_eq!(visible(20, 19), 13..20);
        assert!(visible(20, 10).contains(&10));
    }
}
