# Window zones

PowerToys' FancyZones, rewritten for Yggdrasil in Rust: drag a window to the notch (Fylgja) at the top of a screen and a sheet of
layouts opens under it; drop the window on a zone and it fills that part of the screen. Super+Shift+Z opens the same sheet for the
window in front (then click a zone; Esc cancels). Settings > Displays > Window zones turns it on or off, sets the space around
windows and picks which layouts the sheet shows (`~/.config/yggdrasil/zones.conf`).

## How a drag is noticed (Hyprland 0.56)

Only a **Super+drag** opens the sheet. Learned on the owner's machine on 2026-10-25:

- **No drag or mouse-button event.** The Lua API's events cover windows, workspaces, monitors and keys; none fires when a window
  starts or stops moving, and `hl.is_key_down(272)` answers false while the left button is held (it tracks keyboard keys only).
- **No binding on the mouse button.** A binding on `mouse:272` (press or release, even `non_consuming`, with or without
  `binds.pass_mouse_when_bound`) kept the release from ending Hyprland's window move: the window went on following the pointer
  after the drop until the next click.
- **Title-bar drags cannot be zoned.** Dragging by the title bar is the app's own move (xdg_toplevel.move). Once the window is
  placed away from the pointer the app never hears the button come up, and picks the window up again as soon as the pointer moves
  (Vivaldi, VS Code, Minos, Agora, Obsidian and Ghostty all did). Super+drag is Hyprland's own move and ends cleanly, so it is the
  one the sheet answers to.
- **The panel cannot see the pointer during a drag** (the compositor holds it), so it asks Hyprland's socket.

So the panel does it all (`zones::Spotter`, `zones::Buttons`):

1. **The button:** each tick (60 ms) it asks the kernel whether the left button is held (`EVIOCGKEY` on the mice and touchpads the
   person at the screen may read; the session package's udev rule `71-yggdrasil-pointer.rules` grants that, never keyboards). One
   ioctl per mouse: nothing else runs while no button is held.
2. **The drag:** while the button is held it asks Hyprland for the window in front and the pointer. Once that window (floating) has
   moved more than 6 px and the pointer is within 180 px of the middle of its screen and 30 px under the bar, it asks once whether
   Super is held (`hl.is_key_down('Super_L'/'Super_R')` through `hyprctl eval`); if so, the sheet opens.
3. **The drop:** the button coming up (the kernel again) places the window on the zone under the pointer. The sheet takes no pointer
   input during a drag, and Snap Assist opens 350 ms after the drop.
4. **A drop that never comes:** a drag sheet whose pointer has not moved for 8 seconds closes itself.

`~/.local/state/yggdrasil/zones.log` keeps a short trail of what was seen (cut at 200 KB), for when a drop goes wrong.

The keyboard path (Super+Shift+Z, `yggdrasil-panelctl zones`) opens the sheet for the window in front on its own screen; a click on
a zone places it.

## Where things are

| Part | File |
|---|---|
| Layouts, `zones.conf`, the geometry (free area less the bar and dock strips, gaps, scale, rotation) | `packages/yggdrasil-conf/src/zones.rs` |
| The sheet, the pointer through Hyprland's socket, placing the window | `packages/panel/src/zones.rs` (hooks in `main.rs`: D-Bus `ZonesDrag`/`ZonesDrop`/`ZonesPick`, `PopupKind::Zones`) |
| Snap Assist (the other zones offered to the other windows) | `packages/panel/src/assist.rs` (`PopupKind::Assist` in `main.rs`) |
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

## Verified on the owner's machine (2026-10-25)

- Super+drag to the notch and a drop on a zone: the window is placed and stays (title-bar drags, before they were left out, were
  placed but then followed the pointer again).
- Snap Assist's window list and placing a window through it (a foot window of the test's own, `cargo test -- --ignored live`).
- Not yet seen: whether `hl.is_key_down('Super_L')` reports a held Super (the panel logs what it answered in zones.log).
