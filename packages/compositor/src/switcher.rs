//! The window switcher (Alt+Tab), and the profile question asked when an empty workspace opens.
//!
//! The switcher: while Alt is held, every window is a card in rows that wrap and stay centred on the screen with the keyboard
//! (on a panel of Yggdrasil's dark style): a title bar with the app's icon and the window's title over a small live picture of
//! the window at its own shape, most recently used first, the chosen card outlined in the accent. Windows that share a screen
//! (snapped to its two halves, or tiled) are one group card, their pictures where they are on the screen. The pictures are the
//! windows' own (their textures, scaled while drawing: nothing is copied), redrawn only where a window changed; nothing of the
//! switcher is drawn, kept or timed while it is closed. The cards' frames, titles and icons are small pictures drawn once per
//! opening (again only for a card whose title or choice changed).
//!
//! The layout (`grid`) and the moves in it are plain functions of the cards' shapes and the screen's size, for the deck of
//! Win+Tab's flat fallback to use too.

use std::ops::Range;

use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::{MemoryRenderBuffer, MemoryRenderBufferRenderElement};
use smithay::backend::renderer::element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement};
use smithay::backend::renderer::element::{AsRenderElements, Id, Kind};
use smithay::desktop::Window;
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle, Scale, Size, Transform};
use tiny_skia::{Pixmap, PixmapPaint};

use crate::draw::{draw_text, fill_circle, fill_rounded, text_width, Fonts};
use crate::look::RectF;
use crate::state::Draw;

// ------------------------------------------------------------------ the layout (logical pixels)

/// The panel's padding around the cards, and the room kept free between the panel and the screen's edges.
pub const PAD: i32 = 16;
pub const MARGIN: i32 = 40;
/// Between two cards, across and down.
pub const GAP: i32 = 14;
/// A card's padding around its title bar and picture, and the title bar's height.
pub const INSET: i32 = 6;
pub const TITLE_H: i32 = 28;
/// The pictures' height: as tall as this when the rows fit, never lower than the least (then only the rows around the chosen
/// card are shown).
pub const PICTURE_H: i32 = 160;
pub const PICTURE_MIN_H: i32 = 64;
/// A card is never narrower than this (room for its title), and a picture keeps its shape between these (width over height).
pub const CARD_MIN_W: i32 = 150;
const MIN_ASPECT: f64 = 0.6;
const MAX_ASPECT: f64 = 2.6;
/// Corners: the panel's, a card's, a picture's; the chosen card's outline, and the icon's size.
const PANEL_RADIUS: i32 = 18;
const CARD_RADIUS: f32 = 10.0;
const PICTURE_RADIUS: f64 = 6.0;
const OUTLINE: f32 = 3.0;
const ICON: f32 = 20.0;
/// Yggdrasil's dark style, as the profile question and the window controls are drawn.
const PANEL: [u8; 4] = [18, 18, 26, 255];
const CARD: (u8, u8, u8, u8) = (34, 34, 46, 255);
const CARD_CHOSEN: (u8, u8, u8, u8) = (50, 50, 68, 255);
const WELL: (u8, u8, u8, u8) = (24, 24, 32, 255);
const TEXT: (u8, u8, u8, u8) = (240, 240, 248, 255);

/// Where the cards go on one screen (each rectangle relative to the screen's corner).
#[derive(Clone, Debug, PartialEq)]
pub struct Grid {
    pub panel: Rectangle<i32, Logical>,
    /// Every card, in the cards' order (those of rows not shown lie outside the panel and are not drawn).
    pub cards: Vec<Rectangle<i32, Logical>>,
    /// The cards of each row.
    pub rows: Vec<Range<usize>>,
    /// The rows shown.
    pub shown: Range<usize>,
}

/// The moves in the grid: the arrow keys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Move {
    Left,
    Right,
    Up,
    Down,
}

fn card_width(aspect: f64, h: i32, max_w: i32) -> i32 {
    let a = if aspect.is_finite() && aspect > 0.0 { aspect.clamp(MIN_ASPECT, MAX_ASPECT) } else { 1.6 };
    ((h as f64 * a).round() as i32 + 2 * INSET).max(CARD_MIN_W).min(max_w)
}

/// The cards in rows of at most `max_w`: each card's width, and the rows.
fn wrap(aspects: &[f64], h: i32, max_w: i32) -> (Vec<i32>, Vec<Range<usize>>) {
    let widths: Vec<i32> = aspects.iter().map(|a| card_width(*a, h, max_w)).collect();
    let mut rows = vec![];
    let (mut start, mut used) = (0, 0);
    for (i, w) in widths.iter().enumerate() {
        if i > start && used + GAP + w > max_w {
            rows.push(start..i);
            start = i;
            used = 0;
        }
        used += if i > start { GAP + w } else { *w };
    }
    if start < widths.len() {
        rows.push(start..widths.len());
    }
    (widths, rows)
}

/// The grid for cards of these shapes (width over height of each card's picture) on a screen of `screen`, `selected` the chosen
/// one: the pictures as tall as `PICTURE_H` if every row fits, lower until they do, and at `PICTURE_MIN_H` only the rows around
/// the chosen card.
pub fn grid(aspects: &[f64], screen: Size<i32, Logical>, selected: usize) -> Grid {
    let max_w = (screen.w - 2 * MARGIN - 2 * PAD).max(CARD_MIN_W);
    let max_h = (screen.h - 2 * MARGIN - 2 * PAD).max(1);
    let mut h = PICTURE_H;
    let (widths, rows) = loop {
        let (widths, rows) = wrap(aspects, h, max_w);
        let card_h = h + TITLE_H + 2 * INSET;
        if rows.len() as i32 * (card_h + GAP) - GAP <= max_h || h <= PICTURE_MIN_H {
            break (widths, rows);
        }
        h = (h - 8).max(PICTURE_MIN_H);
    };
    let card_h = h + TITLE_H + 2 * INSET;
    let fit = (((max_h + GAP) / (card_h + GAP)).max(1) as usize).min(rows.len().max(1));
    let row_of = rows.iter().position(|r| r.contains(&selected)).unwrap_or(0);
    let first = row_of.saturating_sub(fit / 2).min(rows.len().saturating_sub(fit));
    let shown = first..(first + fit).min(rows.len());
    let row_w = |r: &Range<usize>| widths[r.clone()].iter().sum::<i32>() + GAP * (r.len() as i32 - 1).max(0);
    let inner_w = rows[shown.clone()].iter().map(row_w).max().unwrap_or(CARD_MIN_W);
    let inner_h = shown.len() as i32 * (card_h + GAP) - GAP;
    let panel = Rectangle::new(
        ((screen.w - inner_w - 2 * PAD) / 2, (screen.h - inner_h - 2 * PAD) / 2).into(),
        (inner_w + 2 * PAD, inner_h + 2 * PAD).into(),
    );
    let mut cards = vec![Rectangle::default(); aspects.len()];
    for (r, range) in rows.iter().enumerate() {
        let mut x = panel.loc.x + PAD + (inner_w - row_w(range)) / 2;
        let y = panel.loc.y + PAD + (r as i32 - shown.start as i32) * (card_h + GAP);
        for i in range.clone() {
            cards[i] = Rectangle::new((x, y).into(), (widths[i], card_h).into());
            x += widths[i] + GAP;
        }
    }
    Grid { panel, cards, rows, shown }
}

impl Grid {
    pub fn row_of(&self, i: usize) -> usize {
        self.rows.iter().position(|r| r.contains(&i)).unwrap_or(0)
    }

    pub fn shows(&self, i: usize) -> bool {
        i < self.cards.len() && self.shown.contains(&self.row_of(i))
    }

    /// The room for a card's picture (below its title bar).
    pub fn picture(&self, i: usize) -> Rectangle<i32, Logical> {
        let c = self.cards[i];
        Rectangle::new((c.loc.x + INSET, c.loc.y + INSET + TITLE_H).into(), (c.size.w - 2 * INSET, c.size.h - 2 * INSET - TITLE_H).into())
    }

    /// The card shown under a point (relative to the screen's corner).
    pub fn hit(&self, p: Point<f64, Logical>) -> Option<usize> {
        (0..self.cards.len()).find(|i| self.shows(*i) && self.cards[*i].to_f64().contains(p))
    }

    /// Where an arrow key goes from card `i`: Left and Right the card before and after (round the ends), Up and Down the card
    /// of the row above or below nearest in the middle (from the top row up is the bottom row).
    pub fn step(&self, i: usize, m: Move) -> usize {
        let n = self.cards.len();
        if n == 0 {
            return 0;
        }
        match m {
            Move::Left => (i + n - 1) % n,
            Move::Right => (i + 1) % n,
            Move::Up | Move::Down => {
                let rows = self.rows.len();
                let r = self.row_of(i);
                let to = if m == Move::Up { (r + rows - 1) % rows } else { (r + 1) % rows };
                let mid = |c: &Rectangle<i32, Logical>| c.loc.x * 2 + c.size.w;
                let here = mid(&self.cards[i]);
                self.rows[to].clone().min_by_key(|j| (mid(&self.cards[*j]) - here).abs()).unwrap_or(i)
            }
        }
    }
}

/// A picture of `size` drawn as large as fits in `room` (never larger than it is), in the middle: where it goes and its scale.
pub fn fit(room: Rectangle<i32, Logical>, size: Size<i32, Logical>) -> (RectF, f64) {
    if size.w <= 0 || size.h <= 0 {
        return (RectF::of(Rectangle::new(room.loc, (0, 0).into())), 0.0);
    }
    let k = (room.size.w as f64 / size.w as f64).min(room.size.h as f64 / size.h as f64).min(1.0);
    let (w, h) = (size.w as f64 * k, size.h as f64 * k);
    let x = room.loc.x as f64 + (room.size.w as f64 - w) / 2.0;
    let y = room.loc.y as f64 + (room.size.h as f64 - h) / 2.0;
    (RectF { x, y, w, h }, k)
}

// ------------------------------------------------------------------ the switcher while it is open

/// A card as the switcher shows it: its windows (one, or a group's, most recently used first) with their places on the desktop
/// (only their places relative to each other count), and its title and app (a group's: "Group | TITLE and N other windows",
/// the app of its first window).
pub struct Card {
    pub windows: Vec<(Window, Point<i32, Logical>)>,
    pub title: String,
    pub app_id: String,
    /// the Steam game its first window's program belongs to (SteamAppId), for its icon
    pub steam: Option<String>,
}

impl Card {
    /// The rectangle the card's windows cover (on the desktop).
    pub fn bounds(&self) -> Rectangle<i32, Logical> {
        let mut it = self.windows.iter().map(|(w, at)| Rectangle::new(*at, w.geometry().size));
        let first = it.next().unwrap_or_default();
        it.fold(first, |a, b| a.merge(b))
    }
}

/// What a card's frame was drawn for.
#[derive(Clone, PartialEq)]
struct ArtKey {
    title: String,
    app_id: String,
    chosen: bool,
    size: Size<i32, Logical>,
    /// the screen's scale (its bits: a key compared exactly)
    scale: u64,
}

/// The switcher while it is open: the cards (each the windows it brings forward) and the chosen one, the screen it shows on,
/// and the small pictures it has drawn (dropped when it closes).
pub struct Switcher {
    pub entries: Vec<Vec<WlSurface>>,
    pub index: usize,
    pub output: Output,
    accent: (u8, u8, u8),
    art: Vec<(WlSurface, ArtKey, MemoryRenderBuffer)>,
    icons: Vec<((String, u32), Option<Pixmap>)>,
    corner: Option<(i32, MemoryRenderBuffer)>,
    strips: [Id; 3],
}

impl Switcher {
    pub fn new(entries: Vec<Vec<WlSurface>>, index: usize, output: Output, accent: (u8, u8, u8)) -> Switcher {
        Switcher { entries, index, output, accent, art: vec![], icons: vec![], corner: None, strips: [Id::new(), Id::new(), Id::new()] }
    }

    /// A window has gone: off its card, and a card left empty goes.
    pub fn forget(&mut self, s: &WlSurface) {
        for e in &mut self.entries {
            e.retain(|x| x != s);
        }
        self.entries.retain(|e| !e.is_empty());
        self.index = self.index.min(self.entries.len().saturating_sub(1));
        self.art.retain(|(x, _, _)| x != s);
    }

    fn icon(&mut self, app_id: &str, steam: Option<&str>, px: u32) -> Option<&Pixmap> {
        let key = (format!("{app_id}\0{}", steam.unwrap_or("")), px);
        let i = match self.icons.iter().position(|(k, _)| *k == key) {
            Some(i) => i,
            None => {
                self.icons.push((key, crate::icons::for_app(app_id, steam, px)));
                self.icons.len() - 1
            }
        };
        self.icons[i].1.as_ref()
    }
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

/// A pixmap as a picture the renderers take (premultiplied BGRA in memory).
fn to_buffer(pix: &Pixmap) -> MemoryRenderBuffer {
    let mut data = Vec::with_capacity(pix.data().len());
    for p in pix.data().as_chunks::<4>().0 {
        data.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
    }
    MemoryRenderBuffer::from_slice(&data, Fourcc::Argb8888, (pix.width() as i32, pix.height() as i32), 1, Transform::Normal, None)
}

/// The pixels of a card's frame of `size` at `scale`.
fn art_px(size: Size<i32, Logical>, scale: f64) -> (u32, u32) {
    (((size.w as f64 * scale).round() as u32).max(1), ((size.h as f64 * scale).round() as u32).max(1))
}

/// A card's frame, drawn as its key says (the card's size in logical pixels, the scale, the title and app, chosen or not): its background (the accent outline when chosen), the title
/// bar with the icon (the app's, else its first letter in an accent circle) and the title, and the well its picture sits in.
/// Without a font (none found) the text is left out.
fn card_art(fonts: Option<&Fonts>, key: &ArtKey, icon: Option<&Pixmap>, accent: (u8, u8, u8)) -> MemoryRenderBuffer {
    let (title, app_id, size, chosen) = (key.title.as_str(), key.app_id.as_str(), key.size, key.chosen);
    let scale = f64::from_bits(key.scale);
    let f = scale as f32;
    let (pw, ph) = art_px(size, scale);
    let mut pix = Pixmap::new(pw, ph).expect("card size");
    let (w, h) = (pw as f32, ph as f32);
    if chosen {
        fill_rounded(&mut pix, 0.0, 0.0, w, h, CARD_RADIUS * f, (accent.0, accent.1, accent.2, 255));
        let o = OUTLINE * f;
        fill_rounded(&mut pix, o, o, w - 2.0 * o, h - 2.0 * o, (CARD_RADIUS - OUTLINE) * f, CARD_CHOSEN);
    } else {
        fill_rounded(&mut pix, 0.0, 0.0, w, h, CARD_RADIUS * f, CARD);
    }
    let (inset, title_h) = (INSET as f32 * f, TITLE_H as f32 * f);
    fill_rounded(&mut pix, inset, inset + title_h, w - 2.0 * inset, h - 2.0 * inset - title_h, PICTURE_RADIUS as f32 * f, WELL);
    let side = ICON * f;
    let (ix, iy) = (inset + 3.0 * f, inset + (title_h - side) / 2.0);
    match icon {
        Some(p) => pix.draw_pixmap(
            ix.round() as i32,
            iy.round() as i32,
            p.as_ref(),
            &PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        ),
        None => {
            fill_circle(&mut pix, ix + side / 2.0, iy + side / 2.0, side / 2.0, (accent.0, accent.1, accent.2, 255));
            if let Some(fonts) = fonts {
                let letter = app_id
                    .chars()
                    .chain(title.chars())
                    .find(|c| c.is_alphanumeric())
                    .map(|c| c.to_uppercase().to_string())
                    .unwrap_or_else(|| "?".into());
                let size = 12.0 * f;
                let lx = ix + side / 2.0 - text_width(&fonts.bold, size, &letter) / 2.0;
                draw_text(&mut pix, &fonts.bold, size, lx, iy + side / 2.0, (255, 255, 255, 255), &letter);
            }
        }
    }
    if let Some(fonts) = fonts {
        let size = 13.0 * f;
        let tx = ix + side + 8.0 * f;
        draw_text(&mut pix, &fonts.regular, size, tx, inset + title_h / 2.0, TEXT, &clip(fonts, size, title, w - tx - inset));
    }
    to_buffer(&pix)
}

/// A group card's title: "Group | TITLE and N other window(s)".
pub fn group_title(first: &str, others: usize) -> String {
    let first = if first.is_empty() { "A window" } else { first };
    format!("Group | {first} and {others} other window{}", if others == 1 { "" } else { "s" })
}

/// A memory picture of `px` pixels drawn over `size` (logical) at a whole physical place: crisp at any scale.
fn picture_element<R>(
    renderer: &mut R,
    at: Point<i32, smithay::utils::Physical>,
    buf: &MemoryRenderBuffer,
    src: Rectangle<f64, Logical>,
    size: Size<i32, Logical>,
) -> Option<MemoryRenderBufferRenderElement<R>>
where
    R: Draw,
    R::TextureId: Send + Clone + 'static,
{
    MemoryRenderBufferRenderElement::from_buffer(renderer, at.to_f64(), buf, None, Some(src), Some(size), Kind::Unspecified).ok()
}

/// What the switcher draws on its screen this frame, front to back: each shown card's pictures, its frame, then the panel.
pub fn elements<R>(renderer: &mut R, sw: &mut Switcher, cards: &[Card], fonts: Option<&Fonts>) -> Vec<R::Elem>
where
    R: Draw,
    R::TextureId: Send + Clone + 'static,
{
    let mut out: Vec<R::Elem> = vec![];
    let output = sw.output.clone();
    let s = output.current_scale().fractional_scale();
    let screen = crate::state::logical_size(&output);
    let aspects: Vec<f64> = cards
        .iter()
        .map(|c| {
            let b = c.bounds();
            b.size.w as f64 / b.size.h.max(1) as f64
        })
        .collect();
    let g = grid(&aspects, screen, sw.index);
    let phys = |r: RectF| r.physical((0, 0).into(), s);
    let firsts: Vec<WlSurface> = sw.entries.iter().filter_map(|e| e.first().cloned()).collect();
    sw.art.retain(|(x, _, _)| firsts.contains(x));
    for (i, card) in cards.iter().enumerate() {
        if !g.shows(i) {
            continue;
        }
        // the pictures: each window's own, scaled about its corner and clipped to its rounded rectangle
        let bounds = card.bounds();
        let (room, k) = fit(g.picture(i), bounds.size);
        for (w, at) in &card.windows {
            let geo = w.geometry();
            if geo.size.w <= 0 || geo.size.h <= 0 || k <= 0.0 {
                continue;
            }
            let d = RectF {
                x: room.x + (at.x - bounds.loc.x) as f64 * k,
                y: room.y + (at.y - bounds.loc.y) as f64 * k,
                w: geo.size.w as f64 * k,
                h: geo.size.h as f64 * k,
            };
            let rect = phys(d);
            let corner = rect.loc;
            let draw_at = corner - Point::from(((geo.loc.x as f64 * s).round() as i32, (geo.loc.y as f64 * s).round() as i32));
            let els: Vec<WaylandSurfaceRenderElement<R>> = match w.toplevel() {
                Some(t) => render_elements_from_surface_tree(renderer, t.wl_surface(), draw_at, s, 1.0, Kind::Unspecified),
                None => w.render_elements(renderer, draw_at, Scale::from(s), 1.0),
            };
            let stretch = Scale::from((d.w / geo.size.w as f64, d.h / geo.size.h as f64));
            let clip = (rect, (PICTURE_RADIUS * s).round() as i32);
            out.extend(els.into_iter().map(|e| R::Elem::from(crate::look::Clipped::new(e, Some(clip), corner, stretch))));
        }
        // the frame, title and icon
        let size = g.cards[i].size;
        let key = ArtKey { title: card.title.clone(), app_id: card.app_id.clone(), chosen: i == sw.index, size, scale: s.to_bits() };
        let Some(first) = sw.entries.get(i).and_then(|e| e.first()).cloned() else { continue };
        let fresh = sw.art.iter().position(|(x, k, _)| *x == first && *k == key);
        let at = match fresh {
            Some(j) => j,
            None => {
                let px = (ICON as f64 * s).round() as u32;
                let icon = sw.icon(&card.app_id, card.steam.as_deref(), px).cloned();
                let buf = card_art(fonts, &key, icon.as_ref(), sw.accent);
                sw.art.retain(|(x, _, _)| *x != first);
                sw.art.push((first, key, buf));
                sw.art.len() - 1
            }
        };
        let buf = &sw.art[at].2;
        let px = art_px(size, s);
        let src = Rectangle::from_size((px.0 as f64, px.1 as f64).into());
        let loc = phys(RectF::of(g.cards[i])).loc;
        out.extend(picture_element(renderer, loc, buf, src, size).map(R::Elem::from));
    }
    // the panel: four rounded corners and three solid bands (opaque: nothing behind it is drawn)
    let p = phys(RectF::of(g.panel));
    let r = ((PANEL_RADIUS as f64 * s).round() as i32).min(p.size.w / 2).min(p.size.h / 2).max(1);
    if sw.corner.as_ref().is_none_or(|(cr, _)| *cr != r) {
        let mut pix = Pixmap::new(2 * r as u32, 2 * r as u32).expect("corner size");
        fill_rounded(&mut pix, 0.0, 0.0, 2.0 * r as f32, 2.0 * r as f32, r as f32, (PANEL[0], PANEL[1], PANEL[2], PANEL[3]));
        sw.corner = Some((r, to_buffer(&pix)));
    }
    if let Some((_, buf)) = &sw.corner {
        let size: Size<i32, Logical> = ((r as f64 / s).round() as i32, (r as f64 / s).round() as i32).into();
        let (x0, y0, x1, y1) = (p.loc.x, p.loc.y, p.loc.x + p.size.w - r, p.loc.y + p.size.h - r);
        for (x, y, sx, sy) in [(x0, y0, 0, 0), (x1, y0, r, 0), (x1, y1, r, r), (x0, y1, 0, r)] {
            let src = Rectangle::new((sx as f64, sy as f64).into(), (r as f64, r as f64).into());
            out.extend(picture_element(renderer, (x, y).into(), buf, src, size).map(R::Elem::from));
        }
    }
    let band = |x: i32, y: i32, w: i32, h: i32| Rectangle::<i32, smithay::utils::Physical>::new((x, y).into(), (w, h).into());
    for (id, geo) in sw.strips.iter().zip([
        band(p.loc.x, p.loc.y + r, p.size.w, p.size.h - 2 * r),
        band(p.loc.x + r, p.loc.y, p.size.w - 2 * r, r),
        band(p.loc.x + r, p.loc.y + p.size.h - r, p.size.w - 2 * r, r),
    ]) {
        if geo.size.w > 0 && geo.size.h > 0 {
            out.push(R::Elem::from(crate::look::Strip::new(id.clone(), geo, PANEL)));
        }
    }
    out
}

/// The grid the switcher shows now for these cards (for the arrow keys and clicks).
pub fn grid_of(sw: &Switcher, cards: &[Card]) -> Grid {
    let aspects: Vec<f64> = cards
        .iter()
        .map(|c| {
            let b = c.bounds();
            b.size.w as f64 / b.size.h.max(1) as f64
        })
        .collect();
    grid(&aspects, crate::state::logical_size(&sw.output), sw.index)
}

// ------------------------------------------------------------------ the profile question

const CHOOSER_PAD: i32 = 18;
const CHOOSER_GAP: i32 = 12;
pub const CHOOSER_W: i32 = 270;
pub const CHOOSER_H: i32 = 130;

pub fn chooser_size() -> (i32, i32) {
    (CHOOSER_PAD * 2 + 2 * CHOOSER_W + CHOOSER_GAP, CHOOSER_PAD * 2 + CHOOSER_H + 34)
}

/// The question shown when an empty workspace is opened and the default profile is "ask": two cards, one chosen.
pub fn make_chooser(fonts: &Fonts, workspace: usize, selected: usize, accent: (u8, u8, u8)) -> MemoryRenderBuffer {
    let (w, h) = chooser_size();
    let mut pix = Pixmap::new(w as u32, h as u32).expect("chooser size");
    fill_rounded(&mut pix, 0.0, 0.0, w as f32, h as f32, 18.0, (18, 18, 26, 240));
    fill_rounded(&mut pix, 0.0, 0.0, w as f32, 1.0, 0.0, (255, 255, 255, 20));
    draw_text(
        &mut pix,
        &fonts.bold,
        15.0,
        CHOOSER_PAD as f32,
        24.0,
        (240, 240, 248, 255),
        &format!("Workspace {}: how should windows behave here?", workspace + 1),
    );
    let cards = [("Standard", "Windows float. You move and size them."), ("Tiling", "Windows tile on their own, side by side.")];
    for (i, (name, about)) in cards.iter().enumerate() {
        let x = (CHOOSER_PAD + i as i32 * (CHOOSER_W + CHOOSER_GAP)) as f32;
        let y = (CHOOSER_PAD + 22) as f32;
        let chosen = i == selected;
        fill_rounded(
            &mut pix,
            x,
            y,
            CHOOSER_W as f32,
            CHOOSER_H as f32,
            12.0,
            if chosen { (accent.0, accent.1, accent.2, 255) } else { (40, 40, 54, 255) },
        );
        draw_text(&mut pix, &fonts.bold, 17.0, x + 16.0, y + 30.0, (255, 255, 255, 255), name);
        draw_text(
            &mut pix,
            &fonts.regular,
            12.0,
            x + 16.0,
            y + 58.0,
            (255, 255, 255, if chosen { 230 } else { 150 }),
            &clip(fonts, 12.0, about, CHOOSER_W as f32 - 32.0),
        );
        // a little picture of the layout
        let (px, py) = (x + 16.0, y + 76.0);
        if i == 0 {
            fill_rounded(&mut pix, px, py + 4.0, 60.0, 36.0, 4.0, (255, 255, 255, 120));
            fill_rounded(&mut pix, px + 34.0, py, 60.0, 36.0, 4.0, (255, 255, 255, 190));
        } else {
            fill_rounded(&mut pix, px, py, 44.0, 40.0, 4.0, (255, 255, 255, 190));
            fill_rounded(&mut pix, px + 48.0, py, 44.0, 18.0, 4.0, (255, 255, 255, 190));
            fill_rounded(&mut pix, px + 48.0, py + 22.0, 44.0, 18.0, 4.0, (255, 255, 255, 190));
        }
    }
    draw_text(
        &mut pix,
        &fonts.regular,
        11.0,
        CHOOSER_PAD as f32,
        h as f32 - 12.0,
        (160, 160, 176, 255),
        "Left / Right to choose, Enter to confirm, Escape for standard",
    );
    to_buffer(&pix)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen() -> Size<i32, Logical> {
        (1920, 1080).into()
    }

    #[test]
    fn few_cards_are_one_centred_row_at_full_height() {
        let g = grid(&[1.6, 1.6, 0.8], screen(), 1);
        assert_eq!(g.rows, vec![0..3]);
        assert_eq!(g.shown, 0..1);
        let card_h = PICTURE_H + TITLE_H + 2 * INSET;
        assert!(g.cards.iter().all(|c| c.size.h == card_h));
        // each card at its window's shape: wide windows wide, narrow ones narrow
        assert_eq!(g.cards[0].size.w, (PICTURE_H as f64 * 1.6).round() as i32 + 2 * INSET);
        assert!(g.cards[2].size.w < g.cards[0].size.w);
        // centred on the screen, the cards inside the panel
        assert_eq!(g.panel.loc.x * 2 + g.panel.size.w, 1920);
        assert!((g.panel.loc.y * 2 + g.panel.size.h - 1080).abs() <= 1);
        assert!(g.cards.iter().all(|c| g.panel.contains_rect(*c)));
        assert_eq!(g.cards[1].loc.x, g.cards[0].loc.x + g.cards[0].size.w + GAP);
    }

    #[test]
    fn many_cards_wrap_into_rows_and_shrink_to_fit() {
        let g = grid(&[1.6; 12], screen(), 0);
        assert!(g.rows.len() > 1, "{:?}", g.rows);
        assert_eq!(g.rows.iter().map(|r| r.len()).sum::<usize>(), 12);
        let max_w = 1920 - 2 * MARGIN - 2 * PAD;
        for r in &g.rows {
            let (a, b) = (g.cards[r.start], g.cards[r.end - 1]);
            assert!(b.loc.x + b.size.w - a.loc.x <= max_w);
            // each row centred in the panel
            let (left, right) = (a.loc.x - g.panel.loc.x, g.panel.loc.x + g.panel.size.w - (b.loc.x + b.size.w));
            assert!((left - right).abs() <= 1, "{left} {right}");
        }
        assert_eq!(g.shown, 0..g.rows.len(), "all rows fit");
        assert!(g.panel.size.h <= 1080 - 2 * MARGIN);
    }

    #[test]
    fn too_many_cards_show_the_rows_around_the_choice() {
        let g = grid(&[1.0; 200], (800, 600).into(), 150);
        assert!(g.shown.len() < g.rows.len());
        assert!(g.shows(150) && !g.shows(0));
        assert!(g.cards[150].size.h == PICTURE_MIN_H + TITLE_H + 2 * INSET);
        assert!(g.panel.contains_rect(g.cards[150]));
        assert_eq!(g.hit(g.cards[0].loc.to_f64()), None, "a card out of sight is not clicked");
    }

    #[test]
    fn the_arrows_move_in_the_grid() {
        let g = grid(&[1.6; 7], (1000, 900).into(), 0);
        assert!(g.rows.len() >= 2);
        let first_row = g.rows[0].clone();
        assert_eq!(g.step(0, Move::Left), 6);
        assert_eq!(g.step(6, Move::Right), 0);
        assert_eq!(g.step(0, Move::Right), 1);
        let below = g.step(0, Move::Down);
        assert_eq!(g.row_of(below), 1);
        assert_eq!(g.step(below, Move::Up), 0);
        assert!(first_row.contains(&g.step(below, Move::Up)));
        // from the top row up is the bottom row
        assert_eq!(g.row_of(g.step(0, Move::Up)), g.rows.len() - 1);
    }

    #[test]
    fn a_click_finds_its_card() {
        let g = grid(&[1.6, 1.0], screen(), 0);
        let c = g.cards[1];
        assert_eq!(g.hit(Point::from(((c.loc.x + 5) as f64, (c.loc.y + 5) as f64))), Some(1));
        assert_eq!(g.hit(g.panel.loc.to_f64()), None);
    }

    #[test]
    fn a_picture_fits_its_room_in_the_middle_and_is_never_enlarged() {
        let room = Rectangle::new((10, 20).into(), (200, 100).into());
        let (r, k) = fit(room, (800, 800).into());
        assert_eq!(k, 0.125);
        assert_eq!(r, RectF { x: 60.0, y: 20.0, w: 100.0, h: 100.0 });
        let (r, k) = fit(room, (50, 20).into());
        assert_eq!((k, r.w, r.h), (1.0, 50.0, 20.0));
    }

    #[test]
    fn a_group_card_says_how_many_windows_it_has() {
        assert_eq!(group_title("Calculator", 1), "Group | Calculator and 1 other window");
        assert_eq!(group_title("", 2), "Group | A window and 2 other windows");
    }
}
