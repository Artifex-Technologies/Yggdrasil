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
4. **The drop.** Hyprland's `release` binding on `mouse:272` does **not** fire at the end of a window move (the owner's test,
   2026-10-25: the sheet opened, letting go over a zone did nothing). So the end of the drag is read from three signs; whichever
   comes first places the window, once (`DropWatch` in `panel/src/zones.rs`; later signs are ignored):
   - **The window stops following the pointer** (the panel, every 30 ms while the sheet is open, and the session's Lua timer, every
     40 ms): while the button is held the window moves whenever the pointer does (a frame behind at most, which on a fast drag at
     180 Hz is more than a few pixels, so the window's offset from the pointer is no test). Let go and the window stays put while
     the pointer moves on: the pointer moving more than 6 px since the window last moved is the drop. The window goes to the zone
     it was **held** over last, not where the pointer went after.
   - **The pointer comes back to the sheet**: once the compositor lets go of the pointer, it sends the sheet's surface an enter
     (taken only 250 ms after the sheet opened: before that it can be the sheet appearing under the pointer), motion, or a click.
   - **The button state**, if `hl.is_key_down(272)` reports mouse buttons (it answers, false when up; whether it tracks a held
     mouse button is not known): seen down, then up, is the drop. The release binding stays too, for versions where it fires.
   The Lua timer also stops when a click moves no window (the pointer moves on, nothing follows), so it never keeps running.
5. **Snap Assist.** After a window goes into a zone (dropped, or picked with Super+Shift+Z), if the layout has other zones and
   other windows are open on that screen's desktop (not hidden, not full screen, not on a special workspace), a full-screen overlay
   offers them: the first empty zone shows them as tiles (app icon and title, most recently used first, scroll for more); a click
   or Enter (arrows and Tab move) puts that window there and the next empty zone takes its turn, until every zone is filled or no
   window is left. A click anywhere else, or Escape, stops; what was placed stays. One `hyprctl clients` when it opens
   (`panel/src/assist.rs`).
6. **A drop that never comes:** if no sign is seen, a drag sheet whose pointer has not moved for 8 seconds closes itself.

The keyboard path (Super+Shift+Z, `yggdrasil-panelctl zones`) opens the sheet for the window in front on its own screen; the
panel then gets ordinary pointer events, and a click on a zone places the window.

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

## Not verified

- The owner's first real drag (2026-10-25) confirmed the sheet opens and that the release binding does not fire; the drop signs
  above replaced it and are tested in code (`DropWatch`), but not yet by a real drag.
- Checked on the live session: Snap Assist's window list from `hyprctl -j clients` and placing a window into a zone through it (a
  foot window of the test's own, `cargo test -- --ignored live` in packages/panel: it landed exactly on the middle third).
- The session file is not installed or reloaded; it takes effect with the next yggdrasil-session package and a reload.
