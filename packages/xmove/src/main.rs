//! yggdrasil-xmove: X11 programs that draw their own title bar (Steam, some games' launchers) move and resize their window by
//! asking the window manager with `_NET_WM_MOVERESIZE` when you press on their title or edge. Hyprland 0.56 lists that request
//! as supported but does nothing with it, so those windows could not be dragged. This watches the root window for the request
//! (any program may listen for it) and moves the window with the pointer itself, through Hyprland's socket, until the button is
//! let go (Hyprland's own drag cannot be started from outside a Super+mouse shortcut). It sleeps until such a request comes.

use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ChangeWindowAttributesAux, ConnectionExt, EventMask, KeyButMask, Window};
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

/// One request to Hyprland's control socket (what hyprctl does, without starting a program 100 times a second).
fn hypr(request: &str) -> Option<String> {
    let dir = std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr").join(std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?);
    let mut s = UnixStream::connect(dir.join(".socket.sock")).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(1))).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    Some(out)
}

/// A number after `"key":` in Hyprland's JSON (enough for the few fields read here).
fn json_ints(json: &str, key: &str) -> Vec<i64> {
    let Some(i) = json.find(&format!("\"{key}\":")) else { return vec![] };
    let rest = &json[i + key.len() + 3..];
    let rest = rest.trim_start().trim_start_matches('[');
    rest.split(|c| c == ']' || c == '}' || c == '\n').next().unwrap_or("").split(',').filter_map(|n| n.trim().parse().ok()).collect()
}

fn json_str(json: &str, key: &str) -> Option<String> {
    let i = json.find(&format!("\"{key}\":"))?;
    let rest = json[i + key.len() + 3..].trim_start().strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// Moves the focused window (the one just pressed on) with the pointer until the button is let go, the program cancels, or
/// half a minute passed.
fn follow<C: Connection>(conn: &C, root: Window, button: u32) {
    let Some(active) = hypr("j/activewindow") else { return };
    let (Some(addr), at) = (json_str(&active, "address"), json_ints(&active, "at")) else { return };
    if at.len() < 2 {
        return;
    }
    let Some(start) = hypr("j/cursorpos").map(|c| (json_ints(&c, "x"), json_ints(&c, "y"))) else { return };
    let (Some(&sx), Some(&sy)) = (start.0.first(), start.1.first()) else { return };
    let (wx, wy) = (at[0], at[1]);
    let held = match button {
        2 => KeyButMask::BUTTON2,
        3 => KeyButMask::BUTTON3,
        _ => KeyButMask::BUTTON1,
    };
    let began = Instant::now();
    let mut last = (wx, wy);
    while began.elapsed() < Duration::from_secs(30) {
        let Ok(Ok(p)) = conn.query_pointer(root).map(|c| c.reply()) else { return };
        if !p.mask.contains(held) {
            return;
        }
        if let Some(c) = hypr("j/cursorpos") {
            if let (Some(&x), Some(&y)) = (json_ints(&c, "x").first(), json_ints(&c, "y").first()) {
                let to = (wx + x - sx, wy + y - sy);
                if to != last {
                    hypr(&format!(r#"dispatch hl.dsp.window.move({{ x = {}, y = {}, window = "address:{addr}" }})"#, to.0, to.1));
                    last = to;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(7));
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
            let d = m.data.as_data32();
            // resizing from an edge works already (Hyprland's own edge grips); moving is done here
            if ask(d[2]) == Ask::Move {
                follow(&conn, root, d[3]);
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

    #[test]
    fn reads_hyprlands_answers() {
        let w = "{\n    \"address\": \"0x55f99d8c7680\",\n    \"mapped\": true,\n    \"at\": [104, 120],\n    \"size\": [1323, 1082]\n}";
        assert_eq!(json_str(w, "address").as_deref(), Some("0x55f99d8c7680"));
        assert_eq!(json_ints(w, "at"), vec![104, 120]);
        let c = "{\n    \"x\": 1034,\n    \"y\": -3\n}";
        assert_eq!(json_ints(c, "x"), vec![1034]);
        assert_eq!(json_ints(c, "y"), vec![-3]);
    }
}
