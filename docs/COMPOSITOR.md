# labyrinth-compositor

Labyrinth's own Wayland compositor (Rust, on [Smithay](https://github.com/Smithay/smithay)): the piece that replaces KWin, so the screen
lock, the idle manager, the panel and the login screen all run on one compositor that Labyrinth controls. Source: `packages/compositor`.

## Status (2026-10-18): first version, runs nested in a window only

It opens a window on the running desktop (winit backend) and serves its own Wayland socket; programs started with `--run` use it:

    labyrinth-compositor --run /usr/bin/labyrinth-settings --run dbus-run-session -- /usr/bin/labyrinth-panel

It cannot touch the real session: it is just another window on KWin. Nothing about the real session changes until the hardware backend exists.

What works (each checked by hand in the nested window):
- **Windows** (xdg-shell): toplevels open cascaded, a click raises one and gives it the keyboard, closing hands the keyboard to the next; popups are placed.
- **Panels and backgrounds** (wlr-layer-shell): the whole Labyrinth desktop runs in it: the wallpaper, the top bar, Fylgja's notch and the dock from `labyrinth-panel` drew correctly beside a Settings window.
- **Screen lock** (ext-session-lock): `labyrinth-lock` locked, typed the test password and unlocked (exit 0). While locked only the locker's own picture is drawn and only it gets input.
- **Idle** (ext-idle-notify): `labyrinth-idle` was told when the user went idle, and its dimming overlay (layer-shell + viewporter) was accepted.
- **Moving a window** by its own title bar (the xdg `move` request): dragging Settings' title area moved the window; checked with a real pointer drag. **Resizing, maximize and full screen** (the matching requests, remembering and restoring the old size) are written but not yet tried by hand.
- **Window list for the panel** (`wlr-foreign-toplevel-management`, written here because Smithay does not ship it): the panel found it, the top bar showed the active window's title ("Settings") and the dock lit a dot under its icon. Activate, close, minimize, maximize and full screen from the panel are written (minimized windows are kept aside and brought back by activate) but not yet tried by hand.
- **Shortcuts handled by the compositor** (`shortcuts.rs`, replacing `kglobalacceld`): the Super key on its own (start menu, through the panel's D-Bus service), Super+E/I/L/V/S/C/F/Q, Alt+F4, Alt+Tab, Super+arrows (maximize, restore or minimize, snap left and right), Super+Return. `~/.config/labyrinth/shortcuts.conf` changes or removes any of them. Checked in the nested window with test bindings (the host desktop keeps the Super key and Alt+Tab to itself): Ctrl+Shift+Left snapped the focused window to the left half, and Ctrl+Shift+Space opened the panel's start menu. The Super tap, Alt+Tab and the rest are written but could not be tried there.
- **Title bars** drawn by the compositor for programs that ask for server-side decoration (`xdg-decoration`): a dark bar in Yggdrasil's look with the window's title and round minimize, maximize and close buttons; drag to move, double click to maximize, grips just outside the left, right and bottom edges to resize; maximized and snapped windows fit under their bar. Programs that never ask (Minos, Settings, the Marketplace and most modern apps) draw their own and get none. A program that asks for a bar of its own is still given ours, so the look is the same everywhere. `LABYRINTH_COMPOSITOR_DECORATE=all` gives every window a bar (for testing). Checked in the nested window: the bar, its title, the inactive look, and the close button closing a window.
- **Window switcher** (Alt+Tab): holding Alt shows a card for each window, most recently used first; Tab moves on, Shift+Tab goes back, Escape cancels, letting go of Alt picks. Checked with Ctrl+Tab in the nested window (the host keeps Alt+Tab).
- **Workspaces** (four): Super+Ctrl+1..4 and Super+Ctrl+Left/Right switch, Super+Ctrl+Shift+1..4 send the focused window to another one; the dock still lists every window and activating one on another workspace goes there. Checked: sending a window away, and showing only it on the other workspace. The panel's workspace buttons are still KWin's and do not follow these yet.
- **Multimedia keys** are bound to `labyrinth-mediakey` (volume, mute, microphone, the running player, brightness). They could not be tried in the nested window, because the host desktop takes those keys; held keys do not repeat yet.
- Pointer and keyboard (from the host window), wheel, wl_output with xdg-output, clipboard (data device), viewporter, shm.

Not built yet (in the order I would do them):
1. Trying resize, maximize, full screen and minimize by hand.
2. The panel's workspace buttons following the compositor's workspaces (the protocol `ext-workspace`), and key repeat for the volume keys.
4. Tiling, and more workspaces than four.
5. GPU clients (linux-dmabuf), idle inhibit (videos), primary selection, cursor shapes, fractional scale, xdg-activation, xdg-decoration.
6. **The hardware backend**: DRM/KMS output, libinput, a seat (logind or seatd), several monitors, scaling, VRR, NVIDIA. This is what lets it replace KWin for real; it should be developed in a virtual machine (Limbo), never on the live session.
7. Screen recording and screenshots (the portal), output management for Settings > Displays, an X11 layer (XWayland) for Steam and games.

Why not before: the roadmap puts the compositor last, once the pieces around it are stable; they now are (panel, lock, idle, greeter,
portal, polkit), and each of them can be developed and tested on this compositor in a window.

Debugging: `LABYRINTH_COMPOSITOR_DEBUG=1` prints pointer positions, button presses and move requests. To drive it from a script, move the host pointer
with `ydotool` in small steps (the host accelerates movement by about 1.7x) and read the positions it prints.
