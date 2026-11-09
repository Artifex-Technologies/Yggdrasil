# Leak watch

Warns when a program's memory keeps growing for hours, which is what a memory leak looks like from outside. It watches every
program, ours and others' (browsers, Sunshine, system services), in any language.

## Why

On 2026-11-09 the panel had left 819 of its old images with Hyprland: about 4 GB after a day and a half. The panel itself
looked small (121 MB), because the memory was counted in Hyprland, and Hyprland is not ours. So the leak watch looks in two
places:

- **Each program**: its memory in RAM plus what of it was pushed out to swap (`VmRSS` + `VmSwap` in `/proc/PID/status`), so a
  leak that is being swapped out still counts.
- **What each program left with the compositor**: Hyprland's shared memory, grouped by the name the program gave its buffers
  (`memfd:yggdrasil-panel`, from Hyprland's `/proc/PID/smaps`). A program that drops images without telling the compositor
  shows up here and nowhere else.

## How it decides

A timer runs `yggdrasil-leakwatch sample` 10 minutes after login, then every 5 minutes. It takes about 12 ms, then exits:
nothing stays in memory. History is kept for 26 hours in `~/.local/state/yggdrasil/leakwatch/state.json`.

A program is followed once it uses 20 MB (8 MB for buffers left with the compositor). It is a suspect when, over at least 4 hours:

- its lowest reading of each hour climbs (or holds) in at least three hours out of four, and
- the last hour's low is at least 256 MB higher (128 MB for buffers) and at least half again the first hour's low.

Hourly lows ignore short peaks: a game loading a level, or a build, comes back down and is not a leak. A program that is
restarted starts a fresh history.

A suspect gets one notification ("Memory keeps growing: …" or "… is not giving back its images"), then none for the same
program for a day. The leak watch never stops or restarts anything.

## Commands

    yggdrasil-leakwatch report           what is followed: then, now, hours, growth per hour, suspects marked
    yggdrasil-leakwatch report --json    the same for programs (Task Manager)
    yggdrasil-leakwatch sample --dry-run one sample, printed, nothing saved or sent

## Settings

`~/.config/yggdrasil/leakwatch.conf`:

    # programs that may grow on purpose: no warning about them
    ignore = vivaldi-bin, ollama

Turn it off for one account: `systemctl --user disable --now yggdrasil-leakwatch.timer`.
