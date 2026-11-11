# Why Rust: measured

Every program I ported from Python to Rust, measured against the Python it replaced, on the same inputs. The short version
is in each package's README ("Why Rust: measured") and in the main README; this page has the method and every case.

## How these numbers were measured

`tools/bench-port.py` (Python, standard library only) runs the old and the new version of each program on the same inputs:

- **Old versions come from git**, not from the copies kept for a rollback: `git show COMMIT:PATH` into a scratch folder,
  the commit just before each port (table below). The numbers can be made again after the `python/` folders are gone.
- **New versions** are this repository's release builds (`cargo build --release` in each package, `target/release/`).
- **Same inputs, safe ones only**: `--help`, read-only commands, and the made-up worlds of each package's parity script
  (scratch folders, stand-in tools on the PATH, fake hwmon and sysfs trees with a stand-in graphics card, dry runs). The
  root helpers run against their tests' fake trees, never real hardware and never as root; where a helper must be root
  (`bootscreen set`, a `pkg-root` install), it is "root" of a user namespace of its own (`unshare -rm`), as its parity
  script runs it. No display, session bus or sound reaches any run; only app usage asks my own systemd for its running
  scopes (read only).
- **Alternating runs**: two warm-up runs of each (not counted), then old, new, new, old, ... 30 runs of each for the short
  commands, 10 for the long ones (drawing the cursors, a boot picture).
- **What is measured**: each run is started by a 40-line launcher (Rust, built by the script with rustc) that forks,
  execs the program and waits for it with `wait4`. Wall time is on the monotonic clock (the clock of
  `time.perf_counter`); CPU time is user + system, children included; peak memory is `ru_maxrss`, the largest single
  process of the run (a stand-in tool counts when it is larger than the program). The launcher is there because Linux
  keeps a process's peak memory across `exec`: a child forked straight from the 20 MB Python script reports at least
  20 MB, whatever it runs. Tables give the median, with the 10th to 90th percentile of wall time.
- **Services** (the power loops, app usage, the crash watch) run for 60 s in real time after a 5 s start: CPU time and
  context switches (wake-ups) in those 60 s, from every thread's `/proc/PID/task/TID/schedstat` and `status`, plus children
  already waited for; memory of the whole process tree at the end (RSS and PSS from `smaps_rollup`).
- **Size on disk**: the stripped release binary against the script. A script also needs the interpreter, which is shared:
  Python 3.14 is 75 MB installed, and starting it alone (`python3 -c pass`) takes about 9 ms and 9.6 MB. Some scripts also
  needed python-pillow (4.9 MB) or python-gobject (1.5 MB).
- **Change** is (new − old) / old, so −93 % means the Rust one takes 93 % less; the absolute difference is next to it.
- Everything runs at `nice -n 19`, one process at a time (the whole run takes about ten minutes).

    cargo build --release      # in each ported package
    nice -n 19 python3 tools/bench-port.py [--only power,settings-helpers,appusage,crashwatch,pkg-root,theme,cursors,liki,install-system] [--json FILE] [-v]

Machine: my desktop, 16 threads, Linux 7.3, Python 3.14.9.

### The old versions

| Program | Old version (git) |
| --- | --- |
| tune, casefan, gpufan | `98e14bd:packages/settings/helpers/NAME` |
| powerlimit | `8b689f9:packages/settings/helpers/powerlimit` |
| account, bootscreen, game-fix, gpu-run | `725296c:packages/settings/helpers/NAME` |
| labyrinth-appusage | `725296c:packages/appusage/python/labyrinth-appusage` (the script from Labyrinth OS's image, kept by the port) |
| labyrinth-crash-watch | Labyrinth OS repository, `292a53b:profile/airootfs/usr/local/bin/labyrinth-crash-watch` |
| pkg-root | `0cff417:packages/market/helpers/pkg-root` |
| ygg-theme | `0cda95e:packages/theme/bin/ygg-theme` |
| ygg-cursors | `daf2334:packages/theme/bin/ygg-cursors` |
| liki-build, liki-icns, liki-icon-import | `1cd6211:packages/liki/bin/NAME` |
| yggdrasil-install-system | `a234048:packages/installer/helpers/install-system` (run as its shebang runs it, `python3 -I`) |

## Results (2026-11-11)

Every case of the run. The ygg-cursors and Liki cases are from a second run of those two alone: during the first one
another build was running on the computer, and their 10th to 90th percentiles were twice as wide. The medians moved (Liki:
up to 45 ms for Python and 6 ms for Rust; ygg-cursors: 26 and 49 ms), the conclusions did not. Where the Rust one is worse,
the package's README says why.

Four cases are from a later run the same day, after `gpu-run`, `labyrinth-appusage --once` and `ygg-theme apply` stopped
looking every 10, 20 or 5 ms whether the programs they start had ended and began to wait for them instead (a pidfd, in
`packages/pycompat`): `gpu-run list --json` 32.3 → 9.14 ms, `gpu-run 2 PROGRAM` 33.8 → 10.5 ms, `--once` 20.9 → 3.68 ms,
`apply` 21.7 → 3.73 ms (Rust before → after, both measured then, back to back). Their Python columns are from that run too.

"(fake tree)" cases run the Python script through `packages/power-helpers/parity/py_dryrun.py` (the script's own code with
the outside replaced: a copy of the small fake tree, writes printed), and the Rust helper with `--dry-run --root`. "The four
loops as services" are the four Python scripts as four processes (casefan and gpufan sleeping in 0.2 s steps, as they did)
against the one Rust `power run`. The Python crash watch's RSS includes its journalctl child.

### tune, casefan, gpufan, powerlimit and the power service ([packages/power-helpers](../packages/power-helpers/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| tune --help | wall (median; p10–p90) | 27.9 ms (27.6 ms–28.2 ms) | 0.45 ms (0.44 ms–0.47 ms) | −98 %, 27.4 ms faster |
| | CPU time | 27.7 ms | 0.38 ms | −99 %, 27.3 ms less |
| | peak RSS | 14.8 MB | 2.7 MB | −82 %, 12.1 MB less |
| casefan --help | wall (median; p10–p90) | 25.9 ms (25.6 ms–26.2 ms) | 0.45 ms (0.43 ms–0.47 ms) | −98 %, 25.4 ms faster |
| | CPU time | 25.7 ms | 0.38 ms | −99 %, 25.3 ms less |
| | peak RSS | 14.2 MB | 2.6 MB | −82 %, 11.6 MB less |
| gpufan --help | wall (median; p10–p90) | 24.5 ms (24.4 ms–24.8 ms) | 0.45 ms (0.43 ms–0.47 ms) | −98 %, 24.1 ms faster |
| | CPU time | 24.4 ms | 0.38 ms | −98 %, 24.0 ms less |
| | peak RSS | 13.9 MB | 2.5 MB | −82 %, 11.3 MB less |
| powerlimit --help | wall (median; p10–p90) | 22.5 ms (22.2 ms–22.8 ms) | 0.45 ms (0.43 ms–0.46 ms) | −98 %, 22.1 ms faster |
| | CPU time | 22.4 ms | 0.38 ms | −98 %, 22.0 ms less |
| | peak RSS | 13.2 MB | 2.6 MB | −80 %, 10.5 MB less |
| tune get (fake tree) | wall (median; p10–p90) | 42.0 ms (41.5 ms–42.5 ms) | 2.12 ms (1.96 ms–2.28 ms) | −95 %, 39.9 ms faster |
| | CPU time | 41.5 ms | 1.79 ms | −96 %, 39.7 ms less |
| | peak RSS | 18.4 MB | 6.1 MB | −67 %, 12.4 MB less |
| casefan get (fake tree) | wall (median; p10–p90) | 39.9 ms (39.6 ms–40.6 ms) | 2.16 ms (1.93 ms–2.35 ms) | −95 %, 37.7 ms faster |
| | CPU time | 39.4 ms | 1.80 ms | −95 %, 37.6 ms less |
| | peak RSS | 17.8 MB | 6.1 MB | −65 %, 11.6 MB less |
| gpufan get (fake tree) | wall (median; p10–p90) | 38.5 ms (38.1 ms–39.0 ms) | 2.73 ms (2.53 ms–2.99 ms) | −93 %, 35.8 ms faster |
| | CPU time | 37.2 ms | 1.81 ms | −95 %, 35.4 ms less |
| | peak RSS | 17.4 MB | 6.2 MB | −65 %, 11.2 MB less |
| powerlimit get (fake tree) | wall (median; p10–p90) | 36.5 ms (36.0 ms–37.1 ms) | 0.49 ms (0.46 ms–0.52 ms) | −99 %, 36.1 ms faster |
| | CPU time | 36.3 ms | 0.42 ms | −99 %, 35.9 ms less |
| | peak RSS | 17.2 MB | 2.6 MB | −85 %, 14.6 MB less |
| tune watch, one minute of rounds (fake tree) | wall (median; p10–p90) | 46.3 ms (45.5 ms–46.8 ms) | 2.36 ms (2.25 ms–2.53 ms) | −95 %, 44.0 ms faster |
| | CPU time | 46.0 ms | 2.11 ms | −95 %, 43.8 ms less |
| | peak RSS | 18.7 MB | 6.1 MB | −68 %, 12.7 MB less |
| casefan run, one minute of rounds (fake tree) | wall (median; p10–p90) | 40.6 ms (40.0 ms–40.9 ms) | 0.83 ms (0.80 ms–0.86 ms) | −98 %, 39.7 ms faster |
| | CPU time | 40.4 ms | 0.75 ms | −98 %, 39.6 ms less |
| | peak RSS | 17.8 MB | 2.6 MB | −85 %, 15.2 MB less |
| gpufan run, one minute of rounds (fake tree) | wall (median; p10–p90) | 35.8 ms (35.5 ms–36.6 ms) | 0.49 ms (0.46 ms–0.52 ms) | −99 %, 35.3 ms faster |
| | CPU time | 35.6 ms | 0.42 ms | −99 %, 35.2 ms less |
| | peak RSS | 17.3 MB | 2.5 MB | −85 %, 14.8 MB less |
| powerlimit watch, one minute of rounds (fake tree) | wall (median; p10–p90) | 36.8 ms (36.3 ms–37.2 ms) | 0.56 ms (0.53 ms–0.58 ms) | −98 %, 36.2 ms faster |
| | CPU time | 36.5 ms | 0.48 ms | −99 %, 36.1 ms less |
| | peak RSS | 17.2 MB | 2.6 MB | −85 %, 14.6 MB less |
| the four loops as services (fake tree) | CPU per minute | 21.4 ms | 5.43 ms | −75 %, 16.0 ms less |
| | wake-ups per minute | 650 | 170 | −74 %, 480 fewer |
| | RSS (4 / 1 processes) | 66.6 MB | 3.0 MB | −95 %, 63.5 MB less |
| | PSS | 35.4 MB | 0.9 MB | −98 %, 34.5 MB less |
| | start (CPU in the first 5 s) | 141 ms | 1.28 ms | −99 %, 140 ms less |
| on disk | | tune 23.3 KB, casefan 16.0 KB, gpufan 11.2 KB, powerlimit 9.3 KB (scripts; the interpreter is shared) | tune 540 KB, casefan 494 KB, gpufan 457 KB, powerlimit 471 KB, power 623 KB (binaries) | |

### account, bootscreen, game-fix, gpu-run ([packages/settings-helpers](../packages/settings-helpers/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| account list | wall (median; p10–p90) | 23.7 ms (23.4 ms–24.0 ms) | 0.50 ms (0.47 ms–0.53 ms) | −98 %, 23.2 ms faster |
| | CPU time | 23.5 ms | 0.43 ms | −98 %, 23.1 ms less |
| | peak RSS | 13.7 MB | 2.5 MB | −82 %, 11.2 MB less |
| account add (a new user, stand-in tools) | wall (median; p10–p90) | 31.3 ms (30.8 ms–31.9 ms) | 6.91 ms (6.67 ms–7.19 ms) | −78 %, 24.3 ms faster |
| | CPU time | 31.6 ms | 7.35 ms | −77 %, 24.2 ms less |
| | peak RSS | 13.8 MB | 4.3 MB | −69 %, 9.5 MB less |
| bootscreen get | wall (median; p10–p90) | 28.3 ms (28.1 ms–28.8 ms) | 3.00 ms (2.88 ms–3.08 ms) | −89 %, 25.3 ms faster |
| | CPU time | 28.3 ms | 3.08 ms | −89 %, 25.2 ms less |
| | peak RSS | 14.6 MB | 4.2 MB | −71 %, 10.4 MB less |
| bootscreen set picture (12 MP photo) | wall (median; p10–p90) | 1132 ms (1110 ms–1147 ms) | 815 ms (809 ms–820 ms) | −28 %, 317 ms faster |
| | CPU time | 1129 ms | 812 ms | −28 %, 317 ms less |
| | peak RSS | 129.8 MB | 189.3 MB | +46 %, 59.5 MB more |
| game-fix list --json | wall (median; p10–p90) | 26.7 ms (26.5 ms–27.0 ms) | 1.73 ms (1.68 ms–1.79 ms) | −94 %, 25.0 ms faster |
| | CPU time | 26.5 ms | 1.65 ms | −94 %, 24.8 ms less |
| | peak RSS | 15.0 MB | 3.9 MB | −74 %, 11.1 MB less |
| game-fix apply (Dead Space 2) | wall (median; p10–p90) | 27.4 ms (26.9 ms–28.3 ms) | 1.76 ms (1.71 ms–1.84 ms) | −94 %, 25.6 ms faster |
| | CPU time | 27.1 ms | 1.68 ms | −94 %, 25.4 ms less |
| | peak RSS | 15.0 MB | 3.9 MB | −74 %, 11.1 MB less |
| gpu-run list --json | wall (median; p10–p90) | 32.1 ms (31.9 ms–32.5 ms) | 9.14 ms (8.95 ms–9.27 ms) | −72 %, 23.0 ms faster |
| | CPU time | 32.8 ms | 10.2 ms | −69 %, 22.6 ms less |
| | peak RSS | 13.5 MB | 4.3 MB | −68 %, 9.2 MB less |
| gpu-run 2 PROGRAM (starts a stand-in program) | wall (median; p10–p90) | 30.3 ms (30.1 ms–30.6 ms) | 10.5 ms (10.4 ms–10.7 ms) | −65 %, 19.8 ms faster |
| | CPU time | 31.8 ms | 12.4 ms | −61 %, 19.4 ms less |
| | peak RSS | 13.6 MB | 4.6 MB | −66 %, 9.0 MB less |
| on disk | | account 14.9 KB, bootscreen 9.1 KB, game-fix 12.4 KB, gpu-run 10.1 KB (scripts; the interpreter is shared) | account 454 KB, bootscreen 951 KB, game-fix 1285 KB, gpu-run 1228 KB (binaries) | |

### labyrinth-appusage ([packages/appusage](../packages/appusage/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| labyrinth-appusage --once | wall (median; p10–p90) | 26.1 ms (25.8 ms–26.7 ms) | 3.68 ms (3.55 ms–3.83 ms) | −86 %, 22.4 ms faster |
| | CPU time | 24.3 ms | 2.19 ms | −91 %, 22.1 ms less |
| | peak RSS | 12.8 MB | 6.3 MB | −51 %, 6.5 MB less |
| the service, idle | CPU per minute | 1.60 ms | 1.35 ms | −15 %, 0.25 ms less |
| | wake-ups per minute | 8 | 2 | −75 %, 6 fewer |
| | RSS (1 / 1 processes) | 12.9 MB | 2.6 MB | −79 %, 10.2 MB less |
| | PSS | 7.0 MB | 0.6 MB | −92 %, 6.4 MB less |
| | start (CPU in the first 5 s) | 18.5 ms | 1.11 ms | −94 %, 17.4 ms less |
| on disk | | labyrinth-appusage 3.7 KB (scripts; the interpreter is shared) | labyrinth-appusage 503 KB (binaries) | |

### The crash watch ([packages/crashwatch](../packages/crashwatch/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| the service, idle | CPU per minute | 0.23 ms | 0.00 ms | −100 %, 0.23 ms less |
| | wake-ups per minute | 3 | 0 | −100 %, 3 fewer |
| | RSS (2 / 1 processes) | 38.8 MB | 2.4 MB | −94 %, 36.4 MB less |
| | PSS | 23.8 MB | 0.4 MB | −99 %, 23.5 MB less |
| | start (CPU in the first 5 s) | 22.8 ms | 0.38 ms | −98 %, 22.4 ms less |
| on disk | | labyrinth-crash-watch 3.8 KB (scripts; the interpreter is shared) | yggdrasil-crashwatch 350 KB, crashwatch-notice 1269 KB (binaries) | |

### pkg-root ([packages/agora-helpers](../packages/agora-helpers/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| pkg-root, a refused command | wall (median; p10–p90) | 20.0 ms (19.7 ms–20.2 ms) | 0.44 ms (0.43 ms–0.47 ms) | −98 %, 19.5 ms faster |
| | CPU time | 19.8 ms | 0.38 ms | −98 %, 19.4 ms less |
| | peak RSS | 12.8 MB | 2.4 MB | −81 %, 10.4 MB less |
| pkg-root, an install (stand-in pacman) | wall (median; p10–p90) | 273 ms (272 ms–273 ms) | 252 ms (252 ms–252 ms) | −8 %, 20.9 ms faster |
| | CPU time | 23.3 ms | 2.47 ms | −89 %, 20.9 ms less |
| | peak RSS | 12.9 MB | 4.3 MB | −67 %, 8.7 MB less |
| on disk | | pkg-root 5.7 KB (scripts; the interpreter is shared) | pkg-root 363 KB (binaries) | |

### ygg-theme ([packages/theme](../packages/theme/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| ygg-theme scheme dark 124,84,214 | wall (median; p10–p90) | 27.8 ms (27.6 ms–28.3 ms) | 0.58 ms (0.55 ms–0.61 ms) | −98 %, 27.2 ms faster |
| | CPU time | 27.6 ms | 0.50 ms | −98 %, 27.1 ms less |
| | peak RSS | 15.9 MB | 3.4 MB | −79 %, 12.5 MB less |
| ygg-theme gtk dark 124,84,214 | wall (median; p10–p90) | 27.7 ms (27.4 ms–27.9 ms) | 0.57 ms (0.53 ms–0.59 ms) | −98 %, 27.1 ms faster |
| | CPU time | 27.5 ms | 0.49 ms | −98 %, 27.0 ms less |
| | peak RSS | 15.9 MB | 3.3 MB | −79 %, 12.6 MB less |
| ygg-theme gtk3-theme dark | wall (median; p10–p90) | 27.7 ms (27.4 ms–28.0 ms) | 0.56 ms (0.54 ms–0.58 ms) | −98 %, 27.1 ms faster |
| | CPU time | 27.4 ms | 0.48 ms | −98 %, 26.9 ms less |
| | peak RSS | 15.9 MB | 3.3 MB | −79 %, 12.5 MB less |
| ygg-theme gtk4-theme dark | wall (median; p10–p90) | 116 ms (115 ms–116 ms) | 32.9 ms (32.7 ms–33.2 ms) | −72 %, 82.7 ms faster |
| | CPU time | 120 ms | 37.3 ms | −69 %, 82.5 ms less |
| | peak RSS | 57.6 MB | 32.3 MB | −44 %, 25.3 MB less |
| ygg-theme apply (scratch HOME) | wall (median; p10–p90) | 34.7 ms (34.5 ms–39.4 ms) | 3.73 ms (3.61 ms–3.87 ms) | −89 %, 31.0 ms faster |
| | CPU time | 32.8 ms | 3.62 ms | −89 %, 29.1 ms less |
| | peak RSS | 15.9 MB | 4.0 MB | −75 %, 11.9 MB less |
| on disk | | ygg-theme 30.3 KB (scripts; the interpreter is shared) | ygg-theme 1217 KB (binaries) | |

### ygg-cursors ([packages/theme](../packages/theme/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| ygg-cursors OUTDIR --preview | wall (median; p10–p90) | 1057 ms (1036 ms–1075 ms) | 1604 ms (1586 ms–1630 ms) | +52 %, 547 ms slower |
| | CPU time | 1054 ms | 1599 ms | +52 %, 545 ms more |
| | peak RSS | 31.5 MB | 15.1 MB | −52 %, 16.5 MB less |
| on disk | | ygg-cursors 13.9 KB (scripts; the interpreter is shared) | ygg-cursors 447 KB (binaries) | |

### liki-build, liki-icns, liki-icon-import ([packages/liki](../packages/liki/README.md))

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| liki-build --natives-only (the package's pack) | wall (median; p10–p90) | 46.7 ms (46.0 ms–47.3 ms) | 2.75 ms (2.66 ms–2.86 ms) | −94 %, 44.0 ms faster |
| | CPU time | 46.4 ms | 2.70 ms | −94 %, 43.7 ms less |
| | peak RSS | 17.5 MB | 4.1 MB | −77 %, 13.4 MB less |
| liki-build, a user's pack with app logos | wall (median; p10–p90) | 51.4 ms (50.9 ms–51.9 ms) | 7.10 ms (6.96 ms–7.21 ms) | −86 %, 44.3 ms faster |
| | CPU time | 51.1 ms | 7.01 ms | −86 %, 44.1 ms less |
| | peak RSS | 17.6 MB | 4.1 MB | −77 %, 13.5 MB less |
| liki-icns | wall (median; p10–p90) | 9.27 ms (9.17 ms–9.42 ms) | 0.44 ms (0.41 ms–0.46 ms) | −95 %, 8.83 ms faster |
| | CPU time | 9.15 ms | 0.36 ms | −96 %, 8.78 ms less |
| | peak RSS | 10.1 MB | 2.3 MB | −77 %, 7.7 MB less |
| liki-icon-import, a small pack | wall (median; p10–p90) | 48.5 ms (47.8 ms–49.2 ms) | 1.58 ms (1.53 ms–1.62 ms) | −97 %, 46.9 ms faster |
| | CPU time | 48.2 ms | 1.50 ms | −97 %, 46.7 ms less |
| | peak RSS | 18.1 MB | 3.0 MB | −83 %, 15.1 MB less |
| liki-icon-import, a pack of 3,000 icons | wall (median; p10–p90) | 127 ms (126 ms–129 ms) | 30.3 ms (30.0 ms–31.0 ms) | −76 %, 97.0 ms faster |
| | CPU time | 127 ms | 30.1 ms | −76 %, 96.7 ms less |
| | peak RSS | 22.5 MB | 5.7 MB | −74 %, 16.7 MB less |
| on disk | | liki-build 21.6 KB, liki-icns 1.0 KB, liki-icon-import 12.1 KB (scripts; the interpreter is shared) | liki-build 568 KB, liki-icns 283 KB, liki-icon-import 513 KB (binaries) | |

### yggdrasil-install-system ([packages/install-system](../packages/install-system/README.md))

Measured on its own later the same day (`--only install-system`), on the parity script's made-up computer: a refused plan
with no sandbox (nothing runs), and `--check`, `--disks` and a whole install in the parity script's namespace sandbox (an
empty /dev; /mnt, /run and /sys/firmware tmpfs; every program a stand-in). For the bench the stand-ins are tiny shell
scripts with canned answers, made by the parity script's own fake.py for the same computer, so that the program is timed
and not a Python stand-in: in the whole install both start the same 60 or so of them, and their time is in both CPU times.
Both wait 1 s in `udevadm settle`. On a real computer an install's time is unsquashfs, mkinitcpio and GRUB; these 40 ms
are nothing there.

| case | metric | old (Python) | Rust | change |
| --- | --- | --- | --- | --- |
| a plan refused (nothing runs) | wall (median; p10–p90) | 36.0 ms (35.3 ms–36.6 ms) | 0.93 ms (0.85 ms–1.01 ms) | −97 %, 35.0 ms faster |
| | CPU time | 35.6 ms | 0.78 ms | −98 %, 34.8 ms less |
| | peak RSS | 18.0 MB | 3.6 MB | −80 %, 14.4 MB less |
| --check, a plan with browsers and optional parts | wall (median; p10–p90) | 35.6 ms (35.0 ms–36.4 ms) | 0.72 ms (0.70 ms–0.75 ms) | −98 %, 34.9 ms faster |
| | CPU time | 35.3 ms | 0.64 ms | −98 %, 34.7 ms less |
| | peak RSS | 18.0 MB | 3.6 MB | −80 %, 14.4 MB less |
| --disks (four made-up disks) | wall (median; p10–p90) | 44.4 ms (44.0 ms–45.0 ms) | 8.73 ms (8.59 ms–8.95 ms) | −80 %, 35.6 ms faster |
| | CPU time | 44.3 ms | 8.62 ms | −81 %, 35.7 ms less |
| | peak RSS | 17.2 MB | 4.1 MB | −76 %, 13.1 MB less |
| a whole install (stand-ins; 1 s of it is udevadm settle's wait) | wall (median; p10–p90) | 1100 ms (1098 ms–1101 ms) | 1057 ms (1055 ms–1058 ms) | −4 %, 43.0 ms faster |
| | CPU time | 106 ms | 62.4 ms | −41 %, 43.9 ms less |
| | peak RSS | 18.3 MB | 4.5 MB | −75 %, 13.8 MB less |
| on disk | | install-system 56.0 KB (scripts; the interpreter is shared) | yggdrasil-install-system 736 KB (binaries) | |

In `--disks` and the whole install, most of the Rust program's time is the stand-ins (five and about sixty shell processes),
and its peak memory may be one of them.
