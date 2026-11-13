#!/usr/bin/env python3
"""compare: the Rust ygg-theme and the Python script it replaces, run as programs on the same inputs, in dry runs: HOME,
XDG_CONFIG_HOME and XDG_DATA_HOME are a scratch folder made again for each run (at the same path), and gsettings,
systemctl, hyprctl and pkill are stand-ins on the PATH that write down how they were called (with a stand-in Hyprland socket
for the Rust side, which sets the pointer through yggdrasil-wm). Nothing of this user's own is read or written.

  1. what it prints: scheme and gtk (dark, light, many accents, bad accents), gtk3-theme, gtk4-theme (GTK 4's own theme from
     the installed libgtk-4, retinted), --help, a command that does not exist;
  2. GTK 4's own theme retinted in many accents (examples/theme_retint against the script's retint_css);
  3. apply, cursor and their files: every combination of what is there before (appearance.conf, gtk.css with and without
     the marker, an old stylesheet that imports colors.css, kdeglobals with odd content, xsettingsd's file, installed cursor
     themes) and KDE programs or not; exit code, output, the stand-ins' calls and every file afterwards must be the same.

    compare.py [--python FILE] [--bin DIR] [-v]
"""
import argparse
import importlib.machinery
import importlib.util
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))


class StandInHyprland:
    """A stand-in Hyprland socket in a folder of its own: the Rust ygg-theme sets the pointer through yggdrasil-wm (`setcursor` on
    the socket) where the script ran hyprctl. It writes the same line the hyprctl stand-in writes into the calls. Its signature
    names no real Hyprland."""

    def __init__(self, calls):
        import socket
        import threading
        self.dir, self.calls, self.done = tempfile.mkdtemp(prefix="ygg-th-"), calls, False
        os.makedirs(os.path.join(self.dir, "hypr", "parity"))
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(os.path.join(self.dir, "hypr", "parity", ".socket.sock"))
        self.sock.listen(4)
        self.sock.settimeout(0.2)
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        while not self.done:
            try:
                c, _ = self.sock.accept()
            except OSError:
                continue
            with c:
                c.settimeout(0.5)
                try:
                    q = c.recv(4096).decode()
                except OSError:
                    q = ""
                if q.startswith("/"):
                    with open(self.calls, "a") as f:
                        f.write("hyprctl" + "".join(f" [{a}]" for a in q[1:].split(" ")) + "\n")
                c.sendall(b"ok")

    def env(self):
        return {"XDG_RUNTIME_DIR": self.dir, "HYPRLAND_INSTANCE_SIGNATURE": "parity"}

    def close(self):
        self.done = True
        self.thread.join()
        self.sock.close()
        shutil.rmtree(self.dir, ignore_errors=True)
STUB = """#!/bin/sh
printf '%s' "$(basename "$0")" >> "$W/calls"; for a in "$@"; do printf ' [%s]' "$a" >> "$W/calls"; done; echo >> "$W/calls"
"""


def load(path):
    loader = importlib.machinery.SourceFileLoader("ygg_theme", path)
    spec = importlib.util.spec_from_loader("ygg_theme", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


def write(p, text):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def snapshot(root):
    out = {}
    for d, dirs, files in os.walk(root):
        dirs.sort()
        rel = os.path.relpath(d, root)
        if rel == "bin" or rel.startswith("bin/"):
            continue
        for n in sorted(files):
            with open(os.path.join(d, n), "rb") as f:
                out[os.path.normpath(os.path.join(rel, n))] = f.read()
        for n in dirs:
            out[os.path.normpath(os.path.join(rel, n)) + "/"] = b"dir"
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--python")
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(HERE, "..", "..", "..", "target", "release")))
    ap.add_argument("-v", action="store_true")
    a = ap.parse_args()
    py = a.python or next(p for p in (os.path.join(HERE, "..", "python", "ygg-theme"), os.path.join(HERE, "..", "bin", "ygg-theme")) if os.path.exists(p))
    rs = os.path.join(a.bin, "ygg-theme")
    retint = os.path.join(a.bin, "examples", "theme_retint")
    scratch = tempfile.mkdtemp(prefix="ygg-theme-parity-")
    w = os.path.join(scratch, "w")
    fails = []
    counts = {}

    def run(cmd, env):
        p = subprocess.run(cmd, env=env, capture_output=True, timeout=120)
        return p.returncode, p.stdout, p.stderr

    # 1. what it prints
    prints = [["gtk4-theme", "dark"], ["gtk4-theme", "light"], ["gtk4-theme"], ["gtk3-theme", "dark"], ["gtk3-theme", "light"], ["gtk3-theme", "x"],
              ["--help"], ["-h"], [], ["bogus"], ["scheme"], ["gtk"]]
    for dark in ("dark", "light", "Light", ""):
        for acc in ("124,84,214", "255,255,255", "0,0,0", "1,2,3", "10,200,30", "128,128,128", " 12 , 34,56", "256,0,0", "1,2", "1,2,3,4", "a,b,c", "", "١,2,3", "-1,2,3", "+5,6,7"):
            prints += [["scheme", dark, acc], ["gtk", dark, acc]]
    env = dict(os.environ, HOME=scratch, XDG_CONFIG_HOME=os.path.join(scratch, "c"), XDG_DATA_HOME=os.path.join(scratch, "d"))
    for args in prints:
        pr, rr = run([sys.executable, py] + args, env), run([rs] + args, env)
        counts["prints"] = counts.get("prints", 0) + 1
        if pr != rr:
            fails.append(("prints", args))
            print(f"DIFFERS {args}: python {pr[0]} {len(pr[1])} bytes {pr[2][-200:]!r}, rust {rr[0]} {len(rr[1])} bytes {rr[2][-200:]!r}")

    # 2. retint in many accents
    m = load(py)
    try:
        own = {d: m.gtk_own_css(d) for d in (True, False)}
    except Exception as e:  # no python-gobject or no libgtk-4 here
        own = None
        print(f"retint: skipped ({e})")
    if own:
        accents = [(124, 84, 214), (255, 255, 255), (0, 0, 0), (10, 200, 30), (21, 83, 158), (250, 10, 10), (128, 128, 128)] + [((i * 37) % 256, (i * 91) % 256, (i * 53) % 256) for i in range(40)]
        for dark in (True, False):
            for acc in accents:
                want = m.retint_css(own[dark], dark, acc)
                got = subprocess.run([retint, "dark" if dark else "light", "%d,%d,%d" % acc], input=own[dark], capture_output=True, text=True).stdout
                counts["retint"] = counts.get("retint", 0) + 1
                if want != got:
                    fails.append(("retint", (dark, acc)))
                    print(f"DIFFERS retint {dark} {acc}")

    # 3. apply and cursor, and their files
    befores = {
        "nothing": {},
        "own": {"c/yggdrasil/appearance.conf": "style = light\naccent = 1,2,3\nicons = Papirus\ncursor = macOS\nother = kept?\n# a comment\n"},
        "odd-appearance": {"c/yggdrasil/appearance.conf": "style=dark # x\n style = light\naccent =  bad\nicons = ../x\ncursor = Yggdrasil\nstyle =dark\n"},
        "old-css": {"c/gtk-4.0/gtk.css": "@import 'colors.css';\nwindow { border-radius: 3px; }\n", "c/gtk-3.0/gtk.css": "@import \"colors.css\";  \n\n@import 'colors.css';\n.x{}\n"},
        "marker-css": {"c/gtk-4.0/gtk.css": "old colours\n/* --- yours below this line --- */\n.mine { color: red; }\n"},
        "ini": {"c/gtk-3.0/settings.ini": "[Settings]\ngtk-theme-name=Adwaita\n# gtk-icon-theme-name=x\ngtk-font-name=Sans 10\n\n\n[Other]\na=1\n[Settings]\ngtk-theme-name=dup\n",
                "c/gtk-4.0/settings.ini": "loose=1\r\n[Settings]\r\ngtk-cursor-theme-name = old\r\n"},
        "kde": {"c/kdeglobals": "[General]\nfoo=1\nColorScheme=Old\n\n[Other]\nx=2\n[Icons]\nTheme=Breeze\nextra=1\n", "c/kcminputrc": "[Mouse]\ncursorTheme=breeze\ncursorSize=24\n"},
        "xsettings": {"c/xsettingsd/xsettingsd.conf": "Net/ThemeName \"Adwaita\"\nGtk/CursorThemeName \"x\"\n\nXft/DPI 98304\nNet/IconThemeName\n"},
        "cursors": {"d/icons/Mine/cursors/left_ptr": "x", "h/.icons/Other/cursors/left_ptr": "x", "d/icons/NoCursors/index.theme": "x"},
    }
    commands = [["apply"], ["apply", "--dark"], ["apply", "--light", "--accent", "12,34,56"], ["apply", "--accent", "999,0,0"], ["apply", "--accent"], ["apply", "--light", "--dark", "x"],
                ["cursor"], ["cursor", "Mine"], ["cursor", "Other"], ["cursor", "NoCursors"], ["cursor", "../Mine"], ["cursor", ".hidden"], ["cursor", "Yggdrasil"], ["cursor", "it's"],
                ["cursor", "Mine", "extra"]]
    for name, files in befores.items():
        for kde in ("1", "0"):
            for args in commands:
                for tools in (True, False):
                    res = {}
                    for who, cmd in (("python", [sys.executable, py]), ("rust", [rs])):
                        shutil.rmtree(w, ignore_errors=True)
                        os.makedirs(os.path.join(w, "bin"))
                        os.makedirs(os.path.join(w, "h"))
                        if tools:
                            for t in ("gsettings", "systemctl", "hyprctl", "pkill"):
                                write(os.path.join(w, "bin", t), STUB)
                                os.chmod(os.path.join(w, "bin", t), 0o755)
                        for rel, text in files.items():
                            write(os.path.join(w, rel.replace("h/", "h/", 1)), text)
                        e = {"PATH": os.path.join(w, "bin") + (":/usr/bin:/bin" if tools else ""), "W": w, "HOME": os.path.join(w, "h"),
                             "XDG_CONFIG_HOME": os.path.join(w, "c"), "XDG_DATA_HOME": os.path.join(w, "d"), "YGG_THEME_KDE": kde, "LANG": "en_US.UTF-8"}
                        # with the tools there, a stand-in Hyprland too (the Rust side sets the pointer on its socket)
                        hypr = StandInHyprland(os.path.join(w, "calls")) if tools else None
                        if hypr:
                            e.update(hypr.env())
                        try:
                            code, out, err = run(cmd + args, e)
                        finally:
                            if hypr:
                                hypr.close()
                        res[who] = (code, out, err, snapshot(w))
                    counts["files"] = counts.get("files", 0) + 1
                    pr, rr = res["python"], res["rust"]
                    if pr != rr:
                        fails.append(("files", (name, kde, args, tools)))
                        print(f"DIFFERS [{name}, kde={kde}, tools={tools}] {args}: exit {pr[0]}/{rr[0]} out {pr[1]!r}/{rr[1]!r} err {pr[2][-300:]!r}/{rr[2][-300:]!r}")
                        for k in sorted(set(pr[3]) | set(rr[3])):
                            if pr[3].get(k) != rr[3].get(k):
                                print(f"  {k}:\n    python {pr[3].get(k)!r}\n    rust   {rr[3].get(k)!r}")
                    elif a.v:
                        print(f"same [{name}, kde={kde}, tools={tools}] {args}")
    shutil.rmtree(scratch, ignore_errors=True)
    for k, n in counts.items():
        bad = sum(1 for f in fails if f[0] == k)
        print(f"ygg-theme {k}: {n} cases, {'all the same' if not bad else str(bad) + ' differ'}")
    return 1 if fails else 0


if __name__ == "__main__":
    sys.exit(main())
