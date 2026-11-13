//! yggdrasil-close: closes the focused window (Super+Q, Alt+F4, Super+W), except for programs that should keep running in the
//! background: their window is minimized instead (as Super+Down does), so music keeps playing and Fylgja keeps showing what plays.
//! Click the program in the dock to bring it back; quit it from its own menu.
//!
//! The list: ~/.config/yggdrasil/keep-running, one window class per line (# comments). Without the file: Spotify.
//! On whichever compositor runs (yggdrasil-wm).

use yggdrasil_conf::keep_running;
use yggdrasil_wm::Desk;

fn main() {
    let Some(desk) = Desk::detect() else { std::process::exit(1) };
    let class = desk.active_window(false).map(|w| w.class).unwrap_or_default();
    let done = if keep_running::keeps(&keep_running::load(), &class) { desk.minimize_active() } else { desk.close_active() };
    std::process::exit(if done { 0 } else { 1 });
}
