//! The desktop commands of the control socket: what Yggdrasil's own tools ask of the compositor and tell it to do, through the
//! compositor-neutral layer `yggdrasil-wm` (packages/wm), which speaks Hyprland's socket on Hyprland and these commands here.
//! Unlike the test commands of headless.rs (input made inside the compositor, screenshots, output sizes, quit), none of them
//! makes input or reaches outside the compositor: they read the windows, the workspaces and the screens, and move, close,
//! minimize, maximize, place and activate windows, which any program can already ask for its own windows (xdg-shell) or the
//! panel for every window (wlr-foreign-toplevel).
//!
//!   clients                     one line per window: id, app id, title, place, size, workspace (1-based), states, output
//!                               (tab separated; states as `windows`, comma separated)
//!   active                      the window with the keyboard, as a `clients` line (no line when none has it)
//!   workspaces                  one line per workspace: number (1-based), how many windows it holds, "shown" or "-"
//!   workspace N                 show workspace N
//!   activate ID                 bring a window forward (from minimized or another workspace too) and give it the keyboard
//!   close ID                    ask a window to close
//!   minimize ID
//!   maximize ID on|off|toggle
//!   send ID N                   move a window to workspace N
//!   place ID X Y W H            a plain floating window at X,Y (logical, in the space of all outputs), W x H
//!   cursor                      where the pointer is: X Y
//!   screens                     one line per output: name, place, mode size (pixels), scale, the strips panels reserve
//!                               (left, top, right, bottom) and "focused" or "-" (tab separated)
//!   pointer-timeout [SECONDS]   how long the pointer may rest before it is hidden (0: never); with a number, set it
//!   modifiers                   the modifier keys held now: super, ctrl, alt, shift (one line, space separated)
//! ID is a number from `clients`, or `active` for the window with the keyboard.

use smithay::desktop::Window;
use smithay::reexports::wayland_protocols::xdg::shell::server::xdg_toplevel;
use smithay::utils::Rectangle;

use crate::state::{self, App, WORKSPACES};

/// A window's number for the control socket: given the first time it is asked for, kept as long as the window lives, never
/// given twice.
struct WinId(u64);

pub fn window_id(w: &Window) -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(1);
    w.user_data().insert_if_missing(|| WinId(NEXT.fetch_add(1, Ordering::Relaxed)));
    w.user_data().get::<WinId>().map(|i| i.0).unwrap_or(0)
}

/// Text that goes into a tab-separated line: no tabs, line breaks or other control characters (a window's title is the
/// program's to choose, and must not be able to add a line or a field to the answer).
pub fn field(s: &str) -> String {
    s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect()
}

/// A desktop command, or None when `words` is not one (headless.rs then tries its own).
pub fn command(words: &[&str], app: &mut App) -> Option<Result<Vec<String>, String>> {
    let r = match words {
        ["clients"] => Ok(clients(app).into_iter().map(|(_, line)| line).collect()),
        ["active"] => {
            let focus = app.focused_surface();
            Ok(clients(app)
                .into_iter()
                .filter(|(w, _)| focus.as_ref().is_some_and(|f| state::is_window_of(w, f)))
                .map(|(_, line)| line)
                .collect())
        }
        ["workspaces"] => {
            let infos = app.window_infos();
            Ok((0..WORKSPACES)
                .map(|ws| {
                    let n = infos.iter().filter(|i| i.workspace == ws).count();
                    format!("{}\t{n}\t{}", ws + 1, if ws == app.workspace() { "shown" } else { "-" })
                })
                .collect())
        }
        ["workspace", n] => workspace_number(n).map(|n| {
            app.switch_workspace(n);
            vec![]
        }),
        ["activate", id] => find(app, id).map(|(_, s)| {
            app.activate_surface(&s);
            vec![]
        }),
        ["close", id] => find(app, id).map(|(_, s)| {
            app.close_surface(&s);
            vec![]
        }),
        ["minimize", id] => find(app, id).map(|(_, s)| {
            app.minimize_surface(&s);
            vec![]
        }),
        ["maximize", id, how] => find(app, id).and_then(|(w, s)| {
            let on = match *how {
                "on" => true,
                "off" => false,
                "toggle" => !state::win_has(&w, xdg_toplevel::State::Maximized),
                v => return Err(format!("{v}: on, off or toggle")),
            };
            app.set_maximized(&s, on);
            Ok(vec![])
        }),
        ["send", id, n] => find(app, id).and_then(|(_, s)| {
            let n = workspace_number(n)?;
            app.move_to_workspace(&s, n);
            Ok(vec![])
        }),
        ["place", id, x, y, w, h] => find(app, id).and_then(|(win, _)| {
            let num = |v: &str| v.parse::<i32>().map_err(|_| format!("{v}: a whole number"));
            let (x, y, w, h) = (num(x)?, num(y)?, num(w)?, num(h)?);
            if !(1..=16384).contains(&w) || !(1..=16384).contains(&h) || x.abs() > 65536 || y.abs() > 65536 {
                return Err("place: the size must be 1 to 16384, the place within 65536 of the origin".into());
            }
            if app.place_window(&win, Rectangle::new((x, y).into(), (w, h).into())) {
                Ok(vec![])
            } else {
                Err("place: the window is not on the workspace showing".into())
            }
        }),
        ["cursor"] => Ok(vec![format!("{} {}", app.pointer_location.x, app.pointer_location.y)]),
        ["screens"] => Ok(screens(app)),
        ["pointer-timeout"] => Ok(vec![app.pointer_timeout.to_string()]),
        ["pointer-timeout", secs] => match secs.parse::<f64>() {
            Ok(v) if v.is_finite() && (0.0..=86_400.0).contains(&v) => {
                app.pointer_timeout = v;
                Ok(vec![])
            }
            _ => Err(format!("{secs}: seconds, 0 to 86400")),
        },
        ["modifiers"] => {
            let m = app.keyboard.modifier_state();
            let held: Vec<&str> = [(m.logo, "super"), (m.ctrl, "ctrl"), (m.alt, "alt"), (m.shift, "shift")]
                .into_iter()
                .filter(|h| h.0)
                .map(|h| h.1)
                .collect();
            Ok(vec![held.join(" ")])
        }
        _ => return None,
    };
    Some(r)
}

/// "1" to "4": the 0-based workspace.
fn workspace_number(n: &str) -> Result<usize, String> {
    n.parse::<usize>()
        .ok()
        .filter(|n| (1..=WORKSPACES).contains(n))
        .map(|n| n - 1)
        .ok_or_else(|| format!("{n}: a workspace from 1 to {WORKSPACES}"))
}

/// The window an ID names (a number from `clients`, or `active`), and its surface.
fn find(app: &App, id: &str) -> Result<(Window, smithay::reexports::wayland_server::protocol::wl_surface::WlSurface), String> {
    let found = if id == "active" {
        app.focused_surface().and_then(|f| app.window_for(&f))
    } else {
        let n: u64 = id.parse().map_err(|_| format!("{id}: a window number from clients, or active"))?;
        app.all_windows().into_iter().map(|(w, _)| w).find(|w| window_id(w) == n)
    };
    let w = found.ok_or_else(|| format!("no window {id}"))?;
    let s = state::surface_of(&w).ok_or_else(|| format!("window {id} has no surface yet"))?;
    Ok((w, s))
}

/// Every window with its `clients` line.
fn clients(app: &App) -> Vec<(Window, String)> {
    app.window_infos()
        .into_iter()
        .map(|i| {
            let (title, app_id) = state::title_app(&i.window);
            let size = i.window.geometry().size;
            let output = state::surface_of(&i.window).map(|s| app.output_of(&s)).unwrap_or_else(|| app.output.clone());
            let line = format!(
                "{}\t{}\t{}\t{},{}\t{}x{}\t{}\t{}\t{}",
                window_id(&i.window),
                field(&app_id),
                field(&title),
                i.at.x,
                i.at.y,
                size.w,
                size.h,
                i.workspace + 1,
                i.states.join(","),
                field(&output.name())
            );
            (i.window, line)
        })
        .collect()
}

/// The outputs, with the strips the panels reserve and which one has the focus: the output of the window with the keyboard,
/// else the one under the pointer.
fn screens(app: &App) -> Vec<String> {
    let focused = app
        .focused_surface()
        .and_then(|f| app.window_for(&f))
        .and_then(|w| state::surface_of(&w))
        .map(|s| app.output_of(&s))
        .unwrap_or_else(|| app.output_at(app.pointer_location));
    app.space
        .outputs()
        .map(|o| {
            let r = state::output_rect(o);
            let free = app.free_area(o);
            let mode = o.current_mode().map(|m| m.size).unwrap_or_default();
            let (l, t) = (free.loc.x - r.loc.x, free.loc.y - r.loc.y);
            let (rr, b) = ((r.loc.x + r.size.w) - (free.loc.x + free.size.w), (r.loc.y + r.size.h) - (free.loc.y + free.size.h));
            format!(
                "{}\t{},{}\t{}x{}\t{}\t{l},{t},{rr},{b}\t{}",
                field(&o.name()),
                r.loc.x,
                r.loc.y,
                mode.w,
                mode.h,
                o.current_scale().fractional_scale(),
                if *o == focused { "focused" } else { "-" }
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_title_cannot_add_a_line_or_a_field() {
        assert_eq!(super::field("a\tb\nok\r"), "a b ok ");
        assert_eq!(super::field("Ünïcode – fine"), "Ünïcode – fine");
    }
}
