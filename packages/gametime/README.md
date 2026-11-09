# Game time

While a game runs, background updates and chores wait and catch up afterwards; security that watches keeps running. Games
and a local AI model share this computer, and every bit of the processor, disk and network should go to them. Which chores
wait and which never do, and why: [docs/GAME_TIME.md](../../docs/GAME_TIME.md).

Nothing stays running. systemd asks before a chore starts, and a timer catches up every 5 minutes.

## How it works

- Each chore that may wait has a drop-in, `ExecCondition=+/usr/bin/yggdrasil-gametime no-game %n`. With no game this exits 0
  and the chore runs as usual. During a game it notes the unit, logs one line in the chore's journal ("fstrim.service waits
  until the game is over: Steam is running a game (AppId 1091500, process 4210) …") and exits 100. systemd then skips that
  run: no failure, no restart.
- `yggdrasil-gametime catch-up` runs from a timer every 5 minutes, in the system manager and in each user's. When nothing
  waits it is a single stat. Once no game runs, it starts each waiting unit (`systemctl [--user] start --no-block`) and clears
  the list. The system's list is in `/run/yggdrasil-gametime/`, an account's in `$XDG_RUNTIME_DIR/yggdrasil-gametime/`; both are
  in memory and gone after a restart, when every chore's own timer runs it again anyway.
- The `+` runs the check outside the chore's sandbox. Several chores hide other users' processes (`ProtectProc=`), run as their
  own user, or make `/run` read-only (`ProtectSystem=strict`); inside that sandbox the check would see no game and could
  not note anything.
- The exit code is 100, not 1, on purpose: systemd lets a code from `SuccessExitStatus=` through as "run it", and the weather
  units list 1 there. Any trouble (a wrong command line, a list that cannot be written, a crash) lets the chore run now,
  so a chore is never lost.

## What counts as a game

It looks through `/proc` (each process's command line and kernel name) and starts no program to find out:

- **Steam**: the launcher Steam starts every game under, `reaper SteamLaunch AppId=N -- …` (the AppId is in the log line);
- **gamescope**;
- **Lutris**: `lutris-wrapper`, which runs each Lutris game (as a Python script, or renamed to `lutris-wrapper: TITLE`);
- **your own**: names in `/etc/yggdrasil/gametime.conf`, matched against the program's name or its `.exe`, any case:

      games = heroic, eldenring.exe, Cyberpunk2077.exe

Only the program itself is looked at (its first word and its kernel name), never its arguments. A `grep gamescope`, a
`pgrep -f "SteamLaunch AppId="`, a shell or an editor that only mentions these words is never taken for a game. That is how
Sarina's first check (pgrep) was fooled by its own pgrep. The check's own process is skipped too. Steam open with no game
running, and Lutris open on its library, do not count.

gamescope counts whenever it runs. A whole session inside gamescope (a console-style edition) would keep every chore waiting;
such an edition should rely on Steam's reaper instead.

## Commands

    yggdrasil-gametime status                is a game running, and why; what waits. Exit 0: a game, 1: none
    yggdrasil-gametime status --json         the same for programs: {"game", "reason", "kind", "pid", "detail", "waiting"}
    yggdrasil-gametime status --proc DIR     the same against a saved /proc tree (tests/fixtures/steam, …)
    yggdrasil-gametime no-game UNIT          for ExecCondition= (see above)
    yggdrasil-gametime catch-up [--user|--system]

## Virus definitions

ClamAV's `clamav-freshclam.service` is a daemon that keeps running and checks every 2 hours by itself, so it cannot wait for a
game. The package switches to ClamAV's own `clamav-freshclam-once.timer`. A drop-in makes it run every 2 hours (the daemon's
default), up to 10 minutes apart from other computers, and it waits during games like the other chores. The switch is made
when the package is installed or upgraded (the daemon disabled and stopped, the timer enabled and started); removing the
package puts the daemon back. ClamAV's own settings in `/etc/clamav` are not touched. Scanning (the downloads scan,
Heimdall's held files) keeps running in games. Only the download of new definitions waits.

## Tests

`cargo test`: the rules on command lines, fixture `/proc` trees in `tests/fixtures` (made by `tests/make-fixtures.sh`: Steam
running a game with a shell asking pgrep about it first, gamescope after a grep for it, Lutris, only mentions, an idle
desktop), our own process, zombies, the list, and the JSON.
