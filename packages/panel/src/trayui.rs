//! The two pop-ups of the tray: an app's own menu (read over dbusmenu), and the list of icons the apps keep hidden. Both are a plain list of
//! rows under the bar, drawn like the calendar's card.

use resvg::tiny_skia::Pixmap;

use crate::paint::{self, Fonts, Icons};
use crate::shell::Model;
use crate::tray::MenuItem;

pub type Rect = (f32, f32, f32, f32);

#[derive(Clone, Debug)]
pub enum TrayMode {
    /// The hidden icons.
    Overflow,
    /// One app's menu: the entries, and the ids of the submenus opened so far.
    Menu { key: String, items: Vec<MenuItem>, path: Vec<i32> },
}

#[derive(Clone, Debug)]
pub struct TrayPop {
    /// Where on the bar the card hangs from (logical px).
    pub anchor_x: f32,
    pub mode: TrayMode,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowAct {
    /// Activate this tray item (overflow list).
    Activate(String),
    /// Choose a menu entry of this item.
    Choose(String, i32),
    /// Open a submenu.
    Sub(i32),
    Back,
}

const ROW_H: f32 = 30.0;
const SEP_H: f32 = 9.0;
const CARD_W: f32 = 290.0;

/// The entries at the level the user has navigated to.
pub fn current<'a>(items: &'a [MenuItem], path: &[i32]) -> &'a [MenuItem] {
    let mut level = items;
    for id in path {
        match level.iter().find(|m| m.id == *id) {
            Some(m) => level = &m.children,
            None => break,
        }
    }
    level
}

/// Draws the card and returns its rectangle and the clickable rows (logical px; `s` is the scale).
pub fn paint(pix: &mut Pixmap, s: f32, pop: &TrayPop, model: &Model, pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons) -> (Rect, Vec<(Rect, RowAct)>) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let w = pix.width() as f32 / s;
    let dark = model.dark;
    let (bg, border, text, dim) = if dark { ((18, 18, 24, 244), (255, 255, 255, 36), (240, 240, 246, 255), (140, 140, 156, 255)) } else { ((250, 250, 252, 246), (0, 0, 0, 30), (24, 24, 32, 255), (130, 130, 146, 255)) };
    // what to list
    struct Row {
        label: String,
        sep: bool,
        enabled: bool,
        checked: Option<bool>,
        sub: bool,
        icon: Option<usize>,
        act: Option<RowAct>,
    }
    let mut rows: Vec<Row> = vec![];
    let passive: Vec<&crate::tray::TrayItem> = model.tray.iter().filter(|t| !t.shown()).collect();
    match &pop.mode {
        TrayMode::Overflow => {
            for (i, it) in passive.iter().enumerate() {
                let label = if !it.title.is_empty() { it.title.clone() } else { it.id.clone() };
                rows.push(Row { label, sep: false, enabled: true, checked: None, sub: false, icon: Some(i), act: Some(RowAct::Activate(it.key.clone())) });
            }
            if rows.is_empty() {
                rows.push(Row { label: "Nothing hidden".into(), sep: false, enabled: false, checked: None, sub: false, icon: None, act: None });
            }
        }
        TrayMode::Menu { key, items, path } => {
            if !path.is_empty() {
                rows.push(Row { label: "‹  Back".into(), sep: false, enabled: true, checked: None, sub: false, icon: None, act: Some(RowAct::Back) });
            }
            for m in current(items, path) {
                let act = if m.separator || !m.enabled { None } else if !m.children.is_empty() { Some(RowAct::Sub(m.id)) } else { Some(RowAct::Choose(key.clone(), m.id)) };
                rows.push(Row { label: m.label.clone(), sep: m.separator, enabled: m.enabled, checked: m.checked, sub: !m.children.is_empty(), icon: None, act });
            }
            if rows.is_empty() {
                rows.push(Row { label: "(empty menu)".into(), sep: false, enabled: false, checked: None, sub: false, icon: None, act: None });
            }
        }
    }
    let height: f32 = rows.iter().map(|r| if r.sep { SEP_H } else { ROW_H }).sum::<f32>() + 12.0;
    let cx = (pop.anchor_x - CARD_W / 2.0).clamp(8.0, (w - CARD_W - 8.0).max(8.0));
    let cy = 3.0;
    paint::fill_rounded(pix, cx * s, cy * s, CARD_W * s, height * s, 12.0 * s, bg);
    paint::stroke_rounded(pix, (cx + 0.5) * s, (cy + 0.5) * s, (CARD_W - 1.0) * s, (height - 1.0) * s, 12.0 * s, s, border);
    let mut y = cy + 6.0;
    let mut hits = vec![];
    for r in &rows {
        if r.sep {
            paint::fill_rounded(pix, (cx + 12.0) * s, (y + SEP_H / 2.0) * s, (CARD_W - 24.0) * s, s.max(1.0), 0.5, (255, 255, 255, 30));
            y += SEP_H;
            continue;
        }
        let rect = (cx + 4.0, y, CARD_W - 8.0, ROW_H);
        let hot = r.act.is_some() && pointer.0 >= rect.0 && pointer.0 < rect.0 + rect.2 && pointer.1 >= rect.1 && pointer.1 < rect.1 + rect.3;
        if hot {
            paint::fill_rounded(pix, rect.0 * s, rect.1 * s, rect.2 * s, rect.3 * s, 8.0 * s, (255, 255, 255, 26));
        }
        let col = if r.enabled { text } else { dim };
        let mut tx = cx + 16.0;
        if let Some(i) = r.icon {
            if let Some(ic) = passive.get(i).and_then(|it| crate::tray::icon_pixmap(it, icons, (18.0 * s) as u32)) {
                paint::draw_pixmap(pix, &ic, ((cx + 14.0) * s) as i32, ((y + 6.0) * s) as i32);
            }
            tx = cx + 42.0;
        }
        if let Some(c) = r.checked {
            if c {
                // a small filled dot: the fonts have no tick mark
                paint::fill_circle(pix, (cx + 20.0) * s, (y + ROW_H / 2.0) * s, 3.5 * s, col);
            }
            tx = cx + 34.0;
        }
        let shown = crate::fylgja::fit(&fonts.regular, 13.0 * s, &r.label, (CARD_W - (tx - cx) - 28.0) * s);
        paint::draw_text(pix, &fonts.regular, 13.0 * s, tx * s, (y + ROW_H / 2.0) * s, col, &shown);
        if r.sub {
            paint::draw_text(pix, &fonts.bold, 14.0 * s, (cx + CARD_W - 24.0) * s, (y + ROW_H / 2.0) * s, dim, "›");
        }
        if let Some(a) = &r.act {
            hits.push((rect, a.clone()));
        }
        y += ROW_H;
    }
    ((cx, cy, CARD_W, height), hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: i32, label: &str, kids: Vec<MenuItem>) -> MenuItem {
        MenuItem { id, label: label.into(), enabled: true, separator: false, checked: None, children: kids }
    }

    #[test]
    fn navigating_into_submenus() {
        let items = vec![m(1, "Play", vec![]), m(2, "More", vec![m(3, "Inner", vec![m(4, "Deep", vec![])])])];
        assert_eq!(current(&items, &[]).len(), 2);
        assert_eq!(current(&items, &[2])[0].label, "Inner");
        assert_eq!(current(&items, &[2, 3])[0].label, "Deep");
        assert_eq!(current(&items, &[99]).len(), 2, "an unknown id stays where it is");
    }
}
