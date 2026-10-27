# Settings > Apps, Developer options, and the per-app controls

Built 2026-10-15/16. Code: `packages/settings/src/appmgr.rs` (what is known about each app, and changes to it),
`apps_ui.rs` (the pages), the power-limit helper `packages/settings/helpers/powerlimit` (installed as `/usr/lib/yggdrasil/powerlimit`;
`tests/test_settings_powerlimit.py`), and, for the per-app network rules and sandbox, the operating system's launcher declared in its
`apps` integration (INTEGRATIONS.md; on Labyrinth `labyrinth-launch` and `labyrinth-appnet`, tested in Labyrinth's repository).
`cargo test` in `packages/settings`.

## The pages

- **Installed** (tab): every visible desktop entry, sortable by Usage (how often it was opened from here), Size (its folders
  under ~/.config, ~/.cache, ~/.local/share, ~/.var/app) or A to Z. System tools (settings panels, Avahi, ...) are hidden
  unless "Show system tools" is ticked. Each row has icon-only buttons: restart, close (both only while it runs), remove
  (asks first; shows the package; the desktop's own packages are refused).
- **An app's page** (click a row, or `yggdrasil-settings apps:ID`): Network (switch; click the name too), open at login,
  status, storage with Clear cache and Clear data (the latter lists the folders and asks for the password), the privacy
  sandbox, Remove.
- **Optional software** and **Minos** are the other two tabs.
- **Developer options** (About: click the logo five times while holding the ` key): adds "Any connection / Ethernet only /
  Wi-Fi only" next to an app's network switch, and the selected power mode's CPU and graphics-card limits under Power. Stored as
  `developer = yes` in ~/.config/labyrinth/panel.conf.

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
  Graphics card (each with a reset icon left of its slider); choosing another mode shows and changes that mode's limits (Matt's layout, 2026-10-27).
  While the graphics card unlock is armed or measured, `/var/lib/yggdrasil/gpu-unlock/power-hold` exists and no mode goes above
  the card's default watts (the modes' own limits stay saved and come back when the hold ends); `powerlimit get` reports it as
  `gpu.hold` and the slider stops there.
- **Graphics card page** (built 2026-10-27, Matt's design; `src/gpu.rs`): the Graphics card row's title opens it. An
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
    checked on the RTX 2080 Ti with a kill -9. Tests: `helpers/tests/test_gpufan.py`.
  - **Fixtures:** debug builds read `$YGGDRASIL_GPUFAN_FIXTURE` for screenshots of each mode.
- **Processor page** (built 2026-10-27, Matt's design; `src/cpu.rs`): the Processor row's title opens it, like the Graphics
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
    Tests: `helpers/tests/test_tune.py`.
- **Graphics card unlock** (`/usr/lib/yggdrasil/gpuunlock`, built 2026-10-27 as Phase 0 of Labyrinth OS's
  [GPU_UNLOCK.md](https://github.com/Artifex-Technologies/Labyrinth_OS/blob/main/docs/GPU_UNLOCK.md)): a "Tensor-rate unlock"
  row under the Graphics card row, only with Developer options on and only for a card listed exactly (device, board, VBIOS, driver series)
  in `/usr/share/yggdrasil/gpu-unlock/supported.json`; any other card gets no row at all, and `gpuunlock status --json` keeps
  the reason. Arming asks for the password every time (polkit `auth_admin`), takes a snapshot, adds a one-try GRUB entry
  (`/etc/grub.d/41_gpu_unlock`, which prints nothing otherwise) and holds the power; `yggdrasil-gpu-unlock-boot.service` and
  `-verify.service` follow the attempt. Tests: `helpers/tests/test_gpuunlock.py`. A debug build reads the status from
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
- "Usage" is the time an app has been running (`labyrinth-appusage`, a user service that samples every 30 s; apps running before it started are counted from then), then launches. The sizes are the app's folders in your home, not the package.
- The Optional software tab and the start of removing packages use pkexec; the live image has a passwordless polkit rule, so
  prompts only appear on an installed system.

## Not done yet

- Per-app Location and "which other apps it can see" for Flatpak apps (Flatpak overrides could do both).
- A network-usage logger per app (the running-time logger exists).
- A right-click "App settings" entry in the dock that runs `yggdrasil-settings apps:ID`.
