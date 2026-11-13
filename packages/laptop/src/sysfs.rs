//! What the computer has, read from /sys under `root` ("/" outside tests): world-readable files only, so nothing here needs a
//! group or a password, and no input device is opened (reading a keyboard's events is a keylogger's privilege; its capability
//! bitmaps in /sys say only what keys it has).

use std::path::{Path, PathBuf};

/// SW_TABLET_MODE in an input device's switch bitmap (linux/input-event-codes.h).
const SW_TABLET_MODE: u32 = 1;
/// Keys every full keyboard has, in an input device's key bitmap: Q, P, A, Z and the space bar.
const LETTER_KEYS: [u32; 5] = [16, 25, 30, 44, 57];
/// The buses a keyboard you plug in or pair arrives on (input id/bustype): USB and Bluetooth. The built-in keyboard is on
/// i8042 (0x11) or I2C/SPI, and it is folded away in tablet mode.
const PLUGGED_BUSES: [u32; 2] = [0x03, 0x05];

fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|rd| rd.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn text(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// A capability bitmap as the kernel prints it (hex words of the platform's `long`, the lowest last) has bit `n`.
pub fn bitmap_has(bits: &str, n: u32) -> bool {
    let words: Vec<&str> = bits.split_whitespace().collect();
    let (word, bit) = ((n / 64) as usize, n % 64);
    word < words.len() && u64::from_str_radix(words[words.len() - 1 - word], 16).is_ok_and(|v| v >> bit & 1 == 1)
}

/// The input devices (/sys/class/input/input*).
fn inputs(root: &Path) -> Vec<PathBuf> {
    entries(&root.join("sys/class/input"))
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("input")))
        .collect()
}

/// An input device with a tablet-mode switch: a 2-in-1 that folds into a tablet (thinkpad_acpi, intel-hid, intel-vbtn, ...).
pub fn tablet_switch(root: &Path) -> bool {
    inputs(root).iter().any(|d| text(&d.join("capabilities/sw")).is_some_and(|b| bitmap_has(&b, SW_TABLET_MODE)))
}

/// An accelerometer the kernel offers through IIO (a sensor hub's accel_3d, or a chip on I2C), which iio-sensor-proxy reads.
pub fn accelerometer(root: &Path) -> bool {
    entries(&root.join("sys/bus/iio/devices")).iter().any(|d| {
        text(&d.join("name")).is_some_and(|n| n.contains("accel"))
            || entries(d).iter().any(|f| f.file_name().is_some_and(|n| n.to_string_lossy().starts_with("in_accel_")))
    })
}

/// A full keyboard plugged in by USB or paired over Bluetooth (a receiver for a keyboard and mouse counts: it can type).
pub fn plugged_keyboard(root: &Path) -> bool {
    inputs(root).iter().any(|d| {
        let bus = text(&d.join("id/bustype")).and_then(|b| u32::from_str_radix(&b, 16).ok());
        bus.is_some_and(|b| PLUGGED_BUSES.contains(&b))
            && text(&d.join("capabilities/key")).is_some_and(|k| LETTER_KEYS.iter().all(|&n| bitmap_has(&k, n)))
    })
}

/// A ThinkPad's own word on tablet mode (thinkpad_acpi's hotkey_tablet_mode, readable by anyone), for the state at start; the
/// switch's changes come from the compositor afterwards. None on other laptops (they start as laptops until the switch says).
pub fn thinkpad_tablet_mode(root: &Path) -> Option<bool> {
    text(&root.join("sys/devices/platform/thinkpad_acpi/hotkey_tablet_mode")).map(|t| t == "1")
}

#[cfg(test)]
pub(crate) mod fake {
    //! A pretend /sys under a temporary folder.
    use std::path::{Path, PathBuf};

    pub struct Tree(pub PathBuf);

    impl Tree {
        pub fn new(name: &str) -> Tree {
            let d = std::env::temp_dir().join(format!("ygg-laptop-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Tree(d)
        }
        pub fn put(&self, p: &str, t: &str) -> &Tree {
            let f = self.0.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, format!("{t}\n")).unwrap();
            self
        }
        pub fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A key bitmap with these keys, as the kernel prints it (64-bit words, the lowest last).
    pub fn keys(set: &[u32]) -> String {
        let top = set.iter().max().copied().unwrap_or(0) / 64;
        (0..=top)
            .rev()
            .map(|w| format!("{:x}", set.iter().filter(|&&n| n / 64 == w).fold(0u64, |a, &n| a | 1 << (n % 64))))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::fake::{keys, Tree};
    use super::*;

    #[test]
    fn bitmaps_are_read_as_the_kernel_prints_them() {
        assert!(bitmap_has("2", 1) && !bitmap_has("1", 1) && bitmap_has("1 0", 64) && !bitmap_has("1 0", 0));
        assert!(!bitmap_has("", 0) && !bitmap_has("zz", 0) && !bitmap_has("2", 70));
        let k = keys(&[16, 30, 57, 130]);
        assert_eq!(k.split(' ').count(), 3);
        assert!([16, 30, 57, 130].iter().all(|&n| bitmap_has(&k, n)) && !bitmap_has(&k, 44));
    }

    #[test]
    fn a_2_in_1_is_told_from_a_desktop() {
        let yoga = Tree::new("yoga");
        yoga.put("sys/class/input/input3/capabilities/sw", "1") // the lid switch
            .put("sys/class/input/input9/capabilities/sw", "2") // ThinkPad Extra Buttons: SW_TABLET_MODE
            .put("sys/bus/iio/devices/iio:device0/name", "accel_3d")
            .put("sys/devices/platform/thinkpad_acpi/hotkey_tablet_mode", "1")
            .put("sys/class/input/input2/id/bustype", "0011") // the built-in keyboard (i8042)
            .put("sys/class/input/input2/capabilities/key", &keys(&LETTER_KEYS));
        assert!(tablet_switch(yoga.path()) && accelerometer(yoga.path()));
        assert_eq!(thinkpad_tablet_mode(yoga.path()), Some(true));
        assert!(!plugged_keyboard(yoga.path()), "the built-in keyboard folds away with the screen");
        yoga.put("sys/class/input/input20/id/bustype", "0005").put("sys/class/input/input20/capabilities/key", &keys(&LETTER_KEYS));
        assert!(plugged_keyboard(yoga.path()), "a Bluetooth keyboard types");

        let desk = Tree::new("desk");
        desk.put("sys/class/input/input3/capabilities/sw", "1")
            .put("sys/bus/iio/devices/iio:device0/name", "nct6775")
            .put("sys/class/input/input5/id/bustype", "0003")
            .put("sys/class/input/input5/capabilities/key", &keys(&[272, 273, 274, 16])); // a mouse with one macro key
        assert!(!tablet_switch(desk.path()) && !accelerometer(desk.path()) && !plugged_keyboard(desk.path()));
        assert_eq!(thinkpad_tablet_mode(desk.path()), None);
        desk.put("sys/bus/iio/devices/iio:device1/in_accel_x_raw", "12");
        assert!(accelerometer(desk.path()), "an accelerometer chip without a telling name");
        assert!(!tablet_switch(&desk.path().join("missing")));
    }
}
