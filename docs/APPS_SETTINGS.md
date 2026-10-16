# Settings > Apps, Developer options, and the per-app controls

Built 2026-10-15/16. Code: `packages/settings/src/appmgr.rs` (what is known about each app, and changes to it),
`apps_ui.rs` (the pages), and three helpers in `profile/airootfs/usr/local/bin/`: `labyrinth-launch`, `labyrinth-appnet`,
`labyrinth-powerlimit`. Tests: `tests/test_labyrinth_launch.py`, `test_labyrinth_appnet.py`, `test_labyrinth_powerlimit.py`
(`python tests/test_labyrinth_*.py`) and `cargo test` in `packages/settings`.

## The pages

- **Installed** (tab): every visible desktop entry, sortable by Usage (how often it was opened from here), Size (its folders
  under ~/.config, ~/.cache, ~/.local/share, ~/.var/app) or A to Z. System tools (settings panels, Avahi, ...) are hidden
  unless "Show system tools" is ticked. Each row has icon-only buttons: restart, close (both only while it runs), remove
  (asks first; shows the package; the desktop's own packages are refused).
- **An app's page** (click a row, or `labyrinth-settings apps:ID`): Network (switch; click the name too), open at login,
  status, storage with Clear cache and Clear data (the latter lists the folders and asks for the password), the privacy
  sandbox, Remove.
- **Optional software** and **Minos** are the other two tabs.
- **Developer options** (About: click the logo five times while holding the ` key): adds "Any connection / Ethernet only /
  Wi-Fi only" next to an app's network switch, and per-power-mode CPU and graphics-card limits under Power. Stored as
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
- **Power limits** (labyrinth-powerlimit): /etc/labyrinth/power-limits.json; the root service
  `labyrinth-powerlimit.service` applies the active mode's limits on every mode change (CPU: `scaling_max_freq` as a percent
  of `cpuinfo_max_freq`; GPU: `nvidia-smi -pl`). Verified on the RTX 2080 Ti and the Intel pstate CPU; restored to defaults.

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
- A right-click "App settings" entry in the dock that runs `labyrinth-settings apps:ID`.
