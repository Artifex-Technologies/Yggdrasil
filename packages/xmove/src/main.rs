//! yggdrasil-xmove: X11 programs that draw their own title bar (Steam, some games' launchers) move and resize their window by
//! asking the window manager with `_NET_WM_MOVERESIZE` when you press on their title or edge. Hyprland 0.56 lists that request
//! as supported but does nothing with it, so those windows could not be dragged. This watches the root window for the request
//! (any program may listen for it) and starts Hyprland's own drag, the one Super+drag uses; letting go of the button ends it.
//! It sleeps until such a request comes and needs no other permission.

use std::process::{Command, Stdio};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask};
use x11rb::protocol::Event;

/// What `_NET_WM_MOVERESIZE` asks for (its third number): 0-7 resize from that edge or corner, 8 move, 9-10 the same from the
/// keyboard, 11 cancel.
#[derive(Debug, PartialEq)]
enum Ask {
    Move,
    Resize,
    Other,
}

fn ask(direction: u32) -> Ask {
    match direction {
        0..=7 => Ask::Resize,
        8 => Ask::Move,
        _ => Ask::Other,
    }
}

fn dispatch(lua: &str) {
    let child = Command::new("hyprctl").args(["dispatch", lua]).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Ok(mut c) = child {
        let _ = c.wait();
    }
}

fn main() {
    if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_none() {
        eprintln!("yggdrasil-xmove: not in Hyprland, nothing to do");
        return;
    }
    // Xwayland may come up a moment after the session's programs, and comes back if it ever restarts: keep (re)connecting
    loop {
        if let Err(e) = watch() {
            eprintln!("yggdrasil-xmove: {e}; trying again");
        }
        std::thread::sleep(std::time::Duration::from_secs(3));
    }
}

fn watch() -> Result<(), String> {
    let (conn, screen) = x11rb::connect(None).map_err(|e| format!("no X11 display ({e})"))?;
    let root = conn.setup().roots[screen].root;
    let atom = conn.intern_atom(false, b"_NET_WM_MOVERESIZE").map_err(|e| e.to_string())?.reply().map_err(|e| e.to_string())?.atom;
    // the request is sent to the root window for SubstructureRedirect | SubstructureNotify; anyone may select the second
    conn.change_window_attributes(root, &ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY)).map_err(|e| e.to_string())?;
    conn.flush().map_err(|e| e.to_string())?;
    eprintln!("yggdrasil-xmove: X11 windows can be dragged by their own title bar");
    loop {
        let event = conn.wait_for_event().map_err(|e| format!("the X11 display went away ({e})"))?;
        if let Event::ClientMessage(m) = event {
            if m.type_ != atom || m.format != 32 {
                continue;
            }
            match ask(m.data.as_data32()[2]) {
                Ask::Move => dispatch("hl.dsp.window.drag()"),
                Ask::Resize => dispatch("hl.dsp.window.resize()"),
                Ask::Other => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_read_as_the_spec_says() {
        assert_eq!(ask(8), Ask::Move);
        assert_eq!(ask(0), Ask::Resize);
        assert_eq!(ask(7), Ask::Resize);
        assert_eq!(ask(9), Ask::Other, "moving from the keyboard is left alone");
        assert_eq!(ask(11), Ask::Other, "cancel");
    }
}
