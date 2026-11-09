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
  `developer = yes` in ~/.config/yggdrasil/panel.conf.

- **Games** (`yggdrasil-settings games`, 2026-11-05): the graphics cards a game can draw on, fixes for particular games, and frame generation.
  - Every card, with what it does: which one shows the screen, which one the computer started on, its driver. Read by
    `yggdrasil-gpu-run list --json` (helpers/gpu-run, installed as /usr/bin/yggdrasil-gpu-run), the same code that later starts
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
    largest monitor, from hyprctl or xrandr). A file is backed up before its first change (FILE.yggdrasil-backup), or marked
    as made (FILE.yggdrasil-created, from a template in the game's folder or lines in the catalogue); Undo restores it. A game
    Proton has not started yet shows "Start the game once first". No administrator rights. First fixes: **Dead Space 2**
    (its menu never offers 21:9; settings.txt at the screen's size, V-Sync off: true 21:9 tried at 3440x1440) and
    **Fallout 4** (the settings files its launcher would make, from its templates, at the screen's size, borderless; the
    world fills 21:9 (confirmed by the owner in play), the HUD stays 16:9 and stretched; and a launch option that starts the game without Bethesda's launcher, which kept the keyboard
    once the game was up: tried by the owner 2026-11-05, keys work). A fix's `launch` is offered with Copy launch option (with
    `kairos` in front when frame generation is installed); Steam keeps launch options to itself, so it is copied,
    not written. Tests: `helpers/tests/test_game_fix.py`. Patches to games' code come per game.
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
  `gpu.hold` and the slider stops there.
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
    checked on the RTX 2080 Ti with a kill -9. Tests: `helpers/tests/test_gpufan.py`.
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
  - Setting: `/etc/yggdrasil/case-fans.json`. Tests: `helpers/tests/test_casefan.py`.
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
    Tests: `helpers/tests/test_tune.py`.
- **tune, casefan and gpufan in Rust** (2026-11-09, `packages/power-helpers`): the three root helpers above run all the
  time (or while a fan curve is on) and were Python on a Rust desktop. The port keeps their interface so Settings and the
  units do not change: commands, arguments, JSON (byte for byte), settings and state files, exit codes, log lines, and every
  safety rule. The other helpers (account, powerlimit, bootscreen, gpuunlock, game-fix, gpu-run) stay Python for now.
  - **Status:** built and tested, not installed yet. The switch (the package installs the Rust binaries at the same
    `/usr/lib/yggdrasil/` paths; the Python scripts move to `helpers/python/` for a rollback) is the branch
    `power-helpers-switch`, so the daily package publish does not ship it before it has run on this PC under watch.
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
  - **Parity:** the 44 cases of the Python tests are Rust tests with the same names and expectations.
    `parity/compare.py` runs the Rust helpers and the Python scripts (through `parity/py_dryrun.py`, which replaces only
    their outside) in dry runs on the same fake trees: the tests' fixtures, more cases around them, and samples of a real
    computer from `parity/record.py` (read-only), plus casefan's loop over 1,216 temperature sequences. On 2026-11-09:
    83 of 83 cases and all sequences the same.
  - **Different on purpose:** a setting whose "ports" is not an object crashed the Python casefan, leaving the ports at
    their last speed (its `reset` crashed too); the Rust one treats it as a failure: every port at 100 %, after a clean stop
    too, since which port is the pump cannot be told. The help text adds the dry-run paragraph. Python tracebacks become one
    "helper: message" line, still exit code 1. `tune watch` logs where it reads the kernel's messages from.
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
    the option in /proc/cmdline, the result by the module's kernel log, and the entry goes away after its one try. Tests: `helpers/tests/test_gpuunlock.py`. A debug build reads the status from
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
