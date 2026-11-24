# Lock screen

Yggdrasil has its own locker, `yggdrasil-lock` (Rust, tiny-skia, PAM). It uses the Wayland
`ext-session-lock-v1` protocol, which Hyprland offers, so it is the lock screen of the Yggdrasil
session today: when logind asks the session to lock (Super+L, `loginctl lock-session`, the idle
timer) or the machine is about to sleep, `yggdrasil-idle` starts it. (Under KDE it could not be
used, because KWin 6.8 did not offer the protocol; KDE is gone since 2026-10-24.) Yggdrasil's own
compositor offers the protocol too (COMPOSITOR.md).

## Designs

Pick one in Settings > Appearance > Lock screen, or set `design = split|classic|zen|poster` in
`~/.config/yggdrasil/lock.conf`. Default: Split.

- **Split**: wallpaper on the left, clock bottom-left, frosted sign-in panel on the right.
  Inspired by the Split design in Omarchy's Lock Screen Explorer (MIT, (c) 2026 SirJul1337);
  it is redrawn natively, no QML or Quickshell code is used.
- **Classic**, **Zen**, **Poster**: our own.

## Previews

`yggdrasil-lock --preview DESIGN --out FILE.png [--size WxH] [--typed N] [--message T] [--wallpaper P] [--user U]`
renders a design to a picture without touching Wayland or locking anything. Never test the locker
on the live session; use a nested compositor.

## Safety cases

What happens when something goes wrong while the screen is locked, and what shows it:

| Case | What happens | Shown by |
| --- | --- | --- |
| The locker crashes | On the own compositor the session stays locked and nothing of it shows; a new locker can take over and unlock (`yggdrasil-idle` starts one again). On Hyprland this is Hyprland's own `ext-session-lock` handling | `a_locker_that_dies_leaves_the_screen_locked` (packages/compositor/tests/behaviour.rs) |
| Several screens | Every screen is covered and all input goes to the lock until it unlocks; the real lock screen on two screens unlocks with the password | `the_lock_covers_every_output_and_takes_all_input_until_unlocked` (behaviour.rs), `the_lock_screen_covers_both_outputs_and_unlocks_with_the_password` (apps.rs) |
| Only the media keys | Volume and playback keys still work; no other shortcut reaches the session | `only_the_media_keys_work_on_the_lock_screen` (behaviour.rs) |
| A VT switch (Ctrl+Alt+F3) | The lock is the compositor's state, so switching away and back returns to the lock; the other VT is a separate login | not checked yet (by hand, on a real screen) |
| Suspend and resume | `yggdrasil-idle` locks before the computer sleeps; after resume the lock is up before anything of the session shows | not checked yet (by hand, on a real screen) |

