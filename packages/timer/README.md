# yggdrasil-timer

A countdown timer in Yggdrasil's top bar, rewritten in Rust from [NickmDrummer/omarchy-timer](https://github.com/NickmDrummer/omarchy-timer).

- **The pill** is the timer's icon (`timer-symbolic.svg`) alone when no timer runs, the icon and the time left while one does,
  `Paused 4:12` in amber, or a red `Done` until it is reset. A click drops the card down under it; a click elsewhere or a second
  click on the pill closes it.
- **The card** (no title, like the panel's own cards): the countdown with a progress bar, Pause/Resume, +1 min and Reset (Again
  once done); presets (1, 5, 10, 15, 25 and 45 min); and a length of your own, stepped a minute at a time with − and + or typed
  (`25m`, `90s`, `1h30m`, `1:30`), started with Start. One timer at a time: starting another ends the one before. Esc closes.
- **The alarm** rings with the window closed: a critical notification and the `alarm-clock-elapsed` sound (canberra, or pw-play /
  paplay of the freedesktop sound file; silent when the system has none).

Nothing stays running. Starting a timer writes `~/.local/state/yggdrasil/timer/state.json` and `bar.json` and asks systemd for a
one-off user timer (`systemd-run --user --on-active=…`) that runs `yggdrasil-timer ring ID GEN` at the end. The panel reads
`bar.json` (the bar add-on in `timer.conf`) and counts down to its `until` by itself. Systemd's one-off timers do not survive a
reboot, so `yggdrasil-timer reconcile` runs at login (an autostart entry) and whenever the window opens: timers that ended while the
computer was off are shown as done, running ones get their alarm back.

```
yggdrasil-timer                    the card
yggdrasil-timer start DUR [LABEL]  start one from a terminal or a shortcut
yggdrasil-timer reconcile          see above
```

## Installed files

| file | to |
| --- | --- |
| `target/release/yggdrasil-timer` | `/usr/bin/yggdrasil-timer` |
| `timer.conf` | `/usr/share/yggdrasil/integrations/timer.conf` |
| `timer-symbolic.svg` | `/usr/share/yggdrasil/icons/timer-symbolic.svg` |
| `yggdrasil-timer.desktop` | `/usr/share/applications/yggdrasil-timer.desktop` |
| `yggdrasil-timer-reconcile.desktop` | `/etc/xdg/autostart/yggdrasil-timer-reconcile.desktop` |

Needs `systemd` (user manager) and `libnotify`; `libcanberra` or PipeWire for the sound. MIT, like upstream.
