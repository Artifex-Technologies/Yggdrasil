# The installer

`yggdrasil-installer` installs an operating system from its live medium. Yggdrasil supplies the window (the same egui look as
Settings and Minos, no other toolkit); the operating system supplies the program that does the work. The two meet in an
integration file (see INTEGRATIONS.md):

```
# /usr/share/yggdrasil/integrations/<os>-installer.conf
kind = installer
command = /usr/local/bin/labyrinth-install-system
```

Without such a file the window says the system has no installer and offers nothing.

## The pages

Welcome (language) → Keyboard (with a box to try it) → Time zone (searchable; the live system's zone first, no location
service asked) → Disk (the whole disk, erased; encryption on by default, with its password twice) → Your account (name,
account name made from it, computer's name, password twice) → Summary (and a box to tick: the disk is erased) → Installing
(progress, notes, errors) → Done (Restart now). Next stays greyed out with the reason beside it until a page is complete.

## The contract with the OS's program

- `COMMAND --disks` prints the disks it could install onto, as JSON: `[{"path", "size", "model", "removable", "big_enough"}]`.
  It runs as the live user (no root needed) and never offers the live medium itself.
- `pkexec COMMAND` with the choices as one JSON object on standard input does the install, so no password is ever on a command
  line (where any local user could read it):

  ```json
  {"disk": "/dev/nvme0n1", "mode": "erase", "encrypt": true, "encrypt_password": "...",
   "user": {"name": "Ada Lovelace", "login": "ada", "password": "..."},
   "hostname": "ada-laptop", "timezone": "Europe/Berlin", "locale": "de_DE.UTF-8", "keymap": "de"}
  ```
- It reports one line each, and the window shows them: `STEP n N text` (step n of N started), `INFO text`, `WARN text` (shown
  as a note, the install goes on), `ERROR text` (it stopped), `DONE`.
- `COMMAND --check` (with the plan on stdin) checks the plan without changing anything.

Only `mode = erase` exists so far. Installing beside another system (free space, shrinking a partition) and choosing
partitions by hand come next.

## Labyrinth's program

`labyrinth-install-system` (Labyrinth repository, `profile/airootfs/usr/local/bin`) checks the computer (UEFI, the disk not in
use, at least 30 GB), writes a GPT with a 1 GiB EFI partition and the rest for the system, optionally LUKS2 (PBKDF2, because GRUB
reads /boot from inside it; a key file in the start-up image so the password is typed once, in GRUB), btrfs with `@`, `@home`,
`@cache`, `@log`, copies the system from the medium (`unsquashfs`), sets it up inside (Labyrinth's `labyrinth-install-cleanup`,
language, keyboard, time zone, name, the account in `wheel` with sudo; root stays locked; greetd as the login screen), builds
the start-up image (`sd-encrypt` or `encrypt`, whichever the system's mkinitcpio uses), installs GRUB (plus the firmware's
fallback path) with Labyrinth's menu theme, and runs `labyrinth-install-bootfinish`.

Tested in a QEMU/OVMF virtual machine from the 2026-10-23 ISO: whole disk unencrypted (boots to a login, the account, groups,
language, time zone and sudo as chosen) and encrypted (GRUB asks the password once).

Calamares stays on the medium as a fallback (`labyrinth-install --calamares`, or the launcher's second action) until this
installer has been used on real hardware.
