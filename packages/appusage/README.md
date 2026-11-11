# App usage (labyrinth-appusage)

How long each app has been running, for Settings > Apps ("sort by usage"). A user service, on for every account
(`labyrinth-appusage.service`); it keeps app names and seconds only, and nothing leaves the computer.

The time goes into `~/.local/share/labyrinth/app-usage.json` as `seconds`, next to the launch counts labyrinth-launch writes
there (`launches`, `last`); every other key in the file is kept. Settings reads the file.

## How it works

Every app started by labyrinth-launch, by another launcher or by Flatpak runs in a systemd scope of its own
(`lapp_<id>_N.scope`, `app-<id>-PID.scope`, `app-<id>@HASH.scope`, `app-flatpak-<id>-N.scope`), and every scope is a folder in
this user's cgroup tree (`/sys/fs/cgroup/user.slice/user-UID.slice/user@UID.service/...`). The service follows that tree with
inotify: when a scope's folder is made it works out which desktop entry the scope belongs to, and when the app's last scope is
removed it writes the app's time down. In between it sleeps; while an app runs it also writes every five minutes, so Settings
sees time that is still being counted, and once more when the service is stopped (log out, shut down).

Each app counts once however many windows or scopes it has, from its first scope's start to its last one's end. Time while
the computer sleeps is not counted. Apps already running when the service starts count from then.

Without a cgroup v2 tree of the user's systemd it asks `systemctl --user list-units --type=scope --state=running` every 30
seconds instead, as the Python script did.

Measured on my PC (2026-11-10), with three apps running: 0.6 MB (PSS), 156 KB of it its own, against 4.6 MB (PSS) for the
Python script, which also started systemctl every 30 seconds.

## Commands

    labyrinth-appusage            run until stopped (the user service does)
    labyrinth-appusage --once     one sample, as the Python script took it: 30 seconds for every app with a running scope
                                  (asked of systemctl), then exit
    labyrinth-appusage --scopes   the scopes the service sees in the cgroup tree, each with its app

`LABYRINTH_APPUSAGE_FLUSH=SECONDS` changes the five minutes (the tests use it).

## The Python script

It is kept in `python/` with its tests, for a rollback, until this one has run a while: it was
`/usr/local/bin/labyrinth-appusage` in Labyrinth OS's image. `parity/compare.py` runs both on the same inputs (scope names,
usage files, this computer's own scopes) and shows they decide and write the same.

## Why Rust: measured

Measured on 2026-11-11 with `tools/bench-port.py` against the Python script from git, writing to a scratch folder, on my
desktop (16 threads); the service ran 60 s after a 5 s start. Method and every case: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| the service, memory (PSS) | 7.0 MB | 0.6 MB | −92 %, 6.4 MB less |
| the service, memory (RSS) | 12.9 MB | 2.6 MB | −79 %, 10.2 MB less |
| the service, CPU per minute | 1.60 ms | 1.35 ms | −15 %, 0.25 ms less (about the same) |
| the service, wake-ups per minute | 8 | 2 | −75 %, 6 fewer |
| the service, start (CPU in its first 5 s) | 18.5 ms | 1.11 ms | −94 %, 17.4 ms less |
| `--once`, wall time | 26.2 ms | 20.9 ms | −20 %, 5.3 ms faster |
| `--once`, CPU time | 24.6 ms | 2.35 ms | −90 %, 22.2 ms less |
| on disk | 3.7 KB script (+ Python, 75 MB, shared) | 503 KB | +499 KB |

Idle CPU was already small for the script (it woke every 30 s); the gain is memory and that nothing is started every 30 s.
`--once` asks systemctl, as the script did; the Rust one waits for it in 20 ms steps, so most of its 20.9 ms is waiting.

More accurate:

- Time is counted from an app's first scope to its last one's end. The script added 30 s to whatever ran at each sample:
  an app open for 10 s between two samples counted nothing, one started just before a sample got 30 s.
- Parity: 12,018 scope names against two sets of desktop entries, 40 sample sequences (240 samples) on empty, broken and
  odd files, and this computer's own scopes: all the same, apart from the order new apps are added to the file (the
  script's followed its set's order, which changed from run to run; here it is sorted).
- Tests: the script's 2 carried over, 8 more, and a live one on the user's own systemd.
