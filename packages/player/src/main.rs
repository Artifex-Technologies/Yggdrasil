//! VLC, in Yggdrasil's look (yggdrasil-player): Yggdrasil's video player. VLC (libVLC 3) plays the file; the window around the picture is Yggdrasil's own (the same
//! look as Minos and Settings).
//!
//!   yggdrasil-player [FILE]
//!
//! Keys: Space or K play/pause, Left/Right (J/L) ten seconds back/on, Up/Down volume, M mute, F or a double-click full screen,
//! Esc leaves full screen, O opens a file. A file can also be dropped on the window.

mod vlc;

use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, CursorIcon, FontId, Rect, RichText, Sense, Stroke, StrokeKind, TextureHandle, TextureOptions};
use yggdrasil_ui::chrome;
use yggdrasil_ui::theme::{self, Palette};

/// The bars fade out after this long without the mouse moving (while a video plays).
const IDLE: Duration = Duration::from_millis(2500);
const TOP_H: f32 = 40.0;
const BOTTOM_H: f32 = 74.0;
const STEP_MS: i64 = 10_000;
const VIDEO_TYPES: [&str; 14] = ["mkv", "mp4", "m4v", "avi", "mov", "webm", "wmv", "flv", "mpg", "mpeg", "ts", "m2ts", "ogv", "3gp"];

struct Playing {
    path: PathBuf,
    vlc: vlc::Player,
    tex: Option<TextureHandle>,
    /// logind's "don't dim, don't lock" while it plays; dropping it lets the screen dim again.
    inhibit: Option<zbus::zvariant::OwnedFd>,
}

struct App {
    now: Option<Playing>,
    error: Option<String>,
    picking: Option<Receiver<Option<PathBuf>>>,
    last_motion: Instant,
    last_pointer: Option<egui::Pos2>,
    fullscreen: bool,
    volume: i32,
    muted: bool,
    /// Where the seek bar is being dragged to (the jump happens on letting go).
    scrub: Option<f32>,
}

/// 1:02:03, or 2:03 under an hour.
fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

fn is_video(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| VIDEO_TYPES.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false)
}

fn title_of(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Asks logind to hold off dimming and locking while a video plays (yggdrasil-idle and others honour it).
fn inhibit_idle() -> Option<zbus::zvariant::OwnedFd> {
    let sys = zbus::blocking::Connection::system().ok()?;
    let login = zbus::blocking::Proxy::new(&sys, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").ok()?;
    login.call("Inhibit", &("idle", "VLC", "A video is playing", "block")).ok()
}

/// file:///home/x/My%20Film.mkv -> /home/x/My Film.mkv
fn path_from_uri(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() + 0 && i + 2 <= b.len() - 1 {
            if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).ok()?, 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

/// The desktop's own file chooser (the FileChooser portal; in Yggdrasil it is Minos), for one video.
fn pick_video() -> Option<PathBuf> {
    use std::collections::HashMap;
    use zbus::zvariant::{OwnedValue, Value};
    let conn = zbus::blocking::Connection::session().ok()?;
    let token = format!("yggdrasil_player_{}", std::process::id());
    let sender = conn.unique_name()?.trim_start_matches(':').replace('.', "_");
    let request = format!("/org/freedesktop/portal/desktop/request/{sender}/{token}");
    let proxy = zbus::blocking::Proxy::new(&conn, "org.freedesktop.portal.Desktop", request, "org.freedesktop.portal.Request").ok()?;
    let mut answers = proxy.receive_signal("Response").ok()?;
    let filters: Vec<(&str, Vec<(u32, &str)>)> = vec![("Videos", vec![(1, "video/*")])];
    let mut opts: HashMap<&str, Value> = HashMap::new();
    opts.insert("handle_token", Value::from(token.as_str()));
    opts.insert("filters", Value::from(filters));
    conn.call_method(Some("org.freedesktop.portal.Desktop"), "/org/freedesktop/portal/desktop", Some("org.freedesktop.portal.FileChooser"), "OpenFile", &("", "Open a video", opts)).ok()?;
    let msg = answers.next()?;
    let (code, results): (u32, HashMap<String, OwnedValue>) = msg.body().deserialize().ok()?;
    if code != 0 {
        return None;
    }
    let uris: Vec<String> = results.get("uris").and_then(|v| Vec::<String>::try_from(v.clone()).ok())?;
    uris.first().and_then(|u| path_from_uri(u))
}

impl App {
    fn new(file: Option<PathBuf>, ctx: &egui::Context) -> App {
        let mut app = App { now: None, error: None, picking: None, last_motion: Instant::now(), last_pointer: None, fullscreen: false, volume: 100, muted: false, scrub: None };
        if let Some(f) = file {
            app.open(f, ctx);
        }
        app
    }

    fn open(&mut self, path: PathBuf, ctx: &egui::Context) {
        self.close();
        let c = ctx.clone();
        match vlc::Player::open(&path, Box::new(move || c.request_repaint())) {
            Ok(p) => {
                let mut now = Playing { path, vlc: p, tex: None, inhibit: None };
                now.vlc.play();
                now.inhibit = inhibit_idle();
                now.vlc.set_volume(if self.muted { 0 } else { self.volume });
                ctx.send_viewport_cmd(egui::ViewportCommand::Title(format!("{} — VLC", title_of(&now.path))));
                self.now = Some(now);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("{}: {e}", path.display())),
        }
    }

    fn close(&mut self) {
        self.now = None;
    }

    fn toggle_pause(&mut self) {
        if let Some(n) = self.now.as_mut() {
            match n.vlc.state() {
                vlc::State::Playing => {
                    n.vlc.set_pause(true);
                    n.inhibit = None;
                }
                vlc::State::Ended | vlc::State::Stopped => {
                    n.vlc.set_time(0);
                    n.vlc.play();
                    n.inhibit = inhibit_idle();
                }
                _ => {
                    n.vlc.set_pause(false);
                    n.inhibit = inhibit_idle();
                }
            }
        }
    }

    fn step(&mut self, ms: i64) {
        if let Some(n) = self.now.as_ref() {
            let len = n.vlc.length();
            let t = n.vlc.time() + ms;
            n.vlc.set_time(if len > 0 { t.min(len - 1000) } else { t }.max(0));
        }
    }

    fn set_volume(&mut self, v: i32) {
        self.volume = v.clamp(0, 125);
        self.muted = false;
        if let Some(n) = &self.now {
            n.vlc.set_volume(self.volume);
        }
    }

    fn toggle_mute(&mut self) {
        self.muted = !self.muted;
        if let Some(n) = &self.now {
            n.vlc.set_volume(if self.muted { 0 } else { self.volume });
        }
    }

    fn toggle_fullscreen(&mut self, ctx: &egui::Context) {
        self.fullscreen = !self.fullscreen;
        ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
    }

    fn start_pick(&mut self) {
        if self.picking.is_some() {
            return;
        }
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(pick_video());
        });
        self.picking = Some(rx);
    }

    fn keys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        let pressed = |k: Key| ctx.input(|i| i.key_pressed(k));
        if pressed(Key::Space) || pressed(Key::K) {
            self.toggle_pause();
        }
        if pressed(Key::ArrowLeft) || pressed(Key::J) {
            self.step(-STEP_MS);
        }
        if pressed(Key::ArrowRight) || pressed(Key::L) {
            self.step(STEP_MS);
        }
        if pressed(Key::ArrowUp) {
            self.set_volume(self.volume + 5);
        }
        if pressed(Key::ArrowDown) {
            self.set_volume(self.volume - 5);
        }
        if pressed(Key::M) {
            self.toggle_mute();
        }
        if pressed(Key::F) {
            self.toggle_fullscreen(ctx);
        }
        if pressed(Key::Escape) && self.fullscreen {
            self.toggle_fullscreen(ctx);
        }
        if pressed(Key::O) {
            self.start_pick();
        }
    }

    /// Takes VLC's newest picture into the texture, keeps the saved spot fresh, makes a jump that was waiting.
    fn tick(&mut self, ctx: &egui::Context) {
        let Some(n) = self.now.as_mut() else { return };
        let fresh = n.vlc.with_frame(|f| {
            if !f.fresh || f.w == 0 {
                return None;
            }
            f.fresh = false;
            Some(egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.rgba))
        });
        if let Some(Some(img)) = fresh {
            match n.tex.as_mut() {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => n.tex = Some(ctx.load_texture("video", img, TextureOptions::LINEAR)),
            }
        }
        let state = n.vlc.state();
        if state == vlc::State::Ended {
            n.inhibit = None;
        }
        if state == vlc::State::Error {
            self.error = Some(format!("VLC could not play {}", n.path.display()));
        }
    }

    fn bars_shown(&self, ctx: &egui::Context) -> bool {
        let playing = self.now.as_ref().map(|n| n.vlc.state() == vlc::State::Playing).unwrap_or(false);
        let over_bars = ctx.input(|i| i.pointer.hover_pos()).map(|p| {
            let r = ctx.content_rect();
            p.y < r.top() + TOP_H || p.y > r.bottom() - BOTTOM_H
        });
        !playing || self.last_motion.elapsed() < IDLE || over_bars.unwrap_or(false) || self.scrub.is_some()
    }
}

/// A round icon button drawn with lines (play, pause, ...).
#[derive(Clone, Copy, PartialEq)]
enum Icon {
    Play,
    Pause,
    Back,
    Forward,
    Volume,
    Muted,
    Fullscreen,
    Windowed,
    Open,
}

fn icon_button(ui: &mut egui::Ui, pal: &Palette, icon: Icon, size: f32, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let resp = resp.on_hover_text(tip).on_hover_cursor(CursorIcon::PointingHand);
    let p = ui.painter();
    if resp.hovered() {
        p.circle_filled(rect.center(), size / 2.0, Color32::from_white_alpha(28));
    }
    let c = rect.center();
    let s = size * 0.26;
    let ink = Color32::WHITE;
    let line = Stroke::new(2.0, ink);
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(vec![c + vec2(-s * 0.7, -s), c + vec2(s, 0.0), c + vec2(-s * 0.7, s)], ink, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_center_size(c - vec2(s * 0.45, 0.0), vec2(s * 0.5, s * 2.0)), 1.5, ink);
            p.rect_filled(Rect::from_center_size(c + vec2(s * 0.45, 0.0), vec2(s * 0.5, s * 2.0)), 1.5, ink);
        }
        Icon::Back | Icon::Forward => {
            let d = if icon == Icon::Back { -1.0 } else { 1.0 };
            p.circle_stroke(c, s * 1.15, Stroke::new(1.6, pal.dim));
            p.text(c, Align2::CENTER_CENTER, "10", FontId::proportional(size * 0.30), ink);
            p.add(egui::Shape::convex_polygon(vec![c + vec2(d * s * 0.2, -s * 1.15 - 3.0), c + vec2(d * s * 0.2, -s * 1.15 + 3.0), c + vec2(d * s * 0.75, -s * 1.15)], ink, Stroke::NONE));
        }
        Icon::Volume | Icon::Muted => {
            let body = vec![c + vec2(-s, -s * 0.4), c + vec2(-s * 0.4, -s * 0.4), c + vec2(s * 0.2, -s), c + vec2(s * 0.2, s), c + vec2(-s * 0.4, s * 0.4), c + vec2(-s, s * 0.4)];
            p.add(egui::Shape::convex_polygon(body, ink, Stroke::NONE));
            if icon == Icon::Muted {
                p.line_segment([c + vec2(s * 0.5, -s * 0.45), c + vec2(s * 1.15, s * 0.45)], line);
                p.line_segment([c + vec2(s * 1.15, -s * 0.45), c + vec2(s * 0.5, s * 0.45)], line);
            } else {
                p.add(egui::Shape::Path(egui::epaint::PathShape::line((0..=8).map(|k| {
                    let a = -0.8 + 1.6 * k as f32 / 8.0;
                    c + vec2(s * 0.35 + a.cos() * s * 0.75, a.sin() * s * 0.75)
                }).collect(), line)));
            }
        }
        Icon::Fullscreen | Icon::Windowed => {
            let o = if icon == Icon::Fullscreen { 0.0 } else { s * 0.5 };
            for (sx, sy) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let corner = c + vec2(sx * s, sy * s);
                let toward = |dx: f32, dy: f32| corner + vec2(dx, dy);
                let (a, b) = if icon == Icon::Fullscreen { (vec2(-sx * s * 0.6, 0.0), vec2(0.0, -sy * s * 0.6)) } else { (vec2(-sx * o, 0.0), vec2(0.0, -sy * o)) };
                let k = if icon == Icon::Fullscreen { corner } else { corner + vec2(-sx * o, -sy * o) };
                let _ = toward;
                p.line_segment([k, k + if icon == Icon::Fullscreen { a } else { vec2(sx * o, 0.0) }], line);
                p.line_segment([k, k + if icon == Icon::Fullscreen { b } else { vec2(0.0, sy * o) }], line);
            }
        }
        Icon::Open => {
            let r = Rect::from_center_size(c + vec2(0.0, s * 0.15), vec2(s * 2.2, s * 1.6));
            p.rect_stroke(r, 2.0, Stroke::new(1.6, ink), StrokeKind::Inside);
            p.line_segment([r.left_top() + vec2(0.0, -s * 0.3), r.left_top() + vec2(s * 0.8, -s * 0.3)], Stroke::new(1.6, ink));
        }
    }
    resp
}

/// The seek bar: where it is, how much there is, and a dot to drag. Returns the fraction let go at (to jump to).
fn seek_bar(ui: &mut egui::Ui, pal: &Palette, at: f32, scrub: &mut Option<f32>, length_ms: i64) -> Option<f32> {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 18.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), if resp.hovered() || scrub.is_some() { 6.0 } else { 4.0 }));
    let frac_at = |x: f32| ((x - track.left()) / track.width()).clamp(0.0, 1.0);
    let mut jump = None;
    if resp.dragged() || resp.drag_started() {
        if let Some(p) = resp.interact_pointer_pos() {
            *scrub = Some(frac_at(p.x));
        }
    }
    if resp.drag_stopped() || resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            jump = Some(frac_at(p.x));
        }
        *scrub = None;
    }
    let shown = scrub.unwrap_or(at);
    let p = ui.painter();
    p.rect_filled(track, 3.0, Color32::from_white_alpha(60));
    p.rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * shown, track.bottom())), 3.0, pal.accent);
    if resp.hovered() || scrub.is_some() {
        p.circle_filled(pos2(track.left() + track.width() * shown, track.center().y), 7.0, Color32::WHITE);
        if let (Some(h), true) = (resp.hover_pos(), length_ms > 0) {
            let f = frac_at(h.x);
            resp.clone().on_hover_text_at_pointer(clock((f as f64 * length_ms as f64) as i64));
        }
    }
    jump
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        theme::apply_accessibility(ui.ctx());
        let ctx = ui.ctx().clone();
        let accent = theme::system_accent();
        theme::apply(&ctx, true, accent);
        let pal = theme::palette_for(true, accent);
        if !self.fullscreen {
            chrome::resize_edges(&ctx);
        }

        // a file chosen in the chooser, or dropped on the window
        if let Some(rx) = &self.picking {
            if let Ok(r) = rx.try_recv() {
                self.picking = None;
                if let Some(p) = r {
                    self.open(p, &ctx);
                }
            }
        }
        let dropped: Option<PathBuf> = ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).find(|p| !p.as_os_str().is_empty()));
        if let Some(p) = dropped {
            if is_video(&p) {
                self.open(p, &ctx);
            } else {
                self.error = Some(format!("{} is not a video", p.display()));
            }
        }
        let pointer = ctx.input(|i| i.pointer.latest_pos());
        if pointer != self.last_pointer {
            self.last_pointer = pointer;
            self.last_motion = Instant::now();
        }
        self.keys(&ctx);
        self.tick(&ctx);

        let show_bars = self.bars_shown(&ctx);
        let full = ctx.content_rect();
        let playing_something = self.now.is_some();
        let bg = if playing_something { Color32::BLACK } else { pal.bg };
        let mut actions: Vec<Box<dyn FnOnce(&mut App, &egui::Context)>> = vec![];

        egui::CentralPanel::default().frame(egui::Frame::new().fill(bg)).show(ui, |ui| {
            // the picture, as large as fits
            let video = ui.interact(full, ui.id().with("video"), Sense::click());
            if let Some(n) = &self.now {
                if let Some(t) = &n.tex {
                    let size = t.size_vec2();
                    let k = (full.width() / size.x).min(full.height() / size.y);
                    let r = Rect::from_center_size(full.center(), size * k);
                    ui.painter().image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
                }
                if video.double_clicked() {
                    actions.push(Box::new(|a, c| {
                        a.toggle_pause();
                        a.toggle_fullscreen(c);
                    }));
                } else if video.clicked() {
                    actions.push(Box::new(|a, _| a.toggle_pause()));
                }
            }
            if !show_bars {
                ctx.set_cursor_icon(CursorIcon::None);
            }

            // start screen: open, or go on with something
            if !playing_something {
                let card = Rect::from_center_size(full.center(), vec2(full.width().min(520.0) - 32.0, full.height().min(420.0) - 60.0));
                let mut c = ui.new_child(egui::UiBuilder::new().max_rect(card).layout(egui::Layout::top_down(egui::Align::Center)));
                c.add_space(8.0);
                c.label(RichText::new("VLC").size(26.0).strong().color(pal.text));
                c.label(RichText::new("Open a video, or drop one on this window").color(pal.dim));
                c.add_space(14.0);
                let open = c.add(egui::Button::new(RichText::new("Open a video…").size(15.0).color(pal.on_accent())).fill(pal.accent).corner_radius(CornerRadius::same(8)).min_size(vec2(180.0, 36.0)));
                if open.clicked() {
                    actions.push(Box::new(|a, _| a.start_pick()));
                }
                if let Some(e) = &self.error {
                    c.add_space(12.0);
                    c.label(RichText::new(e).color(pal.red));
                }
            }

            // the top bar: name, open, window buttons (not in full screen)
            if show_bars && !self.fullscreen {
                let top = Rect::from_min_size(full.min, vec2(full.width(), TOP_H));
                ui.painter().rect_filled(top, 0.0, if playing_something { Color32::from_black_alpha(150) } else { pal.bg });
                let mut c = ui.new_child(egui::UiBuilder::new().max_rect(top.shrink2(vec2(10.0, 5.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
                if icon_button(&mut c, &pal, Icon::Open, 28.0, "Open a video (O)").clicked() {
                    actions.push(Box::new(|a, _| a.start_pick()));
                }
                let name = self.now.as_ref().map(|n| title_of(&n.path)).unwrap_or_default();
                c.label(RichText::new(name).color(Color32::from_gray(230)));
                let w = c.available_width() - chrome::controls_width();
                chrome::drag_area(&mut c, w);
                chrome::window_controls(&mut c, &pal);
            }

            // the bottom bar: seek bar, then play, jumps, time, volume, tracks, full screen
            if show_bars && playing_something {
                let n = self.now.as_ref().unwrap();
                let bottom = Rect::from_min_max(pos2(full.left(), full.bottom() - BOTTOM_H), full.max);
                ui.painter().rect_filled(bottom, 0.0, Color32::from_black_alpha(160));
                let mut c = ui.new_child(egui::UiBuilder::new().max_rect(bottom.shrink2(vec2(16.0, 6.0))).layout(egui::Layout::top_down(egui::Align::Min)));
                let (t, len) = (n.vlc.time().max(0), n.vlc.length());
                let at = if len > 0 { (t as f64 / len as f64) as f32 } else { 0.0 };
                if let Some(f) = seek_bar(&mut c, &pal, at, &mut self.scrub, len) {
                    actions.push(Box::new(move |a, _| {
                        if let Some(n) = &a.now {
                            n.vlc.set_time((f as f64 * n.vlc.length() as f64) as i64);
                        }
                    }));
                }
                c.horizontal(|c| {
                    let playing = n.vlc.state() == vlc::State::Playing;
                    if icon_button(c, &pal, if playing { Icon::Pause } else { Icon::Play }, 38.0, if playing { "Pause (Space)" } else { "Play (Space)" }).clicked() {
                        actions.push(Box::new(|a, _| a.toggle_pause()));
                    }
                    if icon_button(c, &pal, Icon::Back, 34.0, "10 seconds back (Left)").clicked() {
                        actions.push(Box::new(|a, _| a.step(-STEP_MS)));
                    }
                    if icon_button(c, &pal, Icon::Forward, 34.0, "10 seconds on (Right)").clicked() {
                        actions.push(Box::new(|a, _| a.step(STEP_MS)));
                    }
                    let shown = self.scrub.map(|f| (f as f64 * len as f64) as i64).unwrap_or(t);
                    c.label(RichText::new(format!("{} / {}", clock(shown), clock(len))).color(Color32::from_gray(225)).monospace());
                    c.with_layout(egui::Layout::right_to_left(egui::Align::Center), |c| {
                        if icon_button(c, &pal, if self.fullscreen { Icon::Windowed } else { Icon::Fullscreen }, 34.0, "Full screen (F)").clicked() {
                            actions.push(Box::new(|a, c| a.toggle_fullscreen(c)));
                        }
                        let subs = n.vlc.subtitles();
                        if subs.len() > 1 {
                            let cur = n.vlc.subtitle();
                            c.menu_button(RichText::new("Subtitles").color(Color32::WHITE), |m| {
                                for (id, name) in subs {
                                    let label = if id < 0 { "Off".to_string() } else { name };
                                    if m.selectable_label(id == cur, label).clicked() {
                                        actions.push(Box::new(move |a, _| {
                                            if let Some(n) = &a.now {
                                                n.vlc.set_subtitle(id);
                                            }
                                        }));
                                        m.close();
                                    }
                                }
                            });
                        }
                        let audio = n.vlc.audio_tracks();
                        if audio.len() > 2 {
                            let cur = n.vlc.audio_track();
                            c.menu_button(RichText::new("Audio").color(Color32::WHITE), |m| {
                                for (id, name) in audio.into_iter().filter(|(id, _)| *id >= 0) {
                                    if m.selectable_label(id == cur, name).clicked() {
                                        actions.push(Box::new(move |a, _| {
                                            if let Some(n) = &a.now {
                                                n.vlc.set_audio_track(id);
                                            }
                                        }));
                                        m.close();
                                    }
                                }
                            });
                        }
                        // volume: a short bar, the speaker mutes
                        let (vr, vresp) = c.allocate_exact_size(vec2(96.0, 18.0), Sense::click_and_drag());
                        let track = Rect::from_center_size(vr.center(), vec2(vr.width(), 4.0));
                        let level = if self.muted { 0.0 } else { self.volume as f32 / 125.0 };
                        c.painter().rect_filled(track, 2.0, Color32::from_white_alpha(60));
                        c.painter().rect_filled(Rect::from_min_max(track.min, pos2(track.left() + track.width() * level, track.bottom())), 2.0, pal.accent);
                        if let Some(p) = vresp.interact_pointer_pos().filter(|_| vresp.dragged() || vresp.clicked()) {
                            let v = (((p.x - track.left()) / track.width()).clamp(0.0, 1.0) * 125.0).round() as i32;
                            actions.push(Box::new(move |a, _| a.set_volume(v)));
                        }
                        vresp.on_hover_text(format!("Volume {}% (Up/Down)", if self.muted { 0 } else { self.volume * 100 / 100 }));
                        if icon_button(c, &pal, if self.muted || self.volume == 0 { Icon::Muted } else { Icon::Volume }, 32.0, "Mute (M)").clicked() {
                            actions.push(Box::new(|a, _| a.toggle_mute()));
                        }
                    });
                });
            }
        });
        for a in actions {
            a(self, &ctx);
        }
        if !self.fullscreen {
            chrome::outline(&ctx, &pal);
        }
        // the clock and the fading bars move on without a new picture (paused, or a picture not yet in)
        if self.now.is_some() {
            ctx.request_repaint_after(if show_bars { Duration::from_millis(250) } else { IDLE });
        }
    }

    fn on_exit(&mut self) {
        self.close();
    }
}

fn main() -> eframe::Result {
    let file = std::env::args_os().nth(1).map(PathBuf::from).map(|p| if p.to_string_lossy().starts_with("file://") { path_from_uri(&p.to_string_lossy()).unwrap_or(p) } else { p });
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1100.0, 660.0])
            .with_min_inner_size([480.0, 300.0])
            .with_title("VLC")
            .with_app_id("yggdrasil-player")
            .with_drag_and_drop(true)
            .with_decorations(false),
        ..Default::default()
    };
    eframe::run_native("VLC", options, Box::new(move |cc| Ok(Box::new(App::new(file, &cc.egui_ctx)))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_become_paths() {
        assert_eq!(path_from_uri("file:///home/a/My%20Film%20(2004).mkv"), Some(PathBuf::from("/home/a/My Film (2004).mkv")));
        assert_eq!(path_from_uri("file:///x/%C3%A9t%C3%A9.mp4"), Some(PathBuf::from("/x/été.mp4")));
        assert_eq!(path_from_uri("https://x"), None);
    }

    #[test]
    fn times_read_like_a_clock() {
        assert_eq!(clock(3_723_000), "1:02:03");
        assert_eq!(clock(123_000), "2:03");
        assert_eq!(clock(-5), "0:00");
    }

    #[test]
    fn video_files_are_recognised_by_their_ending() {
        assert!(is_video(Path::new("/a/b.MKV")));
        assert!(!is_video(Path::new("/a/b.txt")));
    }
}
