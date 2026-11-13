#!/usr/bin/env python3
"""parity-shell-ports: the Rust ports of the desktop's last shell scripts against the scripts themselves, on made-up worlds.

The scripts come from git (`git show REV:PATH`, REV below: the commit that fixed their bugs just before the port), so this
can be run again after they are gone. The ports are the release builds of each package (cargo build --release in
packages/session, panel, liki, spell and agora-helpers). Each case builds a world in a scratch folder: a home, a runtime
folder, the files the case needs, and a PATH of stand-ins (each writes down its name and arguments and answers as the case
says) followed by links to the few real tools the scripts use (bash, sed, find, zstd ...), so a program a case does not give is
not there. Nothing reaches the desktop, the session bus, Hyprland, the sound server or the network. Where a program reads a
fixed path (/usr/bin/yggdrasil-spell, the plugin's source, Liki's index.theme, the icon packs, Minos's logos), it runs in a
user and mount namespace of its own with the case's file bound over that path (unshare -rm); the screen reader's cases run in
a process namespace too, so pgrep and the port see only the case's stand-in Orca.

For each case both run in a fresh copy of the world, and these are compared: exit code, standard output, standard error, the
calls the stand-ins wrote down (in order) and every file left in the world (type, mode, content). Known differences by design
(the wording of "not found" messages, which the shell words with its own line numbers) are listed per case and checked to be
the only ones.

    python3 -I tools/parity-shell-ports.py [--old REV] [--only NAME,...] [--bench N] [--json FILE] [-v]

--bench N also times the everyday cases (BENCH) N times per side (alternating, after two warm-up runs), with bench-port's launcher (wall time,
CPU time with children, peak RSS), at nice 19.
"""
import argparse
import hashlib
import importlib.util
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
PKG = os.path.join(REPO, "packages")
# the scripts as they were last, with their bugs fixed (the commit before the port)
OLD_REV = "f54dc78"
SCRIPTS = {
    "yggdrasil-zoom": ("session", "packages/session/bin/yggdrasil-zoom"),
    "yggdrasil-screen-reader": ("session", "packages/session/bin/yggdrasil-screen-reader"),
    "yggdrasil-hypr-plugin": ("session", "packages/session/bin/yggdrasil-hypr-plugin"),
    "yggdrasil-hypr-logkeep": ("session", "packages/session/bin/yggdrasil-hypr-logkeep"),
    "yggdrasil-panelctl": ("panel", "packages/panel/bin/yggdrasil-panelctl"),
    "yggdrasil-mediakey": ("panel", "packages/panel/bin/yggdrasil-mediakey"),
    "yggdrasil-mount-open": ("panel", "packages/panel/bin/yggdrasil-mount-open"),
    "liki-icon": ("liki", "packages/liki/bin/liki-icon"),
    "liki-minos-icon": ("liki", "packages/liki/bin/liki-minos-icon"),
    "spell-global": ("spell", "packages/spell/spell-global"),
    "agora-sudo": ("agora-helpers", "packages/market/helpers/sudo"),
}
# the real tools a world's PATH has (after the stand-ins)
TOOLS = ["bash", "sh", "env", "cat", "head", "tail", "cut", "sort", "rm", "mv", "cp", "mkdir", "basename", "dirname", "date",
         "seq", "sleep", "mktemp", "printf", "echo", "find", "xargs", "sed", "awk", "grep", "readlink", "touch", "true", "false",
         "zstd", "kill", "mount", "chmod", "ln", "pgrep", "pkill", "setsid", "python3"]
ARGS = None
SCRATCH = None

STAND_IN = """#!/bin/sh
{ printf '%s' "$(basename "$0")"; for a in "$@"; do printf '\\037%s' "$a"; done; printf '\\036\\n'; } >> "$CALLS"
"""


class World:
    """A case's world: files under W, stand-ins in W/stubs, and what it runs inside (a namespace) when it needs one."""

    def __init__(self, w):
        self.w = w
        self.home = os.path.join(w, "home")
        self.run = os.path.join(w, "run")
        self.stubs = os.path.join(w, "stubs")
        self.binds = []        # (source in the world, fixed path it is bound over)
        self.pidns = False
        self.before = []       # shell lines run in the namespace before the program (a stand-in Orca started ...)
        self.after = []        # shell lines run after it (is that Orca still there ...)
        self.env = {}
        for d in (self.home, self.run, self.stubs):
            os.makedirs(d, exist_ok=True)
        os.chmod(self.run, 0o700)

    def file(self, rel, data="", mode=0o644, mtime=None):
        p = os.path.join(self.w, rel)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.write(data.encode() if isinstance(data, str) else data)
        os.chmod(p, mode)
        if mtime is not None:
            os.utime(p, (mtime, mtime), follow_symlinks=False)
        return p

    def stub(self, name, body="exit 0\n"):
        """A stand-in NAME on the PATH: writes down its arguments, then runs BODY (sh)."""
        self.file(os.path.join("stubs", name), STAND_IN + body, 0o755)

    def bind(self, rel, fixed):
        self.binds.append((os.path.join(self.w, rel), fixed))


def link_tools(d):
    os.makedirs(d, exist_ok=True)
    for t in TOOLS:
        p = shutil.which(t, path="/usr/bin:/bin")
        if p:
            os.symlink(p, os.path.join(d, t))


def old_scripts(rev):
    d = os.path.join(SCRATCH, "old")
    os.makedirs(d)
    out = {}
    for name, (_, path) in SCRIPTS.items():
        if rev == "worktree":
            data = open(os.path.join(REPO, path), "rb").read()
        else:
            data = subprocess.run(["git", "-C", REPO, "show", f"{rev}:{path}"], capture_output=True, check=True).stdout
        p = os.path.join(d, name)
        with open(p, "wb") as f:
            f.write(data)
        os.chmod(p, 0o755)
        out[name] = p
    return out


def new_binary(name):
    crate = SCRIPTS[name][0]
    for d in (os.path.join(PKG, crate, "target", "release"), os.path.join(REPO, "target", "release")):
        p = os.path.join(d, name)
        if os.path.exists(p):
            return p
    raise SystemExit(f"parity-shell-ports: no {name} (cargo build --release in packages/{crate})")


def wrapper(world):
    """The command line that runs a program inside the case's namespaces (none when it needs none)."""
    if not world.binds and not world.pidns and not world.before and not world.after:
        return []
    lines = [f"mount --bind '{src}' '{dst}' || exit 99" for src, dst in world.binds] + world.before
    if world.after:
        lines.append('"$@"; rc=$?')
        lines += world.after
        lines.append("exit $rc")
    else:
        lines.append('exec "$@"')
    flags = ["-rm"] + (["-pf", "--mount-proc"] if world.pidns else [])
    return ["/usr/bin/unshare"] + flags + ["--", os.path.join(SCRATCH, "tools", "sh"), "-c", "\n".join(lines), "sh"]


def environment(world):
    e = {"PATH": f"{world.stubs}:{os.path.join(SCRATCH, 'tools')}", "HOME": world.home, "XDG_RUNTIME_DIR": world.run, "LC_ALL": "C",
         "CALLS": os.path.join(world.w, "calls"), "TMPDIR": os.path.join(world.w, "tmp")}
    os.makedirs(e["TMPDIR"], exist_ok=True)
    e.update(world.env)
    return {k: v for k, v in e.items() if v is not None}


def snapshot(w, skip=("stubs", "calls", "tmp")):
    """Every file left in the world: path -> (type, mode, content hash or link target)."""
    out = {}
    for root, dirs, files in os.walk(w):
        rel_root = os.path.relpath(root, w)
        if rel_root.split(os.sep)[0] in skip:
            dirs[:] = []
            continue
        for n in dirs + files:
            p = os.path.join(root, n)
            rel = os.path.normpath(os.path.join(rel_root, n))
            if rel.split(os.sep)[0] in skip:
                continue
            st = os.lstat(p)
            if os.path.islink(p):
                out[rel] = ("link", os.readlink(p))
            elif os.path.isdir(p):
                out[rel] = ("dir", oct(st.st_mode & 0o7777))
            else:
                out[rel] = ("file", oct(st.st_mode & 0o7777), hashlib.sha256(open(p, "rb").read()).hexdigest()[:16])
    return out


def normalise(case, text):
    for a, b in case.get("replace", []):
        text = text.replace(a, b)
    return text


def run_side(case, prog):
    w = os.path.join(SCRATCH, "w")
    shutil.rmtree(w, ignore_errors=True)
    world = World(w)
    case["world"](world)
    argv = wrapper(world) + [prog] + case.get("args", [])
    p = subprocess.run(argv, env=environment(world), capture_output=True, input=case.get("stdin", b""), timeout=60)
    time.sleep(case.get("settle", 0))
    calls = open(world.env.get("CALLS", os.path.join(w, "calls")), "rb").read().decode("utf-8", "replace") if os.path.exists(os.path.join(w, "calls")) else ""
    files = snapshot(w)
    if case.get("rename"):
        files = {case["rename"](k): v for k, v in files.items()}
    r = {"exit": p.returncode, "out": p.stdout.decode("utf-8", "replace"), "err": p.stderr.decode("utf-8", "replace"),
         "calls": calls.replace(w, "W"), "files": files}
    for k in ("out", "err"):
        r[k] = normalise(case, r[k].replace(w, "W"))
    return r


def compare(name, case, old, new):
    a, b = run_side(case, old), run_side(case, new)
    allowed = set(case.get("differs", ()))
    diffs = [k for k in ("exit", "out", "err", "calls", "files") if a[k] != b[k] and k not in allowed]
    if case.get("want"):
        bad = case["want"](a, b)
        if bad:
            diffs.append(f"want: {bad}")
    if diffs or ARGS.v:
        print(f"  {'DIFF' if diffs else 'same'} {name}: {case['label']} {diffs or ''}")
    if diffs:
        for k in diffs:
            if k in a:
                print(f"    old {k}: {a[k]!r}\n    new {k}: {b[k]!r}")
    return not diffs, a, b


# ---------------------------------------------------------------- the cases

def cases_zoom():
    out = []
    for label, args, body, *extra in [
        ("in, from off", ["in"], "case $1 in getoption) printf 'float: 1.000000\\nset: false\\n';; esac\n"),
        ("out, from 2.5", ["out"], "case $1 in getoption) printf 'float: 2.500000\\nset: true\\n';; esac\n"),
        ("off", ["off"], "case $1 in getoption) printf 'float: 4.000000\\n';; esac\n"),
        ("in at the top", ["in"], "case $1 in getoption) printf 'float: 8.000000\\n';; esac\n"),
        ("in, rounded as printf (1.75)", ["in"], "case $1 in getoption) printf '  float:\\t1.75 x\\n';; esac\n"),
        ("in, no float line", ["in"], "case $1 in getoption) echo 'no such option';; esac\n"),
        ("in, Hyprland gone", ["in"], "case $1 in getoption) echo 'HYPRLAND_INSTANCE_SIGNATURE not set' >&2; exit 1;; eval) exit 3;; esac\n"),
        ("out, eval fails with a message", ["out"], "case $1 in getoption) printf 'float: 3\\n';; eval) echo 'error: bad' >&2; exit 4;; esac\n"),
        ("a wrong word", ["up"], ""),
        ("no word", [], ""),
    ]:
        out.append({"label": label, "args": args, "world": (lambda b: lambda w: w.stub("hyprctl", b))(body)})
    out.append({"label": "hyprctl not installed", "args": ["in"], "world": lambda w: None, "differs": ["err"],
                "want": lambda a, b: None if b["err"] == "yggdrasil-zoom: hyprctl: not found\n" and "not found" in a["err"] else b["err"]})
    return out


def cases_screen_reader():
    def orca_stub(w):
        # Orca itself: says it started (its arguments), then stays, as Orca does
        w.stub("orca", "exec sleep 30\n")

    def running(w):
        w.file("bin/orca", "#!/bin/sh\nwhile :; do sleep 0.05; done\n", 0o755)
        w.before.append(f"'{w.w}/bin/orca' & o=$!; sleep 0.2")
        w.after.append("sleep 0.2; if kill -0 $o 2>/dev/null; then echo orca-still-running >> \"$CALLS\"; kill $o; else echo orca-stopped >> \"$CALLS\"; fi")
        w.pidns = True

    def started(w):
        orca_stub(w)
        w.pidns = True
        w.after.append("sleep 0.3")

    return [
        {"label": "Orca running: stopped", "world": lambda w: (running(w), w.stub("notify-send"))},
        {"label": "Orca running, notify-send fails", "world": lambda w: (running(w), w.stub("notify-send", "exit 5\n"))},
        {"label": "Orca running, no notify-send", "world": lambda w: running(w)},
        {"label": "Orca installed: started", "world": lambda w: (started(w), w.stub("notify-send"))},
        {"label": "no Orca", "world": lambda w: (w.__setattr__("pidns", True), w.stub("notify-send"))},
        {"label": "no Orca, no notify-send", "world": lambda w: w.__setattr__("pidns", True)},
    ]


SRC_CPP = "/usr/share/yggdrasil/hyprland/plugin/minimize.cpp"
COMMIT = "0123456789abcdef0123"


def cases_hypr_plugin():
    def world(version=None, listed=False, load="ok", gxx="ok", pkgconf=True, so=None, src_mtime=1_700_000_000):
        def make(w):
            v = version if version is not None else json.dumps({"branch": "main", "commit": COMMIT, "tag": "v0.57.1"})
            w.file("version.json", v)
            w.stub("hyprctl", f"""case "$1 $2" in
  "-j version") cat '{w.w}/version.json';;
  "plugin list") {"echo 'Plugin yggdrasil-minimize by Yggdrasil'" if listed else "echo 'no plugins loaded'"};;
  "plugin load") {"echo ok" if load == "ok" else "echo 'could not load: " + load + "' >&2; echo partial"};;
esac
""")
            if gxx:
                w.stub("g++", 'out=; prev=; for a in "$@"; do [ "$prev" = -o ] && out=$a; prev=$a; done\n'
                       + ('printf so > "$out"\necho built-warning >&2\n' if gxx == "ok" else "echo 'error: no member' >&2; exit 1\n"))
            if pkgconf:
                w.stub("pkg-config", "printf -- '-I/usr/include/hyprland  -I/usr/include/pixman-1\\t-DWLR_USE_UNSTABLE \\n'\n")
            else:
                w.stub("pkg-config", "echo 'Package hyprland was not found' >&2; exit 1\n")
            w.file("src.cpp", "// the plugin\n", mtime=src_mtime)
            w.bind("src.cpp", SRC_CPP)
            cache = "home/.cache/yggdrasil/hyprland-plugin"
            w.file(f"{cache}/0ld0ld0/yggdrasil-minimize.so", "old")
            w.file(f"{cache}/README", "a file, not a folder: stays")
            if so is not None:
                w.file(f"{cache}/{COMMIT}/yggdrasil-minimize.so", so[0], mtime=so[1])
        return make
    return [
        {"label": "already loaded", "world": world(listed=True)},
        {"label": "Hyprland not answering", "world": world(version="not json")},
        {"label": "commit not hex", "world": world(version=json.dumps({"commit": "ABCDEF12"}))},
        {"label": "commit too short", "world": world(version=json.dumps({"commit": "abc"}))},
        {"label": "commit with a newline at its end", "world": world(version=json.dumps({"commit": COMMIT + "\n"}))},
        {"label": "no compiler", "world": world(gxx=None)},
        {"label": "no headers", "world": world(pkgconf=False)},
        {"label": "built and loaded", "world": world()},
        {"label": "build fails", "world": world(gxx="fail")},
        {"label": "built before, newer than the source: loaded", "world": world(so=("built", 1_800_000_000))},
        {"label": "built before, older than the source: built again", "world": world(so=("built", 1_600_000_000))},
        {"label": "built before but empty: built again", "world": world(so=("", 1_800_000_000))},
        {"label": "loading fails", "world": world(load="wrong version", so=("built", 1_800_000_000))},
        {"label": "XDG_CACHE_HOME", "world": lambda w: (world()(w), w.env.update(XDG_CACHE_HOME=os.path.join(w.w, "cache")))},
    ]


def cases_logkeep():
    def world(sig="abc_123", log=True, lock="999999\n", logs=True):
        def make(w):
            if sig is not None:
                w.env["HYPRLAND_INSTANCE_SIGNATURE"] = sig
            inst = f"run/hypr/{sig}"
            if log:
                w.file(f"{inst}/hyprland.log", "line 1\nline 2\n")
            if lock is not None:
                w.file(f"{inst}/hyprland.lock", lock)
            if logs:
                d = "home/.local/state/yggdrasil/hyprland-logs"
                w.file(f"{d}/2026-11-01-0900-hyprland.log", "an older session\n")
                w.file(f"{d}/2026-11-02-0900-hyprland.log", "another\n")
                w.file(f"{d}/.hidden.log", "not matched by *.log\n")
                for i in range(7):
                    w.file(f"{d}/2026-10-2{i}-0900-hyprland.log.zst", f"z{i}", mtime=1_700_000_000 + i * 100)
                w.file(f"{d}/same-a.log.zst", "s", mtime=1_700_000_250)
                w.file(f"{d}/same-b.log.zst", "s", mtime=1_700_000_250)
        return make
    import re
    date = re.compile(r"\d{4}-\d{2}-\d{2}-\d{4}-hyprland\.log$")
    rename = lambda k: date.sub("DATE-hyprland.log", k) if "hyprland-logs" in k and not k.startswith("home/.local/state/yggdrasil/hyprland-logs/2026") else k
    return [
        {"label": "no Hyprland", "world": world(sig=None)},
        {"label": "a signature with a slash", "world": world(sig="../x")},
        {"label": "kept, compressed, the oldest removed, followed", "world": world(), "rename": rename},
        {"label": "first run (no folder yet)", "world": world(logs=False), "rename": rename},
        {"label": "a lock without a process number", "world": world(lock="hyprland\n"), "rename": rename},
        {"label": "no log within five seconds", "world": world(log=False), "rename": rename},
        {"label": "XDG_STATE_HOME", "world": lambda w: (world()(w), w.env.update(XDG_STATE_HOME=os.path.join(w.w, "state"))), "rename": rename},
    ]


def cases_panelctl():
    out = []
    for cmd in ["start", "calendar", "notifications", "fylgja", "ask", "ask-press", "ask-release", "zones", "zones-drag", "zones-drop",
                "shortcuts", "cursor-tap"]:
        out.append({"label": cmd, "args": [cmd], "world": lambda w: w.stub("busctl")})
    out += [
        {"label": "fylgja 4", "args": ["fylgja", "4"], "world": lambda w: w.stub("busctl")},
        {"label": "start, extra words", "args": ["start", "x", "y"], "world": lambda w: w.stub("busctl")},
        {"label": "clipboard", "args": ["clipboard", "x"], "world": lambda w: w.stub("yggdrasil-clipboard", "exit 6\n")},
        {"label": "no panel", "args": ["ask"], "world": lambda w: w.stub("busctl", "echo 'Call failed: The name is not activatable' >&2; exit 1\n")},
        {"label": "a wrong word", "args": ["open"], "world": lambda w: w.stub("busctl")},
        {"label": "no word", "args": [], "world": lambda w: w.stub("busctl")},
        {"label": "busctl not installed", "args": ["start"], "world": lambda w: None, "differs": ["err"],
         "want": lambda a, b: None if b["err"] == "yggdrasil-panelctl: busctl: not found\n" and "not found" in a["err"] else b["err"]},
    ]
    return out


def cases_mediakey():
    bus = ("case \"$2\" in list) printf ':1.5 123 x\\norg.freedesktop.Notifications 9 y\\norg.mpris.MediaPlayer2.vlc 77 vlc u\\n"
           "org.mpris.MediaPlayer2.spotify 8 s\\n';; esac\n")
    out = []
    for key in ["volume-up", "volume-down", "mute", "mic-mute", "brightness-up", "brightness-down"]:
        out.append({"label": key, "args": [key], "world": lambda w: (w.stub("wpctl"), w.stub("brightnessctl"))})
    for key in ["play-pause", "next", "previous", "stop"]:
        out.append({"label": key, "args": [key], "world": lambda w: w.stub("busctl", bus)})
    out += [
        {"label": "play-pause, no player", "args": ["play-pause"], "world": lambda w: w.stub("busctl", "case \"$2\" in list) echo ':1.5 1 x';; esac\n")},
        {"label": "play-pause, no busctl", "args": ["play-pause"], "world": lambda w: None},
        {"label": "next, the player fails", "args": ["next"],
         "world": lambda w: w.stub("busctl", bus.replace("esac", "call) echo 'Call failed' >&2; exit 1;; esac"))},
        {"label": "mute fails", "args": ["mute"], "world": lambda w: w.stub("wpctl", "echo nope >&2; exit 4\n")},
        {"label": "brightness, not installed", "args": ["brightness-up"], "world": lambda w: None, "differs": ["err"],
         "want": lambda a, b: None if b["err"] == "yggdrasil-mediakey: brightnessctl: not found\n" else b["err"]},
        {"label": "a wrong key", "args": ["louder"], "world": lambda w: None},
        {"label": "no key", "args": [], "world": lambda w: None},
    ]
    return out


def cases_mount_open():
    def world(findmnt="", udisks="Mounted /dev/sdb1 at /run/media/me/USB.", code=0):
        def make(w):
            w.stub("findmnt", f"printf '%s' '{findmnt}'\n")
            w.stub("udisksctl", f"printf '%s\\n' '{udisks}'\nexit {code}\n")
            w.stub("setsid")
        return make
    return [
        {"label": "no device", "args": [], "world": world()},
        {"label": "not a device", "args": ["sdb1"], "world": world()},
        {"label": "a bad name", "args": ["/dev/sdb1;x"], "world": world()},
        {"label": "a name with a space", "args": ["/dev/sd b"], "world": world()},
        {"label": "mounted already", "args": ["/dev/sdb1"], "world": world(findmnt="/run/media/me/USB\n/mnt/second\n")},
        {"label": "mounted now", "args": ["/dev/sdb1"], "world": world()},
        {"label": "mounted now, a name with ' at ' in it", "args": ["/dev/sdb1"], "world": world(udisks="Mounted /dev/sdb1 at /run/media/me/Photos at home.")},
        {"label": "mounted now, a name with spaces", "args": ["/dev/nvme0n1p3"], "world": world(udisks="Mounted /dev/nvme0n1p3 at /run/media/me/My Disk.")},
        {"label": "udisks refuses", "args": ["/dev/sdb1"], "world": world(udisks="Error mounting /dev/sdb1: GDBus.Error: Not authorized", code=1)},
        {"label": "udisks says something else", "args": ["/dev/sdb1"], "world": world(udisks="Nothing")},
    ]


def cases_sudo():
    def world(code=0):
        return lambda w: w.stub("pkexec", f"exit {code}\n")
    return [
        {"label": "-v", "args": ["-v"], "world": world()},
        {"label": "nothing", "args": [], "world": world()},
        {"label": "pacman, no cancel file", "args": ["pacman", "-S", "--noconfirm", "x"], "world": world()},
        {"label": "pacman with a cancel file", "args": ["-E", "pacman", "-U", "/tmp/x.pkg"], "world": lambda w: (world()(w), w.env.update(AGORA_CANCEL_FILE="/run/user/1000/agora-cancel"))},
        {"label": "another program with a cancel file", "args": ["--", "systemctl", "restart", "x"], "world": lambda w: (world()(w), w.env.update(AGORA_CANCEL_FILE="/c"))},
        {"label": "an empty cancel file name", "args": ["pacman", "-Syu"], "world": lambda w: (world()(w), w.env.update(AGORA_CANCEL_FILE=""))},
        {"label": "-- then an option-like word", "args": ["-k", "--", "-weird", "a b"], "world": world()},
        {"label": "pkexec refused", "args": ["pacman", "-R", "x"], "world": world(126)},
        {"label": "an empty word", "args": ["", "x"], "world": world()},
    ]


def cases_spell_global():
    def world(w):
        w.file("yggdrasil-spell", STAND_IN + "exit 0\n", 0o755)
        w.bind("yggdrasil-spell", "/usr/bin/yggdrasil-spell")
    return [
        {"label": "approve", "args": ["approve", "colour", "Yggdrasil"], "world": world},
        {"label": "deny", "args": ["deny", "teh"], "world": world},
        {"label": "approve, no words", "args": ["approve"], "world": world},
        {"label": "another command", "args": ["merge"], "world": world},
        {"label": "nothing", "args": [], "world": world},
    ]


ICNS = None


def icns_bytes(sizes):
    """A made .icns with a PNG chunk of each size (w, h)."""
    body = b""
    for i, (wd, ht) in enumerate(sizes):
        png = b"\x89PNG\r\n\x1a\n" + b"\x00\x00\x00\x0dIHDR" + wd.to_bytes(4, "big") + ht.to_bytes(4, "big") + bytes([i]) * 20
        body += b"ic%02d" % (7 + i) + (len(png) + 8).to_bytes(4, "big") + png
    return b"icns" + (len(body) + 8).to_bytes(4, "big") + body


def cases_liki_icon():
    def world(index=True, user_index=False):
        def make(w):
            w.stub("gtk-update-icon-cache")
            os.symlink(new_binary("liki-icon").replace("liki-icon", "liki-icns"), os.path.join(w.stubs, "liki-icns"))
            sysdir = os.path.join(w.w, "sys-liki")
            os.makedirs(sysdir)
            if index:
                w.file("sys-liki/index.theme", "[Icon Theme]\nName=Liki\n")
            w.bind("sys-liki", "/usr/share/icons/Liki")
            if user_index:
                w.file("home/.local/share/icons/Liki/index.theme", "mine\n", 0o600)
            w.file("in/app.svg", "<svg/>")
            w.file("in/app.PNG", "png", 0o640)
            w.file("in/app.icns", icns_bytes([(128, 128), (512, 512), (256, 256)]))
            w.file("in/broken.icns", b"not icns")
            w.file("in/notes.txt", "x")
            w.file("home/.local/share/icons/Liki/apps/scalable/steam.png", "old")
            w.file("home/.local/share/icons/Liki/apps/scalable/steam.svg", "old")
            for n in ["Finder_dark.icns", "Spotify.png", "STEAM_new.SVG", "vs_code.svg", "Unknown.png", ".hidden.png", "This-PC.png", "Broken_vlc.icns"]:
                data = icns_bytes([(64, 64), (1024, 1024)]) if n.startswith("Finder") else b"not icns" if n.startswith("Broken") else n.encode()
                w.file(f"pack/{n}", data)
            os.makedirs(os.path.join(w.w, "pack", "Blender_dir.png"))
        return make
    W = "{W}"
    return [
        {"label": "an svg", "args": ["firefox", f"{W}/in/app.svg"], "world": world()},
        {"label": "a png (upper-case extension)", "args": ["steam", f"{W}/in/app.PNG"], "world": world()},
        {"label": "an icns", "args": ["steam", f"{W}/in/app.icns"], "world": world()},
        {"label": "a broken icns", "args": ["steam", f"{W}/in/broken.icns"], "world": world()},
        {"label": "a text file", "args": ["steam", f"{W}/in/notes.txt"], "world": world()},
        {"label": "a file that is not there", "args": ["steam", f"{W}/in/none.svg"], "world": world()},
        {"label": "three words", "args": ["a", f"{W}/in/app.svg", "b"], "world": world()},
        {"label": "the user's index.theme is kept", "args": ["firefox", f"{W}/in/app.svg"], "world": world(user_index=True)},
        {"label": "no index.theme in the system", "args": ["firefox", f"{W}/in/app.svg"], "world": world(index=False)},
        {"label": "reset", "args": ["--reset", "steam"], "world": world()},
        {"label": "reset, no app", "args": ["--reset"], "world": world()},
        {"label": "import", "args": ["--import", f"{W}/pack"], "world": world()},
        {"label": "import, folder with a slash", "args": ["--import", f"{W}/pack/"], "world": world()},
        {"label": "import, not a folder", "args": ["--import", f"{W}/none"], "world": world()},
        {"label": "nothing", "args": [], "world": world()},
    ]


def cases_minos_icon():
    def world(conf=None, pack_files=(), user_files=(), plain=True, loop=False):
        def make(w):
            w.stub("gtk-update-icon-cache")
            if conf is not None:
                w.file("home/.config/yggdrasil/appearance.conf", conf)
            os.makedirs(os.path.join(w.w, "sys-icons", "Liki"))
            for rel, data in pack_files:
                w.file(f"sys-icons/{rel}", data)
            for rel, data in user_files:
                w.file(f"home/.local/share/icons/{rel}", data)
            if loop:
                os.makedirs(os.path.join(w.w, "sys-icons/Pack/sub"), exist_ok=True)
                os.symlink("..", os.path.join(w.w, "sys-icons/Pack/sub/up"))
            w.bind("sys-icons", "/usr/share/icons")
            if plain:
                for m in ("dark", "light"):
                    w.file(f"minos/{m}.svg", f'<svg mode="{m}"><path stroke="@LINE@" fill="@LINE@"/>\n<g stroke="@LINE@"/></svg>\n')
            else:
                os.makedirs(os.path.join(w.w, "minos"))
            w.bind("minos", "/usr/share/liki/minos-icon")
            w.file("home/.local/share/icons/hicolor/scalable/apps/minos.svg", "stale")
            w.file("home/.local/share/icons/hicolor/256x256/apps/minos.png", "stale")
        return make
    if not os.path.isdir("/usr/share/liki/minos-icon"):
        print("  (no /usr/share/liki/minos-icon to bind over: Minos's logo cases are skipped)")
    pack = "accent = 200,100,50\nstyle = dark\nicons = Pack\n"
    return [
        {"label": "Liki (no settings yet)", "args": [], "world": world()},
        {"label": "a pack with its own minos", "args": [], "world": world(pack, [("Pack/apps/48/minos.png", "m")])},
        {"label": "a pack's file manager (svg), user copy", "args": [], "world": world(pack, [("Pack/apps/x.txt", "")], [("Pack/scalable/apps/org.kde.dolphin.svg", "d"), ("Pack/a/nautilus.svg", "n")])},
        {"label": "a pack's file manager, first name wins", "args": [], "world": world(pack, [("Pack/a/org.kde.dolphin.svg", "d"), ("Pack/b/system-file-manager.svg", "s")])},
        {"label": "a pack's largest png", "args": [], "world": world(pack, [("Pack/16/file-manager.png", "1"), ("Pack/256/file-manager.png", "12345"), ("Pack/48/file-manager.png", "123"), ("Pack/32/org.kde.dolphin.png", "123456789")])},
        {"label": "pngs of equal size", "args": [], "world": world(pack, [("Pack/a/file-manager.png", "12"), ("Pack/b/file-manager.png", "34")])},
        {"label": "nothing of the pack's: the logo, dark", "args": [], "world": world(pack, [("Pack/apps/other.svg", "o")])},
        {"label": "the logo, light, accent from the settings", "args": [], "world": world(pack.replace("dark", "light"))},
        {"label": "the logo, accent and style given", "args": ["10,20,30", "light"], "world": world(pack)},
        {"label": "accent '-', style given", "args": ["-", "dark"], "world": world(pack)},
        {"label": "accent given, style '' (Settings)", "args": ["1,2,3", ""], "world": world(pack.replace("dark", "light"))},
        {"label": "'- - plain' (fixed: '-' reads the style)", "args": ["-", "-", "plain"], "world": world(pack.replace("dark", "light"))},
        {"label": "an accent that is not three numbers (fixed)", "args": ["abc", "light"], "world": world(pack)},
        {"label": "a bad accent in the settings", "args": [], "world": world("accent = red\nicons = Pack\n")},
        {"label": "a pack name with a slash", "args": [], "world": world("icons = ../Pack\n", [("Pack/a/nautilus.svg", "n")])},
        {"label": "a pack that loops", "args": [], "world": world(pack, [("Pack/z/nautilus.svg", "n")], loop=True)},
        {"label": "a pack that is not installed", "args": [], "world": world("icons = Gone\n")},
        {"label": "no logo files (the package's are missing)", "args": [], "world": world(pack, plain=False), "differs": ["files"],
         "want": lambda a, b: None if a["exit"] == b["exit"] == 2 else (a["exit"], b["exit"])},
    ]


# the cases --bench times (one or two a program: the everyday ones)
BENCH = {
    "yggdrasil-zoom": ["in, from off"],
    "yggdrasil-screen-reader": ["Orca running: stopped"],
    "yggdrasil-hypr-plugin": ["already loaded", "built and loaded"],
    "yggdrasil-hypr-logkeep": ["kept, compressed, the oldest removed, followed"],
    "yggdrasil-panelctl": ["start"],
    "yggdrasil-mediakey": ["volume-up", "play-pause"],
    "yggdrasil-mount-open": ["mounted now"],
    "agora-sudo": ["pacman with a cancel file"],
    "spell-global": ["approve"],
    "liki-icon": ["an icns", "import"],
    "liki-minos-icon": ["a pack's largest png", "nothing of the pack's: the logo, dark"],
}

PROGRAMS = {
    "yggdrasil-zoom": cases_zoom,
    "yggdrasil-screen-reader": cases_screen_reader,
    "yggdrasil-hypr-plugin": cases_hypr_plugin,
    "yggdrasil-hypr-logkeep": cases_logkeep,
    "yggdrasil-panelctl": cases_panelctl,
    "yggdrasil-mediakey": cases_mediakey,
    "yggdrasil-mount-open": cases_mount_open,
    "agora-sudo": cases_sudo,
    "spell-global": cases_spell_global,
    "liki-icon": cases_liki_icon,
    "liki-minos-icon": cases_minos_icon,
}


def expand(case):
    """{W} in a case's arguments is its world's folder."""
    w = os.path.join(SCRATCH, "w")
    c = dict(case)
    c["args"] = [a.replace("{W}", w) for a in case.get("args", [])]
    return c


# ---------------------------------------------------------------- timing (--bench)

def load_bench():
    spec = importlib.util.spec_from_file_location("bench_port", os.path.join(REPO, "tools", "bench-port.py"))
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    m.SCRATCH = SCRATCH
    m.build_launcher()
    return m


def time_case(bm, case, old, new, n):
    def one(prog):
        w = os.path.join(SCRATCH, "w")
        shutil.rmtree(w, ignore_errors=True)
        world = World(w)
        case["world"](world)
        return bm.measure([prog] + case.get("args", []), environment(world), wrap=wrapper(world))
    for _ in range(2):
        one(old), one(new)
    res = {"old": [], "new": []}
    for i in range(n):
        for who in (("old", "new") if i % 2 == 0 else ("new", "old")):
            res[who].append(one(old if who == "old" else new))
    return {k: bm.summary(v) for k, v in res.items()}


def main():
    global ARGS, SCRATCH
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--old", default=OLD_REV, help="the scripts' revision (default %(default)s; 'worktree' for the files as they are)")
    ap.add_argument("--only", help="comma-separated: " + ", ".join(PROGRAMS))
    ap.add_argument("--bench", type=int, default=0, metavar="N", help="also time the everyday cases N times per side")
    ap.add_argument("--json", help="write the results (and timings) to this file")
    ap.add_argument("-v", action="store_true")
    ARGS = ap.parse_args()
    try:
        os.nice(19 - os.nice(0))
    except OSError:
        pass
    SCRATCH = tempfile.mkdtemp(prefix="parity-shell-")
    try:
        link_tools(os.path.join(SCRATCH, "tools"))
        old = old_scripts(ARGS.old)
        bm = load_bench() if ARGS.bench else None
        total = same = 0
        results = {}
        for name in (ARGS.only.split(",") if ARGS.only else PROGRAMS):
            new = new_binary(name)
            cases = [expand(c) for c in PROGRAMS[name]()]
            ok = 0
            results[name] = {"cases": []}
            for c in cases:
                good, a, b = compare(name, c, old[name], new)
                ok += good
                entry = {"label": c["label"], "same": good}
                if bm and c["label"] in BENCH.get(name, ()):
                    t = entry["time"] = time_case(bm, c, old[name], new, ARGS.bench)
                    o, n = t["old"], t["new"]
                    print(f"  time {c['label']}: script {o['wall_ms']:.2f} ms ({o['wall_p10_ms']:.2f}-{o['wall_p90_ms']:.2f}), CPU {o['cpu_ms']:.2f} ms, "
                          f"{o['rss_mb']:.1f} MB; Rust {n['wall_ms']:.2f} ms ({n['wall_p10_ms']:.2f}-{n['wall_p90_ms']:.2f}), CPU {n['cpu_ms']:.2f} ms, "
                          f"{n['rss_mb']:.1f} MB", flush=True)
                results[name]["cases"].append(entry)
            results[name]["size"] = {"old": os.path.getsize(old[name]), "new": os.path.getsize(new)}
            print(f"{name}: {ok}/{len(cases)} the same", flush=True)
            total += len(cases)
            same += ok
        print(f"all: {same}/{total} the same")
        if ARGS.json:
            with open(ARGS.json, "w") as f:
                json.dump(results, f, indent=1)
        return 0 if same == total else 1
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main())
