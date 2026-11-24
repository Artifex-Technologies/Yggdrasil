//! Colors, after EdenExplorer's dark look: a near-black blue-grey, a purple accent for the active
//! tab and selection, and green / amber / red drive-usage bars. The light palette is a plain
//! counterpart, not a design of its own.

use eframe::egui::{self, Color32, CornerRadius};
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub sidebar: Color32,
    pub strip: Color32,
    pub field: Color32,
    pub border: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub accent: Color32,
    pub tab_active: Color32,
    pub hover: Color32,
    pub select: Color32,
    pub row_alt: Color32,
    pub folder: Color32,
    pub file: Color32,
    pub drive: Color32,
    pub green: Color32,
    pub amber: Color32,
    pub red: Color32,
    /// What is picked on a map or a chart, set apart from the accent (the time zone picker's chosen zone): orange unless the
    /// operating system's branding names one (`highlight = #rrggbb`).
    pub highlight: Color32,
    /// Settings > Accessibility > High contrast: black and white, strong lines, the accent pushed until it reads.
    pub high_contrast: bool,
}

/// High contrast as last read from Settings > Accessibility (`apply_accessibility`); every palette made after follows it.
static HIGH_CONTRAST: AtomicBool = AtomicBool::new(false);

/// Whether this program's palettes are the high-contrast ones.
pub fn high_contrast() -> bool {
    HIGH_CONTRAST.load(Ordering::Relaxed)
}

/// Turns this program's high-contrast palettes on or off; true when that changed.
pub fn set_high_contrast(on: bool) -> bool {
    HIGH_CONTRAST.swap(on, Ordering::Relaxed) != on
}

/// The default palette (Minos's own purple).
pub fn palette(dark: bool) -> Palette {
    palette_for(dark, None)
}

impl Palette {
    /// Text and marks that sit on the accent color: white, or near-black when the accent itself is light (a white accent); with
    /// high contrast, whichever of black and white stands out more.
    pub fn on_accent(&self) -> Color32 {
        if self.high_contrast {
            return if contrast(Color32::BLACK, self.accent) >= contrast(Color32::WHITE, self.accent) {
                Color32::BLACK
            } else {
                Color32::WHITE
            };
        }
        let l = |c: u8| {
            let v = c as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let lum = 0.2126 * l(self.accent.r()) + 0.7152 * l(self.accent.g()) + 0.0722 * l(self.accent.b());
        if lum > 0.45 {
            Color32::from_rgb(24, 20, 36)
        } else {
            Color32::WHITE
        }
    }
}

/// The palette with a different accent color: the accent itself, plus the active-tab and selection
/// colors mixed from it so they stay readable on the background. High contrast when Settings says so (`high_contrast`).
pub fn palette_for(dark: bool, accent: Option<[u8; 3]>) -> Palette {
    palette_with(dark, accent, high_contrast())
}

/// [`palette_for`] with high contrast given: text at least 7:1 on every background it sits on, the dim text too, lines and
/// the status colours at least 3:1 and 4.5:1 (WCAG's AAA text, non-text and AA levels; the test checks every pair).
pub fn palette_with(dark: bool, accent: Option<[u8; 3]>, high: bool) -> Palette {
    if high {
        let mut p = high_contrast_palette(dark);
        let a = accent.map(|[r, g, b]| Color32::from_rgb(r, g, b)).unwrap_or(p.accent);
        let backs = [p.bg, p.sidebar, p.field, p.row_alt];
        p.accent = toward_contrast(a, &backs, 7.0);
        p.highlight = toward_contrast(p.highlight, &backs, 4.5);
        p.tab_active = tint(p.bg, a, p.text, if dark { 0.55 } else { 0.40 });
        p.select = tint(p.bg, a, p.text, if dark { 0.35 } else { 0.30 });
        return p;
    }
    let mut p = base_palette(dark);
    if let Some([r, g, b]) = accent {
        let a = Color32::from_rgb(r, g, b);
        // the accent is also text and lines: one too dark to read on the dark style (Rosewood #65000b) is lifted, same hue;
        // the tab and selection colours keep the real one
        p.accent = if dark { readable_on_dark(a) } else { a };
        p.tab_active = mix(p.bg, a, if dark { 0.55 } else { 0.40 });
        p.select = mix(p.bg, a, if dark { 0.28 } else { 0.30 });
    }
    p
}

/// A colour whose brightest channel is under 150 is scaled up until it is 176 (hue kept), so it reads on the dark style.
pub fn readable_on_dark(c: Color32) -> Color32 {
    let m = c.r().max(c.g()).max(c.b());
    if m >= 150 || m == 0 {
        return c;
    }
    let k = 176.0 / m as f32;
    let f = |x: u8| (x as f32 * k).round().min(255.0) as u8;
    Color32::from_rgb(f(c.r()), f(c.g()), f(c.b()))
}

/// The WCAG contrast ratio of two opaque colours (1 to 21).
pub fn contrast(a: Color32, b: Color32) -> f32 {
    let lum = |c: Color32| {
        let l = |x: u8| {
            let v = x as f32 / 255.0;
            if v <= 0.03928 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * l(c.r()) + 0.7152 * l(c.g()) + 0.0722 * l(c.b())
    };
    let (x, y) = (lum(a), lum(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// `c` moved toward white (on dark backgrounds) or black (on light ones) until it has `min` contrast with each of `backs`; hue
/// kept as far as it can be.
fn toward_contrast(c: Color32, backs: &[Color32], min: f32) -> Color32 {
    let bg = backs[0];
    let to = if contrast(bg, Color32::BLACK) < contrast(bg, Color32::WHITE) { Color32::WHITE } else { Color32::BLACK };
    (0..=20).map(|i| mix(c, to, i as f32 / 20.0)).find(|x| backs.iter().all(|b| contrast(*x, *b) >= min)).unwrap_or(to)
}

/// The accent mixed into `bg`, as strongly as `strongest` but no more than keeps `text` at 7:1 on it.
fn tint(bg: Color32, accent: Color32, text: Color32, strongest: f32) -> Color32 {
    (0..=10).map(|i| mix(bg, accent, strongest * (10 - i) as f32 / 10.0)).find(|x| contrast(text, *x) >= 7.0).unwrap_or(bg)
}

/// Black and white with strong lines; the accent, selection and highlight are set from the chosen accent in `palette_with`.
fn high_contrast_palette(dark: bool) -> Palette {
    let mut p = base_palette(dark);
    p.high_contrast = true;
    if dark {
        p.bg = Color32::BLACK;
        p.sidebar = Color32::BLACK;
        p.strip = Color32::BLACK;
        p.field = Color32::from_rgb(10, 10, 12);
        p.border = Color32::from_rgb(200, 200, 210);
        p.text = Color32::WHITE;
        p.dim = Color32::from_rgb(214, 214, 222);
        p.hover = Color32::from_rgb(44, 44, 52);
        p.row_alt = Color32::from_rgb(16, 16, 20);
        p.folder = Color32::from_rgb(255, 210, 80);
        p.file = Color32::from_rgb(230, 230, 236);
        p.drive = Color32::from_rgb(220, 220, 228);
        p.green = Color32::from_rgb(90, 230, 130);
        p.amber = Color32::from_rgb(255, 196, 70);
        p.red = Color32::from_rgb(255, 112, 122);
    } else {
        p.bg = Color32::WHITE;
        p.sidebar = Color32::WHITE;
        p.strip = Color32::from_rgb(240, 240, 244);
        p.field = Color32::WHITE;
        p.border = Color32::from_rgb(64, 64, 74);
        p.text = Color32::BLACK;
        p.dim = Color32::from_rgb(56, 56, 66);
        p.hover = Color32::from_rgb(222, 222, 230);
        p.row_alt = Color32::from_rgb(244, 244, 248);
        p.folder = Color32::from_rgb(150, 100, 0);
        p.file = Color32::from_rgb(60, 64, 76);
        p.drive = Color32::from_rgb(60, 64, 76);
        p.green = Color32::from_rgb(0, 110, 40);
        p.amber = Color32::from_rgb(140, 84, 0);
        p.red = Color32::from_rgb(176, 0, 20);
    }
    p
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// The accent the user chose (Settings > Appearance, `~/.config/yggdrasil/appearance.conf`); None when they chose none.
pub fn system_accent() -> Option<[u8; 3]> {
    yggdrasil_conf::appearance::load().accent
}

/// Whether the desktop is set to the dark style (`~/.config/yggdrasil/appearance.conf`; dark when nothing is set).
pub fn system_dark() -> bool {
    yggdrasil_conf::appearance::load().dark
}

fn base_palette(dark: bool) -> Palette {
    if dark {
        Palette {
            bg: Color32::from_rgb(16, 17, 22),
            sidebar: Color32::from_rgb(14, 15, 20),
            strip: Color32::from_rgb(11, 12, 16),
            field: Color32::from_rgb(24, 25, 32),
            border: Color32::from_rgb(38, 40, 52),
            text: Color32::from_rgb(232, 233, 238),
            dim: Color32::from_rgb(140, 143, 157),
            accent: {
                let a = yggdrasil_conf::integrations::default_accent();
                Color32::from_rgb(a[0], a[1], a[2])
            },
            tab_active: Color32::from_rgb(72, 44, 128),
            hover: Color32::from_rgb(32, 34, 44),
            select: Color32::from_rgb(52, 40, 92),
            row_alt: Color32::from_rgb(19, 20, 26),
            folder: Color32::from_rgb(240, 196, 64),
            file: Color32::from_rgb(190, 194, 206),
            drive: Color32::from_rgb(176, 180, 194),
            green: Color32::from_rgb(58, 158, 94),
            amber: Color32::from_rgb(201, 146, 58),
            red: Color32::from_rgb(180, 58, 69),
            highlight: highlight(true),
            high_contrast: false,
        }
    } else {
        Palette {
            bg: Color32::from_rgb(250, 250, 252),
            sidebar: Color32::from_rgb(243, 243, 247),
            strip: Color32::from_rgb(232, 232, 238),
            field: Color32::from_rgb(255, 255, 255),
            border: Color32::from_rgb(214, 214, 224),
            text: Color32::from_rgb(28, 29, 36),
            dim: Color32::from_rgb(110, 113, 128),
            accent: Color32::from_rgb(110, 70, 200),
            tab_active: Color32::from_rgb(196, 176, 238),
            hover: Color32::from_rgb(232, 232, 240),
            select: Color32::from_rgb(214, 200, 244),
            row_alt: Color32::from_rgb(244, 244, 248),
            folder: Color32::from_rgb(232, 178, 40),
            file: Color32::from_rgb(120, 126, 142),
            drive: Color32::from_rgb(110, 116, 132),
            green: Color32::from_rgb(46, 140, 80),
            amber: Color32::from_rgb(196, 130, 30),
            red: Color32::from_rgb(190, 50, 60),
            highlight: highlight(false),
            high_contrast: false,
        }
    }
}

/// The highlight colour: the branding's, else orange (a little deeper on the light style, so it holds on white).
fn highlight(dark: bool) -> Color32 {
    match yggdrasil_conf::integrations::default_highlight() {
        Some([r, g, b]) => Color32::from_rgb(r, g, b),
        None if dark => Color32::from_rgb(255, 140, 50),
        None => Color32::from_rgb(234, 108, 16),
    }
}

/// Usage bar color: green below 75%, amber below 95%, red above (Eden's 68% green, 92% amber, 100% red).
pub fn usage_color(p: &Palette, fraction: f32) -> Color32 {
    if fraction < 0.75 {
        p.green
    } else if fraction < 0.95 {
        p.amber
    } else {
        p.red
    }
}

impl Palette {
    /// The backgrounds at `pct` percent opaque (100 leaves them as they are); text, lines and accents stay solid.
    pub fn translucent(mut self, pct: u32) -> Palette {
        if pct >= 100 {
            return self;
        }
        let f = |c: Color32| {
            let [r, g, b, a] = c.to_srgba_unmultiplied();
            Color32::from_rgba_unmultiplied(r, g, b, (a as u32 * pct / 100) as u8)
        };
        self.bg = f(self.bg);
        self.sidebar = f(self.sidebar);
        self.strip = f(self.strip);
        self.field = f(self.field);
        self.row_alt = f(self.row_alt);
        self
    }
}

pub fn apply(ctx: &egui::Context, dark: bool, accent: Option<[u8; 3]>) {
    apply_opacity(ctx, dark, accent, 100)
}

/// `apply`, with the window's backgrounds `pct` percent opaque (for a see-through window).
pub fn apply_opacity(ctx: &egui::Context, dark: bool, accent: Option<[u8; 3]>, pct: u32) {
    ctx.data_mut(|d| d.insert_temp(egui::Id::new(LOOK_ID), (dark, accent, pct)));
    let p = palette_for(dark, accent).translucent(pct);
    ctx.set_theme(if dark { egui::Theme::Dark } else { egui::Theme::Light });
    ctx.all_styles_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = p.bg;
        v.window_fill = p.field;
        v.extreme_bg_color = p.field;
        v.faint_bg_color = p.row_alt;
        v.window_stroke = egui::Stroke::new(1.0, p.border);
        v.selection.bg_fill = p.select;
        v.selection.stroke = egui::Stroke::new(1.0, p.accent);
        v.override_text_color = Some(p.text);
        let r = CornerRadius::same(7);
        for w in
            [&mut v.widgets.noninteractive, &mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open]
        {
            w.corner_radius = r;
        }
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.border);
        v.widgets.inactive.weak_bg_fill = p.field;
        v.widgets.inactive.bg_fill = p.field;
        v.widgets.hovered.weak_bg_fill = p.hover;
        v.widgets.hovered.bg_fill = p.hover;
        v.widgets.active.weak_bg_fill = p.select;
        v.widgets.active.bg_fill = p.select;
        if p.high_contrast {
            // every button and field outlined, not told apart by a faint fill alone
            let line = egui::Stroke::new(1.0, p.border);
            v.widgets.inactive.bg_stroke = line;
            v.widgets.hovered.bg_stroke = egui::Stroke::new(1.5, p.text);
            v.widgets.active.bg_stroke = egui::Stroke::new(1.5, p.accent);
        }
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        style.spacing.item_spacing = egui::vec2(8.0, 6.0);
        // Anything clickable (buttons, links like "Show details", rows) shows the pointing hand; text is not selectable, so it never
        // shows the typing cursor. Text boxes still show it, where typing happens.
        style.visuals.interact_cursor = Some(egui::CursorIcon::PointingHand);
        style.interaction.selectable_labels = false;
    });
}

/// Text size, reduced motion and high contrast from Settings > Accessibility (panel.conf), and the system font from Settings > Appearance >
/// Fonts (`fonts::refresh`): read at the first frame, then again only when a file in the settings folder is written (an
/// inotify watch on a thread of its own wakes the window then). Nothing wakes an idle app for it: a once-a-second look kept
/// every app waking (YGG-17). Call it at the top of every frame of every app.
pub fn apply_accessibility(ctx: &egui::Context) {
    cursor_rest(ctx);
    if !settings_changed(ctx, "accessibility") {
        return;
    }
    apply_settings(ctx, &yggdrasil_conf::PanelConf::load());
}

/// The arguments of the last `apply_opacity`, kept in the context so a change of contrast can apply the look again.
const LOOK_ID: &str = "yggdrasil-look";

/// Text size, reduced motion and high contrast from `c`, and the system font.
fn apply_settings(ctx: &egui::Context, c: &yggdrasil_conf::PanelConf) {
    if set_high_contrast(c.high_contrast) {
        if let Some((dark, accent, pct)) = ctx.data(|d| d.get_temp::<(bool, Option<[u8; 3]>, u32)>(egui::Id::new(LOOK_ID))) {
            apply_opacity(ctx, dark, accent, pct);
        }
        ctx.request_repaint();
    }
    let zoom = f32::from(c.text_scale.clamp(yggdrasil_conf::TEXT_SCALE_MIN, yggdrasil_conf::TEXT_SCALE_MAX)) / 100.0;
    if (ctx.zoom_factor() - zoom).abs() > 0.001 {
        ctx.set_zoom_factor(zoom);
    }
    let anim = if c.reduce_motion { 0.0 } else { 1.0 / 12.0 };
    ctx.all_styles_mut(|s| s.animation_time = anim);
    crate::fonts::refresh(ctx);
}

/// How long a text cursor blinks after the last key, click or pointer move; then it stays lit and the window sleeps (GTK's
/// `gtk-cursor-blink-timeout`). A blinking cursor wakes the window twice a second for as long as it has the focus.
const CURSOR_BLINK_FOR: f64 = 10.0;

/// Stops the text cursor blinking once nothing has happened for `CURSOR_BLINK_FOR`, and starts it again at the next input.
fn cursor_rest(ctx: &egui::Context) {
    let last_id = egui::Id::new("yggdrasil-last-input");
    let (now, active) = ctx.input(|i| (i.time, !i.events.is_empty() || i.pointer.is_moving() || i.pointer.any_down()));
    let last: f64 = if active {
        ctx.data_mut(|d| d.insert_temp(last_id, now));
        now
    } else {
        ctx.data_mut(|d| *d.get_temp_mut_or_insert_with(last_id, || now))
    };
    let blink = now - last < CURSOR_BLINK_FOR;
    let on_id = egui::Id::new("yggdrasil-cursor-blinks");
    if ctx.data(|d| d.get_temp::<bool>(on_id)) != Some(blink) {
        ctx.data_mut(|d| d.insert_temp(on_id, blink));
        ctx.all_styles_mut(|s| s.visuals.text_cursor.blink = blink);
    }
}

/// The settings folder's watch: a count that goes up when a file there is written, made, removed or renamed; the windows
/// to wake then.
struct Watch {
    changes: std::sync::atomic::AtomicU64,
    windows: std::sync::Mutex<Vec<egui::Context>>,
}

static WATCH: std::sync::OnceLock<Option<&'static Watch>> = std::sync::OnceLock::new();

/// True at the first call for `who` in a window, and again after a file in the settings folder (`~/.config/yggdrasil`:
/// panel.conf, appearance.conf, fonts) changed since its last call. The window is woken when one changes, so an app reads
/// its settings again then, and never on a timer. Each `who` keeps its own count, so several readers in one app all see a
/// change.
pub fn settings_changed(ctx: &egui::Context, who: &str) -> bool {
    changed_on(*WATCH.get_or_init(|| start_watch(&watched())), ctx, who)
}

fn changed_on(watch: Option<&'static Watch>, ctx: &egui::Context, who: &str) -> bool {
    let Some(w) = watch else {
        // no inotify (an odd system): read every frame the app draws anyway, never waking it for this
        return true;
    };
    {
        let mut windows = w.windows.lock().unwrap_or_else(|e| e.into_inner());
        if !windows.iter().any(|c| c == ctx) {
            windows.push(ctx.clone());
        }
    }
    let now = w.changes.load(std::sync::atomic::Ordering::Acquire);
    let id = egui::Id::new(("yggdrasil-settings-seen", who));
    let seen: Option<u64> = ctx.data(|d| d.get_temp(id));
    if seen == Some(now) {
        return false;
    }
    ctx.data_mut(|d| d.insert_temp(id, now));
    true
}

/// The folders watched: Yggdrasil's settings, and the system font's file for this user and for all
/// (`yggdrasil_conf::fonts`).
fn watched() -> Vec<std::path::PathBuf> {
    let mut dirs = vec![yggdrasil_conf::dirs::config()];
    for f in [yggdrasil_conf::fonts::user_path(), std::path::PathBuf::from(yggdrasil_conf::fonts::SYSTEM_PATH)] {
        dirs.extend(f.parent().map(std::path::Path::to_path_buf));
    }
    dirs
}

/// Watches `dirs` (the user's two are made when missing, as writing a setting would; the system's is not touched).
fn start_watch(dirs: &[std::path::PathBuf]) -> Option<&'static Watch> {
    // SAFETY: inotify on a descriptor of our own, read into a buffer of our own
    let fd = unsafe { libc::inotify_init1(libc::IN_CLOEXEC) };
    if fd < 0 {
        return None;
    }
    let mask = libc::IN_CLOSE_WRITE | libc::IN_CREATE | libc::IN_DELETE | libc::IN_MOVED_TO | libc::IN_MOVED_FROM;
    let mut any = false;
    for (i, dir) in dirs.iter().enumerate() {
        if i < 2 {
            let _ = std::fs::create_dir_all(dir);
        }
        let Ok(path) = std::ffi::CString::new(std::os::unix::ffi::OsStrExt::as_bytes(dir.as_os_str())) else { continue };
        any |= unsafe { libc::inotify_add_watch(fd, path.as_ptr(), mask) } >= 0;
    }
    if !any {
        unsafe { libc::close(fd) };
        return None;
    }
    let w: &'static Watch = Box::leak(Box::new(Watch { changes: 0.into(), windows: std::sync::Mutex::new(vec![]) }));
    std::thread::Builder::new()
        .name("settings-watch".into())
        .spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let n = unsafe { libc::read(fd, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                if n <= 0 {
                    return;
                }
                w.changes.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
                for c in w.windows.lock().unwrap_or_else(|e| e.into_inner()).iter() {
                    c.request_repaint();
                }
            }
        })
        .ok()?;
    Some(w)
}

#[cfg(test)]
mod tests {

    /// Every pair the apps draw in the high-contrast palettes, with the default accent and accents that are hard to read on one
    /// style or the other (a deep red, white, yellow, a mid grey).
    #[test]
    fn the_high_contrast_palettes_meet_the_contrast_levels() {
        use super::*;
        let accents =
            [None, Some([0x65, 0x00, 0x0b]), Some([255, 255, 255]), Some([255, 230, 0]), Some([128, 128, 128]), Some([110, 70, 200])];
        for dark in [true, false] {
            for accent in accents {
                let p = palette_with(dark, accent, true);
                let at = |a: Color32, b: Color32, min: f32, what: &str| {
                    let r = contrast(a, b);
                    assert!(r >= min, "{} {accent:?}: {what} is {r:.2}:1, under {min}:1", if dark { "dark" } else { "light" });
                };
                for (bg, name) in [
                    (p.bg, "bg"),
                    (p.sidebar, "sidebar"),
                    (p.strip, "strip"),
                    (p.field, "field"),
                    (p.hover, "hover"),
                    (p.row_alt, "row_alt"),
                    (p.select, "select"),
                    (p.tab_active, "tab_active"),
                ] {
                    at(p.text, bg, 7.0, &format!("text on {name}"));
                }
                for (bg, name) in [(p.bg, "bg"), (p.sidebar, "sidebar"), (p.field, "field"), (p.row_alt, "row_alt")] {
                    at(p.dim, bg, 7.0, &format!("dim text on {name}"));
                    at(p.accent, bg, 7.0, &format!("accent on {name}"));
                    at(p.border, bg, 3.0, &format!("lines on {name}"));
                    for (c, what) in [(p.green, "green"), (p.amber, "amber"), (p.red, "red"), (p.highlight, "highlight")] {
                        at(c, bg, 4.5, &format!("{what} on {name}"));
                    }
                    for (c, what) in [(p.folder, "folder"), (p.file, "file"), (p.drive, "drive")] {
                        at(c, bg, 3.0, &format!("{what} icon on {name}"));
                    }
                }
                at(p.on_accent(), p.accent, 7.0, "text on the accent");
            }
            // off, the palette is the usual one
            assert!(!palette_with(dark, None, false).high_contrast && palette_with(dark, None, true).high_contrast);
        }
    }

    #[test]
    fn a_text_size_written_in_settings_is_applied_without_polling() {
        use super::*;
        // a watch of its own on a folder of its own (the environment is the whole test process's)
        let dir = std::env::temp_dir().join(format!("ygg-ui-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let watch = start_watch(std::slice::from_ref(&dir));
        assert!(watch.is_some());
        let ctx = egui::Context::default();
        assert!(changed_on(watch, &ctx, "accessibility"), "the first frame reads");
        assert!(!changed_on(watch, &ctx, "accessibility"), "nothing changed: nothing to read again");
        assert!(changed_on(watch, &ctx, "another reader"), "a reader's first call reads");
        let conf = dir.join("panel.conf");
        std::fs::write(&conf, "text_scale = 150\n").unwrap();
        let start = std::time::Instant::now();
        while !changed_on(watch, &ctx, "accessibility") {
            assert!(start.elapsed() < std::time::Duration::from_secs(5), "the watch did not see the write");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // the file is made, then written: read again at each change until the write is in
        let zoom = loop {
            apply_settings(&ctx, &yggdrasil_conf::PanelConf::parse(&std::fs::read_to_string(&conf).unwrap()));
            // egui takes a new zoom at the start of the next frame
            let mut out = ctx.run_ui(egui::RawInput::default(), |_| {});
            out.textures_delta.clear();
            let zoom = ctx.zoom_factor();
            if (zoom - 1.5).abs() < 0.001 || start.elapsed() > std::time::Duration::from_secs(5) {
                break zoom;
            }
            while !changed_on(watch, &ctx, "accessibility") && start.elapsed() < std::time::Duration::from_secs(5) {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        };
        std::fs::remove_dir_all(&dir).unwrap();
        assert!((zoom - 1.5).abs() < 0.001, "{zoom}");
    }

    #[test]
    fn the_text_cursor_stops_blinking_after_ten_quiet_seconds_and_starts_at_a_key() {
        use super::*;
        let ctx = egui::Context::default();
        let frame = |t: f64, events: Vec<egui::Event>| {
            let mut out = ctx.run_ui(egui::RawInput { time: Some(t), events, ..Default::default() }, |_| cursor_rest(&ctx));
            out.textures_delta.clear();
            ctx.global_style().visuals.text_cursor.blink
        };
        assert!(frame(0.0, vec![]));
        assert!(frame(9.0, vec![]));
        assert!(!frame(10.5, vec![]), "ten quiet seconds: the cursor stays lit");
        assert!(frame(11.0, vec![egui::Event::Text("1".into())]), "a key: it blinks again");
        assert!(!frame(21.5, vec![]));
    }

    #[test]
    fn a_very_dark_accent_is_lifted_for_text_on_dark() {
        let rosewood = Color32::from_rgb(101, 0, 11);
        let p = palette_for(true, Some([101, 0, 11]));
        assert_eq!(p.accent, Color32::from_rgb(176, 0, 19), "same hue, readable");
        assert_eq!(palette_for(false, Some([101, 0, 11])).accent, rosewood, "the light style keeps it");
        assert_eq!(readable_on_dark(Color32::from_rgb(124, 84, 214)), Color32::from_rgb(124, 84, 214), "bright enough already");
        assert_eq!(readable_on_dark(Color32::BLACK), Color32::BLACK);
    }

    use super::*;

    #[test]
    fn no_accent_gives_the_default_palette() {
        let a = palette(true);
        let b = palette_for(true, None);
        assert_eq!((a.accent, a.tab_active, a.select), (b.accent, b.tab_active, b.select));
    }

    #[test]
    fn a_system_accent_drives_the_highlights_and_stays_readable() {
        let p = palette_for(true, Some([40, 160, 90]));
        assert_eq!(p.accent, Color32::from_rgb(40, 160, 90));
        // The selection and active tab take the accent's hue but stay darker than the accent itself.
        assert!(p.select.g() > p.select.r() && p.select.g() > p.select.b());
        assert!(p.tab_active.g() > p.tab_active.r());
        assert!(p.select.g() < p.accent.g());
        // The background and text are untouched.
        assert_eq!(p.bg, palette(true).bg);
        assert_eq!(p.text, palette(true).text);
    }

    #[test]
    fn the_light_palette_gets_lighter_mixes() {
        let p = palette_for(false, Some([110, 70, 200]));
        assert!(p.select.r() > 190, "light selection should be pale, got {:?}", p.select);
    }
}
