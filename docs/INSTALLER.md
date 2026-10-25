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

Welcome (language) → Keyboard (first the arrangement: QWERTY, QWERTZ, AZERTY, Dvorak, Colemak or other alphabets; then the
layouts of that kind by name, Dvorak and Colemak being variants; the live session switches to the one chosen, so the box on the
page tries it) → Time zone (searchable by place, without the old aliases; the live system's zone first, no location service
asked) → Disk (the disk, then how: erase all of it, or choose partitions by hand; encryption on by default, with its password
twice) → Your account (name,
account name made from it, computer's name, password twice) → Browsers (only when the OS offers a list: choose one or more,
and which is the default; none is allowed, with a note that Agora has them) → the optional parts (only when the OS offers some,
e.g. Labyrinth's AI and security: Sarina, Heimdall) → Summary (and a box to tick: the disk is erased) → Installing
(progress, notes, errors) → Done (Restart now). Next stays greyed out with the reason beside it until a page is complete.

## The contract with the OS's program

- `COMMAND --disks` prints the disks it could install onto, as JSON: `[{"path", "size", "model", "removable", "big_enough",
  "table", "partitions": [{"path", "number", "start", "size", "fstype", "label", "efi", "mounted"}], "free": [{"start", "size"}]}]`
  (bytes). It runs as the live user (no root needed) and never offers the live medium itself.
- `pkexec COMMAND` with the choices as one JSON object on standard input does the install, so no password is ever on a command
  line (where any local user could read it):

  ```json
  {"disk": "/dev/nvme0n1", "mode": "erase", "encrypt": true, "encrypt_password": "...",
   "user": {"name": "Ada Lovelace", "login": "ada", "password": "..."},
   "hostname": "ada-laptop", "timezone": "Europe/Berlin", "locale": "de_DE.UTF-8", "keymap": "de", "keymap_variant": ""}
  ```

  With partitions chosen by hand, `"mode": "manual"` and, on the chosen disk only:

  ```json
  {"new_table": false, "delete": [4],
   "create": [{"id": "p1", "start": 512110190592, "size": 214748364800, "kind": "linux"}],
   "use": [{"part": "/dev/nvme0n1p1", "mount": "/boot/efi", "format": null},
           {"part": "new:p1", "mount": "/", "format": "btrfs"}]}
  ```

  `delete` comes first, then `create` (bytes, on MiB boundaries, in free space; `kind` linux, efi or swap), then `use`: mount points
  /, /home, /var, /srv, /boot/efi and swap; `format` is the file system to make (btrfs, ext4, xfs; vfat for /boot/efi; swap) or
  null to keep what is there (never for /). Exactly one / (at least 20 GB) and one /boot/efi (at least 260 MB) are needed.
  Encryption applies to / only. An EFI partition that is kept (Windows' own, for one) gets the OS's boot loader beside what is
  on it, and its firmware fallback file is left alone.
- It reports one line each, and the window shows them: `STEP n N text` (step n of N started), `INFO text`, `WARN text` (shown
  as a note, the install goes on), `ERROR text` (it stopped), `DONE`.
- `COMMAND --check` (with the plan on stdin) checks the plan without changing anything.
- Browsers: the installer integration may name a list, `browsers = /usr/share/OS/installer-browsers.json`, holding
  `{"browsers": [{"id", "name", "note", "chosen"}, ...]}` (anything else in an entry, how to install it, is the OS's business).
  With a list, the plan also carries `"browsers": [ids chosen]` and `"default_browser": id`; without one there is no Browsers page
  and the plan says nothing about browsers.
- Optional parts (Labyrinth: "AI and security", Sarina and Heimdall): the integration may name `extras = /usr/share/OS/installer-extras.json`,
  holding `{"title", "intro", "extras": [{"id", "name", "note", "chosen" | "chosen_if_exists": "/absolute/path"}, ...]}`. One page,
  titled by the file, shows each with a switch; an entry starts switched on when `chosen` is true or its `chosen_if_exists` file is on
  the live system (the medium carries it). The plan carries `"extras": [ids chosen]`; with no file there is no page and no key.

Modes: `erase` (the whole disk) and `manual` (partitions chosen by hand, Calamares-style). Still to come: shrinking a partition
to make room automatically ("install alongside"), and partitions on more than one disk in one install.

## Labyrinth's program

`labyrinth-install-system` (Labyrinth repository, `profile/airootfs/usr/local/bin`) checks the computer (UEFI, the disk not in
use, at least 30 GB), writes a GPT with a 1 GiB EFI partition and the rest for the system, optionally LUKS2 (PBKDF2, because GRUB
reads /boot from inside it; a key file in the start-up image so the password is typed once, in GRUB), btrfs with `@`, `@home`,
`@cache`, `@log`, copies the system from the medium (`unsquashfs`), sets it up inside (Labyrinth's `labyrinth-install-cleanup`,
language, keyboard, time zone, name, the account in `wheel` with sudo; root stays locked; greetd as the login screen), builds
the start-up image (`sd-encrypt` or `encrypt`, whichever the system's mkinitcpio uses), installs GRUB (plus the firmware's
fallback path) with Labyrinth's menu theme, and runs `labyrinth-install-bootfinish`.

Tested in a QEMU/OVMF virtual machine from the 2026-10-23 ISO: whole disk unencrypted (boots to a login, the account, groups,
language, time zone and sudo as chosen) and encrypted (GRUB asks the password once). 2026-10-25: manual mode beside a
Windows-like layout (its EFI partition kept, a new 40 GB / in the free space, encrypted): the EFI partition was not formatted,
GRUB went into EFI/Labyrinth without a fallback copy, and the system started to the login screen after one password in GRUB.

Known limit: GRUB reads the encryption password with a US keyboard layout. With another layout (AZERTY, QWERTZ, Dvorak ...)
the same keys give other characters there; to be solved (a GRUB keymap, or a password typed the same way on both).

Calamares stays on the medium as a fallback (`labyrinth-install --calamares`, or the launcher's second action) until this
installer has been used on real hardware.
