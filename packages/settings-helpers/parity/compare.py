#!/usr/bin/env python3
"""compare: the Rust helpers (account, bootscreen, game-fix, gpu-run) and the Python scripts they replace, run as programs on
the same made-up worlds, in dry runs: every system tool they call is a stand-in that writes down what it was asked (and
answers as told), every file they read or write is in a scratch folder, and the root helpers run as "root" of a user
namespace of their own (unshare -rm, with sandbox.sh putting the scratch /etc/mkinitcpio.conf, /proc/cmdline and
/sys/module in place for that process tree only). Nothing on the computer is changed.

For each case both run in the same world (made again from scratch for each, at the same path), and must give the same exit
code, standard output, standard error, calls to the tools (with what they read on standard input), and files afterwards,
byte for byte, apart from what differs on purpose. The locale is en_US.UTF-8 (pkexec passes the user's own on):

  - where the script stopped with a traceback, the Rust helper stops with exit code 1 and one line on standard error;
  - bootscreen's picture.png: the same size and mode, its pixels compared within a tolerance (the scaling filter and the
    PNG encoder are not Pillow's);
  - account: a name with a final newline, which the script's pattern let through to useradd, is refused (exit code 2);
  - game-fix: for a catalogue that is not JSON, the parser's own words after "cannot read FILE: " (Python's json and
    serde_json say it differently).

    compare.py [--python DIR] [--bin DIR] [--only NAME] [-v]

--python: the folder with the Python account, bootscreen, game-fix and gpu-run (default: ../../settings/helpers/python,
          else ../../settings/helpers)
--bin: the folder with the Rust binaries (default: ../../../target/release; build with cargo build --release)
"""
import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
HELPERS = os.path.normpath(os.path.join(HERE, "..", "..", "settings", "helpers"))
CATALOGUE = os.path.normpath(os.path.join(HERE, "..", "..", "settings", "data", "game-fixes.json"))
SANDBOX = os.path.join(HERE, "sandbox.sh")
ARGS = None
SCRATCH = tempfile.mkdtemp(prefix="settings-helpers-parity-")
WORLD = os.path.join(SCRATCH, "world")


def py_helper(name):
    for d in ([ARGS.python] if ARGS.python else []) + [os.path.join(HELPERS, "python"), HELPERS]:
        if os.path.exists(os.path.join(d, name)):
            return os.path.join(d, name)
    raise SystemExit(f"compare: no Python {name} (give --python)")


def write(path, data, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data.encode() if isinstance(data, str) else data)
    if mode:
        os.chmod(path, mode)


STUB = """#!/bin/sh
# a stand-in: writes down how it was called (and what it read, for chpasswd), answers as the case says
name=$(basename "$0")
printf '%s' "$name" >> "$WORLD/calls"
for a in "$@"; do printf ' [%s]' "$a" >> "$WORLD/calls"; done
echo >> "$WORLD/calls"
case "$name" in
  chpasswd) printf 'stdin: ' >> "$WORLD/calls"; cat >> "$WORLD/calls"; echo >> "$WORLD/calls" ;;
  loginctl) printf '%s\\n' "$SESSIONS" ;;
  getent) [ -n "$GETENT" ] && printf '%s\\n' "$GETENT" ;;
  plymouth-set-default-theme) if [ $# -eq 0 ]; then cat "$WORLD/theme" 2>/dev/null; else printf '%s' "$1" > "$WORLD/theme"; fi ;;
  hyprctl) [ -n "$HYPRCTL" ] && printf '%s' "$HYPRCTL" ;;
  xrandr) [ -n "$XRANDR" ] && printf '%s' "$XRANDR" ;;
  lspci) eval "printf '%s' \\"\\$LSPCI_$(echo "$2" | tr ':.' '__')\\"" ;;
  vulkaninfo) [ -n "$VULKANINFO" ] && printf '%s' "$VULKANINFO" ;;
esac
eval "fail=\\$FAIL_$(echo "$name" | tr '-' '_')"
if [ -n "$fail" ]; then printf '%s\\n' "$fail" >&2; printf 'out: %s\\n' "$fail"; exit 1; fi
exit 0
"""

TOOLS = ["useradd", "userdel", "usermod", "gpasswd", "chpasswd", "loginctl", "getent", "visudo", "plymouth-set-default-theme",
         "grub-mkconfig", "mkinitcpio", "hyprctl", "xrandr", "lspci", "vulkaninfo"]


def stubs(world, without=()):
    b = os.path.join(world, "bin")
    for t in TOOLS:
        if t not in without:
            write(os.path.join(b, t), STUB, 0o755)
    return b


def snapshot(root):
    out = {}
    for d, dirs, files in os.walk(root):
        dirs.sort()
        rel = os.path.relpath(d, root)
        if rel.startswith("bin") or "__pycache__" in rel:
            continue
        for n in sorted(files) + [x for x in dirs if os.path.islink(os.path.join(d, x))]:
            p = os.path.join(d, n)
            r = os.path.normpath(os.path.join(rel, n))
            if os.path.islink(p):
                out[r] = ("link", os.readlink(p))
            elif r.endswith("picture.png"):
                out[r] = ("picture", p)
            else:
                with open(p, "rb") as f:
                    out[r] = (oct(os.stat(p).st_mode & 0o777), f.read())
        for n in dirs:
            out[os.path.normpath(os.path.join(rel, n)) + "/"] = "dir"
    return out


def picture_same(a, b):
    from PIL import Image, ImageChops, ImageStat
    ia, ib = Image.open(a), Image.open(b)
    if ia.size != ib.size or ia.mode != ib.mode:
        return f"size/mode {ia.size} {ia.mode} vs {ib.size} {ib.mode}"
    diff = ImageStat.Stat(ImageChops.difference(ia.convert("RGBA"), ib.convert("RGBA"))).mean
    if max(diff) > 3.0:
        return f"mean pixel difference {diff}"
    return None


class Result:
    def __init__(self, p, files):
        self.code, self.out, self.err, self.files = p.returncode, p.stdout, p.stderr, files


class StandInHyprland:
    """A stand-in Hyprland socket in a folder of its own (outside the world): the Rust game-fix asks the compositor through
    yggdrasil-wm where the script ran hyprctl. It answers `j/monitors` as the hyprctl stand-in does ($HYPRCTL, else nothing)
    and writes the same line into the calls, so both sides' calls stay comparable. Its signature names no real Hyprland."""

    def __init__(self, answer):
        import socket
        import threading
        self.dir = tempfile.mkdtemp(prefix="ygg-par-")
        os.makedirs(os.path.join(self.dir, "hypr", "parity"))
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.bind(os.path.join(self.dir, "hypr", "parity", ".socket.sock"))
        self.sock.listen(4)
        self.sock.settimeout(0.2)
        self.answer, self.done = answer.encode(), False
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
                if q == "j/monitors":
                    with open(os.path.join(WORLD, "calls"), "a") as f:
                        f.write("hyprctl [-j] [monitors]\n")
                    c.sendall(self.answer)

    def env(self):
        return {"XDG_RUNTIME_DIR": self.dir, "HYPRLAND_INSTANCE_SIGNATURE": "parity"}

    def close(self):
        self.done = True
        self.thread.join()
        self.sock.close()
        shutil.rmtree(self.dir, ignore_errors=True)


def run_once(build, argv, env, stdin, sandbox):
    shutil.rmtree(WORLD, ignore_errors=True)
    os.makedirs(WORLD)
    extra_env = build(WORLD) or {}
    e = {"PATH": os.path.join(WORLD, "bin") + ":/usr/bin:/bin", "WORLD": WORLD, "HOME": os.path.join(WORLD, "home"), "LANG": "en_US.UTF-8"}
    e.update(extra_env)
    e.update(env)
    e = {k: v for k, v in e.items() if v is not None}
    hypr = StandInHyprland(e.get("HYPRCTL", ""))
    e.update(hypr.env())
    try:
        cmd = (["unshare", "-rm", "sh", SANDBOX, WORLD] if sandbox else []) + argv
        p = subprocess.run(cmd, env=e, input=stdin, capture_output=True, timeout=120, cwd=WORLD)
    finally:
        hypr.close()
    return Result(p, snapshot(WORLD))


FAILS = []
COUNTS = {}


def compare(name, label, build, py_argv, rs_argv, env=None, stdin=b"", sandbox=False, allow=None):
    env = env or {}
    py = run_once(build, py_argv, env, stdin, sandbox)
    rs = run_once(build, rs_argv, env, stdin, sandbox)
    COUNTS[name] = COUNTS.get(name, 0) + 1
    problems = []
    if b"Traceback" in py.err:
        if not (py.code == 1 and rs.code == 1 and b"unexpected error" in rs.err):
            problems.append(f"the script stopped (traceback) and the Rust one did not: rust {rs.code} {rs.err[-200:]!r}; python {py.err[-300:]!r}")
    else:
        if py.code != rs.code:
            problems.append(f"exit code: python {py.code}, rust {rs.code}")
        if py.err != rs.err:
            problems.append(f"stderr:\n    python {py.err[-400:]!r}\n    rust   {rs.err[-400:]!r}")
    if py.out != rs.out:
        problems.append(f"stdout:\n    python {py.out[-600:]!r}\n    rust   {rs.out[-600:]!r}")
    keys = sorted(set(py.files) | set(rs.files))
    for k in keys:
        a, b = py.files.get(k), rs.files.get(k)
        if isinstance(a, tuple) and a[0] == "picture" and isinstance(b, tuple) and b[0] == "picture":
            continue  # compared below while both exist: run_once removes the world, so the pictures were checked in snapshot order
        if a != b:
            problems.append(f"file {k}:\n    python {str(a)[:300]}\n    rust   {str(b)[:300]}")
    if allow and problems:
        problems = [p for p in problems if not allow(p, py, rs)]
    if problems:
        FAILS.append((name, label))
        print(f"DIFFERS {name}: {label}\n  " + "\n  ".join(problems))
    elif ARGS.v:
        print(f"same {name}: {label}")
    return py, rs


# ---------------------------------------------------------------- account

def account_world(passwd, group, protected="", sudoers=()):
    def build(w):
        write(os.path.join(w, "etc/passwd"), passwd)
        write(os.path.join(w, "etc/group"), group)
        write(os.path.join(w, "etc/protected"), protected)
        os.makedirs(os.path.join(w, "sudoers.d"))
        for n in sudoers:
            write(os.path.join(w, "sudoers.d", "60-yggdrasil-nopasswd-" + n), "x\n", 0o440)
        stubs(w)
        return {"YGGDRASIL_PASSWD": os.path.join(w, "etc/passwd"), "YGGDRASIL_GROUP": os.path.join(w, "etc/group"),
                "YGGDRASIL_PROTECTED_ACCOUNTS": os.path.join(w, "etc/protected"), "YGGDRASIL_SUDOERS_D": os.path.join(w, "sudoers.d")}
    return build


PASSWD_A = "root:x:0:0::/root:/bin/bash\nada:x:1000:1000:Ada:/home/ada:/bin/bash\nbob:x:1001:1001:Bob:/home/bob:/bin/bash\nkept:x:1002:1002:Kept:/home/kept:/bin/bash\n"
GROUP_A = "wheel:x:998:ada\nlibvirt:x:990:bob\nuucp:x:987:\n"
PASSWD_B = ("root:x:0:0::/root:/bin/bash\nrobin:x:1000:1000:Robin,,,:/home/robin:/bin/bash\nclaude-agent:x:1001:1001::/home/claude-agent:/bin/bash\n"
            "sam:x:1002:1002:Sam Doe:/home/sam:/bin/bash\nnobody:x:65534:65534::/:/usr/bin/nologin\n")
GROUP_B = "wheel:x:998:robin,claude-agent\nusers:x:985:\naudio:x:996:\nvideo:x:983:\nstorage:x:100:\nlp:x:7:\ndocker:x:970:sam\ngamemode:x:960:robin,sam\n"
PASSWD_ODD = ("ada:x:1000:1000:Ad\u00e9 \"A\",room:/home/ada:/usr/bin/nologin\r\nsup:x:\u00b2:1:x:/:/bin/sh\nshort:x:1500\n"
              "ara:x:\u0661\u0660\u0660\u0665:1:Ara:/home/ara:/bin/false\nbig:x:60000:1::/:/bin/sh\nmid:x:59999:1:Mid:/m:/bin/zsh:extra\n")


def account_cases(py, rs):
    worlds = {
        "A": account_world(PASSWD_A, GROUP_A, "kept\n"),
        "B": account_world(PASSWD_B, GROUP_B, "# the OS's own\nclaude-agent*\nsarina\n", sudoers=("robin",)),
        "odd": account_world(PASSWD_ODD.replace("sup:x:\u00b2:1:x:/:/bin/sh\n", ""), "wheel:x:1:ada,ara\nwireshark:x:2:ara\n\nuucp\n", "  # nothing\n\nmid # a comment\n"),
        "crash": account_world(PASSWD_ODD, "wheel:x:1:\n"),
        "badutf8": account_world("ada:x:1000:1000:\udcff:/h:/bin/sh\n".encode("utf-8", "surrogateescape").decode("latin-1"), GROUP_A),
        "nofiles": lambda w: (stubs(w), {"YGGDRASIL_PASSWD": os.path.join(w, "none"), "YGGDRASIL_GROUP": os.path.join(w, "none"),
                                         "YGGDRASIL_PROTECTED_ACCOUNTS": os.path.join(w, "none"), "YGGDRASIL_SUDOERS_D": os.path.join(w, "none")})[1],
    }
    names = ["alex", "alex\n", "Alex", "1abc", "a b", "a:b", "../x", "", "root", "sarina", "claude-agent2", "x" * 31, "a" + "b" * 30, "x" * 32,
             "-ralex", "--admin", "al\u0435x", "alex;id", "$(id)", "sam", "bob", "kept", "_x", "a-b_c9"]
    cases = []
    for w in worlds:
        cases += [(w, ["list"], b"", {}), (w, ["groups"], b"", {}), (w, [], b"", {}), (w, ["--help"], b"", {}), (w, ["-h"], b"", {}), (w, ["bogus"], b"", {})]
    for n in names:
        cases.append(("B", ["add", n], b"longenough1\n", {}))
    for args, stdin, env in [
        (["add", "alex", "--full-name", "Alex: The Great\nroot"], b"correct horse 9\n", {}),
        (["add", "alex", "--admin", "--full-name"], b"longenough1\n", {}),
        (["add", "alex", "--full-name", "--admin"], b"longenough1\n", {}),
        (["add", "alex", "--full-name", "x:0:0:root:/root:/bin/bash\nm::0:0::/:/bin/sh,\r\x01\x1f \u3000"], b"longenough1\n", {}),
        (["add", "alex", "--full-name", "\u00e9" * 70], b"longenough1\n", {}),
        (["add", "alex", "--full-name", " :,\n "], b"longenough1\n", {}),
        (["add", "alex"], b"short\n", {}),
        (["add", "alex"], b"", {}),
        (["add", "alex"], ("\u00e9" * 8 + "\n").encode(), {}),
        (["add", "alex"], ("\u00e9" * 7 + "\n").encode(), {}),
        (["add", "alex"], b"pa:ss word\r\nmore\n", {}),
        (["add", "alex"], b"longenough\xff\n", {}),
        (["add", "alex"], b"longenough1\n", {"FAIL_chpasswd": "chpasswd: bad"}),
        (["add", "alex"], b"longenough1\n", {"FAIL_useradd": "useradd: boom"}),
        (["add", "alex"], b"longenough1\n", {"GETENT": "alex:x:5:5::/:/bin/sh"}),
        (["admin", "sam", "on"], b"", {}),
        (["admin", "sam", "off"], b"", {}),
        (["admin", "sam", "off"], b"", {"FAIL_gpasswd": "gpasswd: user 'sam' is not a member of 'wheel'"}),
        (["admin", "sam", "on"], b"", {"FAIL_gpasswd": "gpasswd: boom"}),
        (["admin", "robin", "off"], b"", {}),
        (["admin", "robin", "off"], b"", {"PKEXEC_UID": "1002"}),
        (["admin", "claude-agent", "off"], b"", {}),
        (["admin", "nosuch", "on"], b"", {}),
        (["admin", "sam"], b"", {}),
        (["admin", "sam", "yes"], b"", {}),
        (["remove", "sam"], b"", {}),
        (["remove", "sam", "--delete-home"], b"", {}),
        (["remove", "sam"], b"", {"SESSIONS": "1 1002 sam seat0"}),
        (["remove", "sam"], b"", {"SESSIONS": "1 1002 samuel seat0\n2 5 x"}),
        (["remove", "sam"], b"", {"FAIL_userdel": "userdel: busy"}),
        (["remove", "robin"], b"", {}),
        (["remove", "robin"], b"", {"PKEXEC_UID": "", "SUDO_UID": "1000"}),
        (["remove", "robin"], b"", {"PKEXEC_UID": "1000\n"}),
        (["remove", "robin"], b"", {"PKEXEC_UID": "\u0661\u0660\u0660\u0660"}),
        (["remove", "robin"], b"", {"PKEXEC_UID": "\u00b9"}),
        (["remove", "claude-agent"], b"", {}),
        (["remove", "root"], b"", {}),
        (["remove", "nobody"], b"", {}),
        (["remove"], b"", {}),
        (["name", "sam", "Sam: New,Name\n"], b"", {}),
        (["name", "sam", ""], b"", {}),
        (["name", "claude-agent", "x"], b"", {}),
        (["name", "sam"], b"", {}),
        (["password", "sam"], b"a-long-password\n", {}),
        (["password", "sam"], b"short\n", {}),
        (["password", "robin"], b"a-long-password\n", {}),
        (["password", "sam"], b"a-long-password\n", {"FAIL_chpasswd": "nope"}),
        (["password", "sam", "x"], b"a-long-password\n", {}),
        (["login", "sam", "off"], b"", {}),
        (["login", "sam", "on"], b"", {}),
        (["login", "robin", "off"], b"", {}),
        (["login", "sam", "off"], b"", {"SESSIONS": "3 1002 sam"}),
        (["login", "nosuch", "off"], b"", {}),
        (["login", "sam", "maybe"], b"", {}),
        (["sudo", "robin", "nopassword"], b"", {}),
        (["sudo", "robin", "password"], b"", {}),
        (["sudo", "sam", "nopassword"], b"", {}),
        (["sudo", "sam", "password"], b"", {}),
        (["sudo", "robin", "nopassword"], b"", {"FAIL_visudo": "parse error"}),
        (["sudo", "robin", "everything"], b"", {}),
        (["group", "sam", "docker", "on"], b"", {}),
        (["group", "sam", "docker", "off"], b"", {"FAIL_gpasswd": "is not a member"}),
        (["group", "sam", "wheel", "on"], b"", {}),
        (["group", "sam", "libvirt", "on"], b"", {}),
        (["group", "sam", "gamemode", "off"], b"", {}),
        (["group", "claude-agent", "docker", "on"], b"", {}),
        (["group", "sam", "docker"], b"", {}),
    ]:
        cases.append(("B", args, stdin, env))
    for w in ("A", "odd", "crash", "nofiles"):
        for args in (["sudo", "ada", "nopassword"], ["login", "ada", "off"], ["group", "bob", "uucp", "on"], ["name", "mid", "X"], ["remove", "ara"], ["admin", "ara", "off"]):
            cases.append((w, args, b"", {}))
    for w, args, stdin, env in cases:
        for test_flag in (True, False):
            e = {"PKEXEC_UID": "1000"}
            e.update(env)
            if test_flag:
                e["YGGDRASIL_ACCOUNT_TEST"] = "1"
            label = f"[{w}{'' if test_flag else ', root'}] account {args} stdin={stdin!r} env={env}"
            allow = None
            if args[:1] == ["add"] and len(args) > 1 and args[1] == "alex\n":
                # the script's pattern ($ before a final newline) let it through to useradd; the Rust one refuses it
                allow = lambda p, py, rs: rs.code == 2 and b"the user name must be" in rs.err
            compare("account", label, worlds[w], [sys.executable, py] + args, [rs] + args, e, stdin, sandbox=not test_flag, allow=allow)


# ---------------------------------------------------------------- bootscreen

def png(path, w, h, mode="RGB"):
    from PIL import Image
    im = Image.new(mode, (w, h))
    for x in range(0, w, max(1, w // 37)):
        for y in range(0, h, max(1, h // 23)):
            im.putpixel((x, y), (x * 7 % 256, y * 5 % 256, (x + y) % 256) if mode == "RGB" else ((x * 7 % 256, y * 5 % 256, 9, 128) if mode == "RGBA" else x % 256))
    os.makedirs(os.path.dirname(path), exist_ok=True)
    fmt = {".png": "PNG", ".jpg": "JPEG", ".webp": "WEBP", ".gif": "GIF", ".bmp": "BMP"}[os.path.splitext(path)[1]]
    im.save(path, fmt)


def boot_world(grub=None, mkinitcpio="HOOKS=(base udev autodetect kms block filesystems)\n", cmdline="BOOT_IMAGE=/vmlinuz quiet\n",
               nvidia=False, dropin=False, branding=None, theme="spinner", pictures=(), themes=("spinner", "details", "labyrinth-rain"),
               kept=False):
    def build(w):
        stubs(w)
        if grub is not None:
            write(os.path.join(w, "etc/default/grub"), grub)
        if kept:
            write(os.path.join(w, "etc/default/grub.before-bootscreen"), "OLD\n")
        write(os.path.join(w, "mkinitcpio.conf"), mkinitcpio)
        write(os.path.join(w, "cmdline"), cmdline)
        os.makedirs(os.path.join(w, "sysmodule", "nvidia" if nvidia else "i915"))
        if dropin:
            write(os.path.join(w, "etc/mkinitcpio.conf.d/yggdrasil-plymouth.conf"), "# mine\n")
        for t in themes:
            os.makedirs(os.path.join(w, "themes", t), exist_ok=True)
        if branding is not None:
            write(os.path.join(w, "integrations/labyrinth-branding.conf"), branding)
        write(os.path.join(w, "theme"), theme)
        for p, *spec in pictures:
            if spec[0] == "raw":
                write(os.path.join(w, p), spec[1])
            else:
                png(os.path.join(w, p), *spec)
        return {"BOOTSCREEN_THEMES": os.path.join(w, "themes"), "BOOTSCREEN_INTEGRATIONS": os.path.join(w, "integrations"),
                "BOOTSCREEN_DROPIN": os.path.join(w, "etc/mkinitcpio.conf.d/yggdrasil-plymouth.conf"),
                "BOOTSCREEN_NVIDIA_DROPIN": os.path.join(w, "etc/mkinitcpio.conf.d/yggdrasil-plymouth-nvidia.conf"),
                "BOOTSCREEN_GRUB": os.path.join(w, "etc/default/grub")}
    return build


def bootscreen_cases(py, rs):
    G = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT='quiet loglevel=3'\nGRUB_CMDLINE_LINUX=\"\"\n"
    brand = "kind = branding\nboot_theme = labyrinth-rain\n"
    pics = [("pic/a.png", 64, 48), ("pic/wide.png", 5000, 1000), ("pic/tall.jpg", 900, 4500), ("pic/w.webp", 300, 200), ("pic/alpha.png", 40, 40, "RGBA"),
            ("pic/grey.png", 33, 17, "L"), ("pic/g.gif", 10, 10), ("pic/b.bmp", 10, 10), ("pic/eps.png", "raw", "%!PS-Adobe-3.0 EPSF-3.0\nshowpage\n"),
            ("pic/empty.png", "raw", ""), ("pic/trunc.png", "raw", b"\x89PNG\r\n\x1a\n\x00\x00")]
    worlds = {
        "fresh": boot_world(grub=G, branding=brand, pictures=pics),
        "fresh-nvidia": boot_world(grub=G, nvidia=True, branding=brand),
        "ready": boot_world(grub=G.replace("loglevel=3'", "loglevel=3 splash'"), dropin=True, theme="labyrinth-rain", branding=brand),
        "no-grub": boot_world(grub=None, cmdline="quiet splash\n", branding="kind=branding\nboot_theme=missing\n"),
        "hooks-in-conf": boot_world(grub="GRUB_CMDLINE_LINUX_DEFAULT=quiet\r\nX=1\r\n", mkinitcpio="MODULES=()\nHOOKS=(base plymouth)\n", theme="details"),
        "odd-grub": boot_world(grub="GRUB_CMDLINE_LINUX_DEFAULT='quiet\nGRUB_CMDLINE_LINUX_DEFAULT=\n#GRUB_CMDLINE_LINUX_DEFAULT=x\n", kept=True, theme="yggdrasil-picture"),
        "bad-branding": boot_world(grub=G, branding="kind = branding\xff\n".encode("latin-1")),
        "no-mkinitcpio-hooks": boot_world(grub=G, mkinitcpio="# nothing\n"),
    }
    cases = []
    for w in worlds:
        cases.append((w, ["get"], {}))
        cases += [(w, ["set", what], {}) for what in ("animation", "messages")]
    for p in pics:
        cases.append(("fresh", ["set", "picture", os.path.join(WORLD, p[0])], {}))
    cases += [
        ("fresh", ["set", "picture"], {}), ("fresh", ["set", "picture", os.path.join(WORLD, "pic/none.png")], {}), ("fresh", ["set", "picture", os.path.join(WORLD, "pic")], {}),
        ("fresh", ["set", "bogus"], {}), ("fresh", ["set"], {}), ("fresh", [], {}), ("fresh", ["get", "x"], {}), ("fresh", ["--help"], {}),
        ("fresh", ["set", "messages"], {"FAIL_grub_mkconfig": "grub-mkconfig: error " + "x" * 400}),
        ("fresh", ["set", "messages"], {"FAIL_plymouth_set_default_theme": "no such theme"}),
        ("fresh", ["set", "messages"], {"FAIL_mkinitcpio": "==> ERROR: " + "y" * 500}),
        ("ready", ["set", "messages"], {"FAIL_mkinitcpio": ""}),
        ("fresh", ["get"], {"FAIL_plymouth_set_default_theme": "x"}),
    ]
    for w, args, env in cases:
        # set needs root: inside the sandbox it has it; get is run both ways
        for sandbox in ((True,) if args[:1] != ["get"] else (True, False)):
            if not sandbox and w not in ("fresh",):
                continue
            label = f"[{w}{', root' if sandbox else ''}] bootscreen {args} env={env}"
            py_r, rs_r = None, None
            # the pictures: both kept aside to be compared
            res = []

            def build(world, _b=worlds[w]):
                return _b(world)
            pyres = run_once(build, [sys.executable, py] + args, env, b"", True)
            keep_py = os.path.join(SCRATCH, "py-picture.png")
            pp = os.path.join(WORLD, "themes/yggdrasil-picture/picture.png")
            if os.path.exists(pp):
                shutil.copy(pp, keep_py)
            rsres = run_once(build, [rs] + args, env, b"", True)
            COUNTS["bootscreen"] = COUNTS.get("bootscreen", 0) + 1
            problems = []
            if b"Traceback" in pyres.err:
                if not (pyres.code == 1 and rsres.code == 1 and b"unexpected error" in rsres.err):
                    problems.append(f"the script stopped (traceback) and the Rust one did not: rust {rsres.code} {rsres.err[-200:]!r}; python {pyres.err[-300:]!r}")
            elif (pyres.code, pyres.err) != (rsres.code, rsres.err):
                problems.append(f"exit/stderr: python {pyres.code} {pyres.err[-300:]!r}, rust {rsres.code} {rsres.err[-300:]!r}")
            if pyres.out != rsres.out:
                problems.append(f"stdout: python {pyres.out!r}, rust {rsres.out!r}")
            for k in sorted(set(pyres.files) | set(rsres.files)):
                a, b = pyres.files.get(k), rsres.files.get(k)
                if k.endswith("picture.png") and a and b:
                    why = picture_same(keep_py, pp)
                    if why:
                        problems.append(f"picture.png: {why}")
                elif a != b:
                    problems.append(f"file {k}: python {str(a)[:200]} / rust {str(b)[:200]}")
            if problems:
                FAILS.append(("bootscreen", label))
                print(f"DIFFERS bootscreen: {label}\n  " + "\n  ".join(problems))
            elif ARGS.v:
                print(f"same bootscreen: {label}")


# ---------------------------------------------------------------- game-fix

def steam_world(catalogue=None, ds2_file=None, fo4=True, started=True, extra_manifest=None, vdf=None):
    def build(w):
        stubs(w)
        home = os.path.join(w, "home")
        sa = os.path.join(home, ".local/share/Steam/steamapps")
        games = [("47780", "Dead Space 2", "Dead Space 2")] + ([("377160", "Fallout 4", "Fallout 4")] if fo4 else [])
        for appid, name, folder in games:
            write(os.path.join(sa, f"appmanifest_{appid}.acf"), f'"AppState"\n{{\n\t"appid"\t\t"{appid}"\n\t"name"\t\t"{name}"\n\t"installdir"\t\t"{folder}"\n}}\n')
            os.makedirs(os.path.join(sa, "common", folder), exist_ok=True)
            if started:
                os.makedirs(os.path.join(sa, "compatdata", appid, "pfx/drive_c/users/steamuser"))
        if fo4:
            fo = os.path.join(sa, "common/Fallout 4")
            write(os.path.join(fo, "Fallout4/Fallout4Prefs.ini"), "[Display]\r\niPresentInterval=1\r\nbBorderless=1\r\nbFull Screen=0\r\niSize H=720\r\niSize W=1280\r\n\r\n[Imagespace]\r\nbDoDepthOfField=1\r\n")
            write(os.path.join(fo, "Fallout4_Default.ini"), "[General]\nsLanguage=en\n")
        if ds2_file is not None:
            write(os.path.join(sa, "compatdata/47780/pfx/drive_c/users/steamuser/AppData/Local/EA Games/Dead Space 2/settings.txt"), ds2_file)
        if extra_manifest:
            write(os.path.join(sa, "appmanifest_999.acf"), extra_manifest)
        if vdf:
            write(os.path.join(sa, "libraryfolders.vdf"), vdf.replace("WORLD", w))
        cat = os.path.join(w, "catalogue.json")
        if catalogue is None:
            shutil.copy(CATALOGUE, cat)
        else:
            write(cat, catalogue)
        return {"YGGDRASIL_GAME_FIXES": cat}
    return build


CUSTOM = json.dumps({"games": [
    {"appid": "999", "name": "Odd", "title": "Odd fixes", "about": "a\u00e9\"", "files": [
        {"path": "{game}/conf/a.ini", "format": "ini", "set": {"Video": {"Width": "{width}", "Height": "{height}", "VSync": False, "Scale": 1.5},
                                                               "new section": {"k": None, "List": [1, "x"]}}},
        {"path": "{game}/conf/b.txt", "format": "kv", "template_lines": ["A = 1", "", "B=2", "", ""], "set": {"B": "{unknown}", "C": "{width}x{height}", "a": "x"}},
        {"path": "{prefix}/../odd/c.ini", "format": "ini", "newline": "crlf", "template": "{game}/missing.ini", "set": {"S": {"k": "v"}}},
    ]},
    {"appid": 47780, "name": "Number id", "title": "x", "files": []},
    {"appid": "47780", "name": "DS2 twin", "title": "an empty fix", "files": []},
]})


def gamefix_cases(py, rs):
    hypr = json.dumps([{"width": 1920, "height": 1080}, {"width": 3440, "height": 1440}, {"width": 2560, "height": 1600}])
    worlds = {
        "steam": steam_world(),
        "ds2-own": steam_world(ds2_file="Audio.Output = 2\r\nWindow.Width = 1024\r\n"),
        "ds2-bytes": steam_world(ds2_file=b"Name = caf\xe9\r\nWindow.Width = 1024\r\n\xff\xfe = x\nwindow.height=9"),
        "never-started": steam_world(started=False),
        "odd": steam_world(catalogue=CUSTOM, extra_manifest='"appid" "999"\n"name" "Odd"\n"installdir" "odd game"\n'),
        "odd-existing": steam_world(catalogue=CUSTOM, extra_manifest='"appid" "999"\n"installdir" "og"\n'),
        "bad-json": steam_world(catalogue="{not json"),
        "no-games": steam_world(catalogue='{"x": 1}'),
        "games-not-list": steam_world(catalogue='{"games": 5}'),
        "no-catalogue": lambda w: {"YGGDRASIL_GAME_FIXES": os.path.join(w, "none.json")},
        "libraries": steam_world(fo4=False, vdf='"libraryfolders"\n{\n "0" { "path" "WORLD/lib2" }\n "1" { "path" "WORLD/home/.local/share/Steam" }\n}\n'),
    }

    def odd_existing(w, _b=worlds["odd-existing"]):
        env = _b(w)
        write(os.path.join(w, "home/.local/share/Steam/steamapps/common/og/conf/a.ini"),
              "; top\n[video]\nwidth = 800\n\n[Other]\nx=1\n\n\n[Video]\nlate=1\n")
        write(os.path.join(w, "home/.local/share/Steam/steamapps/common/og/conf/b.txt"), "B = old\nc = lower\n\n\n")
        return env
    worlds["odd-existing"] = odd_existing
    screens = [{"YGGDRASIL_SCREEN": "3440x1440"}, {"YGGDRASIL_SCREEN": "", "HYPRCTL": hypr}, {"YGGDRASIL_SCREEN": "1x", "XRANDR": "DP-1 connected primary 2560x1440+0+0 x\nHDMI-1 connected 3840x2160+2560+0\n"},
               {}, {"YGGDRASIL_SCREEN": "\u0662\u0660x10"}, {"HYPRCTL": "[]", "XRANDR": "nothing"}, {"HYPRCTL": '[{"width": 5}]', "XRANDR": " connected 800x600+"}]
    seqs = [["list"], ["list", "--json"], ["apply", "47780"], ["undo", "47780"], ["apply", "377160"], ["apply", "999"], ["undo", "999"], ["apply", "nosuch"],
            ["apply"], ["bogus"], [], ["--help"], ["-h"], ["list", "x", "--json"]]
    def json_message(p, py_r, rs_r):
        # a catalogue that is not JSON: the same line up to the parser's own words (Python's json and serde_json say it differently)
        cut = lambda e: e.split(b".json: ")[0]
        return p.startswith("stderr") and py_r.code == rs_r.code == 1 and cut(py_r.err) == cut(rs_r.err) and b"cannot read" in rs_r.err
    for w in worlds:
        for env in (screens if w in ("steam", "odd") else screens[:1] + screens[3:4]):
            for args in seqs:
                compare("game-fix", f"[{w}] game-fix {args} env={env}", worlds[w], [sys.executable, py] + args, [rs] + args, env,
                        allow=json_message if w == "bad-json" else None)
    # sequences in one world: apply twice, then undo, then list (the files after each step)
    for w in ("steam", "ds2-own", "ds2-bytes", "odd", "odd-existing", "never-started"):
        for appid in ("47780", "377160", "999"):
            steps = [["apply", appid], ["apply", appid], ["list", "--json"], ["undo", appid], ["list"]]
            sh = " && ".join("%s %s ; echo \"[$?]\"" % ("{exe}", " ".join(s)) for s in steps)
            for who, exe in (("python", f"{sys.executable} {py}"), ("rust", rs)):
                pass
            compare("game-fix", f"[{w}] game-fix sequence {appid}", worlds[w], ["sh", "-c", sh.replace("{exe}", f"{sys.executable} {py}")],
                    ["sh", "-c", sh.replace("{exe}", rs)], {"YGGDRASIL_SCREEN": "3440x1440"})


# ---------------------------------------------------------------- gpu-run

VK = ("Devices:\n========\nGPU0:\n\tvendorID           = 0x10de\n\tdeviceID           = 0x1f07\n\tdeviceName         = NVIDIA GeForce RTX 2070\n"
      "GPU1:\n\tapiVersion = 1\n\tvendorID           = 0x8086\n\tdeviceID           = 0xe20b\n\tdeviceName         = Intel(R) Arc(tm) B580 Graphics (BMG G21)\n")
PROBE = """#!/bin/sh
# what the started program got
printf 'argv:'; for a in "$0" "$@"; do printf ' [%s]' "$a"; done; echo
env | grep -E '^(__NV|__GLX|__VK|DRI_PRIME|MESA_VK|DXVK|VKD3D|LSFGVK|DISABLE_LSFGVK|PATH=)' | sort
"""


def gpu_world(cards, lspci=None, vk=VK, extra=None):
    def build(w):
        stubs(w)
        write(os.path.join(w, "bin/probe"), PROBE, 0o755)
        write(os.path.join(w, "bin/gamescope"), PROBE, 0o755)
        write(os.path.join(w, "bin/notprog"), "echo hi\n", 0o755)
        write(os.path.join(w, "bin/noperm"), "#!/bin/sh\n", 0o644)
        write(os.path.join(w, "game dir/run game.sh"), PROBE, 0o755)
        root = os.path.join(w, "sys")
        drm = os.path.join(root, "class", "drm")
        os.makedirs(drm)
        for card, slot, vendor, device, driver, boot, outputs in cards:
            dev = os.path.join(root, "devices", "pci0000:00", slot)
            os.makedirs(dev, exist_ok=True)
            for k, v in (("vendor", vendor), ("device", device), ("boot_vga", "1" if boot else "0")):
                if v is not None:
                    write(os.path.join(dev, k), v + "\n")
            if driver:
                drv = os.path.join(root, "bus", "pci", "drivers", driver)
                os.makedirs(drv, exist_ok=True)
                if not os.path.lexists(os.path.join(dev, "driver")):
                    os.symlink(drv, os.path.join(dev, "driver"))
            os.makedirs(os.path.join(drm, card))
            os.symlink(dev, os.path.join(drm, card, "device"))
            for i, connected in enumerate(outputs):
                write(os.path.join(drm, f"{card}-DP-{i + 1}", "status"), "connected\n" if connected else "disconnected\n")
        if extra:
            extra(root)
        env = {"YGGDRASIL_SYSFS": root, "VULKANINFO": vk}
        for slot, text in (lspci or {}).items():
            env["LSPCI_" + slot.replace(":", "_").replace(".", "_")] = text
        return env
    return build


def gpurun_cases(py, rs):
    two = [("card1", "0000:01:00.0", "0x10de", "0x1f07", "nvidia", True, [True]), ("card2", "0000:03:00.0", "0x8086", "0xe20b", "xe", False, [False, False])]
    names = {"0000:01:00.0": '01:00.0 "VGA compatible controller" "NVIDIA Corporation" "TU106 [GeForce RTX 2070 Rev. A]" -ra1 "x"',
             "0000:03:00.0": '03:00.0 "VGA" "Intel" "Battlemage G21"'}
    worlds = {
        "two": gpu_world(two, names),
        "two-novk": gpu_world(two, {}, vk=""),
        "one": gpu_world(two[:1], names),
        "none": gpu_world([]),
        "three": gpu_world(two + [("card0", "0000:0a:00.0", "0x1002", "0x73bf", "amdgpu", False, [True, True]), ("card3", "0000:0b:00.0", "0x10de", "0x2204", None, False, [])],
                           {"0000:0a:00.0": '"a" "b" "Navi 21"'}),
        "odd": gpu_world([("card1", "0000:01:00.0", "10DE", "0x1f07", "nouveau", False, []), ("card2", "0000:02:00.0", "", "0x1", "x", False, []),
                          ("card4", "0000:04:00.0", "0x1002", None, "amdgpu", False, [True])], vk="vendorID = 0x1002\ndeviceID = 0x0\ndeviceName = Zero\n"),
        "bad-vendor": gpu_world([("card1", "0000:01:00.0", "nvidia", "0x1", "x", False, [])]),
        "bad-vk": gpu_world(two, names, vk="vendorID = 0xzz\ndeviceID = 1\ndeviceName = x\n"),
    }
    W = WORLD
    argsets = [["list"], ["list", "--json"], ["env", "1"], ["env", "2"], ["env", "3"], ["env"], ["env", "x"], ["env", " 2 "], ["env", "+2"], ["env", "\uff12"],
               ["env", "2", "extra"], [], ["--help"], ["-h"],
               ["1", "probe", "a b", "--x"], ["2", "probe", "%command%"], ["2"], ["9", "probe"], ["x", "probe"], ["2", f"{W}/game dir/run game.sh", "-w"],
               ["2", "no-such-program"], ["2", f"{W}/missing/prog"], ["2", "notprog"], ["2", "noperm"], ["2", "bin/probe"], ["2", ""],
               ["1", "--frames-on", "2", "probe", "-arg"], ["2", "--frames-on", "1", "--multiplier", "3", "probe"], ["1", "--frames-on", "2", "--multiplier", "50", "probe"],
               ["1", "--frames-on", "2", "--multiplier", "\u00b2", "probe"], ["1", "--frames-on", "2", "--multiplier", "\u0663", "probe"], ["1", "--frames-on", "1", "probe"],
               ["1", "--frames-on", "2"], ["1", "--frames-on", "9", "probe"], ["1", "--frames-on"], ["1", "--frames-on", "2", "--multiplier", "3"]]
    for w in worlds:
        for args in argsets:
            compare("gpu-run", f"[{w}] gpu-run {args}", worlds[w], [sys.executable, py] + args, [rs] + args)
    # without gamescope on the PATH, and with an empty PATH entry
    for env in ({"PATH": "/usr/bin:/bin"}, {"PATH": ""}, {"PATH": f"{W}/bin::/usr/bin"}):
        for args in (["2", "probe"], ["1", "--frames-on", "2", "probe"]):
            compare("gpu-run", f"[two] gpu-run {args} env={env}", worlds["two"], [sys.executable, py] + args, [rs] + args, env)


def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument("--python")
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(HERE, "..", "..", "..", "target", "release")))
    ap.add_argument("--only")
    ap.add_argument("-v", action="store_true")
    ARGS = ap.parse_args()
    try:
        for name, fn, exe in (("account", account_cases, "account"), ("bootscreen", bootscreen_cases, "bootscreen"), ("game-fix", gamefix_cases, "game-fix"),
                              ("gpu-run", gpurun_cases, "gpu-run")):
            if ARGS.only and ARGS.only != name:
                continue
            fn(py_helper(name), os.path.join(ARGS.bin, exe))
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)
    for name, n in COUNTS.items():
        bad = sum(1 for f in FAILS if f[0] == name)
        print(f"{name}: {n} cases, {'all the same' if not bad else str(bad) + ' differ'}")
    return 1 if FAILS else 0


if __name__ == "__main__":
    sys.exit(main())
