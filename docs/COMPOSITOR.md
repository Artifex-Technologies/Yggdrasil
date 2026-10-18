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
- Pointer and keyboard (from the host window), wheel, wl_output with xdg-output, clipboard (data device), viewporter, shm.

Not built yet (in the order I would do them):
1. Window moving and resizing from the app's own title bar (xdg `move`/`resize` requests), maximize, fullscreen, minimize, window titles and app ids.
2. Window list for the dock and the notch (`wlr-foreign-toplevel-management`, which Smithay does not ship: it has to be written; the panel already uses it).
3. Shortcuts handled by the compositor (the Super key, Alt+Tab, workspaces) instead of KDE's `kglobalacceld`; tiling.
4. GPU clients (linux-dmabuf), idle inhibit (videos), primary selection, cursor shapes, fractional scale, xdg-activation, xdg-decoration.
5. **The hardware backend**: DRM/KMS output, libinput, a seat (logind or seatd), several monitors, scaling, VRR, NVIDIA. This is what lets it replace KWin for real; it should be developed in a virtual machine (Limbo), never on the live session.
6. Screen recording and screenshots (the portal), output management for Settings > Displays, an X11 layer (XWayland) for Steam and games.

Why not before: the roadmap puts the compositor last, once the pieces around it are stable; they now are (panel, lock, idle, greeter,
portal, polkit), and each of them can be developed and tested on this compositor in a window.
