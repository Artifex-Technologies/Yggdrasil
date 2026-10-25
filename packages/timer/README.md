# yggdrasil-timer

Countdown timers in Yggdrasil's top bar, rewritten in Rust from [NickmDrummer/omarchy-timer](https://github.com/NickmDrummer/omarchy-timer).

- **The pill** shows the timer that ends soonest (`Tea 4:12 +1` when more are running), `Paused 4:12`, or a red `Done` until it
  is dismissed. With no timer it reads `Timer`. A click opens the window; a second click closes it.
- **The window**: presets (1, 5, 10, 15, 25, 45 min, 1 h), any length (`7` minutes, `1:30`, `1:02:03`, `90s`, `1h30m`,
  `25 min`), an optional label, and per timer Pause/Resume, +1 min and Cancel (Again and Dismiss once done). Several timers can run
  at once. Esc closes.
- **The alarm** rings with the window closed: a critical notification and the `alarm-clock-elapsed` sound (canberra, or pw-play /
  paplay of the freedesktop sound file; silent when the system has none).

Nothing stays running. Starting a timer writes `~/.local/state/yggdrasil/timer/state.json` and `bar.json` and asks systemd for a
one-off user timer (`systemd-run --user --on-active=…`) that runs `yggdrasil-timer ring ID GEN` at the end. The panel reads
`bar.json` (the bar add-on in `timer.conf`) and counts down to its `until` by itself. Systemd's one-off timers do not survive a
reboot, so `yggdrasil-timer reconcile` runs at login (an autostart entry) and whenever the window opens: timers that ended while the
computer was off are shown as done, running ones get their alarm back.

```
yggdrasil-timer                    the window
yggdrasil-timer start DUR [LABEL]  start one from a terminal or a shortcut
yggdrasil-timer reconcile          see above
```

## Installed files

| file | to |
| --- | --- |
| `target/release/yggdrasil-timer` | `/usr/bin/yggdrasil-timer` |
| `timer.conf` | `/usr/share/yggdrasil/integrations/timer.conf` |
| `yggdrasil-timer.desktop` | `/usr/share/applications/yggdrasil-timer.desktop` |
| `yggdrasil-timer-reconcile.desktop` | `/etc/xdg/autostart/yggdrasil-timer-reconcile.desktop` |

Needs `systemd` (user manager) and `libnotify`; `libcanberra` or PipeWire for the sound. MIT, like upstream.
