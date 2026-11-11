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

## Why Rust: measured

It replaces Labyrinth OS's Python `labyrinth-crash-watch`, which followed the journal through a `journalctl -f` child.
Measured on 2026-11-11 with `tools/bench-port.py` against that script from git (the Labyrinth OS repository), both waiting
for 60 s after a 5 s start, on my desktop (16 threads); stand-ins for notify-send, and the Rust one with `--dry-run`, so a
crash meanwhile would show nothing. Method: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| memory while it waits (RSS) | 38.8 MB (Python and journalctl) | 2.4 MB (one process) | −94 %, 36.4 MB less |
| memory while it waits (PSS) | 23.8 MB | 0.4 MB | −99 %, 23.5 MB less |
| start (CPU in its first 5 s) | 22.8 ms | 0.38 ms | −98 %, 22.4 ms less |
| CPU per minute while it waits | 0.23 ms | 0.00 ms | −100 %, 0.23 ms less |
| wake-ups per minute while it waits | 3 | 0 | −100 %, 3 fewer |
| on disk | 3.8 KB script (+ Python, 75 MB, shared) | 350 KB, and 1.27 MB for the notice program | +1.6 MB |

Where Rust is worse: it sees only crashes whose core is stored (see "What it misses"); the journal showed the others too.

What else changed:

- Instead of reading every crash record in the journal through journalctl, it reads the program and the signal from the
  attributes systemd-coredump sets on the core file, and whose crash it is from the file's name.
- Each notice runs in a short-lived process of its own, so the watcher holds no D-Bus connection; "Ask ..." is offered only
  when the system's assistant can be asked.
- Tests: 11 unit, 3 on the watcher (inotify, attributes, the user filter, the rate limit) and 1 on a private D-Bus.
