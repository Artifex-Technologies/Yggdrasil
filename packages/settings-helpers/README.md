# Settings' on-demand helpers (Rust)

The helpers Settings starts when asked, in Rust, replacing the Python scripts in `packages/settings/helpers` at the same
paths with the same interface: commands, arguments, JSON, files, exit codes and messages.

| Helper | Installed as | For | Root |
| --- | --- | --- | --- |
| `account` | `/usr/lib/yggdrasil/account` | Settings > Users: list, groups, add, admin, remove, name, password, login, sudo, group | through pkexec (`list`, `groups`: no) |
| `bootscreen` | `/usr/lib/yggdrasil/bootscreen` | Appearance > Boot screen: get, set animation / messages / picture FILE | `set` through pkexec |
| `game-fix` | `/usr/lib/yggdrasil/game-fix` | Games > Game fixes: list [--json], apply, undo | no |
| `gpu-run` | `/usr/bin/yggdrasil-gpu-run` | Games: a program on a chosen graphics card (a Steam launch option) | no |
| `gpuunlock` | `/usr/lib/yggdrasil/gpuunlock` | Power > Graphics card unlock: status, arm, disarm, reenable, release, boot, verify, bench; `grub-entry` for `/etc/grub.d/41_gpu_unlock` | through pkexec and its two services (`status`, `bench`: no) |

Each keeps its script's checks as they were: `account`'s names (1 to 31 of a-z 0-9 _ -, starting with a letter, not
reserved, not kept by the operating system), uid range, the caller's own account, logged-in accounts, the last administrator,
passwords only on chpasswd's standard input, the optional groups; `bootscreen`'s three plain picture formats (PNG, JPEG,
WebP: nothing that hands a file to another program) and its pixel limit; `gpu-run`'s card numbers (read as Python's int()
read them) and its exec (no shell, the PATH searched as Python's os.execvpe did); `gpuunlock`'s exact identities (device,
board, VBIOS, driver series; no wildcards), its blockers (the module, its firmware, its start-up hook, a visible boot menu,
a full locked baseline of this card), the one-try entry copied only from one plain Linux entry, and rule 8 for an unlock.

`gpuunlock` runs as root. It runs no shell: every program by argv (modinfo, snapper, grub-mkconfig, systemctl, logger,
nvidia-smi, journalctl, nvcc and the benchmark it builds), with no standard input. Every file in its state folder is
written whole (a new file renamed over the old one) and never through a link. The grub.d fragment stays a few lines of sh
because grub-mkconfig runs with `set -e`: a fragment that fails stops a kernel update from writing the boot menu. It calls
`gpuunlock grub-entry` and always exits 0; the program checks entry.cfg (a plain file, not a link, UTF-8, one entry of ours,
one kernel line ending in the option for one PCI address, nothing that opens another entry, a final newline) and prints it
in one piece, or prints nothing.

Where a script stopped with a traceback, its helper stops too, with exit code 1 and one line on standard error.

What is not 1:1:

- `account`: a name with a final newline passed the script's pattern (Python's `$` matches before it) and went on to useradd,
  which refused it; here it is refused at once (exit code 2). The tools it runs get no standard input instead of the
  helper's own (which they never read).
- `bootscreen`: your own picture is decoded and scaled here instead of by Pillow: the same size (Pillow's thumbnail rule) and
  mode, pixels within a small difference (another scaling filter and PNG encoder). python-pillow is no longer needed.
- `game-fix`: for a catalogue that is not JSON, the parser's own words in the message differ.
- Text the scripts would have refused to decode from a tool's output (lspci, vulkaninfo, loginctl, and gpuunlock's
  programs) is read with the bad bytes replaced instead.
- `gpuunlock`, stricter than its script (so not in the differential):
  - a JSON file with NaN or Infinity in it (which Python's json reads, and which is not JSON) is not read: a state file is
    taken as missing, a benchmark result as "gave no result". Numbers beyond 64 bits are read as floats.
  - Ollama's port: one plain request, no proxy from the environment, no redirect followed, at most 1 MiB read, UTF-8 only.
    Anyone may listen on that port when Ollama does not; the script followed redirects to anywhere.
  - entry.cfg must be UTF-8 with no NUL for `grub-entry` to print it; the shell fragment's grep took either, depending on
    the locale grub-mkconfig ran in.
  - names in /sys/bus/pci/devices that are not UTF-8 are passed over (the kernel never makes them).

## Tests and parity

`cargo test`: every test of the Python scripts carried over (account 14, bootscreen 6, game-fix 7, gpu-run 8), and more,
among them hostile input for `account` (names, full names, passwords and caller ids built to slip past a check).

`parity/compare.py` runs each helper and its script on the same made-up worlds, in dry runs: the system tools are
stand-ins that write down what they were asked, every file is in a scratch folder, and the root helpers run as "root" of a
user namespace of their own (`unshare -rm`, see `parity/sandbox.sh`). Exit codes, output, the tools' calls (with what they
read) and the files afterwards must be the same.

    cargo build --release && python3 parity/compare.py [--only account|bootscreen|game-fix|gpu-run] [-v]

`parity/gpuunlock.py` does the same for `gpuunlock` and its grub.d fragment, more strictly: each world is a whole file
system of its own (`parity/gpuunlock-sandbox.sh`: `unshare -rmnpf`, then pivot_root into the world), with a made-up /sys,
/proc, /etc/default/grub, /boot/grub/grub.cfg, state folder, support table and firmware, stand-ins for all its programs
(the stand-in grub-mkconfig runs the world's fragment) and for Ollama's port, as root and as a user. Exit codes, standard
output and error, every call with its arguments in order, and every file with its mode must be the same; only the time of
day is put aside.

    cargo build --release && python3 parity/gpuunlock.py [--git REV] [--only TEXT] [-v]

On 2026-11-13: 489 of 489 cases the same (status in every state and on hostile /sys, /proc and tables, every blocker,
arming and each way it is refused or put back, links and folders in the state folder, Ollama's port answering 19 ways,
disarm, reenable, release, the attempt across starts, verify through the benchmark and the heat watch, bench and its
baseline, and 34 shapes of entry.cfg through both fragments).

## The Python scripts

They stay in `packages/settings/helpers/python/` with their tests, for a rollback, until these have run a while
(`gpuunlock` and its shell fragment until 2026-11-20).

## Why Rust: measured

Measured on 2026-11-11 with `tools/bench-port.py` against the Python scripts from git, on the parity script's made-up
worlds (stand-in tools, scratch files; `bootscreen set` as root of a user namespace of its own), on my desktop (16 threads).
Method and every case: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| `account list`, wall time | 23.7 ms | 0.50 ms | −98 %, 23.2 ms faster |
| `account add` (stand-in useradd and chpasswd), wall time | 31.3 ms | 6.91 ms | −78 %, 24.3 ms faster |
| `bootscreen get`, wall time | 28.3 ms | 3.00 ms | −89 %, 25.3 ms faster |
| `game-fix apply` (Dead Space 2), wall time | 27.4 ms | 1.76 ms | −94 %, 25.6 ms faster |
| `bootscreen set picture` (a 12-megapixel photo), wall time | 1132 ms | 815 ms | −28 %, 317 ms faster |
| `bootscreen set picture`, peak memory | 129.8 MB | 189.3 MB | **+46 %, 59.5 MB more** |
| `gpu-run list --json`, wall time | 32.1 ms | 9.14 ms | −72 %, 23.0 ms faster |
| `gpu-run 2 PROGRAM`, wall time | 30.3 ms | 10.5 ms | −65 %, 19.8 ms faster |
| a command's peak memory (the others) | 13.5–15.0 MB | 2.5–4.6 MB | −66 to −82 % |
| on disk | 46 KB of scripts (+ Python, 75 MB, and python-pillow, 4.9 MB) | 3.8 MB (four binaries) | +3.8 MB, python-pillow no longer needed |

Where Rust is worse: `bootscreen` decodes and scales the whole photo in memory at once, so its peak is higher than
Pillow's. Until 2026-11-11 `gpu-run` was slower too (32.1 and 33.6 ms): it looked every 10 ms whether lspci and
vulkaninfo had ended, as `game-fix` did for the screen query (not part of these runs: they give the screen size). Both now
go on the moment the program ends; the `gpu-run` numbers above are from a run that day after the change, the rest from
the first run.

### gpuunlock (2026-11-13)

Measured on 2026-11-13 with `tools/bench-port.py --only gpuunlock` (30 runs of each, alternating, after two warm-ups),
against the Python script from git (ff5b6de, with the eight fixes made before the port), in the differential's made-up
worlds; the launcher runs inside the world, so only the program is measured. Python alone (`python3 -c pass`) takes
9.6 ms and 9.6 MB on the same machine.

| Case | Python | Rust | Change |
| --- | --- | --- | --- |
| `status --json` on a supported card (what Settings > Power runs; modinfo run), wall time | 55.3 ms | 1.69 ms | −97 %, 53.6 ms faster |
| the same, peak memory | 23.6 MB | 3.9 MB (the modinfo stand-in's) | −83 % |
| `status --json` with no NVIDIA card (most computers), wall time | 52.5 ms | 0.49 ms | −99 %, 52.0 ms faster |
| the same, peak memory | 23.4 MB | 2.6 MB | −89 %, 20.8 MB less |
| `boot`, the start that is the attempt (writes trying, runs logger), wall time | 54.8 ms | 1.67 ms | −97 %, 53.2 ms faster |
| `verify` on a normal start while armed, wall time | 52.8 ms | 0.48 ms | −99 %, 52.3 ms faster |
| the fragment while armed (entry.cfg checked and printed), wall time | 6.41 ms (sh, head, tail, grep x6, cat) | 1.49 ms (sh and gpuunlock) | −77 %, 4.92 ms faster |
| the fragment with nothing armed (every grub-mkconfig), wall time | 0.89 ms | 1.44 ms | **+62 %, 0.55 ms slower** |
| on disk | 37.4 KB script + 1.6 KB fragment (and Python, which the package no longer needs) | 693 KB binary + 0.9 KB fragment | |

Where Rust is worse: with nothing armed, the old fragment ended at its first test, and the new one starts the program to
find there is no file. Half a millisecond per kernel update. yggdrasil-settings no longer depends on Python at all.

More accurate:

- Parity: account 302, bootscreen 51, game-fix 466 and gpu-run 294 cases, all the same; gpuunlock 489 of 489 (2026-11-13).
- Tests: the scripts' 35 carried over and 20 more, 8 of them hostile input for `account`; gpuunlock's 47 Python tests
  (the three PowerHold ones are powerlimit's, in packages/power-helpers) carried over with the fragment's, in 51 Rust tests
  with the runner, Ollama's port and Python's number reading.
- `gpuunlock`: eight bugs found while listing its behaviours, fixed in the script and its fragment first (Yggdrasil
  ff5b6de, each with a test that failed first): a boot menu that was not UTF-8 after grub-mkconfig stopped arming with the
  one-try entry left in the menu and no power hold; the state files were written through links (entry.cfg not even
  atomically); `GRUB_TIMEOUT=²` stopped status; the exact-identity patterns took a final newline; a benchmark rate of "nan"
  or "inf" was judged an unlock; a power limit of "nan" passed as the default watts; something that is not HTTP on Ollama's
  port stopped arming; the fragment printed an entry with no final newline (gluing grub-mkconfig's "### END" line to its
  brace) and printed through a link.
- The differential also found that serde_json read some numbers one bit off (gpuunlock's ratio, written and read back):
  `packages/pycompat` now reads them as Python does (`float_roundtrip`).
- `account`: a name ending in a newline passed the script's pattern (Python's `$` matches before it) and went on to useradd;
  now it is refused at once.
- Where a script stopped with a traceback, the helper stops with exit code 1 and one line on standard error.
