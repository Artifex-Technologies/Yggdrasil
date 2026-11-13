# Settings > Apps, Developer options, and the per-app controls

Built 2026-10-15/16. Code: `packages/settings/src/appmgr.rs` (what is known about each app, and changes to it),
`apps_ui.rs` (the pages), the power-limit helper `packages/power-helpers/src/powerlimit.rs` (installed as `/usr/lib/yggdrasil/powerlimit`; the Python one it replaced is in `helpers/python/`;
`tests/test_settings_powerlimit.py`), and, for the per-app network rules and sandbox, the operating system's launcher declared in its
`apps` integration (INTEGRATIONS.md; on Labyrinth `labyrinth-launch` and `labyrinth-appnet`, tested in Labyrinth's repository).
`cargo test` in `packages/settings`.

## The layout (2026-11-13)

I reorganised Settings on the model of a tablet's settings (its structure, not its look; the colours stay Yggdrasil's):
the sidebar holds grouped rows with the open page as a filled pill in the accent colour, and each page is rounded cards of
rows, a row that opens a page ending in ›, and a small grey note under a card where it needs one.

- **The way back** (`src/nav.rs`): one header above every page, outside the scrolling part, shows the path ("Appearance ›
  Wallpaper") and, on a sub-page, Back. Back, the mouse's back button and Alt+Left go up one level at a time; a click on the
  open page's sidebar row returns it to its first level. Each level keeps its own scroll position; a level just opened starts at
  its top. Before this a sub-page's own back button scrolled away with the page (the new level kept the old one's scroll
  offset), so the way back was often out of sight.
- **General** is the page Settings opens on: About, Software Update (opens Agora on its Updates page; the system's version
  beside it), Storage (each disk's use, read when the page opens), Date & Time, Keyboard (Peripherals > Keyboard) and
  Language & Region (`localectl`: the system's language for every account, and a separate format region when there is one).
  About and Date & Time moved here from the sidebar; the search still finds them, and `yggdrasil-settings about`,
  `datetime`, `storage` and `language` open them inside General. A page inside another is marked by `parent()` in main.rs.

- **Battery** (`src/battery.rs`; in the sidebar only on a computer with a battery of its own, a power_supply of type
  Battery that is not a device's): the charge in large type with "Last charged to N%" and a bar (a line at the charge
  limit), the last ten days as bars of the battery used each day while unplugged, App & System activity, Battery Health
  (maximum capacity, charge cycles) and the charge limit, which moved here from Power (Power keeps a Battery row that opens
  it, and the power mode). Nothing runs in the background for it: UPower already records the charge (its files in
  /var/lib/upower are readable by root only, so the same samples are asked for over D-Bus with GetHistory), and an app's
  share is the processor time of the systemd scopes the launcher starts it in (lapp_*.scope, app-*.scope, matched to apps by
  yggdrasil-appusage), against the rest of the session. Both are read when the page opens and every 30 seconds while it is
  open. The apps' share counts the apps open now; apps already closed would need a small record kept in the background,
  which I left out.

- **Bluetooth** (`src/bluetooth_ui.rs`), laid out like Wi-Fi: the switch; My Devices (paired, each Connected or Not
  Connected; a tap connects, the (i) opens its page with its kind, battery, address, Connect/Disconnect and Forget This
  Device, asked twice); Other Devices, found while the page is open (a tap pairs, trusts and connects; a code to compare or
  type comes up in yggdrasil-polkit, BlueZ's agent); then the game controllers as before. The search is a BlueZ discovery
  session on a D-Bus connection of the page's own, read every two seconds and dropped when the page is left (BlueZ ends a
  client's discovery when its connection closes, so it stops even if Settings is killed). The "Searching" mark stands still:
  a turning one redrew the whole window several times a second (15 % of a core with software drawing).

- **VPN** (`src/vpn.rs`, under Connections): NetworkManager's VPN and WireGuard connections with a switch each (`nmcli
  connection up/down`) and their state; Tailscale when it is installed, its state and a switch (`tailscale up` / `down`,
  nothing else of it touched); and Add VPN Configuration: WireGuard from a pasted or chosen .conf (read in Settings and handed
  to NetworkManager with AddConnection over D-Bus, the private key inside the message, never in a file of ours), OpenVPN
  from an .ovpn file (`nmcli connection import`; needs the networkmanager-openvpn package, the page says so when it is
  missing), IKEv2 with a user name (`nmcli connection add type vpn vpn-type strongswan`; the password is asked by
  NetworkManager's agent when it first connects; needs networkmanager-strongswan). The security monitor owns the network's
  security policy; this page is only the user interface. `yggdrasil-settings vpn:add` opens Add VPN Configuration.

## The pages

- **The list** (2026-11-13, as on a tablet): a card at the top with Default Apps ("Manage default apps", no longer in the
  sidebar; `yggdrasil-settings default` still opens it, inside Apps), Optional software and Minos (they were tabs), then a
  search field and every visible desktop entry A to Z, a card per letter. It can also be sorted by Usage (how often it was
  opened from here) or Size (its folders under ~/.config, ~/.cache, ~/.local/share, ~/.var/app). System tools (settings panels, Avahi, ...) are hidden
  unless "Show system tools" is ticked. Each row has icon-only buttons: restart, close (both only while it runs), remove
  (asks first; shows the package; the desktop's own packages are refused).
- **An app's page** (click a row, or `yggdrasil-settings apps:ID`): Network (switch; click the name too), open at login,
  status, storage with Clear cache and Clear data (the latter lists the folders and asks for the password), the privacy
  sandbox, Remove.
- **Optional software** and **Minos** are rows on the card at the top, each its own page.
- **Developer options** (About: click the logo five times while holding the ` key): adds "Any connection / Ethernet only /
  Wi-Fi only" next to an app's network switch, and the selected power mode's CPU and graphics-card limits under Power. Stored as
  `developer = yes` in ~/.config/yggdrasil/panel.conf.

- **Games** (`yggdrasil-settings games`, 2026-11-05): the graphics cards a game can draw on, fixes for particular games, and frame generation.
  - Every card, with what it does: which one shows the screen, which one the computer started on, its driver. Read by
    `yggdrasil-gpu-run list --json` (packages/settings-helpers, Rust, installed as /usr/bin/yggdrasil-gpu-run), the same code that later starts
    the game, so the numbers on the page are the numbers it takes.
  - With two or more cards each has **Copy launch option**: `yggdrasil-gpu-run 2 %command%` in a game's Steam launch options runs
    it on card 2. For an NVIDIA card (its own driver) that is PRIME render offload and `__VK_LAYER_NV_optimus=NVIDIA_only`; for
    AMD or Intel (Mesa) `DRI_PRIME=pci-...` and `MESA_VK_DEVICE_SELECT`, hiding an NVIDIA card with `non_NVIDIA_only`; for every
    card `DXVK_FILTER_DEVICE_NAME` and `VKD3D_FILTER_DEVICE_NAME` (Proton's DirectX translation) when vulkaninfo names it. The
    picture still reaches the screen through the card the monitor is plugged into (the drivers copy it across). Tested against
    a pretend two-card /sys (an RTX 2070 with an Arc B580); not yet on a real second card.
  - **Game fixes** (2026-11-05): per-game fixes for installed Steam games, each with Apply / Undo. `helpers/game-fix`
    (/usr/lib/yggdrasil/game-fix) applies the catalogue `data/game-fixes.json` (/usr/share/yggdrasil/game-fixes.json): per game,
    settings files ("Key = value" or ini) and the values to set, which can be the screen's size ({width}, {height}: the
    largest monitor, from the compositor through yggdrasil-wm, or xrandr). A file is backed up before its first change (FILE.yggdrasil-backup), or marked
    as made (FILE.yggdrasil-created, from a template in the game's folder or lines in the catalogue); Undo restores it. A game
    Proton has not started yet shows "Start the game once first". No administrator rights. First fixes: **Dead Space 2**
    (its menu never offers 21:9; settings.txt at the screen's size, V-Sync off: true 21:9 tried at 3440x1440) and
    **Fallout 4** (the settings files its launcher would make, from its templates, at the screen's size, borderless; the
    world fills 21:9 (confirmed by the owner in play), the HUD stays 16:9 and stretched; and a launch option that starts the game without Bethesda's launcher, which kept the keyboard
    once the game was up: tried by the owner 2026-11-05, keys work). A fix's `launch` is offered with Copy launch option (with
    `kairos` in front when frame generation is installed); Steam keeps launch options to itself, so it is copied,
    not written. Tests: `packages/settings-helpers/src/gamefix/tests.rs`. Patches to games' code come per game.
  - **Frame generation (preview)**, Kairos, Labyrinth's own (`labyrinth-framegen` until 2026-11-07; from Agora): every installed Steam game (from the
    appmanifests in all Steam libraries; Proton and Steam's runtimes left out) with Off / 2x / 3x / 4x, saved at once to
    `~/.config/kairos/games.conf` (`<app id> <off|2|3|4>`; the old `~/.config/labyrinth-framegen/games.conf` is read until the first
    change). The game needs `kairos %command%` (the old `labyrinth-framegen %command%` still works) in its
    Steam launch options once (Copy launch option); the launcher reads the choice when the game starts.
  - **Lossless Scaling frame generation (lsfg-vk):** whether lsfg-vk is installed (its Vulkan layer; Get it in Agora otherwise, where
    Labyrinth's catalog has it as "Frame generation", from the AUR's source recipe), whether Lossless Scaling is installed in
    any Steam library (it supplies the frame generation; Open in Steam otherwise), and "Choose games", which opens lsfg-vk's own
    window. lsfg-vk is not part of the desktop: its licence (CC BY-NC-ND 4.0) forbids changing it and commercial use.
  - **On a second card (experimental):** Lossless Scaling's dual-card mode (the game on one card, the extra frames on another) is
    not in lsfg-vk ("will be available in future releases"), and its licence forbids a changed copy. So it is built from
    unchanged parts: `yggdrasil-gpu-run N --frames-on M [--multiplier X] %command%` runs the game on card N inside gamescope
    (SDL backend: it presents through a Vulkan swapchain) on card M, switches lsfg-vk on for gamescope alone (`LSFGVK_ENV=1`,
    `LSFGVK_MULTIPLIER`) and off in the game (`DISABLE_LSFGVK=1`, gamescope's card variables taken away). The extra frames are
    made on card M from gamescope's output. With two cards the page offers it with Copy launch option (once lsfg-vk and
    Lossless Scaling are there): the frames on the card the monitor is plugged into, the game on the other.
    Checked on one card (2026-11-05): gamescope's SDL output loads implicit Vulkan layers (Mesa's device-select was inserted)
    and presents through vkCreateSwapchainKHR / vkQueuePresentKHR, where lsfg-vk works. (MangoHud cannot show this: it skips
    any process named gamescope.) Not yet run with lsfg-vk and Lossless Scaling, nor on two real cards. Costs a little input
    latency, as on Windows.

  ![Settings > Games](../assets/screenshots/settings-games.png)

- **Peripherals** (`yggdrasil-settings peripherals`, `mouse`, `keyboard`; 2026-11-12): the mouse and the keyboard, each on its own
  page, with the lights, DPI and polling rate of the devices that have them. It took the old Keyboard page's place in the
  sidebar; `yggdrasil-settings keyboard:shortcuts` and `shortcuts` still open the shortcut list.
  - **Mouse:** pointer speed, mouse acceleration (adaptive, or flat as most games want), scroll speed, natural scrolling and
    which side the main button is on, for every mouse, and under "Each mouse" for one mouse alone (its switch gives it settings
    of its own, starting from everyone's). Find my mouse moved here from Keyboard.
  - **Keyboard:** repeat delay and rate with a box to try them, the layout and its variant (the system's, for every account and
    the sign-in screen: `localectl set-x11-keymap`, which asks for the password, then the running session at once), and
    "Keyboard and mouse lights off while dimmed", moved here from Power (the same `lights_off` in
    `~/.config/yggdrasil/idle.conf`). Below them Shortcuts, Spelling and languages, and the Color Picker, as before.
  - **Where the settings go:** `~/.config/yggdrasil/input.conf` (its form is in `packages/wm/src/input.rs`), which the session's
    `yggdrasil.lua` reads at every start and reload, so they survive restarts; and the running compositor through yggdrasil-wm
    (`Desk::apply_input`): on Hyprland its Lua (`hl.config({ input = ... })`) at once, and a reload when a mouse's own settings
    change (Hyprland keeps `hl.device` rules until its config is read again). A mouse's settings are given to its other parts
    too (Hyprland lists a mouse's extra buttons as NAME-1, NAME-2, ...). **Not on Yggdrasil's own compositor yet:** there the
    file is kept and the page says so; the compositor is meant to read the same input.conf on start and `reload` (an item for
    its gap list, with libinput on the hardware backend).
  - **Razer** (when OpenRazer is installed): each device from OpenRazer's daemon on the session bus (`org.razer`), what it
    offers read from its introspection: the battery, a mouse's DPI (in steps of 50, up to its own maximum) and polling rate, and
    every lighting zone (the whole device, logo, scroll wheel, backlight, left and right) with its brightness, its effect (off,
    one colour, spectrum, breathing, wave: only those the device has) and its colour. Keyboards and keypads are on the Keyboard
    page, mice and mouse mats on the Mouse page, anything else on Peripherals itself. Every call has a 2-second limit and runs
    off the window's thread; a colour is sent once it has stayed put for 0.4 s; the daemon is never started by asking (a row
    offers "Start it" when it is installed but not running). `src/razer.rs`.
  - **Other makers:** a row that opens Polychromatic, Solaar (Logitech), ckb-next (Corsair), OpenRGB or Piper when installed;
    with none of them and no OpenRazer, one line points to Agora > Devices.
  - **Tests:** the OpenRazer client against a made-up daemon on a peer-to-peer D-Bus socket pair (devices, zones and effects
    from introspection, every change reaching the right method, a daemon that hangs giving up at the time limit); the
    input.conf form, its writer (temporary folders only) and the Hyprland requests (yggdrasil-wm, against its stand-in
    socket); the session's reader in plain Lua (`packages/session/tests/input-conf.lua`). The screenshots are made-up devices
    (`tests/fixtures/peripherals.json`) on Yggdrasil's headless compositor.
  - **Cost** (release build on the headless compositor, Mesa's software renderer, three runs each, before and after): the
    window shows after 0.154-0.174 s and 0.164 s; idle 0.9-1.1 % and 0.9-1.0 % of a core on the Keyboard page; 148-149 MB and
    151 MB resident (115-116 MB and 117-118 MB proportional), the extra being the layout list and the bigger page. The program
    is 0.6 MB larger. No new libraries (zbus was there for Wi-Fi).

  ![Settings > Peripherals > Mouse](../assets/screenshots/settings-peripherals-mouse.png)
  ![Settings > Peripherals > Mouse, a Razer mouse](../assets/screenshots/settings-peripherals-razer.png)
  ![Settings > Peripherals > Keyboard](../assets/screenshots/settings-peripherals-keyboard.png)

## Every page

Each page of the sidebar on a made-up computer (`scripts/screenshots.sh`, the README's "Making them again"; the fixtures are in
`packages/settings/tests/fixtures`: `system.json` for a desktop, `laptop.json` for a laptop). Developer options, shown under
About only when switched on, have no picture.

| Page | |
| --- | --- |
| **General**: the page Settings opens on. | ![Settings > General](../assets/screenshots/settings-general.png) |
| **General > Storage**: each disk's use (`system.json`'s made-up disks). | ![Settings > General > Storage](../assets/screenshots/settings-storage.png) |
| **General > Language & Region**: the system's language, a separate region for dates and times. | ![Settings > General > Language & Region](../assets/screenshots/settings-language.png) |
| **Wi-Fi & Network**: the network in use, Known Networks and Other Networks (`wifi.json`). | ![Settings > Wi-Fi & Network](../assets/screenshots/settings-network.png) |
| **Bluetooth**: My Devices (Connected / Not Connected, an (i) each), Other Devices found nearby, a game controller. | ![Settings > Bluetooth](../assets/screenshots/settings-bluetooth.png) |
| **VPN**: a VPN and a WireGuard connection, Tailscale. | ![Settings > VPN](../assets/screenshots/settings-vpn.png) |
| **VPN > Add VPN Configuration**: WireGuard from a file or pasted. | ![Settings > VPN > Add VPN Configuration](../assets/screenshots/settings-vpn-add.png) |
| **Displays**: three displays attached, one disconnected; the arrangement box with the main display (1) selected, its settings below. | ![Settings > Displays](../assets/screenshots/settings-displays.png) |
| **Displays, a disconnected display selected**: grey and smaller in the box, Connect among its settings. | ![Settings > Displays, a disconnected display](../assets/screenshots/settings-displays-connect.png) |
| **Displays, one display** (`displays-single.json`): its settings at once, no box, no Duplicate or Disconnect. | ![Settings > Displays, one display](../assets/screenshots/settings-displays-single.png) |
| **Sound**: outputs and inputs, each with its volume. | ![Settings > Sound](../assets/screenshots/settings-sound.png) |
| **Peripherals > Mouse**: every mouse, or one by one (`peripherals.json`). | ![Settings > Peripherals > Mouse](../assets/screenshots/settings-peripherals-mouse.png) |
| **Peripherals > Mouse, a Razer mouse**: OpenRazer's battery, DPI, polling rate and lights. | ![Settings > Peripherals > Mouse, a Razer mouse](../assets/screenshots/settings-peripherals-razer.png) |
| **Peripherals > Keyboard**: key repeat, the layout, the lights while dimmed. | ![Settings > Peripherals > Keyboard](../assets/screenshots/settings-peripherals-keyboard.png) |
| **Printers**: two printers, their ink and toner. | ![Settings > Printers](../assets/screenshots/settings-printers.png) |
| **Power, a desktop**: the power mode, Cooling and the idle timers (no battery). | ![Settings > Power, a desktop](../assets/screenshots/settings-power.png) |
| **Power, a laptop**: a row to Battery, the fan the firmware runs (`laptop.json`). | ![Settings > Power, a laptop](../assets/screenshots/settings-power-laptop.png) |
| **Battery** (`laptop.json`): the charge, the last ten days, the apps' share, health and the charge limit. | ![Settings > Battery](../assets/screenshots/settings-battery.png) |
| **Power > Cooling**: a fan controller's pump and fans, each with its speed now. | ![Settings > Power > Cooling](../assets/screenshots/settings-cooling.png) |
| **Power > Cooling, a fan's curves**: the processor's curve, dragged point by point. | ![Settings > Power > Cooling, a fan's curves](../assets/screenshots/settings-cooling-curves.png) |
| **Appearance**: light or dark, the accent, wallpapers, icons. | ![Settings > Appearance](../assets/screenshots/settings-appearance.png) |
| **Appearance > Lock screen**: the designs with their previews. | ![Settings > Appearance > Lock screen](../assets/screenshots/settings-lock-screen-designs.png) |
| **Sarina**: the operating system's assistant, from its integration. | ![Settings > Sarina](../assets/screenshots/settings-assistant.png) |
| **Apps**: Default Apps, Optional software and Minos on a card at the top, then every installed app A to Z under its letter, with a search. | ![Settings > Apps](../assets/screenshots/settings-apps-list.png) |
| **Apps > Default Apps**: what opens each kind of file. | ![Settings > Apps > Default Apps](../assets/screenshots/settings-default-apps.png) |
| **Games**: graphics cards, game fixes, frame generation per game. | ![Settings > Games](../assets/screenshots/settings-games.png) |
| **Security**: the security monitor, the password logs, the distress password, Limbo. | ![Settings > Security](../assets/screenshots/settings-security.png) |
| **Security > Distress password**: it opens only after your account password. | ![Settings > Security > Distress password](../assets/screenshots/settings-distress-open.png) |
| **Security > Distress password, the settings**: the password, and how many times within how long. | ![Settings > Security > Distress password, the settings](../assets/screenshots/settings-distress.png) |
| **Security > Distress password, the two sections**: Section 1, and Section 2 that asks for your password again. | ![Settings > Security > Distress password, the two sections](../assets/screenshots/settings-distress-sections.png) |
| **Security > Distress password, saving Section 2**: the warning, and your password once more. | ![Settings > Security > Distress password, saving Section 2](../assets/screenshots/settings-distress-warning.png) |
| **Firewall**: on or off. | ![Settings > Firewall](../assets/screenshots/settings-firewall.png) |
| **Users**: two accounts, the keyring. | ![Settings > Users](../assets/screenshots/settings-users.png) |
| **Restore points**: Snapper's restore points, and how to go back to one. | ![Settings > Restore points](../assets/screenshots/settings-restore.png) |
| **Date & Time**: the time zone, set automatically or by hand: click your part of the map, or search the short list (about 80 friendly zones by UTC offset). | ![Settings > Date & Time](../assets/screenshots/settings-date-time.png) |
| **Date & Time, light style**: the chosen zone in the highlight colour, orange unless the OS's branding names another. | ![Settings > Date & Time, light](../assets/screenshots/settings-date-time-light.png) |
| **About**: the operating system, the computer and its parts. | ![Settings > About](../assets/screenshots/settings-about.png) |

## How it works

- **labyrinth-launch ID** starts an app inside its own systemd scope and slice (`lapp_<id>.slice`; no dash in the name, so it
  sits directly under the user manager). The panel (dock, start menu) and Settings use it. It counts launches
  (~/.local/share/labyrinth/app-usage.json) and applies the app's sandbox settings
  (~/.config/labyrinth/apps/ID.json).
- **Network** (labyrinth-appnet, run with pkexec): one nftables table `inet labyrinth_apps` with a rule per limited app
  matching its slice (`socket cgroupv2 level 4 "user.slice/user-UID.slice/user@UID.service/lapp_ID.slice"`). Modes: block
  (also refuses lookups to the local DNS resolver), Ethernet only (`en*`, `eth*`), Wi-Fi only (`wl*`). Rules live in
  /etc/labyrinth/app-network.json. nft needs the cgroup to exist when it loads a rule, so
  `labyrinth-appnet-prepare.service` (user) starts the slices at login and asks `labyrinth-appnet.service` (system) to load
  the rules; a polkit rule lets an active local user start only that service. Verified on real traffic: block, Ethernet-only
  (this PC is on Wi-Fi, so blocked) and Wi-Fi-only each behaved, and an app outside the slice was unaffected.
- **Privacy sandbox** (experimental): bubblewrap, set per app. "Chosen folders" gives the app a tmpfs home with only those
  folders (and its own config/cache/data folders) bound in; "cannot see other programs" gives it its own PID namespace.
  Verified with the calculator: its home showed only the shared folder, and only two processes were visible.
  Also started, with the sandbox on and chosen folders empty: Ghostty (GTK) and Vivaldi (Chromium): both came up and Vivaldi
  opened a window (not driven further). Because the app's own folders are shared, Vivaldi used its normal profile.
- **Power limits** (`/usr/lib/yggdrasil/powerlimit`): /etc/yggdrasil/power-limits.json (read from /etc/labyrinth/ until the first
  change); the root service `yggdrasil-powerlimit.service` (the operating system switches it on) applies the active mode's limits on every mode change (CPU: `scaling_max_freq` as a percent
  of `cpuinfo_max_freq`; GPU: `nvidia-smi -pl`). Verified on the RTX 2080 Ti and the Intel pstate CPU; restored to defaults.
  On the Power page they sit in the Power mode group, under the mode selector, for the mode that is selected: Processor, then
  Graphics card (each with a reset icon left of its slider); choosing another mode shows and changes that mode's limits (my layout, 2026-10-27).
  While the graphics card unlock is armed or measured, `/var/lib/yggdrasil/gpu-unlock/power-hold` exists and no mode goes above
  the card's default watts (the modes' own limits stay saved and come back when the hold ends); `powerlimit get` reports it as
  `gpu.hold` and the slider stops there. Rust since pkgrel 36 (ported 2026-11-09 with its tests and dry-run parity,
  below); `powerlimit watch` is now a part of yggdrasil-power.
- **Graphics card page** (built 2026-10-27, my design; `src/gpu.rs`): the Graphics card row's title opens it. An
  Afterburner-style panel for an NVIDIA card: a live readout (temperature, power against the limit, core and memory clocks,
  load, video memory, fans, and what is holding the clocks back) with the last two minutes as small graphs, each on its own
  fixed scale, the value at a moment on hover; the selected power mode's power limit; and the fans (`/usr/lib/yggdrasil/gpufan`):
  the card's own control, one speed, or a curve edited by dragging its points. The readout is one `nvidia-smi -lms 1000` that
  runs only while the panel is on screen. Nothing on the page overclocks.
  - **Pop-out:** the icon next to the page's title opens the same panel in its own window, `yggdrasil-settings --gpu-window`
    (app id `yggdrasil-gpu`), started as a process of its own, so it stays open after Settings is closed. Only one at a time:
    a second start brings the first forward.
  - **Fans:** `gpufan set` (pkexec, `auth_admin_keep`) saves /etc/yggdrasil/gpu-fan.json and starts or stops the root
    service `yggdrasil-gpufan`, which drives the fans through NVML (no X server needed). Never below the card's lowest
    speed; at 85 °C or more, full speed. A speed comes down only once the card is 3 °C cooler. If the temperature cannot be
    read, the fans go back to the card. Stopping the service, or a crash (`ExecStopPost`), gives the fans back to the card;
    checked on the RTX 2080 Ti with a kill -9. Rust since 2026-11-09 (below). Tests: `packages/power-helpers/src/gpufan/tests.rs`.
  - **Fixtures:** debug builds read `$YGGDRASIL_GPUFAN_FIXTURE` for screenshots of each mode.
- **Cooling: case fans and pumps** (helper and service 2026-11-04; Settings > Power > Cooling 2026-11-05, `src/fans.rs`): `casefan` drives a Corsair
  Commander Pro (kernel driver `corsair_cpro`) as the root service `yggdrasil-casefan`.
  - **The page** (Power > Cooling, or `yggdrasil-settings power:cooling`; `power:fans` also works): the processor's and the card's temperatures,
    whether fan control is on, and for each port its speed now (percent and RPM, or "no speed signal").
    - Each card is headed by its port. Its first line is a text box with the person's own name for what is plugged in
      (up to 60 characters; "Graphics card pump", "Top exhaust"), with the speed now on the right. Kind makes the port a
      pump or a fan, because no two computers are wired alike.
    - A pump gets one slider, 60-100 %.
    - A fan gets Quiet, Balanced (casefan's default) or Cool, or Custom: its own processor and graphics card curves, dragged
      in the same editor as the graphics card's fan curve (`src/curve.rs`, shared).
    - A fan with no speed signal is never set below 35 %, because it cannot be seen stopping.
    - **Follows:** a fan can run on another fan's curves, so one setting drives both (`"follow": "5"` in the setting; casefan
      copies the leader's curves in when it loads it). Both run at the same percent, each at its own share of its top speed.
      Only a fan that follows nobody can be followed (no chains, no loops), and a pump never follows. A fan with no speed
      signal following curves that go below 35 % gets a warning.
    - Connected ports the setting leaves alone are listed, with "Control it as a fan".
    - Apply sends the whole setting to `pkexec casefan set -` on stdin (policy `org.yggdrasil.casefan.set`), which cleans it,
      saves it and restarts the service. The page reads `casefan get` every 3 s only while it is open.

    ![Settings > Power > Cooling, a fan's curves](../assets/screenshots/settings-cooling-curves.png)

  - Each connected port has a role: a pump keeps one speed, never below 60 %; a fan follows two curves, one for the processor's
    package temperature and one for the graphics card's, and runs at whichever asks for more.
  - At 92 °C on the processor or 83 °C on the card every port runs at 100 %. A temperature it cannot read puts every port at
    100 % and ends the service, which systemd starts again.
  - After a clean stop, fans go to 60 % and the pump to 100 %; after a crash (`SERVICE_RESULT` not success), everything goes to
    100 %. The Commander Pro keeps the last speed it was given until it loses power; it does not go back to its own stored
    curves by itself.
  - Setting: `/etc/yggdrasil/case-fans.json`. Rust since 2026-11-09 (below). Tests: `packages/power-helpers/src/casefan/tests.rs`.
  - On the owner's PC: port 1 is the graphics card's pump (about 3,830 RPM whatever it is set to), port 6 the 3,000 RPM Noctua
    exhaust, and port 5 the 80mm Noctua over the cables and RAM, which reports no speed (its own curves, never below 35 %).
- **Security > Limbo** (2026-11-05, `src/limbo_ui.rs`; `yggdrasil-settings security:limbo`): Limbo's sandboxes, written against
  Limbo's protocol (Limbo `docs/SETTINGS_PAGE.md`, "Protocol") through the `limbo` command, so the page never speaks HTTP.
  - Shown without a password (`limbo settings show --json` over `user.sock`, for members of `limbo-users`): Limbo's version,
    whether KVM is available, the space used (base images, the guests' own changes, evidence), each guest (system, running,
    stopped or held, processors, memory, network, its own changes, owner), the runs and hand-started guests, and the Windows
    licenses (masked).
  - **Ask for my password for Limbo changes:** the switch (`pkexec limbo settings password on|off`).
  - **Start, Stop, Free space** on the person's own guests: run as the person; when Limbo answers `password_required` the
    same command runs again through pkexec. A held guest has no buttons; its row says "held by Heimdall".
  - **Windows licenses:** add (name, and the key on stdin, never a command line) and remove, through pkexec.
  - Quarantine decisions are not here: they are Heimdall's (Security > the monitor's page).
  - A Limbo from before this protocol is named as such ("older than this page"); one that does not run, or a person not in
    `limbo-users`, gets a plain line saying what to do. Read every 5 s only while the page is open.
- **Processor page** (built 2026-10-27, my design; `src/cpu.rs`): the Processor row's title opens it, like the Graphics
  card's, with its own pop-out window (`--cpu-window`, app id `yggdrasil-cpu`).
  - **Readout:** package temperature and the hottest core, clocks (average and fastest), load and package power, with the
    same graphs (they fill their box from the first seconds, then cover the last two minutes). Read from sysfs and /proc once a second while the panel is shown; the power comes from the
    tuning service, because the energy counter is root-only.
  - **Limits:** the selected mode's speed cap (powerlimit), turbo, and the long- and short-term power limits (PL1/PL2,
    35-250 W, with the BIOS's values as the reset).
  - **Multipliers** (a K processor): all cores at once, or by how many cores are busy; never higher with more cores busy,
    up to 3 above the processor's own top.
- **Tuning and its crash guard** (`/usr/lib/yggdrasil/tune`, root service `yggdrasil-tune`; `src/tune.rs`): the processor's
  multipliers (MSR 0x1AD) and the graphics card's clocks (NVML offsets: Afterburner's Core and Mem, and a clock cap that,
  with an offset, undervolts). NVIDIA gives Linux no way to set the card's voltage.
  - **Try, Keep, Undo:** `try` applies values for this session; `keep` makes them start with the computer; Undo and
    Defaults go back.
  - **The guard:** a marker with the boot id exists exactly while something is tuned, and only a clean shutdown removes it.
    A start that finds it from an earlier start (a freeze, a reset, a power cut while tuned) applies nothing and says why,
    until "Turn back on". An NVIDIA Xid in the kernel log while the card's clocks are tuned puts them back at once.
  - **What is guarded:** power limits and turbo are kept in /etc/yggdrasil/tuning.json and applied at every start
    unguarded, since they cannot freeze anything. The processor's own values are read once per start of the computer.
  - **Checked on the i9-9900K and the 2080 Ti** with lower values only (multipliers down one, core -50 MHz) and back.
    Rust since 2026-11-09 (below). Tests: `packages/power-helpers/src/tune/tests.rs`.
- **tune, casefan, gpufan and powerlimit in Rust** (2026-11-09, `packages/power-helpers`): the four root helpers above run
  all the time (gpufan while a fan curve is on) and were Python on a Rust desktop. The port keeps their interface so Settings
  and the units do not change: commands, arguments, environment variables, JSON (byte for byte), settings and state files,
  exit codes, log lines, and every safety rule. The other helpers (account, bootscreen, gpuunlock, game-fix, gpu-run) stay
  Python for now.
  - **Status:** the package installs the Rust tune, casefan, gpufan and powerlimit at the same `/usr/lib/yggdrasil/` paths
    (tune, casefan and gpufan since pkgrel 35, powerlimit since 36, with the merged service below). The Python scripts and
    their tests are kept in `helpers/python/` until the Rust ones have run a while; to go back, reinstall the package before
    (pkgrel 35 for the merged service, 34 for the Python helpers).
  - **Light:** no crate but libc, no async runtime, one thread each; libnvidia-ml is opened at run time (dlopen), so
    without the NVIDIA driver they run and say so. `tune watch` reads the kernel's messages from `/dev/kmsg` instead of a
    `journalctl -k -f` child (journalctl only when /dev/kmsg cannot be opened; the service logs which one it reads).
  - **Memory, measured 2026-11-09:** each Rust helper is about 2.8 MB on its own; `nvmlInit` adds about 26 MB in any
    process (the NVIDIA 620 driver maps libcuda and starts a thread), so with the card open each is about 29 MB. The Python
    ones showed 16.5 MB (casefan) and 19 MB (tune) resident but had about 23 MB each swapped to zram, so about 40 MB each,
    and tune's journalctl child held another 46 MB: about 88 MB for tune, about 29 MB in Rust.
  - **Dry run:** every command takes `--dry-run` (each write is printed as `dry run: ...`, nothing is written: no sysfs, MSR,
    NVML set call, file, systemctl or logger), and with it `--root DIR` (every system path under DIR, the card from
    DIR/nvml.json: no hardware at all) and `--steps N` (N rounds of a service's loop on a virtual clock).
  - **Parity:** the 44 cases of the Python tests are Rust tests with the same names and expectations, and powerlimit's six
    (`tests/test_settings_powerlimit.py`, and `test_gpuunlock.py`'s PowerHold, which tests powerlimit's hold).
    `parity/compare.py` runs the Rust helpers and the Python scripts (through `parity/py_dryrun.py`, which replaces only
    their outside) in dry runs on the same fake trees: the tests' fixtures, more cases around them, and samples of a real
    computer from `parity/record.py` (read-only), plus casefan's loop over 1,216 temperature sequences. In a fake tree the
    commands that only ask (busctl for the power mode, `nvidia-smi --query-gpu`) answer from `ROOT/commands.json`, which
    record.py fills from this computer's answers. On 2026-11-09, with a sample of my PC: 120 of 120 cases (37 of them
    powerlimit's) and all sequences the same.
  - **Different on purpose:** a setting whose "ports" is not an object crashed the Python casefan, leaving the ports at
    their last speed (its `reset` crashed too); the Rust one treats it as a failure: every port at 100 %, after a clean stop
    too, since which port is the pump cannot be told. The help text adds the dry-run paragraph. Python tracebacks become one
    "helper: message" line, still exit code 1. `tune watch` logs where it reads the kernel's messages from. powerlimit: an
    infinite number from nvidia-smi (Python crashed on `round(inf)`) counts as no answer, like "[N/A]".
- **One service, yggdrasil-power** (2026-11-09, `packages/power-helpers/src/power.rs`, `/usr/lib/yggdrasil/power`):
  `nvmlInit` costs every process about 26 MB, and three always-on services paid it separately (tune, casefan, and gpufan
  with a curve). yggdrasil-power runs the loops of tune watch, casefan run, gpufan run and powerlimit watch in one process,
  one thread each (no async runtime), with the card opened once and shared (NVML is thread-safe; it is shut down when no
  part holds it).
  - **The old units are switches:** yggdrasil-tune, -casefan, -gpufan and -powerlimit stay, each a oneshot unit that
    requires yggdrasil-power: its start runs `power on PART` (a token in `/run/yggdrasil/power/on/PART`, and it waits until
    the service has the part), its stop `power off PART` (it waits for the part's clean stop). So `systemctl enable`,
    `disable`, `start`, `stop`, `is-active` and the restart that `casefan set` and `gpufan set` run work by the old names, a
    restart of yggdrasil-casefan is still a clean stop of the fans then a fresh read of the setting, and it touches no other
    part (an alias of yggdrasil-power would have restarted tune too, which clears a trial, and `gpufan set auto`'s
    `disable --now` would have stopped everything). yggdrasil-power has no [Install]; the switches pull it in, and it stops
    when none is on. Settings, the polkit rules and the installer (which enables yggdrasil-powerlimit) do not change.
  - **Requires, not BindsTo** (checked 2026-11-09 with stand-in units in a user manager): with BindsTo a crash of the service
    stopped the switches before its ExecStopPost ran, so that saw no part switched on (casefan's failure stop would have been
    missed), and the restart found nothing to run. With Requires a crash or a kill -9 leaves the switches on for
    ExecStopPost, then systemd restarts them with the service; restarting one switch leaves the service and the others
    alone; stopping the last one stops the service; at the service's start limit the switches end inactive, so `is-active`
    tells the truth.
  - **Upgrading:** the package's pacman hook (`90-yggdrasil-power.hook`, after systemd's reload) runs `power adopt`, which
    restarts each of the four units that still runs its old helper process, so an upgraded system moves into the one service
    at once instead of at its next restart.
  - **Each part's safety stays its own:** a part's loop that fails makes its old ExecStopPost's stop on its thread (casefan:
    every port 100 %; gpufan: fans back to the card) and starts again after its old RestartSec, up to its old start limit
    (casefan and gpufan 10 in 120 s; then it stays at its failure stop and its unit is stopped, so `is-active` says so); the
    other parts go on. A panic is caught on the part's thread, which makes its failure stop; every other part makes its
    failure stop too and the service ends with exit code 1. If casefan's loop stops moving for 20 s (stuck in NVML or a
    driver), the supervisor, which never calls NVML, puts every Commander Pro port at 100 % through sysfs and ends the
    service with exit code 1. Whatever way the service ends, ExecStopPost (`power stopped`) makes the stop of each part still
    switched on from a new process, and systemd starts the service again after 5 s. Without NVML, casefan cannot read the
    card, so its ports go to 100 % as before, while tune and powerlimit go on.
  - **Parity:** `compare.py` also runs the merged service's dry run (each line it prints starts with `[part]`) and checks,
    part by part, the same lines in the same order and the same exit code as each helper run on its own followed by its old
    ExecStopPost (`casefan reset`, `gpufan reset`): 17 of 17 cases the same on 2026-11-09 (the tests' trees, a card whose
    readings run out, no driver, a failing init, no Commander Pro, gpufan on auto, the tuning guard, an Xid, mode changes,
    parts switched off, and my PC's sample). The supervisor's isolation rules have their own tests (`src/power/tests.rs`).
  - **Memory, measured 2026-11-09** (dry runs on a sample of my PC with the real card loaded, `--real-card`): the merged
    service 29.6 MB resident (24.0 MB proportional), six threads; on their own tune 29.1, casefan 28.7, gpufan 28.8 and the
    Rust powerlimit 2.7 MB, 89.2 MB together. Running on my PC before the switch: tune 29.8, casefan 29.4 and the Python
    powerlimit 11.5 MB (plus 2.9 MB in swap), and gpufan another 29 MB while a curve is on. CPU is noise either way
    (about 0.1 % of a core for all four).
  - **Status:** the package installs it (pkgrel 36): `/usr/lib/yggdrasil/power`, yggdrasil-power.service, the four units as
    its switches, and the Rust powerlimit. Kept on the branch `power-service-switch` (the daily publish builds main) until it
    has run on my PC. To go back: reinstall pkgrel 35, then start the four units again (they are separate services again).
- **Graphics card unlock** (`/usr/lib/yggdrasil/gpuunlock`, built 2026-10-27 as Phase 0 of Labyrinth OS's
  [GPU_UNLOCK.md](https://github.com/Artifex-Technologies/Labyrinth_OS/blob/main/docs/GPU_UNLOCK.md)): a "Tensor-rate unlock"
  row under the Graphics card row, only with Developer options on and only for a card listed exactly (device, board, VBIOS, driver series)
  in `/usr/share/yggdrasil/gpu-unlock/supported.json`; any other card gets no row at all, and `gpuunlock status --json` keeps
  the reason.
  - **Method:** since 2026-10-27, nlzy/turing-unlock's kernel module, which loads before the NVIDIA driver and acts only with
    `turing_unlock.targets=` on the command line.
  - **Arming:** asks for the password every time (polkit `auth_admin`), needs the module, its firmware and its mkinitcpio
    hook installed and a visible boot menu, takes a snapshot, and holds the power.
  - **The one-try entry:** arming adds "Labyrinth Linux, graphics card unlock (one try)" to the boot menu. It is a copy of
    the first entry with the option, printed by `/etc/grub.d/41_gpu_unlock` only while armed. The owner picks it by hand at
    a restart; it is never the default.
  - **Following an attempt:** `yggdrasil-gpu-unlock-boot.service` and `-verify.service` follow it. The attempt is known by
    the option in /proc/cmdline, the result by the module's kernel log, and the entry goes away after its one try. Tests: `helpers/tests/test_gpuunlock.py` (CI runs them on every push since 2026-11-11). A debug build reads the status from
  `$YGGDRASIL_GPUUNLOCK_FIXTURE` (for screenshots of the other states).

## Ask before connecting (built 2026-10-16)

An app's page has "Ask before connecting to a new place". In that mode (`labyrinth-appnet set APP ask`) the app may use loopback and
the destinations you have allowed; any other connection is dropped by nftables after one rate-limited log line (with the user id).
Three small pieces turn that into a question:

- `labyrinth-appnet-ask` (root service `labyrinth-appnet-ask.service`) reads those kernel-log lines and makes ONE question per app and
  destination address (not per connection). Allow adds the address to the app's nftables set at once (the app's next retry connects,
  and TCP retries by itself within a second or so) and remembers it in `/var/lib/labyrinth/app-ask.json` (root only, 0600). Deny is
  remembered and never asked again. "Allow all" puts the app back to any connection. No answer: asked again after a minute.
- `labyrinth-appnet-ask-agent` (user service) shows each question as a notification with Allow / Allow all / Deny, with the host name
  when a reverse lookup answers quickly.
- Settings lists "Places you decided about" with Forget (asked again next time). It talks to the service through
  `/run/labyrinth/appnet-ask.sock`, so changing a decision needs no password; the kernel tells the service whose apps are whose.

Only apps you set to Ask are affected, so the package manager, time sync and Heimdall are never asked about (the roadmap's "do not
repeat Vipre's over-protectiveness"). Honest limits: an address, not a name (a big content network can look like many addresses);
your own programs could answer the questions too (OpenSnitch and Little Snitch share this); a connection waits for your answer, so
apps may show a short "connecting" delay the first time. Not built: names from DNS or SNI, incoming connections, a default of Ask for
new apps.

Tests: `python tests/test_labyrinth_appnet_ask.py` (rules, question engine, the real socket service with a fake kernel log and a fake
notify-send); `unshare -Urn bash tests/appnet_ask/netns_test.sh` (real nftables: dropped, allowed, allowed at run time, in a throw-away
namespace); `sudo bash tests/appnet_ask/kernel_log_check.sh` (the real kernel log line against the parser: NOT run yet, it needs root).

## Limits (be honest about them)

- Network rules cover apps started through labyrinth-launch (dock, start menu, Settings). An app started another way, or one
  that was already running, is not in its slice until restarted. Single-instance apps already running elsewhere (a browser)
  need a full restart.
- The sandbox does not cover Flatpak apps (they have their own); Location is not controlled yet.
- "Usage" is the time an app has been running (`labyrinth-appusage`, a user service from packages/appusage that follows each app's scopes in the cgroup tree and counts from an app's first scope to its last; apps running before it started are counted from then; it writes when an app stops and every five minutes while one runs), then launches. The sizes are the app's folders in your home, not the package.
- The Optional software tab and the start of removing packages use pkexec; the live image has a passwordless polkit rule, so
  prompts only appear on an installed system.

## Not done yet

- Per-app Location and "which other apps it can see" for Flatpak apps (Flatpak overrides could do both).
- A network-usage logger per app (the running-time logger exists).
- A right-click "App settings" entry in the dock that runs `yggdrasil-settings apps:ID`.
