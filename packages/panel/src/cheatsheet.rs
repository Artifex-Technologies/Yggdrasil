//! The keyboard shortcuts card (Win + /): every shortcut the session has right now, grouped (yggdrasil_conf::shortcuts), on a card in
//! the middle of the screen over a dimmed background. Any key or click closes it.

use std::process::{Command, Stdio};

use resvg::tiny_skia::Pixmap;
use yggdrasil_conf::shortcuts::{self, Bind, Shortcut, GROUPS};

use crate::paint::{self, Fonts, Rgba};

/// The live shortcuts (Hyprland's binds), or the session file's when Hyprland does not answer.
pub fn load() -> Vec<Shortcut> {
    let live = Command::new("hyprctl")
        .args(["-j", "binds"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .and_then(|o| serde_json::from_slice::<serde_json::Value>(&o.stdout).ok())
        .map(|v| from_json(&v))
        .unwrap_or_default();
    if !live.is_empty() {
        return live;
    }
    std::fs::read_to_string("/usr/share/yggdrasil/hyprland/yggdrasil.lua").map(|t| shortcuts::from_lua(&t)).unwrap_or_default()
}

/// `hyprctl -j binds` as the shortcut list.
pub fn from_json(v: &serde_json::Value) -> Vec<Shortcut> {
    let binds: Vec<Bind> = v
        .as_array()
        .map(|a| {
            a.iter()
                .map(|b| Bind {
                    modmask: b["modmask"].as_u64().unwrap_or(0) as u32,
                    key: b["key"].as_str().filter(|k| !k.is_empty()).map(str::to_string).unwrap_or_else(|| format!("code:{}", b["keycode"].as_u64().unwrap_or(0))),
                    desc: b["description"].as_str().unwrap_or("").to_string(),
                    release: b["release"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    shortcuts::list(&binds)
}

/// `text` cut to fit `max` pixels, with an ellipsis.
fn fit(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> String {
    if paint::text_width(font, size, text) <= max {
        return text.to_string();
    }
    let mut t: String = text.to_string();
    while !t.is_empty() && paint::text_width(font, size, &format!("{t}…")) > max {
        t.pop();
    }
    format!("{}…", t.trim_end())
}

/// The card over the whole surface (`s` is its scale).
pub fn paint(pix: &mut Pixmap, s: f32, list: &[Shortcut], dark: bool, fonts: &Fonts) {
    let (w, h) = (pix.width() as f32 / s, pix.height() as f32 / s);
    pix.fill(paint::color((0, 0, 0, 110)));
    let (bg, text, dim, pill): (Rgba, Rgba, Rgba, Rgba) = if dark {
        ((24, 24, 30, 248), (240, 240, 245, 255), (150, 150, 165, 255), (255, 255, 255, 22))
    } else {
        ((250, 250, 252, 248), (20, 20, 28, 255), (100, 100, 115, 255), (0, 0, 0, 16))
    };
    let groups: Vec<(&str, Vec<&Shortcut>)> = GROUPS.iter().map(|g| (*g, list.iter().filter(|x| x.group == *g).collect::<Vec<_>>())).filter(|(_, r)| !r.is_empty()).collect();
    // three columns (two on a narrow screen), groups placed in order, each in the shortest column so far
    let cols = if w >= 1300.0 { 3 } else { 2 };
    let (row_h, head_h, gap) = (26.0, 34.0, 14.0);
    let cw = ((w - 80.0).min(1240.0) / cols as f32).floor();
    let mut heights = vec![0.0f32; cols];
    let mut placed: Vec<(usize, f32, usize)> = vec![];
    for (gi, (_, rows)) in groups.iter().enumerate() {
        let c = (0..cols).min_by(|a, b| heights[*a].total_cmp(&heights[*b])).unwrap_or(0);
        placed.push((gi, heights[c], c));
        heights[c] += head_h + rows.len() as f32 * row_h + gap;
    }
    let pad = 28.0;
    let card_w = cw * cols as f32 + pad * 2.0;
    let body_h = heights.iter().cloned().fold(0.0, f32::max);
    let card_h = (60.0 + body_h + pad).min(h - 40.0);
    let (x0, y0) = ((w - card_w) / 2.0, (h - card_h) / 2.0);
    paint::fill_rounded(pix, x0 * s, y0 * s, card_w * s, card_h * s, 16.0 * s, bg);
    paint::draw_text(pix, &fonts.bold, 16.0 * s, (x0 + pad) * s, (y0 + pad + 6.0) * s, text, "Keyboard shortcuts");
    let hint = "Any key closes this";
    let hw = paint::text_width(&fonts.regular, 11.5 * s, hint);
    paint::draw_text(pix, &fonts.regular, 11.5 * s, (x0 + card_w - pad) * s - hw, (y0 + pad + 6.0) * s, dim, hint);
    if groups.is_empty() {
        paint::draw_text(pix, &fonts.regular, 13.0 * s, (x0 + pad) * s, (y0 + 70.0) * s, dim, "No shortcuts could be read from the compositor.");
        return;
    }
    let top = y0 + 60.0;
    let bottom = y0 + card_h - 12.0;
    for (gi, gy, c) in placed {
        let (name, rows) = &groups[gi];
        let gx = x0 + pad + c as f32 * cw;
        let mut y = top + gy;
        if y + head_h > bottom {
            continue;
        }
        paint::draw_text(pix, &fonts.bold, 11.0 * s, gx * s, (y + 12.0) * s, dim, &name.to_uppercase());
        y += head_h - 6.0;
        for r in rows {
            if y + row_h > bottom {
                break;
            }
            let keys = fit(&fonts.bold, 12.0 * s, &r.keys, (cw * 0.45) * s);
            let kw = paint::text_width(&fonts.bold, 12.0 * s, &keys) / s;
            paint::fill_rounded(pix, gx * s, (y + 2.0) * s, (kw + 14.0) * s, 21.0 * s, 6.0 * s, pill);
            paint::draw_text(pix, &fonts.bold, 12.0 * s, (gx + 7.0) * s, (y + 12.5) * s, text, &keys);
            let tx = gx + kw + 24.0;
            let what = fit(&fonts.regular, 12.5 * s, &r.what, (gx + cw - 16.0 - tx) * s);
            paint::draw_text(pix, &fonts.regular, 12.5 * s, tx * s, (y + 12.5) * s, text, &what);
            y += row_h;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hyprlands_list_is_read() {
        let v = serde_json::json!([
            {"modmask": 64, "key": "SLASH", "keycode": 0, "description": "Keyboard shortcuts", "release": false},
            {"modmask": 68, "key": "", "keycode": 10, "description": "Desktop 1", "release": false},
            {"modmask": 4, "key": "CONTROL_L", "keycode": 0, "description": "Find my mouse (press Ctrl twice)", "release": true},
        ]);
        let l = from_json(&v);
        assert!(l.iter().any(|s| s.keys == "Win + /" && s.group == "System"));
        assert!(l.iter().any(|s| s.keys == "Win + Ctrl + 1" && s.group == "Desktops"));
        assert!(l.iter().any(|s| s.keys == "Ctrl twice" && s.group == "Mouse"));
    }

    #[test]
    fn the_card_draws_on_a_small_screen_too() {
        let Some(fonts) = Fonts::load() else { return };
        let list = from_json(&serde_json::json!([{"modmask": 64, "key": "E", "description": "Files (Minos) with a very long description that has to be cut", "release": false}]));
        for (w, h) in [(1280, 720), (3440, 1440)] {
            let mut pix = Pixmap::new(w, h).unwrap();
            paint(&mut pix, 1.0, &list, true, &fonts);
            assert!(pix.pixel(w / 2, h / 2).unwrap().alpha() > 200, "the card is in the middle");
        }
    }
}
