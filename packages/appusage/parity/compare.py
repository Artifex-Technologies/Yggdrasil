#!/usr/bin/env python3
"""compare: the Rust labyrinth-appusage and the Python script it replaces, on the same inputs. Nothing outside a scratch
folder is written.

  1. which app a scope belongs to: many made-up unit names (both launchers' forms, flatpak, \\x2d, hashes, digits of other
     scripts, newlines, near misses) against two sets of desktop ids; every answer must be the same;
  2. sample(): sequences of samples on usage files that start empty, broken, or with odd entries (floats, text, true, null,
     a list); after each step the apps counted, whether it stopped with an error, and the file's bytes must be the same;
  3. this computer, read only: `--once` of both with a scratch XDG_DATA_HOME (the desktop entries linked in) must write the
     same file, and the scopes the Rust service finds in the cgroup tree (`--scopes`) must be the ones systemctl lists as
     running, with the same apps.

What differs on purpose is not compared here: the service counts each app's real running time from its scope's start to its
end (the script added 30 s for each sample that found it running), and writes every five minutes instead of every 30 s.

    compare.py [--python SCRIPT] [--bin DIR] [--seeds N] [-v]

--python: the Python labyrinth-appusage (default: ../python/labyrinth-appusage)
--bin: the folder with labyrinth-appusage and examples/appusage_replay (default: ../../../target/release; build with
       cargo build --release --bins --examples)
"""
import argparse
import importlib.machinery
import importlib.util
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))


def load(path):
    loader = importlib.machinery.SourceFileLoader("labyrinth_appusage", path)
    spec = importlib.util.spec_from_loader("labyrinth_appusage", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


def rust_cases(replay, cases):
    p = subprocess.run([replay], input="\n".join(json.dumps(c) for c in cases) + "\n", capture_output=True, text=True)
    if p.returncode != 0:
        raise SystemExit("appusage_replay failed: " + p.stderr)
    return [json.loads(l) for l in p.stdout.splitlines()]


IDS_PLAIN = ["firefox", "spotify-launcher", "com.microsoft.VSCode", "org.gimp.GIMP", "foo", "a", "a-1", "flatpak", "x_y", "dev.app",
             "app", "café", "1", "foo@bar", "my app", "a٣"]
# two ids that sanitize to the same name: the script's answer then depends on its set's order (it changes from run to run)
IDS_CLASH = IDS_PLAIN + ["x.y", "dev-app"]
PIECES = ["firefox", "spotify", "launcher", "spotify-launcher", "spotify\\x2dlauncher", "spotify_launcher", "com.microsoft.VSCode",
          "com_microsoft_VSCode", "org.gimp.GIMP", "foo", "a", "a-1", "flatpak", "x_y", "x.y", "dev.app", "dev_app", "app", "café",
          "caf_", "1", "foo@bar", "my app", "my_app", "a٣", "", "-", "_", "\n", "@", "x2d"]
SUFFIXES = ["-1", "-42", "-٣", "-²", "@abc123", "@ABC", "@", "-", "_3", "_", "-1-2", "_1_2", "_٣", "-1@ab", "@ab-1", ""]
ENDS = [".scope", ".scope", ".scope", ".service", ".scope\n", ".slice", ""]
PREFIXES = ["app-", "app-", "app-flatpak-", "lapp_", "lapp_", "session-", "", "app-flatpak-flatpak-", "lapp_app-"]


def units(rng, n):
    out = ["lapp_spotify_launcher_123.scope", "app-firefox-4567.scope", "app-spotify\\x2dlauncher-9.scope", "app-flatpak-1.scope",
           "app-a-1-2.scope", "lapp__1.scope", "app--1.scope", "app-a\nb-1.scope", "init.scope"]
    for _ in range(n):
        mid = "".join(rng.choice(PIECES) for _ in range(rng.choice((1, 1, 1, 2, 3))))
        out.append(rng.choice(PREFIXES) + mid + rng.choice(SUFFIXES) + rng.choice(ENDS))
    return out


STARTS = [None, "", "{not json", "[1, 2]", "null", '{"firefox": {"seconds": 10.9, "last": 1.5}}', '{"foo": {"seconds": " 7 "}}',
          '{"firefox": {"seconds": true}}', '{"firefox": 0}', '{"firefox": 5}', '{"firefox": {"seconds": "lots"}}',
          '{"firefox": {"seconds": null}}', '{"firefox": {"launches": 3, "last": 1700000000}}', '{"caf\\u00e9": {"seconds": 1}}',
          '{"a": {"seconds": 1e3}}', '{"a": {"seconds": -5}}', '{"a": {}}', '{"a": []}', '{"a": [1]}', "\xff\xfe"]


def keys(text):
    try:
        v = json.loads(text)
    except ValueError:
        return None
    return list(v) if isinstance(v, dict) else None


def same_file(py_text, rust_text, py_before, rust_before):
    """The same file but for the order of the apps added: the script added them in its set's order, which changes from run to
    run (the Rust one adds them sorted). In each, every key that was in its file before keeps its place, ahead of the new ones."""
    try:
        if json.loads(py_text) != json.loads(rust_text):
            return False
    except ValueError:
        return False
    for text, before in ((py_text, py_before), (rust_text, rust_before)):
        old = keys(before) or []
        if keys(text)[:len(old)] != old:
            return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", default=os.path.join(HERE, "..", "python", "labyrinth-appusage"))
    ap.add_argument("--bin", default=os.path.join(HERE, "..", "..", "..", "target", "release"))
    ap.add_argument("--seeds", type=int, default=40)
    ap.add_argument("-v", action="store_true")
    a = ap.parse_args()
    py = load(a.python)
    replay = os.path.join(a.bin, "examples", "appusage_replay")
    binary = os.path.join(a.bin, "labyrinth-appusage")
    failures = 0
    rng = random.Random(1)

    # 1. matching
    n_match = 0
    for ids in (IDS_PLAIN, IDS_CLASH):
        us = units(rng, 6000)
        got = rust_cases(replay, [{"mode": "match", "ids": ids, "units": us}])[0]
        idset = set(ids)
        for u, r in zip(us, got):
            want = py.match_unit(u, idset)
            n_match += 1
            if r == want:
                continue
            if ids is IDS_CLASH and u.startswith("lapp_") and r and want and py.sanitize(r) == py.sanitize(want):
                continue  # the clash: either id is what the script could have answered
            failures += 1
            print(f"match differs: {u!r}: python {want!r}, rust {r!r}")
    print(f"matching: {n_match} unit names, {'all the same' if not failures else str(failures) + ' differ'}")

    # 2. sampling
    scratch = tempfile.mkdtemp(prefix="appusage-parity-")
    n_steps = 0
    before = failures
    for seed in range(a.seeds):
        r = random.Random(seed)
        start = STARTS[seed % len(STARTS)]
        steps = [[r.choice((30, 30, 1, 120, 0)), r.sample(units(r, 30)[:9] + ["app-firefox-1.scope", "lapp_foo_2.scope", "app-a-7.scope"], r.randint(0, 6))] for _ in range(6)]
        results = {}
        for who in ("python", "rust"):
            p = os.path.join(scratch, who, str(seed), "labyrinth", "app-usage.json")
            os.makedirs(os.path.dirname(p), exist_ok=True)
            if start is not None:
                with open(p, "w", encoding="latin-1") as f:
                    f.write(start)
            if who == "rust":
                results[who] = rust_cases(replay, [{"mode": "sample", "path": p, "ids": IDS_PLAIN, "steps": steps}])[0]
                continue
            out = []
            for secs, us in steps:
                try:
                    res = {"apps": sorted(py.sample(secs, ids=set(IDS_PLAIN), units=us, path=p))}
                except Exception:
                    res = {"error": True}
                try:
                    text = open(p, encoding="utf-8").read()
                except (OSError, ValueError):
                    text = open(p, encoding="latin-1").read() if os.path.exists(p) else ""
                out.append([res, text])
            results[who] = out
        n_steps += len(steps)
        prev_py = prev_rust = start or ""
        for i, (pr, rr) in enumerate(zip(results["python"], results["rust"])):
            same = pr == rr or (pr[0] == rr[0] and same_file(pr[1], rr[1], prev_py, prev_rust))
            prev_py, prev_rust = pr[1], rr[1]
            if not same:
                failures += 1
                print(f"sample differs (seed {seed}, step {i}, start {start!r}):\n  python {pr}\n  rust   {rr}")
                break
        if a.v:
            print(f"seed {seed}: {results['rust'][-1]}")
    print(f"sampling: {a.seeds} sequences, {n_steps} samples, {'all the same' if failures == before else str(failures - before) + ' differ'}")

    # 3. this computer
    before = failures
    real_data = os.environ.get("XDG_DATA_HOME") or os.path.expanduser("~/.local/share")
    files = {}
    for who, cmd in (("python", [sys.executable, a.python, "--once"]), ("rust", [binary, "--once"])):
        d = os.path.join(scratch, "live-" + who)
        os.makedirs(d)
        for sub in ("applications", "flatpak"):
            if os.path.exists(os.path.join(real_data, sub)):
                os.symlink(os.path.join(real_data, sub), os.path.join(d, sub))
        p = subprocess.run(cmd, env=dict(os.environ, XDG_DATA_HOME=d), capture_output=True, text=True)
        path = os.path.join(d, "labyrinth", "app-usage.json")
        files[who] = (p.returncode, open(path).read() if os.path.exists(path) else "")
    if files["python"][0] != files["rust"][0] or not same_file(files["python"][1], files["rust"][1], "", ""):
        failures += 1
        print(f"--once differs:\n  python {files['python']}\n  rust   {files['rust']}")
    else:
        print(f"--once on this computer: the same file ({len(json.loads(files['rust'][1] or '{}'))} apps running)")
    listed = subprocess.run(["systemctl", "--user", "list-units", "--type=scope", "--state=running", "--no-legend", "--plain", "--no-pager"],
                            capture_output=True, text=True).stdout
    sd = {l.split()[0] for l in listed.splitlines() if l.split()}
    p = subprocess.run([binary, "--scopes"], capture_output=True, text=True)
    cg = {}
    for l in p.stdout.splitlines():
        unit, _, app = l.partition("\t")
        cg[unit] = app
    ids = py.desktop_ids()
    if set(cg) != sd:
        failures += 1
        print(f"scopes differ: only systemctl: {sorted(sd - set(cg))}, only the cgroup tree: {sorted(set(cg) - sd)}")
    for unit, app in cg.items():
        if (py.match_unit(unit, ids) or "") != app:
            failures += 1
            print(f"scope {unit}: python {py.match_unit(unit, ids)!r}, rust {app!r}")
    if failures == before:
        print(f"scopes on this computer: the cgroup tree and systemctl agree ({len(cg)} scopes, "
              f"{len({x for x in cg.values() if x})} apps)")
    shutil.rmtree(scratch, ignore_errors=True)
    print("parity: " + ("all the same" if not failures else f"{failures} differences"))
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
