//! What an X11 window asked for before it was shown: `_NET_WM_STATE` set on the window before it is mapped (a game that wants
//! to start full screen, a program that wants to start maximized). EWMH says the window manager reads it when the window is
//! mapped; Smithay's window manager does not, so a game that asked would open as an ordinary window.
//!
//! The property is read on a thread of its own, over its own connection to XWayland, so the compositor never waits for the X
//! server (XWayland is also a client of the compositor: a blocking read here could wait on a server that waits on us). The
//! answer comes back to the event loop through a channel and is acted on then, a moment after the window is shown.

use std::sync::mpsc::{channel, Sender};

use smithay::reexports::calloop::channel::{channel as loop_channel, Event};
use smithay::reexports::calloop::LoopHandle;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt};

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
    Asked {
        window,
        fullscreen: states.contains(&a.fullscreen),
        maximized: states.contains(&a.max_vert) && states.contains(&a.max_horz),
    }
}

/// The thread that reads the property; dropped with the compositor.
pub struct Reader {
    tx: Sender<u32>,
}

impl Reader {
    /// Connects to XWayland's display `:n` on a new thread. None when the event loop refused the channel.
    pub fn start(display: u32, handle: &LoopHandle<'static, App>) -> Option<Reader> {
        let (tx, rx) = channel::<u32>();
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
                let (Some(state), Some(fullscreen), Some(max_vert), Some(max_horz)) =
                    (atom("_NET_WM_STATE"), atom("_NET_WM_STATE_FULLSCREEN"), atom("_NET_WM_STATE_MAXIMIZED_VERT"), atom("_NET_WM_STATE_MAXIMIZED_HORZ"))
                else {
                    return;
                };
                let atoms = Atoms { fullscreen, max_vert, max_horz };
                // ends when the compositor drops the Reader (the sender) or the X server goes away
                for window in rx {
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
        let _ = self.tx.send(window);
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
}
