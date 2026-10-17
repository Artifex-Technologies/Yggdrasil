# Users page and the keyring sync tool

Two pieces that keep the login password and gnome-keyring's login keyring together (see DESKTOP_ROADMAP.md, "Keeping the keyring password ...").

## Settings > Users (`packages/settings/src/users.rs`, `page_users` in main.rs)
- Shows who you are (name, login, administrator or standard, home folder) and the state of the keyring (unlocked / locked / not running).
- **Change password**: current, new, new again, a strength bar, the usual checks (8 or more characters, different from the current one, both new ones equal).
  It changes the password through the system's own `passwd`, run in a terminal of its own (`script -qefc passwd /dev/null`) and answered question by question.
  That is the point: `passwd` goes through PAM with the old and the new password, so `pam_gnome_keyring` (the `-password optional` line in
  `/etc/pam.d/passwd`) changes the keyring's password with it. KDE's Users page uses AccountsService, which has no old password and would leave the keyring behind.
  Failures are put in plain words ("The current password is not right.", "The new password is too weak: ...", "do not match").
- Tested with a stand-in `passwd` (right answers, wrong current password, weak new password); the real `passwd` was never run by the tests.
- **Not built:** adding and removing other accounts, changing the full name or the account type (they need administrator actions through polkit).

## labyrinth-keyring-sync (`packages/keyring-sync`)
- Runs at login (`/etc/xdg/autostart/labyrinth-keyring-sync.desktop` in the profile) and does nothing unless the login keyring is locked.
- If it is locked (an administrator reset the password, or a tool changed it without PAM): a small window asks for the previous password and the current one.
  The current one is checked with PAM first, then the keyring's password is changed to it with gnome-keyring's `ChangeWithMasterPassword` (the call Seahorse uses)
  and the keyring is opened. "I don't remember it" starts a new, empty login keyring (after the current password is checked): the old file is moved aside as
  `login.keyring.old-<time>`, not deleted.
- `--check` prints the state; `--cli` (tests only) changes the password from `LABYRINTH_KEYRING_OLD` / `LABYRINTH_KEYRING_CURRENT`.
- Tested end to end against a private gnome-keyring (`tests/test_keyring_sync.sh`, run inside `dbus-run-session`): a keyring locked with an old password; a wrong
  previous password is refused; the right one changes it and an item stays intact; the new password opens it and the old one no longer does.
- The window itself was looked at on the real screen (against a private test keyring). The "start new keyring" path is written but not exercised.
- Both are built but not installed system-wide on this machine (release builds of the egui crates take minutes): `cargo build --release` in each folder, then install to `/usr/bin`.


## Other accounts (added 2026-10-16)

Settings > Users now lists the other accounts (uid 1000 and up, without the audited `claude-agent` account) and can add one (full name, user name,
standard or administrator, password with strength meter), make one an administrator or standard, and remove one (home folder kept unless
you tick "also delete their files"). Each change asks for an administrator password (polkit action `org.labyrinth.account.manage`, always
asks). The work is done by `labyrinth-account` (`/usr/local/bin`, run with pkexec; `tests/test_labyrinth_account.py`): it validates names,
puts the password on standard input only (never a command line), and refuses the account that asked, `claude-agent`, logged-in accounts,
system users, and removing or demoting the last administrator. Not tested: a real run through pkexec (it needs your password); install:
`sudo install -m755 profile/airootfs/usr/local/bin/labyrinth-account /usr/local/bin/ && sudo install -m644 profile/airootfs/usr/share/polkit-1/actions/org.labyrinth.account.policy /usr/share/polkit-1/actions/`.
A new account's keyring is created at its first login, with its login password.
