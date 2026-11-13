//! The pointer, drawn by the compositor: the cursor theme's pictures (Yggdrasil's own from yggdrasil-theme, or the theme chosen in
//! Settings and stored by `ygg-theme cursor`), a program's own cursor surface, hidden while the screen is dimmed, and kept on a
//! screen when its screen goes.
//!
//! - **Which theme**: the one `ygg-theme cursor` set on the running compositor (the control socket's `cursor-theme NAME SIZE`,
//!   through yggdrasil-wm), else the one it recorded in `~/.config/yggdrasil/appearance.conf` (`cursor = NAME`, when that theme
//!   is installed; read again when the file changes), else Yggdrasil's own. The size is XCURSOR_SIZE (the session's, which
//!   ygg-theme writes to environment.d), else 24; it is multiplied by the screen's scale, and the theme's nearest size is used.
//!   Themes are looked up as libXcursor does (~/.local/share/icons, ~/.icons, the data folders' icons, their `Inherits`). With no
//!   theme at all a plain arrow is drawn.
//! - **What it looks like**: the name a program asks for (cursor-shape-v1; "default" when the pointer is over no program), or the
//!   program's own picture (wl_pointer.set_cursor, a shared-memory buffer, with its hotspot), or nothing when it hides it.
//!   Animated cursors show their first picture.
//! - **Hidden**: after `pointer-timeout` seconds without motion (yggdrasil-idle sets 0.25 while the screen is dimmed, as it sets
//!   Hyprland's `cursor:inactive_timeout`); the next motion shows it again.
//! - **Drawn**: headless, over a screen's picture as a hardware cursor plane is: screenshots and screen copies show it only when
//!   asked (`screenshot NAME FILE pointer`, grim -c, wlr-screencopy's overlay_cursor, ext-image-copy-capture's paint_cursors), so a
//!   copy of the screen is the same with the pointer anywhere. Nested, the host window draws it (main.rs).

use std::sync::Arc;
use std::time::Instant;

use smithay::input::pointer::{CursorIcon, CursorImageStatus, CursorImageSurfaceData};
use smithay::output::Output;
use smithay::reexports::wayland_server::protocol::{wl_shm, wl_surface::WlSurface};
use smithay::utils::{Logical, Point};
use smithay::wayland::compositor::with_states;

use crate::state::App;

/// Yggdrasil's own pointer theme (yggdrasil-theme installs it as /usr/share/icons/Yggdrasil) and its size.
pub const DEFAULT_THEME: &str = "Yggdrasil";
pub const DEFAULT_SIZE: u32 = 24;

/// A pointer's picture: premultiplied pixels (B G R A in memory, as ARGB8888), its size and hotspot in pixels.
#[derive(Debug, Clone, PartialEq)]
pub struct Picture {
    pub w: u32,
    pub h: u32,
    pub hot: (i32, i32),
    pub data: Vec<u8>,
}

/// A theme's shape at a size in pixels: the key its picture is kept under.
type Shape = (String, u32, CursorIcon);

/// The pointer's state: what programs asked it to look like, when it last moved, the theme from the files, and the pictures
/// loaded from that theme so far.
pub struct Pointer {
    pub status: CursorImageStatus,
    pub last_motion: Instant,
    pub configured: (String, u32),
    cache: Vec<(Shape, Option<Arc<Picture>>)>,
}

impl Default for Pointer {
    fn default() -> Pointer {
        Pointer { status: CursorImageStatus::default_named(), last_motion: Instant::now(), configured: configured(), cache: vec![] }
    }
}

/// A theme's name as it may be used: a plain name, no path.
pub fn plain(name: &str) -> bool {
    !name.is_empty() && name.len() <= 128 && !name.contains('/') && !name.starts_with('.') && !name.chars().any(|c| c.is_control())
}

/// `cursor = NAME` in appearance.conf's text (the last such line).
pub fn appearance_cursor(text: &str) -> Option<String> {
    let mut found = None;
    for line in text.lines() {
        let line = line.split('#').next().unwrap_or("").trim();
        if let Some((k, v)) = line.split_once('=') {
            if k.trim() == "cursor" {
                let v = v.trim();
                found = plain(v).then(|| v.to_string());
            }
        }
    }
    found
}

/// XCURSOR_SIZE's value, when it is a size from 8 to 256.
pub fn size_from(v: Option<&str>) -> u32 {
    v.and_then(|s| s.trim().parse::<u32>().ok()).filter(|s| (8..=256).contains(s)).unwrap_or(DEFAULT_SIZE)
}

/// The theme and size from the files and the session, as ygg-theme reads its own choice: appearance.conf's `cursor` when that
/// theme is installed, else Yggdrasil's own; XCURSOR_SIZE, else 24.
pub fn configured() -> (String, u32) {
    let text = std::fs::read_to_string(yggdrasil_conf::appearance::path()).unwrap_or_default();
    let name = appearance_cursor(&text).filter(|n| installed(n)).unwrap_or_else(|| DEFAULT_THEME.into());
    (name, size_from(std::env::var("XCURSOR_SIZE").ok().as_deref()))
}

/// A cursor theme of that name is installed (it has a default pointer, itself or through the themes it inherits).
fn installed(name: &str) -> bool {
    xcursor::CursorTheme::load(name).load_icon("default").is_some()
}

/// The theme's picture for a pointer shape at `px` pixels: the shape's own name, then its other names, then the default arrow;
/// the image of the theme's size nearest `px` (a larger one when two are as near), its first picture.
pub fn load(theme: &str, icon: CursorIcon, px: u32) -> Option<Picture> {
    let t = xcursor::CursorTheme::load(theme);
    let mut names: Vec<&str> = vec![icon.name()];
    names.extend(icon.alt_names().iter().copied());
    names.extend(["default", "left_ptr"]);
    let path = names.iter().find_map(|n| t.load_icon(n))?;
    let meta = std::fs::metadata(&path).ok()?;
    if meta.len() > 16 << 20 {
        return None;
    }
    let images = xcursor::parser::parse_xcursor(&std::fs::read(&path).ok()?)?;
    let best = images.iter().min_by_key(|i| (i.size.abs_diff(px), std::cmp::Reverse(i.size)))?;
    let (w, h) = (best.width, best.height);
    if w == 0 || h == 0 || w > 1024 || h > 1024 || best.pixels_rgba.len() < (w * h * 4) as usize {
        return None;
    }
    // the file's pixels are ARGB words, little-endian: B G R A in memory, premultiplied, as ARGB8888
    Some(Picture { w, h, hot: (best.xhot.min(w - 1) as i32, best.yhot.min(h - 1) as i32), data: best.pixels_rgba.clone() })
}

/// A plain arrow (white, outlined in black) `px` pixels tall, for when no cursor theme is installed.
pub fn arrow(px: u32) -> Picture {
    let s = px.max(8) as f64;
    // the arrow's outline, in fractions of its height
    const SHAPE: [(f64, f64); 7] = [(0.0, 0.0), (0.0, 0.78), (0.2, 0.6), (0.33, 0.9), (0.45, 0.85), (0.33, 0.56), (0.56, 0.56)];
    let poly: Vec<(f64, f64)> = SHAPE.iter().map(|(x, y)| (x * s, y * s)).collect();
    let inside = |x: f64, y: f64| {
        let mut c = false;
        let mut j = poly.len() - 1;
        for i in 0..poly.len() {
            let ((xi, yi), (xj, yj)) = (poly[i], poly[j]);
            if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
                c = !c;
            }
            j = i;
        }
        c
    };
    let (w, h) = ((s * 0.6).ceil() as u32 + 1, s.ceil() as u32 + 1);
    let mut data = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let at = |dx: i32, dy: i32| inside(x as f64 + 0.5 + dx as f64, y as f64 + 0.5 + dy as f64);
            if !at(0, 0) {
                continue;
            }
            let edge = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|(dx, dy)| !at(*dx, *dy));
            let v = if edge { 0 } else { 255 };
            let o = ((y * w + x) * 4) as usize;
            data[o..o + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    Picture { w, h, hot: (0, 0), data }
}

/// A program's own cursor surface (wl_pointer.set_cursor): its shared-memory picture at `scale`, with its hotspot. A picture
/// drawn for another scale is stretched to it (nearest pixel).
pub fn from_surface(surface: &WlSurface, scale: f64) -> Option<Picture> {
    use smithay::backend::renderer::utils::RendererSurfaceStateUserData;
    with_states(surface, |states| {
        let hot = states.data_map.get::<CursorImageSurfaceData>().map(|d| d.lock().unwrap().hotspot).unwrap_or_default();
        let data = states.data_map.get::<RendererSurfaceStateUserData>()?;
        let data = data.lock().unwrap();
        let buffer_scale = data.buffer_scale().max(1) as f64;
        let buffer = data.buffer()?;
        smithay::wayland::shm::with_buffer_contents(buffer, |ptr, len, b| {
            let (bw, bh, stride, offset) = (b.width, b.height, b.stride, b.offset);
            if bw <= 0 || bh <= 0 || bw > 1024 || bh > 1024 || stride < bw * 4 || offset < 0 {
                return None;
            }
            if offset as usize + stride as usize * bh as usize > len {
                return None;
            }
            let opaque = match b.format {
                wl_shm::Format::Argb8888 => false,
                wl_shm::Format::Xrgb8888 => true,
                _ => return None,
            };
            // SAFETY: the rows lie inside the pool (checked above); a program writing meanwhile only spoils its own pointer
            let src = unsafe { std::slice::from_raw_parts(ptr.add(offset as usize), stride as usize * bh as usize) };
            let f = scale / buffer_scale;
            let (w, h) = (((bw as f64 * f).round() as u32).max(1), ((bh as f64 * f).round() as u32).max(1));
            let mut out = vec![0u8; (w * h * 4) as usize];
            for y in 0..h {
                let sy = ((y as f64 + 0.5) / f).floor().min(bh as f64 - 1.0) as usize;
                for x in 0..w {
                    let sx = ((x as f64 + 0.5) / f).floor().min(bw as f64 - 1.0) as usize;
                    let i = sy * stride as usize + sx * 4;
                    let o = ((y * w + x) * 4) as usize;
                    out[o..o + 4].copy_from_slice(&src[i..i + 4]);
                    if opaque {
                        out[o + 3] = 255;
                    }
                }
            }
            let hot = ((hot.x as f64 * scale).round() as i32, (hot.y as f64 * scale).round() as i32);
            Some(Picture { w, h, hot, data: out })
        })
        .ok()
        .flatten()
    })
}

/// Paint a pointer's picture over a picture of `w` x `h` 4-byte pixels (`stride` bytes a row), its top-left corner at `at`
/// (premultiplied "over"; what falls outside is left out).
pub fn blend(dst: &mut [u8], stride: usize, (w, h): (u32, u32), pic: &Picture, at: (i32, i32)) {
    for py in 0..pic.h as i32 {
        let y = at.1 + py;
        if y < 0 || y >= h as i32 {
            continue;
        }
        for px in 0..pic.w as i32 {
            let x = at.0 + px;
            if x < 0 || x >= w as i32 {
                continue;
            }
            let s = ((py as u32 * pic.w + px as u32) * 4) as usize;
            let a = pic.data[s + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = y as usize * stride + x as usize * 4;
            let Some(dp) = dst.get_mut(d..d + 4) else { continue };
            for (c, d) in dp.iter_mut().enumerate() {
                let sc = pic.data[s + c].min(pic.data[s + 3]) as u32;
                *d = (sc + (*d as u32 * (255 - a) + 127) / 255).min(255) as u8;
            }
        }
    }
}

impl App {
    /// The theme and size in use: as ygg-theme set it on the running compositor, else from the files.
    pub fn pointer_theme(&self) -> (String, u32) {
        self.cursor_theme.clone().unwrap_or_else(|| self.cursor.configured.clone())
    }

    /// Whether the pointer is hidden for resting (`pointer-timeout`, while the screen is dimmed).
    pub fn pointer_resting(&self) -> bool {
        self.pointer_timeout > 0.0 && self.cursor.last_motion.elapsed().as_secs_f64() >= self.pointer_timeout
    }

    /// What the pointer looks like on this output now (None: hidden or not on it), and its top-left corner in the output's
    /// pixels.
    pub fn pointer_picture(&mut self, output: &Output) -> Option<(Arc<Picture>, (i32, i32))> {
        if self.pointer_resting() {
            return None;
        }
        let scale = output.current_scale().fractional_scale();
        let pic = match self.cursor.status.clone() {
            CursorImageStatus::Hidden => return None,
            CursorImageStatus::Surface(s) => Arc::new(from_surface(&s, scale)?),
            CursorImageStatus::Named(icon) => {
                let (theme, size) = self.pointer_theme();
                let px = (size as f64 * scale).round() as u32;
                let key = (theme.clone(), px, icon);
                match self.cursor.cache.iter().find(|(k, _)| *k == key) {
                    Some((_, p)) => p.clone(),
                    None => {
                        let p = load(&theme, icon, px).or_else(|| (icon == CursorIcon::Default).then(|| arrow(px))).map(Arc::new);
                        // a few shapes at a few sizes; a new theme starts afresh
                        self.cursor.cache.retain(|((t, _, _), _)| *t == theme);
                        self.cursor.cache.push((key, p.clone()));
                        p
                    }
                }
                .or_else(|| Some(Arc::new(arrow(px))))?
            }
        };
        let origin = output.current_location();
        let rel = self.pointer_location - origin.to_f64();
        let (x, y) = ((rel.x * scale).round() as i32 - pic.hot.0, (rel.y * scale).round() as i32 - pic.hot.1);
        let size = output.current_mode()?.size;
        let on = x < size.w && y < size.h && x + pic.w as i32 > 0 && y + pic.h as i32 > 0;
        on.then_some((pic, (x, y)))
    }

    /// One line for the control socket's `pointer-look`: hidden, or the theme, size and shape, or the program's own picture.
    pub fn pointer_look(&self) -> String {
        if self.pointer_resting() {
            return "hidden\tresting".into();
        }
        match &self.cursor.status {
            CursorImageStatus::Hidden => "hidden\tprogram".into(),
            CursorImageStatus::Named(icon) => {
                let (t, s) = self.pointer_theme();
                format!("theme\t{t}\t{s}\t{}", icon.name())
            }
            CursorImageStatus::Surface(s) => {
                let p = from_surface(s, 1.0);
                format!("surface\t{}", p.map(|p| format!("{}x{}\t{},{}", p.w, p.h, p.hot.0, p.hot.1)).unwrap_or("-".into()))
            }
        }
    }

    /// The theme in appearance.conf changed (the 2-second look at the config files): its pictures are loaded anew.
    pub fn pointer_files_changed(&mut self) {
        let now = configured();
        if now != self.cursor.configured {
            self.cursor.configured = now;
            self.cursor.cache.clear();
        }
    }

    /// The screens changed: a pointer left on no screen goes back onto one. When its screen went it goes to the middle of the
    /// first screen; otherwise (its screen moved or got smaller) to the nearest point of the nearest screen.
    pub fn keep_pointer_on_screens(&mut self, before: &[(String, smithay::utils::Rectangle<i32, Logical>)]) {
        let pos = self.pointer_location;
        let rects: Vec<(String, smithay::utils::Rectangle<i32, Logical>)> =
            self.space.outputs().map(|o| (o.name(), crate::state::output_rect(o))).collect();
        if rects.iter().any(|(_, r)| r.to_f64().contains(pos)) {
            return;
        }
        let was = before.iter().find(|(_, r)| r.to_f64().contains(pos)).map(|(n, _)| n.clone());
        let gone = was.is_some_and(|n| !rects.iter().any(|(m, _)| *m == n));
        let to = if gone {
            rects.first().map(|(_, r)| Point::from(((r.loc.x + r.size.w / 2) as f64, (r.loc.y + r.size.h / 2) as f64)))
        } else {
            self.nearest_on_screens(pos)
        };
        if let Some(to) = to {
            self.pointer_moved_to(to);
        }
    }

    /// The point of the screens nearest `pos` (itself when it is on one); None with no screen.
    pub fn nearest_on_screens(&self, pos: Point<f64, Logical>) -> Option<Point<f64, Logical>> {
        let inside = |r: smithay::utils::Rectangle<i32, Logical>| -> Point<f64, Logical> {
            Point::from((
                pos.x.clamp(r.loc.x as f64, (r.loc.x + r.size.w - 1).max(r.loc.x) as f64),
                pos.y.clamp(r.loc.y as f64, (r.loc.y + r.size.h - 1).max(r.loc.y) as f64),
            ))
        };
        let dist = |p: Point<f64, Logical>| (p.x - pos.x).powi(2) + (p.y - pos.y).powi(2);
        self.space.outputs().map(|o| inside(crate::state::output_rect(o))).min_by(|a, b| dist(*a).total_cmp(&dist(*b)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_is_read_from_appearance_conf() {
        assert_eq!(appearance_cursor("style = dark\naccent = 1,2,3\ncursor = macOS\n"), Some("macOS".into()));
        assert_eq!(appearance_cursor("cursor = A\ncursor = B # the later line wins\n"), Some("B".into()));
        assert_eq!(appearance_cursor("style = dark\n"), None, "none: Yggdrasil's own");
        assert_eq!(appearance_cursor("cursor = ../../etc\n"), None, "a path is not a theme");
        assert_eq!(size_from(Some("32")), 32);
        assert_eq!(size_from(Some("4")), DEFAULT_SIZE);
        assert_eq!(size_from(None), DEFAULT_SIZE);
    }

    #[test]
    fn a_pointer_is_painted_over_the_picture() {
        // a 2x2 picture of grey, a 2x1 pointer: one opaque red pixel, one half-transparent white one; put at (1, 1) only its
        // first pixel lands, at (-1, 0) only its second
        let grey = [100u8, 100, 100, 255];
        let pic = Picture { w: 2, h: 1, hot: (0, 0), data: vec![0, 0, 255, 255, 128, 128, 128, 128] };
        let mut dst: Vec<u8> = grey.repeat(4);
        blend(&mut dst, 8, (2, 2), &pic, (1, 1));
        assert_eq!(&dst[12..16], &[0, 0, 255, 255]);
        assert_eq!(&dst[0..12], &grey.repeat(3)[..]);
        let mut dst: Vec<u8> = grey.repeat(4);
        blend(&mut dst, 8, (2, 2), &pic, (-1, 0));
        assert_eq!(&dst[0..4], &[178, 178, 178, 255], "half white over grey");
        assert_eq!(&dst[4..16], &grey.repeat(3)[..]);
    }

    #[test]
    fn the_plain_arrow_has_its_tip_at_the_hotspot() {
        let a = arrow(24);
        assert_eq!(a.hot, (0, 0));
        let drawn = |x: u32, y: u32| a.data[((y * a.w + x) * 4 + 3) as usize] == 255;
        assert!(drawn(0, 1) || drawn(1, 1), "the tip is drawn");
        let white = a.data.chunks(4).filter(|p| p == &[255, 255, 255, 255]).count();
        let black = a.data.chunks(4).filter(|p| p == &[0, 0, 0, 255]).count();
        assert!(white > 30 && black > 30, "filled and outlined: {white} white, {black} black");
    }
}
