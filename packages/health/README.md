# Health

Break reminders for Yggdrasil: rest your eyes, move, drink water, breathe, eat and wind down. A pill in the top bar counts down
to the next one; clicking it drops down its card, drawn by the panel like the other bar cards: the next reminder with a progress
bar, switches for reminders, quiet hours and sound, the list of reminders (a click turns one on or off), steppers for their
intervals and times and for quiet hours, and pause (15 min, 1 h, until tomorrow) or resume. There is no window.

A Rust rewrite of [omarchy-health-mode](https://github.com/musgandapur645-sudo/omarchy-health-mode) (MIT, see
`data/UPSTREAM-LICENSE`): the same reminders, default cadences and messages, plus quiet hours and pausing.

| Reminder     | Default          |
|--------------|------------------|
| Eyes         | every 20 min     |
| Eye exercise | off (every 5 h)  |
| Move         | every 45 min     |
| Water        | every 90 min     |
| Breathe      | every 3 h        |
| Meal         | 14:00            |
| Wind down    | 23:30            |

## How it behaves

- Nothing is sent while the session is locked or idle (the screensaver interface, or logind's IdleHint/LockedHint), while
  paused, or in quiet hours. An interval reminder that comes due then starts counting again from that moment, so you never get
  a burst of overdue reminders when you come back.
- A clock reminder (Meal, Wind down) that came due while you were away goes out once when you are back the same day. One more
  than 3 hours late (the computer was off) is let go for the day. One inside quiet hours does not go out.
- Reminders are quiet notifications (`notify-send -a Health -u low`) with an optional soft sound (`paplay`, freedesktop's
  `message.oga`).

The eye-exercise reminder only reminds. It does not prescribe or teach an exercise.

## Commands

```
yggdrasil-health daemon                the service (yggdrasil-health.service, a user unit)
yggdrasil-health press ID              a control on the card (the panel runs it: toggle:eye, int:move:+, at:meal:-, pause:15, ...)
yggdrasil-health summary               settings and the next reminder, as JSON
yggdrasil-health fire water            send one reminder now
yggdrasil-health pause 30|tomorrow     pause for minutes, or until midnight
yggdrasil-health resume
```

The service wakes when the next reminder is due, and at least once a minute to pick up changed settings and a return from idle.

## Files

- `~/.config/yggdrasil/health.json`: the settings (written by `press`, `pause` and `resume`).
- `~/.local/state/yggdrasil/health/state.json`: when each reminder last went out (written by the service).
- `~/.local/state/yggdrasil/health/bar.json`: what the top bar shows (Yggdrasil's `bar-item` JSON with `until`,
  `{countdown}` and the `card`; see `docs/INTEGRATIONS.md`). `press` rewrites it at once, so the card follows each click.

## Installed files

| From                          | To                                                        |
|-------------------------------|-----------------------------------------------------------|
| `yggdrasil-health`            | `/usr/bin/yggdrasil-health`                               |
| `data/yggdrasil-health.service` | `/usr/lib/systemd/user/yggdrasil-health.service`        |
| `data/health.conf`            | `/usr/share/yggdrasil/integrations/health.conf`           |
| `data/health-symbolic.svg`    | `/usr/share/yggdrasil/icons/health-symbolic.svg`          |
| `data/UPSTREAM-LICENSE`       | `/usr/share/licenses/yggdrasil-health/UPSTREAM-LICENSE`   |

Turn the reminders on with `systemctl --user enable --now yggdrasil-health.service`.
