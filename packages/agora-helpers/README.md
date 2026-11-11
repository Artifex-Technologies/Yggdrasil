# Agora's administrator helper (Rust): pkg-root

`/usr/lib/agora/pkg-root` runs one pacman command with administrator rights for Agora, so an install can be cancelled and
cleaned up. Agora's sudo stand-in (`packages/market/helpers/sudo`, given to yay) runs it through pkexec:

    pkexec /usr/lib/agora/pkg-root CANCELFILE pacman OPTIONS... PACKAGES...

It replaces the Python script `packages/market/helpers/pkg-root` with the same rules:

- only pacman, and only install (`-S`, `-U`), remove (`-R`, `-Rs`, `-Rns`), mark (`-D` with `--asdeps` or `--asexplicit`),
  and a refresh that is always a full upgrade (`-S -y` becomes `-Syu`), one at a time, with a short list of plain options;
- `--config` may only name `/etc/pacman.conf`, `--ignore` only a plain list of names, `-U` only real package files
  (`/....pkg.tar.zst|xz|gz`); everything after `--` is a package name;
- the cancel file must be in the invoking user's runtime folder, `/run/user/PKEXEC_UID/agora/`, with no `..`.

While pacman runs (in a session of its own) the cancel file is looked for every 0.25 s. On cancel: SIGTERM to pacman's
group (SIGKILL after 15 s), the packages this run installed are removed (`pacman -Rns`), the package files it downloaded
deleted, and the exit code is 130. Otherwise pacman's own exit code is given back (for a signal, 256 minus its number, as
Python's `sys.exit(-N)` gave). Its cache folder is fixed: a root helper takes no path from its environment.

Not 1:1: a package name or `--ignore` list that ends in a newline passed the script's patterns (`$` matches before a final
newline in Python); here it is refused.

## Tests and parity

`cargo test`: the script's 12 tests (`tests/test_agora_pkg_root.py`) carried over, hostile arguments, pacman's exit codes,
and two cancels run in a user and mount namespace of their own (`tests/sandbox.sh`: an empty `/run/user` and a scratch
pacman cache, for that process tree only; skipped where `unshare -rm` is not allowed).

`parity/compare.py` runs it and the script on the same cases in that sandbox, with a stand-in pacman: refusals, the shapes
yay sends, pacman failing or killed, cancels before, during and just after a run. Exit code, output, pacman's calls, the
installed list and the cache folder must be the same.

    cargo build --release && python3 parity/compare.py [-v]

## Why Rust: measured

Measured on 2026-11-11 with `tools/bench-port.py` against the Python script from git, with a stand-in pacman (the install in
the parity script's namespace sandbox), on my desktop (16 threads). Method and every case: [docs/PORT_NUMBERS.md](../../docs/PORT_NUMBERS.md).

| Metric | Python | Rust | Change |
| --- | --- | --- | --- |
| a refused command, wall time | 20.0 ms | 0.44 ms | −98 %, 19.5 ms faster |
| an install (stand-in pacman), wall time | 273 ms | 252 ms | −8 %, 20.9 ms faster |
| an install, CPU time | 23.3 ms | 2.47 ms | −89 %, 20.9 ms less |
| peak memory | 12.8–12.9 MB | 2.4–4.3 MB | −67 to −81 % |
| on disk | 5.7 KB script (+ Python, 75 MB) | 363 KB | +357 KB; Agora no longer needs Python |

Both look for the cancel file every 0.25 s while pacman runs, so an install takes at least 250 ms either way.

More accurate:

- A package name or `--ignore` list ending in a newline passed the script's patterns (Python's `$` matches before a final
  newline); it is refused now.
- Parity: 136 cases in the sandbox (refusals, yay's shapes, pacman failing or killed, cancels before, during and just after
  a run): all the same.
- Tests: the script's 12 carried over, 5 on hostile arguments and cancel files, pacman's exit codes, and two cancels in a
  namespace of their own.
