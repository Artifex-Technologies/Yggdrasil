//! The voice "island": a black pill that grows out of the notch while Sarina listens, thinks or speaks, with a waveform that
//! follows the sound: your voice while you hold the shortcut, hers while she answers.
//!
//! The wave is the classic iOS 9 Siri wave (the algorithm of PXSiriWave9 / SiriWave.js, Apache-2.0): a few sine curves that
//! share one amplitude, each squeezed towards the ends by the attenuation g(x) = (K / (K + x^4))^K, K = 4, drawn with additive
//! blending so where curves cross they glow. Written again here for tiny-skia; nothing from those projects is copied.

use std::path::Path;

use resvg::tiny_skia::{self, BlendMode, Paint, PathBuilder, Pixmap, Stroke, Transform};

use crate::paint::{self, Rgba};

pub const WIDTH: u32 = 420;
pub const HEIGHT: u32 = 104;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Recording: the wave follows the microphone.
    Listening,
    /// Sarina is working: a slow, small wave.
    Thinking,
    /// Sarina is talking: the wave follows her voice.
    Speaking,
}

/// What the island shows right now.
#[derive(Clone, Copy, Debug)]
pub struct Viz {
    pub mode: Mode,
    /// Smoothed loudness 0..1.
    pub level: f32,
    /// Moves the curves sideways: advances with time.
    pub phase: f32,
    /// How visible the island is (0..1), eased in and out.
    pub shown: f32,
}

/// The three curve colours.
pub fn palette(index: u8) -> [Rgba; 3] {
    match index {
        1 => [(32, 133, 252, 255), (94, 252, 169, 255), (253, 71, 103, 255)],     // Siri
        2 => [(150, 100, 240, 255), (196, 150, 255, 255), (236, 222, 255, 255)],  // Labyrinth: the OS purple (accent 124,84,214), lighter and lighter
        _ => [(214, 120, 52, 255), (255, 178, 92, 255), (255, 226, 176, 255)],    // copper, like the ribbon the owner chose
    }
}

/// Gives the curve its calm ends: 1 in the middle, near 0 at the sides. x runs from -2 to 2.
pub fn attenuation(x: f32) -> f32 {
    let k = 4.0f32;
    (k / (k + x.powi(4))).powf(k)
}

/// The height (in -1..1 of the full amplitude) of curve `i` at x.
pub fn curve(i: usize, x: f32, phase: f32) -> f32 {
    // (frequency, speed, amplitude factor, phase offset)
    const C: [(f32, f32, f32, f32); 4] = [(1.1, 1.0, 1.0, 0.0), (1.6, 1.35, 0.7, 1.7), (2.3, 0.8, 0.5, 3.1), (0.8, 1.7, 0.35, 4.4)];
    let (f, sp, a, off) = C[i % C.len()];
    a * (std::f32::consts::TAU * 0.5 * f * x - phase * sp + off).sin() * attenuation(x)
}

/// Draws the island into `pix` (the whole surface, WIDTH x HEIGHT logical pixels times `s`).
pub fn paint(pix: &mut Pixmap, s: f32, v: &Viz, colors: [Rgba; 3]) {
    let (w, h) = (pix.width() as f32, pix.height() as f32);
    pix.fill(paint::color((0, 0, 0, 0)));
    let a = v.shown.clamp(0.0, 1.0);
    if a <= 0.01 {
        return;
    }
    // the pill grows out of the notch: it hangs from the top edge and its height follows `shown`
    // the surface spans the screen; the pill is WIDTH wide, centred
    let full = (WIDTH as f32 * s).min(w);
    let pw = full * (0.55 + 0.45 * a);
    let ph = (12.0 * s) + (h - 12.0 * s) * a;
    let px = (w - pw) / 2.0;
    paint::fill_rounded(pix, px, -20.0 * s, pw, 20.0 * s + ph, 30.0 * s, (0, 0, 0, 255));
    if a < 0.6 {
        return;
    }
    let fade = ((a - 0.6) / 0.4).clamp(0.0, 1.0);
    let cy = ph * 0.58;
    let amp_px = ph * 0.30 * (0.08 + 0.92 * v.level.clamp(0.0, 1.0)) * if v.mode == Mode::Thinking { 0.55 } else { 1.0 };
    let half = pw * 0.40;
    let n = 120;
    // the thin line in the middle that is always there
    let mut mid = PathBuilder::new();
    mid.move_to(w / 2.0 - half, cy);
    mid.line_to(w / 2.0 + half, cy);
    if let Some(p) = mid.finish() {
        let mut pt = Paint::default();
        pt.set_color(paint::color((255, 255, 255, (70.0 * fade) as u8)));
        pt.anti_alias = true;
        pix.stroke_path(&p, &pt, &Stroke { width: 1.0 * s, ..Default::default() }, Transform::identity(), None);
    }
    for (i, col) in [colors[0], colors[1], colors[2], colors[0]].iter().enumerate() {
        let mut pb = PathBuilder::new();
        for k in 0..=n {
            let t = k as f32 / n as f32;
            let x = (t - 0.5) * 4.0; // -2 .. 2
            let y = cy - amp_px * curve(i, x, v.phase);
            let px = w / 2.0 - half + 2.0 * half * t;
            if k == 0 {
                pb.move_to(px, y);
            } else {
                pb.line_to(px, y);
            }
        }
        let Some(path) = pb.finish() else { continue };
        let alpha = |f: f32| ((col.3 as f32) * f * fade) as u8;
        // a soft wide stroke for the glow, then the line itself; additive, so crossings brighten
        for (width, f) in [(5.0 * s, 0.16), (2.4 * s, 0.35), (1.3 * s, 0.95)] {
            let mut pt = Paint::default();
            pt.set_color(paint::color((col.0, col.1, col.2, alpha(f))));
            pt.anti_alias = true;
            pt.blend_mode = BlendMode::Plus;
            let stroke = Stroke { width, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Default::default() };
            pix.stroke_path(&path, &pt, &stroke, Transform::identity(), None);
        }
    }
}

/// How loud the last ~120 ms of a recording being written are (0..1). `pw-record` writes a WAV (16 kHz, mono, s16) that grows as
/// it records, so the end of the file is the newest sound.
pub fn mic_level(path: &Path) -> f32 {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else { return 0.0 };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return 0.0 };
    if len < 44 + 4000 {
        return 0.0;
    }
    let mut buf = vec![0u8; 3840];
    if f.seek(SeekFrom::Start((len - 3840) & !1)).is_err() || f.read_exact(&mut buf).is_err() {
        return 0.0;
    }
    level_of(&buf)
}

/// RMS of 16-bit little-endian samples, mapped to a pleasant 0..1 (quiet room ~0, speaking ~0.5 to 1).
pub fn level_of(bytes: &[u8]) -> f32 {
    let n = bytes.len() / 2;
    if n == 0 {
        return 0.0;
    }
    let sum: f64 = bytes.chunks_exact(2).map(|c| { let v = i16::from_le_bytes([c[0], c[1]]) as f64 / 32768.0; v * v }).sum();
    let rms = (sum / n as f64).sqrt() as f32;
    let db = 20.0 * rms.max(1e-5).log10();       // about -100 .. 0
    ((db + 55.0) / 40.0).clamp(0.0, 1.0)          // -55 dB is silence, -15 dB is loud
}

/// The loudness of speech at `ms` after it started, from the envelope the speaking script wrote (one byte, 0..100, per 50 ms).
pub fn envelope_at(env: &[u8], ms: u64) -> Option<f32> {
    let i = (ms / 50) as usize;
    let a = *env.get(i)? as f32 / 100.0;
    let b = env.get(i + 1).map(|v| *v as f32 / 100.0).unwrap_or(a);
    let f = (ms % 50) as f32 / 50.0;
    Some(a + (b - a) * f)
}

/// Eases the shown level: quick to rise, slow to fall.
pub fn smooth(prev: f32, raw: f32) -> f32 {
    if raw > prev { prev + (raw - prev) * 0.6 } else { prev + (raw - prev) * 0.15 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ends_of_the_curve_are_calm_and_the_middle_is_free() {
        assert!((attenuation(0.0) - 1.0).abs() < 1e-6);
        assert!(attenuation(2.0) < 0.02);
        assert!(attenuation(0.7) > attenuation(1.4));
    }

    #[test]
    fn loudness_of_silence_and_of_speech() {
        assert_eq!(level_of(&[0u8; 400]), 0.0);
        let loud: Vec<u8> = (0..200).flat_map(|i| (if i % 2 == 0 { 9000i16 } else { -9000 }).to_le_bytes()).collect();
        let l = level_of(&loud);
        assert!(l > 0.6 && l <= 1.0, "{l}");
        let soft: Vec<u8> = (0..200).flat_map(|i| (if i % 2 == 0 { 300i16 } else { -300 }).to_le_bytes()).collect();
        assert!(level_of(&soft) < l);
    }

    #[test]
    fn the_envelope_is_read_by_time_and_runs_out() {
        let env = [0u8, 100, 50];
        assert_eq!(envelope_at(&env, 0), Some(0.0));
        assert_eq!(envelope_at(&env, 50), Some(1.0));
        assert!((envelope_at(&env, 25).unwrap() - 0.5).abs() < 1e-6);
        assert_eq!(envelope_at(&env, 150), None);
    }

    #[test]
    fn smoothing_rises_fast_and_falls_slowly() {
        assert!(smooth(0.0, 1.0) > 0.5);
        assert!(smooth(1.0, 0.0) > 0.8);
    }

    #[test]
    fn it_draws_something_and_stays_inside_the_surface() {
        let mut pix = Pixmap::new(420, 104).unwrap();
        paint(&mut pix, 1.0, &Viz { mode: Mode::Listening, level: 0.8, phase: 1.0, shown: 1.0 }, palette(0));
        let lit = pix.data().chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(lit > 5000, "the island is drawn ({lit} pixels)");
        let mut none = Pixmap::new(420, 104).unwrap();
        paint(&mut none, 1.0, &Viz { mode: Mode::Thinking, level: 0.0, phase: 0.0, shown: 0.0 }, palette(0));
        assert_eq!(none.data().chunks_exact(4).filter(|p| p[3] > 0).count(), 0, "nothing when hidden");
    }

    /// `LABYRINTH_VIZ_PNG=/tmp/x cargo test render_pictures` writes pictures of the island to look at.
    #[test]
    fn render_pictures() {
        let Some(dir) = std::env::var_os("LABYRINTH_VIZ_PNG") else { return };
        for (name, level, shown, pal, phase) in [("copper-loud", 0.9, 1.0, 0u8, 2.0f32), ("copper-quiet", 0.25, 1.0, 0, 5.0), ("siri-loud", 0.9, 1.0, 1, 2.0), ("labyrinth-mid", 0.55, 1.0, 2, 3.5), ("growing", 0.5, 0.7, 0, 1.0)] {
            let mut pix = Pixmap::new(840, 208).unwrap(); // 2x
            // a desktop-ish backdrop so the black island reads as it will
            pix.fill(tiny_skia::Color::from_rgba8(70, 50, 120, 255));
            paint(&mut pix, 2.0, &Viz { mode: Mode::Speaking, level, phase, shown }, palette(pal));
            pix.save_png(std::path::Path::new(&dir).join(format!("{name}.png"))).unwrap();
        }
    }
}
