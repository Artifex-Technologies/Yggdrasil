# Fylgja (the notch)

Named for the Norse spirit that follows a person for life. A tribute to hitodama: a black tab in the
top bar's middle that opens into a card. Hover to open, wheel to move between sections, move the
pointer away to close. Code: `packages/panel/src/fylgja.rs`.

## How it behaves

- Rest the pointer on the closed tab for about a moment and, if something is playing, a small card
  slides down with it (cover, title, artist, previous / play-pause / next). With nothing playing the tab
  just stays as it is; the wheel or a click on the tab still opens the full notch.
- The wheel (or a click on the card) opens the full notch. All seven section titles are always shown in
  the black band along the top, the current one lit; scroll to move on, or click a title to go there. Moving the pointer away slides it back up.
- Opened by a shortcut it stays until Esc or a click outside: **Super+V** the clipboard history,
  **Super+S** a question for Sarina (see [SARINA_VOICE.md](SARINA_VOICE.md)).
- While open, the bar's black tab widens into the card so there is no separate bar or step.

## Sections

Controls, Today, Stats, Notifications, Wallpaper, Power (in that order). The clipboard history, once a section here, is a window
of its own since 2026-10-25: `yggdrasil-clipboard` (Super+V), see [packages/clipboard/README.md](../packages/clipboard/README.md).

| Section | State |
| --- | --- |
| Controls | Built: tiles for Wi-Fi (tap = radio on/off, chevron = Settings), Bluetooth (same), Audio (tap = mute, chevron = a drop-down list of outputs over the card, shown only when there is an output), Night Light (tap = on/off, chevron = a drop-down under the tile with the temperature, 1,500-5,500 K, on a slider painted in the colours it gives, and a Night Light settings button that opens Settings > Displays, where the schedule and the gradual fade are; the icon is the one omarchy-nightlight shows, Material Design Icons' theme-light-dark; see below) and Dark mode; the master volume bar (click a point or drag to set the level; the app sliders drag too) with a caret, shown only while an app is playing sound, that drops down a mixer (every app playing sound, each with its own slider); and a now-playing card (left out, so the card gets shorter, when nothing is playing) with the cover (the browser's video thumbnail when the browser gives one), title, artist, buttons and progress |
| Today | Built: time with seconds, full date, number of open windows |
| Stats | Built: ring gauges for CPU (name, clock, process count), GPU (name, load, clock, VRAM used / total) and RAM (used / total, free); under them the disk (name, used / total), network speed now (down, up) and uptime; on the right the top apps by CPU or RAM (a switch), and an internet speed test. Read once a second only while Stats is showing. The speed test runs only when its button is pressed: it downloads about 40 MB and uploads about 15 MB against Cloudflare's speed servers (`speed.cloudflare.com`) with `curl` |
| Notifications | Built: grouped by app, newest group first; open a group to list them, click one to go to the app, Clear all (see NOTIFICATIONS.md). Replaced the old Apps section |
| Wallpaper | Built: thumbnails of the pictures Settings offers, click to apply (`plasma-apply-wallpaperimage`), pages |
| Power | Built: Lock, Sleep, Hibernate (when the machine can), Restart, Shut down |

## Night Light

Done by hyprsunset (Hyprland's own). The code is shared by the notch and Settings > Displays (the same settings, shown as rows
there): `packages/yggdrasil-conf/src/nightlight.rs`. hyprsunset is spoken to on its own socket
(`$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE/.hyprsunset.sock`) and found in `/proc`; no `hyprctl` or `pgrep` is started.
The settings are kept in `~/.config/hypr/hyprsunset.conf`, whose first line holds them as they are shown
(`# yggdrasil: temperature=… schedule=… fade=… minutes=…`); a file of the user's own found there is moved to
`hyprsunset.conf.before-yggdrasil` the first time. The panel starts hyprsunset when the session starts and starts it again if it
stops (checked every 30 seconds).

- Without a schedule the switch turns the light on and off and it stays that way, also after signing in (one profile at midnight).
- With a schedule the switch overrides it until the schedule's next change. A fade is written out as a staircase of one-minute
  profiles, evenly spaced in mireds (adapted from omarchy-nightlight); the turn-on time is when the screen starts warming, the
  turn-off time when it starts cooling. Switching the light off inside the lit stretch of a schedule with a fade stops
  hyprsunset until the fade out has ended (`~/.local/state/yggdrasil/nightlight-off-until`), since the staircase would warm
  it again a minute later.
- A new temperature shows at once while the light is on; the restart hyprsunset needs to take it into its profiles waits until
  the light is off, so the screen does not flash.

## Still to do

- Wallpaper: a "current" marker, adding a folder of your own, slideshow.
- Keyboard use inside the notch (arrows between sections).
- Sarina speaking her answer; Sarina's ask-box for typing.
- A weather section.

## Not verified yet

- Wi-Fi and Bluetooth toggles with a real adapter (the VM has none), Night Light against a running hyprsunset (the file it writes, the fade steps and the drop-down's drawing are tested, and Settings' rows were seen in the session; hyprsunset itself was not installed on the machine it was built on), Dark mode from the tile, choosing another sound output (the VM has one), seeking by clicking the progress bar (not built: it is display only)
- Media buttons and the volume sliders against a real player (the VM has no audio; a stand-in player
  showed the card, cover and buttons drawing correctly).
- Per-app volume rows.
- Stats on real hardware: the NVIDIA readings (`nvidia-smi`; only the parsing is tested against a sample line), AMD (read from `/sys`, untested), Intel graphics (not read at all), the disk's model name on an NVMe drive, the CPU clock on a real machine. The VM showed CPU, RAM, disk, uptime, top apps and the speed test working (179 Mbps down, 33 up).
- The speed test is approximate (a few streams of a few MB); a gigabit line will read low.
- Super+V and Super+S pressed on a real keyboard (the shortcuts are written into
  `kglobalshortcutsrc`; the same D-Bus calls were verified with `busctl`). Super+V may also reach
  Plasma's own clipboard popup; the config turns that one off.
- Hibernate in the Power section on real hardware.
- Cover pictures from the web (Spotify) are fetched once with `curl` and cached in
  `~/.cache/labyrinth/fylgja/`.
- Seen once: the card sat 28 px too high after clicking a dot and I could not make it happen again.
