# Settings > Power's root helpers (Rust)

The helpers behind Settings > Power, in Rust, replacing the Python scripts at the same paths with the same interface:
commands, arguments, JSON (byte for byte), settings and state files, exit codes, log lines and every safety rule.

| Helper | Installed as | For |
| --- | --- | --- |
| `tune` | `/usr/lib/yggdrasil/tune` | processor multipliers, power limits and turbo, graphics card clocks, with the crash guard |
| `casefan` | `/usr/lib/yggdrasil/casefan` | the Corsair Commander Pro's fans and pump (Power > Cooling) |
| `gpufan` | `/usr/lib/yggdrasil/gpufan` | the NVIDIA card's fans |
| `powerlimit` | `/usr/lib/yggdrasil/powerlimit` | each power mode's processor speed cap and graphics card watts |
| `power` | `/usr/lib/yggdrasil/power` | the root service yggdrasil-power: the loops of the four in one process, the NVIDIA library loaded once |

The details (the units, the switches, each part's safety, upgrading, going back) are in
[docs/APPS_SETTINGS.md](../../docs/APPS_SETTINGS.md). `--dry-run` on every command prints each write instead of making it;
with `--root DIR` it runs against a fake tree with a stand-in card (`DIR/nvml.json`), and `--steps N` runs N rounds of a
loop on a virtual clock.

## Tests and parity

`cargo test`: the Python scripts' tests under the same names (17 tune, 17 casefan, 10 gpufan, 6 powerlimit and the power
hold) on the same seams (fake hwmon trees, a fake card, stand-in MSRs), plus the merged service's rules.

`parity/compare.py` runs each Rust helper and its Python script (through `parity/py_dryrun.py`, which keeps the script's own
code and replaces only the outside) on the same fake trees, in dry runs, and compares output and exit codes line by line.

    cargo build --release --bins --examples && python3 parity/compare.py [-v]

The Python scripts stay in `packages/settings/helpers/python/` with their tests, for a rollback.

## Why Rust: measured

Measured on 2026-11-11 with `tools/bench-port.py` against the Python scripts from git, on the parity script's fake trees
(no hardware, a stand-in card), on my desktop (16 threads). Method and every case: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| `tune get`, wall time | 42.0 ms | 2.12 ms | −95 %, 39.9 ms faster |
| `casefan get`, wall time | 39.9 ms | 2.16 ms | −95 %, 37.7 ms faster |
| `powerlimit get`, wall time | 36.5 ms | 0.49 ms | −99 %, 36.1 ms faster |
| one minute of casefan's loop (30 rounds, virtual clock), CPU | 40.4 ms | 0.75 ms | −98 %, 39.6 ms less |
| peak memory of `get` and the loops | 17.2–18.7 MB | 2.6 MB (6.1 MB¹) | −65 to −85 % |
| the four loops as services, CPU per minute | 21.4 ms | 5.43 ms | −75 %, 16.0 ms less |
| the four loops as services, wake-ups per minute | 650 | 170 | −74 %, 480 fewer |
| the four loops as services, memory (RSS) | 66.6 MB in 4 processes | 3.0 MB in 1 | −95 %, 63.5 MB less |
| the four loops as services, memory (PSS) | 35.4 MB | 0.9 MB | −98 %, 34.5 MB less |
| on disk | 60 KB of scripts (+ Python, 75 MB, shared) | 2.5 MB (five binaries) | +2.5 MB |

¹ `tune get`, `casefan get` and `gpufan get` ask `systemctl is-active`; 6.1 MB is systemctl's own peak.

The Python casefan and gpufan woke five times a second (they slept in 0.2 s steps to notice a stop); the Rust loops wake
every 2 s and are told of a stop at once. On my PC with the real card (2026-11-09, [docs/APPS_SETTINGS.md](../../docs/APPS_SETTINGS.md)):
the Python tune held about 88 MB (its journalctl child 46 MB of it) and casefan about 40 MB; the Rust service holds
29.6 MB for all four (24.0 MB PSS), most of it the NVIDIA library, now loaded once instead of in every process.

More accurate:

- Parity: tune, casefan and gpufan 83 of 83 dry-run cases, the help text, and casefan's loop over 1,216 of 1,216
  temperature sequences the same; powerlimit 34 more cases (107 of 107 in all); the merged service 17 of 17.
- casefan: a setting whose `ports` was not an object crashed the Python one and left the fans and pump at their last
  speed (its `reset` crashed too); now every port goes to 100 %.
- tune reads the kernel's messages from `/dev/kmsg` itself, without a journalctl child.
- New: if casefan's loop stands still for 20 s, the service puts every Commander Pro port at 100 % (the Python one had
  no watchdog).
