# Keyboard shortcuts

The desktop's own (Win is the Super key) as `packages/session/hyprland/yggdrasil.lua` sets them, then each app's. The
desktop's part is made from that file (`YGGDRASIL_WRITE_DOCS=1 cargo test -p yggdrasil-conf` in `packages/yggdrasil-conf` writes it
again; a test fails while it is out of date); the apps' part, from "In the apps" on, is written by hand. Settings > Peripherals >
Keyboard > Shortcuts and the cheat sheet (Win + /) show the live list, your own additions included.

## Windows

| Keys | What it does |
| --- | --- |
| Win + Shift + Z | Window zones for the window in front |
| Alt + F4 | Close the window (programs that keep running: hide it) |
| Win + Q | Close the window (programs that keep running: hide it) |
| Win + F | Full screen |
| Win + ↑ | Maximize (again: restore), or focus up on a tiling screen |
| Win + ↓ | Minimize, or focus down on a tiling screen |
| Win + W | Close the window (as in Omarchy) |
| Win + Shift + M | Show minimized windows |
| Win + ← | Left half, or focus left on a tiling screen |
| Win + → | Right half, or focus right on a tiling screen |
| Win + T | Tile or float the window |
| Alt + Tab | Next window |
| Alt + Shift + Tab | Previous window |

## Desktops

| Keys | What it does |
| --- | --- |
| Win + Ctrl + ← | Previous desktop |
| Win + Ctrl + → | Next desktop |
| Win + Ctrl + 1-9 | Desktop 1-9 |
| Win + Ctrl + Shift + 1-9 | Move the window to desktop 1-9 |

## Apps

| Keys | What it does |
| --- | --- |
| Win + S | Talk to the assistant (tap to talk hands-free, tap again to end) |
| Ctrl + Alt + S | Talk to the assistant (tap to talk hands-free, tap again to end) |
| Win + V | Clipboard history and snippets |
| Win + E | Files (Minos) |
| Win + Return | Terminal |
| Ctrl + Shift + Esc | Task Manager (again: brings it to the front) |
| Win + Shift + C | Color picker |
| Win + Alt + − | Magnifier: zoom out |
| Win + Alt + 0 | Magnifier off |
| Win + Alt + S | Screen reader (Orca) on or off |

## System

| Keys | What it does |
| --- | --- |
| Win | Start menu |
| Win + C | Calendar |
| Win + N | Notifications |
| Win + I | Settings |
| Win + L | Lock the screen |
| Win + / | Keyboard shortcuts |

## Mouse

| Keys | What it does |
| --- | --- |
| Win + Left drag | Move the window |
| Win + Right drag | Resize the window |
| Win + Alt + = | Magnifier: zoom in around the pointer |
| Ctrl twice | Find my mouse |

## Screenshots

| Keys | What it does |
| --- | --- |
| Print Screen | Screenshot (area, window or screen) |
| Win + Shift + S | Screenshot (area, window or screen) |

## Media keys

| Keys | What it does |
| --- | --- |
| Volume up | Volume up |
| Volume down | Volume down |
| Mute | Mute |
| Microphone mute | Microphone mute |
| Play | Play |
| Pause | Pause |
| Next track | Next track |
| Previous track | Previous track |
| Stop | Stop |
| Brightness up | Brightness up |
| Brightness down | Brightness down |
| Display | Screens |
| RFKill | Airplane mode |
| WLAN | Airplane mode |
| NotificationCenter | Notifications |
| Favorites | Star key |

## In the apps

Each app's main keys; the link has all of them.

| App | Main keys | All of them |
| --- | --- | --- |
| Top bar, dock, start menu | Esc closes any popup; in the start menu type to search, Enter opens; Win + / any key closes the shortcuts card | [keys/panel.md](keys/panel.md) |
| Minos (files) | Ctrl + T / W new / close tab, Alt + ← / → back / forward, Ctrl + F search, Ctrl + L address, F2 rename, Delete to the bin, Alt + Enter properties, F3 preview | [keys/minos.md](keys/minos.md) |
| Clipboard (Win + V) | ↑ ↓ move, Enter copies, Shift + Enter pastes, Tab history / snippets, Ctrl + P pin, Ctrl + N new snippet | [keys/clipboard.md](keys/clipboard.md) |
| Color Picker (Win + Shift + C) | arrows move one pixel, wheel zooms, Enter or click picks, Esc cancels | [keys/colorpicker.md](keys/colorpicker.md) |
| Screenshot, lock and login screens | screenshot: drag an area, click a window, Enter whole screen; lock: Enter unlocks, Esc clears; login: Tab another user, F2 session | [keys/lock-and-login.md](keys/lock-and-login.md) |
| Authenticator | type to search, ↑ ↓ move, Enter copies the code | [keys/authenticator.md](keys/authenticator.md) |
| Calculator | type the sum, Enter works it out, Esc clears | [keys/calculator.md](keys/calculator.md) |
| Saga MD (Markdown) | Ctrl + E formatted / source, Ctrl + S save, Ctrl + O open, Alt + ← back, Ctrl + + / − / 0 text size | [keys/saga-md.md](keys/saga-md.md) |
| Video player | Space or K play / pause, ← → or J / L step, ↑ ↓ volume, M mute, F full screen, O open | [keys/player.md](keys/player.md) |
| 2048 | arrows, WASD or HJKL slide, U undo, N new game | [keys/2048.md](keys/2048.md) |
| Sudoku | arrows or HJKL move, 1-9 place, N notes, U undo, Shift + H hint, P pause | [keys/sudoku.md](keys/sudoku.md) |
| Window switcher (Alt + Tab, Yggdrasil's own compositor) | while Alt is held: Tab / Shift + Tab next / back, arrows move in the grid, Enter, letting go of Alt or a click switches, Esc cancels | [COMPOSITOR.md](COMPOSITOR.md#window-switching-alt--tab-2026-11-19) |
| Settings, Agora, installer | no shortcuts of their own (Tab, Enter and typing as usual) | [keys/settings-agora-installer.md](keys/settings-agora-installer.md) |
