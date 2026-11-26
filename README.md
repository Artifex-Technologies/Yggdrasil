# Yggdrasil

[![checks](https://github.com/Artifex-Technologies/Yggdrasil/actions/workflows/checks.yml/badge.svg)](https://github.com/Artifex-Technologies/Yggdrasil/actions/workflows/checks.yml)

**Yggdrasil** is a desktop environment written in Rust. It is the desktop of [Labyrinth OS](https://github.com/Artifex-Technologies/Labyrinth_OS)
and is built to be reused by other systems (LykanOS, formerly Chymaera OS, will use it too), so nothing here may depend on one
operating system: what an OS adds (an assistant, a security monitor, an installer's steps) it describes in integration files
([docs/INTEGRATIONS.md](docs/INTEGRATIONS.md)), and Yggdrasil shows the matching features.

Status: Yggdrasil runs as its own session on [Hyprland](https://hyprland.org) (`yggdrasil-session`), with Yggdrasil's own top bar,
dock, Settings, file manager, app store, lock and login screens, password box and installer. KDE is gone: nothing of it is
started, called or linked. Yggdrasil's own compositor is in progress: it runs nested in a window, headless for tests, and since 2026-11-15 on a real screen through DRM/KMS (the hardware backend's first milestone, tried in a virtual machine) ([docs/COMPOSITOR.md](docs/COMPOSITOR.md)); Hyprland stays the desktop's compositor until it is ready. Every program and helper of the desktop is Rust; the old Python and shell copies kept for a rollback were removed on 2026-11-15. The checklist for a desktop of its own:
[docs/INDEPENDENCE.md](docs/INDEPENDENCE.md).

**How it is tested.** Every push runs the `checks` workflow in an Arch container: each crate is formatted, linted, tested and
audited; the panel runs a stress test for leaks on a headless compositor; Yggdrasil's own compositor runs headless with the
real programs on it, and the login screen runs in cage on three screens; the session's configuration and the package
recipes are parsed. Every program ported from Python was measured against the Python it replaced, on the same inputs:
[docs/PORT_NUMBERS.md](docs/PORT_NUMBERS.md).

![The desktop: the top bar, the dock, and notification pop-ups at the bottom right](assets/screenshots/desktop.png)

## What is in it

| Folder | Program | What it is |
| --- | --- | --- |
| `packages/session` | `yggdrasil-session` | the session on Hyprland: its configuration (Lua), window rules, keys (with `yggdrasil-snap`, `yggdrasil-close` and `yggdrasil-logout` from `packages/wm`), and a small Hyprland plugin so programs' own minimize buttons work; the small tools (Task Manager, Settings, calculator, clipboard, password box, games) float on tiling screens too, at their own size |
| `packages/wm` | (library), `yggdrasil-snap`, `yggdrasil-close`, `yggdrasil-logout` | yggdrasil-wm: what Yggdrasil's own tools ask of the compositor (windows, desktops, screens, the pointer, placing and closing windows), on Hyprland through its socket or on Yggdrasil's own compositor through its control socket, whichever runs |
| `packages/panel` | `yggdrasil-panel` | top bar, dock, tray, start menu, calendar, notifications, wallpaper, and Fylgja (the notch) ([docs/FYLGJA.md](docs/FYLGJA.md), [docs/NOTIFICATIONS.md](docs/NOTIFICATIONS.md)) |
| `packages/settings` | `yggdrasil-settings` | Settings: Wi-Fi & Network (laid out as a phone's: the switch with the network in use under it, Known Networks, Other Networks, each with a padlock, signal bars and an (i); the (i) opens the network's own page: what it broadcasts, read without root and without sending anything (every access point of a mesh and its maker, band, channel, width, signal, security and PMF, Wi-Fi 4 to 7, WPS and the device details it announces, country, 802.11k/v/r, vendor extras, first and last seen), for a saved one its last connection, auto-join, private address, IPv4 and IPv6, DNS, proxy and forget, for the connected one what it gave this computer and This Network: Devices, who is on a trusted home network, from the security integration's `devices_scan`, and Wi-Fi checks, why the Wi-Fi is slow, from its `wifi_check`: [docs/INTEGRATIONS.md](docs/INTEGRATIONS.md); Previously connected networks lists every saved one, newest first; Wired only with a cable in), displays, sound, peripherals (the mouse and keyboard, and Razer lights, DPI and polling rate through OpenRazer), appearance, power, cooling and idle, users and their rights, security (with Privacy: the private microphone, the downloads guard, Remove hidden info in Minos, viewer apps on "ask" and look-alike name warnings; [docs/APPS_SETTINGS.md](docs/APPS_SETTINGS.md)), restore points, apps, games (graphics cards, game fixes, frame generation) and more |
| `packages/minos` | `minos` | the file manager, and (through `packages/portal`) the open and save dialog of every program; right-click > Remove hidden info (a copy without the place a photo was taken, the device and the author) and Scrub location sounds ([keys](docs/keys/minos.md)) |
| `packages/market` | `agora` | Agora, the app store: the catalog, the Arch repositories, the AUR and Flathub in one place, with apps whose name imitates another marked "Look-alike name" ([docs/MARKETPLACE.md](docs/MARKETPLACE.md)) |
| `packages/updates` | (library) | yggdrasil-updates: what the repositories can update, asked without root on a private copy of pacman's sync databases (as checkupdates does), shared by Agora and Settings > General > Software Update; pacman's log as transactions (Update History), and Pause updates |
| `packages/lock` | `yggdrasil-lock`, `yggdrasil-greeter`, `yggdrasil-shot` | the lock screen and the login screen with selectable designs ([docs/LOCKSCREEN.md](docs/LOCKSCREEN.md), [docs/LOGIN_SCREEN.md](docs/LOGIN_SCREEN.md)), the boot splash, and the screenshot tool (Print: freezes the screen first, so open menus can be captured) |
| `packages/installer` | `yggdrasil-installer` | the installer's window: language, network first (Wi-Fi, Ethernet or a USB adapter, hidden networks, each adapter's MAC address shown, networks that turn up later, or skip; the install's downloads are fetched beforehand), keyboard (by type: QWERTY, QWERTZ, AZERTY, Dvorak, Colemak), time zone, disk (erase, or choose partitions beside Windows), account; the OS supplies the install steps ([docs/INSTALLER.md](docs/INSTALLER.md)) |
| `packages/player` | `yggdrasil-player` | video player: VLC (libVLC) in Yggdrasil's window; looks nothing up online |
| `packages/taskmanager` | `yggdrasil-taskmanager` | the Task Manager (Ctrl + Shift + Esc), laid out like Windows 11's: processes with GPU columns, a Performance page for every graphics card (engines, memory, sensors, the AI models Ollama has loaded), startup apps, users, details and services ([README](packages/taskmanager/README.md)) |
| `packages/idle` | `yggdrasil-idle` | dims, blacks out the screen (pointer hidden), locks, turns the screen off and sleeps when you are away; keyboard and mouse lights off with the screen ([README](packages/idle/README.md)) |
| `packages/polkit` | `yggdrasil-polkit` | the password box for administrator actions, Wi-Fi and VPN passwords, and Bluetooth pairing |
| `packages/portal` | `yggdrasil-portal` | the portal backend: Minos as every program's open and save dialog, dark or light and the accent, and on the own compositor screen sharing, screenshots and picked colours ([README](packages/portal/README.md)) |
| `packages/keyring-sync` | `yggdrasil-keyring-sync` | keeps the keyring and the login password in step ([docs/USERS_KEYRING.md](docs/USERS_KEYRING.md)) |
| `packages/xmove` | `yggdrasil-xmove` | lets X11 programs that draw their own title bar (Steam) be moved and resized |
| `packages/compositor` | `yggdrasil-compositor` | Yggdrasil's own compositor (Smithay), in progress |
| `packages/screencast` | `yggdrasil-screencast` | screen sharing on the own compositor: a screen published as a PipeWire video stream, started by the portal ([README](packages/screencast/README.md)) |
| `packages/share-picker` | `yggdrasil-share-picker` | the question before a screen is shared: which screens a program may see ([README](packages/share-picker/README.md)) |
| `packages/calculator`, `packages/weather` | | small apps ([docs/WEATHER.md](docs/WEATHER.md)) |
| `packages/saga-md` | `saga-md` | Saga MD, the Markdown reader: a Markdown or text file formatted, or its source to read and edit; Ctrl + E switches ([README](packages/saga-md/README.md)) |
| `packages/spell` | `yggdrasil-spell`, `yggdrasil-autocorrect` | spell checking in every app: the Hunspell dictionaries, one word list your apps share, a list shared by the computer, and correct as you type ([README](packages/spell/README.md)) |
| `packages/clipboard` | `yggdrasil-clipboard` | clipboard history and snippets (Win + V) |
| `packages/colorpicker` | `yggdrasil-colorpicker` | pick a colour anywhere on the screen and copy it (Win + Shift + C) |
| `packages/authenticator` | `yggdrasil-authenticator` | Proton Authenticator's codes: type to search, Enter copies |
| `packages/timer` | `yggdrasil-timer` | a countdown timer in the top bar |
| `packages/health` | `yggdrasil-health` | break reminders (eyes, movement, water, breathing, meals, wind-down) with a top bar pill |
| `packages/leakwatch` | `yggdrasil-leakwatch` | the leak watch: every 5 minutes (a timer; nothing stays running) each program's memory and the images it left with Hyprland; a warning when one keeps growing for hours |
| `packages/crashwatch` | `yggdrasil-crashwatch` | the crash watch (off until turned on): a notice when one of your programs crashes, with "Ask ..." when the system has an assistant; about 0.8 MB while it waits ([README](packages/crashwatch/README.md)) |
| `packages/gametime` | `yggdrasil-gametime` | game time: while a game runs, background updates and chores wait and catch up afterwards; the leak watch, virus scans and Heimdall keep running ([docs/GAME_TIME.md](docs/GAME_TIME.md)) |
| `packages/controllers` | `yggdrasil-controllers` | game controllers in the top bar: battery, rumble test, light bar colour, notifications |
| `packages/gamepads` | | the game controller library that Settings and the controllers add-on share (reads sysfs) |
| `packages/device-offers` | `yggdrasil-device-offers` | device software: the optional parts this computer's devices ask for (the installer ticks them), offered later once online or when such a device is plugged in ([README](packages/device-offers/README.md)) |
| `packages/laptop` | `yggdrasil-laptop` | a laptop's hardware: tablet mode and screen rotation on a 2-in-1, and the fingerprint reader (off until Secure Boot is on and the disk is encrypted) |
| `packages/tzpicker` | | the time zone picker that Settings > Date & Time and the installer share: a world map and a short list ([README](packages/tzpicker/README.md)) |
| `packages/geolocate` | | where the computer roughly is, for Weather's automatic place and Settings' automatic time zone: its IP address only, through beaconDB (no Wi-Fi networks, no hardware IDs), and nothing while a VPN is on |
| `packages/clouddrives` | | the cloud drives Settings > Apps > Minos > Cloud drives adds, mounts and removes, and Minos lists under Cloud Storage: rclone remotes mounted by a user service (`rclone mount --vfs-cache-mode full`) and folders a sync app keeps ([README](packages/clouddrives/README.md)) |
| `packages/netjoin` | | joining a Wi-Fi network through NetworkManager's D-Bus call (the password never on a command line; a profile that failed is deleted) and nmcli's adapters and network list, shared by Settings and the installer's Network step |
| `packages/wifiscan` | | what nearby Wi-Fi networks broadcast, for Settings: the kernel's last scan read over nl80211 (no root, nothing sent), the information elements parsed as hostile input (property-tested), the vendor extras it recognises (makers come from Heimdall's `heimdall-macvendor` crate: all four IEEE registries, longest prefix first) |
| `packages/agent-usage` | `yggdrasil-agent-usage` | two top bar add-ons: how much of Claude's weekly and 5-hour limits is used, and Agents (every AI agent running, its state, usage and last activity) |
| `packages/browser-warm` | `yggdrasil-browser-warm` | keeps the default browser running with no windows, so new windows and links open almost at once |
| `packages/game-2048`, `packages/sudoku` | `yggdrasil-2048`, `yggdrasil-sudoku` | games: 2048 and Sudoku |
| `packages/pycompat` | | the few Python behaviours the Rust ports keep (how json.dumps prints, int() and strip() on text ...), so they print and decide as the scripts did |
| `packages/yggdrasil-ui`, `packages/yggdrasil-conf` | | the shared look (egui theme, icons, window chrome) and the shared settings and integration files |
| `packages/liki` | `liki-build` and others | **Liki**, the icon pack ([docs/LIKI.md](docs/LIKI.md)) |
| `packages/theme` | `ygg-theme`, `ygg-cursors` | the look of GTK and Qt programs (dark mode included) and the mouse pointer ([docs/THEME.md](docs/THEME.md)) |
| `packages/pkgbuilds` | | an Arch package recipe for each program, built from this repository |

## Version

`VERSION` is the system version, the same as Labyrinth OS's (Labyrinth OS 1.0 since 2026-11-11): every package built here has it, like an iPhone's built-in apps, as `VERSION-<build number>`; Settings > About shows it from `/etc/os-release`. How the numbers work and how to make a release: [Labyrinth_OS docs/VERSIONING.md](https://github.com/Artifex-Technologies/Labyrinth_OS/blob/main/docs/VERSIONING.md).

## Keyboard shortcuts

Every shortcut, the desktop's own and each app's, is in [docs/SHORTCUTS.md](docs/SHORTCUTS.md); each app's full list is in
[docs/keys/](docs/keys/). On the desktop, Win + / shows the live list, your own additions included.

## Screenshots

Applications are shown as windows on a 1920×1080 desktop, in the dark style. Everything in them is made up: the account
(Alex Example), the computer (example-pc), its files, networks, devices and numbers.

| | |
| --- | --- |
| ![The start menu](assets/screenshots/start-menu.png) | ![The calendar](assets/screenshots/calendar.png) |
| *The start menu: pinned apps, every app, search, and your name with the power choices.* | *The calendar, from the date in the top bar.* |
| ![Fylgja, Stats](assets/screenshots/fylgja-stats.png) | ![Fylgja, Notifications](assets/screenshots/fylgja-notifications.png) |
| *Fylgja, the notch: Stats (processor, graphics card, memory, disk, network, the busiest apps).* | *Fylgja: the notification history.* |
| ![The lock screen](assets/screenshots/lock-screen.png) | ![Settings, Lock screen](assets/screenshots/settings-lock-screen-designs.png) |
| *The lock screen (the Split design).* | *Settings: the lock screen designs, each with its preview.* |
| ![Settings, Wi-Fi & Network](assets/screenshots/settings-network.png) | ![Settings, Displays](assets/screenshots/settings-displays.png) |
| *Settings: Wi-Fi & Network, laid out like a phone's.* | *Settings: Displays, an ultrawide and a second screen.* |
| ![Settings, Appearance](assets/screenshots/settings-appearance.png) | ![Settings, Apps](assets/screenshots/settings-apps-list.png) |
| *Settings: Appearance.* | *Settings: Apps, sorted by use, with restart, quit and remove.* |
| ![Settings, Power on a laptop](assets/screenshots/settings-power-laptop.png) | ![Settings, Cooling, a fan's curves](assets/screenshots/settings-cooling-curves.png) |
| *Settings: Power on a laptop: the battery, its health and charge limit, and the fan the firmware runs.* | *Settings: Power > Cooling on a desktop, a fan's own curves, one for the processor and one for the graphics card, dragged point by point. Another fan can follow them.* |
| ![Settings, Games](assets/screenshots/settings-games.png) | ![Settings, Peripherals, Mouse](assets/screenshots/settings-peripherals-mouse.png) |
| *Settings: Games. The graphics cards (with two or more, a Steam launch option runs a game on the one you pick), game fixes with Undo, and frame generation per game.* | *Settings: Peripherals > Mouse. Pointer speed, acceleration, scrolling and the main button, for every mouse or one by one.* |
| ![Minos](assets/screenshots/minos-explorer.png) | ![The Task Manager](assets/screenshots/taskmanager.png) |
| *Minos, the file manager: tabs, places, details.* | *The Task Manager (Ctrl + Shift + Esc): Performance, a graphics card's page.* |
| ![Settings, Security, Privacy](assets/screenshots/settings-security-privacy.png) | ![Minos, right-click on a recording](assets/screenshots/minos-privacy-menu.png) |
| *Settings: Security > Privacy. The private microphone and the privacy pack, each with its own switch, all on by default.* | *Minos: right-click > Scrub location sounds and Remove hidden info, each writing a cleaned copy beside the file.* |
| ![Agora](assets/screenshots/marketplace.png) | ![Saga MD](assets/screenshots/saga-md.png) |
| *Agora, the app store.* | *Saga MD, the Markdown reader.* |
| ![The calculator](assets/screenshots/calculator.png) | ![The installer, keyboard](assets/screenshots/installer-keyboard.png) |
| *The calculator.* | *The installer: keyboards by type.* |
| ![The installer, partitions](assets/screenshots/installer-disk-manual.png) | ![The installer, account](assets/screenshots/installer-account.png) |
| *The installer: choosing partitions beside Windows (sample disks).* | *The installer: the account (sample name).* |
| ![The installer, time zone](assets/screenshots/installer-timezone.png) | ![Settings, Date & Time](assets/screenshots/settings-date-time-light.png) |
| *The installer: the time zone on the map, or in the short list beside it.* | *Settings: Date & Time in the light style, the same map and list.* |
| ![The installer, network](assets/screenshots/installer-network.png) | ![The installer, hidden network](assets/screenshots/installer-network-hidden.png) |
| *The installer: join a network first, or go on without one (made-up networks and addresses).* | *The installer: a hidden network, with this computer's real address for a network that reserves one.* |

Every page of Settings, each with a picture: [docs/APPS_SETTINGS.md](docs/APPS_SETTINGS.md#every-page).

### Making them again

    scripts/screenshots.sh                  # builds what it needs (debug), then every picture into assets/screenshots
    scripts/screenshots.sh --only settings  # only the pictures whose name matches
    scripts/screenshots.sh --list           # their names

One command makes every picture above and in docs/ from this repository's own build, on Yggdrasil's headless compositor
(`yggdrasil-compositor --headless`, [docs/COMPOSITOR.md](docs/COMPOSITOR.md)) with the panel, and never touches the running
desktop: nothing opens on the screen, and it all runs in a bubblewrap sandbox with a blank /home, /run and /tmp, so the desktop's
runtime folder, its session and system buses, the network and the input devices are not there. The made-up computer is in
`tests/screenshots` (the account, a home folder with a few files, the processor and memory, a graphics card, a laptop's sensors, and
stand-ins for the programs the apps run directly) and in Settings' fixtures (`packages/settings/tests/fixtures`: `system.json`,
`laptop.json`, `wifi.json`, `peripherals.json`, read only by a debug build). It takes about 10 minutes; each picture waits until
the screen stops changing. The installer pictures use `yggdrasil-installer --demo PAGE`, and the lock screen is its test build
(a stand-in password check). A new picture is one `shot` line in the script, and a click or typing before it one `ACTIONS` entry.
CI's `compositor-headless` job runs it on Settings' pages, so it keeps working.

## Why Rust: measured

Every program I ported from Python runs 60-99 % faster in Rust in most cases (`casefan get` 39.9 → 2.16 ms) and needs up to 98 % less
memory; `ygg-cursors`, slower at first, draws the cursor theme in 40 ms where Python took a second. Every program, its numbers
and where Rust is worse: [docs/RUST_MIGRATION.md](docs/RUST_MIGRATION.md).

## Building

Every program is its own Cargo project (`cd packages/<name> && cargo build --release`); they all build into one shared folder,
`target/` at the top of the repository (`.cargo/config.toml`), so the libraries they share are compiled once. A program built
by hand is in `target/release/`. `packages/pkgbuilds/<name>/PKGBUILD` builds a package from the source next to it
(`makepkg -f` in that folder). Labyrinth OS builds them with its `labyrinth-publish-repo build` (only what changed), for any 64-bit CPU
(its `scripts/makepkg.conf`); a plain `makepkg` uses this machine's settings, which on CachyOS build for this CPU only. Tests: `cargo test` in each folder; CI runs them and `cargo audit`.

Some pages appear only when the operating system provides what they need (for example Settings > Security > Distress password needs
Labyrinth's `labyrinth-distress`, and the security monitor's pages need a `security` integration such as Heimdall's).

Licence: Mozilla Public License 2.0 ([LICENSE](LICENSE)). See [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md) for what is built on, under its own licences.

## Artifex Ecosystem

| Repository | What it is |
| --- | --- |
| [Labyrinth_OS](https://github.com/Artifex-Technologies/Labyrinth_OS) | privacy- and security-first Linux distribution (CachyOS-based), made for gaming too |
| [LykanOS](https://github.com/Artifex-Technologies/LykanOS) | the security edition, built on the same desktop and tools |
| [Yggdrasil](https://github.com/Artifex-Technologies/Yggdrasil) | **this repository**: the desktop environment both systems use (panel, Settings, Agora, Minos, the compositor) |
| [Sarina](https://github.com/Artifex-Technologies/Sarina) | optional local AI assistant that runs on this computer |
| [Heimdall](https://github.com/Artifex-Technologies/Heimdall) | the security agent: watches the computer, its network and Sarina |
| [Limbo](https://github.com/Artifex-Technologies/Limbo) | disposable sandboxes for apps, Windows programs and untrusted files |
| [Kairos](https://github.com/Artifex-Technologies/Kairos) | frame generation for games (a Vulkan layer) |
| [Argus_App](https://github.com/Artifex-Technologies/Argus_App) | desktop console for the Flipper Zero (a qFlipper fork) |
| [Argus_Firmware](https://github.com/Artifex-Technologies/Argus_Firmware) | Flipper Zero firmware fork |
| [Daedalus_CAD](https://github.com/Artifex-Technologies/Daedalus_CAD) | 3D modelling and CAD (SketchUp-style) |
| [Icarus](https://github.com/Artifex-Technologies/Icarus) | vehicle diagnostics and tuning |
