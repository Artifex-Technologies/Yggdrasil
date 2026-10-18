# Yggdrasil: the desktop environment

Named by the owner on 2026-10-18. **Yggdrasil** is the desktop environment that replaces KDE; **Labyrinth OS** is the distribution that ships it. Chymaera OS
will use Yggdrasil as well, so Yggdrasil must never depend on anything that only Labyrinth has (the games stack, the Artifex store entries, Labyrinth's branding).

## What belongs to Yggdrasil

| Piece | Today's program (package) | Notes |
| --- | --- | --- |
| Panel: top bar, dock, tray, Fylgja, notifications, start menu, wallpaper | `labyrinth-panel` | |
| Settings | `labyrinth-settings` | Labyrinth-only pages (games, Artifex apps) are separable |
| File manager and file chooser | `minos`, `labyrinth-portal` | |
| Marketplace | `labyrinth-market` | shared by Labyrinth and Chymaera through editions |
| Lock screen, login screen | `labyrinth-lock`, `labyrinth-greeter` (package `labyrinth-lock`) | designs shared |
| Idle manager (dim, lock, screen off, sleep) | `labyrinth-idle` | |
| Polkit agent, keyring sync, distress password | `labyrinth-polkit`, `labyrinth-keyring-sync`, `labyrinth-distress` (+ PAM module) | |
| Compositor | `labyrinth-compositor` | first version, nested only (COMPOSITOR.md) |
| Calculator, weather | `labyrinth-calculator`, `labyrinth-weather` | small apps |
| Shared library and settings file | `labyrinth-ui`, `labyrinth-conf`; `~/.config/labyrinth/` | |

## What stays Labyrinth's (not Yggdrasil)
The installer and first boot (`labyrinth-install*`, `labyrinth-plasma-setup`), the boot menu theme, `labyrinth-gaming` and the Proton Games parts, the brand icon
pack and the Labyrinth app tiles (`labyrinth-brand-pack`, `labyrinth-app-icons`), the Sarina and REA setup scripts, and the Artifex store entries. Chymaera has its own.

## Naming plan
1. **Done (2026-10-18):** the name is used in the README, the About page ("Desktop: Yggdrasil") and new docs. Nothing was renamed in code.
2. **Next, one planned pass:** programs and packages of the pieces above become `yggdrasil-*` (`yggdrasil-panel`, `yggdrasil-settings`, `yggdrasil-lock`, `yggdrasil-greeter`,
   `yggdrasil-idle`, `yggdrasil-compositor`, ...), the crates `labyrinth-ui` and `labyrinth-conf` become `yggdrasil-ui` and `yggdrasil-conf`, and settings move to
   `~/.config/yggdrasil/` and `/etc/yggdrasil/`. The old names stay as symlinks and the old config folder is read when the new one is missing, so nothing breaks on a
   running machine. Every place that names them has to change together: PKGBUILDs, autostart files, `.desktop` files, polkit policies, D-Bus names
   (`org.labyrinth.*` stay until a release says otherwise), Heimdall's rules, the tests and the docs.
3. **Chymaera:** the Chymaera repository does not mention Labyrinth at all today. It takes the `yggdrasil-*` packages from the Yggdrasil repository when they exist; until then it
   keeps KDE.

Not decided: whether Yggdrasil gets its own repository (recommended once step 2 is done, so Chymaera and Labyrinth both depend on it), and whether the D-Bus names and
config format change in the same pass. Everything user-visible that already ships is renamed only with the owner's go-ahead.

## Survey of this computer (2026-10-18)
- Repository `Labyrinth_OS`: 716 tracked files, 130 text files mention "labyrinth"; 24 package recipes of which 13 are Labyrinth's own (`labyrinth-*`).
- Installed: 6 desktop programs in `/usr/bin` (calculator, idle, lock, market, panel, polkit, portal, settings, weather, distress, keyring-sync, cloud) and about 50 helper scripts in
  `/usr/local/bin` (`labyrinth-*`); one config folder `~/.config/labyrinth`, one data folder `~/.local/share/labyrinth`, `/etc/labyrinth`, `/usr/share/labyrinth`.
- Chymaera_OS: 67 tracked files, no mention of Labyrinth.
