//! What an X11 window asked for before it was shown: `_NET_WM_STATE` set on the window before it is mapped (a game that wants
//! to start full screen, a program that wants to start maximized). EWMH says the window manager reads it when the window is
//! mapped; Smithay's window manager does not, so a game that asked would open as an ordinary window.
//!
//! The property is read on a thread of its own, over its own connection to XWayland, so the compositor never waits for the X
//! server (XWayland is also a client of the compositor: a blocking read here could wait on a server that waits on us). The
//! answer comes back to the event loop through a channel and is acted on then, a moment after the window is shown.
//!
//! The same thread closes X11 windows: a program that takes part in WM_DELETE_WINDOW is asked to close, as Smithay would ask;
//! one that does not (glxgears, some old programs and games) has its connection ended (XKillClient), as KWin and Hyprland do.
//! Smithay destroys such a window instead, which leaves the program running with no window (found 2026-11-05 with glxgears).
//!
//! It also keeps ICCCM's WM_STATE in step with minimizing: Iconic when a window is minimized, Normal when it is shown again.
//! Smithay only writes it when a window is mapped or unmapped. Wine watches it: a game that minimized itself (old DirectX 9
//! games do when they lose the focus) is restored only when WM_STATE goes back to Normal. Without that Wine kept the game at
//! its minimized size, and on the dock's restore only a strip of the picture showed (Dead Space 2, found 2026-11-05).

use std::sync::mpsc::{channel, Sender};

use smithay::reexports::calloop::channel::{channel as loop_channel, Event};
use smithay::reexports::calloop::LoopHandle;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, PropMode};
use x11rb::wrapper::ConnectionExt as _;

use crate::state::App;

/// One window's answer: what it asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Asked {
    pub window: u32,
    pub fullscreen: bool,
    pub maximized: bool,
}

/// The atoms of `_NET_WM_STATE` that matter here.
#[derive(Clone, Copy, Debug)]
pub struct Atoms {
    pub fullscreen: u32,
    pub max_vert: u32,
    pub max_horz: u32,
}

/// What a `_NET_WM_STATE` list says (maximized needs both directions, as EWMH has it).
pub fn read(window: u32, states: &[u32], a: &Atoms) -> Asked {
    Asked { window, fullscreen: states.contains(&a.fullscreen), maximized: states.contains(&a.max_vert) && states.contains(&a.max_horz) }
}

enum Job {
    Ask(u32),
    Close(u32),
    /// WM_STATE: Iconic (minimized) or Normal
    Iconic(u32, bool),
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
}

/// Whether a WM_PROTOCOLS list takes part in WM_DELETE_WINDOW.
pub fn takes_delete(protocols: &[u32], delete: u32) -> bool {
    protocols.contains(&delete)
}

impl Reader {
    /// Connects to XWayland's display `:n` on a new thread. None when the event loop refused the channel.
    pub fn start(display: u32, handle: &LoopHandle<'static, App>) -> Option<Reader> {
        let (tx, rx) = channel::<Job>();
        let (back, from_thread) = loop_channel::<Asked>();
        handle
            .insert_source(from_thread, |event, _, app| {
                if let Event::Msg(asked) = event {
                    app.x11_asked(asked);
                }
            })
            .ok()?;
        std::thread::Builder::new()
            .name("x11-asked".into())
            .spawn(move || {
                let Ok((conn, _)) = x11rb::rust_connection::RustConnection::connect(Some(&format!(":{display}"))) else {
                    eprintln!("yggdrasil-compositor: cannot read what X11 windows ask for before they show (no connection to :{display})");
                    return;
                };
                let atom = |name: &str| conn.intern_atom(false, name.as_bytes()).ok().and_then(|c| c.reply().ok()).map(|r| r.atom);
                let (Some(state), Some(fullscreen), Some(max_vert), Some(max_horz), Some(protocols), Some(delete), Some(wm_state_atom)) = (
                    atom("_NET_WM_STATE"),
                    atom("_NET_WM_STATE_FULLSCREEN"),
                    atom("_NET_WM_STATE_MAXIMIZED_VERT"),
                    atom("_NET_WM_STATE_MAXIMIZED_HORZ"),
                    atom("WM_PROTOCOLS"),
                    atom("WM_DELETE_WINDOW"),
                    atom("WM_STATE"),
                ) else {
                    return;
                };
                let atoms = Atoms { fullscreen, max_vert, max_horz };
                // ends when the compositor drops the Reader (the sender) or the X server goes away
                for job in rx {
                    let window = match job {
                        Job::Ask(w) => w,
                        Job::Iconic(w, iconic) => {
                            let _ = conn.change_property32(PropMode::REPLACE, w, wm_state_atom, wm_state_atom, &wm_state(iconic));
                            let _ = conn.flush();
                            continue;
                        }
                        Job::Close(w) => {
                            let listed: Vec<u32> = conn
                                .get_property(false, w, protocols, AtomEnum::ATOM, 0, 32)
                                .ok()
                                .and_then(|c| c.reply().ok())
                                .and_then(|r| r.value32().map(|v| v.collect()))
                                .unwrap_or_default();
                            if takes_delete(&listed, delete) {
                                let event = ClientMessageEvent::new(32, w, protocols, [delete, 0, 0, 0, 0]);
                                let _ = conn.send_event(false, w, EventMask::NO_EVENT, event);
                            } else {
                                let _ = conn.kill_client(w);
                            }
                            let _ = conn.flush();
                            continue;
                        }
                    };
                    let Ok(cookie) = conn.get_property(false, window, state, AtomEnum::ATOM, 0, 32) else { break };
                    let states: Vec<u32> = match cookie.reply() {
                        Ok(r) => r.value32().map(|v| v.collect()).unwrap_or_default(),
                        Err(_) => continue, // the window is already gone
                    };
                    let asked = read(window, &states, &atoms);
                    if (asked.fullscreen || asked.maximized) && back.send(asked).is_err() {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Reader { tx })
    }

    /// Ask what this window asked for (answered later, only when it asked for something).
    pub fn ask(&self, window: u32) {
        let _ = self.tx.send(Job::Ask(window));
    }

    /// Mark this window minimized (Iconic) or shown again (Normal) in WM_STATE.
    pub fn iconic(&self, window: u32, iconic: bool) {
        let _ = self.tx.send(Job::Iconic(window, iconic));
    }

    /// Close this window: asked politely when the program takes part in WM_DELETE_WINDOW, its connection ended otherwise.
    pub fn close(&self, window: u32) {
        let _ = self.tx.send(Job::Close(window));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_states_are_read() {
        let a = Atoms { fullscreen: 10, max_vert: 11, max_horz: 12 };
        assert_eq!(read(5, &[10], &a), Asked { window: 5, fullscreen: true, maximized: false });
        assert_eq!(read(5, &[11, 12], &a), Asked { window: 5, fullscreen: false, maximized: true });
        assert_eq!(read(5, &[11], &a), Asked { window: 5, fullscreen: false, maximized: false }, "one direction is not maximized");
        assert_eq!(read(5, &[], &a), Asked { window: 5, fullscreen: false, maximized: false });
    }

    #[test]
    fn wm_state_says_minimized_or_not() {
        assert_eq!(wm_state(true), [3, 0], "IconicState, no icon window");
        assert_eq!(wm_state(false), [1, 0], "NormalState");
    }

    #[test]
    fn closing_asks_only_programs_that_take_part() {
        assert!(takes_delete(&[3, 7], 7));
        assert!(!takes_delete(&[3], 7), "no WM_DELETE_WINDOW: the connection is ended instead");
        assert!(!takes_delete(&[], 7));
    }
}
