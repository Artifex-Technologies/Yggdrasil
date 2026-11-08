# Yggdrasil: the desktop environment

Named by the owner on 2026-10-18. **Yggdrasil** is the desktop environment that replaced KDE (KDE is gone since 2026-10-24); **Labyrinth OS** is the distribution that ships it. LykanOS (formerly
Chymaera OS, then Niflheim OS; [LykanOS](https://github.com/Artifex-Technologies/LykanOS)) will use Yggdrasil as well, so Yggdrasil must never depend on anything that only Labyrinth has (the games stack, the Artifex store entries, Labyrinth's branding).

## What belongs to Yggdrasil

| Piece | Today's program (package) | Notes |
| --- | --- | --- |
| Session (on Hyprland 0.57: configuration, window rules, keys) | `yggdrasil-session` | the desktop's login entry "Yggdrasil". It is Hyprland's watchdog: after a crash Hyprland starts again with Yggdrasil's settings (safe mode only after 3 crashes in 10 minutes), the crashed run's log kept in `~/.local/state/yggdrasil/hyprland-logs/`; monitor connects and disconnects are timed in `monitor-events.log` there |
| Panel: top bar, dock, tray, Fylgja, notifications, start menu, wallpaper | `yggdrasil-panel` | |
| Settings | `yggdrasil-settings` | Labyrinth-only pages (games, Artifex apps) are separable |
| File manager and file chooser | `minos`, `yggdrasil-portal` | |
| Marketplace | `agora` | shared by Labyrinth and LykanOS through editions |
| Lock screen, login screen | `yggdrasil-lock`, `yggdrasil-greeter` (package `yggdrasil-lock`) | designs shared |
| Idle manager (dim, black screen, lock, screen off, sleep) | `yggdrasil-idle` | the pointer hides while dimmed or black. Black screen keeps the monitor on (black pixels; kind to OLEDs), unlike screen off, which some monitors treat as unplugging |
| Polkit agent, keyring sync, distress password | `yggdrasil-polkit`, `yggdrasil-keyring-sync`, `labyrinth-distress` (+ PAM module) | |
| Compositor | `yggdrasil-compositor` | first version, nested only (COMPOSITOR.md); Hyprland is the compositor today |
| Calculator, weather | `yggdrasil-calculator`, `yggdrasil-weather` | small apps |
| Saga MD (Markdown reader) | `saga-md` | Markdown and text files, formatted or as their source (replaced ghostwriter, KDE's, 2026-11-08) |
| Look for other programs (colour scheme, GTK colours, cursors) | `ygg-theme`, `ygg-cursors` (package `yggdrasil-theme`; [THEME.md](THEME.md)) | replaced WhiteSur |
| Icons | `liki-build` and the others (package `liki-icon-theme`; [LIKI.md](LIKI.md)) | the pack is Liki |
| Shared library and settings file | `yggdrasil-ui`, `yggdrasil-conf`; `~/.config/yggdrasil/` | renamed from `labyrinth-ui`, `labyrinth-conf` and `~/.config/labyrinth/` on 2026-10-23 |

## What stays Labyrinth's (not Yggdrasil)
The installer and first boot (`labyrinth-install*`; the KDE first-login script `labyrinth-plasma-setup` went with KDE), the boot menu theme, `labyrinth-gaming` and the Proton Games parts, the brand icon
pack and the Labyrinth app tiles (`labyrinth-brand-pack`, `labyrinth-app-icons`), the Sarina and REA setup scripts, and the Artifex store entries. LykanOS has its own.

## Naming plan
1. **Done (2026-10-18):** the name is used in the README, the About page ("Desktop: Yggdrasil") and new docs. Nothing was renamed in code.
2. **Done (2026-10-23), in one pass:** programs and packages of the pieces above become `yggdrasil-*` (`yggdrasil-panel`, `yggdrasil-settings`, `yggdrasil-lock`, `yggdrasil-greeter`,
   `yggdrasil-idle`, `yggdrasil-compositor`, ...), the crates `labyrinth-ui` and `labyrinth-conf` become `yggdrasil-ui` and `yggdrasil-conf`, and settings move to
   `~/.config/yggdrasil/` and `/etc/yggdrasil/`. The old names stay as symlinks and the old config folder is read when the new one is missing, so nothing breaks on a
   running machine. Every place that names them has to change together: PKGBUILDs, autostart files, `.desktop` files, polkit policies, D-Bus names
   (now `org.yggdrasil.*`, such as `org.yggdrasil.Panel`), Heimdall's rules, the tests and the docs.
3. **LykanOS:** the LykanOS repository (renamed from Chymaera_OS to Niflheim_OS, then LykanOS, on 2026-11-07) does not mention Labyrinth. It takes the `yggdrasil-*` packages from the Yggdrasil repository; until it does, it
   keeps KDE (its package list still has Plasma and SDDM, checked 2026-11-04).

Decided (owner, 2026-10-18):
- The operating system is still called **Labyrinth**; Yggdrasil is only the desktop environment.
- Step 2 (the rename) **waits until the compositor can replace KWin**, so the whole desktop is renamed once, when it is whole. (In the end it was done on
  2026-10-23, when Hyprland was chosen as the compositor instead; Yggdrasil's own compositor is still in progress.)
- Yggdrasil gets **its own repository**: done, [Artifex-Technologies/Yggdrasil](https://github.com/Artifex-Technologies/Yggdrasil) (the desktop's code moved there on 2026-10-18).
- LykanOS (then Chymaera) is on hold: the owner wants Labyrinth in a better state first and does not have LykanOS's target device yet. When it starts, LykanOS uses **Labyrinth as a
  template** (installer, boot theme and the like, with LykanOS's own branding) and Yggdrasil as its desktop. Until then, keep Labyrinth's installer and boot theme easy to
  re-brand: one place for names, colours and logos, no Labyrinth wording baked into logic.
The D-Bus names changed in the same pass as the rename (2026-10-23). Everything user-visible that already ships is renamed only with the owner's go-ahead.

## Survey of this computer (2026-10-18)
- Repository `Labyrinth_OS`: 716 tracked files, 130 text files mention "labyrinth"; 24 package recipes of which 13 are Labyrinth's own (`labyrinth-*`).
- Installed: 6 desktop programs in `/usr/bin` (calculator, idle, lock, market, panel, polkit, portal, settings, weather, distress, keyring-sync, cloud) and about 50 helper scripts in
  `/usr/local/bin` (`labyrinth-*`); one config folder `~/.config/labyrinth`, one data folder `~/.local/share/labyrinth`, `/etc/labyrinth`, `/usr/share/labyrinth`.
- Chymaera_OS (now LykanOS): 67 tracked files, no mention of Labyrinth.
