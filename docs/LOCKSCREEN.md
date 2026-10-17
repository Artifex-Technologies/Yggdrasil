# Lock screen

Labyrinth has its own locker, `labyrinth-lock` (Rust, tiny-skia, PAM). It uses the Wayland
`ext-session-lock-v1` protocol, which KWin 6.8 does not offer. So today KDE's lock screen still
locks the session; `labyrinth-lock` takes over once Labyrinth runs on a compositor that offers the
protocol (the planned Smithay compositor).

## Designs

Pick one in Settings > Appearance > Lock screen, or set `design = split|classic|zen|poster` in
`~/.config/labyrinth/lock.conf`. Default: Split.

- **Split**: wallpaper on the left, clock bottom-left, frosted sign-in panel on the right.
  Inspired by the Split design in Omarchy's Lock Screen Explorer (MIT, (c) 2026 SirJul1337);
  it is redrawn natively, no QML or Quickshell code is used.
- **Classic**, **Zen**, **Poster**: our own.

## Previews

`labyrinth-lock --preview DESIGN --out FILE.png [--size WxH] [--typed N] [--message T] [--wallpaper P] [--user U]`
renders a design to a picture without touching Wayland or locking anything. Never test the locker
on a live KWin session; use a nested compositor.
