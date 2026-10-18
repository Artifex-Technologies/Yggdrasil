# Login screen (replaces SDDM)

`labyrinth-greeter` (built from `packages/lock`, next to the lock screen) draws the login screen with the same designs as the lock
screen (Split by default). It talks to **greetd**, which runs PAM and starts the session; the greeter only passes along what was typed.

- Shown: the account's name and picture (AccountsService icon, else the initial), a password field, the clock.
- Keys: type the password and press Enter; **Tab / Down** and **Shift+Tab / Up** switch account (only while nothing is typed); Escape clears.
- After a wrong password nothing is revealed beyond "Wrong password"; from the fourth in a row the screen makes you wait (2 s doubling to 60 s).
- Settings: `/etc/labyrinth/greeter.conf` (`design`, `session`, `wallpaper`).
- Accounts listed: user ids 1000 and up with a real shell.
- A second question from PAM (a changed-password prompt, a second factor) is refused with a message; use a terminal for those.

## Status (2026-10-18)
- Built and **tested in a nested Hyprland window with a stand-in greetd** (a Python server speaking the same protocol): a wrong password
  shows "Wrong password" and cancels the session; the right password sends `start_session` with the configured command and the greeter
  exits 0. The protocol part (`greetd.rs`) has unit tests (right password, wrong password, info messages, a second question, a dead connection).
- **Not tested with the real greetd**, and not enabled here: SDDM is still the login manager on this machine. The files are ready:
  `/etc/greetd/config.toml` (starts `cage -s -- labyrinth-greeter` as the `greeter` user), `/etc/pam.d/greetd` (the keyring unlocks at
  sign-in like with SDDM) and `/etc/labyrinth/greeter.conf`.
- To try it in a virtual machine (Limbo): `pacman -S greetd cage`, `systemctl disable sddm`, `systemctl enable greetd`, restart.
  A way back: `systemctl disable greetd; systemctl enable sddm` from a text console (Ctrl+Alt+F3).
- Not done: the session list (Labyrinth's own session once it exists), power buttons, an on-screen keyboard, the accessibility settings of
  [ACCESSIBILITY.md](ACCESSIBILITY.md), several screens, and a picture for accounts that have none in AccountsService.
