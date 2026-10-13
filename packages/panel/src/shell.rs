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
    /// The time for the calendar: seconds too in 24-hour mode ("16:41:23").
    pub clock_secs: String,
    pub date: String,
    /// Today as (year, month, day), for the calendar.
    pub today: (i32, u32, u32),
    /// The signed-in user's name, for the start menu.
    pub user: String,
    /// This machine can hibernate (the Folders theme lists it then).
    pub hibernate: bool,
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
    let isz = (15.0 * s) as u32;
    if let Some(ic) = icons.get_svg("logo-solid", include_str!("../assets/logo-solid.svg"), isz, lk.text) {
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
    // The date and the time in one pill; clicking it opens the calendar.
    let stamp = format!("{}   {}", m.date, m.clock);
    let cw = paint::text_width(&fonts.regular, 12.5 * s, &stamp) + 20.0 * s;
    rx -= cw;
    pill(pix, rx, cw);
    if lit(&hits) {
        paint::fill_rounded(pix, rx, py, cw, ph, r, lk.hover);
    }
    paint::draw_text(pix, &fonts.regular, 12.5 * s, rx + 10.0 * s, cy, lk.text, &stamp);
    hits.push(Hit { rect: (rx, 0.0, cw, h), action: Action::Clock });
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

/// The date popover: today's date and the time, then a month you can scroll through. `offset` is
/// how many months from this one. Returns the card's rectangle (x, y, w, h) in logical pixels.
pub fn paint_calendar(pix: &mut Pixmap, s: f32, m: &Model, offset: i32, fonts: &Fonts) -> (f32, f32, f32, f32) {
    use crate::calendar;
    pix.fill(paint::color((0, 0, 0, 0)));
    let w = pix.width() as f32 / s;
    let dark = m.dark;
    let (bg, border, text, dim, faint): (Rgba, Rgba, Rgba, Rgba, Rgba) = if dark {
        ((18, 18, 24, 240), (255, 255, 255, 36), (240, 240, 246, 255), (160, 160, 176, 255), (96, 96, 110, 255))
    } else {
        ((250, 250, 252, 244), (0, 0, 0, 30), (24, 24, 32, 255), (96, 96, 112, 255), (170, 170, 182, 255))
    };
    let (cw, pad, row) = (372.0, 16.0, 34.0);
    let col = (cw - 2.0 * pad) / 7.0;
    let ch = 104.0 + 6.0 * row + 12.0;
    // Under the date pill with the same 6 px the bar's pills have between them, and ending where the pill ends.
    let (cx, cy) = (w - cw - 8.0, 3.0); // the surface starts at the bar's 28 px line; the pills end 3 px above it
    paint::fill_rounded(pix, cx * s, cy * s, cw * s, ch * s, 14.0 * s, bg);
    paint::stroke_rounded(pix, (cx + 0.5) * s, (cy + 0.5) * s, (cw - 1.0) * s, (ch - 1.0) * s, 14.0 * s, s, border);

    let (ty, tm, td) = m.today;
    // header: today's date on the left, the time on the right
    let title = calendar::full_date(ty, tm, td, !m.conf.clock_24h);
    paint::draw_text(pix, &fonts.bold, 14.0 * s, (cx + pad) * s, (cy + 28.0) * s, text, &title);
    let tw = paint::text_width(&fonts.bold, 14.0 * s, &m.clock_secs);
    paint::draw_text(pix, &fonts.bold, 14.0 * s, (cx + cw - pad) * s - tw, (cy + 28.0) * s, text, &m.clock_secs);

    // the month being shown
    let (y, mo) = calendar::shift_month(ty, tm, offset);
    let label = format!("{} {}", calendar::MONTHS[(mo - 1) as usize], y);
    paint::draw_text(pix, &fonts.regular, 12.5 * s, (cx + pad) * s, (cy + 54.0) * s, if offset == 0 { dim } else { text }, &label);

    // weekday letters
    for (i, d) in calendar::DAYS.iter().enumerate() {
        let l = &d[..1];
        let lw = paint::text_width(&fonts.regular, 12.0 * s, l);
        paint::draw_text(pix, &fonts.regular, 12.0 * s, (cx + pad + col * (i as f32 + 0.5)) * s - lw / 2.0, (cy + 82.0) * s, faint, l);
    }
    // the days
    let g = calendar::grid(y, mo);
    for (r, line) in g.iter().enumerate() {
        for (c, (day, in_month)) in line.iter().enumerate() {
            let mid_x = cx + pad + col * (c as f32 + 0.5);
            let mid_y = cy + 104.0 + row * (r as f32 + 0.5);
            let is_today = *in_month && (y, mo, *day) == (ty, tm, td);
            if is_today {
                paint::fill_circle(pix, mid_x * s, mid_y * s, 15.0 * s, if dark { (255, 255, 255, 70) } else { (124, 84, 214, 255) });
            }
            let label = day.to_string();
            let lw = paint::text_width(&fonts.bold, 15.0 * s, &label);
            let color = if is_today && !dark { (255, 255, 255, 255) } else if *in_month { text } else { faint };
            paint::draw_text(pix, &fonts.bold, 15.0 * s, mid_x * s - lw / 2.0, mid_y * s, color, &label);
        }
    }
    (cx, cy, cw, ch)
}

// ---------------------------------------------------------------- the start menu

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PowerAct {
    Lock,
    Sleep,
    Hibernate,
    Restart,
    Shutdown,
}

#[derive(Clone, Debug, PartialEq)]
pub enum StartAct {
    /// Open the app with this desktop id.
    Launch(String),
    Power(PowerAct),
    /// Open (or show) a folder of apps, by index (the Folders theme).
    Folder(usize),
    /// Open or close the power choices (the Folders theme).
    TogglePower,
}

#[derive(Clone, Debug)]
pub struct StartHit {
    /// (x, y, w, h) in logical pixels.
    pub rect: (f32, f32, f32, f32),
    pub act: StartAct,
}

pub struct StartPaint {
    pub card: (f32, f32, f32, f32),
    pub hits: Vec<StartHit>,
    /// How far the "All apps" list can scroll (in rows).
    pub max_scroll: i32,
}

/// Every app to list, A to Z, without the ones that hide themselves.
pub fn all_apps(apps: &[App]) -> Vec<&App> {
    let mut v: Vec<&App> = apps.iter().filter(|a| !a.no_display).collect();
    v.sort_by_key(|a| a.name.to_lowercase());
    v.dedup_by(|a, b| a.name == b.name);
    v
}

/// What matches a search: names that start with it first, then names that contain it.
pub fn search_apps<'a>(apps: &'a [App], q: &str) -> Vec<&'a App> {
    let q = q.trim().to_lowercase();
    if q.is_empty() {
        return vec![];
    }
    let mut starts: Vec<&App> = vec![];
    let mut has: Vec<&App> = vec![];
    for a in all_apps(apps) {
        let n = a.name.to_lowercase();
        if n.starts_with(&q) {
            starts.push(a);
        } else if n.contains(&q) {
            has.push(a);
        }
    }
    starts.extend(has);
    starts
}

/// The dock's pinned apps, for the start menu's grid.
pub fn pinned_apps(m: &Model) -> Vec<&App> {
    m.pinned.iter().filter_map(|id| m.apps.iter().find(|a| &a.id == id && !a.no_display)).collect()
}

/// `text` cut with an ellipsis to fit `max` logical pixels.
fn fit_text(font: &ab_glyph::FontVec, size: f32, text: &str, max: f32) -> String {
    if paint::text_width(font, size, text) <= max {
        return text.to_string();
    }
    let mut t: String = text.to_string();
    while !t.is_empty() && paint::text_width(font, size, &format!("{t}…")) > max {
        t.pop();
    }
    format!("{}…", t.trim_end())
}

struct StartPal {
    bg: Rgba,
    border: Rgba,
    field: Rgba,
    text: Rgba,
    dim: Rgba,
    hover: Rgba,
    select: Rgba,
}

/// The same palette as Minos and Settings: near-black with the Labyrinth purple.
fn start_pal(dark: bool) -> StartPal {
    if dark {
        StartPal { bg: (14, 15, 20, 250), border: (38, 39, 53, 255), field: (27, 28, 37, 255), text: (230, 231, 238, 255), dim: (154, 155, 171, 255), hover: (255, 255, 255, 20), select: (58, 42, 107, 255) }
    } else {
        StartPal { bg: (248, 248, 251, 250), border: (214, 214, 224, 255), field: (234, 234, 242, 255), text: (28, 28, 38, 255), dim: (104, 104, 122, 255), hover: (0, 0, 0, 16), select: (218, 206, 246, 255) }
    }
}

fn app_icon(pix: &mut Pixmap, s: f32, icons: &mut Icons, fonts: &Fonts, app: &App, x: f32, y: f32, size: f32) {
    match icons.get(&app.icon, (size * s) as u32) {
        Some(p) => paint::draw_pixmap(pix, p, (x * s) as i32, (y * s) as i32),
        None => {
            paint::fill_rounded(pix, x * s, y * s, size * s, size * s, size * 0.24 * s, (84, 58, 140, 255));
            let letter: String = app.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            let ls = size * 0.5 * s;
            let lw = paint::text_width(&fonts.bold, ls, &letter);
            paint::draw_text(pix, &fonts.bold, ls, x * s + (size * s - lw) / 2.0, (y + size / 2.0) * s, (236, 230, 248, 255), &letter);
        }
    }
}

/// The start menu: search, the pinned apps, every app A to Z (the wheel scrolls it), your name and
/// the power buttons. `pointer` lights the row under it.
pub fn paint_start(pix: &mut Pixmap, s: f32, m: &Model, query: &str, scroll: i32, pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons) -> StartPaint {
    pix.fill(paint::color((0, 0, 0, 0)));
    let pal = start_pal(m.dark);
    let surf_h = pix.height() as f32 / s;
    let (cx, cy, cw) = (8.0, 3.0, 500.0);
    let ch = (surf_h - 12.0).min(600.0);
    let rr = |pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba| paint::fill_rounded(pix, x * s, y * s, w * s, h * s, r * s, c);
    let inside = |r: (f32, f32, f32, f32)| pointer.0 >= r.0 && pointer.0 < r.0 + r.2 && pointer.1 >= r.1 && pointer.1 < r.1 + r.3;
    rr(pix, cx, cy, cw, ch, 14.0, pal.bg);
    paint::stroke_rounded(pix, (cx + 0.5) * s, (cy + 0.5) * s, (cw - 1.0) * s, (ch - 1.0) * s, 14.0 * s, s, pal.border);
    let mut hits: Vec<StartHit> = vec![];
    let text = |pix: &mut Pixmap, font: &ab_glyph::FontVec, size: f32, x: f32, y: f32, c: Rgba, t: &str| {
        paint::draw_text(pix, font, size * s, x * s, y * s, c, t);
    };

    // search
    let fx = cx + 16.0;
    let fw = cw - 32.0;
    rr(pix, fx, cy + 16.0, fw, 40.0, 10.0, pal.field);
    paint::stroke_rounded(pix, (fx + 0.5) * s, (cy + 16.5) * s, (fw - 1.0) * s, 39.0 * s, 10.0 * s, s, if query.is_empty() { pal.border } else { (124, 84, 214, 255) });
    if query.is_empty() {
        text(pix, &fonts.regular, 14.0, fx + 14.0, cy + 36.0, pal.dim, "Search apps");
    } else {
        text(pix, &fonts.regular, 14.0, fx + 14.0, cy + 36.0, pal.text, query);
        let qw = paint::text_width(&fonts.regular, 14.0 * s, query) / s;
        rr(pix, fx + 14.0 + qw + 1.0, cy + 26.0, 1.5, 20.0, 0.5, (124, 84, 214, 255)); // the caret
    }

    let bottom = cy + ch - 62.0; // the power row starts here
    let mut max_scroll = 0;
    if query.trim().is_empty() {
        // pinned
        text(pix, &fonts.bold, 13.0, cx + 20.0, cy + 76.0, pal.text, "Pinned");
        let pinned = pinned_apps(m);
        let cell_w = (cw - 32.0) / 6.0;
        for (i, a) in pinned.iter().take(12).enumerate() {
            let (col, row) = ((i % 6) as f32, (i / 6) as f32);
            let r = (fx + col * cell_w, cy + 90.0 + row * 84.0, cell_w, 80.0);
            if inside(r) {
                rr(pix, r.0, r.1, r.2, r.3, 10.0, pal.hover);
            }
            app_icon(pix, s, icons, fonts, a, r.0 + (cell_w - 34.0) / 2.0, r.1 + 10.0, 34.0);
            let name = fit_text(&fonts.regular, 11.5 * s, &a.name, (cell_w - 6.0) * s);
            let nw = paint::text_width(&fonts.regular, 11.5 * s, &name) / s;
            text(pix, &fonts.regular, 11.5, r.0 + (cell_w - nw) / 2.0, r.1 + 62.0, pal.text, &name);
            hits.push(StartHit { rect: r, act: StartAct::Launch(a.id.clone()) });
        }
        // all apps
        let list_top = cy + 90.0 + if pinned.len() > 6 { 2.0 } else { 1.0 } * 84.0 + 16.0;
        text(pix, &fonts.bold, 13.0, cx + 20.0, list_top, pal.text, "All apps");
        let apps = all_apps(&m.apps);
        let (row_h, top) = (36.0, list_top + 14.0);
        let visible = (((bottom - 6.0) - top) / row_h).floor().max(1.0) as i32;
        max_scroll = (apps.len() as i32 - visible).max(0);
        let scroll = scroll.clamp(0, max_scroll);
        for (n, a) in apps.iter().skip(scroll as usize).take(visible as usize).enumerate() {
            let r = (fx, top + n as f32 * row_h, fw, row_h);
            if inside(r) {
                rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.hover);
            }
            app_icon(pix, s, icons, fonts, a, r.0 + 10.0, r.1 + 6.0, 22.0);
            let name = fit_text(&fonts.regular, 13.5 * s, &a.name, (fw - 60.0) * s);
            text(pix, &fonts.regular, 13.5, r.0 + 44.0, r.1 + 17.0, pal.text, &name);
            hits.push(StartHit { rect: r, act: StartAct::Launch(a.id.clone()) });
        }
        // a thin scroll bar when there is more
        if max_scroll > 0 {
            let track = bottom - 6.0 - top;
            let th = (track * visible as f32 / apps.len() as f32).max(24.0);
            let ty = top + (track - th) * scroll as f32 / max_scroll as f32;
            rr(pix, cx + cw - 9.0, ty, 3.0, th, 1.5, (255, 255, 255, 40));
        }
    } else {
        // search results
        let found = search_apps(&m.apps, query);
        if found.is_empty() {
            text(pix, &fonts.regular, 13.5, cx + 20.0, cy + 90.0, pal.dim, "No apps match");
        }
        let (row_h, top) = (42.0, cy + 70.0);
        let visible = (((bottom - 6.0) - top) / row_h).floor().max(1.0) as usize;
        for (n, a) in found.iter().take(visible).enumerate() {
            let r = (fx, top + n as f32 * row_h, fw, row_h);
            if n == 0 {
                rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.select); // Enter opens this one
            } else if inside(r) {
                rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.hover);
            }
            app_icon(pix, s, icons, fonts, a, r.0 + 10.0, r.1 + 7.0, 26.0);
            let name = fit_text(&fonts.regular, 14.0 * s, &a.name, (fw - 70.0) * s);
            text(pix, &fonts.regular, 14.0, r.0 + 48.0, r.1 + 19.0, pal.text, &name);
            hits.push(StartHit { rect: r, act: StartAct::Launch(a.id.clone()) });
        }
    }

    // bottom: your name on the left, the power buttons on the right
    paint::fill_rounded(pix, (cx + 1.0) * s, bottom * s, (cw - 2.0) * s, s, 0.0, pal.border);
    let my = bottom + 31.0;
    paint::fill_circle(pix, (cx + 36.0) * s, my * s, 15.0 * s, (124, 84, 214, 255));
    let initial: String = m.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    let iw = paint::text_width(&fonts.bold, 15.0 * s, &initial) / s;
    text(pix, &fonts.bold, 15.0, cx + 36.0 - iw / 2.0, my, (255, 255, 255, 255), &initial);
    text(pix, &fonts.regular, 13.5, cx + 60.0, my, pal.text, &m.user);
    let buttons = [("Lock", PowerAct::Lock), ("Sleep", PowerAct::Sleep), ("Restart", PowerAct::Restart), ("Shut down", PowerAct::Shutdown)];
    let mut bx = cx + cw - 16.0;
    for (label, act) in buttons.iter().rev() {
        let bw = paint::text_width(&fonts.regular, 12.5 * s, label) / s + 22.0;
        bx -= bw;
        let r = (bx, my - 15.0, bw, 30.0);
        rr(pix, r.0, r.1, r.2, r.3, 8.0, if inside(r) { pal.select } else { pal.field });
        text(pix, &fonts.regular, 12.5, r.0 + 11.0, my, pal.text, label);
        hits.push(StartHit { rect: r, act: StartAct::Power(*act) });
        bx -= 6.0;
    }
    StartPaint { card: (cx, cy, cw, ch), hits, max_scroll }
}

// ---------------------------------------------------------------- the start menu, "Folders" theme

/// Whether this machine can hibernate: the kernel offers "disk", the command line says where to resume
/// from, and there is real swap (not zram) at least as big as the memory.
pub fn hibernate_ok(state: &str, cmdline: &str, meminfo: &str, swaps: &str) -> bool {
    if !state.split_whitespace().any(|w| w == "disk") || !cmdline.contains("resume=") {
        return false;
    }
    let ram: u64 = meminfo.lines().find_map(|l| l.strip_prefix("MemTotal:")).and_then(|v| v.split_whitespace().next()).and_then(|v| v.parse().ok()).unwrap_or(0);
    let swap: u64 = swaps.lines().skip(1).filter(|l| !l.starts_with("/dev/zram")).filter_map(|l| l.split_whitespace().nth(2)?.parse::<u64>().ok()).sum();
    ram > 0 && swap >= ram
}

pub fn can_hibernate() -> bool {
    let read = |p: &str| std::fs::read_to_string(p).unwrap_or_default();
    hibernate_ok(&read("/sys/power/state"), &read("/proc/cmdline"), &read("/proc/meminfo"), &read("/proc/swaps"))
}

/// A folder's glyph: a small purple folder.
fn folder_glyph(pix: &mut Pixmap, s: f32, x: f32, y: f32) {
    paint::fill_rounded(pix, x * s, y * s, 9.0 * s, 6.0 * s, 2.0 * s, (150, 110, 235, 255));
    paint::fill_rounded(pix, x * s, (y + 3.0) * s, 18.0 * s, 13.0 * s, 3.0 * s, (124, 84, 214, 255));
}

pub struct FoldersView<'a> {
    pub query: &'a str,
    pub scroll: i32,
    /// The folder whose apps are showing to the side.
    pub cat: Option<usize>,
    pub sub_scroll: i32,
    pub power_open: bool,
}

/// The "Folders" start menu: narrow, favourites on top, then your apps in folders by kind (a folder opens
/// to the side while the pointer is on it), and a power button that opens the power choices.
pub fn paint_start_folders(pix: &mut Pixmap, s: f32, m: &Model, v: &FoldersView, pointer: (f32, f32), fonts: &Fonts, icons: &mut Icons) -> StartPaint {
    use crate::apps;
    pix.fill(paint::color((0, 0, 0, 0)));
    let pal = start_pal(m.dark);
    let surf_h = pix.height() as f32 / s;
    let (cx, cy, cw) = (8.0, 3.0, 300.0);
    let ch = (surf_h - 12.0).min(640.0);
    let rr = |pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba| paint::fill_rounded(pix, x * s, y * s, w * s, h * s, r * s, c);
    let inside = |r: (f32, f32, f32, f32)| pointer.0 >= r.0 && pointer.0 < r.0 + r.2 && pointer.1 >= r.1 && pointer.1 < r.1 + r.3;
    let text = |pix: &mut Pixmap, font: &ab_glyph::FontVec, size: f32, x: f32, y: f32, c: Rgba, t: &str| paint::draw_text(pix, font, size * s, x * s, y * s, c, t);
    let card = |pix: &mut Pixmap, x: f32, y: f32, w: f32, h: f32| {
        paint::fill_rounded(pix, x * s, y * s, w * s, h * s, 14.0 * s, pal.bg);
        paint::stroke_rounded(pix, (x + 0.5) * s, (y + 0.5) * s, (w - 1.0) * s, (h - 1.0) * s, 14.0 * s, s, pal.border);
    };
    card(pix, cx, cy, cw, ch);
    let mut hits: Vec<StartHit> = vec![];
    let mut union = (cx, cy, cw, ch);

    // search
    let (fx, fw) = (cx + 12.0, cw - 24.0);
    rr(pix, fx, cy + 12.0, fw, 36.0, 10.0, pal.field);
    paint::stroke_rounded(pix, (fx + 0.5) * s, (cy + 12.5) * s, (fw - 1.0) * s, 35.0 * s, 10.0 * s, s, if v.query.is_empty() { pal.border } else { (124, 84, 214, 255) });
    if v.query.is_empty() {
        text(pix, &fonts.regular, 13.5, fx + 12.0, cy + 30.0, pal.dim, "Search apps");
    } else {
        text(pix, &fonts.regular, 13.5, fx + 12.0, cy + 30.0, pal.text, v.query);
        let qw = paint::text_width(&fonts.regular, 13.5 * s, v.query) / s;
        rr(pix, fx + 12.0 + qw + 1.0, cy + 21.0, 1.5, 18.0, 0.5, (124, 84, 214, 255));
    }

    let top = cy + 60.0;
    let bottom = cy + ch - 56.0;
    let row_h = 32.0;
    let mut max_scroll = 0;
    let mut cat_y = None;
    let folders = apps::folders(&m.apps);

    if !v.query.trim().is_empty() {
        let found = search_apps(&m.apps, v.query);
        if found.is_empty() {
            text(pix, &fonts.regular, 13.0, cx + 18.0, top + 16.0, pal.dim, "No apps match");
        }
        let visible = ((bottom - top) / 38.0).floor().max(1.0) as usize;
        for (n, a) in found.iter().take(visible).enumerate() {
            let r = (fx, top + n as f32 * 38.0, fw, 38.0);
            if n == 0 {
                rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.select);
            } else if inside(r) {
                rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.hover);
            }
            app_icon(pix, s, icons, fonts, a, r.0 + 8.0, r.1 + 6.0, 24.0);
            text(pix, &fonts.regular, 13.5, r.0 + 42.0, r.1 + 18.0, pal.text, &fit_text(&fonts.regular, 13.5 * s, &a.name, (fw - 56.0) * s));
            hits.push(StartHit { rect: r, act: StartAct::Launch(a.id.clone()) });
        }
    } else {
        // one scrolling list: Favorites, then Folders
        enum Line<'a> {
            Head(&'static str),
            Fav(&'a App),
            Folder(usize),
        }
        let favs = pinned_apps(m);
        let mut lines: Vec<Line> = vec![Line::Head("Favorites")];
        lines.extend(favs.iter().take(8).map(|a| Line::Fav(a)));
        lines.push(Line::Head("Folders"));
        lines.extend((0..folders.len()).map(Line::Folder));
        let visible = (((bottom - 4.0) - top) / row_h).floor().max(1.0) as i32;
        max_scroll = (lines.len() as i32 - visible).max(0);
        let scroll = v.scroll.clamp(0, max_scroll);
        for (n, line) in lines.iter().skip(scroll as usize).take(visible as usize).enumerate() {
            let r = (fx, top + n as f32 * row_h, fw, row_h);
            match line {
                Line::Head(t) => {
                    text(pix, &fonts.bold, 12.0, r.0 + 6.0, r.1 + 18.0, pal.dim, t);
                }
                Line::Fav(a) => {
                    if inside(r) {
                        rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.hover);
                    }
                    app_icon(pix, s, icons, fonts, a, r.0 + 8.0, r.1 + 5.0, 22.0);
                    text(pix, &fonts.regular, 13.5, r.0 + 40.0, r.1 + 16.0, pal.text, &fit_text(&fonts.regular, 13.5 * s, &a.name, (fw - 54.0) * s));
                    hits.push(StartHit { rect: r, act: StartAct::Launch(a.id.clone()) });
                }
                Line::Folder(i) => {
                    let open = v.cat == Some(*i);
                    if open {
                        rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.select);
                        cat_y = Some(r.1);
                    } else if inside(r) {
                        rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.hover);
                    }
                    folder_glyph(pix, s, r.0 + 8.0, r.1 + 7.0);
                    text(pix, &fonts.regular, 13.5, r.0 + 40.0, r.1 + 16.0, pal.text, folders[*i].0);
                    text(pix, &fonts.regular, 13.5, r.0 + r.2 - 18.0, r.1 + 16.0, pal.dim, ">");
                    hits.push(StartHit { rect: r, act: StartAct::Folder(*i) });
                }
            }
        }
        if max_scroll > 0 {
            let track = bottom - 4.0 - top;
            let th = (track * visible as f32 / lines.len() as f32).max(24.0);
            rr(pix, cx + cw - 7.0, top + (track - th) * scroll as f32 / max_scroll as f32, 3.0, th, 1.5, (255, 255, 255, 40));
        }
    }

    // bottom: your name, and the power button
    paint::fill_rounded(pix, (cx + 1.0) * s, bottom * s, (cw - 2.0) * s, s, 0.0, pal.border);
    let my = bottom + 28.0;
    paint::fill_circle(pix, (cx + 30.0) * s, my * s, 14.0 * s, (124, 84, 214, 255));
    let initial: String = m.user.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    let iw = paint::text_width(&fonts.bold, 14.0 * s, &initial) / s;
    text(pix, &fonts.bold, 14.0, cx + 30.0 - iw / 2.0, my, (255, 255, 255, 255), &initial);
    text(pix, &fonts.regular, 13.0, cx + 52.0, my, pal.text, &fit_text(&fonts.regular, 13.0 * s, &m.user, (cw - 120.0) * s));
    let pbtn = (cx + cw - 12.0 - 34.0, my - 17.0, 34.0, 34.0);
    rr(pix, pbtn.0, pbtn.1, pbtn.2, pbtn.3, 9.0, if v.power_open { pal.select } else if inside(pbtn) { pal.hover } else { pal.field });
    let isz = (16.0 * s) as u32;
    if let Some(ic) = icons.get_tinted("system-shutdown-symbolic", isz, pal.text) {
        paint::draw_pixmap(pix, ic, ((pbtn.0 + 9.0) * s) as i32, ((pbtn.1 + 9.0) * s) as i32);
    }
    hits.push(StartHit { rect: pbtn, act: StartAct::TogglePower });

    // the folder that is open, to the side
    if let (Some(ci), Some(y0)) = (v.cat, cat_y) {
        if let Some((name, list)) = folders.get(ci) {
            let sw = 290.0;
            let sx = cx + cw + 4.0;
            let vis_max = (((surf_h - 16.0) - 52.0) / row_h).floor().max(3.0) as usize;
            let shown = list.len().min(vis_max);
            let sh = 44.0 + shown as f32 * row_h + 8.0;
            let sy = (y0 - 8.0).min(surf_h - 8.0 - sh).max(cy);
            card(pix, sx, sy, sw, sh);
            text(pix, &fonts.bold, 12.0, sx + 16.0, sy + 24.0, pal.dim, name);
            let max_sub = list.len().saturating_sub(shown) as i32;
            let sub = v.sub_scroll.clamp(0, max_sub);
            for (n, a) in list.iter().skip(sub as usize).take(shown).enumerate() {
                let r = (sx + 8.0, sy + 38.0 + n as f32 * row_h, sw - 16.0, row_h);
                if inside(r) {
                    rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.hover);
                }
                app_icon(pix, s, icons, fonts, a, r.0 + 8.0, r.1 + 5.0, 22.0);
                text(pix, &fonts.regular, 13.5, r.0 + 40.0, r.1 + 16.0, pal.text, &fit_text(&fonts.regular, 13.5 * s, &a.name, (r.2 - 52.0) * s));
                hits.push(StartHit { rect: r, act: StartAct::Launch(a.id.clone()) });
            }
            max_scroll = max_scroll.max(0);
            union = (cx, cy.min(sy), sx + sw - cx, (cy + ch).max(sy + sh) - cy.min(sy));
        }
    }

    // the power choices, above the button
    if v.power_open {
        let mut items = vec![("Lock", PowerAct::Lock), ("Sleep", PowerAct::Sleep)];
        if m.hibernate {
            items.push(("Hibernate", PowerAct::Hibernate));
        }
        items.push(("Restart", PowerAct::Restart));
        items.push(("Shut down", PowerAct::Shutdown));
        let pw = 170.0;
        let ph = items.len() as f32 * 34.0 + 12.0;
        let (px, py) = (pbtn.0 + pbtn.2 - pw, pbtn.1 - 8.0 - ph);
        rr(pix, px, py, pw, ph, 12.0, pal.field);
        paint::stroke_rounded(pix, (px + 0.5) * s, (py + 0.5) * s, (pw - 1.0) * s, (ph - 1.0) * s, 12.0 * s, s, pal.border);
        for (n, (label, act)) in items.iter().enumerate() {
            let r = (px + 6.0, py + 6.0 + n as f32 * 34.0, pw - 12.0, 34.0);
            if inside(r) {
                rr(pix, r.0, r.1, r.2, r.3 - 2.0, 8.0, pal.select);
            }
            text(pix, &fonts.regular, 13.5, r.0 + 12.0, r.1 + 17.0, pal.text, label);
            hits.push(StartHit { rect: r, act: StartAct::Power(*act) });
        }
    }
    StartPaint { card: union, hits, max_scroll }
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

    #[test]
    fn hibernate_needs_resume_and_real_swap() {
        let mem = "MemTotal:       16000000 kB\n";
        let swaps = "Filename Type Size Used Priority\n/swap/swapfile file 16777216 0 -2\n/dev/zram0 partition 8000000 0 100\n";
        assert!(hibernate_ok("freeze mem disk", "quiet resume=UUID=abc", mem, swaps));
        assert!(!hibernate_ok("freeze mem", "quiet resume=UUID=abc", mem, swaps), "the kernel does not offer it");
        assert!(!hibernate_ok("freeze mem disk", "quiet", mem, swaps), "no resume= on the command line");
        let zram_only = "Filename Type Size Used Priority\n/dev/zram0 partition 32000000 0 100\n";
        assert!(!hibernate_ok("disk", "resume=x", mem, zram_only), "zram cannot hold the image");
    }

    // Window proxies need a live connection, so the grouping logic is tested through a minimal
    // stand-in that mirrors dock_items' use of the model.
    fn app(id: &str, name: &str) -> App {
        App { id: id.into(), name: name.into(), icon: id.into(), ..Default::default() }
    }

    #[test]
    fn pinned_apps_come_first_and_missing_ones_are_skipped() {
        let m = Model { apps: vec![app("minos", "Minos"), app("vivaldi-stable", "Vivaldi")], pinned: vec!["vivaldi-stable".into(), "ghost".into(), "minos".into()], wins: vec![], clock: String::new(), clock_secs: String::new(), date: String::new(), today: (2026, 1, 1), user: String::new(), hibernate: false, conf: PanelConf::default(), dark: true, status: Default::default(), trash_full: false };
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
