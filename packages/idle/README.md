# yggdrasil-idle

Yggdrasil's idle manager: it watches how long you have been away (Wayland `ext-idle-notify`) and, with `mode = act` in
`~/.config/yggdrasil/idle.conf`, dims, blacks out the screen, shows the screensaver, locks, turns the screen off and sleeps.
Settings > Power writes that file; the program picks up changes without a restart.

## Keyboard and mouse lights

With "Keyboard and mouse lights off while dimmed" (Settings > Peripherals > Keyboard), the lights go off when the screen
dims, goes black or off, or shows the screensaver, and come back as they were when you are back (`src/lights.rs`):

- Razer keyboards, mice, keypads and mouse mats through openrazer's daemon on the session bus (when it runs; asking never
  starts it): each lighting zone a device has (the whole device, logo, scroll wheel, backlight, left and right strips). A
  mouse's own brightness can read 0 while its logo and wheel are lit, so every zone counts on its own.
- A laptop's keyboard backlight (`/sys/class/leds/*kbd_backlight*`) through logind, which lets the signed-in user set it.

Only what was lit is turned off and remembered. What to put back is written in full to `~/.local/state/yggdrasil/lights-off`
before anything goes dark, so a session that ends while the lights are off (log-out, reboot, crash) loses nothing: the next
start puts them back (it waits up to 30 s for openrazer's daemon). Everything read back from that list is checked first: a
Razer zone gets a number from 1 to 100, a backlight a whole number up to its own maximum, and only names openrazer or the
kernel could have given.

The runs take turns on one worker thread, so the idle loop never waits for a daemon, and every D-Bus call has a 2 s limit.
Hooks of my own still work: each executable file in `~/.config/yggdrasil/idle.d` runs with `lights-off` or `lights-on`.

Until 2026-11-12 this was a shell hook (`/usr/lib/yggdrasil/idle.d/keyboard-lights`, run through busctl). A list it left
behind has the same format and is put back once. The port changed two things on purpose: a laptop backlight named
`vendor::kbd_backlight` (the usual name) was turned off by the hook but never put back, because its name check allowed no
`:`; and the hook put back any digits it found in the list (150 for a Razer zone, more than a backlight's maximum).

## Tests

`cargo test` runs, besides the rest:

- **the differential**: the old shell hook (`tests/fixtures/keyboard-lights.sh`, byte for byte the last version, kept only
  for this until a week's soak has passed, 2026-11-19) and the Rust code on the same made-up worlds. The hook gets a
  stand-in busctl and sleep on the PATH (canned answers from files, never a bus); the Rust code a made-up openrazer daemon
  and logind on peer-to-peer D-Bus connections inside the test; both a scratch folder for `/sys/class/leds`. Each step's
  calls, list and resulting brightness must be the same: my desk (a keyboard at 75, a mouse at 0 with its logo at 75 and
  wheel at 40, a keypad, a mouse mat, a headset, a name D-Bus refuses, two laptop backlights), no daemon, a list left from
  an earlier session with a daemon that comes up late, a daemon that never comes, and a hostile list. The two deliberate
  differences above are checked on each side on their own.
- **property tests** on the list's parser: random bytes, non-numbers, huge values and bad names are never put back, and
  what the Rust code writes reads back the same.

## Why Rust: measured

Measured on 2026-11-12 on my desktop (16 threads) with the ignored test `lights::tests::measure`: the old hook, using the
real busctl, and the Rust code against the same made-up openrazer daemon and logind (a separate process) on a private bus
with no service files, so nothing can be started on it. My desk as above: six Razer zones and two backlights lit. 30 runs
of each after two warm-up rounds, old and new taking turns; the Rust time includes connecting to both buses, as each run
does. Median, with the 10th to 90th percentile.

| | old hook (sh + busctl) | Rust (in yggdrasil-idle) | change |
| --- | --- | --- | --- |
| lights-off, wall | 100.1 ms (96.9–101.2) | 2.82 ms (2.44–3.20) | −97 %, 97.3 ms faster |
| lights-off, CPU | 104.8 ms | 1.24 ms | −99 % |
| lights-on, wall | 20.5 ms (19.7–21.6) | 0.91 ms (0.80–1.05) | −96 %, 19.6 ms faster |
| lights-on, CPU | 17.8 ms | 0.51 ms | −97 % |
| processes started, lights-off | 175 | 0 (2 short-lived threads: the two bus connections) | |
| processes started, lights-on | 21 | 0 (2 threads) | |

Every busctl call was a process of its own, and every `$(...)` and pipe in the hook another: 175 processes to turn off
eight lights. The idle log also stopped starting `date` for each line.

To measure again (in user, mount and PID namespaces of their own, with the real buses' sockets covered by empty folders, so
neither openrazer nor logind can be reached even by mistake):

    cargo test --release --no-run          # prints the test binary's path: target/release/deps/yggdrasil_idle-HASH
    uid=$(id -u)
    unshare -rmpf --mount-proc sh -c 'mount -t tmpfs none /run/user/'"$uid"' && mount -t tmpfs none /run/dbus && exec "$0" --exact lights::tests::measure --ignored --nocapture' ../../target/release/deps/yggdrasil_idle-HASH
