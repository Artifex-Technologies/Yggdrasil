//! The volume and brightness pop-up: a small black pill a little above the dock, drawn by the panel. It follows the real
//! changes: the output's volume (read from PipeWire when it changes) and the screen's brightness (the backlight's own file,
//! whoever changed it: the brightness keys through brightnessctl, Settings, another program).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use resvg::tiny_skia::Pixmap;

use crate::paint::{self, Fonts, Icons};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OsdKind {
    Volume,
    Brightness,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OsdState {
    pub kind: OsdKind,
    pub percent: u32,
    pub muted: bool,
}

pub const WIDTH: u32 = 280;
pub const HEIGHT: u32 = 52;

/// The icon for what is shown.
pub fn icon_for(st: &OsdState) -> &'static str {
    match st.kind {
        OsdKind::Volume if st.muted || st.percent == 0 => "audio-volume-muted-symbolic",
        OsdKind::Volume if st.percent < 34 => "audio-volume-low-symbolic",
        OsdKind::Volume if st.percent < 67 => "audio-volume-medium-symbolic",
        OsdKind::Volume => "audio-volume-high-symbolic",
        OsdKind::Brightness => "display-brightness-symbolic",
    }
}

/// Draws the pill into `pix` (physical pixels, `s` the scale).
pub fn paint(pix: &mut Pixmap, s: f32, st: &OsdState, fonts: &Fonts, icons: &mut Icons) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let (w, h) = (pix.width() as f32 / s, pix.height() as f32 / s);
    paint::fill_rounded(pix, 0.0, 0.0, w * s, h * s, 14.0 * s, (0, 0, 0, 255));
    paint::stroke_rounded(pix, 0.5 * s, 0.5 * s, (w - 1.0) * s, (h - 1.0) * s, 14.0 * s, s, (255, 255, 255, 30));
    let px = (22.0 * s) as u32;
    if let Some(ic) = icons.get_tinted(icon_for(st), px, (255, 255, 255, 255)) {
        paint::draw_pixmap(pix, ic, (18.0 * s) as i32, ((h - 22.0) / 2.0 * s) as i32);
    }
    // the level
    let (bx, bw) = (54.0, w - 54.0 - 62.0);
    let cy = h / 2.0;
    paint::fill_rounded(pix, bx * s, (cy - 3.0) * s, bw * s, 6.0 * s, 3.0 * s, (255, 255, 255, 36));
    let frac = if st.muted { 0.0 } else { (st.percent.min(100) as f32) / 100.0 };
    if frac > 0.0 {
        paint::fill_rounded(pix, bx * s, (cy - 3.0) * s, (bw * frac).max(6.0) * s, 6.0 * s, 3.0 * s, paint::accent(255));
    }
    let label = if st.muted { "Muted".to_string() } else { format!("{}%", st.percent) };
    let lw = paint::text_width(&fonts.bold, 13.0 * s, &label);
    paint::draw_text(pix, &fonts.bold, 13.0 * s, (w - 18.0) * s - lw, cy * s, (255, 255, 255, 255), &label);
}

fn read_volume() -> Option<(u32, bool)> {
    let out =
        Command::new("wpctl").args(["get-volume", "@DEFAULT_AUDIO_SINK@"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    crate::fylgja::parse_wpctl(&String::from_utf8_lossy(&out.stdout))
}

/// Watches the output's volume and the screen's brightness on their own threads and sends each change.
pub fn spawn_watchers(tx: calloop::channel::Sender<OsdState>) {
    let vtx = tx.clone();
    std::thread::spawn(move || {
        // pactl ends when the sound server restarts (a new device, the voice warming up): it is started again, or the pop-up was
        // gone for good until the panel restarted (seen 2026-10-26: no pactl child, no volume pop-up)
        let mut last = read_volume();
        loop {
            let mut pactl = Command::new("pactl");
            pactl.arg("subscribe").stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null());
            let Ok(mut child) = ends_with_us(&mut pactl).spawn() else {
                std::thread::sleep(std::time::Duration::from_secs(5));
                continue;
            };
            let Some(out) = child.stdout.take() else { return };
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                // "Event 'change' on sink #53" (not a sink-input: that is one app's volume)
                if !(line.contains("'change' on sink #") || line.contains("'change' on server")) {
                    continue;
                }
                let now = read_volume();
                if now != last {
                    last = now;
                    if let Some((percent, muted)) = now {
                        if vtx.send(OsdState { kind: OsdKind::Volume, percent, muted }).is_err() {
                            return;
                        }
                    }
                }
            }
            let _ = child.wait();
            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
    std::thread::spawn(move || {
        // sysfs announces no change of a backlight's level, so it is read four times a second; only where there is a backlight
        // (a laptop or a monitor with one): a desktop PC has none and this thread ends at once
        let Some(dir) = backlight() else { return };
        let read = |f: &str| std::fs::read_to_string(dir.join(f)).ok().and_then(|t| t.trim().parse::<u32>().ok());
        let Some(max) = read("max_brightness").filter(|&m| m > 0) else { return };
        let mut last = read("brightness");
        loop {
            std::thread::sleep(std::time::Duration::from_millis(250));
            let now = read("brightness");
            if now != last {
                last = now;
                if let Some(n) = now {
                    if tx.send(OsdState { kind: OsdKind::Brightness, percent: brightness_percent(n, max), muted: false }).is_err() {
                        return;
                    }
                }
            }
        }
    });
}

/// A child that ends with the thread that starts it (and so with the panel): `pactl subscribe` runs as long as the panel does,
/// and once outlived it, keeping the panel's unit "active (running)" with no panel (2026-11-13, after Hyprland restarted).
fn ends_with_us(c: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    let parent = std::process::id() as libc::pid_t;
    // SAFETY: prctl, getppid and _exit are async-signal-safe and touch no memory of the parent's
    unsafe {
        c.pre_exec(move || {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM);
            // the panel ended between the fork and the prctl: nothing will send the signal now
            if libc::getppid() != parent {
                libc::_exit(0);
            }
            Ok(())
        })
    }
}

/// The screen's backlight: the graphics driver's own ("raw", intel_backlight say, the one brightnessctl changes) before the
/// firmware's and a vendor's.
fn backlight() -> Option<std::path::PathBuf> {
    let mut all: Vec<std::path::PathBuf> = std::fs::read_dir("/sys/class/backlight").ok()?.flatten().map(|e| e.path()).collect();
    all.sort_by_key(|p| match std::fs::read_to_string(p.join("type")).unwrap_or_default().trim() {
        "raw" => 0,
        "firmware" => 1,
        _ => 2,
    });
    all.into_iter().next()
}

fn brightness_percent(now: u32, max: u32) -> u32 {
    ((now.min(max) as f32 / max.max(1) as f32) * 100.0).round() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_watcher_ends_with_the_thread_that_started_it() {
        let pid = std::thread::spawn(|| {
            let mut c = Command::new("sleep");
            c.arg("30").stdin(Stdio::null());
            ends_with_us(&mut c).spawn().expect("sleep").id() as libc::pid_t
        })
        .join()
        .unwrap();
        let mut status = 0;
        // SAFETY: waitpid on our own child
        assert_eq!(unsafe { libc::waitpid(pid, &mut status, 0) }, pid);
        assert!(libc::WIFSIGNALED(status) && libc::WTERMSIG(status) == libc::SIGTERM, "status {status:#x}");
    }

    #[test]
    fn brightness_is_a_percentage_of_the_backlights_range() {
        assert_eq!((brightness_percent(0, 255), brightness_percent(128, 255), brightness_percent(300, 255)), (0, 50, 100));
        assert_eq!(brightness_percent(5, 0), 0, "no range: nothing to show");
    }

    #[test]
    fn icons_follow_the_level() {
        let v = |percent, muted| OsdState { kind: OsdKind::Volume, percent, muted };
        assert_eq!(icon_for(&v(0, false)), "audio-volume-muted-symbolic");
        assert_eq!(icon_for(&v(80, true)), "audio-volume-muted-symbolic");
        assert_eq!(icon_for(&v(20, false)), "audio-volume-low-symbolic");
        assert_eq!(icon_for(&v(50, false)), "audio-volume-medium-symbolic");
        assert_eq!(icon_for(&v(90, false)), "audio-volume-high-symbolic");
        assert_eq!(icon_for(&OsdState { kind: OsdKind::Brightness, percent: 40, muted: false }), "display-brightness-symbolic");
    }
}
