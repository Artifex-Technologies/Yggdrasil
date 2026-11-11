#!/usr/bin/python3 -I
"""fake.py: every program the install program runs, as a stand-in (compare.py links it under each name: lsblk, findmnt,
sfdisk, wipefs, cryptsetup, mkfs.*, mkswap, mount, umount, btrfs, blkid, udevadm, unsquashfs, chroot). Nothing real is
done to any disk: each call is written down in $WORLD/calls.jsonl (its arguments, and what it read on standard input for the
programs that are given one) and answered from a made-up computer in $WORLD/state.json, which the calls change as the real
programs would change the computer (sfdisk's partitions, mkfs's file systems, pacman's packages ...). unsquashfs writes a
made-up system into the target folder (inside the test's own tmpfs). A rule in state["fail"] makes a call fail."""
import hashlib
import json
import os
import sys

W = os.environ["WORLD"]
STATE = os.path.join(W, "state.json")
GIB = 1024**3
MIB = 1024**2
ESP_GUID = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B"
LINUX_GUID = "0FC63DAF-8483-4772-8E79-3D69D8477DE4"
SWAP_GUID = "0657FD6D-A4AB-43C4-84E5-0933C84B4F4F"
TYPES = {"uefi": ESP_GUID, "linux": LINUX_GUID, "swap": SWAP_GUID}


def load():
    with open(STATE) as f:
        return json.load(f)


def save(state):
    with open(STATE + ".new", "w") as f:
        json.dump(state, f, indent=1, sort_keys=True)
    os.replace(STATE + ".new", STATE)


def log(argv, stdin):
    with open(os.path.join(W, "calls.jsonl"), "a") as f:
        f.write(json.dumps({"argv": argv, "stdin": stdin}) + "\n")


def part_name(disk, n):
    return f"{disk}p{n}" if disk[-1].isdigit() else f"{disk}{n}"


def disk_of(state, path):
    return next((d for d in state["disks"] if d["path"] == path), None)


def uuid_for(dev, state):
    gen = state.get("fs", {}).get(dev, {}).get("gen", 0)
    h = hashlib.sha256(f"{dev}#{gen}".encode()).hexdigest()
    return f"{h[:8]}-{h[8:12]}-{h[12:16]}-{h[16:20]}-{h[20:32]}"


def fail_rule(state, argv):
    for rule in state.get("fail", []):
        m = rule["argv"]
        if argv[:len(m)] == m and rule.get("times", 1) != 0:
            if "times" in rule:
                rule["times"] -= 1
            return rule
    return None


# ---------------------------------------------------------------- lsblk, findmnt

def lsblk(state, args):
    if args[:4] == ["-J", "-b", "-d", "-o"]:
        out = []
        for d in state["disks"] + state.get("other_devices", []):
            out.append({k: d.get(k) for k in ("name", "path", "size", "model", "tran", "ro", "type", "rm")})
        return 0, json.dumps({"blockdevices": out}, indent=3) + "\n", ""
    if args[:3] == ["-J", "-b", "-o"]:
        d = disk_of(state, args[4])
        if d is None:
            return 32, "", f"lsblk: {args[4]}: not a block device\n"
        children = []
        for q in sorted(d.get("parts", []), key=lambda q: q["n"]):
            fs = state.get("fs", {}).get(q["path"], {})
            c = {"path": q["path"], "start": q["start"], "size": q["sectors"] * 512, "fstype": fs.get("type", q.get("fstype")),
                 "label": fs.get("label", q.get("label")), "partlabel": q.get("partlabel"), "parttype": q.get("parttype", "").lower() or None,
                 "partn": q["n"], "pttype": d.get("table") or None, "mountpoints": q.get("mountpoints", [None]), "type": "part"}
            if q.get("children"):
                c["children"] = q["children"]
            children.append(c)
        top = {"path": d["path"], "start": None, "size": d["size"], "fstype": None, "label": None, "partlabel": None, "parttype": None,
               "partn": None, "pttype": d.get("table") or None, "mountpoints": d.get("mountpoints", [None]), "type": "disk"}
        if children:
            top["children"] = children
        return 0, json.dumps({"blockdevices": [top]}, indent=3) + "\n", ""
    if args[:3] == ["-nr", "-o", "MOUNTPOINTS"]:
        d = disk_of(state, args[3])
        if d is None:
            return 32, "", f"lsblk: {args[3]}: not a block device\n"
        lines = [" ".join(m for m in d.get("mountpoints", [None]) if m)]
        lines += [" ".join(m for m in q.get("mountpoints", [None]) if m) for q in d.get("parts", [])]
        return 0, "\n".join(lines) + "\n", ""
    if args[:2] == ["-no", "PKNAME"]:
        live = state.get("live") or {}
        return (0, live.get("pkname", "") + "\n", "") if args[2] == live.get("source") else (0, "\n", "")
    return 1, "", "lsblk: unexpected arguments\n"


def findmnt(state, args):
    live = state.get("live")
    if args == ["-no", "SOURCE", "/run/archiso/bootmnt"] and live:
        return 0, live["source"] + "\n", ""
    return 1, "", ""


# ---------------------------------------------------------------- the disk tools

def sfdisk(state, args, stdin):
    disk = next((a for a in args if a.startswith("/dev/")), None)
    d = disk_of(state, disk)
    if d is None:
        return 1, "", f"sfdisk: cannot open {disk}: No such file or directory\n"
    parts = d.setdefault("parts", [])
    if args[:1] == ["-J"]:
        if not d.get("table"):
            return 1, "", f"sfdisk: {disk}: does not contain a recognized partition table\n"
        table = {"label": d["table"], "id": "TEST-TABLE-0001", "device": disk, "unit": "sectors", "firstlba": 2048,
                 "lastlba": d["size"] // 512 - 34, "sectorsize": d.get("sectorsize", 512),
                 "partitions": [{"node": q["path"], "start": q["start"], "size": q["sectors"], "type": q.get("parttype", LINUX_GUID)}
                                for q in sorted(parts, key=lambda q: q["n"])]}
        if d.get("sfdisk_extra"):
            table.update(d["sfdisk_extra"])
        return 0, json.dumps({"partitiontable": table}, indent=3) + "\n", ""
    if args[:1] == ["--delete"]:
        n = int(args[2])
        if not any(q["n"] == n for q in parts):
            return 1, "", f"sfdisk: --delete: partition {n} does not exist\n"
        d["parts"] = [q for q in parts if q["n"] != n]
        return 0, "", ""
    if args[:1] == ["--part-type"]:
        for q in parts:
            if q["n"] == int(args[2]):
                q["parttype"] = TYPES.get(args[3], args[3])
        return 0, "", ""
    ss = d.get("sectorsize", 512)
    if args[:1] == ["--append"]:
        spec = dict(x.strip().split("=", 1) for x in stdin.strip().split(","))
        n = 1
        while any(q["n"] == n for q in parts):
            n += 1
        parts.append({"n": n, "path": part_name(disk, n), "start": int(spec["start"]), "sectors": int(spec["size"]),
                      "parttype": TYPES[spec["type"]]})
        return 0, "", ""
    if args[:2] == ["--wipe", "always"]:
        d["table"] = "gpt"
        d["parts"] = []
        if "--wipe-partitions" in args:
            first = 2048
            esp = GIB // ss
            d["parts"] = [{"n": 1, "path": part_name(disk, 1), "start": first, "sectors": esp, "parttype": ESP_GUID, "partlabel": "EFI"},
                          {"n": 2, "path": part_name(disk, 2), "start": first + esp, "sectors": d["size"] // ss - first - esp - 34,
                           "parttype": LINUX_GUID, "partlabel": stdin.split("name=")[-1].strip()}]
        return 0, "", ""
    return 1, "", "sfdisk: unexpected arguments\n"


def wipefs(state, args):
    d = disk_of(state, args[-1])
    if d is None:
        return 1, "", f"wipefs: error: {args[-1]}: probing initialization failed: No such file or directory\n"
    d["table"] = ""
    d["parts"] = []
    return 0, "", ""


def known_device(state, dev):
    if dev.startswith("/dev/mapper/"):
        return dev[len("/dev/mapper/"):] in state.get("open", {})
    return any(q["path"] == dev for d in state["disks"] for q in d.get("parts", []))


def mkfs(state, kind, args):
    dev, label = args[-1], args[-2]
    if not known_device(state, dev):
        return 1, "", f"{kind}: cannot open {dev}: No such file or directory\n"
    fs = state.setdefault("fs", {})
    gen = fs.get(dev, {}).get("gen", 0) + 1
    fs[dev] = {"type": kind, "label": label, "gen": gen}
    return 0, f"made {kind} on {dev}\n", ""


def cryptsetup(state, args, stdin):
    if args[:1] == ["luksFormat"]:
        dev = args[-1]
        if not known_device(state, dev):
            return 1, "", f"Device {dev} does not exist or access denied.\n"
        fs = state.setdefault("fs", {})
        fs[dev] = {"type": "crypto_LUKS", "gen": fs.get(dev, {}).get("gen", 0) + 1}
        return 0, "", ""
    if args[:1] == ["open"]:
        state.setdefault("open", {})[args[-1]] = args[-2]
        return 0, "", ""
    if args[:1] == ["close"]:
        if args[1] in state.get("open", {}):
            del state["open"][args[1]]
            return 0, "", ""
        return 4, "", f"Device {args[1]} is not active.\n"
    if args[:1] == ["luksAddKey"]:
        return (0, "", "") if os.path.exists(args[-1]) else (1, "", f"Failed to open key file {args[-1]}.\n")
    return 1, "", "cryptsetup: unexpected arguments\n"


def blkid(state, args):
    dev = args[-1]
    fs = state.get("fs", {}).get(dev)
    if fs is None and not dev.startswith("/dev/mapper/"):
        q = next((q for d in state["disks"] for q in d.get("parts", []) if q["path"] == dev), None)
        if q is not None and q.get("fstype"):
            fs = {"type": q["fstype"]}
    if fs is None:
        return 2, "", ""
    if args[:2] == ["-s", "TYPE"]:
        return 0, fs["type"] + "\n", ""
    return 0, uuid_for(dev, state) + "\n", ""


# ---------------------------------------------------------------- the system copied

def make_tree(target, medium):
    for d in medium.get("dirs", []):
        os.makedirs(os.path.join(target, d), exist_ok=True)
    for path, text in medium.get("files", {}).items():
        p = os.path.join(target, path)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "w") as f:
            f.write(text)
    for path, dest in medium.get("links", {}).items():
        p = os.path.join(target, path)
        os.makedirs(os.path.dirname(p), exist_ok=True)
        if os.path.lexists(p):
            os.remove(p)
        os.symlink(dest, p)


def unsquashfs(state, args):
    target = args[args.index("-d") + 1]
    u = state.get("unsquashfs", {})
    make_tree(target, state.get("medium", {}))
    out = "".join(f"{p}\n" for p in u.get("percent", [0, 4, 9, 10, 17, 20, 35, 41, 50, 63, 70, 79, 80, 90, 99, 100]))
    return u.get("code", 0), u.get("stdout", out), u.get("stderr", "")


# ---------------------------------------------------------------- inside the new system (chroot)

def inside(state, target, argv, stdin):
    prog = argv[0]
    rest = argv[1:]
    if prog.startswith("/"):
        p = state.get("programs", {}).get(prog)
        if p is None:
            return 127, "", f"/usr/bin/env: '{prog}': No such file or directory\n"
        state.setdefault("ran", []).append(prog)
        return p.get("code", 0), p.get("stdout", ""), p.get("stderr", "")
    installed = state.setdefault("installed", [])
    if prog == "pacman":
        targets = [x for x in rest if not x.startswith("-")]
        if rest[:1] == ["-Q"]:
            return (0, f"{targets[0]} 1.0-1\n", "") if targets[0] in installed else (1, "", f"error: package '{targets[0]}' was not found\n")
        if rest[:1] == ["-Rns"]:
            missing = [t for t in targets if t not in installed]
            if missing:
                return 1, "", f"error: target not found: {missing[0]}\n"
            staying = [x for x in installed if x not in targets]
            needs = state.get("needs", {})
            if any(t in needs.get(s, []) for s in staying for t in targets):
                return 1, "", "error: failed to prepare transaction (could not satisfy dependencies)\n"
            state["installed"] = staying
            return 0, "", ""
        if rest[:1] in (["-S"], ["-Syu"]):
            ok = state.get("sync_ok" if rest[0] == "-S" else "syu_ok", [])
            if any(t not in ok for t in targets):
                return 1, "", "error: failed retrieving file 'x.pkg.tar.zst' from mirror : Could not resolve host\n"
            for t in targets:
                if t not in installed:
                    installed.append(t)
                desktop = state.get("desktop_of", {}).get(t)
                if desktop:
                    make_tree(target, {"files": {f"usr/share/applications/{desktop}": "[Desktop Entry]\n"}})
            return 0, "", ""
    if prog == "flatpak":
        if rest[:1] == ["install"]:
            app = rest[-1]
            if app not in state.get("flathub_ok", []):
                return 1, "", f"error: Unable to resolve {app}\n"
            make_tree(target, {"files": {f"var/lib/flatpak/exports/share/applications/{app}.desktop": "[Desktop Entry]\n"}})
        return 0, "", ""
    if prog == "getent":
        return (0, f"{rest[1]}:x:100:\n", "") if rest[1] in state.get("groups", []) else (2, "", "")
    if prog in ("useradd", "chpasswd", "passwd", "systemctl", "locale-gen", "hwclock", "mkinitcpio", "grub-install", "grub-mkconfig",
                "usermod", "chown"):
        return 0, "", ""
    return 127, "", f"/usr/bin/env: '{prog}': No such file or directory\n"


def main():
    name = os.path.basename(sys.argv[0])
    argv = [name] + sys.argv[1:]
    state = load()
    reads = (name == "sfdisk" and any(a in argv for a in ("--wipe", "--append"))) or (name == "cryptsetup" and argv[1:2] in (["luksFormat"], ["open"], ["luksAddKey"])) \
        or (name == "chroot" and argv[4:5] == ["chpasswd"])
    stdin = sys.stdin.read() if reads else None
    log(argv, stdin)
    rule = fail_rule(state, argv[4:] if name == "chroot" else argv)
    if rule:
        code, out, err = rule.get("code", 1), rule.get("stdout", ""), rule.get("stderr", f"{name}: failed (made up)\n")
    elif name == "lsblk":
        code, out, err = lsblk(state, argv[1:])
    elif name == "findmnt":
        code, out, err = findmnt(state, argv[1:])
    elif name == "sfdisk":
        code, out, err = sfdisk(state, argv[1:], stdin)
    elif name == "wipefs":
        code, out, err = wipefs(state, argv[1:])
    elif name.startswith("mkfs.") or name == "mkswap":
        code, out, err = mkfs(state, "swap" if name == "mkswap" else name[5:], argv[1:])
    elif name == "cryptsetup":
        code, out, err = cryptsetup(state, argv[1:], stdin)
    elif name == "blkid":
        code, out, err = blkid(state, argv[1:])
    elif name == "unsquashfs":
        code, out, err = unsquashfs(state, argv[1:])
    elif name == "chroot":
        code, out, err = inside(state, argv[1], argv[4:], stdin)
    elif name in ("mount", "umount", "btrfs", "udevadm"):
        code, out, err = 0, "", ""
    else:
        code, out, err = 127, "", f"{name}: not one of the stand-ins\n"
    save(state)
    sys.stdout.write(out)
    sys.stderr.write(err)
    sys.exit(code)


if __name__ == "__main__":
    main()
