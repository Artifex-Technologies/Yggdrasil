# Yggdrasil on its own: no KDE, no Labyrinth

The single checklist for making Yggdrasil a desktop environment of its own (audited 2026-10-23 from the running machine and both
repositories; it replaces the partial snapshot in [KDE_INVENTORY.md](KDE_INVENTORY.md)). Two goals:

1. **No KDE.** Every KDE piece is either rewritten in Rust as part of Yggdrasil, replaced by a small standard non-KDE component, or removed.
2. **No Labyrinth inside Yggdrasil.** Yggdrasil is the desktop; Labyrinth OS, Niflheim OS and any other system install it. Nothing in
   Yggdrasil may be named after, depend on, or assume one OS. OS-specific features plug in from the OS side.

Decisions this follows: Rust and lightweight (the "must be light" rule); secrets in gnome-keyring, not KWallet and not our own
(2026-10-16); replace KDE before new desktop features, the PowerToys-style tools parked on branch `tools-wip` (2026-10-23).

---

## Part 1: KDE, everything that is still there

Audit numbers: 114 KDE packages installed (44 in the `plasma` group, 62 KDE Frameworks, 4 KDE applications, 24 installed explicitly);
23 KDE packages in Labyrinth's ISO list; 29 source files in Yggdrasil call into KDE; 9 Labyrinth scripts and 12 config files in the
Labyrinth image configure KDE. **Yggdrasil's own packages depend on KDE libraries**: `yggdrasil-panel` and `yggdrasil-settings` need
`libkscreen` and `kconfig`.

### 1a. Can go before the compositor (each needs its Yggdrasil replacement finished and switched on first)

| KDE piece | What it does today | Replacement | State |
| --- | --- | --- | --- |
| `powerdevil` | dim, screen off, sleep, brightness, power profiles, battery | `yggdrasil-idle` (Rust) + power-profiles-daemon | idle runs **beside** PowerDevil: finish and switch PowerDevil off |
| `xdg-desktop-portal-kde` | screenshot, screen sharing, colour pick, settings, inhibit, notifications, app chooser | `yggdrasil-portal` (Rust) | file chooser done; the rest to do (screen capture goes through KWin's interface until the compositor) |
| `kwallet-pam`, `kwalletd6`, `ksecretd`, `signon-kwallet-extension` | secret storage | **oo7** (Rust Secret Service, MIT, in Arch extra; decided 2026-10-23 over gnome-keyring, on condition that it keeps secrets private: encrypted at rest with a key from the login password, unlocked by PAM, locked again when it should be; to be verified in its source before any secret moves) | `yggdrasil-keyring-sync` to be pointed at oo7; switch needs the owner (saved passwords move) |
| `kscreenlocker` | lock screen | `yggdrasil-lock` (Rust, ext-session-lock) | built; switch on with a rescue path |
| `sddm` | login screen | greetd + `yggdrasil-greeter` (Rust) | built, tested nested only; install with the old one as fallback |
| `plasma-nm` (secret agent in kded) | Wi-Fi password prompts | NetworkManager secret agent in Yggdrasil | to do |
| `bluedevil` | Bluetooth pairing prompts | BlueZ `Agent1` in Yggdrasil | to do |
| `plasma-pa` | volume applet (kded parts) | Yggdrasil sound (Settings + Fylgja already) | remove once nothing calls it |
| `kded6` modules: tray watcher, media keys, device/space/OOM notices, time zone, accent colour, others | small background services | tray host (built), media keys in the panel, notices through our notifications; the rest dropped | to do (one by one) |
| `plasma-workspace` (`plasmashell`, `startplasma`, `ksmserver`) | wallpaper, session start and logout, autostart | wallpaper layer in the panel; session = a systemd user target that starts the compositor and Yggdrasil | to do (session start with care: it is how you log in) |
| `kscreen`, `libkscreen` (+ `kscreen-doctor` in 4 Yggdrasil files) | display layout and scale | Settings > Displays speaking the output-management Wayland protocol itself; drops the `libkscreen` dependency | to do |
| `kconfig` (library dependency of panel and Settings) | reading KDE config files | plain file parsing; nothing of ours should read KDE config at all once KDE is gone | to do |
| `plasma-systemmonitor` | task manager (Ctrl+Shift+Esc) | a small Yggdrasil task manager | to do |
| `plasma-firewall` | firewall page | Settings > Firewall (exists) | remove |
| `spectacle` | screenshots | Yggdrasil screenshot through `yggdrasil-portal` | to do |
| `breeze`, `breeze-gtk`, `kde-gtk-config`, `klassy`, `kvantum` | KDE theme, GTK theme bridge, window decoration, Qt theming | `yggdrasil-theme` writes GTK (and Qt via a plain qt6ct/kvantum-free style) settings itself; cursor theme of our own or a non-KDE one | to do |
| `kdeplasma-addons`, `plasma6-applets-window-title`, `plasma-browser-integration`, `kio-extras`, `baloo`, `milou`, `kactivitymanagerd`, `kaccess`, `drkonqi`, `kinfocenter`, `systemsettings`, `kmenuedit` | Plasma extras | nothing (Minos has its own search; Settings is ours) | remove (kaccess: only after the accessibility plan) |
| Labyrinth image: `labyrinth-plasma-setup`, `labyrinth-hide-kde-settings`, `labyrinth-displays(-kde)`, `labyrinth-panel-mode`, `labyrinth-switch-mode`, `labyrinth-tiling`, `labyrinth-lockwatch`, `labyrinth-keyring-migrate`, `labyrinth-selfcheck`; `/etc/xdg/kwinrc`, `kscreenlockerrc`, `powerdevilrc`, `plasma-workspace`, `kde-portals.conf`, `sddm.conf.d`, `kwinrulesrc`, `/usr/share/kwin`, `/usr/share/labyrinth/plasma` | configure KDE | each goes with the piece it configures | follows the rows above |

### 1b. The compositor (decided 2026-10-23: chosen by measurement, the shell made independent of it)

Yggdrasil's shell (panel, dock, Fylgja, launcher, lock, notifications) is moved onto **standard Wayland protocols only**
(layer-shell, ext/wlr foreign-toplevel, wlr output management, ext-session-lock, ext-idle-notify, ext-workspace, screencopy through
the portal), so it runs on any compliant compositor and does not care which one an OS picks. The compositor is then chosen by a
**bake-off on real hardware** (first: RTX 2080 Ti, NVIDIA 615, 3440x1440 at 180 Hz, HDR-capable): KWin as the baseline, Hyprland,
cosmic-comp, and Yggdrasil's own Smithay compositor, measured on idle memory and CPU, frame pacing and input latency in real games
(MangoHud logs), VRR at 180 Hz, tearing for low latency, HDR, NVIDIA stability, Xwayland/Steam games, screen sharing. An upstream,
distro-packaged compositor that wins is preferred (it updates with the system); window tools (FancyZones, Grab And Move, Window
Hopper) then talk to it through one small adapter.

| KDE piece | What it does today | Replacement |
| --- | --- | --- |
| `kwin` (+ `kwin-scripts-krohnkite`, `/usr/share/kwin` scripts) | draws every window, input, the Wayland protocols the panel uses (`plasma-window-management`, `org_kde_plasma_shell`), tiling, rules | `yggdrasil-compositor` (Rust, Smithay): standard protocols (layer-shell, foreign-toplevel, output management, screencopy) |
| `kglobalaccel` | global shortcuts, including KWin's own | shortcuts inside the compositor |
| KWin D-Bus calls in Yggdrasil (`org.kde.KWin` in 15 files) | effects, reconfigure, screenshots, colour pick | the compositor's own interface |

Until then KWin and kglobalaccel are the **only** KDE pieces that should remain. The parked tools are written on the compositor.

### 1c. Not the desktop: applications and the installer

These use KDE's libraries (KDE Frameworks) without being the desktop. The libraries stay as long as any of them is installed.

| Package | Uses | Choice for the owner |
| --- | --- | --- |
| `kdenlive` (video editor), `ghostwriter` (Markdown) | KDE Frameworks | keep as apps (Agora), or pick non-KDE ones (e.g. Shotcut/Pitivi; Apostrophe) |
| `calamares` (Labyrinth's installer), `kpmcore` | KDE Frameworks | Labyrinth's choice: keep, or a Rust installer of its own |
| `kaccounts-integration`, `kirigami-addons`, `kquickimageeditor` | pulled in by the above | go with them |

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
| Calls into Labyrinth OS (progress: ✅ 2026-10-23 the hook itself, docs/INTEGRATIONS.md; ✅ assistant: panel, Fylgja, compositor key and a Settings page of declared controls; ✅ packages: Agora's catalog, components and own builds, its root helpers now Agora's own; ✅ branding: accent and wallpaper; ✅ security: alerts, Settings page, quarantine, scan, distress password, lock screen; still to do: cloud, app network, settings-page) | `labyrinth-apps` (Agora installs), `labyrinth-launch`, `labyrinth-panel-mode`, `labyrinth-quarantine`, `labyrinth-sarina`, `labyrinth-sarina-speak`, Sarina/Heimdall/Limbo pages in panel, Settings, Minos, compositor | **integration points**: Yggdrasil defines a small, documented hook (a folder of integration files, e.g. `/usr/share/yggdrasil/integrations/*.toml`: a Fylgja section, a Settings page, a shortcut, a Minos menu item, an Agora installer backend). Labyrinth ships its Sarina, Heimdall, Limbo and package-installer integrations there; another OS ships its own or none. Yggdrasil never names them. |
| Proton Drive in Minos | `LABYRINTH_PROTON_DRIVE`, rclone glue | a generic "cloud drives" hook, Proton Drive provided by the OS (or kept as a neutral rclone feature) |
| ✅ 2026-10-23 Branding assets | Labyrinth logos or colours inside Yggdrasil packages | Yggdrasil's own; the OS supplies its logo and default accent through the integration folder |

---

| Desktop pieces living in Labyrinth's repository (found 2026-10-23) | `labyrinth-panelctl`, `labyrinth-notify-activate`, `labyrinth-mediakey`, the start-menu and clipboard launchers, the `systemsettings` wrapper, `labyrinth-panel-mode`, wallpapers and avatars in `/usr/share/labyrinth` | move into Yggdrasil (renamed) or drop; Labyrinth keeps only OS things |

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
