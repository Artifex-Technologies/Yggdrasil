# Window zones

PowerToys' FancyZones, rewritten for Yggdrasil in Rust: drag a window to the notch (Fylgja) at the top of a screen and a sheet of
layouts opens under it; drop the window on a zone and it fills that part of the screen. Super+Shift+Z opens the same sheet for the
window in front (then click a zone; Esc cancels). Settings > Displays > Window zones turns it on or off, sets the space around
windows and picks which layouts the sheet shows (`~/.config/yggdrasil/zones.conf`).

## How a drag is noticed (Hyprland 0.56)

What Hyprland offers, and what was ruled out:

- **No drag event.** The Lua API's events (`/usr/share/hypr/stubs/hl.meta.lua`, `HL.EventName`) cover windows opening, closing,
  focus, titles, workspaces, monitors and keys; none fires when a window starts or stops moving. socket2 has none either
  (`movewindow` is a move to another workspace).
- **The panel cannot see the pointer during a drag.** The compositor holds the pointer while a window moves (an app's own title
  bar drag and Super+drag alike), so the panel's layer surfaces get no enter or motion events.
- **Window rules added at run time** (`hyprctl eval 'hl.window_rule(...)'`) did not apply to new windows in tests on 2026-10-25, so
  nothing here relies on them.

What is used:

1. **The press:** a binding on `mouse:272` with `non_consuming` (the click still reaches the window) and `ignore_mods` (Super+drag
   too) arms a timer inside the compositor (`hl.timer`, 40 ms). The timer runs only while the left button is down; a plain click
   costs one or two ticks of Lua and no process.
2. **The drag:** on its first tick the timer notes the window in front and where it is. When that window (floating) has moved
   more than 6 pixels and the pointer is near the notch (within 180 px of the screen's centre, no lower than 30 px under the bar),
   it runs `yggdrasil-panelctl zones-drag` once.
3. **While the sheet is open:** the panel asks Hyprland where the pointer is through its socket (`j/cursorpos`, no process per
   question) every 30 ms, and lights the zone under it, both in the sheet and at its real size on the screen. Nothing polls when
   no window is being dragged to the notch.
4. **The drop:** a `release` binding on `mouse:272` (also `non_consuming`, `ignore_mods`) disarms the timer and, if the panel was
   told, runs `yggdrasil-panelctl zones-drop`. The panel looks once more at where the pointer is; on a zone it waits 60 ms (the
   compositor puts the window down where it was let go first), then floats it if it was tiled, sets its size, then its place (a
   resize keeps a window centred, so the other order would undo the move). Dropped anywhere else: nothing happens.
5. **A drop that never comes:** if the release binding is not heard (see "Not verified"), a drag sheet whose pointer has not
   moved for 8 seconds closes itself.

The keyboard path (Super+Shift+Z, `yggdrasil-panelctl zones`) opens the sheet for the window in front on its own screen; the
panel then gets ordinary pointer events, and a click on a zone places the window.

## Where things are

| Part | File |
|---|---|
| Layouts, `zones.conf`, the geometry (free area less the bar and dock strips, gaps, scale, rotation) | `packages/yggdrasil-conf/src/zones.rs` |
| The sheet, the pointer through Hyprland's socket, placing the window | `packages/panel/src/zones.rs` (hooks in `main.rs`: D-Bus `ZonesDrag`/`ZonesDrop`/`ZonesPick`, `PopupKind::Zones`) |
| The bindings and the timer | `packages/session/hyprland/yggdrasil.lua`, the "Window zones" block |
| `zones`, `zones-drag`, `zones-drop` | `packages/panel/bin/yggdrasil-panelctl` |
| Settings > Displays > Window zones | `packages/settings/src/zones_ui.rs` |

Built-in layouts: Halves, Thirds, Wide left (2/3 + 1/3), Wide right, Priority (1/4 + 1/2 + 1/4), Quarters, Grid (3 x 2), Rows,
Focus (one centred zone). The sheet shows Halves, Thirds, Wide left, Quarters and Focus until Settings changes that. Custom layouts
can be added later as more entries in `zones.conf`.

## Checked on 2026-10-25

- The Lua calls the block uses (`hl.timer` with `set_enabled`, `hl.get_cursor_pos`, `hl.get_monitor_at` and its `reserved.top`,
  `hl.get_active_window().floating` and `.at`), through a throwaway `hyprctl eval` that added no bindings.
- Placement: a test window sent the same three dispatches the panel sends landed at exactly the computed rectangle (left half of a
  3440x1440 screen with a 28 px bar, a 70 px dock and an 8 px gap: 8,36 at 1708x1326); `float enable` with a window address works.
- Hyprland's socket answers `j/cursorpos` with `{"x": .., "y": ..}`.
- Geometry tests: gaps at edges and between zones, the bar and dock strips, a second screen at scale 1.5, a rotated screen.

## Not verified

- A real drag: nobody dragged a window to the notch, so the press and release bindings on `mouse:272` with `non_consuming` and
  `ignore_mods` are untested in practice. The likely risk is the release binding not firing at the end of a compositor-driven move
  (an app's title-bar drag or Super+drag). If so, the 8-second guard closes the sheet, and the fix is to detect the drop in the
  timer instead (the window has stopped moving and `hl.is_key_down` reports the button up, if it covers mouse buttons).
- The session file is not installed or reloaded; it takes effect with the next yggdrasil-session package and a reload.
