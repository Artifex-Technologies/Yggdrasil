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
