//! What the bar and the dock show and where things are: pure layout and drawing, no Wayland.

use resvg::tiny_skia::Pixmap;
use wayland_protocols_plasma::plasma_window_management::client::org_kde_plasma_window::OrgKdePlasmaWindow;

use crate::apps::{self, App};
use crate::paint::{self, Fonts, Icons, Rgba};
use labyrinth_conf::{ColorMode, DockPos, PanelConf};

// Plasma window state flags (org_kde_plasma_window_management.state).
pub const ST_ACTIVE: u32 = 0x1;
pub const ST_MINIMIZED: u32 = 0x2;
pub const ST_SKIPTASKBAR: u32 = 0x1000;

/// The colours a bar or the dock is drawn with.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Look {
    pub bg: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub dim: Rgba,
    pub hover: Rgba,
}

/// Dark or light, tinted with the Labyrinth purple ("theme") or plain ("neutral"), at an opacity
/// from 0 (nothing but the content) to 100. The default is Minos's near-black over the wallpaper.
pub fn look(mode: ColorMode, opacity: u32, dark: bool) -> Look {
    let (r, g, b) = match (mode, dark) {
        (ColorMode::Theme, true) => (18, 11, 30),
        (ColorMode::Theme, false) => (240, 234, 252),
        (ColorMode::Neutral, true) => (22, 22, 26),
        (ColorMode::Neutral, false) => (246, 246, 249),
    };
    let alpha = (opacity.min(100) as f32 * 2.55).round() as u8;
    // The edge line fades with the shelf, and is gone when the shelf is.
    let edge = alpha.min(40) / 2 + if alpha > 0 { 6 } else { 0 };
    if dark {
        Look { bg: (r, g, b, alpha), border: (255, 255, 255, edge), text: (236, 230, 248, 255), dim: (178, 168, 200, 255), hover: (255, 255, 255, 34) }
    } else {
        Look { bg: (r, g, b, alpha), border: (0, 0, 0, edge), text: (28, 22, 44, 255), dim: (96, 88, 120, 255), hover: (0, 0, 0, 30) }
    }
}

pub const BAR_H: u32 = 28;
const GAP: f32 = 10.0;
const PAD_X: f32 = 14.0;
const PAD_TOP: f32 = 9.0;

/// The dock's height for an icon size: the icon, padding above and room for the running dot below.
pub fn dock_h(icon: u32) -> u32 {
    icon + 24
}

pub struct Win {
    pub key: u32,
    pub uuid: String,
    pub title: String,
    pub app_id: String,
    pub flags: u32,
    pub ready: bool,
    pub menu: Option<(String, String)>,
    pub proxy: OrgKdePlasmaWindow,
}

impl Win {
    pub fn active(&self) -> bool {
        self.flags & ST_ACTIVE != 0
    }
    pub fn minimized(&self) -> bool {
        self.flags & ST_MINIMIZED != 0
    }
    pub fn listed(&self) -> bool {
        self.ready && self.flags & ST_SKIPTASKBAR == 0 && !self.app_id.is_empty()
    }
}

pub struct Model {
    pub apps: Vec<App>,
    pub pinned: Vec<String>,
    pub wins: Vec<Win>,
    /// The time (24 hours) and the date ("Sun 19 Dec").
    pub clock: String,
    pub date: String,
    pub conf: PanelConf,
    /// The system style is dark (Settings > Appearance); light flips the panels' text colours.
    pub dark: bool,
    /// Desktops, network and Bluetooth, read in the background.
    pub status: crate::status::Status,
    /// The trash has files in it (its dock icon shows full).
    pub trash_full: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    Logo,
    Clock,
    /// A desktop (workspace) pill, by index.
    Workspace(usize),
    /// The "+" that adds a desktop.
    NewWorkspace,
    Notch,
    Network,
    Bluetooth,
    /// A dock icon: the app id of its group.
    Dock(String),
}

#[derive(Clone, Debug)]
pub struct Hit {
    pub rect: (f32, f32, f32, f32),
    pub action: Action,
}

impl Hit {
    pub fn contains(&self, x: f32, y: f32) -> bool {
        let (rx, ry, rw, rh) = self.rect;
        x >= rx && x < rx + rw && y >= ry && y < ry + rh
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct DockItem {
    pub app_id: String,
    pub name: String,
    pub icon: String,
    /// Indices into Model::wins.
    pub wins: Vec<usize>,
    pub pinned: bool,
    pub active: bool,
}

/// Pinned apps first (in order), then every other running app, one icon per app.
pub fn dock_items(m: &Model) -> Vec<DockItem> {
    let mut items: Vec<DockItem> = vec![];
    for id in &m.pinned {
        let app = m.apps.iter().find(|a| &a.id == id);
        let Some(app) = app else { continue };
        items.push(DockItem { app_id: app.id.clone(), name: app.name.clone(), icon: app.icon.clone(), wins: vec![], pinned: true, active: false });
    }
    for (i, w) in m.wins.iter().enumerate().filter(|(_, w)| w.listed()) {
        let app = apps::for_app_id(&m.apps, &w.app_id);
        let id = app.map(|a| a.id.clone()).unwrap_or_else(|| w.app_id.clone());
        let pos = items.iter().position(|it| it.app_id == id).unwrap_or_else(|| {
            items.push(DockItem {
                app_id: id.clone(),
                name: app.map(|a| a.name.clone()).unwrap_or_else(|| w.app_id.clone()),
                icon: app.map(|a| a.icon.clone()).unwrap_or_else(|| w.app_id.clone()),
                wins: vec![],
                pinned: false,
                active: false,
            });
            items.len() - 1
        });
        items[pos].wins.push(i);
        if w.active() {
            items[pos].active = true;
        }
    }
    // The trash sits at the right end, like on a Mac and in Windows' recycle bin.
    items.push(DockItem {
        app_id: TRASH_ID.into(),
        name: "Trash".into(),
        icon: if m.trash_full { "user-trash-full" } else { "user-trash" }.into(),
        wins: vec![],
        pinned: true,
        active: false,
    });
    items
}

/// Dock id of the trash, which is not an app.
pub const TRASH_ID: &str = "labyrinth:trash";

/// The freedesktop trash folder that holds the files.
pub fn trash_dir() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("XDG_DATA_HOME").map(std::path::PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/share")))?;
    Some(base.join("Trash/files"))
}

pub fn trash_has_files() -> bool {
    trash_dir().and_then(|d| std::fs::read_dir(d).ok()).map(|mut r| r.next().is_some()).unwrap_or(false)
}

/// The dock's size in logical pixels for the current items: wide on the bottom, tall on a side.
pub fn dock_size(items: &[DockItem], icon: u32, pos: DockPos) -> (u32, u32) {
    let n = items.len().max(1) as f32;
    let len = (PAD_X * 2.0 + n * icon as f32 + (n - 1.0) * GAP).ceil() as u32;
    if pos.vertical() { (dock_h(icon), len) } else { (len, dock_h(icon)) }
}

pub fn app_name_for_bar(m: &Model) -> String {
    match m.wins.iter().find(|w| w.listed() && w.active()) {
        Some(w) => apps::for_app_id(&m.apps, &w.app_id).map(|a| a.name.clone()).unwrap_or_else(|| w.app_id.clone()),
        None => "Desktop".to_string(),
    }
}

/// The Labyrinth purple, for the current desktop and the active bits.
const ACCENT: Rgba = (124, 84, 214, 255);

/// The top bar: no strip of its own, just pills over the wallpaper (their backdrop is the bar's
/// opacity setting; 0 leaves floating text), a black tab in the middle where the notch will open.
pub fn paint_bar(pix: &mut Pixmap, s: f32, slim: bool, m: &Model, fonts: &Fonts, icons: &mut Icons, hover: Option<usize>) -> Vec<Hit> {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    pix.fill(paint::color((0, 0, 0, 0)));
    let lk = look(m.conf.bar_color, m.conf.bar_opacity, m.dark);
    let cy = h / 2.0;
    let ph = 22.0 * s;
    let py = (h - ph) / 2.0;
    let r = 7.0 * s;
    let mut hits: Vec<Hit> = vec![];
    let pill = |pix: &mut Pixmap, x: f32, wd: f32| {
        if lk.bg.3 > 0 {
            paint::fill_rounded(pix, x, py, wd, ph, r, lk.bg);
        }
    };
    let lit = |hits: &Vec<Hit>| hover == Some(hits.len());

    // The notch's closed tab, hanging from the top edge in the middle of the main screen.
    if !slim {
        let nw = 150.0 * s;
        let nx = (w - nw) / 2.0;
        // drawn from above the edge so the top corners are square and the bottom ones round
        paint::fill_rounded(pix, nx, -12.0 * s, nw, 12.0 * s + ph, 10.0 * s, (0, 0, 0, 255));
        hits.push(Hit { rect: (nx, 0.0, nw, h), action: Action::Notch });
    }

    if slim {
        // The tiling screen's bar: the focused app on the left and the clock in the middle.
        let name = app_name_for_bar(m);
        let nw = paint::text_width(&fonts.bold, 12.0 * s, &name) + 20.0 * s;
        pill(pix, 8.0 * s, nw);
        paint::draw_text(pix, &fonts.bold, 12.0 * s, 18.0 * s, cy, lk.text, &name);
        let stamp = format!("{}  {}", m.date, m.clock);
        let cw = paint::text_width(&fonts.regular, 12.0 * s, &stamp) + 20.0 * s;
        let cx = (w - cw) / 2.0;
        pill(pix, cx, cw);
        if lit(&hits) {
            paint::fill_rounded(pix, cx, py, cw, ph, r, lk.hover);
        }
        paint::draw_text(pix, &fonts.regular, 12.0 * s, cx + 10.0 * s, cy, lk.text, &stamp);
        hits.push(Hit { rect: (cx, 0.0, cw, h), action: Action::Clock });
        return hits;
    }

    // ---- left: the Labyrinth button, the desktops, the focused app's name
    let mut x = 8.0 * s;
    let lw = 30.0 * s;
    pill(pix, x, lw);
    if lit(&hits) {
        paint::fill_rounded(pix, x, py, lw, ph, r, lk.hover);
    }
    let isz = (16.0 * s) as u32;
    if let Some(ic) = icons.get("labyrinthos", isz) {
        paint::draw_pixmap(pix, ic, (x + (lw - isz as f32) / 2.0) as i32, (cy - isz as f32 / 2.0).round() as i32);
    }
    hits.push(Hit { rect: (x, 0.0, lw, h), action: Action::Logo });
    x += lw + 6.0 * s;

    let n = m.status.desktops.len().max(1);
    let slot = 22.0 * s;
    let can_add = n < crate::status::MAX_DESKTOPS;
    let wpw = 4.0 * s * 2.0 + slot * (n as f32 + if can_add { 1.0 } else { 0.0 });
    pill(pix, x, wpw);
    let mut sx = x + 4.0 * s;
    for i in 0..n {
        let current = i == m.status.current.min(n - 1);
        if current {
            paint::fill_rounded(pix, sx + 1.0 * s, py + 2.0 * s, slot - 2.0 * s, ph - 4.0 * s, 5.0 * s, ACCENT);
        } else if lit(&hits) {
            paint::fill_rounded(pix, sx + 1.0 * s, py + 2.0 * s, slot - 2.0 * s, ph - 4.0 * s, 5.0 * s, lk.hover);
        }
        let label = (i + 1).to_string();
        let tw = paint::text_width(&fonts.bold, 12.0 * s, &label);
        paint::draw_text(pix, &fonts.bold, 12.0 * s, sx + (slot - tw) / 2.0, cy, if current { (255, 255, 255, 255) } else { lk.dim }, &label);
        hits.push(Hit { rect: (sx, 0.0, slot, h), action: Action::Workspace(i) });
        sx += slot;
    }
    if can_add {
        if lit(&hits) {
            paint::fill_rounded(pix, sx + 1.0 * s, py + 2.0 * s, slot - 2.0 * s, ph - 4.0 * s, 5.0 * s, lk.hover);
        }
        let tw = paint::text_width(&fonts.bold, 14.0 * s, "+");
        paint::draw_text(pix, &fonts.bold, 14.0 * s, sx + (slot - tw) / 2.0, cy, lk.dim, "+");
        hits.push(Hit { rect: (sx, 0.0, slot, h), action: Action::NewWorkspace });
    }
    x += wpw + 6.0 * s;

    // The focused app's name; it gives way to the notch when the screen is narrow.
    let name = app_name_for_bar(m);
    let nw = paint::text_width(&fonts.bold, 12.5 * s, &name) + 20.0 * s;
    if x + nw < (w - 150.0 * s) / 2.0 - 8.0 * s {
        pill(pix, x, nw);
        paint::draw_text(pix, &fonts.bold, 12.5 * s, x + 10.0 * s, cy, lk.text, &name);
    }

    // ---- right: network, Bluetooth, the clock (24 hours)
    let mut rx = w - 8.0 * s;
    let cw = paint::text_width(&fonts.regular, 12.5 * s, &m.clock) + 20.0 * s;
    rx -= cw;
    pill(pix, rx, cw);
    if lit(&hits) {
        paint::fill_rounded(pix, rx, py, cw, ph, r, lk.hover);
    }
    paint::draw_text(pix, &fonts.regular, 12.5 * s, rx + 10.0 * s, cy, lk.text, &m.clock);
    hits.push(Hit { rect: (rx, 0.0, cw, h), action: Action::Clock });
    rx -= 6.0 * s;
    let dw = paint::text_width(&fonts.regular, 12.5 * s, &m.date) + 20.0 * s;
    rx -= dw;
    pill(pix, rx, dw);
    if lit(&hits) {
        paint::fill_rounded(pix, rx, py, dw, ph, r, lk.hover);
    }
    paint::draw_text(pix, &fonts.regular, 12.5 * s, rx + 10.0 * s, cy, lk.text, &m.date);
    hits.push(Hit { rect: (rx, 0.0, dw, h), action: Action::Clock });
    rx -= 6.0 * s;

    let iw = 28.0 * s;
    let isz = (14.0 * s) as u32;
    let icon_pill = |pix: &mut Pixmap, icons: &mut Icons, hits: &mut Vec<Hit>, rx: &mut f32, name: &str, action: Action| {
        *rx -= iw;
        pill(pix, *rx, iw);
        if hover == Some(hits.len()) {
            paint::fill_rounded(pix, *rx, py, iw, ph, r, lk.hover);
        }
        if let Some(ic) = icons.get_tinted(name, isz, lk.text) {
            paint::draw_pixmap(pix, ic, (*rx + (iw - isz as f32) / 2.0) as i32, (cy - isz as f32 / 2.0).round() as i32);
        }
        hits.push(Hit { rect: (*rx, 0.0, iw, h), action });
        *rx -= 6.0 * s;
    };
    if m.status.bt != crate::status::Bt::Missing {
        let name = match m.status.bt {
            crate::status::Bt::Off => "bluetooth-disabled-symbolic",
            crate::status::Bt::Connected => "bluetooth-active-symbolic",
            _ => "bluetooth-paired-symbolic",
        };
        icon_pill(pix, icons, &mut hits, &mut rx, name, Action::Bluetooth);
    }
    let net = match m.status.net {
        crate::status::Net::Off => "network-offline-symbolic",
        crate::status::Net::Wired => "network-wired-symbolic",
        crate::status::Net::Wifi(sig) if sig >= 75 => "network-wireless-signal-excellent-symbolic",
        crate::status::Net::Wifi(sig) if sig >= 50 => "network-wireless-signal-good-symbolic",
        crate::status::Net::Wifi(sig) if sig >= 25 => "network-wireless-signal-ok-symbolic",
        crate::status::Net::Wifi(_) => "network-wireless-signal-weak-symbolic",
    };
    icon_pill(pix, icons, &mut hits, &mut rx, net, Action::Network);
    hits
}

pub fn paint_dock(pix: &mut Pixmap, s: f32, m: &Model, fonts: &Fonts, icons: &mut Icons, hover: Option<usize>) -> Vec<Hit> {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    pix.fill(paint::color((0, 0, 0, 0)));
    let lk = look(m.conf.dock_color, m.conf.dock_opacity, m.dark);
    let icon = m.conf.dock_icon as f32;
    if lk.bg.3 > 0 {
        let r = (icon * 0.4).max(14.0) * s;
        paint::fill_rounded(pix, 0.5 * s, 0.5 * s, w - s, h - s, r, lk.bg);
        paint::stroke_rounded(pix, 0.5 * s, 0.5 * s, w - s, h - s, r, s, lk.border);
    }
    let items = dock_items(m);
    let mut hits = vec![];
    let isz = (icon * s) as u32;
    let pos = m.conf.dock_pos;
    // Along the dock's length the icons run from `PAD_X`; across it they sit `PAD_TOP` from the
    // side that faces the screen (the top of a bottom dock), leaving the running dot room on the edge side.
    let mut along = PAD_X * s;
    let across = match pos {
        DockPos::Bottom | DockPos::Right => PAD_TOP * s,
        DockPos::Left => (dock_h(m.conf.dock_icon) as f32 - icon - PAD_TOP) * s,
    };
    for (i, it) in items.iter().enumerate() {
        let (x, y) = if pos.vertical() { (across, along) } else { (along, across) };
        let rect = (x - 4.0 * s, y - 4.0 * s, icon * s + 8.0 * s, icon * s + 8.0 * s);
        if hover == Some(i) {
            paint::fill_rounded(pix, rect.0, rect.1, rect.2, rect.3, 12.0 * s, lk.hover);
        }
        match icons.get(&it.icon, isz) {
            Some(p) => paint::draw_pixmap(pix, p, x.round() as i32, y.round() as i32),
            None => {
                // No icon in the theme: a purple tile with the app's first letter.
                paint::fill_rounded(pix, x, y, icon * s, icon * s, 11.0 * s, (84, 58, 140, 255));
                let letter: String = it.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
                let size = icon * 0.5 * s;
                let lw = paint::text_width(&fonts.bold, size, &letter);
                paint::draw_text(pix, &fonts.bold, size, x + (icon * s - lw) / 2.0, y + icon * s / 2.0, (236, 230, 248, 255), &letter);
            }
        }
        // Running indicator under the icon; brighter for the focused app.
        if !it.wins.is_empty() {
            let c = if it.active { lk.text } else { lk.dim };
            let (dx, dy) = match pos {
                DockPos::Bottom => (x + icon * s / 2.0, h - 8.0 * s),
                DockPos::Left => (8.0 * s, y + icon * s / 2.0),
                DockPos::Right => (w - 8.0 * s, y + icon * s / 2.0),
            };
            paint::fill_circle(pix, dx, dy, 2.2 * s, c);
        }
        hits.push(Hit { rect, action: Action::Dock(it.app_id.clone()) });
        along += (icon + GAP) * s;
    }
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    // Window proxies need a live connection, so the grouping logic is tested through a minimal
    // stand-in that mirrors dock_items' use of the model.
    fn app(id: &str, name: &str) -> App {
        App { id: id.into(), name: name.into(), icon: id.into(), ..Default::default() }
    }

    #[test]
    fn pinned_apps_come_first_and_missing_ones_are_skipped() {
        let m = Model { apps: vec![app("minos", "Minos"), app("vivaldi-stable", "Vivaldi")], pinned: vec!["vivaldi-stable".into(), "ghost".into(), "minos".into()], wins: vec![], clock: String::new(), date: String::new(), conf: PanelConf::default(), dark: true, status: Default::default(), trash_full: false };
        let items = dock_items(&m);
        assert_eq!(items.iter().map(|i| i.app_id.as_str()).collect::<Vec<_>>(), vec!["vivaldi-stable", "minos", TRASH_ID]); // the trash is always last
        assert!(items.iter().all(|i| i.pinned && i.wins.is_empty()));
    }

    #[test]
    fn dock_width_grows_with_items() {
        let one = dock_size(&[DockItem { app_id: "a".into(), name: "A".into(), icon: "a".into(), wins: vec![], pinned: true, active: false }], 44, DockPos::Bottom);
        let it = |n: &str| DockItem { app_id: n.into(), name: n.into(), icon: n.into(), wins: vec![], pinned: true, active: false };
        let three = dock_size(&[it("a"), it("b"), it("c")], 44, DockPos::Bottom);
        let side = dock_size(&[it("a"), it("b"), it("c")], 44, DockPos::Left);
        assert_eq!((side.0, side.1), (three.1, three.0)); // a side dock is the bottom dock turned on its end
        assert!(three.0 > one.0);
        assert_eq!(one.1, dock_h(44));
        assert_eq!(three.0 - one.0, 2 * (44 + GAP as u32));
        assert!(dock_size(&[it("a")], 64, DockPos::Bottom).0 > one.0 && dock_size(&[it("a")], 64, DockPos::Bottom).1 > one.1); // bigger icons, bigger dock
    }

    #[test]
    fn looks_follow_style_and_opacity() {
        let dark = look(ColorMode::Theme, 69, true);
        let light = look(ColorMode::Theme, 69, false);
        assert!(dark.bg.2 < 60 && light.bg.0 > 200); // dark vs light base
        assert!(dark.text.0 > 200 && light.text.0 < 60); // text flips
        assert_eq!(look(ColorMode::Theme, 100, true).bg.3, 255);
        let clear = look(ColorMode::Neutral, 0, true);
        assert_eq!((clear.bg.3, clear.border.3), (0, 0)); // invisible shelf, no edge line
        assert!(look(ColorMode::Theme, 69, true).bg.2 > look(ColorMode::Neutral, 69, true).bg.2); // purple tint vs plain
    }
}
