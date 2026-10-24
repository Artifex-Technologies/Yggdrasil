//! Log out, restart and shut down in Yggdrasil's own session ask first, with a countdown, as on Windows and in Plasma: a card in
//! the middle of the screen. When the time runs out (30 seconds) it goes ahead by itself, so walking away still ends the session;
//! Cancel or Escape stops it, "now" or Enter does it at once. Going ahead is yggdrasil-logout's job: it asks every window to
//! close first, so programs can save, and locks the screen instead if one does not close.

use std::time::{Duration, Instant};

use resvg::tiny_skia::Pixmap;

use crate::paint::{self, Fonts};
use crate::shell::{Action, Hit, PowerAct};

pub const WIDTH: u32 = 460;
pub const HEIGHT: u32 = 188;
pub const SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug)]
pub struct Confirm {
    pub act: PowerAct,
    pub until: Instant,
}

impl Confirm {
    pub fn new(act: PowerAct) -> Confirm {
        Confirm { act, until: Instant::now() + Duration::from_secs(SECONDS) }
    }

    pub fn left(&self) -> u64 {
        self.until.saturating_duration_since(Instant::now()).as_secs_f32().ceil() as u64
    }

    pub fn expired(&self) -> bool {
        Instant::now() >= self.until
    }

    /// What yggdrasil-logout is asked to do.
    pub fn verb(&self) -> &'static str {
        match self.act {
            PowerAct::Restart => "reboot",
            PowerAct::Shutdown => "poweroff",
            _ => "logout",
        }
    }
}

/// The question, the sentence under it, and the button that goes ahead now.
pub fn words(act: PowerAct, left: u64) -> (&'static str, String, &'static str) {
    let s = if left == 1 { "" } else { "s" };
    match act {
        PowerAct::Restart => ("Restart?", format!("Restarting in {left} second{s}."), "Restart now"),
        PowerAct::Shutdown => ("Shut down?", format!("Shutting down in {left} second{s}."), "Shut down now"),
        _ => ("Log out?", format!("Logging out in {left} second{s}."), "Log out now"),
    }
}

/// Draws the card into `pix` (physical pixels, `s` the scale); returns the buttons (physical pixels).
pub fn paint(pix: &mut Pixmap, s: f32, c: &Confirm, hover: Option<usize>, fonts: &Fonts) -> Vec<Hit> {
    pix.fill(paint::color((0, 0, 0, 0)));
    let (w, h) = (pix.width() as f32 / s, pix.height() as f32 / s);
    paint::fill_rounded(pix, 0.0, 0.0, w * s, h * s, 18.0 * s, (16, 16, 22, 248));
    paint::stroke_rounded(pix, 0.5 * s, 0.5 * s, (w - 1.0) * s, (h - 1.0) * s, 18.0 * s, s, (255, 255, 255, 30));
    let (title, line, now) = words(c.act, c.left());
    paint::draw_text(pix, &fonts.bold, 19.0 * s, 26.0 * s, 40.0 * s, (240, 240, 245, 255), title);
    paint::draw_text(pix, &fonts.regular, 13.5 * s, 26.0 * s, 72.0 * s, (210, 210, 220, 255), &line);
    paint::draw_text(pix, &fonts.regular, 12.0 * s, 26.0 * s, 96.0 * s, (150, 150, 165, 255), "Open programs are asked to close first, so they can save.");
    // the time left, as a thin bar
    let frac = (c.until.saturating_duration_since(Instant::now()).as_secs_f32() / SECONDS as f32).clamp(0.0, 1.0);
    paint::fill_rounded(pix, 26.0 * s, 114.0 * s, (w - 52.0) * s, 4.0 * s, 2.0 * s, (255, 255, 255, 26));
    if frac > 0.0 {
        paint::fill_rounded(pix, 26.0 * s, 114.0 * s, ((w - 52.0) * frac).max(4.0) * s, 4.0 * s, 2.0 * s, paint::accent(255));
    }
    let (bh, by) = (36.0, h - 36.0 - 22.0);
    let now_w = paint::text_width(&fonts.bold, 13.0 * s, now) / s + 36.0;
    let cancel_w = paint::text_width(&fonts.regular, 13.0 * s, "Cancel") / s + 36.0;
    let now_r = (w - 26.0 - now_w, by, now_w, bh);
    let cancel_r = (now_r.0 - 10.0 - cancel_w, by, cancel_w, bh);
    let phys = |r: (f32, f32, f32, f32)| (r.0 * s, r.1 * s, r.2 * s, r.3 * s);
    let hits = vec![Hit { rect: phys(cancel_r), action: Action::ConfirmCancel }, Hit { rect: phys(now_r), action: Action::ConfirmNow }];
    paint::fill_rounded(pix, cancel_r.0 * s, by * s, cancel_w * s, bh * s, 10.0 * s, if hover == Some(0) { (255, 255, 255, 40) } else { (255, 255, 255, 18) });
    paint::draw_text(pix, &fonts.regular, 13.0 * s, (cancel_r.0 + 18.0) * s, (by + bh / 2.0) * s, (235, 235, 240, 255), "Cancel");
    paint::fill_rounded(pix, now_r.0 * s, by * s, now_w * s, bh * s, 10.0 * s, paint::accent(if hover == Some(1) { 255 } else { 220 }));
    paint::draw_text(pix, &fonts.bold, 13.0 * s, (now_r.0 + 18.0) * s, (by + bh / 2.0) * s, (255, 255, 255, 255), now);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_what_and_when() {
        assert_eq!(words(PowerAct::Logout, 30).1, "Logging out in 30 seconds.");
        assert_eq!(words(PowerAct::Shutdown, 1).1, "Shutting down in 1 second.");
        assert_eq!(words(PowerAct::Restart, 5).2, "Restart now");
        assert_eq!(Confirm::new(PowerAct::Shutdown).verb(), "poweroff");
        assert_eq!(Confirm::new(PowerAct::Logout).verb(), "logout");
        let c = Confirm::new(PowerAct::Logout);
        assert!(!c.expired() && (29..=30).contains(&c.left()));
    }

    /// Writes the card to a picture, to look at it: YGG_CONFIRM_PNG=/path/card.png cargo test confirm -- --ignored
    #[test]
    #[ignore]
    fn picture() {
        let Some(out) = std::env::var_os("YGG_CONFIRM_PNG") else { return };
        let fonts = Fonts::load().expect("fonts");
        let s = 2.0;
        let mut pix = Pixmap::new((WIDTH as f32 * s) as u32, (HEIGHT as f32 * s) as u32).unwrap();
        let c = Confirm { act: PowerAct::Logout, until: Instant::now() + Duration::from_secs(21) };
        paint(&mut pix, s, &c, Some(0), &fonts);
        pix.save_png(out).unwrap();
    }
}
