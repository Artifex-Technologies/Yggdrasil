# Yggdrasil on its own: no KDE, no Labyrinth

> **Status 2026-11-04: KDE is gone.** It was removed from the owner's PC on 2026-10-24 and from the code on 2026-10-26 (commit 93cd99d:
> nothing of KDE is started, called or linked). Yggdrasil runs as its own session on Hyprland 0.57 (`packages/session`); its own
> compositor is still in progress and runs only nested. Part 1 below is kept as the record of the audit of 2026-10-23; the "Now"
> notes say what is true today. Kept on purpose: the tray protocol's standard `org.kde` names, `kdeglobals` (the file Qt programs
> read colours from), `X-KDE` autostart keys other programs use, and a one-time import of the tiling and colour choices made under KDE.
> Still missing since KDE left: what `kaccess` and KWin gave for accessibility (sticky keys, magnifier and the rest; ACCESSIBILITY.md).

The single checklist for making Yggdrasil a desktop environment of its own (audited 2026-10-23 from the running machine and both
repositories; it replaces the partial snapshot in [KDE_INVENTORY.md](KDE_INVENTORY.md)). Two goals:

1. **No KDE.** Every KDE piece is either rewritten in Rust as part of Yggdrasil, replaced by a small standard non-KDE component, or removed.
2. **No Labyrinth inside Yggdrasil.** Yggdrasil is the desktop; Labyrinth OS, Niflheim OS and any other system install it. Nothing in
   Yggdrasil may be named after, depend on, or assume one OS. OS-specific features plug in from the OS side.

Decisions this follows: Rust and lightweight (the "must be light" rule); secrets in gnome-keyring, not KWallet and not our own
(2026-10-16; kept on 2026-11-07, no oo7); replace KDE before new desktop features, the PowerToys-style tools parked on branch `tools-wip` (2026-10-23).

---

## Part 1: KDE, everything that was still there (2026-10-23)

Audit numbers: 114 KDE packages installed (44 in the `plasma` group, 62 KDE Frameworks, 4 KDE applications, 24 installed explicitly);
23 KDE packages in Labyrinth's ISO list; 29 source files in Yggdrasil call into KDE; 9 Labyrinth scripts and 12 config files in the
Labyrinth image configure KDE. **Yggdrasil's own packages depend on KDE libraries**: `yggdrasil-panel` and `yggdrasil-settings` need
`libkscreen` and `kconfig`.

### 1a. Can go before the compositor (each needs its Yggdrasil replacement finished and switched on first)

**What KWin keeps tied to it (found 2026-10-23):** KWin 6.8 offers neither `ext-session-lock-v1` (Yggdrasil's lock screen), output power control (display off) nor the standard screen-copy protocols (the portal's screenshots and screen sharing). Those rows therefore move with the compositor (1b), not before it.


| KDE piece | What it did (2026-10-23) | Replacement | State |
| --- | --- | --- | --- |
| ✅ 2026-10-23 `powerdevil` | dim, screen off, sleep, brightness, power profiles, battery | `yggdrasil-idle` (timers, and the "don't sleep" requests as logind block locks) + power-profiles-daemon; brightness keys through the panel and `brightnessctl` | masked; display-off still calls `kscreen-doctor` until the compositor offers output power. Not yet: low-battery warning (laptops). **Now (2026-11-04):** display-off goes through the compositor's output-power protocol; no KDE tool is called. |
| `xdg-desktop-portal-kde` | screenshot, screen sharing, colour pick, settings, inhibit, notifications, app chooser | `yggdrasil-portal` (Rust) | file chooser done; the rest to do (screen capture goes through KWin's interface until the compositor). **Now (2026-11-04):** KDE's portal is gone; `yggdrasil-portal` serves the file chooser and appearance (dark or light, accent), Hyprland's portal screenshots and screen sharing, the GTK one the rest. |
| ✅ `kwallet-pam`, `kwalletd6`, `ksecretd`, `signon-kwallet-extension` | secret storage | **gnome-keyring** (decided 2026-11-07: I stay on GNOME Keyring; the move to oo7, chosen on 2026-10-23, is dropped) | done: KWallet is gone; gnome-keyring serves the secrets and `yggdrasil-keyring-sync` works with it. |
| `kscreenlocker` | lock screen | `yggdrasil-lock` (Rust, ext-session-lock) | built; switch on with a rescue path. **Now (2026-11-04):** kscreenlocker is gone; `yggdrasil-lock` is the lock screen (Hyprland offers ext-session-lock). |
| ✅ 2026-10-24 `sddm` | login screen | greetd + `yggdrasil-greeter` (Rust) | in use on the owner's machine (SDDM removed) and in the Labyrinth image (the live medium signs in through greetd's `initial_session`) |
| ✅ 2026-10-23 `plasma-nm` (secret agent in kded) | Wi-Fi password prompts | NetworkManager secret agent in the question box (`yggdrasil-polkit`, netagent.rs): Wi-Fi, 802.1x, VPN, mobile; answers only NetworkManager, stores nothing | done |
| ✅ 2026-10-23 `bluedevil` | Bluetooth pairing prompts | BlueZ `Agent1` in the question box (btagent.rs): codes shown, typed or confirmed; answers only BlueZ | done (pairing a new device not yet tried with real hardware) |
| `plasma-pa` | volume applet (kded parts) | Yggdrasil sound (Settings + Fylgja already) | remove once nothing calls it. **Now (2026-11-04):** removed. |
| ✅ 2026-10-23 `kded6` (25 modules) | tray registry, Wi-Fi and Bluetooth prompts, media keys, GTK theme sync, accent, low-disk notice, and unused ones | panel: tray registry (snwatcher.rs), media keys (mediakeys.rs, the same shortcut names), low-disk warning; portal: dark/light and accent (`appearance.conf`); question box: Wi-Fi and Bluetooth | masked; kaccess stays (screen reader key, sticky keys) until the compositor. Not yet: the out-of-memory notice. **Now (2026-11-04):** kded and kaccess are gone; the panel's own watcher, the media keys (bound by Hyprland to `yggdrasil-mediakey`) and the rest are in use. **Nothing replaces kaccess yet**: no sticky keys and no screen-reader key. |
| `plasma-workspace` (`plasmashell`, `startplasma`, `ksmserver`) | wallpaper, session start and logout, autostart | wallpaper layer in the panel; session = a systemd user target that starts the compositor and Yggdrasil | to do (session start with care: it is how you log in). **Now (2026-11-04):** done: `yggdrasil-session` (on Hyprland) and the `yggdrasil-session.target` user target; the panel draws the wallpaper. |
| `kscreen`, `libkscreen` (+ `kscreen-doctor` in 4 Yggdrasil files) | display layout and scale | Settings > Displays speaking the output-management Wayland protocol itself; drops the `libkscreen` dependency | to do. **Now (2026-11-04):** done differently: Settings > Displays writes Hyprland's monitor settings; the `libkscreen` dependency is gone. |
| `kconfig` (library dependency of panel and Settings) | reading KDE config files | plain file parsing; nothing of ours should read KDE config at all once KDE is gone | to do. **Now (2026-11-04):** the `kconfig` dependency is gone (plain file parsing). Still read on purpose: `kdeglobals` (written by Yggdrasil for Qt programs) and, once, the tiling and colour choices KDE left in `kwinrc` and `kdeglobals`. |
| `plasma-systemmonitor` | task manager (Ctrl+Shift+Esc) | a small Yggdrasil task manager | to do. **Now (2026-11-04):** removed; Ctrl+Shift+Esc opens `btop` in a terminal. A Yggdrasil task manager is still to do. |
| `plasma-firewall` | firewall page | Settings > Firewall (exists) | remove. **Now (2026-11-04):** removed. |
| `spectacle` | screenshots | Yggdrasil screenshot through `yggdrasil-portal` | to do. **Now (2026-11-04):** removed; `yggdrasil-shot` takes screenshots (Print, Super+Shift+S). |
| `breeze`, `breeze-gtk`, `kde-gtk-config`, `klassy`, `kvantum` | KDE theme, GTK theme bridge, window decoration, Qt theming | `yggdrasil-theme` writes GTK (and Qt via a plain qt6ct/kvantum-free style) settings itself; cursor theme of our own or a non-KDE one | to do. **Now (2026-11-04):** done: none of these is installed; `yggdrasil-theme` writes the GTK and Qt colours and has its own cursor theme (THEME.md). |
| `kdeplasma-addons`, `plasma6-applets-window-title`, `plasma-browser-integration`, `kio-extras`, `baloo`, `milou`, `kactivitymanagerd`, `kaccess`, `drkonqi`, `kinfocenter`, `systemsettings`, `kmenuedit` | Plasma extras | nothing (Minos has its own search; Settings is ours) | remove (kaccess: only after the accessibility plan). **Now (2026-11-04):** all removed, kaccess included, before the accessibility plan had a replacement for it. |
| ✅ 2026-10-24 Labyrinth image: `labyrinth-plasma-setup`, `labyrinth-hide-kde-settings`, `labyrinth-displays(-kde)`, `labyrinth-panel-mode`, `labyrinth-switch-mode`, `labyrinth-tiling`, `labyrinth-lockwatch`, `labyrinth-keyring-migrate`, `labyrinth-selfcheck`; `/etc/xdg/kwinrc`, `kscreenlockerrc`, `powerdevilrc`, `plasma-workspace`, `kde-portals.conf`, `sddm.conf.d`, `kwinrulesrc`, `/usr/share/kwin`, `/usr/share/labyrinth/plasma` | configure KDE | each goes with the piece it configures | removed from the Labyrinth image (labyrinth-displays lost its KDE mode; labyrinth-selfcheck checks Hyprland and greetd instead) |

### 1b. The compositor (decided 2026-10-23: chosen by measurement, the shell made independent of it)

Yggdrasil's shell (panel, dock, Fylgja, launcher, lock, notifications) is moved onto **standard Wayland protocols only**
(layer-shell, ext/wlr foreign-toplevel, wlr output management, ext-session-lock, ext-idle-notify, ext-workspace, screencopy through
the portal), so it runs on any compliant compositor and does not care which one an OS picks. The compositor is then chosen by a
**bake-off on real hardware** (first: RTX 2080 Ti, NVIDIA 615, 3440x1440 at 180 Hz, HDR-capable): KWin as the baseline, Hyprland,
cosmic-comp, and Yggdrasil's own Smithay compositor, measured on idle memory and CPU, frame pacing and input latency in real games
(MangoHud logs), VRR at 180 Hz, tearing for low latency, HDR, NVIDIA stability, Xwayland/Steam games, screen sharing. An upstream,
distro-packaged compositor that wins is preferred (it updates with the system); window tools (FancyZones, Grab And Move, Window
Hopper) then talk to it through one small adapter.

| KDE piece | What it did (2026-10-23) | Replacement |
| --- | --- | --- |
| `kwin` (+ `kwin-scripts-krohnkite`, `/usr/share/kwin` scripts) | draws every window, input, the Wayland protocols the panel uses (`plasma-window-management`, `org_kde_plasma_shell`), tiling, rules | `yggdrasil-compositor` (Rust, Smithay): standard protocols (layer-shell, foreign-toplevel, output management, screencopy). **Now (2026-11-04):** Hyprland won the bake-off and is the compositor; KWin and Krohnkite are gone. `yggdrasil-compositor` is still a first version that runs only nested; its hardware backend is not built. |
| `kglobalaccel` | global shortcuts, including KWin's own | shortcuts inside the compositor. **Now (2026-11-04):** Hyprland binds every key; kglobalacceld is gone (the KDE Frameworks library `kglobalaccel` is still installed only because a KDE-based app, ghostwriter, needs it). |
| KWin D-Bus calls in Yggdrasil (`org.kde.KWin` in 15 files) | effects, reconfigure, screenshots, colour pick | the compositor's own interface. **Now (2026-11-04):** no `org.kde.KWin` call is left in Yggdrasil. |

Until then KWin and kglobalaccel are the **only** KDE pieces that should remain. The parked tools are written on the compositor.
(Now: no KDE piece of the desktop remains. The parked tools were resumed on 2026-10-25, written for Hyprland and Yggdrasil's panel.)

### 1c. Not the desktop: applications and the installer

These use KDE's libraries (KDE Frameworks) without being the desktop. The libraries stay as long as any of them is installed.

| Package | Uses | Choice for the owner |
| --- | --- | --- |
| `kdenlive` (video editor), `ghostwriter` (Markdown) | KDE Frameworks | keep as apps (Agora), or pick non-KDE ones (e.g. Shotcut/Pitivi; Apostrophe). **Now (2026-11-04):** ghostwriter is installed, kdenlive is not. |
| `calamares` (Labyrinth's installer), `kpmcore` | KDE Frameworks | Labyrinth's choice: keep, or a Rust installer of its own. **Now (2026-11-04):** `yggdrasil-installer` exists; calamares and kpmcore are still installed on the owner's PC. **2026-11-07:** Calamares is removed from Labyrinth's ISO; `yggdrasil-installer` is its installer. |
| `kaccounts-integration`, `kirigami-addons`, `kquickimageeditor` | pulled in by the above | go with them. **Now (2026-11-04):** not installed. |

---

## Part 2: Labyrinth inside Yggdrasil

Audit numbers: 11 packages named `labyrinth-*`; crates `labyrinth_conf`, `labyrinth_ui`, `labyrinth_weather`; 658 lines mention Labyrinth.

| What | Today | Neutral |
| --- | --- | --- |
| ✅ 2026-10-23 Package and program names | `labyrinth-panel`, `-settings`, `-minos`, `-market`, `-lock`, `-greeter`, `-polkit`, `-portal`, `-idle`, `-calculator`, `-weather`, `-keyring-sync` | `yggdrasil-*`; Minos keeps `minos`, Agora is `agora` (package `yggdrasil-agora`), Liki keeps its name |
| ✅ 2026-10-23 Crates | `labyrinth_conf`, `labyrinth_ui`, `labyrinth_weather` | `yggdrasil_conf`, `yggdrasil_ui`, `yggdrasil_weather` |
| ✅ 2026-10-23 Config and data | `~/.config/labyrinth/` (24 uses), `/usr/share/labyrinth/` (14), `~/.local/state/labyrinth/` | `~/.config/yggdrasil/`, `/usr/share/yggdrasil/`; old files migrated once on first start |
| ✅ 2026-10-23 D-Bus | `org.labyrinth.Panel`, portal `org.freedesktop.impl.portal.desktop.labyrinth` | `org.yggdrasil.Panel`, `org.freedesktop.impl.portal.desktop.yggdrasil` |
| Environment | `LABYRINTH_*` | `YGGDRASIL_*`: ✅ 2026-10-23 for the desktop's own; the OS ones (Proton Drive, edition, Sarina, app network, Agora's install helper) go with the integration hooks |
| ✅ 2026-10-23 UI text | "Labyrinth Settings", "Labyrinth Marketplace", "Labyrinth Calculator", "Labyrinth Panel" | "Settings", "Agora", "Calculator", "Yggdrasil Panel"; the OS name, where shown (About), read from `/etc/os-release` (with the integration hooks) |
| Calls into Labyrinth OS (progress: ✅ 2026-10-23 the hook itself, docs/INTEGRATIONS.md; ✅ assistant: panel, Fylgja, compositor key and a Settings page of declared controls; ✅ packages: Agora's catalog, components and own builds, its root helpers now Agora's own; ✅ branding: accent and wallpaper; ✅ security: alerts, Settings page, quarantine, scan, distress password, lock screen; ✅ apps: the launcher, network rules, sandbox and usage in Settings > Apps; ✅ settings-page: the file format (docs/INTEGRATIONS.md) and its reader (yggdrasil-conf/src/integrations.rs); Settings does not list those pages as rows yet) | `labyrinth-apps` (Agora installs), `labyrinth-launch`, `labyrinth-panel-mode`, `labyrinth-quarantine`, `labyrinth-sarina`, `labyrinth-sarina-speak`, Sarina/Heimdall/Limbo pages in panel, Settings, Minos, compositor | **integration points**: Yggdrasil defines a small, documented hook (a folder of integration files, e.g. `/usr/share/yggdrasil/integrations/*.toml`: a Fylgja section, a Settings page, a shortcut, a Minos menu item, an Agora installer backend). Labyrinth ships its Sarina, Heimdall, Limbo and package-installer integrations there; another OS ships its own or none. Yggdrasil never names them. |
| ✅ 2026-10-23 Proton Drive in Minos | `LABYRINTH_PROTON_DRIVE`, rclone glue | kept as Minos's own rclone feature (it is not Labyrinth's): the user unit ships with Minos, `MINOS_PROTON_DRIVE` / `MINOS_RCLONE_SOCKET` |
| ✅ 2026-10-23 Branding assets | Labyrinth logos or colours inside Yggdrasil packages | Yggdrasil's own; the OS supplies its logo and default accent through the integration folder |

---

| Desktop pieces living in Labyrinth's repository (found 2026-10-23; ✅ the account and power-limit helpers moved into Settings' package as `/usr/lib/yggdrasil/{account,powerlimit}`, the Agora helpers as `/usr/lib/agora/{sudo,pkg-root}`, the Proton Drive unit into Minos, `yggdrasil-panelctl`, `yggdrasil-mediakey` and `yggdrasil-mount-open` into the panel package; the KDE-era ones, `labyrinth-panel-mode` with `labyrinth-notify-activate`, `labyrinth-tiling`, the KWin screen-keys script and the `systemsettings` wrapper, leave with KDE in Part 1) | `labyrinth-panelctl`, `labyrinth-notify-activate`, `labyrinth-mediakey`, the start-menu and clipboard launchers, the `systemsettings` wrapper, `labyrinth-panel-mode`, wallpapers and avatars in `/usr/share/labyrinth` | move into Yggdrasil (renamed) or drop; Labyrinth keeps only OS things |

Done on the owner's machine 2026-10-23: all twelve packages replaced under the new names; the panel, password box, idle watch and
portal running as `yggdrasil-*`; Super+S on the new panel; the desktop's files moved to `~/.config/yggdrasil` (old loose files are
in the owner's cleanup script).

## Order of work

1. **Part 2 first** (the rename and the integration hooks). It touches every package; doing it before the KDE work means the new
   pieces are written with the right names once, not renamed later. Mechanical, testable, reversible; old config paths migrate.
2. **Part 1a**, top to bottom, each switched on only after it works, each with a way back on the live machine (lock and login above all).
3. **Part 1b**: the compositor, then the parked tools on it.
4. **Part 1c**: the owner's choices about apps and the installer.

Each finished row is ticked here with its date, so this file always says exactly what is left.
