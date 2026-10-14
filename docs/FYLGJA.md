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

Controls, Today, Stats, Clipboard, Apps, Wallpaper, Power (in that order).

| Section | State |
| --- | --- |
| Controls | Built: tiles for Wi-Fi (tap = radio on/off, chevron = Settings), Bluetooth (same), Audio (tap = mute, chevron = a drop-down list of outputs over the card, shown only when there is an output), Night Light (constant 4000 K; a schedule needs a location) and Dark mode; the master volume bar (click a point or drag to set the level; the app sliders drag too) with a caret, shown only while an app is playing sound, that drops down a mixer (every app playing sound, each with its own slider); and a now-playing card (left out, so the card gets shorter, when nothing is playing) with the cover (the browser's video thumbnail when the browser gives one), title, artist, buttons and progress |
| Today | Built: time with seconds, full date, number of open windows |
| Stats | Built: ring gauges for CPU (name, clock, process count), GPU (name, load, clock, VRAM used / total) and RAM (used / total, free); under them the disk (name, used / total), network speed now (down, up) and uptime; on the right the top apps by CPU or RAM (a switch), and an internet speed test. Read once a second only while Stats is showing. The speed test runs only when its button is pressed: it downloads about 40 MB and uploads about 15 MB against Cloudflare's speed servers (`speed.cloudflare.com`) with `curl` |
| Clipboard | Built: the last 25 texts, click to copy again, Clear. In memory only; skips password managers (`x-kde-passwordManagerHint`); off in Settings > Appearance > Notch. Text only for now |
| Apps | Built: the dock's pinned apps |
| Wallpaper | Built: thumbnails of the pictures Settings offers, click to apply (`plasma-apply-wallpaperimage`), pages |
| Power | Built: Lock, Sleep, Hibernate (when the machine can), Restart, Shut down |

## Still to do

- Clipboard: images, pinning an entry, search.
- Wallpaper: a "current" marker, adding a folder of your own, slideshow.
- Keyboard use inside the notch (arrows between sections).
- Sarina speaking her answer; Sarina's ask-box for typing.
- A weather section.

## Not verified yet

- Wi-Fi and Bluetooth toggles with a real adapter (the VM has none), Night Light actually tinting the screen (the setting is written and KWin reloaded), Dark mode from the tile, choosing another sound output (the VM has one), seeking by clicking the progress bar (not built: it is display only)
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
