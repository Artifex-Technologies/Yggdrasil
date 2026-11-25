//! The compositor's second connection to XWayland, for what an X11 window manager does and Smithay's does not. It runs on a
//! thread of its own, so the compositor never waits for the X server (XWayland is also a client of the compositor: a blocking
//! read here could wait on a server that waits on us); the compositor hands it jobs and waits for no answer. (What a window
//! asks for before it is shown, `_NET_WM_STATE`, which gave this file its name, Smithay's window manager reads itself:
//! xwayland.rs, map_window_request.)
//!
//! It says which X11 window is active, as EWMH window managers do: `_NET_ACTIVE_WINDOW` on the root, and the X input
//! focus on the window. Smithay sets the property only when the focus has moved, and for a "globally active" window (input
//! hint False with WM_TAKE_FOCUS: every Wine window) it only sends WM_TAKE_FOCUS and waits for the program to take the focus.
//! Wine waits for the window manager to mark its window active first, so neither moved: Fallout 4 through Proton got no keys
//! at all (found 2026-11-23). Hyprland and KWin set both themselves.
//!
//! It also keeps each X11 program's own icon (`_NET_WM_ICON`) for the dock, whose windows no desktop entry matches (a Windows
//! program through Wine, an X11 program that installed none): the best size of it is written as a PNG, one for each window
//! class, in the runtime folder's `yggdrasil-window-icons` (yggdrasil_conf::dirs::window_icons), where the panel looks for an
//! icon its theme does not have. The first window of a class gives it; it is read when the window is shown and again when its
//! title changes (a program that sets its icon after its window shows).
//!
//! It also keeps ICCCM's WM_STATE in step with minimizing: Iconic when a window is minimized, Normal when it is shown again.
//! Smithay only writes it when a window is mapped or unmapped. Wine watches it: a game that minimized itself (old DirectX 9
//! games do when they lose the focus) is restored only when WM_STATE goes back to Normal. Without that Wine kept the game at
//! its minimized size, and on the dock's restore only a strip of the picture showed (Dead Space 2, found 2026-11-05).

use std::sync::mpsc::{channel, Sender};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt, EventMask, InputFocus, PropMode};
use x11rb::wrapper::ConnectionExt as _;

enum Job {
    /// this window's own icon, kept for its class (unless the class has one already)
    Icon(u32, String),
    /// this X11 window has the keyboard now (0: none has: a Wayland window or nothing has it)
    Activate(u32),
    /// WM_STATE: Iconic (minimized) or Normal
    Iconic(u32, bool),
}

/// The icon size the dock wants: the smallest offered that is at least this big, else the biggest.
const ICON_WANT: u32 = 64;
/// At most this many 32-bit values of `_NET_WM_ICON` are read (4 MB: a 1024x1024 icon and smaller ones).
const ICON_LONGS: u32 = 1 << 20;

/// The best icon in a `_NET_WM_ICON` list (width, height, then width x height ARGB pixels, for each size): its size and its
/// pixels as RGBA bytes. Sizes past the list's end or bigger than 1024 end the list.
pub fn best_icon(data: &[u32]) -> Option<(u32, u32, Vec<u8>)> {
    let mut found: Vec<(u32, u32, &[u32])> = vec![];
    let mut i = 0usize;
    while i + 2 <= data.len() {
        let (w, h) = (data[i], data[i + 1]);
        if w == 0 || h == 0 || w > 1024 || h > 1024 {
            break;
        }
        let n = (w * h) as usize;
        let Some(px) = data.get(i + 2..i + 2 + n) else { break };
        found.push((w, h, px));
        i += 2 + n;
    }
    let big_enough = found.iter().filter(|(w, h, _)| (*w).min(*h) >= ICON_WANT).min_by_key(|(w, h, _)| w * h);
    let (w, h, px) = big_enough.or_else(|| found.iter().max_by_key(|(w, h, _)| w * h))?;
    let rgba = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]).collect();
    Some((*w, *h, rgba))
}

/// Writes a class's icon (RGBA pixels) as a PNG in `dir`, whole or not at all (written beside it, then moved into place).
fn keep_icon(dir: &std::path::Path, file: &str, (w, h, rgba): (u32, u32, Vec<u8>)) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(".{file}.new"));
    {
        let f = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let mut enc = png::Encoder::new(f, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut wr = enc.write_header().map_err(std::io::Error::other)?;
        wr.write_image_data(&rgba).map_err(std::io::Error::other)?;
    }
    std::fs::rename(&tmp, dir.join(file))
}

/// ICCCM's WM_STATE values.
const NORMAL_STATE: u32 = 1;
const ICONIC_STATE: u32 = 3;

/// The WM_STATE property's value: the state and no icon window.
pub fn wm_state(iconic: bool) -> [u32; 2] {
    [if iconic { ICONIC_STATE } else { NORMAL_STATE }, 0]
}

/// The thread that reads the property; dropped with the compositor.
pub struct Reader {
    tx: Sender<Job>,
    /// a byte written here wakes the thread for the job just sent
    wake: std::os::unix::net::UnixStream,
}

impl Reader {
    /// Connects to XWayland's display `:n` on a new thread. None when the thread could not be started.
    pub fn start(display: u32) -> Option<Reader> {
        let (tx, rx) = channel::<Job>();
        // the thread sleeps until a job comes (a byte on this pair says so) or XWayland has an event for it
        let (wake, woken) = std::os::unix::net::UnixStream::pair().ok()?;
        woken.set_nonblocking(true).ok()?;
        std::thread::Builder::new()
            .name("x11-asked".into())
            .spawn(move || {
                use std::io::Read;
                use std::os::fd::AsRawFd;
                use x11rb::protocol::xproto::ChangeWindowAttributesAux;
                use x11rb::protocol::Event as XEvent;
                let Ok((conn, screen)) = x11rb::rust_connection::RustConnection::connect(Some(&format!(":{display}"))) else {
                    eprintln!("yggdrasil-compositor: no second connection to XWayland's :{display} (x11asked.rs: the active X11 window, icons, WM_STATE)");
                    return;
                };
                let root = conn.setup().roots.get(screen).map(|r| r.root).unwrap_or(0);
                let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom);
                let (Some(wm_state_atom), Some(active)) = (atom("WM_STATE"), atom("_NET_ACTIVE_WINDOW")) else {
                    return;
                };
                let net_wm_icon = atom("_NET_WM_ICON");
                let icons = yggdrasil_conf::dirs::window_icons();
                // the root's properties are watched: Smithay rewrites _NET_ACTIVE_WINDOW on every focus event, with the window
                // the event was reported on (the root, for a focus set on a window), so the right one is written again
                let _ = conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::PROPERTY_CHANGE));
                let _ = conn.flush();
                // the window marked active (0: none), once a job has said
                let mut want: Option<u32> = None;
                let set_active = |w: u32| {
                    let now = conn
                        .get_property(false, root, active, AtomEnum::WINDOW, 0, 1)
                        .ok()
                        .and_then(|c| c.reply().ok())
                        .and_then(|r| r.value32().and_then(|mut v| v.next()));
                    if now != Some(w) {
                        let _ = conn.change_property32(PropMode::REPLACE, root, active, AtomEnum::WINDOW, &[w]);
                        let _ = conn.flush();
                    }
                };
                // what XWayland sent (x11rb may also hold events it read while waiting for a reply): false when it went away
                let drain = |want: Option<u32>| -> bool {
                    loop {
                        match conn.poll_for_event() {
                            Ok(Some(XEvent::PropertyNotify(n))) if n.window == root && n.atom == active => {
                                if let Some(w) = want {
                                    set_active(w);
                                }
                            }
                            Ok(Some(_)) => {}
                            Ok(None) => return true,
                            Err(_) => return false,
                        }
                    }
                };
                loop {
                    if !drain(want) {
                        return;
                    }
                    // the jobs (ends when the compositor drops the Reader)
                    loop {
                        let job = match rx.try_recv() {
                            Ok(j) => j,
                            Err(std::sync::mpsc::TryRecvError::Empty) => break,
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                        };
                        match job {
                            Job::Icon(w, class) => {
                                let (Some(dir), Some(file), Some(prop)) =
                                    (&icons, yggdrasil_conf::dirs::window_icon_file(&class), net_wm_icon)
                                else {
                                    continue;
                                };
                                if dir.join(&file).exists() {
                                    continue;
                                }
                                let data: Vec<u32> = conn
                                    .get_property(false, w, prop, AtomEnum::CARDINAL, 0, ICON_LONGS)
                                    .ok()
                                    .and_then(|c| c.reply().ok())
                                    .and_then(|r| r.value32().map(|v| v.collect()))
                                    .unwrap_or_default();
                                if let Some(icon) = best_icon(&data) {
                                    if let Err(e) = keep_icon(dir, &file, icon) {
                                        eprintln!("yggdrasil-compositor: {class}'s icon could not be kept: {e}");
                                    }
                                }
                            }
                            Job::Activate(w) => {
                                want = Some(w);
                                if w != 0 {
                                    let _ = conn.set_input_focus(InputFocus::POINTER_ROOT, w, x11rb::CURRENT_TIME);
                                }
                                set_active(w);
                            }
                            Job::Iconic(w, iconic) => {
                                let _ = conn.change_property32(PropMode::REPLACE, w, wm_state_atom, wm_state_atom, &wm_state(iconic));
                                let _ = conn.flush();
                            }
                        }
                    }
                    // events a reply brought in, before sleeping
                    if !drain(want) {
                        return;
                    }
                    let mut drain = [0u8; 64];
                    while matches!((&woken).read(&mut drain), Ok(n) if n > 0) {}
                    let mut fds = [
                        libc::pollfd { fd: conn.stream().as_raw_fd(), events: libc::POLLIN, revents: 0 },
                        libc::pollfd { fd: woken.as_raw_fd(), events: libc::POLLIN, revents: 0 },
                    ];
                    // SAFETY: two valid pollfd entries on the stack, for as long as the call
                    if unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) } < 0
                        && std::io::Error::last_os_error().kind() != std::io::ErrorKind::Interrupted
                    {
                        return;
                    }
                }
            })
            .ok()?;
        Some(Reader { tx, wake })
    }

    fn send(&self, job: Job) {
        use std::io::Write;
        if self.tx.send(job).is_ok() {
            let _ = (&self.wake).write(&[1]);
        }
    }

    /// Keep this window's own icon for its class (`class`), unless the class has one already.
    pub fn icon(&self, window: u32, class: &str) {
        self.send(Job::Icon(window, class.to_string()));
    }

    /// Mark this window minimized (Iconic) or shown again (Normal) in WM_STATE.
    pub fn iconic(&self, window: u32, iconic: bool) {
        self.send(Job::Iconic(window, iconic));
    }

    /// This X11 window has the keyboard (0: no X11 window has): marked active and given the X input focus.
    pub fn activate(&self, window: u32) {
        self.send(Job::Activate(window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_best_icon_is_picked_from_the_list() {
        // a 2x2 red one, then a 64x64 blue one: the 64 is the dock's
        let mut data = vec![2, 2];
        data.extend([0xffff0000u32; 4]);
        data.extend([64, 64]);
        data.extend(std::iter::repeat_n(0x800000ffu32, 64 * 64));
        let (w, h, px) = best_icon(&data).unwrap();
        assert_eq!((w, h), (64, 64));
        assert_eq!(&px[..4], &[0, 0, 255, 128], "ARGB to RGBA, alpha kept");
        // only small ones: the biggest
        assert_eq!(best_icon(&data[..6]).map(|(w, h, _)| (w, h)), Some((2, 2)));
        // a list that ends early, or says a size it cannot hold: what came before it, or nothing
        let mut cut = data.clone();
        cut.truncate(6 + 2 + 100);
        assert_eq!(best_icon(&cut).map(|(w, h, _)| (w, h)), Some((2, 2)));
        assert_eq!(best_icon(&[5000, 5000, 1, 2, 3]), None);
        assert_eq!(best_icon(&[]), None);
        assert_eq!(best_icon(&[0, 0]), None);
    }

    #[test]
    fn wm_state_says_minimized_or_not() {
        assert_eq!(wm_state(true), [3, 0], "IconicState, no icon window");
        assert_eq!(wm_state(false), [1, 0], "NormalState");
    }
}
