# Yggdrasil

**Yggdrasil** is a desktop environment written in Rust. It is the desktop of [Labyrinth OS](https://github.com/Artifex-Technologies/Labyrinth_OS)
and is built to be reused by other systems (Niflheim OS, formerly Chymaera OS, will use it too), so nothing here may depend on one
operating system: what an OS adds (an assistant, a security monitor, an installer's steps) it describes in integration files
([docs/INTEGRATIONS.md](docs/INTEGRATIONS.md)), and Yggdrasil shows the matching features.

Status: Yggdrasil runs as its own session on [Hyprland](https://hyprland.org) (`yggdrasil-session`), with Yggdrasil's own top bar,
dock, Settings, file manager, app store, lock and login screens, password box and installer. KDE is gone: nothing of it is
started, called or linked. Yggdrasil's own compositor is a first
version that runs in a window ([docs/COMPOSITOR.md](docs/COMPOSITOR.md)). The checklist for a desktop of its own:
[docs/INDEPENDENCE.md](docs/INDEPENDENCE.md).

![The desktop: the top bar, the dock, and notification pop-ups at the bottom right](assets/screenshots/desktop.png)

## What is in it

| Folder | Program | What it is |
| --- | --- | --- |
| `packages/session` | `yggdrasil-session` | the session on Hyprland: its configuration (Lua), window rules, keys, and a small Hyprland plugin so programs' own minimize buttons work |
| `packages/panel` | `yggdrasil-panel` | top bar, dock, tray, start menu, calendar, notifications, wallpaper, and Fylgja (the notch) ([docs/FYLGJA.md](docs/FYLGJA.md), [docs/NOTIFICATIONS.md](docs/NOTIFICATIONS.md)) |
| `packages/settings` | `yggdrasil-settings` | Settings: network, displays, sound, appearance, power and idle, users and their rights, security, restore points, apps and more |
| `packages/minos` | `minos` | the file manager, and (through `packages/portal`) the open and save dialog of every program ([keys](docs/keys/minos.md)) |
| `packages/market` | `agora` | Agora, the app store: the catalog, the Arch repositories, the AUR and Flathub in one place ([docs/MARKETPLACE.md](docs/MARKETPLACE.md)) |
| `packages/lock` | `yggdrasil-lock`, `yggdrasil-greeter`, `yggdrasil-shot` | the lock screen and the login screen with selectable designs ([docs/LOCKSCREEN.md](docs/LOCKSCREEN.md), [docs/LOGIN_SCREEN.md](docs/LOGIN_SCREEN.md)), the boot splash, and the screenshot tool (Print: freezes the screen first, so open menus can be captured) |
| `packages/installer` | `yggdrasil-installer` | the installer's window: language, keyboard (by type: QWERTY, QWERTZ, AZERTY, Dvorak, Colemak), time zone, disk (erase, or choose partitions beside Windows), account; the OS supplies the install steps ([docs/INSTALLER.md](docs/INSTALLER.md)) |
| `packages/player` | `yggdrasil-player` | video player: VLC (libVLC) in Yggdrasil's window |
| `packages/idle` | `yggdrasil-idle` | dims, locks, turns the screen off and sleeps when you are away; keyboard lights off with the screen |
| `packages/polkit` | `yggdrasil-polkit` | the password box for administrator actions, Wi-Fi and VPN passwords, and Bluetooth pairing |
| `packages/portal` | `yggdrasil-portal` | the file chooser portal (Minos) |
| `packages/keyring-sync` | `yggdrasil-keyring-sync` | keeps the keyring and the login password in step ([docs/USERS_KEYRING.md](docs/USERS_KEYRING.md)) |
| `packages/xmove` | `yggdrasil-xmove` | lets X11 programs that draw their own title bar (Steam) be moved and resized |
| `packages/compositor` | `yggdrasil-compositor` | Yggdrasil's own compositor (Smithay), in progress |
| `packages/calculator`, `packages/weather` | | small apps ([docs/WEATHER.md](docs/WEATHER.md)) |
| `packages/yggdrasil-ui`, `packages/yggdrasil-conf` | | the shared look (egui theme, icons, window chrome) and the shared settings and integration files |
| `packages/liki` | `liki-build` and others | **Liki**, the icon pack ([docs/LIKI.md](docs/LIKI.md)) |
| `packages/theme` | `ygg-theme`, `ygg-cursors` | the look of GTK and Qt programs (dark mode included) and the mouse pointer ([docs/THEME.md](docs/THEME.md)) |
| `packages/pkgbuilds` | | an Arch package recipe for each program, built from this repository |

<!-- shortcuts:start -->
## Keyboard shortcuts

The desktop's own (Win is the Super key). Settings > Keyboard Shortcuts and Win + / show the live list, your own
additions included; [docs/SHORTCUTS.md](docs/SHORTCUTS.md) is the same list. This part is made from
`packages/session/hyprland/yggdrasil.lua` (`YGGDRASIL_WRITE_DOCS=1 cargo test` in `packages/yggdrasil-conf`).

### Windows

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

### Desktops

| Keys | What it does |
| --- | --- |
| Win + Ctrl + ← | Previous desktop |
| Win + Ctrl + → | Next desktop |
| Win + Ctrl + 1-4 | Desktop 1-4 |
| Win + Ctrl + Shift + 1-4 | Move the window to desktop 1-4 |

### Apps

| Keys | What it does |
| --- | --- |
| Win + S | Talk to the assistant (hold) |
| Ctrl + Alt + S | Talk to the assistant (hold) |
| Win + V | Clipboard history and snippets |
| Win + E | Files (Minos) |
| Win + Return | Terminal |
| Win + Shift + C | Color picker |

### System

| Keys | What it does |
| --- | --- |
| Win | Start menu |
| Win + C | Calendar |
| Win + N | Notifications |
| Win + I | Settings |
| Win + L | Lock the screen |
| Ctrl + Shift + Esc | Running programs |
| Win + / | Keyboard shortcuts |

### Mouse

| Keys | What it does |
| --- | --- |
| Win + Left drag | Move the window |
| Win + Right drag | Resize the window |
| Ctrl twice | Find my mouse |

### Screenshots

| Keys | What it does |
| --- | --- |
| Print Screen | Screenshot (area, window or screen) |
| Win + Shift + S | Screenshot (area, window or screen) |

### Media keys

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
<!-- shortcuts:end -->

### In the apps

| App | Main keys | All of them |
| --- | --- | --- |
| Top bar, dock, start menu | Esc closes any popup; in the start menu type to search, Enter opens; Win + / any key closes the shortcuts card | [docs/keys/panel.md](docs/keys/panel.md) |
| Minos (files) | Ctrl + T / W new / close tab, Alt + ← / → back / forward, Ctrl + F search, Ctrl + L address, F2 rename, Delete to the bin, F3 preview | [docs/keys/minos.md](docs/keys/minos.md) |
| Clipboard (Win + V) | ↑ ↓ move, Enter copies, Shift + Enter pastes, Tab history / snippets, Ctrl + P pin, Ctrl + N new snippet | [docs/keys/clipboard.md](docs/keys/clipboard.md) |
| Color Picker (Win + Shift + C) | arrows move one pixel, wheel zooms, Enter or click picks, Esc cancels | [docs/keys/colorpicker.md](docs/keys/colorpicker.md) |
| Screenshot, lock and login screens | screenshot: drag an area, click a window, Enter whole screen; lock: Enter unlocks, Esc clears; login: Tab another user, F2 session | [docs/keys/lock-and-login.md](docs/keys/lock-and-login.md) |
| Authenticator | type to search, ↑ ↓ move, Enter copies the code | [docs/keys/authenticator.md](docs/keys/authenticator.md) |
| Calculator | type the sum, Enter works it out, Esc clears | [docs/keys/calculator.md](docs/keys/calculator.md) |
| Video player | Space or K play / pause, ← → or J / L step, ↑ ↓ volume, M mute, F full screen, O open | [docs/keys/player.md](docs/keys/player.md) |
| 2048 | arrows, WASD or HJKL slide, U undo, N new game | [docs/keys/2048.md](docs/keys/2048.md) |
| Sudoku | arrows or HJKL move, 1-9 place, N notes, U undo, Shift + H hint, P pause | [docs/keys/sudoku.md](docs/keys/sudoku.md) |
| Settings, Agora, installer | no shortcuts of their own (Tab, Enter and typing as usual) | [docs/keys/settings-agora-installer.md](docs/keys/settings-agora-installer.md) |

## Screenshots

Applications are shown as windows on a 1920×1080 desktop.

| | |
| --- | --- |
| ![The start menu](assets/screenshots/start-menu.png) | ![The calendar](assets/screenshots/calendar.png) |
| *The start menu: favourites, folders by category, search.* | *The calendar, from the date in the top bar.* |
| ![Fylgja, Stats](assets/screenshots/fylgja-stats.png) | ![Fylgja, Notifications](assets/screenshots/fylgja-notifications.png) |
| *Fylgja, the notch: Stats (processor, graphics card, memory, disk, network, the busiest apps).* | *Fylgja: the notification history, grouped by app.* |
| ![Settings, Appearance](assets/screenshots/settings-appearance.png) | ![Settings, Apps](assets/screenshots/settings-apps-list.png) |
| *Settings: Appearance.* | *Settings: Apps, sorted by use, with restart, quit and remove.* |
| ![Settings, Lock screen](assets/screenshots/settings-lock-screen-designs.png) | ![Minos](assets/screenshots/minos-explorer.png) |
| *Settings: the lock screen designs.* | *Minos, the file manager: tabs, places, details.* |
| ![Agora](assets/screenshots/marketplace.png) | ![The installer, keyboard](assets/screenshots/installer-keyboard.png) |
| *Agora, the app store.* | *The installer: keyboards by type.* |
| ![The installer, partitions](assets/screenshots/installer-disk-manual.png) | ![The installer, account](assets/screenshots/installer-account.png) |
| *The installer: choosing partitions beside Windows (sample disks).* | *The installer: the account (sample name).* |

The installer pictures use `yggdrasil-installer --demo PAGE`: sample disks and names, and nothing can be installed from it.

## Building

Every program is its own Cargo project (`cd packages/<name> && cargo build --release`). `packages/pkgbuilds/<name>/PKGBUILD` builds a package from the source next to it
(`makepkg -f` in that folder). Labyrinth OS builds them with its `scripts/build-local-packages.sh`. Tests: `cargo test` in each folder; CI runs them and `cargo audit`.

Some pages appear only when the operating system provides what they need (for example Settings > Security > Distress password needs
Labyrinth's `labyrinth-distress`, and the security monitor's pages need a `security` integration such as Heimdall's).

Licence: Mozilla Public License 2.0 ([LICENSE](LICENSE)). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for what is built on, under its own licences.
