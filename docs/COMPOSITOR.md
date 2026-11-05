# yggdrasil-compositor

Yggdrasil's own Wayland compositor (Rust, on [Smithay](https://github.com/Smithay/smithay)): the piece that would let the screen
lock, the idle manager, the panel and the login screen all run on one compositor that Yggdrasil controls. Source: `packages/compositor`.

**Today (2026-11-04):** Yggdrasil's desktop runs as its own session on **Hyprland 0.57** (`packages/session`, `yggdrasil-session`,
configured in `packages/session/hyprland/yggdrasil.lua`). Hyprland is the real compositor; this one is still in progress and runs
only nested in a window (below). KDE, and with it KWin, is gone (2026-10-24).

## Status (2026-10-18): first version, runs nested in a window only

It opens a window on the running desktop (winit backend) and serves its own Wayland socket; programs started with `--run` use it:

    yggdrasil-compositor --run /usr/bin/yggdrasil-settings --run dbus-run-session -- /usr/bin/yggdrasil-panel

It cannot touch the real session: it is just another window on the running desktop (that was KWin when the checks below were made; today it is Hyprland, where it has not been tried again). Nothing about the real session changes until the hardware backend exists.

What works (each checked by hand in the nested window):
- **Windows** (xdg-shell): toplevels open cascaded, a click raises one and gives it the keyboard, closing hands the keyboard to the next; popups are placed.
- **Panels and backgrounds** (wlr-layer-shell): the whole Labyrinth desktop runs in it: the wallpaper, the top bar, Fylgja's notch and the dock from `yggdrasil-panel` drew correctly beside a Settings window.
- **Screen lock** (ext-session-lock): `yggdrasil-lock` locked, typed the test password and unlocked (exit 0). While locked only the locker's own picture is drawn and only it gets input.
- **Idle** (ext-idle-notify): `yggdrasil-idle` was told when the user went idle, and its dimming overlay (layer-shell + viewporter) was accepted.
- **Moving a window** by its own title bar (the xdg `move` request): dragging Settings' title area moved the window; checked with a real pointer drag. **Resizing, maximize and full screen** (the matching requests, remembering and restoring the old size) are written but not yet tried by hand.
- **Window list for the panel** (`wlr-foreign-toplevel-management`, written here because Smithay does not ship it): the panel found it, the top bar showed the active window's title ("Settings") and the dock lit a dot under its icon. Activate, close, minimize, maximize and full screen from the panel are written (minimized windows are kept aside and brought back by activate) but not yet tried by hand.
- **Shortcuts handled by the compositor** (`shortcuts.rs`; when this was written they were KDE's `kglobalacceld`, today Hyprland binds the keys): the Super key on its own (start menu, through the panel's D-Bus service), Super+E/I/L/V/S/C/F/Q, Alt+F4, Alt+Tab, Super+arrows (maximize, restore or minimize, snap left and right), Super+Return. `~/.config/labyrinth/shortcuts.conf` changes or removes any of them. Checked in the nested window with test bindings (the host desktop keeps the Super key and Alt+Tab to itself): Ctrl+Shift+Left snapped the focused window to the left half, and Ctrl+Shift+Space opened the panel's start menu. The Super tap, Alt+Tab and the rest are written but could not be tried there.
- **Window controls** drawn by the compositor for programs that ask for server-side decoration (`xdg-decoration`, mostly Qt programs): no title bar, only a grip and the round minimize, maximize and close buttons in the top right corner of the window with no background of their own (they take on the look of the program under them), level with the program's own menu or tool bar; the program keeps the whole height of its window. Drag by the grip (double click maximizes); or hold Super and drag anywhere with the left button to move, the right button to resize; grips just outside the edges resize too. A program that asks to draw its own controls (Minos, Settings, the Marketplace) gets none. `YGGDRASIL_COMPOSITOR_DECORATE=all` gives every window the pill (for testing). Checked in the nested window with Wireshark: the pill sat beside its menu row. Moving by the grip, Super+drag and the edge grips are written but were not tried by hand.
- **Window switcher** (Alt+Tab): holding Alt shows a card for each window, most recently used first; Tab moves on, Shift+Tab goes back, Escape cancels, letting go of Alt picks. Checked with Ctrl+Tab in the nested window (the host keeps Alt+Tab).
- **Workspaces** (four): Super+Ctrl+1..4 and Super+Ctrl+Left/Right switch, Super+Ctrl+Shift+1..4 send the focused window to another one; the dock still lists every window and activating one on another workspace goes there. Checked: sending a window away, and showing only it on the other workspace. The panel's workspace buttons follow Hyprland's workspaces (through `hyprctl`), not these yet.
- **Multimedia keys** are bound to `yggdrasil-mediakey` (in the panel package) (volume, mute, microphone, the running player, brightness). They could not be tried in the nested window, because the host desktop takes those keys; held keys do not repeat yet.
- **Workspace profiles** (`tiling.rs`): each workspace is **standard** (windows float) or **tiling** (Hyprland-style "dwindle": the first window takes the left half, the next the top right, and so on, with gaps). `~/.config/labyrinth/workspaces.conf` sets a default (standard, tiling, or **ask** each time an empty workspace is opened) and an exception per workspace; Settings > Appearance > Workspaces writes it and the compositor reads it again when it changes. Super+T floats or tiles one window. Checked in the nested window: with "ask" the question appeared on the empty workspace, Right and Enter chose Tiling, and the three windows that opened afterwards were laid out side by side.
- Pointer and keyboard (from the host window), wheel, wl_output with xdg-output, clipboard (data device), viewporter, shm.

**X11 programs (XWayland), built 2026-11-04, not yet tried by hand** (`src/xwayland.rs`; for Steam's client, games through Proton
or Wine, and older programs). XWayland starts with the compositor and its display is printed as `DISPLAY=:n`; programs given with
`--run` wait the fraction of a second until it has answered, then get that DISPLAY (never the host's), and so do programs started by
shortcuts. `YGGDRASIL_COMPOSITOR_XWAYLAND=0` leaves it out. What is written:
- An X11 window is placed, tiled, listed for the panel (dock, top bar), shown in Alt+Tab and given the keyboard like a Wayland
  window; the keyboard also sets the X input focus, without which XWayland passes keys to none of its windows. It gets the window
  controls unless it says it draws its own (Motif hints: Steam and most games do). Activate, minimize, maximize, full screen,
  snap, close, the window controls, Super+drag and the program's own move and resize requests use the same code as for Wayland
  windows. Hiding or destroying it is the same clean-up as a closed Wayland window.
- Menus and tooltips (override-redirect windows), and menus, tooltips, notifications and splash screens that go through the window
  manager, are drawn where they ask, on top, never get the keyboard and are not listed; a click on one goes to it.
- A window the compositor placed (tiled, maximized, full screen, snapped) keeps its place when it asks for another; others may change
  their size but not move themselves.
- **What a window asked for before it was shown** (2026-11-05, `src/x11asked.rs`): a game that wants to start full screen, or a
  program that wants to start maximized, sets `_NET_WM_STATE` before its window is shown. Smithay's window manager does not read
  it, so a thread of its own reads it over its own connection to XWayland (the compositor never waits for the X server) and the
  window goes full screen or maximized a moment after it shows. A game (below) as big as the screen that said nothing is still
  taken as going full screen; other programs no longer are.
- Clipboard and middle-click selection both ways between X11 and Wayland programs; an X11 program can read them only while one of
  its windows has the keyboard. The program with the keyboard is now the one offered the clipboard (this was missing for Wayland
  programs too: Smithay offers it only to the focused program).
- Full screen X11 windows are drawn opaque (a shader that ignores the alpha channel), so a game that leaves junk in it no longer
  shows the desktop through in patches. X11 windows without an alpha visual were already opaque (XWayland hands them over as XRGB).
- **Games are drawn opaque windowed too** (2026-11-05): a Steam game (X11 class `steam_app_<id>`) or a Windows program through Wine
  or Proton (`<name>.exe`), whatever its alpha channel holds. Anything else that needs it goes in `~/.config/yggdrasil/opaque.conf`,
  one app id (X11 class) per line, read again when it changes.
- **Dock icons for X11 programs** (in the panel, so the Hyprland session has them too): a window is matched to its desktop entry by
  its class (`StartupWMClass`), and a Steam game's window (`steam_app_<id>`) to the shortcut Steam made (`steam://rungameid/<id>`);
  a game without a shortcut gets the icon Steam saved (`steam_icon_<id>`).

Still missing for X11: the top panels are drawn over full screen windows, Wayland and X11 alike (fine for a desktop, wrong for a
game); XWayland always runs (about 20 to 40 MB) instead of starting on the first X11 program; no HiDPI scaling for X11 programs;
drag and drop between X11 and Wayland programs; X11 window icons for programs with no desktop entry (`_NET_WM_ICON`); and the X11
cursor theme and XSETTINGS.

To try it: `yggdrasil-compositor --run xterm` (typing, its menus on Ctrl+click, moving, the window controls), then Steam
(`--run steam`: its menus, the clipboard both ways with a Wayland program, minimize and restore from the dock), then a Proton
game full screen (no see-through patches, keys and mouse reach it). `YGGDRASIL_COMPOSITOR_DEBUG=1` also shows XWayland's own messages.

Not built yet (in the order I would do them):
1. Trying resize, maximize, full screen and minimize by hand, and X11 programs (above): xterm, Steam, a Proton game.
2. The panel's workspace buttons following the compositor's workspaces (the protocol `ext-workspace`), and key repeat for the volume keys.
4. More workspaces than four.
5. Built 2026-11-04, not yet tried by hand: a full screen window in front covers the top bar and dock (they go behind it and get
   no pointer; the Overlay layer stays in front). Also built 2026-11-04, not yet tried by hand: GPU clients (linux-dmabuf, with default feedback naming the render node), idle inhibit,
   primary selection, cursor shapes (the nested window's pointer follows them), fractional scale (1.0 until outputs get a scale),
   xdg-activation (only with a token from the person's own input in the last 10 s).
6. **The hardware backend**: DRM/KMS output, libinput, a seat (logind or seatd), several monitors, scaling, VRR, NVIDIA, explicit sync (linux-drm-syncobj-v1, which NVIDIA needs to avoid flicker and frames
   out of order), direct scanout for full screen games, and tearing control. This is what would let it run the real session instead of Hyprland; it should be developed in a virtual machine (Limbo), never on the live session.
7. Screen recording and screenshots (the portal), output management for Settings > Displays.

Why not before: the roadmap puts the compositor last, once the pieces around it are stable; they now are (panel, lock, idle, greeter,
portal, polkit), and each of them can be developed and tested on this compositor in a window.

Debugging: `YGGDRASIL_COMPOSITOR_DEBUG=1` prints pointer positions, button presses and move requests. To drive it from a script, move the host pointer
with `ydotool` in small steps (the host accelerates movement by about 1.7x) and read the positions it prints.

## Lightness (a rule for all of Yggdrasil)
Yggdrasil must stay as light and quick as the features allow, on any machine, including old and small ones. For the compositor that means: nothing is drawn while
nothing changes (a frame is drawn only after a program committed a picture, input arrived or a setting changed, and a frame with no change is not even sent on);
no polling beyond a 2-second look at two small config files; pictures such as the window controls and the switcher are drawn once and kept until what they show changes.
Measured 2026-10-18 on this computer, nested: the compositor used about 2 % of a core while idle because it redrew 60 times a second; after the change it uses 0 %.
Its memory (about 117 MB) is mostly the graphics driver's context; the panel is about 89 MB, the idle manager 3 MB, the polkit agent 8 MB. Still to measure and trim: the panel's memory,
start-up time of each program, and the compositor's cost with several large windows.


## The bake-off (set up 2026-10-23)

Decision rule (owner, 2026-10-23): the compositor is chosen by measurement on this machine's real games and a CAD model, and the
shell stays on standard protocols whichever wins. `tools/bakeoff/yggdrasil-bakeoff` adds one login entry per candidate, each
starting the compositor with only Steam in it. Round 1 also had KWin alone and the full Plasma desktop as the baseline; Hyprland
won, KDE is gone, and those two entries were removed from the tool (2026-10-26). Their results stay in `~/bakeoff` for the report.
The table is round 1 as it was set up:

| Entry | Compositor | Notes |
| --- | --- | --- |
| Bake-off: KWin alone | `kwin_wayland --xwayland --exit-with-session=steam` | KDE's compositor without Plasma; removed with KDE |
| Bake-off: Hyprland | Hyprland (0.56 then), a generated Lua config | games may tear (`immediate` rule), VRR per `yggdrasil-bakeoff vrr` |
| Bake-off: COSMIC compositor | cosmic-comp 1.0 (Smithay, Rust) | Steam is started into it once its Wayland and X sockets exist |
| Bake-off: gamescope | Valve's, `gamescope -e -f -- steam -gamepadui` | the Steam Deck way; a candidate for a game session, not for the desktop |
| Bake-off: Plasma | the full KDE desktop | baseline; removed with KDE |

MangoHud measures every Vulkan game started there (Proton games are): 20 s after the game starts, 90 s of frame times, no overlay,
into `~/bakeoff/NAME-vrrX/`. `yggdrasil-bakeoff report` compares average FPS, 1% and 0.1% lows, frame-time spread and stutters.
Round 1 is with variable refresh off everywhere; round 2 (`yggdrasil-bakeoff vrr on`) with it on. Still to add: input latency (needs
a camera or a photodiode, or at least the owner's judgement), and the CAD model in Daedalus. Yggdrasil's own compositor joins once it
runs on the hardware (today it runs nested only).

## Title bars in the Hyprland session (2026-10-24)

Hyprland offers every program server-side decoration (`xdg-decoration`) but draws no title bar. Programs that draw their own
anyway are unaffected: Yggdrasil's apps (`with_decorations(false)`), GTK programs, Ghostty, Firefox, Chromium and Electron.
Qt programs accept the offer and would have no title bar, so `yggdrasil-session` sets
`QT_WAYLAND_DISABLED_INTERFACES=zxdg_decoration_manager_v1`: Qt does not see the offer and draws its own title bar with minimize,
maximize and close. Checked in a nested Hyprland with Qt Linguist (title bar present) and VLC before the change (none).
