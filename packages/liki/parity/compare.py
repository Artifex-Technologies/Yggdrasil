#!/usr/bin/env python3
"""compare: the Rust liki-build, liki-icns and liki-icon-import and the Python scripts they replace (python/), run as
programs on the same made-up worlds: HOME is a scratch folder made again for each run at the same path, the tools they
call (gtk-update-icon-cache) are stand-ins on the PATH that write down how they were called and leave a fixed cache file,
and every file read or written is in the scratch folder (the system's own /usr/share/applications is read by both, as it
is). Nothing of this user's own is read or written.

For each case both run in the same world and must give the same exit code, standard output, standard error, calls and
files afterwards (contents, modes, links), byte for byte, apart from what differs on purpose:

  - where the script stopped with a traceback, the Rust tool stops too, with exit code 1 and one line on standard error;
  - zipfile's warnings (two members at the same place, an empty Unicode path field) are not printed by the Rust one.

  1. liki-build: every shape and style, --natives-only, --out, names (with "[" and "]": the glob bug), --rebuild with
     every kind of remembered file, accents, desktop files (aliases, suffixes, CRLF, hidden ones, "/" in Icon=), a damaged
     brand-icons folder, a stale pack, XDG_* set to nothing, no gtk-update-icon-cache, and argparse's help and errors at
     several widths and in colour;
  2. liki-icns: made .icns files, then many broken ones (bytes changed, cut short);
  3. liki-icon-import: made packs (plain and compiled appfilter.xml in UTF-8 and UTF-16, sizes, folders, names, the
     theme there before), argparse; then fuzzing: the compiled appfilter.xml, the plain one and the archive itself with
     bytes changed, cut, repeated (the drawable names come back in the message when nothing matches, so the text decoding
     is compared too).

    compare.py [--python DIR] [--bin DIR] [--only build|icns|import] [--fuzz N] [--seed N] [-v]
"""
import argparse
import io
import json
import os
import random
import shutil
import struct
import subprocess
import sys
import tempfile
import zipfile
import zlib

HERE = os.path.dirname(os.path.abspath(__file__))
PKG = os.path.dirname(HERE)
SHARE = os.path.join(PKG, "share")
ARGS = None
SCRATCH = tempfile.mkdtemp(prefix="liki-parity-")
WORLD = os.path.join(SCRATCH, "world")
STUB = """#!/bin/sh
printf '%s' "$(basename "$0")" >> "$W/calls"; for a in "$@"; do printf ' [%s]' "$a" >> "$W/calls"; done; echo >> "$W/calls"
for a in "$@"; do last="$a"; done
[ -d "$last" ] && printf 'cache\\n' > "$last/icon-theme.cache"
exit 0
"""
STATS = {}


def write(p, data, mode=None):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        f.write(data.encode() if isinstance(data, str) else data)
    if mode is not None:
        os.chmod(p, mode)


def snapshot(root):
    out = {}
    for d, dirs, files in os.walk(root):
        dirs.sort()
        rel = os.path.relpath(d, root)
        if rel == "bin" or rel.startswith("bin/") or rel == "input" or rel.startswith("input/"):
            continue
        for n in sorted(files) + sorted(x for x in dirs if os.path.islink(os.path.join(d, x))):
            p = os.path.join(d, n)
            r = os.path.normpath(os.path.join(rel, n))
            if os.path.islink(p):
                out[r] = ("link", os.readlink(p))
            else:
                with open(p, "rb") as f:
                    out[r] = (oct(os.stat(p).st_mode & 0o777), f.read())
        for n in dirs:
            out[os.path.normpath(os.path.join(rel, n)) + "/"] = "dir"
    return out


def clean_stderr(s):
    """Python's stderr without zipfile's warnings (the line and the source line after it)."""
    lines = s.split("\n")
    out, skip = [], False
    for line in lines:
        if skip:
            skip = False
            continue
        if ": UserWarning: " in line:
            skip = True
            continue
        out.append(line)
    return "\n".join(out)


def run(cmd, env_extra, unset=(), cwd=None, stdin=None):
    env = {k: v for k, v in os.environ.items() if k not in ("COLUMNS", "LINES", "NO_COLOR", "FORCE_COLOR", "PYTHON_COLORS", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "LIKI_BRAND_ICONS", "LABYRINTH_BRAND_ICONS")}
    env.update({"HOME": os.path.join(WORLD, "home"), "W": WORLD, "PATH": os.path.join(WORLD, "bin") + ":/usr/bin:/bin", "LANG": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1", "TERM": "xterm"})
    env.pop("LC_ALL", None)
    for k in unset:
        env.pop(k, None)
    env.update(env_extra)
    r = subprocess.run(cmd, env=env, cwd=cwd or WORLD, capture_output=True, stdin=subprocess.DEVNULL, timeout=120)
    return r.returncode, r.stdout, r.stderr.decode("utf-8", "replace")


def compare(group, name, setup, argv, env=None, unset=(), tool=None):
    """Run the script and the binary on the same fresh world; record a mismatch."""
    tool = tool or {"build": "liki-build", "icns": "liki-icns"}[group]
    env = env or {}
    results = []
    for which in ("python", "rust"):
        if os.path.exists(WORLD):
            shutil.rmtree(WORLD)
        os.makedirs(os.path.join(WORLD, "home"))
        os.makedirs(os.path.join(WORLD, "bin"))
        if setup.__name__ != "no_cache_tool":
            write(os.path.join(WORLD, "bin", "gtk-update-icon-cache"), STUB, 0o755)
        setup()
        exe = [sys.executable, os.path.join(ARGS.python, tool)] if which == "python" else [os.path.join(ARGS.bin, tool)]
        code, out, err = run(exe + argv, env, unset)
        results.append((code, out, err, snapshot(WORLD)))
    (pc, po, pe, ps), (rc, ro, re_, rs) = results
    problems = []
    if "Traceback (most recent call last)" in pe:
        if rc != 1 or len(re_.strip().splitlines()) != 1:
            problems.append(f"traceback in Python; Rust exit {rc}, stderr {re_!r}")
        pe = re_ = None
    else:
        pe = clean_stderr(pe)
    if pc != rc and not (pe is None and rc == 1 and pc == 1):
        problems.append(f"exit {pc} vs {rc}")
    if po != ro:
        problems.append(f"stdout {po!r} vs {ro!r}")
    if pe is not None and pe != re_:
        problems.append(f"stderr {pe!r} vs {re_!r}")
    if ps != rs:
        keys = sorted(set(ps) | set(rs))
        diff = [k for k in keys if ps.get(k) != rs.get(k)]
        problems.append(f"files differ: {diff[:6]}")
    s = STATS.setdefault(group, [0, 0])
    s[0] += 1
    if problems:
        s[1] += 1
        print(f"MISMATCH {group}: {name}")
        for p in problems:
            print("   ", p[:600])
    elif ARGS.verbose:
        print(f"ok {group}: {name} (exit {pc})")


# ---------------------------------------------------------------- liki-build

def brand_icons(dst, damage=None):
    shutil.copytree(os.path.join(SHARE, "brand-icons"), os.path.join(dst, "brand-icons"))
    shutil.copytree(os.path.join(SHARE, "symbolic"), os.path.join(dst, "symbolic"))
    b = os.path.join(dst, "brand-icons")
    if damage == "no-viewbox":
        write(os.path.join(b, "native", "timer.svg"), '<svg xmlns="x"><path d="M0 0"/></svg>')
    elif damage == "zero-viewbox":
        write(os.path.join(b, "icons", "firefox.svg"), '<svg viewBox="0 0 0 0"><path/></svg>')
    elif damage == "odd-viewbox":
        write(os.path.join(b, "native", "minos.svg"), '<?xml version="1.0"?>\r\n<svg viewBox="" x="1"><title>t</title><svg viewBox=" -1_0.5 2e1  24 1.5e7 9 ">\r\n<path d="M1"/></svg>\n \n')
    elif damage == "no-colors":
        os.remove(os.path.join(b, "colors.json"))
    elif damage == "bad-colour":
        c = json.load(open(os.path.join(b, "colors.json")))
        c["firefox"] = "zz7139"
        write(os.path.join(b, "colors.json"), json.dumps(c))
    elif damage == "symbolic-dir":
        os.makedirs(os.path.join(dst, "symbolic", "folder.svg"))
    elif damage == "latin1-native":
        write(os.path.join(b, "native", "home.svg"), b'<svg viewBox="0 0 24 24"><title>\xe9</title></svg>')
    return b


def desktop(name, text):
    write(os.path.join(WORLD, "home", ".local", "share", "applications", name), text)


DESKTOPS = [
    ("firefox.desktop", "[Desktop Entry]\nName=Firefox\nIcon=firefox\n"),
    ("vlc.desktop", "[Desktop Entry]\r\nName=VLC media player\r\nIcon=org.videolan.vlc\r\n"),
    ("code.desktop", "[Desktop Entry]\nName=Visual Studio Code\nIcon=code\nExec=code %F\n[Desktop Action new]\nIcon=other\n"),
    ("spot.desktop", "[Desktop Entry]\nName=Spotify\nIcon=spotify-launcher\n"),
    ("steam.desktop", "Icon=steam_icon_570\nName=Some Game\n"),
    ("dup.desktop", "[Desktop Entry]\nName=Firefox again\nIcon=firefox\n"),
    (".hidden.desktop", "[Desktop Entry]\nName=Hidden\nIcon=discord\n"),
    ("escape.desktop", "[Desktop Entry]\nName=Firefox\nIcon=../../../../escape/firefox\n"),
    ("sub.desktop", "[Desktop Entry]\nName=Firefox\nIcon=sub/firefox\n"),
    ("abs.desktop", "[Desktop Entry]\nName=Firefox\nIcon=/usr/share/pixmaps/firefox.png\n"),
    ("native.desktop", "[Desktop Entry]\nName=Settings\nIcon=preferences-system\n"),
    ("blank.desktop", "[Desktop Entry]\nIcon=\nIcon=  obsidian  \nName=\n"),
    ("bad.desktop", b"[Desktop Entry]\nName=\xff\xfeBrave\nIcon=brave-browser\n"),
    ("cr.desktop", "Name=Discord\rIcon=com.discordapp.Discord\r"),
    ("uni.desktop", "[Desktop Entry]\nName=Tor Browser\nIcon=torbrowser-launcher x\n"),
]


def build_cases():
    src = os.path.join(WORLD, "input")

    def std(damage=None, desktops=True, conf=None, appearance=None, stale=None):
        def setup():
            b = brand_icons(src, damage)
            if desktops:
                for n, t in DESKTOPS:
                    desktop(n, t)
                os.makedirs(os.path.join(WORLD, "home", ".local", "share", "applications", "dir.desktop"), exist_ok=True)
            if conf is not None:
                write(os.path.join(WORLD, "home", ".config", "labyrinth", "brand-pack.json"), conf)
            if appearance is not None:
                write(os.path.join(WORLD, "home", ".config", "yggdrasil", "appearance.conf"), appearance)
            if stale:
                stale()
            return b
        return setup

    env = {"LIKI_BRAND_ICONS": os.path.join(src, "brand-icons")}
    for shape in ("glyph", "squircle", "rounded", "circle", "hexagon"):
        for style in ("brand", "dark", "purple"):
            compare("build", f"{shape} {style}", std(), ["--shape", shape, "--style", style], env)
    compare("build", "natives only into --out", std(), ["--natives-only", "--out", os.path.join(WORLD, "out", "Liki")], env)
    compare("build", "--out empty is the default", std(), ["--out", "", "--name", "X"], env)
    compare("build", "a name with spaces", std(), ["--name", "My Pack", "--shape=circle"], env)

    def other_pack():
        write(os.path.join(WORLD, "home", ".local", "share", "icons", "Liki1", "apps", "scalable", "keep.svg"), "<svg/>")
        write(os.path.join(WORLD, "home", ".local", "share", "icons", "Liki[12]", "apps", "scalable", "stale.svg"), "old")
        write(os.path.join(WORLD, "home", ".local", "share", "icons", "Liki[12]", "apps", "scalable", ".hidden.svg"), "kept")
        write(os.path.join(WORLD, "home", ".local", "share", "icons", "Liki[12]", "apps", "scalable", "notes.txt"), "kept")
    compare("build", "a [ in the name leaves another pack alone (glob bug)", std(stale=other_pack), ["--name", "Liki[12]"], env)

    def svg_dir():
        os.makedirs(os.path.join(WORLD, "home", ".local", "share", "icons", "Liki", "places", "scalable", "x.svg"))
    compare("build", "a folder named .svg in the pack", std(stale=svg_dir), [], env)
    for conf in [None, "", "{", "[]", "null", '{"shape": "circle", "style": "dark", "name": "Old"}', '{"shape": 5, "style": "dark"}',
                 '{"shape": "triangle", "style": "purple", "name": "Mine"}', '{"name": ""}', '{"name": "../x"}', '{"name": "a/b"}',
                 '{"name": ".."}', '{"name": "tab\\there"}', '{"name": "\\ud800"}', '{"name": "\\u00e9t\\u00e9"}', '{"shape": "hexagon", "x": NaN}',
                 '{"shape": "rounded", "shape": "circle"}', '﻿{"shape": "circle"}', '{"shape": "circle"} x', b'{"name": "\xff"}', '{"style": ["dark"]}']:
        compare("build", f"--rebuild with {conf!r}", std(conf=conf), ["--rebuild"], env)
    compare("build", "--rebuild ignores --out given remembered", std(conf='{"name": "R"}'), ["--rebuild", "--shape", "circle"], env)
    for app in ["accent = 10,20,30\n", "accent=300,1,1\naccent = 1,2,3\n", "accent = a,b,c\naccent=1,2,3\n", "accent=1,2\n", "# accent=1,2,3\n",
                "accent = ١٢,3,4\n", "style = dark\r\naccent = 255, 0 ,128 # mine\r\n", b"accent = \xff,1,1\n", "x=1\naccent= 1_0,2,3", "accent = +5,-0,0x1\n"]:
        compare("build", f"accent {app!r}", std(appearance=app, desktops=False), ["--natives-only"], env)
    for damage in ("no-viewbox", "zero-viewbox", "odd-viewbox", "no-colors", "bad-colour", "symbolic-dir", "latin1-native"):
        compare("build", f"damaged brand icons: {damage}", std(damage=damage), [], env)

    def no_cache_tool():
        brand_icons(src)
    compare("build", "no gtk-update-icon-cache (it is optional)", no_cache_tool, ["--natives-only"], env)
    compare("build", "XDG_CONFIG_HOME and XDG_DATA_HOME set to nothing", std(conf='{"name": "Z"}'), [], dict(env, XDG_CONFIG_HOME="", XDG_DATA_HOME=""))
    compare("build", "XDG dirs elsewhere", std(), [], dict(env, XDG_CONFIG_HOME=os.path.join(WORLD, "cfg"), XDG_DATA_HOME=os.path.join(WORLD, "data")))
    compare("build", "LABYRINTH_BRAND_ICONS when LIKI_ is empty", std(), ["--natives-only"], {"LIKI_BRAND_ICONS": "", "LABYRINTH_BRAND_ICONS": os.path.join(src, "brand-icons")})
    compare("build", "brand icons path with a slash at the end", std(), ["--natives-only"], {"LIKI_BRAND_ICONS": os.path.join(src, "brand-icons") + "/"})
    # the command line
    argvs = [["--help"], ["-h"], ["--shape", "x"], ["--sh", "circle"], ["--s", "x"], ["--n", "x"], ["--bogus"], ["x"], ["--name"], ["--name", "-x"],
             ["--name", "-1"], ["--rebuild=1"], ["-hx"], ["-h=x"], ["--out="], ["--"], ["--", "x"], ["-x"], ["-"], ["--style=dark", "--style", "nope"],
             ["--he"], ["--natives-only", "--natives-only", "--na"], ["--name=a=b", "--out", os.path.join(WORLD, "o")], ["--shape", "circle", "extra", "more"],
             ["--name", "a b", "--name", "-a b"], ["--nat=1"], ["-h", "--bogus"], ["--bogus", "-h"], ["--shape"], ["--shape", "--style", "dark"]]
    for argv in argvs:
        compare("build", f"argv {argv}", std(desktops=False), argv, env)
    for extra in [{"COLUMNS": "40"}, {"COLUMNS": "60"}, {"COLUMNS": "120"}, {"COLUMNS": "200"}, {"COLUMNS": "10"}, {"COLUMNS": "1"}, {"COLUMNS": "x"},
                  {"FORCE_COLOR": "1"}, {"FORCE_COLOR": "1", "NO_COLOR": "1"}, {"PYTHON_COLORS": "1", "NO_COLOR": "1"}, {"PYTHON_COLORS": "1", "COLUMNS": "50"},
                  {"FORCE_COLOR": "1", "TERM": "dumb"}, {"PYTHON_COLORS": "0", "FORCE_COLOR": "1"}]:
        for argv in (["--help"], ["--shape", "x"]):
            compare("build", f"argv {argv} with {extra}", std(desktops=False), argv, dict(env, **extra))


# ---------------------------------------------------------------- liki-icns

def png(w, h, tail=b"", rgb=(120, 80, 210)):
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    raw = b"".join(b"\x00" + bytes(rgb) * w for _ in range(min(h, 4)))
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"") + tail


def icns(chunks):
    body = b"".join(k + struct.pack(">I", len(d) + 8) + d for k, d in chunks)
    return b"icns" + struct.pack(">I", len(body) + 8) + body


def icns_cases(rng):
    good = icns([(b"ic07", png(128, 128)), (b"ARGB", b"\x00" * 64), (b"ic09", png(512, 512, b"a")), (b"ic10", png(512, 512, b"b")), (b"ic08", png(256, 256))])
    inp = os.path.join(WORLD, "input", "in.icns")
    out = os.path.join(WORLD, "out.png")

    def with_input(data):
        def setup():
            write(inp, data)
        return setup
    compare("icns", "made .icns", with_input(good), [inp, out])
    compare("icns", "no PNG inside", with_input(icns([(b"ARGB", b"x" * 40)])), [inp, out])
    compare("icns", "not an .icns", with_input(b"PNG?"), [inp, out])
    compare("icns", "a PNG chunk too short", with_input(icns([(b"ic07", png(1, 1)[:20])])), [inp, out])
    compare("icns", "the output in a missing folder", with_input(good), [inp, os.path.join(WORLD, "nope", "out.png")])
    compare("icns", "no input", with_input(good), [os.path.join(WORLD, "missing.icns"), out])
    compare("icns", "the same file in and out", with_input(icns([(b"ARGB", b"x")])), [inp, inp])
    for argv in ([], [inp], [inp, out, out]):
        compare("icns", f"argv {argv}", with_input(good), argv)
    for k in range(ARGS.fuzz):
        data = bytearray(good)
        for _ in range(rng.randint(1, 6)):
            r = rng.random()
            i = rng.randrange(len(data))
            if r < 0.5:
                data[i] = rng.randrange(256)
            elif r < 0.7:
                del data[i:i + rng.randint(1, 40)]
            elif r < 0.85:
                data[i:i] = bytes(rng.randrange(256) for _ in range(rng.randint(1, 8)))
            else:
                data = data[:i]
                if not data:
                    data = bytearray(b"i")
        compare("icns", f"fuzz {k}", with_input(bytes(data)), [inp, out])


# ---------------------------------------------------------------- liki-icon-import

def axml(items, utf8=False, strings_extra=()):
    """A compiled Android XML file <resources><item component=.. drawable=../>...</resources>."""
    strings = ["resources", "item", "component", "drawable"] + list(strings_extra)
    for c, d in items:
        for s in (c, d):
            if s not in strings:
                strings.append(s)
    idx = {s: i for i, s in enumerate(strings)}
    pool_data, offs = b"", []
    for s in strings:
        offs.append(len(pool_data))
        if utf8:
            b = s.encode("utf-8", "surrogatepass")
            n = len(s)
            pool_data += (bytes([0x80 | (n >> 8), n & 0xFF]) if n > 0x7F else bytes([n]))
            pool_data += (bytes([0x80 | (len(b) >> 8), len(b) & 0xFF]) if len(b) > 0x7F else bytes([len(b)])) + b + b"\x00"
        else:
            pool_data += struct.pack("<H", len(s)) + s.encode("utf-16-le", "surrogatepass") + b"\x00\x00"
    while len(pool_data) % 4:
        pool_data += b"\x00"
    hsize = 28
    pool = struct.pack("<HHIIIIII", 1, hsize, hsize + 4 * len(strings) + len(pool_data), len(strings), 0, 0x100 if utf8 else 0, hsize + 4 * len(strings), 0) + b"".join(struct.pack("<I", o) for o in offs) + pool_data

    def start(tag, attrs):
        a = b"".join(struct.pack("<iiiHBBI", -1, idx[k], idx[v], 8, 0, 3, idx[v]) for k, v in attrs)
        return struct.pack("<HHIII", 0x0102, 16, 36 + len(a), 1, 0xFFFFFFFF) + struct.pack("<iiHHHHHH", -1, idx[tag], 20, 20, len(attrs), 0, 0, 0) + a
    body = start("resources", [])
    for c, d in items:
        body += start("item", [("component", c), ("drawable", d)])
    return struct.pack("<HHI", 3, 8, 8 + len(pool) + len(body)) + pool + body


def apk(path, plain=None, compiled=None, pics=(), extra=(), compression=zipfile.ZIP_DEFLATED):
    with zipfile.ZipFile(path, "w", compression) as z:
        if plain is not None:
            z.writestr("assets/appfilter.xml", plain)
        if compiled is not None:
            z.writestr("res/xml/appfilter.xml", compiled)
        for name, data in pics:
            z.writestr(name, data)
        for name, data in extra:
            z.writestr(name, data)


ITEMS = [("ComponentInfo{org.mozilla.firefox/org.mozilla.fennec.App}", "firefox"), ("ComponentInfo{org.videolan.vlc/x.Y}", "ic_vlc"),
         ("ComponentInfo{com.spotify.music/com.spotify.music.MainActivity}", "spotify"), ("ComponentInfo{com.valvesoftware.android.steam.community/x}", "steam"),
         ("ComponentInfo{com.discord/com.discord.main.MainActivity}", "discord"), ("ComponentInfo{com.android.settings/x.A}", "settings"), ("", "nocomponent")]
PICS = [("res/drawable-nodpi/firefox.png", png(192, 192)), ("res/drawable-xxhdpi/firefox.png", png(256, 256)), ("res/mipmap-hdpi/ic_vlc.png", png(96, 96)),
        ("assets/spotify.png", png(512, 512)), ("res/drawable/steam.png", png(64, 32)), ("res/drawable/discord.png", png(16, 16)),
        ("RES/Drawable-x/settings.PNG", png(128, 128)), ("res/drawable/sub/nocomponent.png", png(64, 64)), ("res/raw/x.png", png(64, 64))]


def import_desktops():
    for n, t in [("firefox.desktop", "[Desktop Entry]\nName=Firefox\nIcon=firefox\nExec=firefox %u\n"), ("vlc.desktop", "[Desktop Entry]\nName=VLC\nIcon=vlc\nExec=/usr/bin/vlc --started-from-file %U\n"),
                 ("spotify.desktop", "[Desktop Entry]\nName=Spotify\nIcon=spotify-client\nStartupWMClass=spotify\n"), ("com.valvesoftware.Steam.desktop", "[Desktop Entry]\nName=Steam\nIcon=steam\n"),
                 ("bad.desktop", "[Desktop Entry]\nName=Evil\nIcon=../evil\nExec=evil\n"), ("hidden.desktop", "[Desktop Entry]\nNoDisplay=true\nIcon=ghost\n"),
                 ("settings.desktop", "[Desktop Entry]\nName=Settings\nIcon=preferences-system\nExec=settings\n")]:
        desktop(n, t)


def import_cases(rng):
    pack = os.path.join(WORLD, "input", "Test Pack v1.2.3 (Patched).apk")
    out = os.path.join(WORLD, "themes")
    plain = "<resources>" + "".join(f'<item component="{c}" drawable="{d}"/>' for c, d in ITEMS) + "</resources>"

    def made(desktops=True, before=None, **kw):
        def setup():
            os.makedirs(os.path.dirname(pack), exist_ok=True)
            apk(pack, **kw)
            if desktops:
                import_desktops()
            if before:
                before()
        return setup
    base = ["--out", out]
    compare("import", "plain appfilter", made(plain=plain, pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "compiled UTF-16 appfilter", made(compiled=axml(ITEMS), pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "compiled UTF-8 appfilter", made(compiled=axml(ITEMS, utf8=True), pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "both, plain first", made(plain=plain, compiled=axml(ITEMS[:1]), pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "stored, not deflated", made(plain=plain, pics=PICS, compression=zipfile.ZIP_STORED), [pack] + base, tool="liki-icon-import")
    compare("import", "the default name and folder", made(plain=plain, pics=PICS), [pack], tool="liki-icon-import")
    compare("import", "a name of its own", made(plain=plain, pics=PICS), [pack, "--name", "Mine", "--out", out], tool="liki-icon-import")
    compare("import", "nothing installed matches", made(plain=plain, pics=PICS, desktops=False), [pack] + base, tool="liki-icon-import")
    compare("import", "no PNG", made(plain=plain), [pack] + base, tool="liki-icon-import")
    compare("import", "no appfilter", made(pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "an appfilter that does not parse", made(plain="<resources><item", compiled=b"\x03\x00junk", pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "namespaces, entities, a DTD", made(plain='<?xml version="1.0"?><!DOCTYPE r [<!ENTITY f "firefox">]><r xmlns:a="u"><item a:drawable="ic_vlc" component="x"/><item drawable="&f;" a:component="y"/></r>', pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "an undefined entity after an external DTD", made(plain='<?xml version="1.0"?><!DOCTYPE r SYSTEM "x.dtd"><r><item drawable="firefox"/>&nope;</r>', pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "UTF-16 XML", made(plain='<?xml version="1.0" encoding="UTF-16"?><r><item drawable="firefox"/></r>'.encode("utf-16"), pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "Latin-1 XML", made(plain='<?xml version="1.0" encoding="ISO-8859-1"?><r><item drawable="firefox" component="\xe9"/></r>'.encode("latin-1"), pics=PICS), [pack] + base, tool="liki-icon-import")
    compare("import", "dotless i and long s in folder names", made(plain=plain, pics=[("res/mıpmap/firefox.png", png(64, 64)), ("reſ/drawable/ic_vlc.png", png(64, 64)), ("res/drawable/spotify.png", png(64, 64))]), [pack] + base, tool="liki-icon-import")

    def old_theme():
        write(os.path.join(out, "Test Pack", "index.theme"), "old")
        write(os.path.join(out, "Test Pack", "64x64", "apps", "gone.png"), "old")
        write(os.path.join(out, "Other", "keep"), "keep")
    compare("import", "the old import is replaced", made(plain=plain, pics=PICS, before=old_theme), [pack] + base, tool="liki-icon-import")

    def linked_theme():
        write(os.path.join(WORLD, "elsewhere", "index.theme"), "x")
        os.makedirs(out, exist_ok=True)
        os.symlink(os.path.join(WORLD, "elsewhere"), os.path.join(out, "Test Pack"))
    compare("import", "the theme folder is a link", made(plain=plain, pics=PICS, before=linked_theme), [pack] + base, tool="liki-icon-import")
    compare("import", "not a zip", lambda: write(pack, b"not a zip at all, sorry"), [pack] + base, tool="liki-icon-import")
    compare("import", "no such file", lambda: None, [pack] + base, tool="liki-icon-import")
    compare("import", "a folder", lambda: os.makedirs(pack), [pack] + base, tool="liki-icon-import")
    for argv in ([], ["--help"], ["--name"], [pack, pack], ["--out", out, "--", pack], [pack, "--na", "X", "--out", out], ["--bogus", pack], [pack, "--name="], ["-", "--out", out]):
        compare("import", f"argv {argv}", made(plain=plain, pics=PICS), argv, tool="liki-icon-import")
    for name in ["Camo Dark v1.8.1 (Patched).apk", "Pack_2.0[mod].zip", ".apk", "(only).apk", "Icons 1.2.3a.apk", "Ünïcode v2.0 pack ①.apk", "x.y.z", "v1.2", "a(b.apk", "über-pack v10.4.apk"]:
        p = os.path.join(WORLD, "input", name)

        def setup(p=p):
            os.makedirs(os.path.dirname(p), exist_ok=True)
            apk(p, plain=plain, pics=PICS)
            import_desktops()
        compare("import", f"default name of {name!r}", setup, [p, "--out", out], tool="liki-icon-import")

    # fuzzing: the compiled appfilter, the plain one, the archive
    good_axml = axml(ITEMS + [("ComponentInfo{a.b/c}", "xé\U0001f600")], strings_extra=["\ud800lone"])
    good_axml8 = axml(ITEMS, utf8=True)
    for k in range(ARGS.fuzz):
        data = bytearray(rng.choice([good_axml, good_axml8]))
        for _ in range(rng.randint(1, 5)):
            i = rng.randrange(len(data))
            if rng.random() < 0.8:
                data[i] = rng.randrange(256)
            else:
                del data[i:i + rng.randint(1, 16)]
        compare("import-axml", f"fuzz {k}", made(compiled=bytes(data), pics=PICS, desktops=False), [pack] + base, tool="liki-icon-import")
    xml_bits = ['<item drawable="firefox" component="ComponentInfo{a/b}"/>', '<item drawable="ic_vlc"/>', "<!-- c -->", "<?pi x?>", "&amp;", "&#65;", "<![CDATA[x]]>", '<i:item xmlns:i="u" drawable="steam"/>']
    for k in range(ARGS.fuzz):
        text = "<resources>" + "".join(rng.choice(xml_bits) for _ in range(rng.randint(1, 6))) + "</resources>"
        data = bytearray(text.encode())
        for _ in range(rng.randint(0, 3)):
            i = rng.randrange(len(data))
            data[i] = rng.choice(b"<>&\"'=/ \x00\xff\xc3!?;#x") if rng.random() < 0.7 else rng.randrange(256)
        compare("import-xml", f"fuzz {k}", made(plain=bytes(data), pics=PICS, desktops=False), [pack] + base, tool="liki-icon-import")
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("assets/appfilter.xml", plain)
        for n, d in PICS[:5]:
            z.writestr(n, d)
        z.comment = b"a comment"
    good_zip = buf.getvalue()
    for k in range(ARGS.fuzz):
        data = bytearray(good_zip)
        for _ in range(rng.randint(1, 4)):
            r = rng.random()
            i = rng.randrange(len(data))
            if r < 0.6:
                data[i] = rng.randrange(256)
            elif r < 0.8:
                del data[i:i + rng.randint(1, 30)]
            else:
                data[i:i] = data[rng.randrange(len(data)):][:rng.randint(1, 30)]
        compare("import-zip", f"fuzz {k}", lambda d=bytes(data): write(pack, d), [pack] + base, tool="liki-icon-import")
    # the archive's own fields only (headers, central directory, end record), the pictures' bytes left alone
    heads = [i for sig in (b"PK\x03\x04", b"PK\x01\x02", b"PK\x05\x06") for i in range(len(good_zip)) if good_zip.startswith(sig, i)]
    for k in range(ARGS.fuzz):
        data = bytearray(good_zip)
        for _ in range(rng.randint(1, 3)):
            h = rng.choice(heads)
            i = h + rng.randrange(4, 46)
            if i < len(data):
                data[i] = rng.choice([0, 1, 0xFF, 0x80, rng.randrange(256), data[i] ^ (1 << rng.randrange(8))])
        compare("import-zip-fields", f"fuzz {k}", lambda d=bytes(data): write(pack, d), [pack] + base, tool="liki-icon-import")
    crafted_zip_cases(pack, base, plain)


def crafted_zip_cases(pack, base, plain):
    def zipbytes(entries, comment=b"", zip64=False):
        import warnings
        warnings.simplefilter("ignore")
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
            for e in entries:
                if isinstance(e[0], zipfile.ZipInfo):
                    z.writestr(e[0], e[1], compress_type=e[0].compress_type)
                else:
                    n, d = e
                    if zip64:
                        with z.open(n, "w", force_zip64=True) as f:
                            f.write(d.encode() if isinstance(d, str) else d)
                    else:
                        z.writestr(n, d)
            z.comment = comment
        return buf.getvalue()

    def info(name, data, extra=b"", flag=0, method=zipfile.ZIP_DEFLATED):
        i = zipfile.ZipInfo(name)
        i.extra = extra
        i.flag_bits |= flag
        i.compress_type = method
        return i, data
    pics = [(n, d) for n, d in PICS[:5]]
    ok = [("assets/appfilter.xml", plain)] + pics
    cases = {
        "a comment": zipbytes(ok, comment=b"PK\x05\x06 a comment that looks like an end"),
        "a plain comment": zipbytes(ok, comment=b"made by hand"),
        "zip64 members": zipbytes(ok, zip64=True),
        "data before the archive": b"MZ" + b"\x00" * 300 + zipbytes(ok),
        "the same name twice": zipbytes(ok + [("res/drawable-nodpi/firefox.png", png(300, 300))]),
        "names differing in case": zipbytes(ok + [("ASSETS/AppFilter.XML", "<r><item drawable='ic_vlc'/></r>")]),
        "a Unicode path field": zipbytes(ok[:1] + [info("res/drawable/old.png", png(64, 64), extra=struct.pack("<HHBL", 0x7075, 5 + len(b"res/drawable/firefox.png"), 1, zlib.crc32(b"res/drawable/old.png")) + b"res/drawable/firefox.png")]),
        "a Unicode path field with a wrong crc": zipbytes(ok[:1] + [info("res/drawable/old.png", png(64, 64), extra=struct.pack("<HHBL", 0x7075, 9, 1, 0) + b"abcd")]),
        "a broken extra field": zipbytes(ok[:1] + [info("res/drawable/firefox.png", png(64, 64), extra=b"\x01\x00\xff\x00")]),
        "an encrypted member": zipbytes(ok[:1] + [info("res/drawable/firefox.png", png(64, 64), flag=0x1)]),
        "a member of an unknown method": zipbytes(ok[:1] + [info("res/drawable/firefox.png", png(64, 64), method=zipfile.ZIP_STORED)]).replace(b"PK\x01\x02\x14\x03\x14\x00\x00\x00\x00\x00", b"PK\x01\x02\x14\x03\x14\x00\x00\x00\x63\x00", 1),
        "empty": b"PK\x05\x06" + b"\x00" * 18,
        "too short": b"PK\x05\x06",
    }
    nul = bytearray(zipbytes(ok + [("res/drawable/vlcXextra.png", png(64, 64))]))
    for i in range(len(nul) - 5):
        if nul[i:i + 5] == b"vlcXe":
            nul[i + 3] = 0
    cases["a NUL in a name"] = bytes(nul)
    # a deflated picture that inflates to more than its stated size
    raw = png(128, 128) + b"\x00" * 200000
    big = zlib.compressobj(9, zlib.DEFLATED, -15)
    comp = big.compress(raw) + big.flush()
    small = png(128, 128)
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w") as z:
        z.writestr("assets/appfilter.xml", plain)
        z.writestr("res/drawable/firefox.png", comp)
    data = bytearray(buf.getvalue())
    # the member says: deflated, the small picture's size and CRC; its stream inflates to far more
    for sig, at_method, at_crc, at_usize in ((b"PK\x03\x04", 8, 14, 22), (b"PK\x01\x02", 10, 16, 24)):
        i = data.index(sig, data.index(b"firefox.png") - 60 if sig == b"PK\x03\x04" else 0)
        while data[i + (30 if sig == b"PK\x03\x04" else 46):][:24] != b"res/drawable/firefox.png":
            i = data.index(sig, i + 4)
        struct.pack_into("<H", data, i + at_method, 8)
        struct.pack_into("<I", data, i + at_crc, zlib.crc32(small))
        struct.pack_into("<I", data, i + at_usize, len(small))
    cases["a member longer than it says"] = bytes(data)
    # overlapping members: the second local header placed inside the first member's data
    over = bytearray(zipbytes(ok))
    cd = over.index(b"PK\x01\x02")
    cd2 = over.index(b"PK\x01\x02", cd + 4)
    struct.pack_into("<I", over, cd2 + 42, struct.unpack_from("<I", over, cd + 42)[0] + 2)
    cases["overlapping members"] = bytes(over)
    same = bytearray(zipbytes(ok))
    struct.pack_into("<I", same, cd2 + 42, struct.unpack_from("<I", same, cd + 42)[0])
    cases["two members at the same place"] = bytes(same)
    for name, data in cases.items():
        compare("import-crafted", name, lambda d=data: (write(pack, d), import_desktops()), [pack] + base, tool="liki-icon-import")


def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--python", default=os.path.join(PKG, "python"))
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(PKG, "..", "..", "target", "release")))
    ap.add_argument("--only")
    ap.add_argument("--fuzz", type=int, default=300)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("-v", "--verbose", action="store_true")
    ARGS = ap.parse_args()
    rng = random.Random(ARGS.seed)
    try:
        if ARGS.only in (None, "build"):
            build_cases()
        if ARGS.only in (None, "icns"):
            icns_cases(rng)
        if ARGS.only in (None, "import"):
            import_cases(rng)
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)
    bad = 0
    for g, (n, m) in STATS.items():
        print(f"{g}: {n - m}/{n} the same")
        bad += m
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
