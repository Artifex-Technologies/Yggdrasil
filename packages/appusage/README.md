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
