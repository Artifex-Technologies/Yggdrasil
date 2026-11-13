//! Hyprland, through its control socket ($XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.socket.sock): the same requests
//! `hyprctl` sends (`j/clients` for `hyprctl -j clients`, `/dispatch LUA` for `hyprctl dispatch LUA`, `/eval LUA` for `hyprctl
//! eval LUA`; recorded from hyprctl 0.57.1, tests/fixtures/hyprland-0.57.1/wire.txt), without starting a program for each. The
//! answers are read as Yggdrasil's tools always read hyprctl's (the same fields and the same defaults when one is missing).
//! A few tools spoke to the socket themselves before (the Task Manager, yggdrasil-xmove) with requests of their own form
//! ("dispatch ..." without the slash); they keep sending exactly those.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;

use crate::{Desktops, MonitorRef, Rect, Screen, Window};

/// Answers needed at once, while a window is dragged (the pointer, the window in front): as the zones always asked.
const QUICK: Duration = Duration::from_millis(200);
const NORMAL: Duration = Duration::from_secs(2);
/// The lock screen's splash asks whether the desktop is up while it waits: as it always asked.
const SPLASH: Duration = Duration::from_millis(500);

#[derive(Clone, Debug, PartialEq)]
pub struct Hyprland {
    pub socket: PathBuf,
    /// The runtime folder, where an answer from Hyprland's Lua is written (`keys_down`).
    pub runtime: PathBuf,
}

/// Hyprland's address of a window as the Lua dispatchers take it ("address:0x..."); only hex digits get through, so nothing of
/// a window's own can reach the Lua text.
pub fn address(id: &str) -> Option<String> {
    let hex = id.strip_prefix("0x")?;
    (!hex.is_empty() && hex.len() <= 16 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| format!("address:0x{hex}"))
}

/// A workspace as the dispatchers take it: a number.
fn workspace(id: &str) -> Option<i64> {
    id.parse::<i64>().ok()
}

/// A name that may go into a request line or Lua text as it is: letters, digits and `._-+`, not empty, at most 128.
pub fn plain_word(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || "._-+".contains(c))
}

impl Hyprland {
    fn ask(&self, request: &str, timeout: Duration) -> Option<String> {
        crate::sock::request(&self.socket, request.as_bytes(), timeout)
    }

    /// A request exactly as given (for the tools that wrote their own), bounded like every other.
    pub fn request(&self, raw: &str, timeout: Duration) -> Option<String> {
        self.ask(raw, timeout)
    }

    /// `hyprctl COMMAND...` without -j (`reload`, `setcursor NAME SIZE`, `configerrors`).
    pub fn command(&self, line: &str) -> Option<String> {
        self.ask(&format!("/{line}"), NORMAL)
    }

    /// Hyprland's event socket (.socket2.sock), beside the control socket.
    pub fn events_socket(&self) -> PathBuf {
        self.socket.with_file_name(".socket2.sock")
    }

    /// `hyprctl -j WHAT`.
    pub fn query(&self, what: &str) -> Option<String> {
        self.ask(&format!("j/{what}"), NORMAL)
    }

    /// `hyprctl dispatch LUA`: Hyprland's answer ("ok", or what went wrong).
    pub fn dispatch(&self, lua: &str) -> Option<String> {
        self.ask(&format!("/dispatch {lua}"), NORMAL)
    }

    /// `hyprctl eval LUA`.
    pub fn eval(&self, lua: &str) -> Option<String> {
        self.ask(&format!("/eval {lua}"), NORMAL)
    }

    pub fn windows(&self) -> Vec<Window> {
        self.query("clients").map(|j| parse_clients(&j)).unwrap_or_default()
    }

    /// The windows, or None when Hyprland did not answer or its answer is not a list.
    pub fn try_windows(&self) -> Option<Vec<Window>> {
        let v: Value = serde_json::from_str(&self.query("clients")?).ok()?;
        Some(v.as_array()?.iter().filter_map(client).collect())
    }

    pub fn active_window(&self, quick: bool) -> Option<Window> {
        parse_client_one(&self.ask("j/activewindow", if quick { QUICK } else { NORMAL })?)
    }

    pub fn screens(&self, quick: bool) -> Vec<Screen> {
        self.ask("j/monitors", if quick { QUICK } else { NORMAL }).map(|j| parse_monitors(&j)).unwrap_or_default()
    }

    pub fn cursor(&self) -> Option<(f64, f64)> {
        parse_cursor(&self.ask("j/cursorpos", QUICK)?)
    }

    pub fn desktops(&self) -> Desktops {
        parse_desktops(&self.query("workspaces").unwrap_or_default(), &self.query("activeworkspace").unwrap_or_default())
    }

    /// Shows desktop `id` (Hyprland makes a numbered workspace when it is first shown).
    pub fn show_desktop(&self, id: &str) {
        if id.parse::<u32>().is_ok() {
            self.dispatch(&format!(r#"hl.dsp.focus({{ workspace = "{id}" }})"#));
        }
    }

    pub fn send_to_desktop(&self, window: &str, to: &str) {
        let (Some(a), Some(to)) = (address(window), workspace(to)) else { return };
        self.dispatch(&format!(r#"hl.dsp.window.move({{ workspace = "{to}", follow = false, window = "{a}" }})"#));
    }

    /// The dock's minimize: Hyprland has no minimize of its own and ignores the request, so Yggdrasil's session keeps minimized
    /// windows on the hidden "minimized" workspace (Super+Down puts them there).
    pub fn minimize(&self, w: &Window) -> bool {
        let Some(a) = address(&w.id) else { return false };
        self.dispatch(&format!(r#"hl.dsp.window.move({{ workspace = "special:minimized", follow = false, window = "{a}" }})"#));
        true
    }

    /// Brings the window forward: a minimized one comes back to the workspace in front on the focused screen (and the hidden one
    /// closes if it was open), and it is focused and raised above the others (focusing alone leaves a floating window behind).
    pub fn restore(&self, w: &Window) -> bool {
        let Some(a) = address(&w.id) else { return false };
        if !w.minimized {
            self.dispatch(&format!(r#"hl.dsp.focus({{ window = "{a}" }})"#));
            self.dispatch("hl.dsp.window.bring_to_top()");
            return true;
        }
        let monitors: Value = self.query("monitors").and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
        let focused = monitors.as_array().and_then(|a| a.iter().find(|m| m["focused"].as_bool() == Some(true)).cloned());
        let ws = focused.as_ref().and_then(|m| m["activeWorkspace"]["id"].as_i64()).unwrap_or(1);
        if focused.as_ref().and_then(|m| m["specialWorkspace"]["name"].as_str()) == Some("special:minimized") {
            self.dispatch(r#"hl.dsp.workspace.toggle_special("minimized")"#);
        }
        self.dispatch(&format!(r#"hl.dsp.window.move({{ workspace = "{ws}", follow = false, window = "{a}" }})"#));
        self.dispatch(&format!(r#"hl.dsp.focus({{ window = "{a}" }})"#));
        self.dispatch("hl.dsp.window.bring_to_top()");
        true
    }

    /// Floating first (a tiled window cannot be placed), then its size, then its place (a resize keeps a window centred, so the
    /// other order would undo the move). Each request with Hyprland's answer, for the zones' trail.
    pub fn place(&self, window: &str, floating: bool, (x, y, w, h): Rect) -> Vec<(String, Option<String>)> {
        let Some(win) = address(window) else { return vec![] };
        let mut cmds = vec![];
        if !floating {
            cmds.push(format!(r#"hl.dsp.window.float({{ action = "enable", window = "{win}" }})"#));
        }
        cmds.push(format!(r#"hl.dsp.window.resize({{ x = {w}, y = {h}, window = "{win}" }})"#));
        cmds.push(format!(r#"hl.dsp.window.move({{ x = {x}, y = {y}, window = "{win}" }})"#));
        cmds.into_iter().map(|c| (c.clone(), self.dispatch(&c))).collect()
    }

    /// Whether any of these keys (xkb names) is held now, as Hyprland sees it: asked through its Lua, which answers into a file in
    /// the runtime folder (`tag` names the file, so two askers do not read each other's answer).
    pub fn keys_down(&self, keys: &[&str], tag: &str) -> Option<bool> {
        let safe = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || "_-".contains(c));
        if !keys.iter().all(|k| safe(k)) || !safe(tag) {
            return None;
        }
        let out = self.runtime.join(tag);
        let shown = out.to_str().filter(|p| !p.contains(['\'', '\\', '\n']))?.to_string();
        let _ = std::fs::remove_file(&out);
        let test = keys.iter().map(|k| format!("hl.is_key_down('{k}')")).collect::<Vec<_>>().join(" or ");
        self.eval(&format!("local f = io.open('{shown}', 'w') if f then f:write(tostring({test})) f:close() end"))?;
        let answer = std::fs::read_to_string(&out).ok()?;
        let _ = std::fs::remove_file(&out);
        match answer.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }

    /// `cursor:inactive_timeout` now (seconds; 0 = never hide).
    pub fn pointer_timeout(&self) -> Option<f32> {
        parse_option_float(&self.query("getoption cursor:inactive_timeout")?)
    }

    pub fn set_pointer_timeout(&self, secs: f32) {
        self.eval(&format!("hl.config({{ cursor = {{ inactive_timeout = {secs} }} }})"));
    }

    /// The Task Manager's close (the request it always sent): the window is asked, as its close button would.
    pub fn close_window(&self, window: &str) -> bool {
        let Some(a) = address(window) else { return false };
        self.ask(&format!("dispatch hl.dsp.window.close({{ window = '{a}' }})"), NORMAL).is_some_and(|r| r.trim() == "ok")
    }

    /// The Task Manager's "bring to the front".
    pub fn focus_window(&self, window: &str) -> bool {
        let Some(a) = address(window) else { return false };
        self.ask(&format!("dispatch hl.dsp.focus({{ window = '{a}' }})"), NORMAL).is_some_and(|r| r.trim() == "ok")
    }

    /// yggdrasil-logout's close of every window.
    pub fn ask_to_close(&self, window: &str) -> bool {
        let Some(a) = address(window) else { return false };
        self.dispatch(&format!("hl.dsp.window.close({{ window = \"{a}\" }})")).is_some()
    }

    /// Ends the session (Hyprland exits).
    pub fn exit(&self) -> bool {
        self.dispatch("hl.dsp.exit()").is_some()
    }

    /// Focuses the window of a class (a program's own window that is already open).
    pub fn focus_class(&self, class: &str) -> bool {
        plain_word(class) && self.dispatch(&format!("hl.dsp.focus({{ window = \"class:{class}\" }})")).is_some_and(|r| r.trim() == "ok")
    }

    /// Ctrl+V (Ctrl+Shift+V with `shift`: a terminal's paste) pressed in the window in front by the compositor itself.
    pub fn paste(&self, shift: bool) -> bool {
        let m = if shift { "CTRL SHIFT" } else { "CTRL" };
        self.dispatch(&format!("sendshortcut {m}, V, activewindow")).is_some()
    }

    /// Every screen, those switched off too (`monitors all`).
    pub fn screens_all(&self) -> Vec<Screen> {
        self.query("monitors all").map(|j| parse_monitors(&j)).unwrap_or_default()
    }

    /// Hyprland reads its config again: Ok, or what it answered.
    pub fn reload(&self) -> Result<(), String> {
        match self.command("reload") {
            Some(r) if r.trim() == "ok" => Ok(()),
            Some(r) => Err(r.trim().to_string()),
            None => Err("Hyprland did not answer".into()),
        }
    }

    /// The key bindings, as the shortcut lists read them.
    pub fn binds(&self) -> Option<Vec<yggdrasil_conf::shortcuts::Bind>> {
        Some(parse_binds(&serde_json::from_str::<Value>(&self.query("binds")?).ok()?))
    }

    /// The mistakes in the config (one per line; empty when there are none).
    pub fn config_errors(&self) -> Option<String> {
        self.command("configerrors").map(|t| t.trim().to_string())
    }

    /// The pointer's theme and size, now.
    pub fn set_cursor(&self, theme: &str, size: u32) -> bool {
        plain_word(theme) && self.command(&format!("setcursor {theme} {size}")).is_some()
    }

    /// Whether a panel or background of this namespace is shown on some screen.
    pub fn has_layer(&self, namespace: &str) -> Option<bool> {
        Some(self.ask("j/layers", SPLASH)?.contains(&format!("\"{namespace}\"")))
    }

    /// The keyboard layout the live session types with (the installer tries the one chosen).
    pub fn set_keyboard(&self, layout: &str, variant: &str) -> bool {
        self.eval(&format!("hl.config({{ input = {{ kb_layout = {layout:?}, kb_variant = {variant:?} }} }})")).is_some()
    }

    // The session's own keys (yggdrasil-snap, yggdrasil-close): they act on the focused window.

    pub fn focus_direction(&self, d: char) -> bool {
        matches!(d, 'l' | 'r' | 'u' | 'd') && self.dispatch(&format!("hl.dsp.focus({{ direction = \"{d}\" }})")).is_some()
    }

    pub fn maximize_active(&self) -> bool {
        self.dispatch(r#"hl.dsp.window.fullscreen({ mode = "maximized" })"#).is_some()
    }

    pub fn minimize_active(&self) -> bool {
        self.dispatch(r#"hl.dsp.window.move({ workspace = "special:minimized", follow = false })"#).is_some()
    }

    pub fn close_active(&self) -> bool {
        self.dispatch("hl.dsp.window.close()").is_some()
    }

    /// The size first, then the place (a resize keeps a window centred).
    pub fn snap_active(&self, (x, y, w, h): Rect) -> bool {
        self.dispatch(&format!("hl.dsp.window.resize({{ x = {w}, y = {h} }})"));
        self.dispatch(&format!("hl.dsp.window.move({{ x = {x}, y = {y} }})")).is_some()
    }
}

/// One window of `clients` (or `activewindow`); None without an address.
fn client(c: &Value) -> Option<Window> {
    let pair = |v: &Value| Some((v.get(0)?.as_f64()?, v.get(1)?.as_f64()?));
    let ws_name = c["workspace"]["name"].as_str().unwrap_or("").to_string();
    Some(Window {
        id: c["address"].as_str()?.to_string(),
        class: c["class"].as_str().unwrap_or("").to_string(),
        initial_class: c["initialClass"].as_str().unwrap_or("").to_string(),
        title: c["title"].as_str().unwrap_or("").to_string(),
        workspace: c["workspace"]["id"].as_i64(),
        minimized: ws_name == "special:minimized",
        special: ws_name.starts_with("special:"),
        at: pair(&c["at"]),
        size: pair(&c["size"]),
        floating: c["floating"].as_bool(),
        fullscreen: c["fullscreen"].as_i64().unwrap_or(0) != 0,
        mapped: c["mapped"].as_bool().unwrap_or(true),
        hidden: c["hidden"].as_bool().unwrap_or(false),
        monitor: c["monitor"].as_i64().map(MonitorRef::Id),
        focus_order: c["focusHistoryID"].as_i64().unwrap_or(i64::MAX),
        pid: c["pid"].as_i64().filter(|&p| p > 0 && p <= u32::MAX as i64).map(|p| p as u32),
    })
}

/// `clients`: every window, in Hyprland's order.
pub fn parse_clients(json: &str) -> Vec<Window> {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    v.as_array().into_iter().flatten().filter_map(client).collect()
}

/// `activewindow`: the window in front ("{}" when there is none).
pub fn parse_client_one(json: &str) -> Option<Window> {
    client(&serde_json::from_str::<Value>(json).ok()?)
}

/// `monitors`: the screens (one without a name, place or size is left out).
pub fn parse_monitors(json: &str) -> Vec<Screen> {
    let v: Value = serde_json::from_str(json).unwrap_or_default();
    let Some(a) = v.as_array() else { return vec![] };
    a.iter()
        .filter_map(|m| {
            let r = m["reserved"].as_array();
            let rv = |i: usize| r.and_then(|r| r.get(i)).and_then(|v| v.as_f64()).unwrap_or(0.0).round() as i32;
            let special = m["specialWorkspace"]["name"].as_str().unwrap_or("");
            Some(Screen {
                id: m["id"].as_i64(),
                name: m["name"].as_str()?.to_string(),
                x: m["x"].as_i64()? as i32,
                y: m["y"].as_i64()? as i32,
                width: m["width"].as_u64()? as u32,
                height: m["height"].as_u64()? as u32,
                scale: m["scale"].as_f64(),
                transform: m["transform"].as_u64().unwrap_or(0) as u32,
                reserved: [rv(0), rv(1), rv(2), rv(3)],
                focused: m["focused"].as_bool() == Some(true),
                disabled: m["disabled"].as_bool().unwrap_or(false),
                active_workspace: m["activeWorkspace"]["id"].as_i64(),
                special_workspace: (!special.is_empty()).then(|| special.to_string()),
                special_workspace_id: m["specialWorkspace"]["id"].as_i64().filter(|id| *id != 0),
                refresh: m["refreshRate"].as_f64(),
                modes: m["availableModes"]
                    .as_array()
                    .map(|l| l.iter().filter_map(|x| x.as_str().map(str::to_string)).collect())
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// `binds`: what the shortcut lists show (a key given by its code, such as the number row's, has no name: `code:N`).
pub fn parse_binds(v: &Value) -> Vec<yggdrasil_conf::shortcuts::Bind> {
    v.as_array()
        .map(|a| a.as_slice())
        .unwrap_or(&[])
        .iter()
        .map(|b| yggdrasil_conf::shortcuts::Bind {
            modmask: b["modmask"].as_u64().unwrap_or(0) as u32,
            key: b["key"]
                .as_str()
                .filter(|k| !k.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("code:{}", b["keycode"].as_u64().unwrap_or(0))),
            desc: b["description"].as_str().unwrap_or("").to_string(),
            release: b["release"].as_bool().unwrap_or(false),
        })
        .collect()
}

pub fn parse_cursor(json: &str) -> Option<(f64, f64)> {
    let v: Value = serde_json::from_str(json).ok()?;
    Some((v["x"].as_f64()?, v["y"].as_f64()?))
}

/// `workspaces` and `activeworkspace`: the numbered workspaces that exist (they do while they hold a window or are shown) and the
/// one in front, in order, and the position of the one in front. The hidden ones ("special:minimized") are not desktops.
pub fn parse_desktops(workspaces: &str, active: &str) -> Desktops {
    let mut ids: Vec<i64> = serde_json::from_str::<Value>(workspaces)
        .ok()
        .and_then(|v| v.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|w| w["id"].as_i64())
        .filter(|&id| id >= 1)
        .collect();
    let current = serde_json::from_str::<Value>(active).ok().and_then(|v| v["id"].as_i64()).filter(|&id| id >= 1);
    ids.extend(current);
    ids.sort_unstable();
    ids.dedup();
    let at = current.and_then(|c| ids.iter().position(|&id| id == c)).unwrap_or(0);
    Desktops { ids, at }
}

/// `getoption NAME` for a number: its "float" field.
pub fn parse_option_float(out: &str) -> Option<f32> {
    let at = out.find("\"float\"")?;
    let rest = out[at + 7..].trim_start_matches([':', ' ']);
    rest.split([',', '}', '\n']).next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    // (moved with the parsing from the panel's status.rs and zones.rs)

    #[test]
    fn desktops_and_the_current_one_are_read() {
        let ws = r#"[{"id":3,"name":"3","windows":1},{"id":1,"name":"1","windows":3},{"id":-98,"name":"special:minimized","windows":1}]"#;
        let d = parse_desktops(ws, r#"{"id":2,"name":"2"}"#);
        assert_eq!(d, Desktops { ids: vec![1, 2, 3], at: 1 }, "the shown one is a desktop even while empty; hidden ones are not");
        assert_eq!(parse_desktops("nonsense", ""), Desktops::default());
    }

    #[test]
    fn the_window_in_front_is_read() {
        let w = parse_client_one(r#"{"address":"0x55","at":[120,48],"size":[800,600]}"#).unwrap();
        assert_eq!((w.id.as_str(), w.at, w.size), ("0x55", Some((120.0, 48.0)), Some((800.0, 600.0))));
        assert_eq!((w.floating, w.fullscreen, w.mapped, w.hidden), (None, false, true, false), "missing fields as before");
        assert_eq!(parse_client_one("{}"), None);
    }

    #[test]
    fn hyprland_answers_are_read() {
        let j = r#"[{"id":0,"name":"DP-1","x":0,"y":0,"width":3440,"height":1440,"scale":1.00,"transform":0,"reserved":[0,28,0,70]}]"#;
        let m = parse_monitors(j);
        assert_eq!(m[0].monitor().free_area(), (0, 28, 3440, 1342));
        assert_eq!(parse_monitors(r#"[{"name":"DP-2","x":0,"y":0}]"#), vec![], "a screen without a size is left out");
        assert_eq!(parse_cursor(r#"{"x": 1720.5, "y": 12}"#), Some((1720.5, 12.0)));
        assert_eq!(parse_cursor("garbage"), None);
        assert_eq!(parse_option_float(r#"{"option": "cursor:inactive_timeout", "float": 0.250000, "set": true }"#), Some(0.25));
        assert_eq!(parse_option_float("no"), None);
    }

    #[test]
    fn only_an_address_reaches_the_lua() {
        assert_eq!(address("0x5a0000001000").as_deref(), Some("address:0x5a0000001000"));
        for bad in ["5a00", "0x", "0xzz", "0x1\" })", "0x1 or true"] {
            assert_eq!(address(bad), None, "{bad}");
        }
    }
}
