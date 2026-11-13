//! A laptop's hardware for Yggdrasil (2026-11-13, for a ThinkPad X1 Yoga Gen 8 and other 2-in-1s):
//!
//! - `sysfs`: what the computer has, read from /sys (an accelerometer, a tablet-mode switch, keyboards on a cable or Bluetooth),
//!   under a root folder so tests use a pretend tree.
//! - `tablet`: tablet mode and rotation. Folded into a tablet, the built-in screen follows the accelerometer (iio-sensor-proxy)
//!   and its touchscreen and pen turn with it; the on-screen keyboard shows while no keyboard is plugged in. Settings locks the
//!   rotation.
//! - `fingerprint`: the reader through fprintd (Settings > Users), and the opt-in line in /etc/pam.d/sudo that lets a finger
//!   answer sudo, written only by the root helper after an administrator's password.

pub mod fingerprint;
pub mod sysfs;
pub mod tablet;
