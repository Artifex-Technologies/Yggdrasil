# Window zones

PowerToys' FancyZones, rewritten for Yggdrasil in Rust: hold Super and drag a window to the notch (Fylgja) at the top of a screen,
and a sheet of layouts opens under it; drop the window on a zone and it fills that part of the screen. Super+Shift+Z opens the
same sheet for the window in front (then click a zone; Esc cancels). Settings > Displays > Window zones turns it on or off, sets
the space around windows and picks which layouts the sheet shows (`~/.config/yggdrasil/zones.conf`).

## How a drag is noticed

Only a **Super+drag** opens the sheet. I learned why on my machine on 2026-10-25, on Hyprland 0.56:

- **No drag or mouse-button event.** The Lua API's events cover windows, workspaces, monitors and keys; none fires when a window
  starts or stops moving, and `hl.is_key_down(272)` answers false while the left button is held (it tracks keyboard keys only).
- **No binding on the mouse button.** A binding on `mouse:272` (press or release, even `non_consuming`, with or without
  `binds.pass_mouse_when_bound`) kept the release from ending Hyprland's window move: the window went on following the pointer
  after the drop until the next click.
- **Title-bar drags cannot be zoned.** Dragging by the title bar is the app's own move (xdg_toplevel.move). Once the window is
  placed away from the pointer the app never hears the button come up, and picks the window up again as soon as the pointer moves
  (Vivaldi, VS Code, Minos, Agora, Obsidian and Ghostty all did). Super+drag is Hyprland's own move and ends cleanly, so it is the
  one the sheet answers to.
- **The panel cannot see the pointer during a drag** (the compositor holds it), so it asks the compositor's socket (through
  yggdrasil-wm, `packages/wm`: Hyprland's socket, or the control socket of Yggdrasil's own compositor; the panel's side is the
  same on both).

So the panel does it all (`packages/panel/src/zones.rs`):

1. **The button** (`ButtonEvents`): while window zones are on (Settings, `zones.conf`: on, with a layout picked), the panel waits
   for the left button's presses and releases as events from the kernel, and for nothing else. With zones off it holds no mouse
   open, so a press, a held button or a release wakes nothing; turning them off closes the mice (and puts away a sheet that is
   open), turning them on opens them (the settings folder's watch sees `zones.conf` change). It opens each `event*` device in
   `/dev/input` that it can open and that has a left button and no A key: the mice and touchpads the person at the screen may
   read (the session package's udev rule `71-yggdrasil-pointer.rules` grants that, never for keyboards; a mouse's macro keys
   show up as a keyboard device and are left out too). Two `EVIOCSMASK`
   calls tell each device to report the left button alone: one bitmap of the event types holding `EV_KEY` alone, one of the keys
   and buttons holding `BTN_LEFT` alone. Both bitmaps are made of whole longs (`c_ulong`), as the kernel reads them: it refuses
   any other size (`EINVAL`), and a mask of the event types in 4 bytes, refused that way, let every motion and wheel turn wake the
   panel. The kernel delivers no empty report, so moving the mouse or turning the wheel wakes nothing. All the devices sit in one
   epoll that the panel's event loop waits on: a press or release wakes it, and what came is read. The watch asks for the button's
   state itself (`EVIOCGKEY`) only when it opens a device and when the kernel says it dropped events (`SYN_DROPPED`). When a mouse
   is plugged in or out or becomes readable (an inotify watch on `/dev/input`), the devices are opened again while zones are on.
   `YGGDRASIL_PANEL_INPUT` names another folder in place of `/dev/input`: the tests give one of their own, empty, or with a FIFO
   named `event0` that stands in for a mouse (the panel takes a FIFO there as a mouse; the kernel's folder holds devices only).
2. **The drag** (`Spotter`): while the left button is held and no popup of the panel is open, a 60 ms timer runs; it stops when
   the button comes up. Each tick asks the compositor for the window in front, and the first tick of a press remembers it and
   where it was. Once that window, floating, has moved more than 6 px, each tick also asks for the pointer and the screens. When
   the pointer is at the notch (within 180 px of the middle of its screen, no lower than 30 px under the bar), the panel asks once
   per drag whether Super is held (on Hyprland `hl.is_key_down('Super_L'/'Super_R')` through an `eval` on its socket, on
   Yggdrasil's own compositor its control socket's `modifiers`). If so, the sheet opens (unless window zones are off, no layout is
   picked, or the window is full screen).
3. **The drop** (`DropWatch`): while the sheet is open, a 30 ms timer asks the compositor where the pointer is (the zone under it
   lights up) and where the window is. The drop is the left button's release: its event wakes the panel (the watch of point 1;
   nothing asks the button in between) and the window goes to the zone under the pointer. A move that ends with no release the
   panel hears ends the drag too, once the window no longer follows the pointer, which moved on more than 6 px while the window
   stayed (the window goes to the zone it was held over last). Whichever comes first ends it, once. A drop off the zones only
   closes the sheet. The sheet takes no pointer input during a drag, so the pointer lands on the dragged app when the button comes
   up. The window is placed 60 ms after the drop, and Snap Assist opens 350 ms after it.
4. **A drop that never comes:** a drag sheet whose pointer has not moved for 8 seconds closes itself.

`~/.local/state/yggdrasil/zones.log` (under `$XDG_STATE_HOME` when that is set) keeps a short trail of what was seen, for when a
drop goes wrong: a drag at the notch and Super's answer, the sheet, how the drop was seen, and each request that placed the window
with the compositor's answer. It is emptied once it passes 200 KB.

The keyboard path (Super+Shift+Z, `yggdrasil-panelctl zones`) opens the sheet for the window in front on its own screen (the same
keys again close it); a click on a zone places the window at once, and Snap Assist follows.

## Where things are

| Part | File |
| --- | --- |
| Layouts, `zones.conf`, the geometry (free area less the bar and dock strips, gaps, scale, rotation) | `packages/yggdrasil-conf/src/zones.rs` |
| The left button, the drag, the sheet, the drop, placing the window | `packages/panel/src/zones.rs` (hooks in `main.rs`: the buttons' epoll and the `/dev/input` watch in the event loop, `zones.conf` in the settings folder's watch, the `Tick::Spot` and `Tick::Follow` timers, D-Bus `ZonesPick`, `PopupKind::Zones`) |
| Asking the compositor (the window in front, the pointer, Super held, the screens, placing) | `packages/wm` (yggdrasil-wm: Hyprland or Yggdrasil's own compositor) |
| Snap Assist (the other zones offered to the other windows) | `packages/panel/src/assist.rs` (`PopupKind::Assist` in `main.rs`) |
| Read access to the mice and touchpads | `packages/session/udev/71-yggdrasil-pointer.rules` |
| The Super+Shift+Z binding | `packages/session/hyprland/yggdrasil.lua`, the "Window zones" block (Hyprland); `packages/compositor/src/shortcuts.rs` (Yggdrasil's own compositor) |
| `zones` | `packages/panel/src/bin/yggdrasil-panelctl.rs` |
| Settings > Displays > Window zones | `packages/settings/src/zones_ui.rs` |

Built-in layouts: Halves, Thirds, Wide left (2/3 + 1/3), Wide right, Priority (1/4 + 1/2 + 1/4), Quarters, Grid (3 x 2), Rows,
Focus (one centred zone). The sheet shows Halves, Thirds, Wide left, Quarters and Focus until Settings changes that. `zones.conf`
names them by id (`halves`, `thirds`, `two-thirds-left`, `two-thirds-right`, `priority`, `quarters`, `grid`, `rows`, `focus`)
and drops an id it does not know.

## Checked

- 2026-10-25, on Hyprland: a test window sent the same three dispatches the panel sends landed at exactly the computed rectangle
  (left half of a 3440x1440 screen with a 28 px bar, a 70 px dock and an 8 px gap: 8,36 at 1708x1326); `float enable` with a
  window address works. Hyprland's socket answers `j/cursorpos` with `{"x": .., "y": ..}`.
- 2026-11-24, on a virtual mouse: a mask of the event types in 4 bytes is refused, and `EVIOCGMASK` shows motion (`EV_REL`) and
  `EV_MSC` still passing; in 8 bytes it is taken and only `EV_KEY` passes.
- 2026-11-25, on Yggdrasil's own compositor, headless: with window zones off, a held button woke the panel 49 times a second (its
  60 ms look for a drag) and now not once; during a drag the sheet no longer opens the mouse a second time to ask the button 33
  times a second, and the drop is seen at the release (numbers in [COMPOSITOR.md](COMPOSITOR.md), "Lightness").
- Tests: the geometry (`packages/yggdrasil-conf`: gaps at edges and between zones, the bar and dock strips, a second screen at
  scale 1.5, a rotated screen); the button (`packages/panel`, src/zones.rs: a pipe stands in for a mouse, a left-button press
  wakes the spotter, another button changes nothing, the release ends its ticking and forgets the drag; in a folder of the test's
  own, a FIFO standing in for a mouse is opened while zones are on, closed when they go off and not opened when they are off from
  the start, and a press then wakes nothing; the masks are whole longs holding `EV_KEY` and `BTN_LEFT` alone; the drop seen once,
  when the window stops following the pointer); on Yggdrasil's own compositor, headless (`packages/compositor`, tests/apps.rs):
  the keyboard path (the sheet opens on the window's screen, and a click on a zone puts a terminal exactly at its rectangle) and
  the drag, with a FIFO as the panel's mouse (a Super+drag to the notch opens the sheet, the mouse held open once; the button's
  release puts the terminal exactly in the left zone; zones turned off in `zones.conf` close the mouse, and a held button wakes
  the panel no more than an idle one; on again, the mouse is opened again).

## Verified on my machine (2026-10-25)

- Super+drag to the notch and a drop on a zone: the window is placed and stays (title-bar drags, before they were left out, were
  placed but then followed the pointer again).
- Snap Assist's window list and placing a window through it (a foot window of the test's own, `cargo test -- --ignored live`).
- Hyprland's `hl.is_key_down` reports a held Super (zones.log): a drag answered true opened the sheet, the drop was seen as the
  left button coming up and the window went to its zone; a drag answered false opened nothing.
