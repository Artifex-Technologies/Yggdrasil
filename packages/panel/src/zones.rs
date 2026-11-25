//! Window zones (FancyZones-style): drag a window to the notch and a sheet of layouts opens under it; drop the window on a zone
//! and it fills that part of the screen. Super+Shift+Z opens the same sheet for the window in front (then click a zone).
//!
//! No compositor tells the panel that a window is being dragged, and the panel gets no pointer events then (the compositor holds
//! the pointer). So the panel watches the mouse's left button itself, from the kernel, and only while window zones are on: its
//! presses and releases come as events ([`ButtonEvents`]); with zones off no mouse is open, and a press wakes nothing. While the
//! button is held, the panel asks the compositor every 60 ms for the window in front, and once that moved, for the pointer: a
//! window dragged with Super to the notch opens the sheet ([`Spotter`]). While the sheet is open, the panel asks every 30 ms where
//! the pointer and the window are, to light the zone under the pointer (yggdrasil-wm: Hyprland's socket or Yggdrasil's own
//! compositor's, no process per question). Layouts and the gap come from `zones.conf` (yggdrasil_conf::zones).
//!
//! The drop is the button's release event: the window goes to the zone under the pointer. A window that stops following the
//! pointer ends the drag too, at the zone it was held over last ([`DropWatch`]), and a sheet that hears no drop closes after 8 s
//! of a still pointer ([`STALE`]).

use resvg::tiny_skia::Pixmap;
use yggdrasil_conf::zones::{self, Layout, Monitor, Rect, ZonesConf};
use yggdrasil_wm::Desk;

use crate::paint::{self, Fonts};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// A window is being dragged: the pointer is followed, the button's release places it.
    Drag,
    /// Opened from the keyboard for the window in front: a click places it.
    Pick,
}

#[derive(Clone, Debug)]
pub struct Session {
    pub mode: Mode,
    /// The window to place (the compositor's name for it: Hyprland's address, or our compositor's number).
    pub address: String,
    pub floating: bool,
    pub monitor: Monitor,
    pub layouts: Vec<Layout>,
    pub gap: u32,
    /// The zone under the pointer: layout and zone index.
    pub hover: Option<(usize, usize)>,
    /// Where the pointer is on the sheet's surface (logical pixels), while dragging.
    pub pointer: Option<(f32, f32)>,
    /// When the pointer last moved: a drag sheet that hears no drop closes after a while.
    pub moved: std::time::Instant,
    /// Watches the window for the end of a drag that no release showed.
    pub watch: DropWatch,
}

/// The end of a drag as the window shows it, for when the move ends with no release the panel hears. It ends the drag once; later
/// signs are ignored.
///
/// While the button is held the window follows the pointer, moving whenever it does (a frame behind at most, which on a fast drag
/// is more than a few pixels, so the window's offset from the pointer is no test). Let go, and the window stays where it is while the
/// pointer moves on: the drop is the pointer moving more than `SLACK` since the window last moved.
#[derive(Clone, Debug)]
pub struct DropWatch {
    /// The pointer's offset from the dragged window's top-left corner when the sheet opened (the window was being dragged).
    pub grip: Option<(f64, f64)>,
    /// Where the window was last seen, and where the pointer was when it last moved.
    pub last_at: Option<(f64, f64)>,
    pub anchor: Option<(f64, f64)>,
    /// The zone under the pointer the last time the window was seen following it (where it was let go).
    pub held: Option<(usize, usize)>,
    pub done: bool,
}

/// How far (logical pixels) the pointer may move away from its grip before the window is taken as let go.
pub const SLACK: f64 = 6.0;

impl DropWatch {
    pub fn new(grip: Option<(f64, f64)>) -> DropWatch {
        DropWatch { grip, last_at: None, anchor: None, held: None, done: false }
    }

    /// The pointer and the window, read while following: Some(zone or none) once the window no longer follows the pointer (the
    /// drop, at the zone held last); otherwise remembers `hover` as where it is held.
    pub fn follow(
        &mut self,
        cursor: (f64, f64),
        window_at: Option<(f64, f64)>,
        hover: Option<(usize, usize)>,
    ) -> Option<Option<(usize, usize)>> {
        if self.done {
            return None;
        }
        let Some(at) = window_at.filter(|_| self.grip.is_some()) else {
            // the window cannot be judged: just remember where the pointer is
            self.held = hover;
            return None;
        };
        if self.last_at != Some(at) {
            // the window moved with the pointer: still held, over this zone
            self.last_at = Some(at);
            self.anchor = Some(cursor);
            self.held = hover;
            return None;
        }
        let (ax, ay) = self.anchor.unwrap_or(cursor);
        if (cursor.0 - ax).abs() + (cursor.1 - ay).abs() > SLACK {
            self.done = true;
            return Some(self.held);
        }
        None
    }
}

/// How long a drag sheet waits, with the pointer still, for a drop it may never hear.
pub const STALE: std::time::Duration = std::time::Duration::from_secs(8);

const BTN_LEFT: usize = 0x110;
const KEY_A: usize = 30;

fn ioc_read(nr: u64, len: usize) -> u64 {
    (2u64 << 30) | ((len as u64) << 16) | ((b'E' as u64) << 8) | nr
}

fn bit(buf: &[u8], n: usize) -> bool {
    buf.get(n / 8).is_some_and(|b| b & (1 << (n % 8)) != 0)
}

/// The folder of input devices: /dev/input, or the one `YGGDRASIL_PANEL_INPUT` names (the tests' own: a panel under test never
/// reads the computer's mice).
pub fn input_dir() -> std::path::PathBuf {
    std::env::var_os("YGGDRASIL_PANEL_INPUT").map(std::path::PathBuf::from).unwrap_or_else(|| "/dev/input".into())
}

/// The pointer devices this user may read in `dir`: each `event*` device that has a left button and is not a keyboard (a mouse's
/// macro-key half shows up as a keyboard). A FIFO there stands in for a mouse: the tests write a mouse's events into one (the
/// kernel's folder holds devices only).
fn pointer_devices(dir: &std::path::Path) -> Vec<std::fs::File> {
    use std::os::fd::AsRawFd;
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    let mut devices = vec![];
    let Ok(rd) = std::fs::read_dir(dir) else { return devices };
    for e in rd.flatten() {
        if !e.file_name().to_string_lossy().starts_with("event") {
            continue;
        }
        // without waiting: a FIFO with no writer would wait for one
        let Ok(f) = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NONBLOCK).open(e.path()) else { continue };
        if e.file_type().is_ok_and(|t| t.is_fifo()) {
            devices.push(f);
            continue;
        }
        let mut caps = [0u8; 96];
        // SAFETY: EVIOCGBIT(EV_KEY), which keys and buttons the device has, writes at most `caps.len()` bytes into the buffer
        let n = unsafe { libc::ioctl(f.as_raw_fd(), ioc_read(0x20 + 1, caps.len()) as _, caps.as_mut_ptr()) };
        if n > 0 && bit(&caps, BTN_LEFT) && !bit(&caps, KEY_A) {
            devices.push(f);
        }
    }
    devices
}

/// The left mouse button's presses and releases as events, from the pointer devices this user may read. Each device is told to
/// report the left button and nothing else (EVIOCSMASK), so moving the mouse or turning the wheel wakes nothing; all of them sit
/// in one epoll, which the panel's event loop watches. Nothing is read or asked between presses.
pub struct ButtonEvents {
    epoll: std::os::fd::OwnedFd,
    /// Each device, and whether its left button is down.
    devices: Vec<(std::fs::File, bool)>,
}

const EV_SYN: u16 = 0;
const EV_KEY: u16 = 1;
const SYN_DROPPED: u16 = 3;
/// How many event types and key codes there are (linux/input-event-codes.h: EV_CNT, KEY_CNT).
const EV_CNT: usize = 0x20;
const KEY_CNT: usize = 0x300;
const LONG_BITS: usize = libc::c_ulong::BITS as usize;
type TypeMask = [libc::c_ulong; EV_CNT.div_ceil(LONG_BITS)];
type KeyMask = [libc::c_ulong; KEY_CNT.div_ceil(LONG_BITS)];

/// A bitmap as EVIOCSMASK reads it: in whole longs. The kernel refuses any other size (EINVAL): a mask of the event types in 4
/// bytes was refused, and every motion and wheel turn of a mouse still woke the panel.
fn longs<const N: usize>(bits: &[usize]) -> [libc::c_ulong; N] {
    let mut m = [0; N];
    for &b in bits {
        m[b / LONG_BITS] |= 1 << (b % LONG_BITS);
    }
    m
}

/// The masks for the left button alone: of the event types, keys and buttons; of those, BTN_LEFT.
fn left_button_masks() -> (TypeMask, KeyMask) {
    (longs(&[EV_KEY as usize]), longs(&[BTN_LEFT]))
}

/// EVIOCSMASK's argument (linux/input.h, struct input_mask).
#[repr(C)]
struct InputMask {
    kind: u32,
    codes_size: u32,
    codes_ptr: u64,
}

fn ioc_write(nr: u64, len: usize) -> u64 {
    (1u64 << 30) | ((len as u64) << 16) | ((b'E' as u64) << 8) | nr
}

/// Only the left button from now on: of the event types, keys and buttons alone; of those, BTN_LEFT alone. An empty report is
/// not delivered, so a motion wakes no one. A device that does not take masks (an old kernel) still works, it just wakes more.
fn left_button_only(f: &std::fs::File) {
    use std::os::fd::AsRawFd;
    let (types, keys) = left_button_masks();
    for (kind, codes) in [(EV_SYN as u32, &types[..]), (EV_KEY as u32, &keys[..])] {
        let m = InputMask { kind, codes_size: std::mem::size_of_val(codes) as u32, codes_ptr: codes.as_ptr() as u64 };
        // SAFETY: EVIOCSMASK reads the struct and `codes_size` bytes at `codes_ptr`, both alive for the call
        unsafe { libc::ioctl(f.as_raw_fd(), ioc_write(0x93, std::mem::size_of::<InputMask>()) as _, &m) };
    }
}

/// Whether the left button is down on this device now (EVIOCGKEY); false when it cannot be asked.
fn left_down_now(f: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    let mut state = [0u8; 96];
    // SAFETY: EVIOCGKEY writes at most `state.len()` bytes into the buffer
    let n = unsafe { libc::ioctl(f.as_raw_fd(), ioc_read(0x18, state.len()) as _, state.as_mut_ptr()) };
    n >= 0 && bit(&state, BTN_LEFT)
}

impl ButtonEvents {
    /// These devices (the tests give pipes that stand in for them); none: nothing open, nothing wakes.
    pub fn with(files: Vec<std::fs::File>) -> ButtonEvents {
        let epoll = rustix::event::epoll::create(rustix::event::epoll::CreateFlags::CLOEXEC).expect("an epoll");
        let mut b = ButtonEvents { epoll, devices: vec![] };
        b.take(files);
        b
    }

    fn take(&mut self, files: Vec<std::fs::File>) {
        use rustix::event::epoll;
        for f in files {
            let _ = rustix::fs::fcntl_setfl(&f, rustix::fs::OFlags::NONBLOCK);
            left_button_only(&f);
            if epoll::add(&self.epoll, &f, epoll::EventData::new_u64(0), epoll::EventFlags::IN).is_ok() {
                let down = left_down_now(&f);
                self.devices.push((f, down));
            }
        }
    }

    /// Other devices in place of these: the mice again after one was plugged in or out, or none once window zones are off.
    /// Closing a device takes it out of the epoll.
    fn replace(&mut self, files: Vec<std::fs::File>) {
        self.devices.clear();
        self.take(files);
    }

    /// The epoll that is ready when a press or release can be read (a second handle to it, for the event loop).
    pub fn poll_fd(&self) -> std::io::Result<std::os::fd::OwnedFd> {
        use std::os::fd::AsFd;
        self.epoll.as_fd().try_clone_to_owned()
    }

    /// Reads what the devices reported (all of it: the epoll is not ready again until something new comes).
    pub fn read(&mut self) {
        use std::io::Read;
        const SIZE: usize = std::mem::size_of::<libc::input_event>();
        for (f, down) in &mut self.devices {
            let mut buf = [0u8; SIZE * 16];
            loop {
                let n = match f.read(&mut buf) {
                    Ok(n) if n >= SIZE => n,
                    _ => break, // nothing more for now (EAGAIN), or the device went (its node goes too: reopen follows)
                };
                for ev in buf[..n].as_chunks::<SIZE>().0 {
                    // SAFETY: input_event is plain data, and the chunk is exactly its size
                    let ev: libc::input_event = unsafe { std::ptr::read_unaligned(ev.as_ptr() as *const libc::input_event) };
                    match (ev.type_, ev.code as usize) {
                        (EV_KEY, BTN_LEFT) => *down = ev.value != 0,
                        // the kernel dropped events (a full queue): ask for the button itself
                        (EV_SYN, c) if c == SYN_DROPPED as usize => *down = left_down_now(f),
                        _ => {}
                    }
                }
            }
        }
    }

    /// The left button is down on one of the devices.
    pub fn held(&self) -> bool {
        self.devices.iter().any(|(_, d)| *d)
    }
}

/// Notices a window being dragged to the notch, with no binding on the mouse button (a binding on the left button kept Hyprland
/// 0.56 from ending its own window move when the button came up). The left button's presses and releases come as events
/// ([`ButtonEvents`]) from the mice in the input folder, open only while window zones are on; only while the button is held does
/// the panel tick and ask the compositor where the window in front is and where the pointer is.
pub struct Spotter {
    pub events: ButtonEvents,
    /// The folder of input devices, and whether window zones are on (only then are its mice open).
    dir: std::path::PathBuf,
    on: bool,
    start: Option<(String, (f64, f64))>,
    told: bool,
}

impl Spotter {
    /// The mice in `dir`, open while window zones are `on`.
    pub fn new(dir: std::path::PathBuf, on: bool) -> Spotter {
        let mut s = Spotter { events: ButtonEvents::with(vec![]), dir, on, start: None, told: false };
        s.devices_changed();
        s
    }

    /// Window zones turned on or off: the mice are opened, or closed (with zones off a press or a held button wakes nothing).
    pub fn set_on(&mut self, on: bool) {
        if on != self.on {
            self.on = on;
            self.devices_changed();
        }
    }

    /// A mouse was plugged in or out, or became readable (the input folder changed): the devices again, while zones are on.
    pub fn devices_changed(&mut self) {
        self.events.replace(if self.on { pointer_devices(&self.dir) } else { vec![] });
        self.forget_released();
    }

    /// While this is true the panel calls `tick` (every 60 ms): the left button is held.
    pub fn held(&self) -> bool {
        self.events.held()
    }

    /// The devices reported a press or release: read it; a release ends the drag being followed.
    pub fn read(&mut self) {
        self.events.read();
        self.forget_released();
    }

    /// The button is up: the next press starts a new drag.
    fn forget_released(&mut self) {
        if !self.events.held() {
            self.start = None;
            self.told = false;
        }
    }

    /// Call every tick while the button is held. True once per drag, when a floating window being dragged has reached the notch
    /// of its screen.
    pub fn tick(&mut self) -> bool {
        if !self.events.held() {
            self.forget_released();
            return false;
        }
        let Some(desk) = Desk::detect() else { return false };
        let Some(win) = desk.active_window(true) else { return false };
        let Some(at) = win.at else { return false };
        let (address, floating) = (win.id, win.floating.unwrap_or(false));
        match &self.start {
            Some((a, from)) if *a == address => {
                if self.told || !floating || (at.0 - from.0).abs() + (at.1 - from.1).abs() <= 6.0 {
                    return false;
                }
                let Some(c) = desk.cursor() else { return false };
                let monitors: Vec<Monitor> = desk.screens_quick().iter().map(|s| s.monitor()).collect();
                if near_notch(&monitors, c) {
                    // once per drag: only a Super+drag opens the sheet. A title-bar drag is the app's own move, and such an app
                    // never hears the button come up once its window is placed away from the pointer: it picks the window up
                    // again on the next move (every app tried, 2026-10-25). Super+drag is Hyprland's move and ends cleanly.
                    self.told = true;
                    let held = desk.super_held();
                    note(&format!("drag at the notch; Super held: {held:?}"));
                    return held == Some(true);
                }
                false
            }
            _ => {
                // the press: the window it went to, and where it was
                self.start = Some((address, at));
                self.told = false;
                false
            }
        }
    }
}

/// The pointer is at the notch: within 180 px of the middle of its screen, no lower than 30 px under the bar.
fn near_notch(monitors: &[Monitor], (cx, cy): (f64, f64)) -> bool {
    monitors.iter().any(|m| {
        let (x, y, w, h) = m.logical();
        let inside = cx >= x as f64 && cx < (x + w) as f64 && cy >= y as f64 && cy < (y + h) as f64;
        inside && (cx - (x as f64 + w as f64 / 2.0)).abs() < 180.0 && cy < (y + m.reserved[1]) as f64 + 30.0
    })
}

/// A line in the drag trail (~/.local/state/yggdrasil/zones.log, shared with the session's Lua), to find why a drop is missed.
pub fn note(msg: &str) {
    use std::io::Write;
    let Some(dir) = std::env::var_os("XDG_STATE_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".local/state")))
    else {
        return;
    };
    let path = dir.join("yggdrasil/zones.log");
    let big = std::fs::metadata(&path).map(|m| m.len() > 200_000).unwrap_or(false);
    let f = if big { std::fs::File::create(&path) } else { std::fs::OpenOptions::new().create(true).append(true).open(&path) };
    if let Ok(mut f) = f {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let _ = writeln!(f, "{} panel: {msg}", t % 100_000_000);
    }
}

/// Where the pointer is in the global space (cheap enough to ask every few tens of milliseconds while a window is dragged;
/// nothing asks otherwise).
pub fn cursor() -> Option<(f64, f64)> {
    Desk::detect()?.cursor()
}

/// The window in front: its name and top-left corner (global logical pixels).
pub fn front_window() -> Option<(String, (f64, f64))> {
    let w = Desk::detect()?.active_window(true)?;
    Some((w.id, w.at?))
}

/// Window zones are on, with a layout to show (Settings > Displays > Window zones).
fn usable(conf: &ZonesConf) -> bool {
    conf.enabled && !conf.layouts.is_empty()
}

/// Whether window zones are on now (`zones.conf`): only then are the mice watched.
pub fn on() -> bool {
    usable(&ZonesConf::load())
}

/// Starts placing the window in front: on the screen under the pointer when dragging, the window's own screen from the keyboard.
pub fn begin(mode: Mode) -> Option<Session> {
    let conf = ZonesConf::load();
    if !usable(&conf) {
        return None;
    }
    let desk = Desk::detect()?;
    let win = desk.active_window(false).filter(|w| !w.id.is_empty())?;
    if win.fullscreen {
        return None;
    }
    let screens = desk.screens();
    let monitors: Vec<Monitor> = screens.iter().map(|s| s.monitor()).collect();
    let monitor = match mode {
        Mode::Drag => desk.cursor().and_then(|(x, y)| monitors.iter().find(|m| m.contains(x, y)).cloned()),
        Mode::Pick => win.monitor.as_ref().and_then(|r| screens.iter().find(|s| s.is(r))).map(|s| s.monitor()),
    }
    .or_else(|| monitors.first().cloned())?;
    // where the pointer holds the window: while it does, the window follows the pointer
    let at = win.at;
    let grip = match (mode, desk.cursor(), at) {
        (Mode::Drag, Some((cx, cy)), Some((wx, wy))) => Some((cx - wx, cy - wy)),
        _ => None,
    };
    Some(Session {
        mode,
        address: win.id,
        floating: win.floating.unwrap_or(true),
        monitor,
        layouts: conf.shown(),
        gap: conf.gap,
        hover: None,
        pointer: None,
        moved: std::time::Instant::now(),
        watch: DropWatch::new(grip),
    })
}

/// Where the zone goes on the screen, in global logical pixels.
pub fn target(s: &Session, (li, zi): (usize, usize)) -> Option<Rect> {
    let z = *s.layouts.get(li)?.zones.get(zi)?;
    Some(zones::place(z, s.monitor.free_area(), s.gap))
}

/// Puts the window there: floating first (a tiled window cannot be placed), then its size, then its place (a resize keeps a
/// window centred, so the other order would undo the move).
pub fn apply(s: &Session, hit: (usize, usize)) {
    let Some(r) = target(s, hit) else { return };
    place(&s.address, s.floating, r);
}

/// Puts a window (the compositor's name for it) at a rectangle (global logical pixels): floating first (a tiled window cannot be
/// placed), then its size, then its place (yggdrasil-wm).
pub fn place(address: &str, floating: bool, rect: Rect) {
    let Some(desk) = Desk::detect() else { return };
    for (c, answer) in desk.place(address, floating, rect) {
        note(&format!("{c} -> {answer:?}"));
    }
}

/// The sheet's surface starts under the bar on the left edge of the screen: global pointer to surface coordinates.
pub fn to_surface(s: &Session, (gx, gy): (f64, f64)) -> (f32, f32) {
    let (mx, my, _, _) = s.monitor.logical();
    ((gx - mx as f64) as f32, (gy - my as f64 - s.monitor.reserved[1] as f64) as f32)
}

const THUMB_W: f32 = 132.0;
const PAD: f32 = 14.0;

/// A zone's rectangle on the sheet with its (layout, zone index).
pub type ZoneHit = (Rect4, (usize, usize));

/// The sheet under the notch: a card with one small picture per layout, its zones drawn in it. Returns each zone's rectangle on
/// the surface (logical pixels) with its layout and zone index. Also lights, at its real size and place, the zone under the
/// pointer.
pub fn paint(pix: &mut Pixmap, scale: f32, s: &Session, fonts: &Fonts, dark: bool) -> (Rect4, Vec<ZoneHit>) {
    pix.fill(paint::color((0, 0, 0, 0)));
    let w = pix.width() as f32 / scale;
    let (_, _, sw, sh) = s.monitor.free_area();
    let aspect = if sw > 0 { sh as f32 / sw as f32 } else { 0.5625 };
    let th = (THUMB_W * aspect).clamp(48.0, 110.0);
    let n = s.layouts.len().max(1) as f32;
    let cw = n * THUMB_W + (n - 1.0) * 10.0 + 2.0 * PAD;
    let ch = PAD + 20.0 + th + 22.0 + PAD;
    let (cx, cy) = ((w - cw) / 2.0, 8.0);
    let (bg, text, dim, tile, zone, edge) = if dark {
        ((24, 24, 30, 240), (240, 240, 245, 255), (150, 150, 165, 255), (255, 255, 255, 14), (255, 255, 255, 34), (255, 255, 255, 60))
    } else {
        ((250, 250, 252, 242), (20, 20, 28, 255), (100, 100, 115, 255), (0, 0, 0, 10), (0, 0, 0, 26), (0, 0, 0, 50))
    };
    // the zone under the pointer, where the window will go
    if let Some(hit) = s.hover {
        if let Some((gx, gy, gw, gh)) = target(s, hit) {
            let (lx, ly) = to_surface(s, (gx as f64, gy as f64));
            paint::fill_rounded(pix, lx * scale, ly * scale, gw as f32 * scale, gh as f32 * scale, 12.0 * scale, paint::accent(70));
            paint::stroke_rounded(
                pix,
                lx * scale,
                ly * scale,
                gw as f32 * scale,
                gh as f32 * scale,
                12.0 * scale,
                2.0 * scale,
                paint::accent(220),
            );
        }
    }
    paint::fill_rounded(pix, cx * scale, cy * scale, cw * scale, ch * scale, 16.0 * scale, bg);
    let title = if s.mode == Mode::Drag { "Drop on a zone" } else { "Pick a zone (Esc to cancel)" };
    paint::draw_text(pix, &fonts.bold, 12.5 * scale, (cx + PAD) * scale, (cy + PAD + 7.0) * scale, text, title);
    let mut hits = vec![];
    for (li, l) in s.layouts.iter().enumerate() {
        let tx = cx + PAD + li as f32 * (THUMB_W + 10.0);
        let ty = cy + PAD + 20.0;
        paint::fill_rounded(pix, tx * scale, ty * scale, THUMB_W * scale, th * scale, 8.0 * scale, tile);
        for (zi, z) in l.zones.iter().enumerate() {
            let g = 3.0;
            let r = (tx + z.0 * THUMB_W + g, ty + z.1 * th + g, z.2 * THUMB_W - 2.0 * g, z.3 * th - 2.0 * g);
            let on = s.hover == Some((li, zi));
            paint::fill_rounded(
                pix,
                r.0 * scale,
                r.1 * scale,
                r.2 * scale,
                r.3 * scale,
                5.0 * scale,
                if on { paint::accent(255) } else { zone },
            );
            if !on {
                paint::stroke_rounded(pix, r.0 * scale, r.1 * scale, r.2 * scale, r.3 * scale, 5.0 * scale, 1.0 * scale, edge);
            }
            hits.push((r, (li, zi)));
        }
        let nw = paint::text_width(&fonts.regular, 11.0 * scale, l.name) / scale;
        paint::draw_text(pix, &fonts.regular, 11.0 * scale, (tx + (THUMB_W - nw) / 2.0) * scale, (ty + th + 12.0) * scale, dim, l.name);
    }
    ((cx, cy, cw, ch), hits)
}

/// A rectangle on the surface in logical pixels (x, y, width, height).
pub type Rect4 = (f32, f32, f32, f32);

/// The zone under a point of the surface.
pub fn hit_at(hits: &[(Rect4, (usize, usize))], (px, py): (f32, f32)) -> Option<(usize, usize)> {
    hits.iter().find(|(r, _)| px >= r.0 && px < r.0 + r.2 && py >= r.1 && py < r.1 + r.3).map(|(_, h)| *h)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pipe standing in for a mouse: the panel's end, and the end its events are written into.
    fn fake_mouse() -> (std::fs::File, std::fs::File) {
        use std::os::fd::FromRawFd;
        let mut fds = [0; 2];
        // SAFETY: pipe fills the two descriptors, each owned by one File from here on
        assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
        unsafe { (std::fs::File::from_raw_fd(fds[0]), std::fs::File::from_raw_fd(fds[1])) }
    }

    fn send(w: &mut std::fs::File, type_: u16, code: u16, value: i32) {
        use std::io::Write;
        let ev = libc::input_event { time: libc::timeval { tv_sec: 0, tv_usec: 0 }, type_, code, value };
        // SAFETY: input_event is plain data; its bytes are what the kernel would write
        let bytes = unsafe { std::slice::from_raw_parts(&ev as *const _ as *const u8, std::mem::size_of::<libc::input_event>()) };
        w.write_all(bytes).unwrap();
    }

    fn ready(fd: &std::os::fd::OwnedFd) -> bool {
        use rustix::event::{poll, PollFd, PollFlags};
        let mut p = [PollFd::new(fd, PollFlags::IN)];
        poll(&mut p, Some(&rustix::time::Timespec { tv_sec: 0, tv_nsec: 50_000_000 })).unwrap() > 0
    }

    #[test]
    fn the_left_button_wakes_the_spotter_and_its_release_ends_the_ticking() {
        let (mouse, mut kernel) = fake_mouse();
        let mut s = Spotter { events: ButtonEvents::with(vec![mouse]), dir: std::path::PathBuf::new(), on: true, start: None, told: false };
        let epoll = s.events.poll_fd().unwrap();
        assert!(!s.held() && !ready(&epoll), "nothing held, nothing to read: no ticks");
        // pressed: the event loop is woken, and the spotter wants its ticks
        send(&mut kernel, EV_KEY, BTN_LEFT as u16, 1);
        send(&mut kernel, EV_SYN, 0, 0);
        assert!(ready(&epoll));
        s.read();
        assert!(s.held(), "held: the 60 ms ticks run");
        assert!(!ready(&epoll), "read: not woken again until the next press or release");
        s.start = Some(("0x1".into(), (10.0, 10.0)));
        // another button, as a mouse without masks would still report it: changes nothing
        send(&mut kernel, EV_KEY, 0x111, 1);
        s.read();
        assert!(s.held());
        // let go: the ticks stop, and the drag followed is forgotten
        send(&mut kernel, EV_KEY, BTN_LEFT as u16, 0);
        assert!(ready(&epoll));
        s.read();
        assert!(!s.held(), "let go: no ticks");
        assert!(s.start.is_none(), "the next press starts a new drag");
    }

    /// A folder of the test's own in place of /dev/input, removed at the end.
    struct Folder(std::path::PathBuf);

    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// How many handles this process holds on a file (/proc/self/fd).
    fn handles(path: &std::path::Path) -> usize {
        let fds = std::fs::read_dir("/proc/self/fd").unwrap();
        fds.flatten().filter(|e| std::fs::read_link(e.path()).is_ok_and(|l| l == path)).count()
    }

    #[test]
    fn the_mice_are_open_only_while_zones_are_on_and_with_zones_off_a_press_wakes_nothing() {
        use std::os::unix::ffi::OsStrExt;
        // the test's input folder: a FIFO standing in for a mouse (event0), a file that is no device (event1), a FIFO by
        // another name (mouse0)
        let dir = Folder(std::env::temp_dir().join(format!("ygg-panel-input-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&dir.0);
        std::fs::create_dir_all(&dir.0).unwrap();
        for name in ["event0", "mouse0"] {
            let c = std::ffi::CString::new(dir.0.join(name).as_os_str().as_bytes()).unwrap();
            // SAFETY: mkfifo reads a valid C string
            assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo {name}");
        }
        std::fs::write(dir.0.join("event1"), "not a device").unwrap();
        // as /proc names it
        let mouse = std::fs::canonicalize(dir.0.join("event0")).unwrap();
        // the test's end: reading and writing, so neither end waits for the other to open
        let mut kernel = std::fs::OpenOptions::new().read(true).write(true).open(&mouse).unwrap();

        // zones on: the mouse is open, and a press wakes the loop
        let mut s = Spotter::new(dir.0.clone(), true);
        assert_eq!(s.events.devices.len(), 1, "event0 alone is a mouse");
        assert_eq!(handles(&mouse), 2, "the test's end and the spotter's");
        let epoll = s.events.poll_fd().unwrap();
        send(&mut kernel, EV_KEY, BTN_LEFT as u16, 1);
        assert!(ready(&epoll));
        s.read();
        assert!(s.held(), "held: the panel looks for a drag");
        send(&mut kernel, EV_KEY, BTN_LEFT as u16, 0);
        assert!(ready(&epoll));
        s.read();
        assert!(!s.held());

        // zones off: the mouse is closed, so a press reaches no one and nothing ticks
        s.set_on(false);
        assert_eq!(handles(&mouse), 1, "only the test's end");
        send(&mut kernel, EV_KEY, BTN_LEFT as u16, 1);
        assert!(!ready(&epoll), "zones off: a press wakes nothing");
        assert!(!s.held());
        // a mouse plugged in meanwhile is not opened either
        s.devices_changed();
        assert!(s.events.devices.is_empty() && handles(&mouse) == 1);
        // and with zones off from the start nothing is opened at all
        let off = Spotter::new(dir.0.clone(), false);
        assert!(off.events.devices.is_empty() && handles(&mouse) == 1);
    }

    fn session() -> Session {
        let monitor =
            Monitor { name: "DP-1".into(), x: 0, y: 0, width: 3440, height: 1440, scale: 1.0, transform: 0, reserved: [0, 28, 0, 70] };
        Session {
            mode: Mode::Drag,
            address: "0xabc".into(),
            floating: true,
            monitor,
            layouts: zones::builtin(),
            gap: 8,
            hover: None,
            pointer: None,
            moved: std::time::Instant::now(),
            watch: DropWatch::new(Some((40.0, 10.0))),
        }
    }

    #[test]
    fn the_drop_is_seen_once_the_window_stops_following() {
        // the window follows the pointer: no drop, and the zone under it is remembered
        let mut w = DropWatch::new(Some((40.0, 10.0)));
        assert_eq!(w.follow((140.0, 30.0), Some((100.0, 20.0)), Some((0, 1))), None);
        assert_eq!(w.follow((300.0, 31.0), Some((260.0, 21.0)), Some((0, 0))), None);
        // a fast drag: the window a frame behind the pointer by far more than the slack, but it moved: still held
        assert_eq!(w.follow((360.0, 31.0), Some((290.0, 21.0)), Some((0, 0))), None);
        // the pointer still for a moment, the window caught up: still held
        assert_eq!(w.follow((360.0, 31.0), Some((320.0, 21.0)), Some((0, 0))), None);
        assert_eq!(w.follow((362.0, 31.0), Some((320.0, 21.0)), Some((0, 0))), None, "within the slack");
        // let go: the pointer moves on, the window stays; the drop is at the zone it was held over, not where the pointer went
        assert_eq!(w.follow((390.0, 40.0), Some((320.0, 21.0)), Some((1, 0))), Some(Some((0, 0))));
        // every later sign is ignored
        assert_eq!(w.follow((400.0, 40.0), Some((260.0, 21.0)), None), None);
        assert_eq!(w.follow((500.0, 90.0), Some((260.0, 21.0)), Some((1, 0))), None);

        // without a grip (no window was being dragged when the sheet opened) the window is not judged, however far the pointer goes
        let mut w = DropWatch::new(None);
        assert_eq!(w.follow((10.0, 10.0), Some((0.0, 0.0)), Some((3, 0))), None);
        assert_eq!(w.follow((400.0, 300.0), Some((0.0, 0.0)), None), None, "without a grip the window is not judged");
    }

    #[test]
    fn the_target_leaves_the_bar_and_dock_free() {
        let s = session();
        let (x, y, w, h) = target(&s, (0, 0)).unwrap(); // the left half
        assert_eq!((x, y), (8, 36));
        assert_eq!(y + h, 1440 - 70 - 8);
        assert_eq!(x + w, 1720 - 4);
        assert_eq!(target(&s, (0, 9)), None);
    }

    #[test]
    fn the_pointer_is_mapped_onto_the_sheet_and_its_zones() {
        let mut s = session();
        // a second screen to the right: its surface starts at its own left edge, under its bar
        s.monitor.x = 3440;
        assert_eq!(to_surface(&s, (3440.0 + 100.0, 28.0 + 50.0)), (100.0, 50.0));
        let hits = vec![((10.0, 10.0, 20.0, 20.0), (1, 2))];
        assert_eq!(hit_at(&hits, (15.0, 15.0)), Some((1, 2)));
        assert_eq!(hit_at(&hits, (35.0, 15.0)), None);
    }
}

#[cfg(test)]
mod button_tests {
    use super::*;

    #[test]
    fn ioctl_numbers_and_bits() {
        // EVIOCGKEY(96) and EVIOCGBIT(EV_KEY, 96) as the kernel headers spell them
        assert_eq!(ioc_read(0x18, 96), 0x80604518);
        assert_eq!(ioc_read(0x21, 96), 0x80604521);
        let mut b = [0u8; 96];
        b[BTN_LEFT / 8] = 1 << (BTN_LEFT % 8);
        assert!(bit(&b, BTN_LEFT) && !bit(&b, KEY_A));
    }

    /// EVIOCSMASK takes whole longs only: the kernel refused the 4-byte mask of event types (EINVAL), so motion and the wheel
    /// still woke the panel. The masks hold the left button and nothing else.
    #[test]
    fn the_masks_are_whole_longs_with_the_left_button_alone() {
        assert_eq!(ioc_write(0x93, std::mem::size_of::<InputMask>()), 0x40104593);
        let (types, keys) = left_button_masks();
        let long = std::mem::size_of::<libc::c_ulong>();
        assert!(std::mem::size_of_val(&types) % long == 0 && std::mem::size_of_val(&types) * 8 >= EV_CNT);
        assert!(std::mem::size_of_val(&keys) % long == 0 && std::mem::size_of_val(&keys) * 8 >= KEY_CNT);
        let set =
            |m: &[libc::c_ulong]| (0..m.len() * LONG_BITS).filter(|&b| m[b / LONG_BITS] >> (b % LONG_BITS) & 1 == 1).collect::<Vec<_>>();
        assert_eq!(set(&types), [EV_KEY as usize]);
        assert_eq!(set(&keys), [BTN_LEFT]);
    }
}
