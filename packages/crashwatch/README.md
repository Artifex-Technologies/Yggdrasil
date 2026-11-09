# Crash watch

When one of your own programs crashes, a notice says which one and how ("game.x86_64 crashed: /opt/Game/game.x86_64
(SIGSEGV)."). When the system has an assistant, the notice also offers to ask it ("Want Sarina to take a look?"); without
one, nothing about an assistant is shown.

It is off until you turn it on, for each account:

    systemctl --user enable --now yggdrasil-crashwatch

and off again with `systemctl --user disable --now yggdrasil-crashwatch`.

## How it works

systemd-coredump stores the core of a crashed program in `/var/lib/systemd/coredump/` (`Storage=external`, the default). The
crash watch follows that folder with inotify. A new file's name says whose crash it is (`core.COMM.UID.BOOT_ID.PID.TIME.zst`);
only your own (your user ID) count. Which program and which signal it reads from the extended attributes systemd-coredump puts
on the file (`user.coredump.exe`, `user.coredump.signal`), which only you may read, through the file's ACL. It never opens a
core, never sends one anywhere, and needs no rights of its own.

At most one notice per program every ten minutes (the clock keeps counting while the computer sleeps), so a crash loop does
not become a storm of notices.

Each notice runs in its own short-lived process (`/usr/lib/yggdrasil/crashwatch-notice`) that talks to the notification
server on the session bus. With an assistant, it waits up to ten minutes for "Ask ...", or until the notice is closed. A
click shows "... is looking at the ... crash" at once, asks the assistant (at most three minutes, in your home folder) and
replaces that notice with the answer, which stays until you close it. Then the process exits. The watcher itself is one
blocking read with no threads and no D-Bus connection: about 0.4 MB (PSS), 128 KB of it its own.

## The assistant

The assistant is the system's `assistant` integration file (Yggdrasil's docs/INTEGRATIONS.md). The crash watch offers it only
when that file has a `question` command (the question is added as the last argument; it prints the answer; it should only
read). `crash_question` may say how to ask, with `{program}` and `{signal}` in it; without it, a plain question is used.

## What it misses

A crash whose core is not stored: `Storage=journal` or `none` in coredump.conf, a core over `ExternalSizeMax`, or a process
whose core size limit (`ulimit -c`) is 0. Following the journal instead would catch those, at about 50 MB of memory. A game
that runs through Proton/Wine usually reports its own errors without a Linux crash, so there is no core and no notice.

## Commands

    yggdrasil-crashwatch --dry-run              print "EXE crashed (SIGNAL)" for each new crash of yours instead of a notice
    crashwatch-notice --print EXE SIGNAL        what a notice would say (and what the assistant would be asked)
