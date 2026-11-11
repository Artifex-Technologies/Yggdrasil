#!/usr/bin/env python3
"""cursors: the Rust ygg-cursors and the Python script it replaces (python/ygg-cursors, Pillow), run as programs on the same
output folders: the same exit code, standard output and standard error, the same files and links, every X cursor file byte
for byte (the format is written by the tools themselves, so the same pixels make the same file), and the preview sheet's
decoded pixels exactly (the PNG encoders differ, not the picture). Where an X cursor file differs, the header, the table
of contents and each image chunk are compared on their own and the largest pixel difference is shown.

The fonts are varied with a stand-in fc-match on the PATH: the help cursor's "?" in several fonts (TrueType, CFF, a
variable font, one without a "?", a colour bitmap font FreeType cannot size), a path FreeType cannot open, a bare file
name found in the font folders, nothing, and no fc-match at all (Pillow's own default font). Nothing of this user's own is
read or written: HOME is a scratch folder.

    cursors.py [--python FILE] [--bin FILE] [-v]
"""
import argparse
import os
import shutil
import struct
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.dirname(HERE)
ARGS = None
SCRATCH = tempfile.mkdtemp(prefix="ygg-cursors-parity-")
WORLD = os.path.join(SCRATCH, "world")
RESULTS = [0, 0]


def stub_fc_match(answer):
    b = os.path.join(WORLD, "bin")
    os.makedirs(b, exist_ok=True)
    p = os.path.join(b, "fc-match")
    with open(p, "w") as f:
        f.write("#!/bin/sh\nprintf '%s' " + shlex_quote(answer) + "\n")
    os.chmod(p, 0o755)


def shlex_quote(s):
    return "'" + s.replace("'", "'\"'\"'") + "'"


def files(root):
    out = {}
    for d, dirs, fs in os.walk(root):
        dirs.sort()
        for n in sorted(fs) + sorted(x for x in dirs if os.path.islink(os.path.join(d, x))):
            p = os.path.join(d, n)
            r = os.path.relpath(p, root)
            if os.path.islink(p):
                out[r] = ("link", os.readlink(p))
            else:
                with open(p, "rb") as f:
                    out[r] = ("file", f.read())
        for n in dirs:
            out[os.path.relpath(os.path.join(d, n), root) + "/"] = ("dir", None)
    return out


def xcursor_chunks(data):
    """header, toc entries, and per chunk (header bytes, pixels as ints)"""
    magic, hsize, version, n = struct.unpack("<4sIII", data[:16])
    toc = [struct.unpack("<III", data[16 + 12 * i:28 + 12 * i]) for i in range(n)]
    chunks = []
    for _, _, pos in toc:
        head = data[pos:pos + 36]
        w, h = struct.unpack("<II", head[16:24])
        px = struct.unpack("<%dI" % (w * h), data[pos + 36:pos + 36 + 4 * w * h])
        chunks.append((head, px))
    return (magic, hsize, version, n), toc, chunks


def explain_xcursor(a, b):
    try:
        ha, ta, ca = xcursor_chunks(a)
        hb, tb, cb = xcursor_chunks(b)
    except struct.error as e:
        return f"not a readable X cursor file: {e}"
    if ha != hb or ta != tb:
        return f"header or table of contents differ: {ha} {hb}"
    worst = []
    for i, ((h1, p1), (h2, p2)) in enumerate(zip(ca, cb)):
        if h1 != h2:
            worst.append(f"chunk {i} header differs")
        diff = max((max(abs(((x >> s) & 255) - ((y >> s) & 255)) for s in (0, 8, 16, 24)) for x, y in zip(p1, p2) if x != y), default=0)
        if diff:
            n = sum(1 for x, y in zip(p1, p2) if x != y)
            worst.append(f"chunk {i}: {n} pixels differ, by up to {diff}")
    return "; ".join(worst) or "same chunks, other bytes differ"


def picture(path):
    from PIL import Image
    im = Image.open(path)
    return im.size, im.mode, im.tobytes()


def run(cmd, env_extra, path_with_bin):
    env = {k: v for k, v in os.environ.items() if k not in ("XDG_DATA_HOME", "XDG_DATA_DIRS")}
    env.update({"HOME": os.path.join(WORLD, "home"), "LANG": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1"})
    env["PATH"] = (os.path.join(WORLD, "bin") + ":" if path_with_bin else "") + "/usr/bin:/bin"
    env.update(env_extra)
    r = subprocess.run(cmd, env=env, cwd=WORLD, capture_output=True, timeout=600)
    return r.returncode, r.stdout, r.stderr.decode("utf-8", "replace")


def case(name, argv, setup=lambda: None, fc=None, no_fc=False, env=None):
    """fc: what the stand-in fc-match prints (None: the real one); no_fc: no fc-match on the PATH at all."""
    results = []
    for which in ("python", "rust"):
        shutil.rmtree(WORLD, ignore_errors=True)
        os.makedirs(os.path.join(WORLD, "home"))
        if fc is not None:
            stub_fc_match(fc)
        elif not no_fc:
            os.makedirs(os.path.join(WORLD, "bin"))
            os.symlink(shutil.which("fc-match"), os.path.join(WORLD, "bin", "fc-match"))
        setup()
        exe = [sys.executable, ARGS.python] if which == "python" else [ARGS.bin]
        code, out, err = run(exe + [a.replace("@", WORLD) for a in argv], env or {}, not no_fc)
        snap = files(os.path.join(WORLD, "out")) if os.path.isdir(os.path.join(WORLD, "out")) else {}
        pic = picture(os.path.join(WORLD, "out", "preview.png")) if os.path.isfile(os.path.join(WORLD, "out", "preview.png")) else None
        results.append((code, out, err, snap, pic))
    (pc, po, pe, ps, pp), (rc, ro, re_, rs, rp) = results
    problems = []
    if "Traceback (most recent call last)" in pe:
        if rc != 1 or len(re_.strip().splitlines()) != 1:
            problems.append(f"traceback in Python; Rust exit {rc}, stderr {re_!r}")
    else:
        if pe != re_:
            problems.append(f"stderr {pe!r} vs {re_!r}")
        if pc != rc:
            problems.append(f"exit {pc} vs {rc}")
    if po != ro:
        problems.append(f"stdout {po!r} vs {ro!r}")
    for k in sorted(set(ps) | set(rs)):
        a, b = ps.get(k), rs.get(k)
        if k == "preview.png":
            continue
        if a != b:
            if a and b and a[0] == b[0] == "file" and k.startswith("cursors/"):
                problems.append(f"{k}: {explain_xcursor(a[1], b[1])}")
            else:
                problems.append(f"{k}: {str(a)[:80]} vs {str(b)[:80]}")
    if pp != rp:
        problems.append("preview.png: " + ("missing in one" if not (pp and rp) else f"size/mode {pp[:2]} vs {rp[:2]}" if pp[:2] != rp[:2] else f"{sum(1 for x, y in zip(pp[2], rp[2]) if x != y)} bytes of pixels differ"))
    RESULTS[0] += 1
    if problems:
        RESULTS[1] += 1
        print(f"MISMATCH {name}")
        for p in problems[:12]:
            print("   ", p)
    elif ARGS.verbose:
        n = sum(1 for v in ps.values() if v[0] == "file")
        print(f"ok {name}: exit {pc}, {n} files{', preview pixels the same' if pp else ''}")


def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", default=os.path.join(PKG, "python", "ygg-cursors"))
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(PKG, "..", "..", "target", "release", "ygg-cursors")))
    ap.add_argument("-v", "--verbose", action="store_true")
    ARGS = ap.parse_args()
    try:
        case("the theme and the preview, the system's fc-match", ["@/out", "--preview"])
        fonts = "/usr/share/fonts"
        for f in ["noto/NotoSans-Regular.ttf", "noto/NotoSerif-Bold.ttf", "noto/NotoSansMono-Bold.ttf", "gnu-free/FreeSerifBold.otf", "gnu-free/FreeSans.otf",
                  "Adwaita/AdwaitaSans-Regular.ttf", "Adwaita/AdwaitaMono-BoldItalic.ttf", "noto/NotoSansArabic-Bold.ttf", "noto/NotoColorEmoji.ttf"]:
            if os.path.exists(os.path.join(fonts, f)):
                case(f"the ? in {f}", ["@/out"], fc=os.path.join(fonts, f))
        case("fc-match names a file that is not there", ["@/out"], fc="/nonexistent/font.ttf")
        case("fc-match gives a bare file name (found in the font folders)", ["@/out"], fc="NotoSans-Regular.ttf")
        case("fc-match gives a name without extension", ["@/out"], fc="FreeSerifBold")
        case("fc-match prints nothing", ["@/out"], fc="")
        case("fc-match prints whitespace around the path", ["@/out"], fc="  /usr/share/fonts/noto/NotoSans-Regular.ttf \n")
        case("no fc-match: Pillow's default font", ["@/out", "--preview"], no_fc=True)
        case("no arguments", [])
        case("only options", ["--preview", "--other"])
        case("an option before the folder", ["--x", "@/out"])

        def stale():
            c = os.path.join(WORLD, "out", "cursors")
            os.makedirs(c)
            os.symlink("elsewhere", os.path.join(c, "hand2"))
            with open(os.path.join(c, "xterm"), "w") as f:
                f.write("a file where a link goes")
            os.symlink("gone", os.path.join(c, "wait"))
        case("a theme there before (old links, a file, a dangling main file)", ["@/out"], setup=stale)

        def blocked():
            os.makedirs(os.path.join(WORLD, "out", "cursors", "ibeam"))
        case("a folder where a link goes", ["@/out"], setup=blocked)

        def out_file():
            with open(os.path.join(WORLD, "out"), "w") as f:
                f.write("x")
        case("the output is a file", ["@/out"], setup=out_file)
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)
    print(f"ygg-cursors: {RESULTS[0] - RESULTS[1]}/{RESULTS[0]} the same")
    return 1 if RESULTS[1] else 0


if __name__ == "__main__":
    sys.exit(main())
