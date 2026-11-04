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
