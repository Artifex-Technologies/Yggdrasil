# Settings' on-demand helpers (Rust)

The four helpers Settings starts when asked, in Rust, replacing the Python scripts in `packages/settings/helpers` at the
same paths with the same interface: commands, arguments, JSON, files, exit codes and messages.

| Helper | Installed as | For | Root |
| --- | --- | --- | --- |
| `account` | `/usr/lib/yggdrasil/account` | Settings > Users: list, groups, add, admin, remove, name, password, login, sudo, group | through pkexec (`list`, `groups`: no) |
| `bootscreen` | `/usr/lib/yggdrasil/bootscreen` | Appearance > Boot screen: get, set animation / messages / picture FILE | `set` through pkexec |
| `game-fix` | `/usr/lib/yggdrasil/game-fix` | Games > Game fixes: list [--json], apply, undo | no |
| `gpu-run` | `/usr/bin/yggdrasil-gpu-run` | Games: a program on a chosen graphics card (a Steam launch option) | no |

Each keeps its script's checks as they were: `account`'s names (1 to 31 of a-z 0-9 _ -, starting with a letter, not
reserved, not kept by the operating system), uid range, the caller's own account, logged-in accounts, the last administrator,
passwords only on chpasswd's standard input, the optional groups; `bootscreen`'s three plain picture formats (PNG, JPEG,
WebP: nothing that hands a file to another program) and its pixel limit; `gpu-run`'s card numbers (read as Python's int()
read them) and its exec (no shell, the PATH searched as Python's os.execvpe did).

Where a script stopped with a traceback, its helper stops too, with exit code 1 and one line on standard error.

What is not 1:1:

- `account`: a name with a final newline passed the script's pattern (Python's `$` matches before it) and went on to useradd,
  which refused it; here it is refused at once (exit code 2). The tools it runs get no standard input instead of the
  helper's own (which they never read).
- `bootscreen`: your own picture is decoded and scaled here instead of by Pillow: the same size (Pillow's thumbnail rule) and
  mode, pixels within a small difference (another scaling filter and PNG encoder). python-pillow is no longer needed.
- `game-fix`: for a catalogue that is not JSON, the parser's own words in the message differ.
- Text the scripts would have refused to decode from a tool's output (lspci, vulkaninfo, loginctl) is read with the bad
  bytes replaced instead.

## Tests and parity

`cargo test`: every test of the Python scripts carried over (account 14, bootscreen 6, game-fix 7, gpu-run 8), and more,
among them hostile input for `account` (names, full names, passwords and caller ids built to slip past a check).

`parity/compare.py` runs each helper and its script on the same made-up worlds, in dry runs: the system tools are
stand-ins that write down what they were asked, every file is in a scratch folder, and the root helpers run as "root" of a
user namespace of their own (`unshare -rm`, see `parity/sandbox.sh`). Exit codes, output, the tools' calls (with what they
read) and the files afterwards must be the same.

    cargo build --release && python3 parity/compare.py [--only account|bootscreen|game-fix|gpu-run] [-v]

## The Python scripts

They stay in `packages/settings/helpers` (after the switch, `helpers/python/`) with their tests, for a rollback, until
these have run a while.

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

More accurate:

- Parity: account 302, bootscreen 51, game-fix 466 and gpu-run 294 cases, all the same.
- Tests: the scripts' 35 carried over and 20 more, 8 of them hostile input for `account`.
- `account`: a name ending in a newline passed the script's pattern (Python's `$` matches before it) and went on to useradd;
  now it is refused at once.
- Where a script stopped with a traceback, the helper stops with exit code 1 and one line on standard error.
