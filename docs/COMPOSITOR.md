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
- Pointer and keyboard (from the host window), wheel, wl_output with xdg-output, clipboard (data device), viewporter, shm.

Not built yet (in the order I would do them):
1. Trying resize, maximize, full screen and minimize by hand.
2. **Title bars drawn by the compositor** (`xdg-decoration`, in Labyrinth's own look, like Minos and Settings) for programs that do not draw their own, instead of a plain black bar. It needs a window element that carries its bar so overlapping windows stack correctly, and text drawn into a texture. Programs that draw their own (Minos, Settings, the Marketplace, most modern apps) keep theirs.
4. Shortcuts handled by the compositor (the Super key, Alt+Tab, workspaces) instead of KDE's `kglobalacceld`; tiling.
5. GPU clients (linux-dmabuf), idle inhibit (videos), primary selection, cursor shapes, fractional scale, xdg-activation, xdg-decoration.
6. **The hardware backend**: DRM/KMS output, libinput, a seat (logind or seatd), several monitors, scaling, VRR, NVIDIA. This is what lets it replace KWin for real; it should be developed in a virtual machine (Limbo), never on the live session.
7. Screen recording and screenshots (the portal), output management for Settings > Displays, an X11 layer (XWayland) for Steam and games.

Why not before: the roadmap puts the compositor last, once the pieces around it are stable; they now are (panel, lock, idle, greeter,
portal, polkit), and each of them can be developed and tested on this compositor in a window.

Debugging: `LABYRINTH_COMPOSITOR_DEBUG=1` prints pointer positions, button presses and move requests. To drive it from a script, move the host pointer
with `ydotool` in small steps (the host accelerates movement by about 1.7x) and read the positions it prints.
