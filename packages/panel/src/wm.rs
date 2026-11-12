//! The compositor for the panel, through yggdrasil-wm (packages/wm): Hyprland or Yggdrasil's own, whichever runs. The bar's
//! desktops, the dock's minimize and restore where the compositor needs its own (Hyprland ignores wlr-foreign-toplevel's minimize:
//! Yggdrasil's session keeps minimized windows on the hidden "minimized" workspace), the focused screen.

use yggdrasil_wm::{Desk, Window};

/// The screen that has the focus: its name.
pub fn focused_monitor() -> Option<String> {
    Desk::detect()?.focused_screen().map(|s| s.name)
}

/// The dock's minimize: true when the compositor's own way was taken (else the caller asks through wlr-foreign-toplevel).
pub fn minimize(app_id: &str, title: &str) -> bool {
    Desk::detect().is_some_and(|d| d.minimize_app(app_id, title))
}

/// The dock's activate, from minimized too: true when the compositor's own way was taken.
pub fn restore(app_id: &str, title: &str) -> bool {
    Desk::detect().is_some_and(|d| d.restore_app(app_id, title))
}

/// Shows desktop `id`.
pub fn show_desktop(id: &str) {
    if let Some(d) = Desk::detect() {
        d.show_desktop(id);
    }
}

/// Removes desktop `id` the way the bar always did: its windows move to the desktop before it (the one after, for the first),
/// and the emptied workspace goes away by itself once it is not shown.
pub fn remove_desktop(id: &str, desktops: &[crate::status::Desktop]) {
    let Some(i) = desktops.iter().position(|d| d.id == id) else { return };
    let to = if i > 0 {
        desktops[i - 1].id.clone()
    } else if desktops.len() > 1 {
        desktops[1].id.clone()
    } else {
        return;
    };
    let Some(d) = Desk::detect() else { return };
    for w in windows_on(&d.windows(), id) {
        d.send_to_desktop(&w, &to);
    }
    d.show_desktop(&to);
}

fn windows_on(windows: &[Window], id: &str) -> Vec<String> {
    let id: i64 = id.parse().unwrap_or(0);
    windows.iter().filter(|w| w.workspace == Some(id)).map(|w| w.id.clone()).collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_windows_of_a_desktop_are_found() {
        let c = yggdrasil_wm::hyprland::parse_clients(
            r#"[{"address":"0xa","workspace":{"id":2}},{"address":"0xb","workspace":{"id":1}},{"address":"0xc","workspace":{"id":2}}]"#,
        );
        assert_eq!(super::windows_on(&c, "2"), ["0xa", "0xc"]);
    }
}
