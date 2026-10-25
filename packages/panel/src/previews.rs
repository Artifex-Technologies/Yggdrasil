//! The dock's window list, Windows-taskbar style: resting on a dock app shows a card above it with one tile per window: a header
//! (the app's icon, what the window shows, and a cross that appears only while the pointer is on that tile) and a picture of the
//! window under it. Clicking a tile brings the window forward, the cross (or a middle click) closes it.
//!
//! What a tile is called comes from the window's title, by kind of app: a browser's page title, the folder VS Code has open, the
//! file an editor has open, the folder a file manager shows, a terminal's title as it is, and otherwise the title without the
//! app's name; the app's name when nothing is left.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use resvg::tiny_skia::Pixmap;

use crate::paint::{self, Fonts, Icons};
use crate::shell::Model;
use crate::trayui::{Rect, RowAct};

const BROWSERS: [(&str, &[&str]); 11] = [
    ("vivaldi", &["Vivaldi"]),
    ("firefox", &["Mozilla Firefox", "Firefox"]),
    ("librewolf", &["LibreWolf"]),
    ("brave", &["Brave"]),
    ("chromium", &["Chromium"]),
    ("google-chrome", &["Google Chrome"]),
    ("opera", &["Opera", "Opera GX"]),
    ("mullvad", &["Mullvad Browser"]),
    ("waterfox", &["Waterfox"]),
    ("torbrowser", &["Tor Browser"]),
    ("microsoft-edge", &["Microsoft Edge"]),
];
const CODE: [&str; 4] = ["vscode", "code", "codium", "code-oss"];
const FILE_MANAGERS: [&str; 6] = ["minos", "nautilus", "dolphin", "thunar", "nemo", "pcmanfm"];
const EDITORS: [&str; 8] = ["gedit", "texteditor", "kate", "kwrite", "zed", "mousepad", "xed", "gnome-text-editor"];
const TERMINALS: [&str; 8] = ["ghostty", "foot", "kitty", "alacritty", "wezterm", "konsole", "terminal", "xterm"];

/// " - " or " — " (or " – "): how apps join the parts of a title.
fn parts(title: &str) -> Vec<&str> {
    let mut out = vec![title];
    for sep in [" — ", " – ", " - "] {
        out = out.into_iter().flat_map(|p| p.split(sep)).collect();
    }
    out.into_iter().map(str::trim).filter(|p| !p.is_empty()).collect()
}

fn has(class: &str, names: &[&str]) -> bool {
    names.iter().any(|n| class == *n || class.ends_with(&format!(".{n}")) || class.contains(n))
}

/// The last part of a path ("~/Documents/Labyrinth_OS" -> "Labyrinth_OS"); anything else as it is.
fn base(s: &str) -> &str {
    let t = s.trim_end_matches('/');
    if t.contains('/') {
        t.rsplit('/').next().unwrap_or(t)
    } else {
        t
    }
}

/// `title` without a trailing " - <suffix>" (any of them, any separator).
fn strip_suffix<'a>(title: &'a str, suffixes: &[&str]) -> &'a str {
    for sep in [" — ", " – ", " - "] {
        for s in suffixes {
            if let Some(t) = title.strip_suffix(&format!("{sep}{s}")) {
                return t.trim();
            }
        }
    }
    title.trim()
}

/// What a window's tile is called. `class` is the window's app id (as the compositor gives it), `app` the app's name.
pub fn label(class: &str, app: &str, title: &str) -> String {
    let c = class.to_lowercase();
    let title = title.trim();
    let named = |s: &str| if s.trim().is_empty() { app.to_string() } else { s.trim().to_string() };
    if title.is_empty() {
        return app.to_string();
    }
    if let Some((_, names)) = BROWSERS.iter().find(|(k, _)| c.contains(k)) {
        return named(strip_suffix(title, names));
    }
    if has(&c, &CODE) && !has(&c, &TERMINALS) {
        // "file — folder — Visual Studio Code", "folder — Visual Studio Code", "● file - folder - VSCodium"
        let t = strip_suffix(title, &["Visual Studio Code", "VSCodium", "Code - OSS", "Code"]);
        if t == title && ["Visual Studio Code", "VSCodium", "Code - OSS"].contains(&title) {
            return app.to_string();
        }
        let p = parts(t.trim_start_matches(['●', '*', ' ']));
        return named(match p.len() {
            0 => "",
            1 => p[0],
            n => p[n - 1],
        });
    }
    if has(&c, &FILE_MANAGERS) {
        let t = strip_suffix(title, &[app, "Minos", "Files", "Dolphin", "Thunar", "Nemo", "PCManFM"]);
        return named(base(t));
    }
    if has(&c, &EDITORS) {
        // "*notes.txt (~/Documents) - gedit": the file's name
        let first = parts(title).first().copied().unwrap_or("");
        let first = first.trim_start_matches(['●', '*', ' ']);
        let first = first.split(" (").next().unwrap_or(first);
        return named(base(first));
    }
    if has(&c, &TERMINALS) {
        return title.to_string();
    }
    named(strip_suffix(title, &[app]))
}

/// Which opening of the card the picture-taking belongs to: closing the card (or opening another) ends the round, and its thread stops.
static ROUND: AtomicU64 = AtomicU64::new(0);

/// The card closed: stop taking pictures.
pub fn end_round() {
    ROUND.fetch_add(1, Ordering::Relaxed);
}

/// Takes pictures of these windows (keys are the panel's window ids) through the standard capture protocols, at once and then
/// about once a second while this card stays open; each one goes to `send`. A window that gives no picture is not asked again
/// in this round (a minimized one keeps its icon). Nothing runs once the card has closed.
pub fn spawn_pictures(wants: Vec<crate::capture::Want>, box_w: u32, box_h: u32, send: impl Fn(String, Pixmap) + Send + 'static) {
    if wants.is_empty() {
        return;
    }
    let round = ROUND.fetch_add(1, Ordering::Relaxed) + 1;
    std::thread::spawn(move || {
        let mut wants = wants;
        while ROUND.load(Ordering::Relaxed) == round && !wants.is_empty() {
            let mut got = std::collections::HashSet::new();
            let ok = crate::capture::capture(&wants, box_w, box_h, |k, p| {
                got.insert(k.clone());
                if ROUND.load(Ordering::Relaxed) == round {
                    send(k, p);
                }
            });
            if ok.is_err() {
                return; // the compositor has no window capture: the cards keep their icons
            }
            wants.retain(|w| got.contains(&w.key));
            std::thread::sleep(Duration::from_millis(1000));
        }
    });
}

const TILE_W: f32 = 236.0;
const THUMB_W: f32 = 220.0;
const THUMB_H: f32 = 138.0;
const HEAD_H: f32 = 30.0;
const PAD: f32 = 8.0;
const TILE_H: f32 = HEAD_H + THUMB_H + PAD;
/// The most tiles in a row; more windows wrap to another row.
const PER_ROW: usize = 6;

/// The picture box (logical px), for asking for pictures that fit.
pub fn thumb_box() -> (f32, f32) {
    (THUMB_W, THUMB_H)
}

/// Where the card and each tile go: the card centred over `anchor_x` (kept on screen), at the bottom of a `w` x `h` surface;
/// tiles in rows of up to `PER_ROW`. Pure, for tests.
pub fn layout(w: f32, h: f32, anchor_x: f32, n: usize) -> (Rect, Vec<Rect>) {
    if n == 0 {
        return ((0.0, 0.0, 0.0, 0.0), vec![]);
    }
    let fit = (((w - 16.0 - 2.0 * PAD) / TILE_W).floor() as usize).max(1);
    let per_row = PER_ROW.min(fit).min(n);
    let rows = n.div_ceil(per_row);
    let cw = per_row as f32 * TILE_W + 2.0 * PAD;
    let ch = rows as f32 * (TILE_H + PAD) + PAD;
    let cx = (anchor_x - cw / 2.0).clamp(8.0, (w - cw - 8.0).max(8.0));
    let cy = (h - ch - 4.0).max(3.0);
    let tiles = (0..n).map(|k| (cx + PAD + (k % per_row) as f32 * TILE_W, cy + PAD + (k / per_row) as f32 * (TILE_H + PAD), TILE_W - 6.0, TILE_H)).collect();
    ((cx, cy, cw, ch), tiles)
}

/// The cross in a tile's header (logical px).
pub fn cross_rect(tile: Rect) -> Rect {
    (tile.0 + tile.2 - 30.0, tile.1 + 3.0, 26.0, 24.0)
}

/// Draws the card of `app_id`'s windows and returns it with what each part does when clicked.
#[allow(clippy::too_many_arguments)]
pub fn paint(pix: &mut Pixmap, s: f32, anchor_x: f32, app_id: &str, model: &Model, thumbs: &std::collections::HashMap<String, Pixmap>, pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons) -> (Rect, Vec<(Rect, RowAct)>) {
    let (w, h) = (pix.width() as f32 / s, pix.height() as f32 / s);
    let dark = model.dark;
    let (bg, border, text, dim, hover, well) = if dark {
        ((24, 24, 30, 246), (255, 255, 255, 34), (240, 240, 246, 255), (150, 150, 166, 255), (255, 255, 255, 20), (255, 255, 255, 12))
    } else {
        ((250, 250, 252, 248), (0, 0, 0, 30), (24, 24, 32, 255), (110, 110, 126, 255), (0, 0, 0, 14), (0, 0, 0, 10))
    };
    let items = crate::shell::dock_items(model);
    let Some(item) = items.iter().find(|i| i.app_id == app_id) else { return ((0.0, 0.0, 0.0, 0.0), vec![]) };
    let wins = item.wins.clone();
    let (card, tiles) = layout(w, h, anchor_x, wins.len());
    if tiles.is_empty() {
        return (card, vec![]);
    }
    paint::fill_rounded(pix, card.0 * s, card.1 * s, card.2 * s, card.3 * s, 12.0 * s, bg);
    paint::stroke_rounded(pix, (card.0 + 0.5) * s, (card.1 + 0.5) * s, (card.2 - 1.0) * s, (card.3 - 1.0) * s, 12.0 * s, s, border);
    let over = |r: Rect| pointer.0 >= r.0 && pointer.0 < r.0 + r.2 && pointer.1 >= r.1 && pointer.1 < r.1 + r.3;
    let mut hits = vec![];
    for (&wi, &tile) in wins.iter().zip(tiles.iter()) {
        let Some(win) = model.wins.get(wi) else { continue };
        let hot = over(tile);
        if hot {
            paint::fill_rounded(pix, tile.0 * s, tile.1 * s, tile.2 * s, tile.3 * s, 8.0 * s, hover);
        }
        if win.active() {
            // the window in front: a thin accent line under its header
            paint::fill_rounded(pix, (tile.0 + 8.0) * s, (tile.1 + HEAD_H - 2.0) * s, (tile.2 - 16.0) * s, 2.0 * s, s, paint::accent(255));
        }
        // header: icon, name, and (only on this tile) the cross
        let isz = (18.0 * s) as u32;
        if let Some(ic) = icons.get(&item.icon, isz) {
            paint::draw_pixmap(pix, ic, ((tile.0 + 6.0) * s) as i32, ((tile.1 + HEAD_H / 2.0 - 9.0) * s) as i32);
        }
        let name = label(&win.app_id, &item.name, &win.title);
        let room = tile.2 - 6.0 - 18.0 - 8.0 - if hot { 32.0 } else { 6.0 };
        let shown = crate::fylgja::fit(&fonts.regular, 12.5 * s, &name, room * s);
        paint::draw_text(pix, &fonts.regular, 12.5 * s, (tile.0 + 30.0) * s, (tile.1 + HEAD_H / 2.0) * s, if win.minimized() { dim } else { text }, &shown);
        hits.push((tile, RowAct::Window(wi)));
        if hot {
            let xr = cross_rect(tile);
            let on_x = over(xr);
            if on_x {
                paint::fill_rounded(pix, xr.0 * s, xr.1 * s, xr.2 * s, xr.3 * s, 6.0 * s, (232, 72, 84, 255));
            }
            let tw = paint::text_width(&fonts.regular, 18.0 * s, "\u{d7}");
            paint::draw_text(pix, &fonts.regular, 18.0 * s, (xr.0 + xr.2 / 2.0) * s - tw / 2.0, (xr.1 + xr.3 / 2.0) * s, if on_x { (255, 255, 255, 255) } else { text }, "\u{d7}");
            hits.push((xr, RowAct::CloseWin(wi)));
        }
        // the picture, centred in its box: the app's icon until it arrives (and for a window the compositor does not draw)
        let (bx, by) = (tile.0 + (tile.2 - THUMB_W) / 2.0, tile.1 + HEAD_H);
        paint::fill_rounded(pix, bx * s, by * s, THUMB_W * s, THUMB_H * s, 6.0 * s, well);
        match thumbs.get(&win.uuid) {
            Some(t) => {
                let (tw, th) = (t.width() as f32 / s, t.height() as f32 / s);
                let (tx, ty) = (bx + (THUMB_W - tw) / 2.0, by + (THUMB_H - th) / 2.0);
                paint::draw_pixmap(pix, t, (tx * s) as i32, (ty * s) as i32);
                if win.minimized() {
                    // a minimized window's last picture, dimmed
                    paint::fill_rounded(pix, tx * s, ty * s, tw * s, th * s, 0.0, (bg.0, bg.1, bg.2, 150));
                }
            }
            None => {
                if let Some(ic) = icons.get(&item.icon, (48.0 * s) as u32) {
                    paint::draw_pixmap(pix, ic, ((bx + THUMB_W / 2.0 - 24.0) * s) as i32, ((by + THUMB_H / 2.0 - 24.0) * s) as i32);
                }
            }
        }
        if win.minimized() {
            let note = "Minimized";
            let nw = paint::text_width(&fonts.regular, 11.0 * s, note) / s;
            paint::fill_rounded(pix, (bx + THUMB_W - nw - 14.0) * s, (by + THUMB_H - 22.0) * s, (nw + 10.0) * s, 18.0 * s, 9.0 * s, (0, 0, 0, 150));
            paint::draw_text(pix, &fonts.regular, 11.0 * s, (bx + THUMB_W - nw - 9.0) * s, (by + THUMB_H - 13.0) * s, (230, 230, 236, 255), note);
        }
    }
    (card, hits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_become_what_the_window_shows() {
        // seen on this machine
        assert_eq!(label("vivaldi-stable", "Vivaldi", "Artifex-Technologies/Yggdrasil: Yggdrasil: a desktop environment in Rust for Labyrinth OS. - Vivaldi"), "Artifex-Technologies/Yggdrasil: Yggdrasil: a desktop environment in Rust for Labyrinth OS.");
        assert_eq!(label("com.microsoft.VSCode", "Visual Studio Code", "Welcome - Labyrinth_OS - Visual Studio Code"), "Labyrinth_OS");
        // other shapes
        assert_eq!(label("code", "Visual Studio Code", "● main.rs — Yggdrasil — Visual Studio Code"), "Yggdrasil");
        assert_eq!(label("code", "Visual Studio Code", "Labyrinth_OS — Visual Studio Code"), "Labyrinth_OS");
        assert_eq!(label("codium", "VSCodium", "VSCodium"), "VSCodium");
        assert_eq!(label("firefox", "Firefox", "Arch Linux — Mozilla Firefox"), "Arch Linux");
        assert_eq!(label("librewolf", "LibreWolf", "Privacy Guides — LibreWolf"), "Privacy Guides");
        assert_eq!(label("brave-browser", "Brave", "YouTube - Brave"), "YouTube");
        assert_eq!(label("vivaldi-stable", "Vivaldi", "Vivaldi"), "Vivaldi");
        assert_eq!(label("minos", "Minos", "/home/zlothra/Documents/Github"), "Github");
        assert_eq!(label("org.gnome.Nautilus", "Files", "Downloads – Files"), "Downloads");
        assert_eq!(label("org.gnome.gedit", "Text Editor", "*notes.txt (~/Documents) - gedit"), "notes.txt");
        assert_eq!(label("dev.zed.Zed", "Zed", "main.rs — yggdrasil"), "main.rs");
        assert_eq!(label("com.mitchellh.ghostty", "Ghostty", "~/Documents/Github"), "~/Documents/Github");
        assert_eq!(label("steam", "Steam", "Steam"), "Steam");
        assert_eq!(label("spotify", "Spotify", "Song - Artist - Spotify"), "Song - Artist");
        assert_eq!(label("obsidian", "Obsidian", ""), "Obsidian");
    }

    #[test]
    fn the_card_sits_over_the_icon_and_wraps_after_six() {
        let (card, tiles) = layout(3440.0, 400.0, 1720.0, 2);
        assert_eq!(tiles.len(), 2);
        assert!((card.0 + card.2 / 2.0 - 1720.0).abs() < 1.0, "centred over the icon");
        assert!(card.1 + card.3 <= 400.0 - 4.0);
        let (_, t7) = layout(3440.0, 800.0, 1720.0, 7);
        assert_eq!(t7[6].0, t7[0].0, "the seventh starts a new row");
        assert!(t7[6].1 > t7[0].1);
        // near the left edge, kept on screen; a narrow screen fits fewer per row
        let (c, _) = layout(3440.0, 400.0, 20.0, 3);
        assert_eq!(c.0, 8.0);
        let (_, narrow) = layout(600.0, 900.0, 300.0, 4);
        assert!(narrow[2].1 > narrow[0].1);
        assert!(cross_rect(tiles[0]).0 + cross_rect(tiles[0]).2 <= tiles[0].0 + tiles[0].2);
    }
}
