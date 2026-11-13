# Login screen (replaced SDDM)

**Status 2026-11-04:** this is the login screen on the owner's PC: greetd is enabled and starts `yggdrasil-greeter` in cage, the
session it starts is `yggdrasil` (the Yggdrasil session on Hyprland), and SDDM is not installed (KDE is gone since 2026-10-24).

`yggdrasil-greeter` (built from `packages/lock`, next to the lock screen) draws the login screen with the same designs as the lock
screen (Split by default). It talks to **greetd**, which runs PAM and starts the session; the greeter only passes along what was typed.

- Shown: the account's name and picture (AccountsService icon, else the initial), a password field, the clock.
- Keys: type the password and press Enter; **Tab / Down** and **Shift+Tab / Up** switch account (only while nothing is typed); Escape clears.
- After a wrong password nothing is revealed beyond "Wrong password"; from the fourth in a row the screen makes you wait (2 s doubling to 60 s).
- Settings: `/etc/yggdrasil/greeter.conf` (`design`, `session`, `wallpaper`). `session` names a file in `/usr/share/wayland-sessions`
  (e.g. `yggdrasil`); F2 on the login screen picks another when more are installed. The greeter starts its `Exec` and gives greetd the session's environment (`XDG_SESSION_TYPE=wayland`,
  `XDG_CURRENT_DESKTOP` and `XDG_SESSION_DESKTOP` from `DesktopNames`), as SDDM does.
- Accounts listed: user ids 1000 and up with a real shell.
- A second question from PAM (a changed-password prompt, a second factor) is refused with a message; use a terminal for those.

## Several screens (2026-11-13)

The sign-in (clock, account, password) is on the **main display** only; every other screen is plain black, in the same frame.

- Why it was at the far right: cage (`-m extend`, its default) puts all screens side by side and makes the one full-screen
  window as big as all of them together. The greeter drew one picture over that whole space, and the Split design puts its
  sign-in panel at the right edge, so it landed on the rightmost screen.
- Now the window is black (one black pixel stretched over it, wp_single_pixel_buffer + wp_viewporter; no memory) and the sign-in
  is a subsurface of its own, the main display's size, placed over that screen (`packages/lock/src/screens.rs`). The places
  come from xdg-output, the names, makers and models from wl_output; screens plugged in or out while it is up are followed.
  A compositor that keeps the window on one screen gets the sign-in over the whole window, as before.
- No screen is switched off: black only. Turning outputs off is what crashed Hyprland on NVIDIA (a flip timeout), and cage
  has no output-power protocol; switching one off through wlr-output-management would be a full modeset at the login screen.
- **Which screen is main** is the latest sign-in's choice in Settings > Displays. The greeter runs as `greeter` and cannot read
  home folders, so the panel hands the choice over at each sign-in and whenever it changes: `pkexec
  /usr/lib/yggdrasil/greeter-display MAKE MODEL CONNECTOR` (polkit action `org.yggdrasil.greeter.main-display`: yes without a
  password for an account signed in at this computer, no for anyone else, so it never asks; only run when the copy differs).
  The helper checks the values and writes `/var/lib/yggdrasil/greeter/display.conf` (root's, 0644; the folder 0755): the
  monitor's maker, model and connector, nothing else (no serial, nothing about the account). A sign-in without a choice
  leaves the file as it is. Without the file, or when the chosen monitor is not connected, the top-left screen is main, as
  for the panel.
- The greeter treats the file as untrusted: read without following links, only when root owns it and nobody else can write
  it, at most 1 KiB, every line a comment or one of the three keys once, with names of 1 to 64 printable characters and a
  connector of letters, digits and `-`. Anything else and the whole file is ignored (`yggdrasil_conf::display::parse_system`).
- The screens are not moved to the places in monitors.lua at the login screen: with all but the main one black nothing shows
  where they are (the greeter takes no pointer), and placing them would be an output configuration (a commit to the graphics
  driver) at the login screen, the kind of change avoided on NVIDIA.
- Tests: the main screen from made-up screen lists (no choice, a choice, a choice not connected, odd places), the copy's
  parser with hostile input, the helper's writing, and `packages/lock/tests/greeter_screens.rs`: the greeter in cage with
  three headless screens, pixels checked (sign-in on exactly one screen, the others all black), in CI too.

## Status (2026-10-23)
- **Run with the real greetd** on this machine (a test instance on VT 8, greetd 0.10.3, cage 0.3.1): greetd opened the greeter's PAM
  session for the `greeter` account, cage and the greeter started and stayed up. Note: greetd switches to its terminal by itself
  (`switch = true` is its default), so a test instance takes the screen. Signing in through it, and the switch itself, are a step
  in the owner's to-do script (with `back-to-sddm.sh` as the way back). It was done: greetd is the login manager now and SDDM has been
  removed with KDE, so that way back no longer applies.

## Status (2026-10-18)
- Built and **tested in a nested Hyprland window with a stand-in greetd** (a Python server speaking the same protocol): a wrong password
  shows "Wrong password" and cancels the session; the right password sends `start_session` with the configured command and the greeter
  exits 0. The protocol part (`greetd.rs`) has unit tests (right password, wrong password, info messages, a second question, a dead connection).
- **Not tested with the real greetd**, and not enabled here: SDDM was still the login manager on this machine then. The files are ready:
  `/etc/greetd/config.toml` (starts `cage -s -- yggdrasil-greeter` as the `greeter` user), `/etc/pam.d/greetd` (the keyring unlocks at
  sign-in like with SDDM) and `/etc/labyrinth/greeter.conf` (now `/etc/yggdrasil/greeter.conf`).
- To try it in a virtual machine (Limbo): `pacman -S greetd cage`, `systemctl disable sddm`, `systemctl enable greetd`, restart.
  A way back: `systemctl disable greetd; systemctl enable sddm` from a text console (Ctrl+Alt+F3).
- Not done: power buttons, an on-screen keyboard, the accessibility settings of
  [ACCESSIBILITY.md](ACCESSIBILITY.md), and a picture for accounts that have none in AccountsService.
