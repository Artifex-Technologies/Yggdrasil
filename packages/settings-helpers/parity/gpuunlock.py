#!/usr/bin/env python3
"""gpuunlock: the Rust helper and the Python script it replaces (with the shell grub.d fragment of each), run as programs in the
same made-up worlds, side by side. Each world is a whole file system of its own (gpuunlock-sandbox.sh: user, mount, network
and process namespaces from `unshare -rmnpf`, then pivot_root into the world): a made-up /sys with its graphics cards, /proc
with the command line, boot id and NVIDIA driver files, /etc/default/grub, /boot/grub/grub.cfg, the state folder, the
support table, the firmware; every program the helper runs is a stand-in on the PATH that writes down how it was called and
answers as the case says (modinfo, snapper, grub-mkconfig, systemctl, logger, nvidia-smi, journalctl, nvcc, the benchmark it
compiles), and Ollama's port has a stand-in when the case gives one. Nothing reaches the real /boot, /etc/default/grub, the
graphics card or the network.

For each case both run in the same world (made again from scratch, at the same path) with the same environment, and must
give the same exit code, standard output, standard error, calls to the stand-ins (with their arguments, in order), and files
afterwards (contents and modes), byte for byte, with the time of day as the one thing put aside. What differs on purpose:

  - where the script stopped with a traceback, the Rust helper stops with exit code 1 and one line on standard error;
  - the grub.d fragment: the old one checks entry.cfg with grep and cat; the new one calls `gpuunlock grub-entry`. Both are run
    as grub-mkconfig runs them (here, by the stand-in grub-mkconfig and directly) and must print the same.

Not in the cases, as they differ by design (the README says why): a JSON file with NaN or Infinity in it, output of a program
that is not UTF-8, an entry.cfg that is not UTF-8 or has a NUL in it, a redirect from Ollama's port.

    gpuunlock.py [--python FILE] [--fragment FILE] [--git REV] [--bin FILE] [--only TEXT] [-v]

--python, --fragment: the old helper and fragment (default: ../../settings/helpers/python/gpuunlock and 41_gpu_unlock, the
                      copies kept for a rollback); --git REV takes both from git instead (before the switch: ff5b6de)
--bin: the Rust gpuunlock (default: ../../../target/release/gpuunlock; cargo build --release)
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.normpath(os.path.join(HERE, "..", "..", ".."))
SETTINGS = os.path.join(REPO, "packages", "settings")
SANDBOX = os.path.join(HERE, "gpuunlock-sandbox.sh")
SCRATCH = tempfile.mkdtemp(prefix="gpuunlock-parity-")
WORLD = os.path.join(SCRATCH, "world")
ARGS = None
TIME = re.compile(rb"\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d[+-]\d{4}")

MSI = {"bdf": "0000:01:00.0", "device": "1e07", "sub": ("1462", "3711"), "vbios": "90.02.17.00.3e", "bar0": "0x00000000de000000"}
ASUS = {"bdf": "0000:02:00.0", "device": "1e07", "sub": ("1043", "866a"), "vbios": "90.02.17.00.47", "bar0": "0x00000000fa000000"}
OPEN = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release Build  (notroot@)  Fri Oct 23 2026\n"
CLOSED = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  615.78.08  Release Build  (notroot@)  Fri Oct 23 2026\n"
FIRST = ("menuentry 'Labyrinth Linux' --class labyrinth --class gnu-linux $menuentry_id_option 'gnulinux-simple-3118e509' {\n"
         "\tload_video\n\tinsmod btrfs\n\tsearch --no-floppy --fs-uuid --set=root 3118e509\n"
         "\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw rootflags=subvol=@ quiet splash\n"
         "\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n}\n")
GRUB_BASE = "### BEGIN /etc/grub.d/10_linux ###\n" + FIRST + "submenu 'Advanced options' $menuentry_id_option 'gnulinux-advanced' {\n}\n"
UNLOCKED_LOG = ("turing_unlock: entry: plm=0xffffff8f ss0=0x2300000 ss1=0x6 wpr2=0x0:0x1ffffe00 boot=0x0\n"
                "turing_unlock: TU102 FEAT UNLOCKED\npci 0000:01:00.0: turing_unlock: unlocked\n")
LOCKED = {"device": "NVIDIA GeForce RTX 2080 Ti", "pci": "0000:01:00.0", "sms": 68, "quick": False, "samples": 7,
          "check": {"compared": 4096, "wrong": 0, "max_abs_err": 0, "ok": True},
          "gemm_f16_acc_f32": [{"n": 4096, "tflops": 50.1}, {"n": 8192, "tflops": 52.8}], "mma_f16_acc_f32": 49.9, "mma_f16_acc_f16": 99.6}
UNLOCKED = dict(LOCKED, gemm_f16_acc_f32=[{"n": 4096, "tflops": 81.7}, {"n": 8192, "tflops": 77.9}], mma_f16_acc_f32=99.1, mma_f16_acc_f16=99.4)
PMON_HEAD = ("# gpu         pid   type     sm    mem    enc    dec    jpg    ofa    command \n"
             "# Idx           #    C/G      %      %      %      %      %      %    name \n")
PMON_IDLE = ("    0       1672     G      -      -      -      -      -      -    Hyprland       \n"
             "    0       2282   C+G      -      -      -      -      -      -    sunshine       \n"
             "    0       6195   C+G      2      1      -      -      -      -    renderD128 --cr\n")
GOOD_ENTRY = ("menuentry 'Labyrinth Linux, graphics card unlock (one try)' --class gnu-linux --class os --id yggdrasil-gpu-unlock {\n"
              "\tsearch --no-floppy --fs-uuid --set=root 3118e509\n"
              "\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet turing_unlock.targets=0000:01:00.0\n"
              "\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n}\n")
SB_ON = b"\x06\x00\x00\x00\x01"
SB_VAR = "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c"
STATE = "var/lib/yggdrasil/gpu-unlock"

# Every program gpuunlock runs: writes down its name and arguments, then answers from /case.
STUB = r"""#!/bin/sh
name=${0##*/}
{ printf '%s' "$name"; for a in "$@"; do printf ' [%s]' "$a"; done; echo; } >> /calls
c=/case
[ -f "$c/$name.sleep" ] && exec sleep "$(cat "$c/$name.sleep")"
code=0
[ -f "$c/$name.code" ] && code=$(cat "$c/$name.code")
case "$name" in
  modinfo) if [ -f "$c/modinfo.out" ]; then cat "$c/modinfo.out"; else echo /usr/lib/modules/7.3.0-4-cachyos/updates/dkms/turing_unlock.ko.zst; fi ;;
  snapper) if [ -f "$c/snapper.out" ]; then cat "$c/snapper.out"; else echo 240; fi ;;
  journalctl) cat "$c/kernel.log" 2>/dev/null ;;
  systemctl) [ -f "$c/systemctl.fail" ] && [ "$2" = "$(cat "$c/systemctl.fail")" ] && { echo "Failed to enable unit: $2 does not exist" >&2; exit 1; } ;;
  nvidia-smi)
    case " $* " in
      *" pmon "*) m=pmon ;;
      *--query-compute-apps*) m=compute ;;
      *power.limit*) m=power ;;
      *temperature.gpu*) m=thermal ;;
      *) m=other ;;
    esac
    [ -f "$c/nvidia-smi.$m.code" ] && code=$(cat "$c/nvidia-smi.$m.code")
    cat "$c/nvidia-smi.$m" 2>/dev/null ;;
  grub-mkconfig)
    [ "$code" = 0 ] || { echo "grub-mkconfig: error: the stand-in was told to fail" >&2; exit "$code"; }
    out=$2
    { cat "$c/mkconfig.extra" 2>/dev/null; cat "$c/grub.base"; /etc/grub.d/41_gpu_unlock; cat "$c/mkconfig.tail" 2>/dev/null; } > "$out.new"
    mv "$out.new" "$out" ;;
  nvcc)
    [ "$code" = 0 ] || { echo "nvcc fatal: the stand-in was told to fail" >&2; exit "$code"; }
    [ -f "$c/nvcc.nothing" ] && exit 0
    while [ $# -gt 0 ]; do [ "$1" = -o ] && o=$2; shift; done
    printf '#!/bin/sh\n{ printf bench; for a in "$@"; do printf " [%%s]" "$a"; done; echo; } >> /calls\ncat /case/bench.out 2>/dev/null\ncat /case/bench.err >&2 2>/dev/null\nexit $(cat /case/bench.code 2>/dev/null || echo 0)\n' > "$o"
    chmod 755 "$o" ;;
esac
exit "$code"
"""

OLLAMA = r"""import socket
s = socket.socket()
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", 11434))
s.listen(8)
open("/tmp/ollama-ready", "w").close()
answer = open("/case/ollama.answer", "rb").read()
while True:
    c, _ = s.accept()
    c.settimeout(2)
    try:
        c.recv(65536)
        with open("/calls", "a") as f:
            f.write("ollama [GET /api/ps]\n")
        c.sendall(answer)
    except OSError:
        pass
    c.close()
"""

TOOLS = ["modinfo", "snapper", "grub-mkconfig", "systemctl", "logger", "nvidia-smi", "journalctl"]


def write(path, data, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data.encode() if isinstance(data, str) else data)
    if mode is not None:
        os.chmod(path, mode)


def http(body, status="200 OK", extra=""):
    b = body.encode() if isinstance(body, str) else body
    return f"HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {len(b)}\r\n{extra}\r\n".encode() + b


class World:
    """What a case's world holds; build() writes it at WORLD/root for one side."""

    def __init__(self):
        self.files = {}      # path under / -> bytes or str (None: leave out)
        self.modes = {}
        self.links = {}      # path -> target
        self.dirs = []
        self.case = {}       # /case/NAME -> content (what the stand-ins answer)
        self.nvcc = "opt"    # where nvcc is: "opt" (/opt/cuda/bin), "path" (/standins), None
        self.tools = list(TOOLS)
        self.env = {}
        self.base()

    def base(self):
        f = self.files
        f["etc/default/grub"] = "GRUB_DEFAULT=0\nGRUB_TIMEOUT=2\nGRUB_TIMEOUT_STYLE=menu\n"
        f[".usr/lib/firmware/turing-unlock/610.43.03/v67-tu102-610.43.03.bin"] = "fw"
        f["etc/mkinitcpio.conf.d/zz-turing-unlock.conf"] = "MODULES+=(turing_unlock)\n"
        f["boot/grub/grub.cfg"] = GRUB_BASE
        with open(os.path.join(SETTINGS, "data", "gpu-unlock-supported.json"), "rb") as t:
            f[".usr/share/yggdrasil/gpu-unlock/supported.json"] = t.read()
        with open(os.path.join(SETTINGS, "helpers", "gpuunlock-bench.cu"), "rb") as t:
            f[".usr/lib/yggdrasil/gpuunlock-bench.cu"] = t.read()
        self.boot("aaaa-1111-2222-3333", "BOOT_IMAGE=/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet splash")
        self.card(MSI)
        self.state("baseline.json", json.dumps(dict(LOCKED, bdf=MSI["bdf"], vbios=MSI["vbios"].upper())))
        self.case["grub.base"] = GRUB_BASE
        self.case["nvidia-smi.pmon"] = PMON_HEAD + PMON_IDLE
        self.case["nvidia-smi.power"] = "260.00, 260.00\n"
        self.case["nvidia-smi.thermal"] = "88, Not Active\n"

    def boot(self, ident, cmdline):
        self.files[".proc/boot_id"] = ident + "\n"
        self.files[".proc/cmdline"] = cmdline + "\n"

    def card(self, c, driver=OPEN, loaded=True, bdf=None):
        bdf = bdf or c["bdf"]
        for k in [k for k in self.files if k.startswith("sys/bus/pci/devices/") or k.startswith(".proc/driver/")]:
            del self.files[k]
        self.add_card(c, driver, loaded, bdf)

    def add_card(self, c, driver=OPEN, loaded=True, bdf=None, vendor="0x10de", klass="0x030000"):
        bdf = bdf or c["bdf"]
        dev = f"sys/bus/pci/devices/{bdf}/"
        for k, v in {"vendor": vendor, "device": "0x" + c["device"], "subsystem_vendor": "0x" + c["sub"][0],
                     "subsystem_device": "0x" + c["sub"][1], "class": klass,
                     "resource": f"{c['bar0']} 0x00000000deffffff 0x0000000000040200"}.items():
            self.files[dev + k] = v + "\n"
        if loaded:
            self.files[".proc/driver/nvidia/version"] = driver
            self.files[f".proc/driver/nvidia/gpus/{bdf}/information"] = (
                f"Model: \t\t NVIDIA GeForce RTX 2080 Ti\nGPU UUID: \t GPU-0000\nVideo BIOS: \t {c['vbios']}\nBus Location: \t {bdf}\n")

    def no_card(self):
        for k in [k for k in self.files if k.startswith("sys/bus/pci/devices/")]:
            del self.files[k]

    def state(self, name, content):
        self.files[f"{STATE}/{name}"] = content

    def table(self, change):
        t = json.loads(self.files[".usr/share/yggdrasil/gpu-unlock/supported.json"])
        change(t)
        self.files[".usr/share/yggdrasil/gpu-unlock/supported.json"] = json.dumps(t)

    def armed(self):
        """What a successful arm leaves (the state an attempt starts from)."""
        self.state("armed", json.dumps({"time": "2026-11-13T09:00:00+0000", "entry": "yggdrasil-gpu-unlock", "boot_id": "aaaa-1111-2222-3333",
                                        "snapshot": "240", "bdf": MSI["bdf"]}))
        self.state("entry.cfg", GOOD_ENTRY)
        self.state("power-hold", json.dumps({"watts": None, "since": "2026-11-13T09:00:00+0000", "why": "graphics-card unlock armed"}))
        self.files["boot/grub/grub.cfg"] = GRUB_BASE + GOOD_ENTRY

    def build(self, side):
        root = os.path.join(WORLD, "root")
        shutil.rmtree(WORLD, ignore_errors=True)
        for d in ["usr", "proc", "dev", "tmp", ".old", "etc/grub.d", "var/cache", "standins", "case", "root", "home/u", ".proc/driver",
                  ".usr/share/yggdrasil", ".usr/lib/yggdrasil", ".usr/lib/firmware", "opt", "sys"] + self.dirs:
            os.makedirs(os.path.join(root, d), exist_ok=True)
        for name, target in (("bin", "usr/bin"), ("lib", "usr/lib"), ("lib64", "usr/lib"), ("sbin", "usr/bin")):
            os.symlink(target, os.path.join(root, name))
        for k, v in self.files.items():
            if v is not None:
                write(os.path.join(root, k), v, self.modes.get(k))
        for k, t in self.links.items():
            os.makedirs(os.path.dirname(os.path.join(root, k)), exist_ok=True)
            os.symlink(t, os.path.join(root, k))
        for k, v in self.case.items():
            write(os.path.join(root, "case", k), v)
        for t in self.tools:
            write(os.path.join(root, "standins", t), STUB, 0o755)
        if self.nvcc == "opt":
            write(os.path.join(root, "opt/cuda/bin/nvcc"), STUB, 0o755)
        elif self.nvcc == "path":
            write(os.path.join(root, "standins/nvcc"), STUB, 0o755)
        write(os.path.join(root, "standins/ollama-server.py"), OLLAMA)
        write(os.path.join(root, "calls"), "")
        prog, frag = side
        write(os.path.join(root, ".usr/lib/yggdrasil/gpuunlock"), prog, 0o755)
        write(os.path.join(root, "etc/grub.d/41_gpu_unlock"), frag, 0o755)
        return root


SKIP = {"standins", "case", "calls", ".old", "usr", "proc", "dev", "tmp", "bin", "lib", "lib64", "sbin", ".usr/lib/yggdrasil/gpuunlock",
        "etc/grub.d/41_gpu_unlock"}


def snapshot(root):
    out = {}
    for d, dirs, files in os.walk(root):
        rel = os.path.relpath(d, root)
        if rel != "." and (rel.split("/")[0] in SKIP):
            dirs[:] = []
            continue
        dirs.sort()
        for n in sorted(files) + [x for x in dirs if os.path.islink(os.path.join(d, x))]:
            p = os.path.join(d, n)
            r = os.path.normpath(os.path.join(rel, n))
            if r in SKIP or r.split("/")[0] in SKIP:
                continue
            if os.path.islink(p):
                out[r] = ("link", os.readlink(p))
            else:
                with open(p, "rb") as fh:
                    out[r] = (oct(os.stat(p).st_mode & 0o777), TIME.sub(b"<time>", fh.read()))
        for n in dirs:
            if not os.path.islink(os.path.join(d, n)):
                out[os.path.normpath(os.path.join(rel, n)) + "/"] = oct(os.stat(os.path.join(d, n)).st_mode & 0o777)
    with open(os.path.join(root, "calls"), "rb") as fh:
        calls = fh.read()
    return out, calls


class Result:
    def __init__(self, p, files, calls):
        self.code, self.out, self.err = p.returncode, TIME.sub(b"<time>", p.stdout), TIME.sub(b"<time>", p.stderr)
        self.files, self.calls = files, calls


def run_once(world, side, argv, user):
    root = world.build(side)
    env = {"PATH": "/standins:/usr/bin", "HOME": "/root" if user == "root" else "/home/u", "LANG": "C.UTF-8", "TZ": "UTC0"}
    env.update(world.env)
    cmd = ["unshare", "-rmnpf", "sh", SANDBOX, root, user, "/usr/bin/env", "-i"] + [f"{k}={v}" for k, v in env.items()] + argv
    p = subprocess.run(cmd, capture_output=True, timeout=120, env={"PATH": "/usr/bin"})
    files, calls = snapshot(root)
    return Result(p, files, calls)


FAILS = []
COUNT = 0


def compare(label, world, argv, user="root"):
    global COUNT
    if ARGS.only and ARGS.only not in label:
        return
    COUNT += 1
    py = run_once(world, SIDES["python"], argv, user)
    rs = run_once(world, SIDES["rust"], argv, user)
    problems = []
    if b"Traceback" in py.err:
        if not (py.code == 1 and rs.code == 1 and b"unexpected error" in rs.err):
            problems.append(f"the script stopped (traceback) and the Rust one did not: rust {rs.code} {rs.err[-300:]!r}; python {py.err[-400:]!r}")
    else:
        if py.code != rs.code:
            problems.append(f"exit code: python {py.code}, rust {rs.code}")
        if py.err != rs.err:
            problems.append(f"stderr:\n    python {py.err[-500:]!r}\n    rust   {rs.err[-500:]!r}")
    if py.out != rs.out:
        problems.append(f"stdout:\n    python {py.out[-700:]!r}\n    rust   {rs.out[-700:]!r}")
    if py.calls != rs.calls:
        problems.append(f"calls:\n    python {py.calls[-700:]!r}\n    rust   {rs.calls[-700:]!r}")
    for k in sorted(set(py.files) | set(rs.files)):
        a, b = py.files.get(k), rs.files.get(k)
        if a != b:
            problems.append(f"file {k}:\n    python {str(a)[:400]}\n    rust   {str(b)[:400]}")
    if problems:
        FAILS.append(label)
        print(f"DIFFERS: {label}\n  " + "\n  ".join(problems))
    elif ARGS.v:
        print(f"same: {label} (exit {py.code})")
    return py, rs


def W(change=None):
    w = World()
    if change:
        change(w)
    return w


def cases():
    status = lambda label, w, user="root": (compare(label + " / status --json", w, ["/usr/lib/yggdrasil/gpuunlock", "status", "--json"], user),
                                            compare(label + " / status", w, ["/usr/lib/yggdrasil/gpuunlock", "status"], user))
    run = lambda label, w, *a, user="root": compare(label, w, ["/usr/lib/yggdrasil/gpuunlock", *a], user)
    G = "/usr/lib/yggdrasil/gpuunlock"

    # ---- arguments
    for a in [[], ["-h"], ["--help"], ["bogus"], ["arm", "now"], ["status", "--json", "extra"], ["status", "x"], ["disarm", "x"],
              ["bench", "--fast"], ["bench", "--quick", "--baseline"], ["bench", "--quick", "--quick"], ["verify", ""], ["", "status"]]:
        run(f"arguments {a}", W(), *a)

    # ---- the card and the table
    status("supported", W())
    status("not root", W(), user="user")
    status("another board", W(lambda w: w.card(ASUS)))
    status("the other driver series", W(lambda w: w.card(MSI, driver=CLOSED)))
    status("driver not loaded", W(lambda w: w.card(MSI, loaded=False)))
    status("no card", W(lambda w: w.no_card()))
    status("no PCI folder", W(lambda w: (w.no_card(), w.files.update({"sys/x": "y"}))))
    status("an AMD card only", W(lambda w: (w.no_card(), w.add_card(dict(ASUS, device="73bf"), vendor="0x1002", bdf="0000:03:00.0", loaded=False))))
    status("AMD first, NVIDIA second", W(lambda w: w.add_card(dict(ASUS, device="73bf"), vendor="0x1002", bdf="0000:00:02.0", loaded=False)))
    status("the card's sound function first", W(lambda w: w.add_card(dict(MSI, device="10f7"), klass="0x040300", bdf="0000:01:00.1", loaded=False)))
    status("two NVIDIA cards", W(lambda w: w.add_card(ASUS)))
    status("a 3D controller, upper case", W(lambda w: w.card(dict(MSI, device="1E07"), driver=OPEN) or w.files.update({"sys/bus/pci/devices/0000:01:00.0/class": "0x030200\n", "sys/bus/pci/devices/0000:01:00.0/vendor": " 0X10DE \n"})))
    for res in ["", "zz", "-0x10 0x1", "0x_de00_0000", "0xde000000\n0x1", "0x" + "f" * 20, "\u0661\u0662"]:
        status(f"resource {res!r}", W(lambda w, res=res: w.files.update({"sys/bus/pci/devices/0000:01:00.0/resource": res})))
    for info in ["", "Video BIOS: \t \n", "Model:\nVideo BIOS: 90.02.17.00.3e\n", "Video BIOS:90.02.17.00.3e\r\nModel: X\r\n", "Model: A\u2028Video BIOS: 90.02.17.00.3e\n",
                 "Video BIOS: \u00df90.02.17.00.3e\n"]:
        status(f"information {info!r}", W(lambda w, info=info: w.files.update({".proc/driver/nvidia/gpus/0000:01:00.0/information": info})))
    for ver in ["", "\n", "NVRM version: NVIDIA UNIX Open Kernel Module  615.78  x\n", "NVRM version: 615.78.08.1 x\n", "NVRM version:\x1c615.78.08\x1cx\n",
                "NVRM version: \u0666\u0661\u0665.\u0667\u0668 x\n", "NVRM version:  615.78.08", "Open Kernel Module 1.2.3 \n"]:
        status(f"driver version {ver!r}", W(lambda w, ver=ver: w.files.update({".proc/driver/nvidia/version": ver})))
    status("version file a folder", W(lambda w: w.files.pop(".proc/driver/nvidia/version") and w.dirs.append(".proc/driver/nvidia/version")))
    status("not UTF-8 in sysfs", W(lambda w: w.files.update({".proc/driver/nvidia/gpus/0000:01:00.0/information": b"Model: Ti\xff\xfe\nVideo BIOS: 90.02.17.00.3e\n"})))
    status("Secure Boot on", W(lambda w: w.files.update({SB_VAR: SB_ON})))
    status("Secure Boot off", W(lambda w: w.files.update({SB_VAR: b"\x06\x00\x00\x00\x00"})))
    status("Secure Boot variable short", W(lambda w: w.files.update({SB_VAR: b"\x01\x01\x01\x01"})))
    for field, bad in [("subsystem", "1462:*"), ("vbios", "90.02.17.00.*"), ("device", "10de:1e0?"), ("subsystem", ""), ("driver_series", "any"),
                       ("evidence", ""), ("evidence", "http://x"), ("method", "turing-unlock-efi"), ("method", None), ("subsystem", "1462:3711\n"),
                       ("vbios", ["90.02.17.00.3E"]), ("device", 1234), ("status", "measured-no-benefit"), ("status", None), ("vbios", "90.02.17.00.3e")]:
        status(f"table {field}={bad!r}", W(lambda w, field=field, bad=bad: w.table(lambda t: t["entries"][0].__setitem__(field, bad))))
    status("table entry without status", W(lambda w: w.table(lambda t: t["entries"][0].pop("status"))))
    status("table entry without evidence", W(lambda w: w.table(lambda t: t["entries"][0].pop("evidence"))))
    status("table two entries, the second for this card", W(lambda w: w.table(lambda t: t["entries"].insert(0, dict(t["entries"][0], subsystem="1043:866a", vbios="90.02.17.00.47")))))
    status("table same card, other series too", W(lambda w: w.table(lambda t: t["entries"].append(dict(t["entries"][0], driver_series="proprietary")))))
    status("table other series only", W(lambda w: w.table(lambda t: t["entries"][0].__setitem__("driver_series", "proprietary"))))
    status("ASUS with the table listing ASUS as not experimental", W(lambda w: (w.card(ASUS), w.table(lambda t: t["entries"].append(dict(t["entries"][0], subsystem="1043:866a", vbios="90.02.17.00.47", status="retired"))))))
    status("Another model only", W(lambda w: w.card(dict(MSI, device="2204"))))
    for name, content in [("not JSON", "{"), ("a list", "[]"), ("no entries", "{}"), ("entries not a list", '{"entries": {}}'),
                          ("an entry not an object", '{"entries": [1]}'), ("empty entries", '{"entries": []}'), ("not UTF-8", b'{"entries": [\xff]}'),
                          ("a BOM", "\ufeff{}"), ("missing", None)]:
        status(f"table {name}", W(lambda w, content=content: w.files.update({".usr/share/yggdrasil/gpu-unlock/supported.json": content})))
    status("table a folder", W(lambda w: (w.files.pop(".usr/share/yggdrasil/gpu-unlock/supported.json"), w.dirs.append(".usr/share/yggdrasil/gpu-unlock/supported.json"))))

    # ---- the state the tile shows
    for name, st in [("failed with why", {"state": "failed", "why": "x"}), ("failed without why", {"state": "failed"}),
                     ("failed with a list as why", {"state": "failed", "why": ["a", 1]}), ("failed, last attempt", {"state": "failed", "why": "w", "last_attempt": {"time": "t", "result": "failed", "x": 1.5}}),
                     ("unlocked this start", {"state": "unlocked", "boot_id": "aaaa-1111-2222-3333", "since": "s", "ratio": 1.9859719438877756, "why": "Tensor rate 2.0x"}),
                     ("unlocked this start, no why", {"state": "unlocked", "boot_id": "aaaa-1111-2222-3333", "ratio": 2}),
                     ("unlocked an earlier start", {"state": "unlocked", "boot_id": "old", "since": "s", "ratio": 2.0}),
                     ("throttled", {"state": "throttled", "boot_id": "aaaa-1111-2222-3333", "why": "hot"}), ("idle", {"state": "idle"}),
                     ("state a number", {"state": 3}), ("unicode", {"state": "failed", "why": "caf\u00e9 \u2603 \U0001f600"})]:
        status(f"state.json {name}", W(lambda w, st=st: w.state("state.json", json.dumps(st))))
    for name, content in [("not JSON", "{"), ("a list", "[1]"), ("not UTF-8", b'{"state": "failed", "why": "\xff"}')]:
        status(f"state.json {name}", W(lambda w, content=content: w.state("state.json", content)))
    status("armed", W(lambda w: w.armed()))
    status("trying", W(lambda w: w.state("trying", json.dumps({"boot_id": "aaaa-1111-2222-3333"}))))
    status("power hold", W(lambda w: w.state("power-hold", "{}")))
    status("power hold a dangling link", W(lambda w: w.links.update({f"{STATE}/power-hold": "/nowhere"})))
    status("armed and Secure Boot", W(lambda w: (w.armed(), w.files.update({SB_VAR: SB_ON}))))
    status("failed and armed", W(lambda w: (w.armed(), w.state("state.json", '{"state": "failed", "why": "x"}'))))

    # ---- what blocks arming (status shows it, arm refuses)
    blockers = [
        ("modinfo fails", lambda w: w.case.update({"modinfo.code": "1", "modinfo.out": "modinfo: ERROR: Module turing_unlock not found."})),
        ("modinfo says nothing", lambda w: w.case.update({"modinfo.out": "  \n"})),
        ("modinfo missing", lambda w: w.tools.remove("modinfo")),
        ("firmware missing", lambda w: w.files.pop(".usr/lib/firmware/turing-unlock/610.43.03/v67-tu102-610.43.03.bin")),
        ("firmware a file", lambda w: (w.files.pop(".usr/lib/firmware/turing-unlock/610.43.03/v67-tu102-610.43.03.bin"), w.files.update({".usr/lib/firmware/turing-unlock/610.43.03": "x"}))),
        ("start-up hook missing", lambda w: w.files.pop("etc/mkinitcpio.conf.d/zz-turing-unlock.conf")),
        ("start-up hook a folder", lambda w: (w.files.pop("etc/mkinitcpio.conf.d/zz-turing-unlock.conf"), w.dirs.append("etc/mkinitcpio.conf.d/zz-turing-unlock.conf"))),
        ("no baseline", lambda w: w.state("baseline.json", None)),
        ("empty baseline", lambda w: w.state("baseline.json", "{}")),
        ("baseline quick", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, quick=True, bdf=MSI["bdf"], vbios="90.02.17.00.3E")))),
        ("baseline wrong results", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, check={"ok": False}, bdf=MSI["bdf"], vbios="90.02.17.00.3E")))),
        ("baseline without check", lambda w: w.state("baseline.json", json.dumps({k: v for k, v in dict(LOCKED, bdf=MSI["bdf"], vbios="90.02.17.00.3E").items() if k != "check"}))),
        ("baseline check a list", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, check=[1], bdf=MSI["bdf"], vbios="90.02.17.00.3E")))),
        ("baseline check null", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, check=None, bdf=MSI["bdf"], vbios="90.02.17.00.3E")))),
        ("baseline rate 0", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, mma_f16_acc_f32=0, bdf=MSI["bdf"], vbios="90.02.17.00.3E")))),
        ("baseline of another card", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, bdf="0000:02:00.0", vbios="90.02.17.00.3E")))),
        ("baseline of another firmware", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, bdf=MSI["bdf"], vbios="90.02.17.00.47")))),
        ("baseline without an address", lambda w: w.state("baseline.json", json.dumps(dict(LOCKED, vbios="90.02.17.00.3E")))),
        ("baseline not JSON", lambda w: w.state("baseline.json", "nope")),
    ]
    for grub in ["GRUB_TIMEOUT=2\nGRUB_TIMEOUT_STYLE=hidden\n", "GRUB_TIMEOUT=0\n", "GRUB_TIMEOUT='0'\n", 'GRUB_TIMEOUT="0"\n', "GRUB_TIMEOUT=\u0660\n",
                 "GRUB_TIMEOUT=\u00b2\n", "GRUB_TIMEOUT=00\n", "GRUB_TIMEOUT= 0 \n", "#GRUB_TIMEOUT=0\n", "GRUB_TIMEOUT=-1\nGRUB_TIMEOUT=0\n",
                 "GRUB_TIMEOUT_STYLE=countdown\nGRUB_TIMEOUT=5\n", "GRUB_TIMEOUT=0\r\n", "", None, "GRUB_TIMEOUT='hidden\n", "GRUB_TIMEOUT_STYLE='hidden'\n"]:
        blockers.append((f"/etc/default/grub {grub!r}", lambda w, grub=grub: w.files.update({"etc/default/grub": grub})))
    for name, change in blockers:
        status(f"blocker: {name}", W(change))
        run(f"blocker: {name} / arm", W(change), "arm")
    status("modinfo takes too long", W(lambda w: w.case.update({"modinfo.sleep": "12"})))

    # ---- arming
    run("arm", W(), "arm")
    run("arm, not root", W(), "arm", user="user")
    run("arm, Secure Boot on", W(lambda w: w.files.update({SB_VAR: SB_ON})), "arm")
    run("arm, another board", W(lambda w: w.card(ASUS)), "arm")
    run("arm, no card", W(lambda w: w.no_card()), "arm")
    run("arm, already armed", W(lambda w: w.armed()), "arm")
    run("arm, failed before", W(lambda w: w.state("state.json", '{"state": "failed", "why": "x"}')), "arm")
    run("arm, a program computing", W(lambda w: w.case.update({"nvidia-smi.pmon": PMON_HEAD + PMON_IDLE + "    0       4242     C     97     40      -      -      -      -    ollama         \n"})), "arm")
    run("arm, a game over three samples", W(lambda w: w.case.update({"nvidia-smi.pmon": PMON_HEAD + ("    0       7000     G     85     30      -      -      -      -    GameThread     \n" + PMON_IDLE) * 2 + "    0       7000     G     75     30      -      -      -      -    GameThread     \n"})), "arm")
    run("arm, a short redraw", W(lambda w: w.case.update({"nvidia-smi.pmon": PMON_HEAD + "    0       6195   C+G     20      5      -      -      -      -    renderD128 --cr\n" + PMON_IDLE * 2})), "arm")
    run("arm, pmon with a superscript", W(lambda w: w.case.update({"nvidia-smi.pmon": PMON_HEAD + "    0       7000     G     \u00b2     30      -      -      -      -    x\n"})), "arm")
    run("arm, pmon without a header", W(lambda w: w.case.update({"nvidia-smi.pmon": "    0  1  C  99  -  -  -  -  -  x\n"})), "arm")
    run("arm, no pmon: the compute list", W(lambda w: w.case.update({"nvidia-smi.pmon.code": "6", "nvidia-smi.compute": "4242, /usr/bin/ollama\n\n77, game.exe\n"})), "arm")
    run("arm, no pmon, nothing computing", W(lambda w: w.case.update({"nvidia-smi.pmon.code": "6", "nvidia-smi.compute": ""})), "arm")
    run("arm, no nvidia-smi", W(lambda w: w.tools.remove("nvidia-smi")), "arm")
    for name, answer in [("a model loaded", http('{"models": [{"name": "qwen3:8b", "size": 5}, {"model": "x"}]}')),
                         ("no model", http('{"models": []}')), ("models null", http('{"models": null}')), ("no models key", http("{}")),
                         ("a 404", http("not found", "404 Not Found")), ("a 500", http("{}", "500 Internal Server Error")),
                         ("not JSON", http("<html>")), ("not HTTP", b"hello, this is not a web server\r\n\r\n"), ("nothing", b""),
                         ("chunked", b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n6\r\n{\"mode\r\n15\r\nls\": [{\"name\": \"a\"}]}\r\n0\r\n\r\n"),
                         ("HTTP/1.0 to the end", b"HTTP/1.0 200 OK\r\n\r\n{\"models\": [{\"name\": \"m\"}]}"),
                         ("cut short", b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\n{\"models\": [{\"name\": \"cut\"}]}"),
                         ("a list", http("[1]")), ("a model not an object", http('{"models": ["x"]}')), ("a name not text", http('{"models": [{"name": 5}]}')),
                         ("models an object", http('{"models": {"a": 1}}')), ("unicode name", http('{"models": [{"name": "caf\\u00e9 \u2603"}]}')),
                         ("a BOM", http(b"\xef\xbb\xbf" + b'{"models": [{"name": "b"}]}'))]:
        run(f"arm, Ollama's port: {name}", W(lambda w, answer=answer: w.case.update({"ollama.answer": answer})), "arm")
    run("arm, no grub.cfg", W(lambda w: w.files.update({"boot/grub/grub.cfg": None})), "arm")
    run("arm, grub.cfg starts with a submenu", W(lambda w: w.files.update({"boot/grub/grub.cfg": "submenu 'x' {\n}\n" + GRUB_BASE})), "arm")
    run("arm, first entry already has the option", W(lambda w: w.files.update({"boot/grub/grub.cfg": GRUB_BASE.replace("quiet splash", "quiet splash turing_unlock.targets=all")})), "arm")
    run("arm, first entry has two kernel lines", W(lambda w: w.files.update({"boot/grub/grub.cfg": GRUB_BASE.replace("\tinitrd", "\tlinux /x\n\tinitrd")})), "arm")
    run("arm, first entry never closes", W(lambda w: w.files.update({"boot/grub/grub.cfg": FIRST.replace("}\n", "")})), "arm")
    run("arm, grub.cfg not UTF-8", W(lambda w: w.files.update({"boot/grub/grub.cfg": GRUB_BASE.encode() + b"menuentry 'Syst\xe8me' {\n}\n"})), "arm")
    run("arm, grub.cfg with CRLF", W(lambda w: w.files.update({"boot/grub/grub.cfg": GRUB_BASE.replace("\n", "\r\n")})), "arm")
    run("arm, grub.cfg with Unicode line ends and spaces", W(lambda w: w.files.update({"boot/grub/grub.cfg": GRUB_BASE.replace("\tload_video\n", "\tload_video\u2028\u3000x\n").replace("\tlinux\t", "\u2003linux\t")})), "arm")
    run("arm, the snapshot fails", W(lambda w: w.case.update({"snapper.code": "1", "snapper.out": "Creating snapshot failed: " + "x" * 300})), "arm")
    run("arm, no snapper", W(lambda w: w.tools.remove("snapper")), "arm")
    run("arm, snapper with spaces", W(lambda w: w.case.update({"snapper.out": "  241\n\n"})), "arm")
    run("arm, grub-mkconfig fails", W(lambda w: w.case.update({"grub-mkconfig.code": "1"})), "arm")
    run("arm, the first entry changes", W(lambda w: w.case.update({"mkconfig.extra": "menuentry 'Something else' $menuentry_id_option 'other' {\n}\n"})), "arm")
    run("arm, another system's entry in latin-1 after grub-mkconfig", W(lambda w: w.case.update({"mkconfig.tail": b"menuentry 'Syst\xe8me' {\n}\n"})), "arm")
    run("arm, a unit cannot be enabled", W(lambda w: w.case.update({"systemctl.fail": "yggdrasil-gpu-unlock-verify.service"})), "arm")
    run("arm, power-hold.tmp a folder", W(lambda w: w.dirs.append(f"{STATE}/power-hold.tmp/x")), "arm")
    run("arm, armed.tmp a folder", W(lambda w: w.dirs.append(f"{STATE}/armed.tmp/x")), "arm")
    run("arm, entry.cfg a folder", W(lambda w: w.dirs.append(f"{STATE}/entry.cfg/x")), "arm")
    run("arm, links in the state folder", W(lambda w: (w.files.update({"victim": "mine\n"}), w.links.update({f"{STATE}/{n}": "/victim" for n in ("power-hold.tmp", "armed.tmp", "entry.cfg", "entry.cfg.tmp", "log")}))), "arm")
    run("arm, a stale temporary file", W(lambda w: w.state("armed.tmp", "old")), "arm")
    run("arm, log a folder", W(lambda w: w.dirs.append(f"{STATE}/log")), "arm")
    run("arm, no logger", W(lambda w: w.tools.remove("logger")), "arm")
    run("arm, a card whose address has a newline", W(lambda w: w.card(MSI, bdf="0000:01:00.0\n")), "arm")

    # ---- disarm, reenable, release
    run("disarm, nothing armed", W(), "disarm")
    run("disarm", W(lambda w: w.armed()), "disarm")
    run("disarm, grub-mkconfig fails", W(lambda w: (w.armed(), w.case.update({"grub-mkconfig.code": "3"}))), "disarm")
    run("disarm, no entry.cfg", W(lambda w: (w.armed(), w.state("entry.cfg", None))), "disarm")
    run("disarm, not root", W(lambda w: w.armed()), "disarm", user="user")
    run("disarm, armed a folder", W(lambda w: w.dirs.append(f"{STATE}/armed")), "disarm")
    run("reenable, not failed", W(), "reenable")
    run("reenable", W(lambda w: w.state("state.json", json.dumps({"state": "failed", "why": "x", "last_attempt": {"time": "t"}, "zeta": 1, "alpha": [1, 2.5]}))), "reenable")
    run("reenable, state.json a list", W(lambda w: w.state("state.json", "[]")), "reenable")
    run("reenable, not root", W(lambda w: w.state("state.json", '{"state": "failed"}')), "reenable", user="user")
    run("release, armed", W(lambda w: w.armed()), "release")
    run("release, trying", W(lambda w: (w.state("trying", "{}"), w.state("power-hold", "{}"))), "release")
    run("release, no hold", W(), "release")
    run("release", W(lambda w: w.state("power-hold", '{"watts": 200}')), "release")
    run("release, not root", W(lambda w: w.state("power-hold", "{}")), "release", user="user")

    # ---- the start after arming (boot), and the desktop after it (verify)
    picked = "BOOT_IMAGE=/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet splash turing_unlock.targets=0000:01:00.0"
    run("boot, nothing pending", W(), "boot")
    run("boot, armed, a normal start", W(lambda w: (w.armed(), w.boot("bbbb", "root=x quiet"))), "boot")
    run("boot, armed, the entry picked", W(lambda w: (w.armed(), w.boot("bbbb", picked))), "boot")
    run("boot, the option without arming", W(lambda w: w.boot("bbbb", picked)), "boot")
    run("boot, armed {}", W(lambda w: (w.armed(), w.state("armed", "{}"), w.boot("bbbb", picked))), "boot")
    run("boot, armed not JSON", W(lambda w: (w.armed(), w.state("armed", "x"), w.boot("bbbb", picked))), "boot")
    run("boot, an attempt that never reached the desktop", W(lambda w: (w.state("trying", json.dumps({"time": "2026-11-13T09:00:00+0000", "boot_id": "aaaa"})), w.boot("cccc", "root=x"))), "boot")
    run("boot, an attempt without a time", W(lambda w: (w.state("trying", json.dumps({"boot_id": "aaaa"})), w.boot("cccc", "root=x"))), "boot")
    run("boot, an attempt with a number as time", W(lambda w: (w.state("trying", json.dumps({"time": 1234, "boot_id": "aaaa"})), w.boot("cccc", "root=x"))), "boot")
    run("boot, failed attempt and picked again", W(lambda w: (w.state("trying", json.dumps({"time": "t", "boot_id": "aaaa"})), w.state("state.json", '{"state": "idle", "keep": true}'), w.boot("cccc", picked))), "boot")
    run("boot, this start's attempt again", W(lambda w: (w.state("trying", json.dumps({"time": "t", "boot_id": "bbbb"})), w.boot("bbbb", picked))), "boot")
    run("boot, trying a list", W(lambda w: (w.state("trying", "[]"), w.boot("bbbb", picked))), "boot")
    run("boot, the option in odd places", W(lambda w: w.boot("bbbb", "xturing_unlock.targets=all turing_unlock.targets= \tturing_unlock.targets=0000:01:00.0")), "boot")
    run("boot, the option glued", W(lambda w: w.boot("bbbb", "xturing_unlock.targets=all")), "boot")
    run("boot, not root", W(lambda w: w.boot("bbbb", picked)), "boot", user="user")

    def attempt(w, log="", bench=None, base=None):
        w.state("entry.cfg", GOOD_ENTRY)
        w.state("power-hold", json.dumps({"watts": None}))
        w.files["boot/grub/grub.cfg"] = GRUB_BASE + GOOD_ENTRY
        w.state("trying", json.dumps({"time": "2026-11-13T09:00:00+0000", "boot_id": "bbbb", "armed": "x"}))
        w.boot("bbbb", picked)
        w.case["kernel.log"] = log
        if bench is not None:
            w.case["bench.out"] = bench if isinstance(bench, str) else json.dumps(bench)
        if base is not None:
            w.state("baseline.json", base if isinstance(base, str) else json.dumps(base))
    base_ok = dict(LOCKED, bdf=MSI["bdf"], vbios="90.02.17.00.3E")
    run("verify, nothing pending", W(), "verify")
    run("verify, armed (a normal start)", W(lambda w: w.armed()), "verify")
    run("verify, an earlier attempt's entry left", W(lambda w: w.state("entry.cfg", GOOD_ENTRY)), "verify")
    run("verify, trying from another start", W(lambda w: (w.state("trying", json.dumps({"boot_id": "zzzz"})), w.state("entry.cfg", GOOD_ENTRY))), "verify")
    run("verify, the module did not run", W(lambda w: attempt(w, "usb 1-3: new device\n")), "verify")
    run("verify, the module failed", W(lambda w: attempt(w, "turing_unlock: failure unlock.c:314 gsp_fwsec_frts error=-5\npci 0000:01:00.0: turing_unlock: unlock failed (-5)\n")), "verify")
    run("verify, the module gave no result", W(lambda w: attempt(w, "turing_unlock: loading " + "y" * 200 + "\n")), "verify")
    run("verify, unlocked, no benchmark source", W(lambda w: (attempt(w, UNLOCKED_LOG), w.files.pop(".usr/lib/yggdrasil/gpuunlock-bench.cu"))), "verify")
    run("verify, unlocked, no CUDA", W(lambda w: (attempt(w, UNLOCKED_LOG), setattr(w, "nvcc", None))), "verify")
    run("verify, unlocked, nvcc fails", W(lambda w: (attempt(w, UNLOCKED_LOG), w.case.update({"nvcc.code": "2"}))), "verify")
    run("verify, unlocked, nvcc makes nothing", W(lambda w: (attempt(w, UNLOCKED_LOG), w.case.update({"nvcc.nothing": ""}))), "verify")
    run("verify, unlocked, the benchmark says nothing", W(lambda w: attempt(w, UNLOCKED_LOG, "")), "verify")
    run("verify, unlocked, the benchmark stops", W(lambda w: attempt(w, UNLOCKED_LOG, {"error": "cudaSetDevice: no device " + "z" * 200})), "verify")
    run("verify, unlocked, the benchmark's last line not JSON", W(lambda w: attempt(w, UNLOCKED_LOG, json.dumps(UNLOCKED) + "\ndone")), "verify")
    run("verify, unlocked, wrong results", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, check={"compared": 4096, "wrong": 3, "ok": False}), base_ok)), "verify")
    run("verify, unlocked, quick", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, quick=True), base_ok)), "verify")
    run("verify, unlocked, no baseline", W(lambda w: attempt(w, UNLOCKED_LOG, UNLOCKED, "{}")), "verify")
    run("verify, unlocked, not fast enough", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, mma_f16_acc_f32=60.0), base_ok)), "verify")
    run("verify, unlocked, the control moved", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, mma_f16_acc_f16=70.0), base_ok)), "verify")
    run("verify, unlocked, a rate as text", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, mma_f16_acc_f32="99.1"), base_ok)), "verify")
    run("verify, unlocked, nan", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, mma_f16_acc_f32="nan"), base_ok)), "verify")
    run("verify, unlocked, baseline inf", W(lambda w: attempt(w, UNLOCKED_LOG, UNLOCKED, dict(base_ok, mma_f16_acc_f16="inf"))), "verify")
    run("verify, unlocked, a rate missing", W(lambda w: attempt(w, UNLOCKED_LOG, {k: v for k, v in UNLOCKED.items() if k != "mma_f16_acc_f16"}, base_ok)), "verify")
    run("verify, unlocked, a list", W(lambda w: attempt(w, UNLOCKED_LOG, [1, 2], base_ok)), "verify")
    run("verify, unlocked, a number", W(lambda w: attempt(w, UNLOCKED_LOG, "5", base_ok)), "verify")
    run("verify, unlocked, text with error", W(lambda w: attempt(w, UNLOCKED_LOG, '"an error"', base_ok)), "verify")
    run("verify, unlocked, check a list", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, check=[1]), base_ok)), "verify")
    run("verify, unlocked, cuBLAS as text", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, gemm_f16_acc_f32=[{"n": 8192.0, "tflops": "77.9"}]), base_ok)), "verify")
    run("verify, unlocked, cuBLAS without 8192", W(lambda w: attempt(w, UNLOCKED_LOG, dict(UNLOCKED, gemm_f16_acc_f32=[{"n": 4096, "tflops": 1}]), base_ok)), "verify")
    run("verify, unlocked and confirmed, hot at once", W(lambda w: attempt(w, UNLOCKED_LOG, UNLOCKED, base_ok)), "verify")
    run("verify, unlocked and confirmed, slowing itself down", W(lambda w: (attempt(w, UNLOCKED_LOG, UNLOCKED, base_ok), w.case.update({"nvidia-smi.thermal": "70, Active\n"}))), "verify")
    run("verify, unlocked and confirmed, no temperature", W(lambda w: (attempt(w, UNLOCKED_LOG, UNLOCKED, base_ok), w.case.update({"nvidia-smi.thermal": "[N/A], Active\n"}))), "verify")
    run("verify, unlocked and confirmed, a thermal line without a comma", W(lambda w: (attempt(w, UNLOCKED_LOG, UNLOCKED, base_ok), w.case.update({"nvidia-smi.thermal": "88\n1,2\n"}))), "verify")
    run("verify, unlocked and confirmed, the benchmark on stderr", W(lambda w: (attempt(w, UNLOCKED_LOG, "", base_ok), w.case.update({"bench.err": json.dumps(UNLOCKED)}))), "verify")
    run("verify, unlocked and confirmed, a cached benchmark", W(lambda w: (attempt(w, UNLOCKED_LOG, UNLOCKED, base_ok), setattr(w, "nvcc", None), w.files.update({"var/cache/yggdrasil/gpuunlock-bench/gpuunlock-bench-" + bench_digest(): "#!/bin/sh\necho '" + json.dumps(UNLOCKED) + "'\n"}), w.modes.update({"var/cache/yggdrasil/gpuunlock-bench/gpuunlock-bench-" + bench_digest(): 0o755}))), "verify")
    run("verify, not root", W(lambda w: attempt(w, UNLOCKED_LOG)), "verify", user="user")
    run("verify, no card any more", W(lambda w: (attempt(w, UNLOCKED_LOG, UNLOCKED, base_ok), w.no_card())), "verify")

    # ---- the benchmark
    run("bench --quick, not root", W(lambda w: w.case.update({"bench.out": json.dumps(dict(LOCKED, quick=True))})), "bench", "--quick", user="user")
    run("bench, not root", W(lambda w: w.case.update({"bench.out": json.dumps(LOCKED)})), "bench", user="user")
    run("bench, not root, XDG_CACHE_HOME", W(lambda w: (w.case.update({"bench.out": json.dumps(LOCKED)}), w.env.update({"XDG_CACHE_HOME": "/home/u/c"}))), "bench", user="user")
    run("bench, not root, empty XDG_CACHE_HOME and HOME /", W(lambda w: (w.case.update({"bench.out": json.dumps(LOCKED)}), w.env.update({"XDG_CACHE_HOME": "", "HOME": "/"}), w.dirs.append(".cache"))), "bench", user="user")
    run("bench, nvcc on the PATH", W(lambda w: (w.case.update({"bench.out": json.dumps(LOCKED)}), setattr(w, "nvcc", "path"))), "bench")
    run("bench, wrong results", W(lambda w: w.case.update({"bench.out": json.dumps(dict(LOCKED, check={"ok": False})), "bench.code": "1"})), "bench")
    run("bench, busy", W(lambda w: w.case.update({"nvidia-smi.pmon": PMON_HEAD + "    0       4242     C     97     40      -      -      -      -    ollama         \n"})), "bench")
    run("bench, no card", W(lambda w: w.no_card()), "bench")
    run("bench, no driver", W(lambda w: w.card(MSI, loaded=False)), "bench")
    run("bench, no driver version", W(lambda w: w.files.update({".proc/driver/nvidia/version": "NVRM version: unknown\n"})), "bench")
    run("bench --baseline", W(lambda w: (w.state("baseline.json", None), w.case.update({"bench.out": json.dumps(LOCKED)}))), "bench", "--baseline")
    run("bench --baseline over an old one", W(lambda w: w.case.update({"bench.out": json.dumps(dict(LOCKED, time="old", watts=1))})), "bench", "--baseline")
    run("bench --baseline, not root", W(), "bench", "--baseline", user="user")
    run("bench --baseline, unlocked", W(lambda w: w.case.update({"kernel.log": UNLOCKED_LOG})), "bench", "--baseline")
    for power in ["290.00, 260.00", "260.50, 260.00", "261.5, 260", "nan, 260.00", "inf, inf", "[N/A], [N/A]", "", "260, 260, 1", " 2_60 , 260 \n"]:
        run(f"bench --baseline, power {power!r}", W(lambda w, power=power: (w.state("baseline.json", None), w.case.update({"nvidia-smi.power": power, "bench.out": json.dumps(LOCKED)}))), "bench", "--baseline")
    run("bench --baseline, wrong results", W(lambda w: (w.state("baseline.json", None), w.case.update({"bench.out": json.dumps(dict(LOCKED, check={"ok": False}))}))), "bench", "--baseline")
    run("bench --baseline, no check", W(lambda w: (w.state("baseline.json", None), w.case.update({"bench.out": json.dumps({k: v for k, v in LOCKED.items() if k != "check"})}))), "bench", "--baseline")
    run("bench --baseline, a rate missing", W(lambda w: (w.state("baseline.json", None), w.case.update({"bench.out": json.dumps({k: v for k, v in LOCKED.items() if k != "mma_f16_acc_f16"})}))), "bench", "--baseline")
    run("bench --baseline, cuBLAS not a list", W(lambda w: (w.state("baseline.json", None), w.case.update({"bench.out": json.dumps(dict(LOCKED, gemm_f16_acc_f32=5))}))), "bench", "--baseline")
    run("bench --baseline, odd numbers", W(lambda w: (w.state("baseline.json", None), w.case.update({"bench.out": '{"check": {"ok": 1}, "mma_f16_acc_f32": 1e2, "mma_f16_acc_f16": 1E-5, "z": -0.0, "a": 12345678901234567, "u": "\\u00e9\\ud83d\\ude00"}'}))), "bench", "--baseline")
    run("bench --baseline, busy", W(lambda w: w.case.update({"nvidia-smi.pmon": PMON_HEAD + "    0       4242     C     97     40      -      -      -      -    ollama         \n"})), "bench", "--baseline")
    run("bench, the cache a file", W(lambda w: w.files.update({"var/cache/yggdrasil/gpuunlock-bench": "x"})), "bench")
    run("bench --quick, cached", W(lambda w: (setattr(w, "nvcc", None), w.files.update({"var/cache/yggdrasil/gpuunlock-bench/gpuunlock-bench-" + bench_digest(): "#!/bin/sh\necho '{\"check\": {\"ok\": true}}'\n"}), w.modes.update({"var/cache/yggdrasil/gpuunlock-bench/gpuunlock-bench-" + bench_digest(): 0o755}))), "bench", "--quick")
    run("bench, the benchmark exits 3 with nothing", W(lambda w: w.case.update({"bench.out": "", "bench.code": "3"})), "bench")

    # ---- the grub.d fragment, as grub-mkconfig runs it
    frag = lambda label, content, link=False, folder=False: compare(
        f"fragment: {label}",
        W(lambda w: (w.links.update({f"{STATE}/entry.cfg": "/good.cfg"}) if link else None,
                     w.files.update({"good.cfg": GOOD_ENTRY}) if link else None,
                     w.dirs.append(f"{STATE}/entry.cfg") if folder else None,
                     w.state("entry.cfg", content) if content is not None else None)),
        ["/etc/grub.d/41_gpu_unlock"])
    g = GOOD_ENTRY
    frag("no attempt", None)
    frag("the entry while armed", g)
    for i, bad in enumerate([g.replace("--id yggdrasil-gpu-unlock", "--id other"), g.replace("targets=0000:01:00.0", "targets=all"),
                             g.replace("targets=0000:01:00.0", "targets=0000:01:00.0 init=/bin/sh"), g + g,
                             g.replace("\tinitrd", "\tsubmenu 'x' {\n\tinitrd"), g.replace("\tinitrd", "\tlinux\t/other\n\tinitrd"),
                             g.replace("\tinitrd", "  menuentry 'x' {\n\tinitrd"), g.replace("\tinitrd", "\u2003linux\t/x\n\tinitrd"),
                             g.replace("\tinitrd", "\u00a0linux\t/x\n\tinitrd"),
                             g.replace("rw quiet turing", "rw quiet  turing"), g.replace(" turing_unlock", "turing_unlock"),
                             g.replace("\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet", "\tlinux"),
                             g.replace("\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet", "\tlinux "),
                             g.replace("\tlinux\t", "linux16 "), g.replace("\tlinux\t", "\tlinuxefi\t"), g.replace("0000:01:00.0", "0000:01:00.8"),
                             g.replace("0000:01:00.0", "0000:01:00.0\r"), g.replace("\n", "\r\n"), g.replace("rw quiet", "rw\u3000quiet"),
                             g[:-2], g[:-1], g + "\n", "", "\n", "}\n", g.replace("menuentry", "menuentry\t", 1), "# a comment\n" + g,
                             g.replace("{\n", "{ \n", 1), g.replace("\n}\n", "\n }\n"), g.replace("caf", "caf\u00e9")]):
        frag(f"shape {i}: {bad[:60]!r}...{bad[-40:]!r}", bad)
    frag("a link", None, link=True)
    frag("a folder", None, folder=True)


def bench_digest():
    import hashlib
    with open(os.path.join(SETTINGS, "helpers", "gpuunlock-bench.cu"), "rb") as f:
        return hashlib.sha256(f.read()).hexdigest()[:16]


SIDES = {}


def main():
    global ARGS
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--python", default=os.path.join(SETTINGS, "helpers", "python", "gpuunlock"))
    ap.add_argument("--fragment", default=os.path.join(SETTINGS, "helpers", "python", "41_gpu_unlock"))
    ap.add_argument("--git", help="take the old helper and fragment from this commit (before the switch: ff5b6de)")
    ap.add_argument("--bin", default=os.path.join(REPO, "target", "release", "gpuunlock"))
    ap.add_argument("--only")
    ap.add_argument("-v", action="store_true")
    ARGS = ap.parse_args()
    if ARGS.git:
        show = lambda p: subprocess.run(["git", "-C", REPO, "show", f"{ARGS.git}:{p}"], capture_output=True, check=True).stdout
        old, old_frag = show("packages/settings/helpers/gpuunlock"), show("packages/settings/data/41_gpu_unlock")
    else:
        old, old_frag = open(ARGS.python, "rb").read(), open(ARGS.fragment, "rb").read()
    SIDES["python"] = (old, old_frag)
    with open(ARGS.bin, "rb") as f, open(os.path.join(SETTINGS, "data", "41_gpu_unlock"), "rb") as g:
        SIDES["rust"] = (f.read(), g.read())
    try:
        cases()
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)
    print(f"{COUNT - len(FAILS)} of {COUNT} cases the same" + (f"; differ: {', '.join(FAILS)}" if FAILS else ""))
    return 1 if FAILS or not COUNT else 0


if __name__ == "__main__":
    sys.exit(main())
