//! What a laptop adds to the hardware backend (udev.rs), the parts that need no hardware to be worked out (and so are tested
//! here):
//! - **The internal screen**: a connector named eDP, LVDS or DSI is the laptop's own panel (`is_internal`).
//! - **The lid** (libinput's lid switch): closed while another screen is connected and on, the panel is taken off its CRTC and
//!   its windows and panels move as when a screen is unplugged; opened, it comes back with its settings (`lid_takes_off`).
//!   Closed with no other screen, the panel stays, nothing is drawn on it and its CRTC is switched off: logind suspends (its
//!   HandleLidSwitch), and when it does not (set to ignore, or an inhibitor) the panel is dark.
//! - **The tablet-mode switch** (a 2-in-1 folded back): recorded and shown by the control socket's `switches`; nothing follows
//!   from it yet. Both switches are known from the start (`Switches::device_added`).
//! - **The default scale** of a screen Settings' monitors.lua gives none (`default_scale`): from its size and resolution.
//! - **Waking from suspend** (`SleepWatch`): the kernel cancels a realtime timer set with TFD_TIMER_CANCEL_ON_SET when the
//!   system resumes (and when the clock is set); the time the system was suspended (CLOCK_BOOTTIME runs on through a suspend,
//!   CLOCK_MONOTONIC does not) tells a resume from a clock set. No D-Bus and no logind needed, nothing woken while nothing
//!   happens; libseat does not pause the session for a suspend to idle (s2idle), so this is how the backend knows.

use std::os::fd::{AsFd, BorrowedFd, FromRawFd, OwnedFd};
use std::time::Duration;

/// A laptop's own panel: eDP (embedded DisplayPort), LVDS (older laptops) or DSI (tablets and small boards), by the kernel's
/// connector name.
pub fn is_internal(name: &str) -> bool {
    let kind = name.rsplit_once('-').map(|(k, _)| k).unwrap_or(name);
    matches!(kind, "eDP" | "LVDS" | "DSI")
}

/// The sizes some EDIDs give instead of the screen's size (its aspect ratio, in centimetres or millimetres): projectors and
/// some TVs. Such a size says nothing.
const ASPECT_AS_SIZE: [(i32, i32); 6] = [(16, 9), (16, 10), (160, 90), (160, 100), (1600, 900), (1600, 1000)];

/// The scale a screen gets when Settings' monitors.lua gives it none, from its size (millimetres, as its EDID says) and
/// resolution, in Settings' steps of 0.25: the screen's pixels per inch over the density things look right at, 135 for a screen
/// under 20 inches (a laptop's, seen up close) and 110 for a larger one (on a desk, further away), the rule GNOME uses. A 14-inch
/// 1920x1200 panel (161 per inch) is 1.25, as I set it in Settings on the laptop's Hyprland session; a 27-inch 4K monitor 1.5;
/// 1920x1080 at 24 inches and 3440x1440 at 34 inches 1. From 1 to 3, and never so large that less than 800x480 is left. An
/// unknown size (0, projectors, an aspect ratio instead of a size) is 1.
pub fn default_scale(size_mm: (i32, i32), mode: (i32, i32)) -> f64 {
    let ((wmm, hmm), (w, h)) = (size_mm, mode);
    if wmm <= 0 || hmm <= 0 || w <= 0 || h <= 0 || ASPECT_AS_SIZE.contains(&(wmm, hmm)) {
        return 1.0;
    }
    let inches = ((wmm as f64).powi(2) + (hmm as f64).powi(2)).sqrt() / 25.4;
    let dpi = ((w as f64).powi(2) + (h as f64).powi(2)).sqrt() / inches;
    let target = if inches < 20.0 { 135.0 } else { 110.0 };
    let mut scale = ((dpi / target) * 4.0).round() / 4.0;
    scale = scale.clamp(1.0, 3.0);
    while scale > 1.0 && (w as f64 / scale < 800.0 || h as f64 / scale < 480.0) {
        scale -= 0.25;
    }
    scale
}

/// The internal screens the lid takes off their CRTC: with the lid closed, every internal one while another screen is connected
/// and on (`screens`: each connected screen's name and whether it is on, that is not switched off in Settings). None with the
/// lid open, and none when the internal screens are all there is (logind's to suspend; they stay, dark).
pub fn lid_takes_off(lid_closed: bool, screens: &[(String, bool)]) -> Vec<String> {
    if !lid_closed || !screens.iter().any(|(n, on)| *on && !is_internal(n)) {
        return vec![];
    }
    screens.iter().filter(|(n, _)| is_internal(n)).map(|(n, _)| n.clone()).collect()
}

/// The lid and the tablet-mode switch as libinput last told them (None: no such switch seen). libinput has no way to ask a
/// switch's state: it tells one that is on (the lid closed, tablet mode) with a toggle right after its device appears, so a
/// switch whose device appeared is off (the lid open) until told otherwise (`device_added`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Switches {
    pub lid_closed: Option<bool>,
    pub tablet_mode: Option<bool>,
}

impl Switches {
    /// The control socket's `switches`: "lid open|closed|unknown" and "tablet-mode on|off|unknown".
    pub fn describe(&self) -> Vec<String> {
        let lid = match self.lid_closed {
            Some(true) => "closed",
            Some(false) => "open",
            None => "unknown",
        };
        let tablet = match self.tablet_mode {
            Some(true) => "on",
            Some(false) => "off",
            None => "unknown",
        };
        vec![format!("lid {lid}"), format!("tablet-mode {tablet}")]
    }

    pub fn lid_closed(&self) -> bool {
        self.lid_closed == Some(true)
    }

    /// A switch device appeared with a lid switch and/or a tablet-mode switch: each is off (open) until libinput's toggle says
    /// it is on, so `switches` tells the state from the start. A switch already told stays as it is.
    pub fn device_added(&mut self, lid: bool, tablet_mode: bool) {
        if lid && self.lid_closed.is_none() {
            self.lid_closed = Some(false);
        }
        if tablet_mode && self.tablet_mode.is_none() {
            self.tablet_mode = Some(false);
        }
    }
}

/// The time the system has spent suspended since it started: CLOCK_BOOTTIME (which counts a suspend) less CLOCK_MONOTONIC
/// (which does not).
pub fn suspended_so_far() -> Duration {
    let read = |clock| {
        let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        // SAFETY: clock_gettime writes into the timespec given
        unsafe { libc::clock_gettime(clock, &mut t) };
        Duration::new(t.tv_sec.max(0) as u64, t.tv_nsec.clamp(0, 999_999_999) as u32)
    };
    read(libc::CLOCK_BOOTTIME).saturating_sub(read(libc::CLOCK_MONOTONIC))
}

/// How long the system slept between two looks at `suspended_so_far`, when it did (a second or more: the two clocks drift
/// apart by a few microseconds otherwise).
pub fn slept(before: Duration, after: Duration) -> Option<Duration> {
    let d = after.saturating_sub(before);
    (d >= Duration::from_secs(1)).then_some(d)
}

/// A file descriptor that becomes readable when the system wakes from a suspend (or the clock is set): a realtime timer far in
/// the future that the kernel cancels then (TFD_TIMER_CANCEL_ON_SET). It never fires by itself.
pub struct SleepWatch {
    fd: OwnedFd,
    suspended: Duration,
}

impl SleepWatch {
    pub fn new() -> std::io::Result<SleepWatch> {
        // SAFETY: a new timerfd of our own
        let fd = unsafe { libc::timerfd_create(libc::CLOCK_REALTIME, libc::TFD_CLOEXEC | libc::TFD_NONBLOCK) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: just made, owned from here on
        let w = SleepWatch { fd: unsafe { OwnedFd::from_raw_fd(fd) }, suspended: suspended_so_far() };
        w.arm()?;
        Ok(w)
    }

    fn arm(&self) -> std::io::Result<()> {
        let far = libc::itimerspec {
            it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 },
            // some thousands of years from now: it is only ever cancelled
            it_value: libc::timespec { tv_sec: 1 << 40, tv_nsec: 0 },
        };
        let flags = libc::TFD_TIMER_ABSTIME | libc::TFD_TIMER_CANCEL_ON_SET;
        // SAFETY: our own timerfd and a timer value of our own
        let r = unsafe { libc::timerfd_settime(std::os::fd::AsRawFd::as_raw_fd(&self.fd), flags, &far, std::ptr::null_mut()) };
        if r < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    /// After the descriptor became readable: set the timer again, and how long the system slept (None: the clock was set, or
    /// nothing happened).
    pub fn woke(&mut self) -> Option<Duration> {
        let mut buf = [0u8; 8];
        // SAFETY: reading the 8-byte count into a buffer of our own (it fails with ECANCELED after a cancel, as expected)
        let _ = unsafe { libc::read(std::os::fd::AsRawFd::as_raw_fd(&self.fd), buf.as_mut_ptr().cast(), 8) };
        if let Err(e) = self.arm() {
            eprintln!("yggdrasil-compositor: the wake-up watch could not be set again: {e}");
        }
        let now = suspended_so_far();
        let d = slept(self.suspended, now);
        self.suspended = now;
        d
    }
}

impl AsFd for SleepWatch {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_laptops_own_screen_is_known_by_its_connector() {
        for n in ["eDP-1", "eDP-2", "LVDS-1", "DSI-1"] {
            assert!(is_internal(n), "{n}");
        }
        for n in ["DP-1", "DP-4", "HDMI-A-1", "Virtual-1", "HEADLESS-1", "DVI-D-1", "eDPX-1"] {
            assert!(!is_internal(n), "{n}");
        }
    }

    #[test]
    fn the_default_scale_comes_from_the_screens_size() {
        // this laptop: 1920x1200, 30x19 cm (Hyprland on it: 1.25, as Settings set it)
        assert_eq!(default_scale((300, 190), (1920, 1200)), 1.25);
        assert_eq!(default_scale((309, 174), (1920, 1080)), 1.25, "a 14-inch full HD panel");
        assert_eq!(default_scale((344, 194), (1920, 1080)), 1.0, "15.6 inches");
        assert_eq!(default_scale((302, 189), (2880, 1800)), 1.75, "a sharp 14-inch panel");
        assert_eq!(default_scale((344, 194), (3840, 2160)), 2.0, "4K at 15.6 inches");
        assert_eq!(default_scale((597, 336), (3840, 2160)), 1.5, "a 27-inch 4K monitor");
        assert_eq!(default_scale((470, 529), (2560, 2880)), 1.25, "an LG DualUp, as Settings has it on my PC");
        assert_eq!(default_scale((800, 335), (3440, 1440)), 1.0, "a 34-inch ultrawide");
        assert_eq!(default_scale((531, 299), (1920, 1080)), 1.0, "24 inches: never below 1");
        assert_eq!(default_scale((0, 0), (1920, 1080)), 1.0, "a projector says no size");
        assert_eq!(default_scale((160, 90), (3840, 2160)), 1.0, "an aspect ratio instead of a size");
        assert_eq!(default_scale((60, 40), (1920, 1200)), 2.25, "a tiny sharp screen: 853x533 is left");
        assert_eq!(default_scale((30, 20), (1280, 800)), 1.5, "never less than 800x480 left (853x533)");
        for (mm, px) in [((300, 190), (1920, 1200)), ((597, 336), (3840, 2160)), ((60, 40), (1920, 1200))] {
            let s = default_scale(mm, px);
            assert_eq!((s * 4.0).fract(), 0.0, "Settings' steps of 0.25: {s}");
        }
    }

    #[test]
    fn the_lid_takes_the_panel_off_only_when_another_screen_shows() {
        let s = |v: &[(&str, bool)]| v.iter().map(|(n, on)| (n.to_string(), *on)).collect::<Vec<_>>();
        assert_eq!(lid_takes_off(true, &s(&[("eDP-1", true), ("DP-1", true)])), ["eDP-1"]);
        assert!(lid_takes_off(false, &s(&[("eDP-1", true), ("DP-1", true)])).is_empty(), "open: it stays");
        assert!(lid_takes_off(true, &s(&[("eDP-1", true)])).is_empty(), "alone: logind's (it stays, dark)");
        assert!(lid_takes_off(true, &s(&[("eDP-1", true), ("HDMI-A-1", false)])).is_empty(), "the other one is switched off");
        assert_eq!(lid_takes_off(true, &s(&[("eDP-1", false), ("DP-3", true), ("DP-4", true)])), ["eDP-1"]);
    }

    #[test]
    fn the_switches_are_told_as_they_are() {
        assert_eq!(Switches::default().describe(), ["lid unknown", "tablet-mode unknown"]);
        let s = Switches { lid_closed: Some(true), tablet_mode: Some(false) };
        assert_eq!(s.describe(), ["lid closed", "tablet-mode off"]);
        assert!(s.lid_closed() && !Switches::default().lid_closed());
    }

    #[test]
    fn a_switch_is_known_from_its_device_on() {
        // the laptop: lid-switch (a lid), then intel-hid-switches (tablet mode); nothing toggled: open and off from the start
        let mut s = Switches::default();
        s.device_added(false, false);
        assert_eq!(s.describe(), ["lid unknown", "tablet-mode unknown"], "a device without these switches says nothing");
        s.device_added(true, false);
        s.device_added(false, true);
        assert_eq!(s.describe(), ["lid open", "tablet-mode off"]);
        // started closed: libinput's toggle right after the device came
        let mut s = Switches::default();
        s.device_added(true, true);
        s.lid_closed = Some(true);
        // a second tablet-mode device (thinkpad-extra-buttons) does not undo what was told
        s.tablet_mode = Some(true);
        s.device_added(true, true);
        assert_eq!(s.describe(), ["lid closed", "tablet-mode on"]);
    }

    #[test]
    fn a_resume_is_told_from_a_clock_set() {
        let s = Duration::from_secs;
        assert_eq!(slept(s(10), s(310)), Some(s(300)), "five minutes asleep");
        assert_eq!(slept(s(10), s(10) + Duration::from_micros(40)), None, "the clock was set: no time asleep");
        assert_eq!(slept(s(10), s(5)), None);
        let w = SleepWatch::new().expect("a timerfd");
        let mut p = libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&w.as_fd()), events: libc::POLLIN, revents: 0 };
        // SAFETY: one pollfd of our own, no waiting
        assert_eq!(unsafe { libc::poll(&mut p, 1, 0) }, 0, "it does not fire by itself");
        assert!(suspended_so_far() < Duration::from_secs(60 * 60 * 24 * 365));
    }
}
