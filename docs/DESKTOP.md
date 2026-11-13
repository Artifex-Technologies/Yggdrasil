# Desktops (2026-10-11): two modes, chosen at login (history)

**Status 2026-11-04: this page is a record of how Labyrinth's desktop was put together on 2026-10-11, before Yggdrasil.**
KDE is gone: it was removed from the owner's PC on 2026-10-24 and from the code on 2026-10-26. Today there is one desktop, the
Yggdrasil session on Hyprland 0.57 (`packages/session`, `yggdrasil-session`), with Yggdrasil's own top bar, dock, lock screen
and login screen. Windows float by default; screens set to tile in Settings > Displays tile them Omarchy-style, which Hyprland
window rules now do instead of Krohnkite. Plasma, the KWin scripts, Krohnkite, Klassy, the WhiteSur themes and
`labyrinth-plasma-setup` named below are no longer used. The shortcuts in use today are in the main README.

As it was on 2026-10-11: Labyrinth shipped two desktops and you picked one at the login screen. Both share the same apps,
files, Minos, installer, snapshots and settings.

| | macOS-style (default) | Tiling (Omarchy-style) |
| --- | --- | --- |
| Session | KDE Plasma (Wayland); gone since 2026-10-24 | Hyprland |
| Look | WhiteSur (macOS Big Sur), the look of SmallSur: traffic-light-style buttons on the right in Windows order (minimize, maximize, close), Apple-style menu bar, floating dock | Dark green, blurred translucent windows, Waybar |
| Windows | Float; snap to edges; Meta + T opens KDE's tiler | Float by default too (macOS-like rules), SUPER + T tiles one |
| Menus | The focused app's File/Edit/View in the top bar (global menu) | None (apps keep their own) |
| Terminal | Ghostty (Win + Return): tabs in the title bar; F12 drop-down with tabs | foot (SUPER + Return) |
| Launcher | Tap Meta (Kickoff), or Alt + F1 | SUPER + Space (fuzzel) |
| Dock | Floating, always visible like the Windows taskbar: maximized and new windows stop above it | None (as in Omarchy) |
| Full screen | Meta + F (or F11 in the app) covers the dock and top bar; Meta + Space brings them up over the app, Esc puts them away | SUPER + F |
| Lock screen | Plasma's | hyprlock, the "Split" design from omarchy-lock-explorer |

Switch: `labyrinth-switch-mode macos|tiling` (also in the launcher as "Switch to Tiling Desktop" and in
the Hyprland power menu). It sets what the login screen starts on and logs you out.

## One screen macOS-style, one screen Omarchy-style (2026-10-11)

Inside the macOS-style desktop, one monitor could work like Omarchy while the others stayed
Windows/macOS-like (in the Yggdrasil session today the same split is made by Hyprland window rules, from
`~/.config/yggdrasil/tiling`). It was one Plasma session, so the clipboard, drag and drop and moving windows
between screens all work. On this machine the LG DualUp is that screen (`"tiling": true` in
`displays.json`; labyrinth-displays turned it on once per port; both removed 2026-11-11, nothing ran them any more). By hand: `labyrinth-tiling HDMI-A-1`,
`labyrinth-tiling off`, `labyrinth-tiling` (show).

| | Everyday screens | Tiling screen |
| --- | --- | --- |
| Windows | Float; new ones fit above the dock | Tiled by Krohnkite (`kwin-scripts-krohnkite`), 8 px gaps, no title bars |
| Bar | Top bar + dock | Slim bar: workspaces left, clock centre, no dock |
| Workspaces | One | Its own 1-9 (Plasma 6.7 per-screen virtual desktops) |
| Win + Arrows | Snap, maximize, restore/minimize | Move focus |
| Win + Shift + Arrows | Window to the other monitor | Swap tiles |
| Win + 1..9 / Shift | Dock app N | Workspace N / send window there |
| Win + W | Overview | Close window |
| Win + Space | Dock and top bar over full screen | Launcher (KRunner) |
| Win + T | Plasma's tile editor | Float or tile the window |
| Win + - / = (Shift) | Zoom out / in | Shrink / grow tile width (height) |
| Same everywhere | Win + Return terminal, Win + F full screen, Win + Shift + Alt + Left/Right window to the other monitor, Win + E/R/I/L/V | |

How: the KWin script `labyrinth-screen-keys` owns these keys and calls the Windows-style action or
the Krohnkite one depending on the screen of the focused window (the built-ins they replace are
unbound in `kglobalshortcutsrc` and called by name). `labyrinth-tiling` writes Krohnkite's settings
(`ignoreScreen` = every other connector), enables per-screen desktops and adds the slim bar.
Verified in a VM with two monitors (shaped like the Acer and the DualUp, at half size).

## Window title bars (2026-10-11)

Goal: like Vivaldi and Spotify on Windows, or GNOME's header bars, the window buttons share the top
row with the app's own controls, and there is no grey title bar or chin wasting space.

- **Apps that draw their own title bar** (client-side decorations): Minos (its top strip is the
  title bar: menu, theme, status, drag area and traffic lights; `packages/minos/src/chrome.rs`), GTK
  and libadwaita (GNOME) apps, Chromium/Electron apps with their native-frame option off (Vivaldi:
  Settings > Appearance > "Use native window decoration" off; VS Code and Obsidian default to their
  own frame), Firefox, Steam, Spotify. Plasma draws nothing around them.
- **Terminal:** Ghostty replaced Konsole and Yakuake (2026-10-11): it draws its own title bar with
  the tabs in it (`gtk-titlebar-style = tabs`), traffic-light buttons from
  `~/.config/ghostty/labyrinth.css`, and F12 opens its drop-down "quick terminal" (with tabs). Konsole
  could not put its toolbar beside the window buttons. A Labyrinth terminal of our own may replace
  Ghostty later.
- **KDE/Qt apps** (under KDE; Klassy is not installed now, and in the Hyprland session Qt programs draw their own title bar,
  see [COMPOSITOR.md](COMPOSITOR.md)) could not draw their own, so the title bar was Klassy (`klassy`, our PKGBUILD) set up
  by `labyrinth-plasma-setup` from `usr/share/labyrinth/klassy/klassyrc` (generated by
  `scripts/gen-klassyrc.py`): macOS traffic lights (exact colours, symbol on hover) on the right in
  Windows order, title bar in the app's own colour so it and the toolbar read as one header, no
  border or chin (BorderSize None) with invisible resize edges just outside the window, soft shadow.
  The app style is Kvantum's WhiteSurDark (dark toolbars to match).
- Verified in the VM: Konsole's title bar and toolbar are the same colour (#363636), no chin, resizing
  from the right, bottom and left edges works. WhiteSur's own aurorae decoration was dropped: it draws
  a grey border and a 12 px strip under every window and has no invisible resize edges.

How the macOS mode was put together (KDE era; none of this is used now):
- Packages: `plasma-desktop` etc. from Arch, and our `whitesur-*` packages (`packages/pkgbuilds/whitesur-*`,
  built from vinceliuice's WhiteSur projects, GPL-3, pinned to a commit with a checksum).
- First login: `labyrinth-plasma-setup` applies the WhiteSur-dark look and runs
  `usr/share/labyrinth/plasma/layout.js`, which builds the top bar (launcher, desktops, app menu,
  tray, clock, power) and the floating dock. Delete `~/.config/labyrinth/plasma-setup-done` to redo it.
- Monitors: `labyrinth-displays --kde` gives each new monitor its best mode (the largest resolution
  that runs at 60 Hz or more, at its highest refresh rate), once per monitor.
- Windows never sit under the dock: KWin reserves its space for maximized windows, and the KWin
  script `labyrinth-fit-windows` (`usr/share/kwin/scripts/`) shrinks a window that opens or grows
  taller than the free area. Full-screen windows are exempt. The Hyprland session lost that rule
  until 2026-11-08 (a window back from maximize took its old size, under the dock). Since then the
  keep-clear block in `packages/session/hyprland/yggdrasil.lua` (with `keep-clear.lua`) moves a
  floating window out from under the bar and the dock, and shortens it if it does not fit. It runs
  when the window opens, when it comes back from maximize or full screen, and when the bar or the
  dock appears on a screen. Dragging a window there by hand is left alone, and so are games and
  true full screen. Since 2026-11-13 no floating window opens bigger than its screen: on my laptop (a
  small screen at scale 1.5 or 2) VS Code's workspace-trust dialog opened larger than the screen
  and I had to drag it around to read it. A window (any program's, dialogs and pop-ups too) that
  opens bigger than the room the bar and the dock leave free is made as big as that room less the
  8-pixel gap and centred in it; one that fits keeps its size. Kept window places are fitted the same
  way (sizes kept before a scale change were too big), and a dialog is no longer kept as a place of
  its own: a window of the same process counts as the same program (VS Code's dialogs are class
  "code", its main window "com.microsoft.VSCode"). Tested in plain Lua
  (`packages/session/tests/keep-clear.lua`, `window-open.lua`).
- Verified in a VM: the desktop, the dock, the window buttons, the global menu in the top bar, the
  Meta-key launcher, the installer, maximize stopping at the dock, full screen and Meta + Alt + P
  (now also Meta + Space) raising the panels over it. Not yet verified: NVIDIA, the real monitors.

# Desktop (history: the Hyprland design notes)

The dock described below was removed from the tiling desktop on 2026-10-11.

Goal: a macOS-like desktop, left-side window buttons, a dock and blur like COSMIC's,
and a top bar like the Omacosy one (workspaces, Spotify, weather, network, volume,
clock, quick settings with power options).

## Decision: Hyprland as the desktop, COSMIC as a second session

| Wanted | COSMIC (cosmic-comp) | Hyprland + Waybar + dock |
| --- | --- | --- |
| Window blur | Merged to `master` 2026-07-09 ([PR #2179](https://github.com/pop-os/cosmic-comp/pull/2179)) but **opt-in per app** (`ext-background-effect-v1`); the PR says COSMIC's own apps do not use it yet and few others do. Release not stated. Reports of glitches on some AMD setups | Compositor blur on any translucent window, today |
| Buttons on the left | No setting found in a search (I could not confirm one exists) | GTK apps: `gtk-decoration-layout` (done, below). Others: needs the `hyprbars` plugin (not done) |
| Top bar with Spotify, weather | Applet list I found (cosmic-applets 1.0.3): app list, audio, battery, bluetooth, input sources, minimize, network, notifications, power, status area, tiling, time, workspaces. **No weather or media applet listed**; the audio applet has MPRIS support. Custom applets are possible in libcosmic (Rust) | Waybar has `mpris`, `network`, `bluetooth`, `pulseaudio`, `clock`, workspaces, and custom modules for the rest |
| Dock | Built in | `nwg-dock-hyprland`, styled after COSMIC's |

So the look you described is easiest and most complete on Hyprland. COSMIC stays
installed (`cosmic-session`) and appears as another session at the login screen, so
you keep the desktop you like and can compare. Making COSMIC itself match (a Rust
applet for weather and Spotify, blur once apps adopt it) is possible later.

**Omacosy** is macOS-only (a Swift bar, AeroSpace, Karabiner, launchd); none of its
code can run here. Its **layout** is what we copy; its bar items map to Waybar modules.

## What was built

All under `profile/airootfs/home/live/.config/` (copied into new users' `/etc/skel` by
`labyrinth-install-cleanup`) plus `/usr/local/bin`. This tiling desktop was removed on 2026-11-05 (Labyrinth OS commit
`bcafed8`): its configs, Waybar, mako, fuzzel, foot and the helper scripts below are gone, and Yggdrasil has its own bar,
notifications, launcher and lock screen. Only the GTK settings and the weather remain.

| Piece | File | State |
| --- | --- | --- |
| Session, keybindings (Omarchy's), NVIDIA env | `hypr/hyprland.lua`, `hypr/bindings/tiling.lua` | Removed 2026-11-05 (was adapted from Chymaera's, now LykanOS's, boot-tested Lua config) |
| Look: rounding 12, shadows, blur, gaps 6/12 | `hypr/looks/labyrinth.lua` | Removed 2026-11-05 |
| Top bar | `waybar/config.jsonc`, `style.css` | Removed 2026-11-05 (never rendered) |
| Dock | `nwg-dock-hyprland/style.css`, started by `labyrinth-shell` | Removed 2026-11-05 with `labyrinth-shell` (never rendered) |
| Spotify controls | bar modules + `playerctl -p spotify` | Removed 2026-11-05 with the bar (never run) |
| Weather (optional component) | `packages/yggdrasil-weather` (Rust) | **Tested**, and run live against the real services. See [WEATHER.md](WEATHER.md) |
| Power / quick settings | `labyrinth-quick-settings` (fuzzel menu) | Removed 2026-11-05 (it was tested with a stand-in menu and systemctl) |
| Left-side buttons (GTK apps) | `gtk-3.0/settings.ini`, `gtk-4.0/settings.ini` | Standard GTK setting; not seen on screen |
| Translucent terminal | `foot/foot.ini` | Removed 2026-11-05 |

Tests: Labyrinth OS's `tests/test_desktop_scripts.sh` now has 2 checks (the GTK buttons' order, and that none of the removed
desktop's configs is left); `tests/test_bar_tools.py` went with the bar on 2026-11-05.

## What we took from Omacosy

Omacosy ([MIT](https://github.com/paulsp94/omacosy)) is a macOS desktop, so these are its
*ideas*, rewritten for Linux; none of its code is used.

| Omacosy | Labyrinth | State |
| --- | --- | --- |
| Bar: workspaces, active app, Spotify, weather, network, volume, clock | Waybar modules, same order | Removed 2026-11-05 with Waybar (never rendered) |
| Floats pill (shown only when the workspace has floating windows) | `labyrinth-floats` | Removed 2026-11-11 (Waybar, which showed it, went on 2026-11-05; Yggdrasil's own panel replaced it) |
| Activity pill opens a floating btop | `custom/activity` + a float rule for `labyrinth-activity` | Removed 2026-11-05 with Waybar (never run) |
| `Super+K` keybinding cheat sheet | `labyrinth-keys` (reads `hyprctl binds -j`) | Removed 2026-11-05 (it was tested with a fake `hyprctl`) |
| Caps Lock as the Super key | Not enabled. In Hyprland it is one input option (`kb_options = "caps:super"`); add it to `hyprland.lua` if you want it | Not done |
| Workspace overview with live previews and search | Would need a Hyprland plugin (hyprexpo); its Lua configuration is unverified | Not done |
| Focus follows mouse, focus ring | `follow_mouse = 1` and a thin border already set | Done |
| Trackpad swipes, per-monitor workspace slots | Not applicable (desktop, one monitor assumed) | Skipped |
| Theme system (one 22-color palette driving everything) | Colors are hard-coded in the CSS/Lua today | Not done; the Omarchy theme system is the likely route |
| Bar popups and sliders (volume, brightness) | Clicks open pavucontrol and friends | Not done |
| Clean uninstall manifest | Not needed: Labyrinth is the OS, not an overlay | Skipped |

## Your two displays

Read from Windows on 2026-10-10 (read-only):

| | Display 1 (primary) | Display 2 |
| --- | --- | --- |
| Model (EDID) | Acer Predator X34 GS | LG DualUp (reported as "LG SDQHD") |
| Shape | 3440x1440 ultrawide, 144 Hz (Windows showed 143 Hz) | Tall 2560x2880 (8:9), 60 Hz |
| Windows scaling | 100% | 125% (shown as 2048x2304) |
| Position | 0,0 | Right of display 1, top edge 783 px above display 1's top |

Removed 2026-11-11 (nothing ran it any more; kept here as the record of the layout): `labyrinth-displays` turned that into Hyprland's documented
`hl.monitor({ output, mode, position, scale })` calls and saves them in
`~/.config/hypr/monitors.lua`, which `hyprland.lua` loads in a protected call. Rules in
`/usr/share/labyrinth/displays.json` match on the monitor's model, **not** its port name
(Hyprland's names such as `DP-1` depend on which port each cable is in, which is unknown
until the machine runs). So swapping ports does not break the layout. 125% of
2560x2880 divides to whole pixels (2048x2304), so no rounding. A monitor it does not
recognize is left to Hyprland (preferred mode, automatic position).

**Tested:** the planning logic, with fixtures shaped like `hyprctl monitors all -j`
(13 checks): your layout, either order, other ports, unknown monitors, missing modes,
write-once-and-reload. **Not tested, because it needs the real machine:**

- Whether Hyprland's real description/model strings contain "X34 GS" and "SDQHD". If not,
  nothing breaks (both fall back to automatic) but the layout is not applied; run
  `labyrinth-displays --list` and fix the `match` text.
- Whether Linux offers the DualUp's 2560x2880 mode. One retailer review says it needed
  fiddling; that is anecdotal. The rule asks for that exact mode and falls back to
  preferred. A forced mode may be needed.
- Whether the 144 Hz mode and G-Sync Compatible (variable refresh) work with the NVIDIA
  driver on Wayland. VRR is **left off**; I found no source either way.
- **Apps running through XWayland** (Steam, most Proton games) can look blurry on the
  125% display. Not addressed.
- The bar and dock appear on **both** monitors; pinning them to one is not set up.
- Which monitor games use when fullscreen, and workspace assignment per monitor.

## Optional software

Spotify, Obsidian, Steam, Proton Drive, Proton Pass and the rest are optional
*components*, not part of every install; see Labyrinth's [COMPONENTS.md](https://github.com/Artifex-Technologies/Labyrinth_OS/blob/main/docs/COMPONENTS.md).

## Quick settings and hibernate

The power button opens a menu: Settings, Sleep, Hibernate, Log out, Restart, Power
off. Log out, Restart and Power off ask to confirm. Settings opens Network, Sound,
Bluetooth, Displays, Gaming check.

**Hibernate is only listed when it can work**: the kernel supports it, a real (non-zram)
swap at least as big as RAM exists, and `resume=` is on the kernel command line. The
installer currently uses zram only, so today Hibernate does **not** appear. To get it,
the install needs a swap file or partition of about 64 GB (~6% of the 1 TB disk) plus
resume configuration. That is a decision for you; see the end of this file.

## Not done / unknown

- **Left buttons for non-GTK windows.** Hyprland draws no title bars. Qt, Electron
  and Firefox windows follow their own settings. The `hyprbars` plugin can draw
  macOS-style buttons on every window, but I do not know its Lua configuration syntax
  for Hyprland 0.57 and did not guess.
- **Dock is not COSMIC's dock.** It copies the look (translucent rounded shelf), not
  its behavior (applet-based, panel integration).
- **The bar has no popups** (no slider panel for volume or Wi-Fi); clicking opens the
  matching app (pavucontrol, nm-connection-editor, blueman). A Quickshell shell like
  Omarchy's would add them; it needs the Omarchy repo's packages, which LykanOS (formerly
  Chymaera OS) also has not adopted.
- **Battery** (2026-11-13): on a laptop, the battery sits beside the clock; its drop-down has the power mode and the charge
  limit, and Settings > Power the same (Settings' `battery` helper, a bar add-on). Brightness is in the volume and brightness pop-up.
- **Tablet mode and rotation** (2026-11-13, for a 2-in-1 such as the ThinkPad X1 Yoga): `/usr/lib/yggdrasil/tablet watch`
  (packages/laptop, in Settings' package, autostarted; it says "off" and stops on a computer without an accelerometer or a
  tablet-mode switch). Folding the screen back flips the switch; the session's binds (`switch:on:` / `switch:off:` in
  yggdrasil.lua) tell the watcher. Folded, the built-in screen (eDP) follows the accelerometer through iio-sensor-proxy, and its
  touchscreen and built-in pen are given the same turn so a touch lands under the finger; opened, it goes upright. The sensor
  is claimed only while it is followed. Hyprland loses the turn when it rereads its config, so the watcher turns it again on
  `configreloaded`. The on-screen keyboard (wvkbd from the AUR, Agora's "On-screen keyboard") shows while folded and no keyboard
  is plugged in by USB or Bluetooth (read from /sys, no input device is opened). Settings > Displays: Lock rotation, and the
  keyboard switch (~/.config/yggdrasil/tablet.conf). Not yet: Yggdrasil's own compositor cannot turn a screen (its
  output management refuses a transform), so there the watcher shows the keyboard only; the turn direction and the touch
  turn are to be checked on the laptop (Labyrinth OS docs/LAPTOP.md).
- **Fingerprint reader** (2026-11-13): fprintd and libfprint are an online component (Agora's "Fingerprint reader"; the
  installer ticks it when a reader libfprint supports is plugged in). Settings > Users > Fingerprint enrols a finger (fprintd
  asks for your password through the polkit agent) and removes them. **Fingerprint for sudo** is a switch there, off until you
  turn it on: `pkexec /usr/lib/yggdrasil/fingerprint sudo on` (org.yggdrasil.fingerprint.sudo, an administrator's password every
  time) puts a marker line and `auth sufficient pam_fprintd.so` at the top of /etc/pam.d/sudo; "sufficient", so no finger
  (Ctrl+C, the time running out, no reader) falls through to the password as before. `sudo off` takes exactly those two lines
  out. The plan for the rest, not done:
  1. **Lock screen:** not through PAM (with pam_fprintd in its stack, a typed password would first wait for a finger). The
     lock talks to fprintd itself: VerifyStart while the password field is shown, unlock on `verify-match`, the password still
     works at any moment; a switch "Unlock with fingerprint" beside the sudo one, off by default. After three failed fingers,
     only the password.
  2. **Password boxes (polkit):** our polkit agent runs polkit's helper over PAM's `polkit-1` stack; the same marked-line
     approach for `polkit-1`, a third switch, once the agent shows "Touch the reader" from PAM's text messages.
  3. **Login screen:** last, and maybe never: the disk's encryption needs the password at boot anyway, and the keyring is opened
     with the login password.
- **Weather privacy:** Open-Meteo and aviationweather.gov receive the coordinates of the
  place you set (never guessed from your IP). See WEATHER.md.
- **`minos` has no package yet**, so the `SUPER + SHIFT + F` binding will not
  open it on an installed system until we write one.
- **Package names**: all but the CachyOS/our-own ones were checked against Arch's live
  package database (84 of 97 found). Not found there and so unverified: the
  `linux-cachyos*` kernels, `proton-cachyos`, `protonup-qt` (likely AUR or a CachyOS
  repo), and our two local packages (`calamares`, `proton-pass`).
- Nothing here has run on a real GPU. Chymaera's (now LykanOS's) equivalent was boot-tested only with
  software rendering in a VM.

## Seeing the real desktop in a VM

The built ISO is booted in QEMU/KVM (WSLg cannot run Hyprland or Plasma's compositor):

    scripts/vm/run-iso.sh iso|disk|new      # boot the ISO, the installed disk, or a fresh disk
    python3 scripts/vm/qmp-shot.py out.png [--key meta_l-ret] [--click X Y]   # screenshots, no window
    python3 scripts/vm/serial.py "command"  # a root shell on the live system's serial console

Software rendering, so it is slow and says nothing about GPU behavior. Your two monitors cannot be
reproduced here.

Bugs this found that no static check could (all fixed):
- `foot.ini` used the section name `[colors]`; foot 1.28 renamed it `[colors-dark]`, so the terminal never
  became translucent.
- Opening the launcher changed the bar's "active app" label to `fuzzel`, widening the bar; on a narrow
  screen the right-hand end (including the power button) was pushed off the edge. The label now ignores
  fuzzel. The VM's window opened at 640x480, which made it obvious.
- fuzzel's default theme is bright cream; it now matches the desktop (`fuzzel/fuzzel.ini`, checked with
  `fuzzel --check-config`).
- No wallpaper meant nothing for the bar or blur to sit on; added a placeholder (`scripts/gen-wallpaper.py`).
- The explorer had no desktop entry, so the dock showed a broken icon.
- An empty capsule showed in the bar when no window was open.

Mouse use is the way to drive it in the VM window: Windows keeps the Super key for itself.
Verified working there by screenshot: bar, dock, tiling, rounded translucent windows, the launcher, the
power menu (no Hibernate entry, as designed), and the explorer and weather running inside it.
**Not verified:** actual blur strength (no GPU), `rounding_power`, your multi-monitor layout, VRR.

## Bar layout (updated) and the global menu

- **Left:** launcher, workspaces (only existing ones, max 9, a `+` while fewer than 9), then the
  focused app's name in bold (`hyprland/window`, with friendly names for common apps). With no
  window focused the name disappears and it is the plain bar.
- **Right:** Spotify controls, floats/activity, weather, network, Bluetooth, volume, clock, power.
- **Tabs in the title bar** is the browser's own client-side title bar, not something the desktop
  does: Chrome/Chromium/Brave on Linux do it unless "Use system title bar and borders" is on, and
  Firefox does it by default. They take their button side from `gtk-decoration-layout`
  (`close,minimize,maximize:`, already set), so the buttons sit on the left.
- **Global menu (File, Edit, View... in the bar) is NOT built.** SmallSur, which this was taken
  from, is an XFCE theme: XFCE's panel hosts `vala-panel-appmenu`, and Waybar has nothing like it.
  It needs a small service (`labyrinth-appmenu`) that implements the `com.canonical.AppMenu.Registrar`
  D-Bus name, follows the focused window through Hyprland's socket, reads that app's menu over
  `com.canonical.dbusmenu`, and draws it in the bar. Apps that publish a menu that way: Chromium
  family and Electron apps (VS Code, Obsidian), Qt apps with the appmenu platform theme, and GTK3
  apps with `appmenu-gtk-module`. GTK4, Firefox and Spotify do not, so for them the bar would show
  only the app name. Minos would get its own menu entries natively.
