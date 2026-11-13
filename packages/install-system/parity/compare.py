#!/usr/bin/env python3
"""compare: the Rust yggdrasil-install-system and the Python script it replaces, run as programs on the same made-up
computers. Nothing real is touched:

  - every case runs in a throwaway user and mount namespace (unshare -rm, see sandbox.sh): "root" there is this user, with no
    power over the real computer; /dev is an empty tmpfs (no real disk can even be named), and /mnt, /run, /sys/firmware and
    the integration folders are tmpfs of their own; /etc/os-release, /etc/passwd and /etc/resolv.conf are the world's, and so is
    /etc/NetworkManager (a tmpfs with its saved networks: the real ones are never seen);
  - every program the install runs (lsblk, sfdisk, wipefs, cryptsetup, mkfs.*, mount, unsquashfs, chroot ...) is a stand-in
    (fake.py) on a PATH that holds nothing else: it writes down its arguments and what it read, and answers from a made-up
    computer whose disks are /dev/vdz and the like.

For each case both programs run on the same world (made again from scratch for each) and must give the same exit code,
standard output, standard error, calls (with what each read on standard input), files left in /mnt and /run, and the made-up
computer's state afterwards. Where the script stopped with a traceback, the Rust program stops with exit code 1 and one line
on standard error. On top of that, for every case:

  - the passwords in the plan appear nowhere but on chpasswd's and cryptsetup's standard input: not in any program's
    arguments, not in what the programs print, not in any file;
  - a plan that is refused (exit code 2) runs no program at all.

    compare.py [--python FILE] [--bin FILE] [--only TEXT] [--fuzz N] [--fuzz-install N] [--seed N] [--corpus DIR] [-j N] [-v] [--show]

--python: the script (default: ../../installer/helpers/python/install-system, else ../../installer/helpers/install-system)
--bin: the Rust program (default: ../../../target/release/yggdrasil-install-system; cargo build --release)
--fuzz N: N random plans (mutated ones, random JSON, random bytes), each run as an install
--fuzz-install N: N random plans that pass the checks, each run as a whole install
--corpus DIR: every file in DIR (the fuzz target's corpus, say) as a plan, each run as an install
"""
import argparse
import copy
import json
import os
import random
import shutil
import subprocess
import sys
import tempfile
import threading
from concurrent.futures import ThreadPoolExecutor

HERE = os.path.dirname(os.path.abspath(__file__))
FAKE = os.path.join(HERE, "fake.py")
SANDBOX = os.path.join(HERE, "sandbox.sh")
TOOLS = ["lsblk", "findmnt", "sfdisk", "wipefs", "cryptsetup", "mkfs.btrfs", "mkfs.ext4", "mkfs.xfs", "mkfs.vfat", "mkswap", "mount",
         "umount", "btrfs", "blkid", "udevadm", "unsquashfs", "chroot"]
GB = 1024**3
MIB = 1024**2
ESP = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
LINUX = "0FC63DAF-8483-4772-8E79-3D69D8477DE4"
MSR = "E3C9E316-0B5C-4DB8-817D-F92DF00215AE"
WINDOWS = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7"
SWAP = "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"
ACCOUNT_PW = "pw-ACCOUNT-1"
DISK_PW = "pw-DISK-secret-2"
ARGS = None
LOCK = threading.Lock()

# ---------------------------------------------------------------- the made-up computer

MODEL_MAP = """# consolelayout\txlayout\txmodel\txvariant\txoptions\tbcp47
uk\tgb\tpc105\t-\tterminate:ctrl_alt_bksp\ten-GB
us\tus\tpc105+inet\t-\tterminate:ctrl_alt_bksp\ten-US,en
dvorak\tus\tpc105\tdvorak\tterminate:ctrl_alt_bksp\t-
de-latin1\tde\tpc105\t-\tterminate:ctrl_alt_bksp\tde
es-dvorak\tes\tmicrosoftpro\tdvorak\tterminate:ctrl_alt_bksp\t-
"""
GRUB = """# GRUB boot loader configuration

GRUB_DEFAULT=saved
GRUB_TIMEOUT=5
GRUB_DISTRIBUTOR="Arch"
GRUB_CMDLINE_LINUX_DEFAULT="loglevel=3 quiet"
GRUB_CMDLINE_LINUX=""

# Uncomment to enable booting from LUKS encrypted devices
#GRUB_ENABLE_CRYPTODISK=y

#GRUB_DISABLE_SUBMENU=y
GRUB_TERMINAL_INPUT=console
#GRUB_TERMINAL_OUTPUT=console
#GRUB_SAVEDEFAULT=true
"""
SYSTEMD_HOOKS = "MODULES=()\nBINARIES=()\nFILES=()\nHOOKS=(base systemd autodetect microcode modconf kms keyboard sd-vconsole block filesystems fsck)\n"
BUSYBOX_HOOKS = "MODULES=()\nHOOKS=(base udev autodetect microcode modconf kms keyboard keymap consolefont block filesystems fsck)\n"


def medium():
    files = {
        "etc/locale.gen": "# Configuration file for locale-gen\n#\n#  en_GB.UTF-8 UTF-8\n#en_US.UTF-8 UTF-8\n#en_US ISO-8859-1\n#de_DE.UTF-8 UTF-8\n#de_DE@euro ISO-8859-15\n#fr_FR.UTF-8 UTF-8\n",
        "etc/default/grub": GRUB,
        "etc/mkinitcpio.conf": SYSTEMD_HOOKS,
        "etc/xdg/mimeapps.list": "[Default Applications]\nx-scheme-handler/http=vivaldi-stable.desktop\ntext/html=vivaldi-stable.desktop\nvideo/mp4=yggdrasil-player.desktop\n",
        "etc/yggdrasil/dock": "# pins\nminos\nvivaldi-stable\nfirefox\nsteam\n",
        "etc/machine-id": "0123456789abcdef0123456789abcdef\n",
        "boot/initramfs-linux.img": "image\n",
        "boot/initramfs-linux-fallback.img": "image\n",
        "boot/vmlinuz-linux": "kernel\n",
        "usr/lib/systemd/system/greetd.service": "[Unit]\nDescription=Greeter daemon\n",
        "usr/share/systemd/kbd-model-map": MODEL_MAP,
        "usr/share/kbd/keymaps/i386/qwerty/us.map.gz": "",
        "usr/share/kbd/keymaps/i386/azerty/fr.map.gz": "",
        "usr/share/kbd/keymaps/i386/colemak/colemak.map.gz": "",
        "usr/share/kbd/keymaps/i386/qwertz/de.map": "",
        "usr/share/applications/vivaldi-stable.desktop": "[Desktop Entry]\n",
        "usr/share/applications/firefox.desktop": "[Desktop Entry]\n",
        "usr/local/bin/example-install-cleanup": "#!/bin/sh\n",
        "usr/local/bin/example-install-finish": "#!/bin/sh\n",
        "usr/local/bin/example-grub-theme": "#!/bin/sh\n",
    }
    return {"files": files, "links": {"etc/resolv.conf": "../run/systemd/resolve/stub-resolv.conf", "etc/localtime": "/usr/share/zoneinfo/UTC"},
            "dirs": ["var/lib", "run", "proc", "sys", "dev"]}


def part(disk, n, start_mib, size_mib, ptype, fstype=None, label=None, mounted=None, partlabel=None):
    path = f"{disk}p{n}" if disk[-1].isdigit() else f"{disk}{n}"
    return {"n": n, "path": path, "start": start_mib * 2048, "sectors": size_mib * 2048, "parttype": ptype, "fstype": fstype, "label": label,
            "partlabel": partlabel, "mountpoints": [mounted] if mounted else [None]}


def disk(path="/dev/vdz", size=500 * GB, table="", parts=(), model="TEST DISK 0001", tran="virtio", rm=False, ro=False, mounted=None):
    return {"name": path[5:], "path": path, "size": size, "model": model, "tran": tran, "rm": rm, "ro": ro, "type": "disk", "table": table,
            "parts": list(parts), "mountpoints": [mounted] if mounted else [None]}


def windows_disk(path="/dev/vdz"):
    """A 500 GB disk with Windows on it: its EFI partition, its reserved partition, Windows itself, and 200 GB free at the end."""
    return disk(path, table="gpt", parts=[
        part(path, 1, 1, 300, ESP, "vfat", "SYSTEM", partlabel="EFI system partition"),
        part(path, 2, 301, 16, MSR, partlabel="Microsoft reserved partition"),
        part(path, 3, 317, 299 * 1024, WINDOWS, "ntfs", "Windows"),
    ])


BROWSERS = {"browsers": [
    {"id": "vivaldi", "name": "Vivaldi", "chosen": True, "packages": ["vivaldi", "vivaldi-ffmpeg-codecs"], "desktop": ["vivaldi-stable.desktop"]},
    {"id": "firefox", "name": "Firefox", "packages": ["firefox"], "desktop": ["firefox.desktop"]},
    {"id": "librewolf", "name": "LibreWolf", "packages": ["librewolf"], "desktop": ["librewolf.desktop"]},
    {"id": "opera-gx", "name": "Opera GX", "flatpak": "com.opera.opera-gx", "desktop": ["com.opera.opera-gx.desktop"]},
    {"name": "no id"}]}
EXTRAS = {"title": "More", "extras": [{"id": "assistant", "name": "Assistant", "components": ["assistant"]},
                                      {"id": "monitor", "name": "Monitor", "components": ["monitor"]},
                                      {"id": "mice", "name": "Mice", "components": ["mice", "nothing-such"]}]}
COMPONENTS = {"components": [
    {"id": "assistant", "name": "The Assistant", "packages": ["assistant-git"], "enable_global": ["assistant.service"]},
    {"id": "monitor", "name": "The Monitor", "packages": ["monitor-git", "clamav"], "enable_system": ["monitor.service", "clamav-freshclam.service"],
     "add_groups": ["monitor", "wheel"]},
    {"id": "mice", "pkgbuild": "mice-tool", "aur": ["mice-lib"]}]}
CONF = """kind = installer
command = /usr/bin/yggdrasil-install-system
name = Example
cleanup = /usr/local/bin/example-install-cleanup
finish = /usr/local/bin/example-install-finish
boot_menu_theme = /usr/local/bin/example-grub-theme
kernel_options = quiet splash
groups = users lp video network storage wheel audio openrazer kvm
reserved_logins = assistant monitor
browsers = /run/os/installer-browsers.json
extras = /run/os/installer-extras.json
components = /run/os/components.json
first_login = /var/lib/example/first-login-components.json
"""
PASSWD = ("root:x:0:0::/root:/bin/bash\nbin:x:1:1::/:/usr/bin/nologin\nlive:x:1000:1000:Live:/home/live:/bin/bash\n"
          "greeter:x:964:964::/var/lib/greeter:/usr/bin/nologin\nnobody:x:65534:65534:Kernel Overflow User:/:/usr/bin/nologin\n"
          "taken:x:1001:1001::/home/taken:/bin/bash\n")


def world(**kw):
    """A made-up computer: what compare() builds for one case."""
    w = {
        "conf": {"example-installer.conf": CONF},
        "etc_conf": {},
        "os_release": 'NAME="Example OS"\nID=example\nPRETTY_NAME="Example OS"\n',
        "passwd": PASSWD,
        "resolv": "# made up for the tests\nsearch example.invalid\noptions edns0\n",
        "efi": True,
        "efivars": True,
        "sfs": True,
        "run_files": {"/run/os/installer-browsers.json": json.dumps(BROWSERS), "/run/os/installer-extras.json": json.dumps(EXTRAS),
                      "/run/os/components.json": json.dumps(COMPONENTS)},
        "mnt_files": {},
        # the live system's /etc/NetworkManager (networks(): its saved networks); none at all by default
        "nm": None,
        "state": {
            "disks": [disk(), disk("/dev/sdy", size=16 * GB, table="dos", tran="usb", rm=True, model="TEST USB 0002",
                                   parts=[part("/dev/sdy", 1, 1, 15000, "0x17", "iso9660", "EXAMPLE_202611", mounted="/run/archiso/bootmnt")])],
            "other_devices": [{"name": "loop0", "path": "/dev/loop0", "size": 2 * GB, "model": None, "tran": None, "ro": True, "rm": False, "type": "loop"},
                              {"name": "zram0", "path": "/dev/zram0", "size": 8 * GB, "model": None, "tran": None, "ro": False, "rm": False, "type": "disk"},
                              {"name": "sr0", "path": "/dev/sr0", "size": 1 * GB, "model": "TEST DVD", "tran": "sata", "ro": False, "rm": True, "type": "rom"}],
            "live": {"source": "/dev/sdy1", "pkname": "sdy"},
            "installed": ["vivaldi", "vivaldi-ffmpeg-codecs", "firefox", "assistant-git", "monitor-git", "clamav", "steam"],
            "needs": {},
            "sync_ok": ["librewolf", "firefox", "vivaldi", "vivaldi-ffmpeg-codecs"],
            "syu_ok": [],
            "flathub_ok": ["com.opera.opera-gx"],
            "desktop_of": {"librewolf": "librewolf.desktop", "firefox": "firefox.desktop", "vivaldi": "vivaldi-stable.desktop"},
            "groups": ["users", "lp", "video", "network", "storage", "wheel", "audio", "kvm"],
            "programs": {"/usr/local/bin/example-install-cleanup": {"code": 0, "stdout": "cleaned\n"},
                         "/usr/local/bin/example-install-finish": {"code": 0, "stdout": "all good\n"},
                         "/usr/local/bin/example-grub-theme": {"code": 0}},
            "medium": medium(),
            "fail": [],
        },
    }
    for k, v in kw.items():
        if k == "state":
            for sk, sv in v.items():
                # the OS's programs are added to (or changed), not replaced
                if sk == "programs":
                    w["state"]["programs"].update(sv)
                else:
                    w["state"][sk] = sv
        else:
            w[k] = v
    return w


def plan(**kw):
    p = {"disk": "/dev/vdz", "mode": "erase", "encrypt": False, "encrypt_password": "",
         "user": {"name": "Ada Example", "login": "ada", "password": ACCOUNT_PW},
         "hostname": "test-pc", "timezone": "Europe/Lisbon", "locale": "en_US.UTF-8", "keymap": "us", "keymap_variant": ""}
    p.update(kw)
    return p


def manual_plan(**kw):
    p = plan(mode="manual", new_table=False, delete=[], create=[{"id": "r", "start": 300 * GB, "size": 150 * GB, "kind": "linux"}],
             use=[{"part": "/dev/vdz1", "mount": "/boot/efi", "format": None}, {"part": "new:r", "mount": "/", "format": "btrfs"}])
    p.update(kw)
    return p


# ---------------------------------------------------------------- running one

def write(path, data, mode=None):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(data.encode() if isinstance(data, str) else data)
    if mode is not None:
        os.chmod(path, mode)


def build(w, spec):
    shutil.rmtree(w, ignore_errors=True)
    for d in ("bin", "root/run", "root/mnt", "root/firmware", "root/share-integrations", "root/etc-yggdrasil"):
        os.makedirs(os.path.join(w, d))
    for t in TOOLS:
        os.symlink(FAKE, os.path.join(w, "bin", t))
    write(os.path.join(w, "etc/os-release"), spec["os_release"])
    write(os.path.join(w, "etc/passwd"), spec["passwd"])
    write(os.path.join(w, "etc/resolv.conf"), spec["resolv"])
    for name, text in spec["conf"].items():
        write(os.path.join(w, "root/share-integrations", name), text)
    for name, text in spec["etc_conf"].items():
        write(os.path.join(w, "root/etc-yggdrasil/integrations", name), text)
    if spec["efi"]:
        os.makedirs(os.path.join(w, "root/firmware/efi/efivars" if spec["efivars"] else "root/firmware/efi"))
    if spec["sfs"]:
        sfs = spec["sfs"] if isinstance(spec["sfs"], str) else "/run/archiso/bootmnt/example/x86_64/airootfs.sfs"
        write(os.path.join(w, "root", sfs.lstrip("/")), "squashfs\n")
    os.makedirs(os.path.join(w, "root/run/udev"), exist_ok=True)
    for path, text in spec["run_files"].items():
        write(os.path.join(w, "root", path.lstrip("/")), text)
    for path, text in spec["mnt_files"].items():
        write(os.path.join(w, "root", path.lstrip("/")), text)
    os.makedirs(os.path.join(w, "root/nm"))
    nm = spec.get("nm")
    if nm is not None:
        top = os.fsencode(os.path.join(w, "root/nm"))
        conns = os.path.join(top, b"system-connections")
        os.makedirs(conns, mode=0o700)
        for name, data in nm["files"].items():
            write(os.path.join(conns, os.fsencode(name)), data, 0o600)
        for name, dest in nm["links"].items():
            os.symlink(dest, os.path.join(conns, os.fsencode(name)))
        for name in nm["fifos"]:
            os.mkfifo(os.path.join(conns, os.fsencode(name)), 0o600)
        for name in nm["dirs"]:
            os.makedirs(os.path.join(conns, os.fsencode(name)))
        os.chmod(conns, 0o700)
    write(os.path.join(w, "state.json"), json.dumps(spec["state"], indent=1, sort_keys=True))


class Run:
    def __init__(self, p, w):
        self.code, self.out, self.err = p.returncode, p.stdout, p.stderr
        calls = os.path.join(w, "calls.jsonl")
        self.calls = [json.loads(l) for l in open(calls)] if os.path.exists(calls) else []
        snap = os.path.join(w, "snapshot.json")
        self.files = json.load(open(snap)) if os.path.exists(snap) else None
        self.state = json.load(open(os.path.join(w, "state.json")))


def run_once(w, spec, argv, stdin, env):
    build(w, spec)
    e = {"PATH": "/usr/bin:/bin", "FAKEPATH": os.path.join(w, "bin"), "WORLD": w, "LANG": "C.UTF-8"}
    e.update(env)
    p = subprocess.run(["unshare", "-r", "-m", "--propagation", "private", "sh", SANDBOX, w] + argv, env=e, input=stdin, capture_output=True,
                       timeout=600)
    if p.returncode in (97, 98, 99):
        raise SystemExit(f"compare: the sandbox could not be made ({p.returncode}): {p.stderr.decode(errors='replace')[-500:]}")
    return Run(p, w)


FAILS = []
COUNTS = {}


def secrets_of(stdin):
    """The passwords a plan carries (the account's and the disk's), as text: those with the tests' mark "pw-" in them (a random
    plan's password can be a word such as "vfat", which is rightly everywhere)."""
    try:
        p = json.loads(stdin)
    except ValueError:
        return []
    out = []
    if isinstance(p, dict):
        u = p.get("user")
        if isinstance(u, dict) and isinstance(u.get("password"), str) and "pw-" in u["password"]:
            out.append(u["password"])
        if isinstance(p.get("encrypt_password"), str) and "pw-" in p["encrypt_password"]:
            out.append(p["encrypt_password"])
    return out


def leaks(r, secrets):
    """Where a password showed up that it must not: anything but chpasswd's and cryptsetup's standard input."""
    found = []
    for s in secrets:
        if s.encode() in r.out or s.encode() in r.err:
            found.append(f"{s!r} printed")
        for c in r.calls:
            if any(s in a for a in c["argv"]):
                found.append(f"{s!r} in the arguments of {c['argv'][:3]}")
            if c["stdin"] and s in c["stdin"]:
                prog = c["argv"][4] if c["argv"][0] == "chroot" else c["argv"][0]
                if not (prog == "chpasswd" or (prog == "cryptsetup" and c["argv"][1] in ("luksFormat", "open", "luksAddKey"))):
                    found.append(f"{s!r} on the standard input of {prog}")
        for path, (_, _, content) in (r.files or {}).items():
            if s in content or s.encode().hex() in content:
                found.append(f"{s!r} in {path}")
    return found


def compare(group, label, spec, args=(), stdin=b"", env=None):
    if ARGS.only and ARGS.only not in f"{group}: {label}":
        return
    env = env or {}
    if isinstance(stdin, (dict, list)):
        stdin = json.dumps(stdin).encode()
    base = tempfile.mkdtemp(prefix="w", dir=SCRATCH)
    try:
        w = os.path.join(base, "world")
        py = run_once(w, spec, [PYTHON] + list(args), stdin, env)
        rs = run_once(w, spec, [BIN] + list(args), stdin, env)
    finally:
        shutil.rmtree(base, ignore_errors=True)
    problems = []
    if b"Traceback" in py.err:
        if not (rs.code == 1 and b"unexpected error" in rs.err):
            problems.append(f"the script stopped (traceback) and the Rust one did not: rust {rs.code} {rs.err[-300:]!r}; python {py.err[-400:]!r}")
    else:
        if py.code != rs.code:
            problems.append(f"exit code: python {py.code}, rust {rs.code}")
        if py.err != rs.err:
            problems.append(f"stderr:\n    python {py.err[-500:]!r}\n    rust   {rs.err[-500:]!r}")
    if py.out != rs.out:
        problems.append(f"stdout:\n    python {py.out[-1500:]!r}\n    rust   {rs.out[-1500:]!r}")
    if py.calls != rs.calls:
        for i, (a, b) in enumerate(zip(py.calls, rs.calls)):
            if a != b:
                problems.append(f"call {i}:\n    python {a}\n    rust   {b}")
                break
        else:
            problems.append(f"calls: python made {len(py.calls)}, rust {len(rs.calls)}: {(py.calls if len(py.calls) > len(rs.calls) else rs.calls)[min(len(py.calls), len(rs.calls))]}")
    for k in sorted(set(py.files or {}) | set(rs.files or {})):
        a, b = (py.files or {}).get(k), (rs.files or {}).get(k)
        if a != b:
            problems.append(f"file {k}:\n    python {str(a)[:400]}\n    rust   {str(b)[:400]}")
    if py.state != rs.state:
        problems.append("the made-up computer's state afterwards differs")
    secrets = secrets_of(stdin)
    for who, r in (("python", py), ("rust", rs)):
        for l in leaks(r, secrets):
            problems.append(f"{who}: password leak: {l}")
        if r.code == 2 and r.calls:
            problems.append(f"{who}: a refused plan ran {len(r.calls)} programs: {r.calls[0]['argv']}")
    with LOCK:
        if ARGS.show:
            print(f"--- {group}: {label}\n{py.out.decode(errors='replace')}" + "".join(f"  {c['argv']}" + (f" <<< {c['stdin']!r}" if c['stdin'] else "") + "\n" for c in py.calls))
        COUNTS[group] = COUNTS.get(group, 0) + 1
        if problems:
            FAILS.append((group, label))
            print(f"DIFFERS {group}: {label}\n  " + "\n  ".join(problems), flush=True)
        elif ARGS.v:
            print(f"same {group}: {label} (exit {py.code}, {len(py.calls)} calls)", flush=True)
    return py, rs


# ---------------------------------------------------------------- the cases

def fail(*argv, code=1, stderr=None, stdout="", times=None):
    r = {"argv": list(argv), "code": code, "stdout": stdout}
    if stderr is not None:
        r["stderr"] = stderr
    if times is not None:
        r["times"] = times
    return r


def with_medium(change=None, drop=(), links=None):
    m = medium()
    m["files"].update(change or {})
    for d in drop:
        m["files"].pop(d, None)
    if links is not None:
        m["links"] = links
    return m


HOME_NET = b"[connection]\nid=Home Net\ntype=wifi\npermissions=user:live:;\n\n[wifi]\nssid=Home Net\n\n[wifi-security]\nkey-mgmt=wpa-psk\npsk=WIFI-3\n"


def networks(files=None, links=None, fifos=(), dirs=()):
    """The live system's saved networks (/etc/NetworkManager/system-connections): file name -> its bytes, links, pipes, folders."""
    return {"files": files if files is not None else {"Home Net.nmconnection": HOME_NET, "Cafe.nmconnection": b"[connection]\nid=Cafe\ntype=wifi\n"},
            "links": links or {}, "fifos": list(fifos), "dirs": list(dirs)}


def cases_disks(jobs):
    w = world()
    jobs.append(("disks", "the default computer (the live USB, loop, zram and DVD left out)", w, ["--disks"], b""))
    nv = "/dev/nvmez0n1"
    many = world(state={"disks": [
        windows_disk(),
        disk(nv, size=1000 * GB, table="gpt", model="  TEST NVME 0003  ", tran="nvme", parts=[
            part(nv, 1, 1, 512, ESP, "vfat", None, partlabel="ESP", mounted="/boot/efi"),
            part(nv, 2, 513, 100 * 1024, LINUX, "crypto_LUKS", None),
            part(nv, 4, 200 * 1024, 32 * 1024, SWAP, "swap", None, mounted="[SWAP]"),
            part(nv, 3, 513 + 100 * 1024 + 7, 3, LINUX)]),
        disk("/dev/vdx", size=20 * GB, model=None, tran=None),
        disk("/dev/vdw", size=64 * GB, table="dos", ro=True),
        disk("/dev/mmcblkz0", size=32 * GB, table="dos", tran=None, rm=True, parts=[part("/dev/mmcblkz0", 1, 4, 30000, "0xef", "vfat", "CARD")]),
        disk("/dev/sdy", size=16 * GB, table="dos", tran="usb", rm=True)]})
    many["state"]["disks"][1]["parts"][1]["children"] = [{"path": "/dev/mapper/x", "type": "crypt", "mountpoints": ["/home"]}]
    jobs.append(("disks", "Windows, NVMe in use, too small, read-only, a memory card", many, ["--disks"], b""))
    jobs.append(("disks", "no live medium", world(state={"live": None}), ["--disks"], b""))
    jobs.append(("disks", "a bigger smallest disk", world(conf={"x-installer.conf": "kind = installer\nmin_size_gb = 600\n"}), ["--disks"], b""))
    jobs.append(("disks", "lsblk fails", world(state={"fail": [fail("lsblk", "-J", "-b", "-d", stderr="lsblk: failed to access sysfs directory\n")]}), ["--disks"], b""))
    jobs.append(("disks", "lsblk of one disk fails", world(state={"fail": [fail("lsblk", "-J", "-b", "-o", code=32, stderr="lsblk: gone\n")]}), ["--disks"], b""))
    jobs.append(("disks", "lsblk says nothing", world(state={"fail": [fail("lsblk", "-J", "-b", "-d", code=0, stderr="")]}), ["--disks"], b""))
    jobs.append(("disks", "an integration file that is not UTF-8 is skipped", world(conf={"a-installer.conf": b"kind = installer\nmin_size_gb = 600\nname = \xff\n",
                                                                                          "b-installer.conf": "kind = installer\nmin_size_gb = 700\n"}), ["--disks"], b""))
    jobs.append(("disks", "a min_size_gb int() cannot read", world(conf={"x-installer.conf": "kind = installer\nmin_size_gb = ²\n"}), ["--disks"], b""))


def cases_check(jobs):
    for label, p in [
        ("a whole plan", plan()),
        ("not a JSON object", [1]),
        ("unknown mode", plan(mode="shrink")),
        ("a taken account", plan(user={"name": "T", "login": "taken", "password": "x"})),
        ("an account the integration reserves", plan(user={"name": "M", "login": "monitor", "password": "x"})),
        ("nobody", plan(user={"name": "N", "login": "nobody", "password": "x"})),
        ("a time zone that is not there", plan(timezone="Mars/Olympus")),
        ("a browser not offered", plan(browsers=["duckduckgo"])),
        ("browsers chosen", plan(browsers=["firefox", "librewolf"], default_browser="librewolf")),
        ("a default not chosen", plan(browsers=["firefox"], default_browser="vivaldi")),
        ("an extra not offered", plan(extras=["steam"])),
        ("extras chosen", plan(extras=["monitor"])),
        ("manual", manual_plan()),
    ]:
        jobs.append(("check", label, world(), ["--check"], json.dumps(p).encode()))
    jobs.append(("check", "an OS without lists offers no browsers", world(conf={"x.conf": "kind = installer\n"}), ["--check"], json.dumps(plan(browsers=["firefox"])).encode()))
    jobs.append(("check", "the administrator's integration file wins", world(etc_conf={"example-installer.conf": "kind = installer\nreserved_logins = ada\n"}),
                 ["--check"], json.dumps(plan()).encode()))
    for raw in (b"", b"  ", b"{", b"\xff", b"[" * 30, b'{"a": NaN}', b"\xef\xbb\xbf{}"):
        jobs.append(("check", f"stdin {raw[:12]!r}", world(), ["--check"], raw))


def cases_install(jobs):
    def add(label, w=None, p=None, args=(), env=None):
        jobs.append(("install", label, w or world(), list(args), json.dumps(p if p is not None else plan()).encode(), env or {}))

    add("erase, not encrypted")
    add("erase, encrypted (systemd's start-up image)", p=plan(encrypt=True, encrypt_password=DISK_PW))
    add("erase, encrypted (busybox's start-up image)", world(state={"medium": with_medium({"etc/mkinitcpio.conf": BUSYBOX_HOOKS})}), plan(encrypt=True, encrypt_password=DISK_PW))
    add("erase, encrypted, no mkinitcpio.conf", world(state={"medium": with_medium(drop=["etc/mkinitcpio.conf"])}), plan(encrypt=True, encrypt_password=DISK_PW))
    nv = world(state={"disks": [disk("/dev/nvmez0n1", tran="nvme")]})
    add("erase on an NVMe name (p1, p2)", nv, plan(disk="/dev/nvmez0n1"))
    add("erase, no efivars", world(efivars=False))
    add("an OS that says nothing", world(conf={}, os_release="", sfs="/run/archiso/bootmnt/linux/x86_64/airootfs.sfs"))
    add("Labyrinth-like names", world(conf={"l.conf": CONF.replace("name = Example\n", "name = Laby rinth\n")}, os_release='NAME="Labyrinth OS"\nID=example\n'))
    add("browsers: two chosen, one from Flathub", p=plan(browsers=["librewolf", "opera-gx"], default_browser="opera-gx"))
    add("browsers: the first chosen is the default", p=plan(browsers=["firefox", "librewolf"], default_browser=""))
    add("browsers: pacman -S fails, -Syu works", world(state={"sync_ok": [], "syu_ok": ["librewolf"]}), plan(browsers=["librewolf"], default_browser="librewolf"))
    add("browsers: nothing can be had", world(state={"sync_ok": [], "flathub_ok": []}), plan(browsers=["librewolf", "opera-gx"], default_browser="librewolf"))
    add("browsers: none chosen", p=plan(browsers=[], default_browser=""))
    add("browsers: one kept as another program needs it", world(state={"needs": {"steam": ["firefox"]}}), plan(browsers=["librewolf"], default_browser="librewolf"))
    add("browsers: removal in rounds", world(state={"installed": ["vivaldi", "vivaldi-ffmpeg-codecs", "firefox"], "needs": {"vivaldi": ["vivaldi-ffmpeg-codecs"]}}),
        plan(browsers=["librewolf"], default_browser="librewolf"))
    add("browsers: no dock, no mimeapps.list", world(state={"medium": with_medium({"etc/xdg/autostart/x.desktop": ""}, drop=["etc/yggdrasil/dock", "etc/xdg/mimeapps.list"])}),
        plan(browsers=["firefox"]))
    add("browsers: no /etc/xdg at all", world(state={"medium": with_medium(drop=["etc/xdg/mimeapps.list"])}), plan(browsers=["firefox"]))
    add("browsers: resolv.conf an absolute link with ..", world(state={"medium": with_medium(links={"etc/resolv.conf": "/../../run/x/../resolv.conf"})}), plan(browsers=["librewolf"]))
    add("browsers: resolv.conf a file", world(state={"medium": with_medium({"etc/resolv.conf": "nameserver ::1\n"}, links={})}), plan(browsers=["librewolf"]))
    add("extras: one kept, one removed", p=plan(extras=["monitor"]))
    add("extras: what the medium lacks, for the first login", world(state={"installed": ["monitor-git", "clamav"]}), plan(extras=["assistant", "monitor", "mice"]))
    add("extras: no first-login file", world(conf={"x.conf": CONF.replace("first_login = /var/lib/example/first-login-components.json\n", "")}, state={"installed": []}),
        plan(extras=["assistant"]))
    add("extras: none chosen", p=plan(extras=[]))
    add("extras and browsers", p=plan(browsers=["firefox"], default_browser="firefox", extras=["assistant"]))
    for loc in ("de_DE.UTF-8", "C", "en_GB.UTF-8", "de_DE@euro", "fr_FR", "ja_JP.EUC-JP"):
        add(f"language {loc}", p=plan(locale=loc))
    add("language: no locale.gen", world(state={"medium": with_medium(drop=["etc/locale.gen"])}))
    add("language: already there", world(state={"medium": with_medium({"etc/locale.gen": "en_US.UTF-8 UTF-8\n"})}))
    for layout, variant in (("gb", ""), ("us", "dvorak"), ("us", "colemak"), ("fr", ""), ("de", "nodeadkeys"), ("zz", "")):
        add(f"keyboard {layout} {variant}", p=plan(keymap=layout, keymap_variant=variant))
    add("keyboard: no tables", world(state={"medium": with_medium(drop=["usr/share/systemd/kbd-model-map", "usr/share/kbd/keymaps/i386/azerty/fr.map.gz"])}),
        plan(keymap="fr"))
    add("time zone and name", p=plan(timezone="Asia/Tokyo", hostname="a-1"))
    crlf = {k: v.replace("\n", "\r\n") for k, v in medium()["files"].items() if k.startswith(("etc/", "usr/share/systemd"))}
    crlf["etc/yggdrasil/dock"] = "# pins\rminos\r\nfirefox\r"
    add("files with CR and CRLF line ends", world(state={"medium": with_medium(crlf)}), plan(locale="de_DE.UTF-8", keymap="gb", browsers=["firefox"], default_browser="firefox"))
    odd = {"etc/locale.gen": "#\u00a0de_DE.UTF-8\u0085UTF-8\n#\nfr_FR.UTF-8 UTF-8\n#\x0b\n en_US.UTF-8 UTF-8\n",
           "etc/default/grub": "\u2003GRUB_TIMEOUT=9\n#\n\n  GRUB_DEFAULT=saved\n#GRUB_DISABLE_RECOVERY\x1c=true\nGRUB_CMDLINE_LINUX_DEFAULT=x\\y\nGRUB_TERMINAL_OUTPUT=gfxterm"}
    add("Unicode whitespace in locale.gen and GRUB's settings", world(state={"medium": with_medium(odd)}), plan(locale="de_DE.UTF-8"))
    add("the same, en_US", world(state={"medium": with_medium(odd)}), plan(locale="en_US.UTF-8"))
    add("kernel options with a backslash", world(conf={"x.conf": CONF.replace("kernel_options = quiet splash", "kernel_options = quiet a\\1 b\\g<0>")}))
    add("resolv.conf's link leads to a file that is there", world(state={"medium": with_medium({"run/systemd/resolve/stub-resolv.conf": "nameserver ::1\n"})}), plan(browsers=["librewolf"]))
    add("full name with commas and accents", p=plan(user={"name": "Zoë d'Example, Jr.", "login": "zoe", "password": "pw-p:a s\tw é"}))
    add("groups the system lacks", world(state={"groups": ["users", "wheel"]}))
    add("no clean-up program on the medium", world(state={"medium": with_medium(drop=["usr/local/bin/example-install-cleanup"])}))
    add("no greetd", world(state={"medium": with_medium(drop=["usr/lib/systemd/system/greetd.service"])}))
    add("the finishing program warns", world(state={"programs": {"/usr/local/bin/example-install-finish": {"code": 0, "stdout": "ok\nWARNING: no network\nsome WARNING here\n  WARNING:  spaced  \n"}}}))
    add("the finishing program fails", world(state={"programs": {"/usr/local/bin/example-install-finish": {"code": 3, "stdout": "WARNING: x\n" + "y" * 500 + "\n", "stderr": "bad"}}}))
    add("the finishing program fails quietly", world(state={"programs": {"/usr/local/bin/example-install-finish": {"code": 3, "stderr": "only here\n"}}}))
    add("the menu theme fails", world(state={"programs": {"/usr/local/bin/example-grub-theme": {"code": 1, "stdout": "z" * 300}}}))
    add("unsquashfs's own output", world(state={"unsquashfs": {"stdout": "Parallel unsquashfs\r\n5\r12\n\n  20  \nabc\n31\n29\n100", "code": 0}}))
    add("unsquashfs prints a digit int() cannot read", world(state={"unsquashfs": {"stdout": "10\n²\n", "code": 0}}))
    add("unsquashfs fails", world(state={"unsquashfs": {"stdout": "10\n", "stderr": "FATAL ERROR: " + "e" * 450 + "\n", "code": 1}}))
    add("no system copied (no etc)", world(state={"medium": {"files": {}, "links": {}, "dirs": []}}))
    add("saved networks: carried over, the live account's becomes the new one's", world(nm=networks()))
    add("saved networks: links, a folder, a pipe and other files left behind",
        world(nm=networks(files={"Home Net.nmconnection": HOME_NET, "notes.txt": b"x\n"}, links={"Link.nmconnection": "/etc/passwd", "Dangling.nmconnection": "/nowhere"},
                          fifos=["Pipe.nmconnection"], dirs=["Folder.nmconnection"])))
    add("saved networks: only things that are not plain files", world(nm=networks(files={}, links={"Link.nmconnection": "/etc/passwd"})))
    add("saved networks: an empty folder", world(nm=networks(files={})))
    add("saved networks: names and contents that are not UTF-8, CRLF, private to another account",
        world(nm=networks(files={b"Caf\xe9 \xff.nmconnection": b"[connection]\r\nid=\xff\r\npermissions = user:live\r\n",
                                 "Other.nmconnection": b"[connection]\npermissions=user:bob:;user:live:;\n[vpn]\npermissions=user:live:;\n"})),
        plan(user={"name": "Zoe", "login": "zoe_2-x", "password": ACCOUNT_PW}))
    add("saved networks: the new system's folder is a link", world(nm=networks(), state={"medium": with_medium(links={**medium()["links"], "etc/NetworkManager/system-connections": "/run/elsewhere"})}))
    add("saved networks: the new system has one of them already, and a link in another's place",
        world(nm=networks(), state={"medium": with_medium({"etc/NetworkManager/system-connections/Cafe.nmconnection": "old\n"},
                                                          links={**medium()["links"], "etc/NetworkManager/system-connections/Home Net.nmconnection": "/run/victim"})}))
    for label, rule in [
        ("wipefs fails", fail("wipefs", stderr="wipefs: error: /dev/vdz: probing initialization failed: Device or resource busy\n")),
        ("sfdisk fails", fail("sfdisk", "--wipe", stdout="out only\n", stderr="")),
        ("mkfs.btrfs fails", fail("mkfs.btrfs", stderr="ERROR: " + "x" * 600)),
        ("mount fails", fail("mount", "-o", stderr="mount: wrong fs type\n")),
        ("blkid finds nothing", fail("blkid", "-s", "UUID", code=2, stderr="")),
        ("useradd fails", fail("useradd", stderr="useradd: user 'ada' already exists\n")),
        ("chpasswd fails", fail("chpasswd", stderr="chpasswd: (user ada) pam_chauthtok() failed\n")),
        ("mkinitcpio fails", fail("mkinitcpio", stderr="==> ERROR: no kernel\n")),
        ("grub-install fails", fail("grub-install", "--target=x86_64-efi", "--efi-directory=/boot/efi", "--bootloader-id=Example", stderr="grub-install: error: no EFI\n")),
        ("grub's fallback copy fails (no matter)", fail("grub-install", "--target=x86_64-efi", "--efi-directory=/boot/efi", "--removable")),
        ("grub-mkconfig fails", fail("grub-mkconfig", stderr="syntax error\n")),
        ("systemctl enable greetd fails", fail("systemctl", "enable", "-f")),
        ("umount -R fails, then -l", fail("umount", "-R", "/mnt/example-install", times=1)),
        ("the clean-up program fails", fail("/usr/local/bin/example-install-cleanup", stderr="cannot clean\n")),
        ("the live medium's mount is in use", fail("lsblk", "-nr", stdout="/run/x\n", code=0)),
        ("lsblk -nr fails", fail("lsblk", "-nr", stderr="lsblk: x\n")),
        ("udevadm fails (no matter)", fail("udevadm", code=1)),
    ]:
        add(label, world(state={"fail": [rule]}), plan(encrypt=True, encrypt_password=DISK_PW) if "cryptsetup" in label else None)
    add("the live /dev's bind fails (made a slave, then unmounted)", world(state={"fail": [fail("mount", "--rbind", stderr="mount: busy\n")]}))
    add("the live /run/udev's bind fails", world(state={"fail": [fail("mount", "--bind", stderr="mount: no udev\n")]}))
    add("the new system cannot be made a slave: left mounted", world(state={"fail": [fail("mount", "--make-rslave")]}), plan(encrypt=True, encrypt_password=DISK_PW))
    add("the bind fails and so does making a slave: nothing unmounted", world(state={"fail": [fail("mount", "--rbind"), fail("mount", "--make-rslave")]}))
    add("a failure before the bind: unmounted as before", world(state={"fail": [fail("unsquashfs"), fail("mount", "--make-rslave")]}))
    add("cryptsetup luksFormat fails", world(state={"fail": [fail("cryptsetup", "luksFormat", stderr="Cannot format device\n")]}), plan(encrypt=True, encrypt_password=DISK_PW))
    add("cryptsetup open fails", world(state={"fail": [fail("cryptsetup", "open", stderr="No key available\n")]}), plan(encrypt=True, encrypt_password=DISK_PW))
    add("cryptsetup luksAddKey fails", world(state={"fail": [fail("cryptsetup", "luksAddKey", stderr="Key slot full\n")]}), plan(encrypt=True, encrypt_password=DISK_PW))
    add("not UEFI", world(efi=False))
    add("no medium", world(sfs=False))
    add("a disk that is not there", p=plan(disk="/dev/vdq"))
    add("the live USB itself", p=plan(disk="/dev/sdy"))
    add("too small", world(state={"disks": [disk(size=20 * GB)]}))
    add("something mounted", world(state={"disks": [disk(table="gpt", parts=[part("/dev/vdz", 1, 1, 1000, LINUX, "ext4", mounted="/media/x")])]}))
    add("not root", env={"NONROOT": "1"})
    add("a refused plan", p=plan(hostname="-x"))
    jobs.append(("install", "not JSON at all", world(), [], b"{not json", {}))
    add("the medium named in the integration file", world(conf={"x.conf": CONF + "medium = /run/elsewhere/system.sfs\n"}, sfs="/run/elsewhere/system.sfs"))
    add("the target named by YGGDRASIL_INSTALL_TARGET", env={"YGGDRASIL_INSTALL_TARGET": "/mnt/other"})

    # manual
    win = lambda **st: world(state=dict({"disks": [windows_disk()]}, **st))
    add("manual: beside Windows, its EFI partition kept", win(), manual_plan())
    add("manual: beside Windows, encrypted", win(), manual_plan(encrypt=True, encrypt_password=DISK_PW))
    add("manual: Windows deleted, its space used", win(), manual_plan(delete=[3, 2], create=[{"id": "r", "start": 317 * MIB, "size": 400 * GB, "kind": "linux"}]))
    add("manual: a number to delete twice", win(), manual_plan(delete=[3, 3], create=[{"id": "r", "start": 317 * MIB, "size": 300 * GB, "kind": "linux"}]))
    add("manual: the EFI partition formatted (and typed EFI)", win(), manual_plan(use=[{"part": "/dev/vdz1", "mount": "/boot/efi", "format": "vfat"}, {"part": "new:r", "mount": "/", "format": "btrfs"}]))
    add("manual: a new table, every kind", win(), manual_plan(new_table=True, delete=[3], create=[
        {"id": "e", "start": MIB, "size": 512 * MIB, "kind": "efi"}, {"id": "r", "start": 513 * MIB, "size": 100 * GB, "kind": "linux"},
        {"id": "h", "start": 513 * MIB + 100 * GB, "size": 200 * GB, "kind": "linux"}, {"id": "s", "start": 513 * MIB + 300 * GB, "size": 16 * GB, "kind": "swap"},
        {"id": "v", "start": 513 * MIB + 316 * GB, "size": 30 * GB, "kind": "linux"}],
        use=[{"part": "new:e", "mount": "/boot/efi", "format": "vfat"}, {"part": "new:r", "mount": "/", "format": "ext4"},
             {"part": "new:h", "mount": "/home", "format": "xfs"}, {"part": "new:s", "mount": "swap", "format": "swap"},
             {"part": "new:v", "mount": "/var", "format": "btrfs"}]))
    add("manual: btrfs / with its own /home (no @home)", win(), manual_plan(create=[{"id": "r", "start": 300 * GB, "size": 100 * GB, "kind": "linux"},
                                                                                {"id": "h", "start": 400 * GB, "size": 50 * GB, "kind": "linux"}],
                                                                        use=[{"part": "/dev/vdz1", "mount": "/boot/efi", "format": None}, {"part": "new:h", "mount": "/home", "format": "ext4"},
                                                                             {"part": "new:r", "mount": "/", "format": "btrfs"}]))
    kept = windows_disk()
    kept["parts"] += [part("/dev/vdz", 4, 300 * 1024, 40 * 1024, LINUX, "ext4", "data"), part("/dev/vdz", 5, 340 * 1024, 8 * 1024, SWAP, "swap"),
                      part("/dev/vdz", 6, 348 * 1024, 40 * 1024, LINUX, "btrfs", "srv")]
    add("manual: partitions kept as they are (/srv, /opt, swap)", world(state={"disks": [kept]}), manual_plan(create=[{"id": "r", "start": 400 * GB, "size": 90 * GB, "kind": "linux"}],
        use=[{"part": "/dev/vdz1", "mount": "/boot/efi", "format": None}, {"part": "new:r", "mount": "/", "format": "xfs"},
             {"part": "/dev/vdz4", "mount": "/srv", "format": None}, {"part": "/dev/vdz5", "mount": "swap", "format": None},
             {"part": "/dev/vdz6", "mount": "/opt", "format": None}]))
    add("manual: 4K sectors", world(state={"disks": [dict(windows_disk(), sectorsize=4096)]}), manual_plan())
    for label, p, st in [
        ("no partition table", manual_plan(), {"disks": [disk()]}),
        ("no partition to delete", manual_plan(delete=[9, 7]), {}),
        ("deleting a mounted one", manual_plan(delete=[3]), {"disks": [dict(windows_disk(), parts=windows_disk()["parts"][:2] + [dict(windows_disk()["parts"][2], mountpoints=["/win"])])]}),
        ("a partition not on the disk", manual_plan(use=[{"part": "/dev/vdq1", "mount": "/boot/efi", "format": None}, {"part": "new:r", "mount": "/", "format": "btrfs"}]), {}),
        ("a partition being deleted", manual_plan(delete=[1]), {}),
        ("a partition in use", manual_plan(), {"disks": [dict(windows_disk(), parts=[dict(windows_disk()["parts"][0], mountpoints=["/boot/efi"])] + windows_disk()["parts"][1:])]}),
        ("an EFI partition without FAT", manual_plan(use=[{"part": "/dev/vdz3", "mount": "/boot/efi", "format": None}, {"part": "new:r", "mount": "/", "format": "btrfs"}]), {}),
        ("keeping a partition with nothing on it", manual_plan(use=[{"part": "/dev/vdz1", "mount": "/boot/efi", "format": None}, {"part": "/dev/vdz2", "mount": "/home", "format": None},
                                                                    {"part": "new:r", "mount": "/", "format": "btrfs"}]), {}),
        ("a use without format (KeyError)", manual_plan(use=[{"part": "/dev/vdz1", "mount": "/boot/efi"}, {"part": "new:r", "mount": "/", "format": "btrfs"}]), {}),
        ("/ too small", manual_plan(create=[{"id": "r", "start": 300 * GB, "size": 10 * GB, "kind": "linux"}]), {}),
        ("the EFI partition too small", manual_plan(create=[{"id": "r", "start": 300 * GB, "size": 100 * GB, "kind": "linux"}, {"id": "e", "start": 400 * GB, "size": 100 * MIB, "kind": "efi"}],
                                                    use=[{"part": "new:e", "mount": "/boot/efi", "format": "vfat"}, {"part": "new:r", "mount": "/", "format": "btrfs"}]), {}),
        ("overlapping Windows", manual_plan(create=[{"id": "r", "start": 200 * GB, "size": 150 * GB, "kind": "linux"}]), {}),
        ("not on a MiB", manual_plan(create=[{"id": "r", "start": 300 * GB + 512, "size": 150 * GB, "kind": "linux"}]), {}),
        ("two new ones overlapping", manual_plan(create=[{"id": "r", "start": 300 * GB, "size": 150 * GB, "kind": "linux"}, {"id": "s", "start": 350 * GB, "size": 1 * GB, "kind": "swap"}]), {}),
        ("a huge number to delete", manual_plan(delete=[10**40, 2]), {}),
        ("the new partition is not found", manual_plan(), {"fail": [fail("sfdisk", "--append", code=0, stderr="")]}),
        ("sfdisk --delete fails", manual_plan(delete=[3], create=[{"id": "r", "start": 317 * MIB, "size": 300 * GB, "kind": "linux"}]), {"fail": [fail("sfdisk", "--delete", stderr="busy\n")]}),
    ]:
        add(f"manual: {label}", win(**st) if "disks" not in st else world(state=st), p)


# ---------------------------------------------------------------- random plans

def mutate(rng, v, depth=0):
    """A plan changed in one place: a value of another kind, a key taken out or added, text with odd characters."""
    odd = ["", "x\n", "a:b", "\x00", "../..", "/etc/hostname", "é", " ", "new:", "new:r", "/dev/vdz", "/dev/vdz1", "swap", "/", "/boot/efi",
           "btrfs", "vfat", "true", "false", "-1", "0", "UTC", "us", "dvorak", "en_US.UTF-8", "ada", "root", "A" * 40, "😀", "x" * 70]
    kinds = [None, True, False, 0, 1, -1, 2**70, 1.5, float("nan"), float("inf"), "", [], {}, ["a"], {"a": 1}, rng.choice(odd), rng.randrange(-5, 10**12)]
    if isinstance(v, dict) and v and rng.random() < 0.85:
        k = rng.choice(list(v))
        r = rng.random()
        if r < 0.15:
            v.pop(k)
        elif r < 0.25:
            v[rng.choice(["extra", "new_table", "delete", "browsers", "extras", "default_browser", "keymap_variant", "encrypt"])] = rng.choice(kinds)
        else:
            v[k] = mutate(rng, v[k], depth + 1)
        return v
    if isinstance(v, list) and v and rng.random() < 0.85:
        i = rng.randrange(len(v))
        r = rng.random()
        if r < 0.15:
            v.pop(i)
        elif r < 0.25:
            v.append(copy.deepcopy(rng.choice(v)))
        else:
            v[i] = mutate(rng, v[i], depth + 1)
        return v
    if isinstance(v, str) and rng.random() < 0.5:
        return rng.choice([v + rng.choice(odd), rng.choice(odd), v.upper(), v[:-1], v + "\n"])
    if isinstance(v, int) and not isinstance(v, bool) and rng.random() < 0.5:
        return rng.choice([v + 1, v - 1, -v, v * 1024, v // 7, True])
    return rng.choice(kinds)


def random_json(rng, depth=0):
    r = rng.random()
    if depth > 3 or r < 0.3:
        return rng.choice([None, True, 0, -7, 3.25, "x", "ada", "/dev/vdz", 10**30])
    if r < 0.6:
        return [random_json(rng, depth + 1) for _ in range(rng.randrange(4))]
    return {rng.choice(["mode", "disk", "user", "x", "use", "create"]): random_json(rng, depth + 1) for _ in range(rng.randrange(5))}


def fuzz_plans(rng, n):
    bases = [plan(), plan(encrypt=True, encrypt_password=DISK_PW), manual_plan(), plan(browsers=["firefox"], default_browser="firefox", extras=["monitor"]),
             manual_plan(new_table=True, create=[{"id": "e", "start": MIB, "size": 512 * MIB, "kind": "efi"}, {"id": "r", "start": 513 * MIB, "size": 100 * GB, "kind": "linux"}],
                         use=[{"part": "new:e", "mount": "/boot/efi", "format": "vfat"}, {"part": "new:r", "mount": "/", "format": "ext4"}])]
    out = []
    for i in range(n):
        r = rng.random()
        if r < 0.75:
            p = copy.deepcopy(rng.choice(bases))
            for _ in range(rng.randrange(1, 4)):
                p = mutate(rng, p)
            data = json.dumps(p, ensure_ascii=rng.random() < 0.5).encode("utf-8", "surrogatepass")
        elif r < 0.9:
            data = json.dumps(random_json(rng)).encode()
        else:
            base = bytearray(json.dumps(rng.choice(bases)).encode())
            for _ in range(rng.randrange(1, 4)):
                base[rng.randrange(len(base))] = rng.randrange(256)
            data = bytes(base)
        out.append(data)
    return out


def fuzz_installs(rng, n):
    """Random plans that pass the checks: the window's kinds of choices in random combinations."""
    out = []
    for _ in range(n):
        p = plan(encrypt=rng.random() < 0.5, encrypt_password=DISK_PW, locale=rng.choice(["en_US.UTF-8", "de_DE.UTF-8", "C", "fr_FR"]),
                 keymap=rng.choice(["us", "gb", "fr", "de"]), keymap_variant=rng.choice(["", "", "dvorak", "nodeadkeys"]),
                 timezone=rng.choice(["UTC", "Europe/Lisbon", "Asia/Tokyo", "Pacific/Auckland"]))
        if rng.random() < 0.6:
            chosen = rng.sample(["vivaldi", "firefox", "librewolf", "opera-gx"], rng.randrange(5))
            p["browsers"] = chosen
            p["default_browser"] = rng.choice(chosen) if chosen and rng.random() < 0.7 else ""
        if rng.random() < 0.6:
            p["extras"] = rng.sample(["assistant", "monitor", "mice"], rng.randrange(4))
        if rng.random() < 0.5:
            fs = rng.choice(["btrfs", "ext4", "xfs"])
            p.update(mode="manual", new_table=False, delete=rng.choice([[], [3]]))
            start = 317 * MIB if p["delete"] else 300 * GB
            p["create"] = [{"id": "r", "start": start, "size": rng.choice([30, 60, 150]) * GB, "kind": "linux"}]
            p["use"] = [{"part": "/dev/vdz1", "mount": "/boot/efi", "format": rng.choice([None, "vfat"])}, {"part": "new:r", "mount": "/", "format": fs}]
        out.append(p)
    return out


def main():
    global ARGS, PYTHON, BIN, SCRATCH
    ap = argparse.ArgumentParser()
    ap.add_argument("--python")
    ap.add_argument("--bin")
    ap.add_argument("--only")
    ap.add_argument("--fuzz", type=int, default=0)
    ap.add_argument("--fuzz-install", type=int, default=0)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--corpus", help="a folder of plans (a fuzzer's corpus): each is run as an install")
    ap.add_argument("-j", type=int, default=os.cpu_count() or 4)
    ap.add_argument("-v", action="store_true")
    ap.add_argument("--show", action="store_true", help="print the script's output and calls for each case")
    ARGS = ap.parse_args()
    helpers = os.path.normpath(os.path.join(HERE, "..", "..", "installer", "helpers"))
    PYTHON = ARGS.python or next(p for p in (os.path.join(helpers, "python", "install-system"), os.path.join(helpers, "install-system")) if os.path.exists(p))
    BIN = ARGS.bin or os.path.normpath(os.path.join(HERE, "..", "..", "..", "target", "release", "yggdrasil-install-system"))
    SCRATCH = tempfile.mkdtemp(prefix="install-system-parity-")
    # what the program counts on, with this computer's own mount(8), on scratch tmpfs folders in a throwaway namespace
    bc = subprocess.run(["unshare", "-r", "-m", "--propagation", "private", "sh", os.path.join(HERE, "bind-check.sh"), os.path.join(SCRATCH, "bind-check")],
                        capture_output=True, text=True, env={"PATH": "/usr/bin:/bin"})
    left = bc.stdout.split()
    if bc.returncode != 0 or len(left) != 3 or left[1:] != ["2", "2"]:
        raise SystemExit(f"compare: mount(8) does not keep the live /dev's submounts as the program counts on: {bc.stdout!r} {bc.stderr[-300:]!r}")
    print(f"mount(8): the live submounts left after umount -R of the new system: {left[0]} of 2 with a plain --rbind (the hazard), "
          f"2 with --rbind --make-rslave, 2 after the cleanup's mount --make-rslave", flush=True)
    jobs = []
    cases_disks(jobs)
    cases_check(jobs)
    cases_install(jobs)
    rng = random.Random(ARGS.seed)
    for i, data in enumerate(fuzz_plans(rng, ARGS.fuzz)):
        jobs.append(("fuzz", f"#{i} {data[:150]!r}", world(state={"disks": [windows_disk()]} if i % 2 else {}), [], data))
    for i, p in enumerate(fuzz_installs(rng, ARGS.fuzz_install)):
        jobs.append(("fuzz-install", f"#{i} {json.dumps(p)[:200]}", world(state={"disks": [windows_disk()]}), [], json.dumps(p).encode()))
    if ARGS.corpus:
        for i, name in enumerate(sorted(os.listdir(ARGS.corpus))):
            with open(os.path.join(ARGS.corpus, name), "rb") as f:
                data = f.read()
            jobs.append(("corpus", f"{name} {data[:150]!r}", world(state={"disks": [windows_disk()]} if i % 2 else {}), [], data))
    try:
        with ThreadPoolExecutor(ARGS.j) as ex:
            futures = [ex.submit(compare, j[0], j[1], j[2], j[3], j[4], j[5] if len(j) > 5 else None) for j in jobs]
            for f in futures:
                f.result()
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)
    for g in COUNTS:
        bad = sum(1 for f in FAILS if f[0] == g)
        print(f"{g}: {COUNTS[g] - bad}/{COUNTS[g]} the same")
    sys.exit(1 if FAILS else 0)


if __name__ == "__main__":
    main()
