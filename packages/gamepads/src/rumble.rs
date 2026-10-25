//! A rumble test through the kernel's force feedback, the way games make a controller rumble: upload a rumble effect to the
//! controller's event device, play it once, and remove it. Needs write access to /dev/input/eventN, which the logged-in user has
//! (udev's uaccess tag on joysticks).

use crate::Pad;
use std::io::Write;
use std::os::unix::io::AsRawFd;

const EV_FF: u16 = 0x15;
const FF_RUMBLE: u16 = 0x50;

/// The kernel's `struct ff_effect` (linux/input.h): a header, then a union whose largest member (the periodic effect) is 32 bytes
/// and holds a pointer, so the union starts 8-aligned at byte 16. 48 bytes on 64-bit Linux.
#[repr(C)]
struct FfEffect {
    kind: u16,
    id: i16,
    direction: u16,
    trigger: [u16; 2],
    replay: [u16; 2],
    union: [u64; 4],
}

#[repr(C)]
struct InputEvent {
    time: libc::timeval,
    kind: u16,
    code: u16,
    value: i32,
}

const fn iow(nr: u64, size: u64) -> u64 {
    (1 << 30) | (size << 16) | ((b'E' as u64) << 8) | nr
}

const EVIOCSFF: u64 = iow(0x80, std::mem::size_of::<FfEffect>() as u64);
const EVIOCRMFF: u64 = iow(0x81, std::mem::size_of::<libc::c_int>() as u64);

/// Rumbles the controller for `ms` milliseconds (both motors, strong).
pub fn rumble(pad: &Pad, ms: u16) -> Result<(), String> {
    let path = pad.event.as_ref().ok_or("the controller has no event device")?;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut effect = FfEffect { kind: FF_RUMBLE, id: -1, direction: 0, trigger: [0, 0], replay: [ms.min(5000), 0], union: [0; 4] };
    // ff_rumble_effect { strong_magnitude: u16, weak_magnitude: u16 } at the start of the union
    effect.union[0] = 0xc000u64 | (0xc000u64 << 16);
    let fd = f.as_raw_fd();
    if unsafe { libc::ioctl(fd, EVIOCSFF as _, &mut effect as *mut FfEffect) } < 0 {
        return Err(format!("this controller does not rumble here ({})", std::io::Error::last_os_error()));
    }
    let play = InputEvent { time: libc::timeval { tv_sec: 0, tv_usec: 0 }, kind: EV_FF, code: effect.id as u16, value: 1 };
    let bytes = unsafe { std::slice::from_raw_parts(&play as *const InputEvent as *const u8, std::mem::size_of::<InputEvent>()) };
    let res = f.write_all(bytes).map_err(|e| e.to_string());
    // the effect plays on its own; it is removed once it is over (closing the device would stop it early)
    std::thread::sleep(std::time::Duration::from_millis(ms.min(5000) as u64 + 50));
    unsafe { libc::ioctl(fd, EVIOCRMFF as _, effect.id as libc::c_int) };
    res
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_kernel_struct_sizes_match() {
        assert_eq!(std::mem::size_of::<super::FfEffect>(), 48);
        assert_eq!(super::EVIOCSFF, 0x40304580);
        assert_eq!(super::EVIOCRMFF, 0x40044581);
        assert_eq!(std::mem::size_of::<super::InputEvent>(), 24);
    }
}
