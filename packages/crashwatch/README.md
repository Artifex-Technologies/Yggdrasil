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

A crash whose core is not stored leaves no file, but systemd-coredump still writes its "process dumped core" entry, and
journald files it in your own journal (`/var/log/journal/MACHINE-ID/user-UID.journal`). So the watcher also follows that
file, in its own process through libsystemd (no `journalctl` child), for that one message
(`MESSAGE_ID=fc2e22bc6ee647b6b90729ab34a250b1`) about your own crashes (`COREDUMP_UID`), from the moment it starts: the
program and the signal are its `COREDUMP_EXE` and `COREDUMP_SIGNAL`. Any program of yours may write to your journal, so only
entries systemd-coredump itself wrote count (journald's own `_EXE` field, which a program cannot set). It opens only the file
journald writes now, never the archived ones, and an inotify watch on it (with the core folder's, on the same descriptor)
wakes it when journald writes there; when journald starts a new file, it is read from its first entry. A crash whose core
is stored is told of twice, by its file and by its entry; the first to arrive counts (the process id tells them apart). If
the journal cannot be read, the watcher says so once and goes on with the stored cores.

At most one notice per program every ten minutes (the clock keeps counting while the computer sleeps), so a crash loop does
not become a storm of notices.

Each notice runs in its own short-lived process (`/usr/lib/yggdrasil/crashwatch-notice`) that talks to the notification
server on the session bus. With an assistant, it waits up to ten minutes for "Ask ...", or until the notice is closed. A
click shows "... is looking at the ... crash" at once, asks the assistant (at most three minutes, in your home folder) and
replaces that notice with the answer, which stays until you close it. Then the process exits. The watcher itself is one
blocking read with no threads and no D-Bus connection: about 0.8 MB (PSS), 0.3 MB of it its own and 0.3 MB the journal
file's pages it reads.

## The assistant

The assistant is the system's `assistant` integration file (Yggdrasil's docs/INTEGRATIONS.md). The crash watch offers it only
when that file has a `question` command (the question is added as the last argument; it prints the answer; it should only
read). `crash_question` may say how to ask, with `{program}` and `{signal}` in it; without it, a plain question is used.

## What it misses

A crash whose core is not stored (`Storage=journal` or `none` in coredump.conf, a core over `ExternalSizeMax`, or a process
whose core size limit, `ulimit -c`, is 0) is seen through the journal since 2026-11-11, except where the journal is kept only
in memory (`Storage=volatile`, or no `/var/log/journal`): journald then keeps one system journal and no journal of your
own, so only the stored cores are seen. A crash while the watcher is not running is not shown later. A game that runs
through Proton/Wine usually reports its own errors without a Linux crash, so there is no core and no notice.

## Commands

    yggdrasil-crashwatch --dry-run              print "EXE crashed (SIGNAL)" for each new crash of yours instead of a notice
    yggdrasil-crashwatch --journal-dir DIR      follow DIR/user-UID.journal instead of your own journal file (tests)
    crashwatch-notice --print EXE SIGNAL        what a notice would say (and what the assistant would be asked)

## Why Rust: measured

It replaces Labyrinth OS's Python `labyrinth-crash-watch`, which followed the journal through a `journalctl -f` child.
Measured on 2026-11-11 with `tools/bench-port.py` against that script from git (the Labyrinth OS repository), both waiting
for 60 s after a 5 s start, on my desktop (16 threads); stand-ins for notify-send, and the Rust one with `--dry-run`, so a
crash meanwhile would show nothing. The numbers are from after it began to follow the journal too (the same day; the
computer was busier then, so the Python's CPU and wake-ups are higher than in the first run). Method:
[docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| memory while it waits (RSS) | 40.1 MB (Python and journalctl) | 4.0 MB (one process) | −90 %, 36.1 MB less |
| memory while it waits (PSS) | 22.9 MB | 0.8 MB | −97 %, 22.1 MB less |
| start (CPU in its first 5 s) | 49.7 ms | 0.61 ms | −99 %, 49.1 ms less |
| CPU per minute while it waits | 9.44 ms | 0.04 ms | −100 %, 9.40 ms less |
| wake-ups per minute while it waits | 170 | 2 | −99 %, 168 fewer |
| on disk | 3.8 KB script (+ Python, 75 MB, shared) | 359 KB, and 1.27 MB for the notice program | +1.6 MB |

Following the journal costs something: before it did (the cores only, measured back to back with the run above) the watcher
took 2.4 MB RSS and 0.3 MB PSS and woke 0 times a minute; now 4.0 MB, 0.8 MB and 2 (each time journald writes to your
journal file).

What else changed:

- Instead of reading every crash record in the journal through journalctl, it reads the program and the signal from the
  attributes systemd-coredump sets on the core file, and whose crash it is from the file's name. The journal is read only
  for the crashes whose core is not stored (since 2026-11-11), in its own process, from your current journal file only, and
  only entries systemd-coredump wrote (the script took the message from any program).
- Each notice runs in a short-lived process of its own, so the watcher holds no D-Bus connection; "Ask ..." is offered only
  when the system's assistant can be asked.
- Tests: 13 unit, 3 on the watcher (inotify, attributes, the user filter, the rate limit), 1 on a journal file (written in
  place, journald's rotation, the user and `_EXE` filters, one notice for a crash told of twice) and 1 on a private D-Bus.
