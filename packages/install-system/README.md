# yggdrasil-install-system (Rust)

The program behind Yggdrasil's installer window: it installs the operating system from its live medium (archiso) onto a disk.
Installed as `/usr/bin/yggdrasil-install-system` (package `yggdrasil-installer`), named by an OS's installer integration file
(`command = /usr/bin/yggdrasil-install-system`). It replaces the Python script of the same name, which I keep in
`packages/installer/helpers/python/install-system` for a rollback, with the same interface:

- the plan as one JSON object on standard input (passwords never on a command line), `--check` (the plan only), `--disks`
  (the disks it could install onto, as JSON);
- the same lines out (`STEP n N text`, `INFO`, `WARN`, `ERROR`, `DONE`) and exit codes (0 done, 2 a refused plan, 1 stopped
  or not root);
- the same programs run with the same arguments in the same order (lsblk, wipefs, sfdisk, cryptsetup, mkfs.*, mount,
  unsquashfs, chroot ...), the same text on their standard input, and the same files written into the new system (fstab,
  hostname, hosts, locale.gen, locale.conf, vconsole.conf, the X11 keyboard, localtime, the sudoers drop-in, the start-up
  image's encryption settings, GRUB's settings, mimeapps.list, the dock, the first-login list).

docs/INSTALLER.md describes the plan, the steps and the contract with the window.

| File | What |
| --- | --- |
| `src/pyjson.rs` | the plan read as Python's json read it (whole numbers of any size, NaN, a key given twice), and written back as json.dumps and str() wrote it |
| `src/plan.rs` | the checks, every message as the script had it |
| `src/conf.rs` | the integration file and os-release, and the OS's values |
| `src/disks.rs` | `--disks` and the check of the chosen disk (lsblk), free space |
| `src/catalog.rs` | the browsers, optional parts and components lists |
| `src/install.rs` | the steps |
| `src/sys.rs` | running programs; the tests put a made-up computer in its place |

## Fixed in both

Before the port the script's checks were made whole, in the script as well (so the two can be compared value for value); each
of these let a plan through that the window never sends:

- names, language and keyboard with a final line break ("ada\n" passed, then useradd refused it after the disk was erased);
- a password with a line break (chpasswd reads one "login:password" per line: the rest could set root's password) or a NUL;
- a list as a password (its length passed the 8-character check), numbers or null where text belongs (tracebacks);
- `true` as a partition number (counted as partition 1, given to sfdisk as "True"), a number as a new partition's id (the
  install stopped after making the partition), `"new_table": "false"` (text is true: every partition erased);
- an absolute time zone, text that is not Unicode, a plan nested deeper than the stack;
- the OS's lists: an entry with a non-text id or a field of the wrong kind stopped the install;
- an absolute resolv.conf link with `..` in it could lead outside the new system.

A number given twice in `delete` is deleted once, and the partitions to delete are checked in order (a set's order decided which
problem was named first).

## What is not 1:1

- Where the script stopped with a traceback (outside the install: `--disks` when lsblk fails or prints something else than it
  expected, a `min_size_gb` int() cannot read), this one stops with exit code 1 and one line on standard error. Inside the
  install both say `ERROR unexpected: ...` and unmount.
- After `unexpected:`, the words for output that is not JSON from lsblk or sfdisk are my own (json's name a line and column;
  the real programs always print JSON), and for output of unsquashfs that is not UTF-8 the position counts from the line (the
  script's from its 8 KB read).
- Whole numbers beyond 10^30 are worked with as 10^30 (their text is kept for messages): no disk is that big.
- The OS's own lists with a lone surrogate or more than 512 levels are read as no list (the script would have read them).
- A new partition's name from sfdisk that is not text stops the install when it is read, not when it is first used.

## What is compared only at the level of the commands

Nothing here runs a real disk tool. What a command does to a real disk (partitioning, LUKS, file systems, GRUB on the
firmware) is compared as the exact command, arguments and standard input both programs run; that those commands install a
working system I check in a virtual machine (docs/INSTALLER.md).

## Tests and parity

`cargo test`: the script's tests carried over, the install on a made-up computer in the test itself (tests/fake; nothing is
started), where the passwords go (only chpasswd's and cryptsetup's standard input), and property tests with random plans: the
checks never panic, a refused plan runs no program, and no plan makes a disk tool name anything but the chosen disk, its
partitions and the mapper (with Windows on another disk beside it). `PROPTEST_CASES=20000 cargo test --release` for more.

`parity/compare.py` runs this program and the script on the same made-up computers, each in a user and mount namespace of its
own (`unshare -rm`, `parity/sandbox.sh`): /dev is an empty tmpfs, /mnt, /run and /sys/firmware are tmpfs, and every program
they run is a stand-in (`parity/fake.py`) that writes down its arguments and answers from a made-up computer with disks such as
/dev/vdz. Exit code, output, every call with its standard input, the files left behind and the made-up computer's state must
be the same; the plan's passwords must appear nowhere but on chpasswd's and cryptsetup's standard input.

    cargo build --release && python3 parity/compare.py [--fuzz 2000] [--fuzz-install 60] [--seed N] [--only TEXT] [-v] [--show]

`fuzz/` is a cargo-fuzz target for the plan (`cargo +nightly fuzz run plan`): any bytes, read, checked and given to the whole
program on a computer that runs nothing. Without cargo-fuzz it builds with stable Rust and libFuzzer's flags:

    cd fuzz && RUSTFLAGS="-Cpasses=sancov-module -Cllvm-args=-sanitizer-coverage-level=4 -Cllvm-args=-sanitizer-coverage-inline-8bit-counters \
      -Cllvm-args=-sanitizer-coverage-pc-table -Cllvm-args=-sanitizer-coverage-trace-compares --cfg fuzzing -Cdebug-assertions -Coverflow-checks" \
      cargo build --release --target x86_64-unknown-linux-gnu --target-dir /tmp/fuzz-target
    /tmp/fuzz-target/x86_64-unknown-linux-gnu/release/plan -max_total_time=300 CORPUS_FOLDER
