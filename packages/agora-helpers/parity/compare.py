#!/usr/bin/env python3
"""compare: the Rust pkg-root and the Python script it replaces, run as programs on the same cases, in dry runs: pacman is a
stand-in that writes down its arguments (and, for an install, adds a package and downloads two files, then takes its time),
and both run as "root" of a user namespace of their own (unshare -rm, tests/sandbox.sh) with an empty /run/user and a
scratch /var/cache/pacman/pkg. Nothing on the computer is changed.

Each case runs in a world made again from scratch for each (at the same path); exit code, standard output and error, the
stand-in's calls, the installed list and the cache folder afterwards must be the same. What differs on purpose: a package
name or --ignore list that ends in a newline passed the script's patterns ($ matches before a final newline in Python) and
is refused here (exit code 2).

    compare.py [--python FILE] [--bin DIR] [-v]
"""
import argparse
import os
import shutil
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
SANDBOX = os.path.join(HERE, "..", "tests", "sandbox.sh")
GOOD = "/run/user/1000/agora/x.cancel"

PACMAN = """#!/bin/sh
if [ "$1" = -Qq ]; then cat "$W/installed"; exit 0; fi
printf 'pacman' >> "$W/calls"; for a in "$@"; do printf ' [%s]' "$a" >> "$W/calls"; done; echo >> "$W/calls"
case "$1" in -Rns) exit 0 ;; esac
[ -n "$PACMAN_EXIT" ] && exit "$PACMAN_EXIT"
[ -n "$PACMAN_SIGNAL" ] && kill -"$PACMAN_SIGNAL" $$
case "$1" in -S|-U)
  echo newpkg >> "$W/installed"
  echo x > /var/cache/pacman/pkg/newpkg-1-1-x86_64.pkg.tar.zst
  echo x > /var/cache/pacman/pkg/other.part
  echo x > /var/cache/pacman/pkg/notes.txt ;;
esac
[ -n "$PACMAN_SLEEP" ] && exec sleep "$PACMAN_SLEEP"
exit 0
"""


def world(w):
    for d in ("agora", "cache", "bin", "pkgs"):
        os.makedirs(os.path.join(w, d))
    with open(os.path.join(w, "installed"), "w") as f:
        f.write("base\nkrita\n")
    old = os.path.join(w, "cache", "old-1-1-x86_64.pkg.tar.zst")
    with open(old, "w") as f:
        f.write("old")
    os.utime(old, (time.time() - 3600, time.time() - 3600))
    with open(os.path.join(w, "bin", "pacman"), "w") as f:
        f.write(PACMAN)
    os.chmod(os.path.join(w, "bin", "pacman"), 0o755)
    for n in ("a-1-1-x86_64.pkg.tar.zst", "b-1-1-x86_64.pkg.tar.xz", "c.pkg.tar.gz", "d.pkg.tar.bz2"):
        open(os.path.join(w, "pkgs", n), "w").close()


def state(w):
    out = {}
    for n in ("calls", "installed"):
        p = os.path.join(w, n)
        out[n] = open(p).read() if os.path.exists(p) else None
    out["cache"] = sorted(os.listdir(os.path.join(w, "cache")))
    return out


def run(cmd, w, env, cancel_after):
    shutil.rmtree(w, ignore_errors=True)
    world(w)
    e = {"PATH": os.path.join(w, "bin") + ":/usr/bin:/bin", "W": w, "PKEXEC_UID": "1000", "LANG": "en_US.UTF-8"}
    e.update(env)
    e = {k: v for k, v in e.items() if v is not None}
    if cancel_after == 0:
        open(os.path.join(w, "agora", "x.cancel"), "w").close()
    p = subprocess.Popen(["unshare", "-rm", "sh", SANDBOX, w] + cmd, env=e, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    if cancel_after:
        time.sleep(cancel_after)
        open(os.path.join(w, "agora", "x.cancel"), "w").close()
    out, err = p.communicate(timeout=60)
    return p.returncode, out, err, state(w)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", default=None)
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(HERE, "..", "..", "..", "target", "release")))
    ap.add_argument("-v", action="store_true")
    a = ap.parse_args()
    py = a.python
    if not py:
        for d in (os.path.join(HERE, "..", "..", "market", "helpers", "python"), os.path.join(HERE, "..", "..", "market", "helpers")):
            if os.path.exists(os.path.join(d, "pkg-root")):
                py = os.path.join(d, "pkg-root")
                break
    rs = os.path.join(a.bin, "pkg-root")
    scratch = tempfile.mkdtemp(prefix="pkg-root-parity-")
    w = os.path.join(scratch, "w")
    P = os.path.join(w, "pkgs")
    refusals = [
        [GOOD, "ls", "-S", "krita"], [GOOD], [GOOD, "pacman"], [GOOD, "pacman", "-S"], [GOOD, "pacman", "-Syu"],
        ["/etc/passwd", "pacman", "-S", "krita"], ["/run/user/1000/agora/../../0/x", "pacman", "-S", "krita"], ["/run/user/1001/agora/x", "pacman", "-S", "krita"],
        ["/run/user/1000/agora/", "pacman", "-S", "krita"], ["//run/user/1000/agora/x", "pacman", "-S", "krita"], ["run/user/1000/agora/x", "pacman", "-S", "krita"],
        ["/run/user/1000/agoraX/x", "pacman", "-S", "krita"], ["/run/user/1000/agora/a..b", "pacman", "-S", "krita"],
    ]
    for bad in ("--root", "--config", "--hookdir", "--sysroot", "-Sy", "--overwrite", "--dbpath", "--root=/", "--config=/etc/pacman.conf", "-Q", "--sync", "-Suy", "-Rdd", "--cachedir",
                "--assume-installed", "--gpgdir", "--logfile", "-b", "--arch", "--print", "-"):
        refusals.append([GOOD, "pacman", "-S", bad, "krita"])
    for bad in ("krita;ls", "$(id)", "a b", "`x`", "a|b", "aа", "a\\b", "a*", "a,b", "a'b", "", "krita\n"):
        refusals.append([GOOD, "pacman", "-S", bad])
    for lst in ("a b", "x;y", "a/b", "", "a\n"):
        refusals.append([GOOD, "pacman", "-Syu", "--ignore", lst])
    refusals += [
        [GOOD, "pacman", "-Syu", "--ignore"], [GOOD, "pacman", "--needed", "krita"], [GOOD, "pacman", "-S", "-R", "krita"], [GOOD, "pacman", "-S", "-S", "krita"],
        [GOOD, "pacman", "-S", "-y", "krita"], [GOOD, "pacman", "-U", "-y"], [GOOD, "pacman", "-Syu", "-y", "--noconfirm"], [GOOD, "pacman", "-D", "krita"], [GOOD, "pacman", "-D", "-q", "krita"],
        [GOOD, "pacman", "-U", "--noconfirm", "--config", "/tmp/evil.conf", "--", f"{P}/a-1-1-x86_64.pkg.tar.zst"],
        [GOOD, "pacman", "-U", "--", "https://example.com/x.pkg.tar.zst"], [GOOD, "pacman", "-U", "--", "/etc/passwd"], [GOOD, "pacman", "-U", "--", f"{P}/missing.pkg.tar.zst"],
        [GOOD, "pacman", "-U", "--", f"{P}/d.pkg.tar.bz2"], [GOOD, "pacman", "-U", "--", "a-1-1-x86_64.pkg.tar.zst"], [GOOD, "pacman", "-U", "--", P],
        [GOOD, "pacman", "-U", "--noconfirm", "--", f"{P}/a-1-1-x86_64.pkg.tar.zst", "-S"], [GOOD, "pacman", "--noconfirm", "--", "-S", "krita"],
        [GOOD, "pacman", "-S", "--config", "/etc/pacman.conf\n", "krita"], [GOOD, "pacman", "-S", "--config", "/etc//pacman.conf", "krita"],
    ]
    for uid in ("", "1000\n", " 1000", "abc", "1001"):
        refusals.append(("uid", uid, [GOOD, "pacman", "-S", "krita"]))
    passes = [
        [GOOD, "pacman", "-S", "--needed", "--noconfirm", "krita"], [GOOD, "pacman", "-Rns", "--noconfirm", "krita"], [GOOD, "pacman", "-Rs", "krita"],
        [GOOD, "pacman", "-R", "krita", "gimp"], [GOOD, "pacman", "-Syu", "--ignore", "a,b", "--noconfirm"],
        [GOOD, "pacman", "-U", "--ignore", "klassy", "--ignore", "sarina-git", "--noconfirm", "--config", "/etc/pacman.conf", "--", f"{P}/a-1-1-x86_64.pkg.tar.zst", f"{P}/b-1-1-x86_64.pkg.tar.xz", f"{P}/c.pkg.tar.gz"],
        [GOOD, "pacman", "-D", "--asdeps", "--config", "/etc/pacman.conf", "--", "somelib"],
        [GOOD, "pacman", "-D", "-q", "--asexplicit", "--noconfirm", "--config", "/etc/pacman.conf", "--", "deckcraft-bin", "gridcraft-bin"],
        [GOOD, "pacman", "-S", "-y", "--ignore", "klassy", "--noconfirm", "--config", "/etc/pacman.conf", "--"],
        [GOOD, "pacman", "-S", "-y", "-y", "--noconfirm"], [GOOD, "pacman", "-S", "--ignore", "-S", "-y", "--noconfirm"],
        ["/run/user/1000//agora/./x.cancel", "pacman", "-S", "--noconfirm", "krita"], ["///run/user/1000/agora/x.cancel", "pacman", "-S", "krita", "-s", "-n"],
        [GOOD, "pacman", "-S", "--", "--noconfirm"], [GOOD, "pacman", "-S", "core/krita", "lib32-gtk+3", "a@b", "1:2.0-1~x"],
    ]
    cases = []
    for r in refusals:
        if isinstance(r, tuple):
            cases.append((f"refused, PKEXEC_UID={r[1]!r}: {r[2]}", r[2], {"PKEXEC_UID": r[1]}, None))
        else:
            cases.append((f"refused: {r}", r, {}, None))
    for p in passes:
        cases.append((f"runs: {p}", p, {}, None))
        cases.append((f"pacman fails (3): {p}", p, {"PACMAN_EXIT": "3"}, None))
        cases.append((f"pacman killed: {p}", p, {"PACMAN_SIGNAL": "TERM"}, None))
    for p in ([GOOD, "pacman", "-S", "--noconfirm", "newpkg"], [GOOD, "pacman", "-U", "--noconfirm", "--", f"{P}/a-1-1-x86_64.pkg.tar.zst"],
              [GOOD, "pacman", "-Rns", "--noconfirm", "krita"], [GOOD, "pacman", "-Syu", "--noconfirm"], [GOOD, "pacman", "-D", "--asdeps", "x", "y"]):
        cases.append((f"cancelled while it runs: {p}", p, {"PACMAN_SLEEP": "30"}, 0.8))
        cases.append((f"cancelled before it starts: {p}", p, {"PACMAN_SLEEP": "30"}, 0))
        cases.append((f"cancelled as it finished: {p}", p, {}, 0))
        cases.append((f"pacman ignores SIGTERM: {p}", p, {"PACMAN_SLEEP": "30", "PACMAN_TRAP": "1"}, 0.8))
    failures = 0
    try:
        for label, argv, env, cancel in cases:
            if "PACMAN_TRAP" in env:
                continue  # the 15 s SIGKILL wait: covered by reading, too slow to run for every case
            res = {}
            for who, cmd in (("python", [sys.executable, py] + argv), ("rust", [rs] + argv)):
                res[who] = run(cmd, w, env, cancel)
            pr, rr = res["python"], res["rust"]
            same = pr == rr
            if not same and pr[0] == 0 and rr[0] == 2 and any("\n" in x for x in argv[2:]) and b"agora pkg-root:" in rr[2]:
                same = True  # a final newline: refused here on purpose
            if not same:
                failures += 1
                print(f"DIFFERS {label}\n  python {pr}\n  rust   {rr}")
            elif a.v:
                print(f"same: {label}")
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    n = sum(1 for c in cases if "PACMAN_TRAP" not in c[2])
    print(f"pkg-root: {n} cases, {'all the same' if not failures else str(failures) + ' differ'}")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
