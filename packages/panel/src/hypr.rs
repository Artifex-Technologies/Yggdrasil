//! Minimizing in Hyprland, which has no minimize of its own and ignores the request: Yggdrasil's session keeps minimized windows
//! on the hidden "minimized" workspace (Super+Down puts them there). The dock does the same, and brings a window back from there
//! to the workspace in front when it is clicked.

use std::process::{Command, Stdio};

pub fn running() -> bool {
    std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some()
}

fn hyprctl(args: &[&str]) -> Option<String> {
    let out = Command::new("hyprctl").args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Hyprland's window for one in the window list (the same class and title; the class alone when only one window has it), and
/// whether it is minimized.
fn find(app_id: &str, title: &str) -> Option<(String, bool)> {
    let clients: serde_json::Value = serde_json::from_str(&hyprctl(&["-j", "clients"])?).ok()?;
    let list = clients.as_array()?;
    let class = |c: &serde_json::Value| c["class"].as_str() == Some(app_id) || c["initialClass"].as_str() == Some(app_id);
    let c = list.iter().find(|c| class(c) && c["title"].as_str() == Some(title)).or_else(|| {
        let same: Vec<&serde_json::Value> = list.iter().filter(|c| class(c)).collect();
        (same.len() == 1).then(|| same[0])
    })?;
    Some((c["address"].as_str()?.to_string(), pick_minimized(c)))
}

fn pick_minimized(c: &serde_json::Value) -> bool {
    c["workspace"]["name"].as_str() == Some("special:minimized")
}

fn dispatch(lua: &str) {
    let _ = hyprctl(&["dispatch", lua]);
}

/// Puts the window on the hidden "minimized" workspace. False when it was not found (then the caller asks the usual way).
pub fn minimize(app_id: &str, title: &str) -> bool {
    let Some((addr, _)) = find(app_id, title) else { return false };
    dispatch(&format!(r#"hl.dsp.window.move({{ workspace = "special:minimized", follow = false, window = "address:{addr}" }})"#));
    true
}

/// A minimized window comes back to the workspace in front on the focused screen (and the hidden one closes if it was open).
/// Returns whether it was minimized; the caller focuses it either way.
pub fn restore(app_id: &str, title: &str) -> bool {
    let Some((addr, true)) = find(app_id, title) else { return false };
    let monitors: serde_json::Value = hyprctl(&["-j", "monitors"]).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    let focused = monitors.as_array().and_then(|a| a.iter().find(|m| m["focused"].as_bool() == Some(true)).cloned());
    let ws = focused.as_ref().and_then(|m| m["activeWorkspace"]["id"].as_i64()).unwrap_or(1);
    if focused.as_ref().and_then(|m| m["specialWorkspace"]["name"].as_str()) == Some("special:minimized") {
        dispatch(r#"hl.dsp.workspace.toggle_special("minimized")"#);
    }
    dispatch(&format!(r#"hl.dsp.window.move({{ workspace = "{ws}", follow = false, window = "address:{addr}" }})"#));
    dispatch(&format!(r#"hl.dsp.focus({{ window = "address:{addr}" }})"#));
    true
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_window_on_the_hidden_workspace_is_minimized() {
        let c: serde_json::Value = serde_json::from_str(r#"{"workspace":{"id":-98,"name":"special:minimized"}}"#).unwrap();
        assert!(super::pick_minimized(&c));
        let c: serde_json::Value = serde_json::from_str(r#"{"workspace":{"id":1,"name":"1"}}"#).unwrap();
        assert!(!super::pick_minimized(&c));
    }
}
