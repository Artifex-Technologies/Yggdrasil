//! Playing inside Huginn: the video's picture as a texture (yggdrasil-player's libVLC layer, so both play with the same code), the
//! controls over it, the small player in the corner while you browse (as YouTube's phone apps do), full screen, and "pop out" into
//! yggdrasil-player's own window from the same second. While something plays the screen does not dim (systemd-inhibit, like the
//! player).

use eframe::egui::{self, pos2, vec2, Align2, Color32, CornerRadius, FontId, Rect, Sense, Stroke, TextureHandle, TextureOptions};
use huginn_core::model::Details;
use yggdrasil_player::vlc::{Player, State};
use yggdrasil_ui::theme::Palette;

pub struct Now {
    pub details: Details,
    pub audio_only: bool,
    vlc: Player,
    tex: Option<TextureHandle>,
    pub volume: i32,
    pub muted: bool,
    /// the seek bar while it is dragged (0-1)
    drag: Option<f32>,
    inhibit: Option<std::process::Child>,
    /// when the pointer last moved over the picture (the controls fade after a while)
    pub last_motion: std::time::Instant,
}

/// What the controls asked for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ask {
    Nothing,
    Minimize,
    PopOut,
    FullScreen,
    Close,
    Expand,
}

/// The control icons, drawn (not font characters, which a font may lack): the same shapes as yggdrasil-player's.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Icon {
    Play,
    Pause,
    Volume,
    Muted,
    FullScreen,
    Windowed,
    PopOut,
    Minimize,
    Close,
}

pub fn icon(p: &egui::Painter, c: egui::Pos2, size: f32, icon: Icon, ink: Color32) {
    let s = size * 0.26;
    let line = Stroke::new(2.0, ink);
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(vec![c + vec2(-s * 0.7, -s), c + vec2(s, 0.0), c + vec2(-s * 0.7, s)], ink, Stroke::NONE));
        }
        Icon::Pause => {
            p.rect_filled(Rect::from_center_size(c - vec2(s * 0.45, 0.0), vec2(s * 0.5, s * 2.0)), 1.5, ink);
            p.rect_filled(Rect::from_center_size(c + vec2(s * 0.45, 0.0), vec2(s * 0.5, s * 2.0)), 1.5, ink);
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
        Icon::FullScreen => {
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let k = c + vec2(sx * s, sy * s);
                p.line_segment([k, k + vec2(-sx * s * 0.6, 0.0)], line);
                p.line_segment([k, k + vec2(0.0, -sy * s * 0.6)], line);
            }
        }
        Icon::Windowed => {
            for (sx, sy) in [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                let k = c + vec2(sx * s * 0.45, sy * s * 0.45);
                p.line_segment([k, k + vec2(sx * s * 0.55, 0.0)], line);
                p.line_segment([k, k + vec2(0.0, sy * s * 0.55)], line);
            }
        }
        Icon::PopOut => {
            // a window, and an arrow out of its top right corner
            let r = Rect::from_center_size(c + vec2(-s * 0.2, s * 0.2), vec2(s * 1.7, s * 1.5));
            p.rect_stroke(r, 2.0, Stroke::new(1.6, ink), egui::StrokeKind::Inside);
            let a = c + vec2(s * 1.05, -s * 1.05);
            p.line_segment([c + vec2(s * 0.05, -s * 0.05), a], line);
            p.line_segment([a, a + vec2(-s * 0.6, 0.0)], line);
            p.line_segment([a, a + vec2(0.0, s * 0.6)], line);
        }
        Icon::Minimize => {
            // a frame with a small filled box in its bottom right corner
            let r = Rect::from_center_size(c, vec2(s * 2.2, s * 1.7));
            p.rect_stroke(r, 2.0, Stroke::new(1.6, ink), egui::StrokeKind::Inside);
            p.rect_filled(Rect::from_min_max(r.center() + vec2(s * 0.05, s * 0.05), r.right_bottom() - vec2(s * 0.25, s * 0.25)), 1.0, ink);
        }
        Icon::Close => {
            p.line_segment([c + vec2(-s * 0.8, -s * 0.8), c + vec2(s * 0.8, s * 0.8)], line);
            p.line_segment([c + vec2(s * 0.8, -s * 0.8), c + vec2(-s * 0.8, s * 0.8)], line);
        }
    }
}

fn stamp(ms: i64) -> String {
    let s = (ms.max(0) / 1000) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

impl Now {
    /// Starts playing (the picture and its sound, or the sound only) from `start` seconds.
    pub fn start(details: Details, audio_only: bool, start: Option<f64>, volume: i32, muted: bool, ctx: &egui::Context) -> Result<Now, String> {
        let s = &details.streams;
        let (v, a) = crate::jobs::sources(s);
        let (main, extra) = if audio_only { (a.clone().or_else(|| v.clone()), None) } else { (v.clone().or_else(|| a.clone()), a.clone().filter(|_| v.is_some())) };
        let main = main.ok_or("no stream to play")?;
        let c = ctx.clone();
        let vlc = Player::open_stream_with(&main, extra.as_deref(), start, s.user_agent.as_deref(), s.referer.as_deref(), Box::new(move || c.request_repaint()))?;
        vlc.play();
        vlc.set_volume(if muted { 0 } else { volume });
        // the screen stays awake while it plays (logind's idle lock, which yggdrasil-idle honours)
        let inhibit = std::process::Command::new("systemd-inhibit")
            .args(["--what=idle", "--who=Huginn", "--why=Playing a video", "--mode=block", "sleep", "infinity"])
            .stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .spawn().ok();
        Ok(Now { details, audio_only, vlc, tex: None, volume, muted, drag: None, inhibit, last_motion: std::time::Instant::now() })
    }

    pub fn id(&self) -> &str {
        &self.details.video.id
    }

    /// Seconds played so far.
    pub fn position(&self) -> f64 {
        self.vlc.time().max(0) as f64 / 1000.0
    }

    pub fn playing(&self) -> bool {
        self.vlc.state() == State::Playing
    }

    pub fn ended(&self) -> bool {
        matches!(self.vlc.state(), State::Ended | State::Error)
    }

    pub fn toggle(&self) {
        self.vlc.set_pause(self.playing());
    }

    pub fn seek_by(&self, seconds: i64) {
        let len = self.vlc.length();
        let t = (self.vlc.time() + seconds * 1000).max(0);
        self.vlc.set_time(if len > 0 { t.min(len - 500) } else { t });
    }

    pub fn seek_to(&self, seconds: f64) {
        self.vlc.set_time((seconds * 1000.0) as i64);
    }

    pub fn set_muted(&mut self, m: bool) {
        self.muted = m;
        self.vlc.set_volume(if m { 0 } else { self.volume });
    }

    /// Takes VLC's newest picture into the texture.
    fn frame(&mut self, ctx: &egui::Context) {
        let img = self.vlc.with_frame(|f| {
            if !f.fresh || f.w == 0 {
                return None;
            }
            f.fresh = false;
            Some(egui::ColorImage::from_rgba_unmultiplied([f.w, f.h], &f.rgba))
        });
        if let Some(Some(img)) = img {
            match &mut self.tex {
                Some(t) => t.set(img, TextureOptions::LINEAR),
                None => self.tex = Some(ctx.load_texture("huginn-video", img, TextureOptions::LINEAR)),
            }
        }
    }

    /// The picture in `rect` (black bars where it does not fill), or the thumbnail with a note while only the sound plays.
    fn picture(&mut self, ui: &egui::Ui, rect: Rect, radius: u8, thumb: Option<&crate::widgets::Tex>, pal: &Palette) {
        ui.painter().rect_filled(rect, CornerRadius::same(radius), Color32::BLACK);
        self.frame(ui.ctx());
        if let (false, Some(t)) = (self.audio_only, &self.tex) {
            let s = t.size_vec2();
            let k = (rect.width() / s.x).min(rect.height() / s.y);
            let r = Rect::from_center_size(rect.center(), s * k);
            ui.painter().image(t.id(), r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
        } else {
            crate::widgets::picture(ui, rect, radius, thumb, pal);
            ui.painter().rect_filled(rect, CornerRadius::same(radius), Color32::from_black_alpha(120));
            let note = if self.audio_only { "\u{266B}  Sound only" } else { "Starting…" };
            ui.painter().text(rect.center(), Align2::CENTER_CENTER, note, FontId::proportional(18.0), Color32::WHITE);
        }
    }

    /// The video page's player: the picture with the controls over its bottom (shown while the pointer moves or it is paused).
    pub fn big(&mut self, ui: &mut egui::Ui, rect: Rect, thumb: Option<&crate::widgets::Tex>, pal: &Palette, full: bool) -> Ask {
        let resp = ui.interact(rect, ui.id().with("huginn-player"), Sense::click());
        self.picture(ui, rect, if full { 0 } else { 16 }, thumb, pal);
        if ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO) && resp.hovered() {
            self.last_motion = std::time::Instant::now();
        }
        let show = !self.playing() || self.drag.is_some() || self.last_motion.elapsed().as_secs_f32() < 2.5;
        let mut ask = Ask::Nothing;
        if resp.double_clicked() {
            return Ask::FullScreen;
        }
        if resp.clicked() {
            self.toggle();
        }
        if !show {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
            if full {
                ui.ctx().set_cursor_icon(egui::CursorIcon::None);
            }
            return ask;
        }
        // the controls: a dark band at the bottom
        let band = Rect::from_min_max(pos2(rect.left(), rect.bottom() - 64.0), rect.right_bottom());
        crate::widgets::gradient_rounded(ui.painter(), band, if full { 0.0 } else { 16.0 }, |p| {
            let f = ((p.y - band.top()) / band.height()).clamp(0.0, 1.0);
            Color32::from_black_alpha((40.0 + 170.0 * f) as u8)
        });
        let (t, len) = (self.vlc.time().max(0), self.vlc.length().max(0));
        // the seek bar
        let bar = Rect::from_min_max(pos2(band.left() + 16.0, band.top() + 14.0), pos2(band.right() - 16.0, band.top() + 22.0));
        let bresp = ui.interact(bar.expand2(vec2(0.0, 6.0)), ui.id().with("huginn-seek"), Sense::click_and_drag());
        let frac = self.drag.unwrap_or(if len > 0 { t as f32 / len as f32 } else { 0.0 });
        ui.painter().rect_filled(Rect::from_min_max(pos2(bar.left(), bar.center().y - 2.0), pos2(bar.right(), bar.center().y + 2.0)), CornerRadius::same(2), Color32::from_white_alpha(70));
        ui.painter().rect_filled(Rect::from_min_max(pos2(bar.left(), bar.center().y - 2.0), pos2(bar.left() + bar.width() * frac, bar.center().y + 2.0)), CornerRadius::same(2), pal.accent);
        if bresp.hovered() || self.drag.is_some() {
            ui.painter().circle_filled(pos2(bar.left() + bar.width() * frac, bar.center().y), 6.0, pal.accent);
        }
        if let Some(p) = bresp.interact_pointer_pos() {
            let f = ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0);
            if bresp.dragged() || bresp.is_pointer_button_down_on() {
                self.drag = Some(f);
            }
        }
        if (bresp.drag_stopped() || bresp.clicked()) && len > 0 {
            if let Some(f) = self.drag.take().or_else(|| bresp.interact_pointer_pos().map(|p| ((p.x - bar.left()) / bar.width()).clamp(0.0, 1.0))) {
                self.vlc.set_time((f as f64 * len as f64) as i64);
            }
        }
        // the buttons: play, the time; on the right mute, minimize, pop out, full screen
        let y = band.bottom() - 22.0;
        let button = |ui: &mut egui::Ui, x: f32, which: Icon, tip: &str| -> bool {
            let r = Rect::from_center_size(pos2(x, y), vec2(32.0, 30.0));
            let resp = ui.interact(r, ui.id().with(("huginn-btn", which)), Sense::click()).on_hover_text(tip);
            if resp.hovered() {
                ui.painter().circle_filled(r.center(), 15.0, Color32::from_white_alpha(36));
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            icon(ui.painter(), r.center(), 30.0, which, Color32::WHITE);
            resp.clicked()
        };
        if button(ui, band.left() + 30.0, if self.playing() { Icon::Pause } else { Icon::Play }, if self.playing() { "Pause (Space)" } else { "Play (Space)" }) {
            self.toggle();
        }
        ui.painter().text(pos2(band.left() + 56.0, y), Align2::LEFT_CENTER, format!("{} / {}", stamp(t), stamp(len)), FontId::monospace(12.5), Color32::from_white_alpha(230));
        let r = band.right();
        if button(ui, r - 30.0, if full { Icon::Windowed } else { Icon::FullScreen }, if full { "Leave full screen (Esc)" } else { "Full screen (F)" }) {
            ask = Ask::FullScreen;
        }
        if button(ui, r - 66.0, Icon::PopOut, "In its own window (yggdrasil-player), from here") {
            ask = Ask::PopOut;
        }
        if !full && button(ui, r - 102.0, Icon::Minimize, "Small player in the corner: keep browsing") {
            ask = Ask::Minimize;
        }
        if button(ui, r - if full { 102.0 } else { 138.0 }, if self.muted { Icon::Muted } else { Icon::Volume }, if self.muted { "Sound on (M)" } else { "Mute (M)" }) {
            let m = !self.muted;
            self.set_muted(m);
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
        ask
    }

    /// The small player in the corner while browsing: the picture, the title, play/pause, close. A click on the picture goes back to the
    /// video's page.
    pub fn mini(&mut self, ctx: &egui::Context, pal: &Palette, thumb: Option<&crate::widgets::Tex>) -> Ask {
        let mut ask = Ask::Nothing;
        let screen = ctx.content_rect();
        let w = 380.0;
        let h = w * 9.0 / 16.0;
        egui::Area::new(egui::Id::new("huginn-mini"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos2(screen.right() - w - 24.0, screen.bottom() - h - 72.0))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(vec2(w, h + 48.0), Sense::hover());
                ui.painter().rect_filled(rect.expand(1.0), CornerRadius::same(13), pal.border);
                ui.painter().rect_filled(rect, CornerRadius::same(12), pal.sidebar);
                let pic = Rect::from_min_size(rect.min, vec2(w, h));
                let presp = ui.interact(pic, ui.id().with("mini-pic"), Sense::click()).on_hover_text("Back to the video");
                self.picture(ui, pic, 12, thumb, pal);
                if presp.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if presp.clicked() {
                    ask = Ask::Expand;
                }
                // the progress line under the picture
                let (t, len) = (self.vlc.time().max(0), self.vlc.length().max(0));
                if len > 0 {
                    let f = t as f32 / len as f32;
                    ui.painter().rect_filled(Rect::from_min_max(pic.left_bottom() - vec2(0.0, 3.0), pos2(pic.left() + pic.width() * f, pic.bottom())), CornerRadius::ZERO, pal.accent);
                }
                let row = Rect::from_min_max(pos2(rect.left(), pic.bottom()), rect.right_bottom());
                let title = crate::widgets::wrapped(ui, &self.details.video.title, 13.0, w - 110.0, 1, pal.text, true);
                ui.painter().galley(pos2(row.left() + 12.0, row.center().y - title.size().y / 2.0), title, pal.text);
                let btn = |ui: &mut egui::Ui, x: f32, which: Icon, tip: &str| -> bool {
                    let r = Rect::from_center_size(pos2(x, row.center().y), vec2(32.0, 30.0));
                    let resp = ui.interact(r, ui.id().with(("mini", which)), Sense::click()).on_hover_text(tip);
                    if resp.hovered() {
                        ui.painter().circle_filled(r.center(), 15.0, pal.hover);
                    }
                    icon(ui.painter(), r.center(), 28.0, which, pal.text);
                    resp.clicked()
                };
                if btn(ui, row.right() - 62.0, if self.playing() { Icon::Pause } else { Icon::Play }, "Play or pause") {
                    self.toggle();
                }
                if btn(ui, row.right() - 26.0, Icon::Close, "Stop") {
                    ask = Ask::Close;
                }
                ui.painter().rect_stroke(rect, CornerRadius::same(12), Stroke::new(1.0, pal.border), egui::StrokeKind::Inside);
            });
        ctx.request_repaint_after(std::time::Duration::from_millis(250));
        ask
    }
}

impl Drop for Now {
    fn drop(&mut self) {
        if let Some(mut c) = self.inhibit.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamps() {
        assert_eq!(stamp(0), "0:00");
        assert_eq!(stamp(61_500), "1:01");
        assert_eq!(stamp(3_753_000), "1:02:33");
        assert_eq!(stamp(-5), "0:00");
    }
}
