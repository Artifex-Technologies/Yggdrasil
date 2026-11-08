# The installer

`yggdrasil-installer` installs an operating system from its live medium. Yggdrasil supplies the window (the same egui look as
Settings and Minos, no other toolkit) and, in the same package, `yggdrasil-install-system`, a program that does the work from an
archiso medium; an operating system uses it, or names a program of its own that keeps the contract below. The two meet in an
integration file (see INTEGRATIONS.md):

```
# /usr/share/yggdrasil/integrations/<os>-installer.conf
kind = installer
command = /usr/bin/yggdrasil-install-system
```

Without such a file the window says the system has no installer and offers nothing.

## yggdrasil-install-system

Since 2026-11-07 (it was Labyrinth's `labyrinth-install-system` until then; LykanOS needed the same one). It partitions (the
whole disk: a 1 GiB EFI partition and the rest; or the partitions chosen by hand), encrypts / with LUKS2 if asked (one password,
typed in GRUB), makes btrfs with the subvolumes `@`, `@home`, `@cache`, `@log` (ext4 or XFS by hand), copies the medium's system,
sets up language, keyboard, time zone, the computer's name and the account (an administrator through sudo; root stays locked),
switches on greetd, builds the start-up image and installs GRUB. What belongs to the OS comes from the same integration file and
`/etc/os-release`; every key is optional:

| Key | What it is | Without it |
| --- | --- | --- |
| `name` | the OS's name in messages, the boot menu, the EFI entry and the disk labels | `NAME` in os-release, less " OS" |
| `medium` | the squashfs to copy | `/run/archiso/bootmnt/<ID>/x86_64/airootfs.sfs` (`ID` from os-release, archiso's `install_dir`) |
| `cleanup` | a program run inside the new system first, taking away what belongs to the live medium (its account, its automatic sign-in, its no-password rules) | a warning: the new system keeps them |
| `finish` | a program run inside the new system last; a failure stops the install, a `WARNING:` line in its output becomes a note | nothing |
| `boot_menu_theme` | a program run inside after the plain boot menu is written | the plain menu |
| `kernel_options` | the kernel's options in GRUB | `quiet` |
| `groups` | the account's groups (those the system has) | users, lp, video, network, storage, wheel, audio, kvm |
| `reserved_logins` | account names not offered, beyond every account the medium already has and root, live and greeter | |
| `min_size_gb` | the smallest disk | 30 |
| `browsers`, `extras` | the lists for the Browsers and optional-parts pages (below; the window reads them too) | no such page |
| `components` | what each optional part installs and switches on | |
| `first_login` | where chosen parts the medium lacks are written down (`{"user", "components"}`) for the OS's own first-login program | they are left for Agora |

Paths are absolute, without `..`. Its tests: `python3 -m unittest discover -s helpers/tests` in `packages/installer`.

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
- Optional parts (Labyrinth: "Optional software", with the groups "AI and security" (Sarina, Heimdall) and "Devices" (Logitech,
  gaming mice, Razer, Corsair, Xbox controllers, HP printers)): the integration may name `extras = /usr/share/OS/installer-extras.json`,
  holding `{"title", "intro", "extras": [{"id", "name", "note", "group", "chosen" | "chosen_if_exists": "/absolute/path" |
  "chosen_if_usb": ["1532", ...]}, ...]}`. One page, titled by the file, shows each with a switch, under its group's heading (entries of
  a group listed together, in the file's order); an entry starts switched on when `chosen` is true, its `chosen_if_exists` file is on
  the live system (the medium carries it), or a USB device from one of its `chosen_if_usb` vendors (hex ids, read from
  /sys/bus/usb/devices/*/idVendor, compared without case) is plugged in. The plan carries `"extras": [ids chosen]`; with no file
  there is no page and no key. (Labyrinth installs what the medium does not carry at the first login, online.)

Modes: `erase` (the whole disk) and `manual` (partitions chosen by hand, Calamares-style). Still to come: shrinking a partition
to make room automatically ("install alongside"), and partitions on more than one disk in one install.

## How the program installs

`yggdrasil-install-system` checks the computer (UEFI, the disk not in use, at least `min_size_gb`), writes a GPT with a 1 GiB EFI
partition and the rest for the system, optionally LUKS2 (PBKDF2, because GRUB reads /boot from inside it; a key file in the
start-up image so the password is typed once, in GRUB), btrfs with `@`, `@home`, `@cache`, `@log`, copies the system from the
medium (`unsquashfs`), sets it up inside (the OS's `cleanup` program, language, keyboard, time zone, name, the account in `wheel`
with sudo; root stays locked; greetd as the login screen), builds the start-up image (`sd-encrypt` or `encrypt`, whichever the
system's mkinitcpio uses), installs GRUB (plus the firmware's fallback path) with the OS's `boot_menu_theme`, and runs its
`finish` program. Labyrinth's integration file names `labyrinth-install-cleanup`, `labyrinth-install-bootfinish` and
`labyrinth-grub-theme` for those.

Tested (as Labyrinth's program) in a QEMU/OVMF virtual machine from the 2026-10-23 ISO: whole disk unencrypted (boots to a login, the account, groups,
language, time zone and sudo as chosen) and encrypted (GRUB asks the password once). 2026-10-25: manual mode beside a
Windows-like layout (its EFI partition kept, a new 40 GB / in the free space, encrypted): the EFI partition was not formatted,
GRUB went into EFI/Labyrinth without a fallback copy, and the system started to the login screen after one password in GRUB.

Known limit: GRUB reads the encryption password with a US keyboard layout. With another layout (AZERTY, QWERTZ, Dvorak ...)
the same keys give other characters there. Since 2026-11-07 the disk page says so in amber under the password fields when the
chosen keyboard is not plain US. A GRUB keymap (`grub-kbdcomp`, with `ckbcomp` from CachyOS's repository) only works with GRUB's
`at_keyboard` input, which on many UEFI computers cannot read a USB keyboard at all: that would lock the person out, so it waits
for a test on real hardware.

Calamares was removed from Labyrinth's ISO on 2026-11-07; this installer is the only one on the medium.
