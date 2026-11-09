#!/usr/bin/env python3
"""record: a read-only sample of this computer as a fake tree for --dry-run --root (and py_dryrun.py): the fan controller's
and the processor's hwmon readings, the graphics card through NVML get calls only (ROOT/nvml.json), the helpers' settings and
state files, the power limits, turbo and boot id. Nothing is written outside OUT, nothing is set on any device.

The MSRs need root to read, so the sample's /dev/cpu/N/msr files are made from what the tune service last published
(/run/yggdrasil/tune.json); without it they are left out.

    record.py OUT            (OUT must not exist yet; it holds this computer's own values: keep it out of the repository)
"""
import ctypes
import glob
import json
import os
import shutil
import struct
import sys


def copy(src, root):
    """A readable file into the tree at the same path (unreadable ones are left out, as they would be for the user)."""
    try:
        with open(src, "rb") as f:
            data = f.read()
    except OSError:
        return False
    dst = os.path.join(root, src.lstrip("/"))
    os.makedirs(os.path.dirname(dst), exist_ok=True)
    with open(dst, "wb") as f:
        f.write(data)
    return True


def nvml():
    """The first card through NVML get calls only."""
    lib = ctypes.CDLL("libnvidia-ml.so.1")
    if lib.nvmlInit_v2() != 0:
        raise OSError("nvmlInit_v2 failed")
    try:
        h = ctypes.c_void_p()
        if lib.nvmlDeviceGetHandleByIndex_v2(ctypes.c_uint(0), ctypes.byref(h)) != 0:
            raise OSError("no card")
        u = lambda fn, *a: (lambda v: v.value if fn(h, *a, ctypes.byref(v)) == 0 else None)(ctypes.c_uint())
        out = {"temp": u(lib.nvmlDeviceGetTemperature, ctypes.c_uint(0))}
        n = u(lib.nvmlDeviceGetNumFans) or 0
        out["fans"] = [u(lib.nvmlDeviceGetFanSpeed_v2, ctypes.c_uint(i)) for i in range(n)]
        lo, hi = ctypes.c_uint(), ctypes.c_uint()
        if lib.nvmlDeviceGetMinMaxFanSpeed(h, ctypes.byref(lo), ctypes.byref(hi)) == 0:
            out["min"], out["max"] = lo.value, hi.value

        class ClockOffset(ctypes.Structure):
            _fields_ = [("version", ctypes.c_uint), ("type", ctypes.c_uint), ("pstate", ctypes.c_uint),
                        ("clockOffsetMHz", ctypes.c_int), ("minClockOffsetMHz", ctypes.c_int), ("maxClockOffsetMHz", ctypes.c_int)]
        for key, kind in (("core", 0), ("mem", 2)):
            o = ClockOffset(version=ctypes.sizeof(ClockOffset) | (1 << 24), type=kind, pstate=0)
            if lib.nvmlDeviceGetClockOffsets(h, ctypes.byref(o)) == 0:
                out[key] = o.clockOffsetMHz
        out["max_core"] = u(lib.nvmlDeviceGetMaxClockInfo, ctypes.c_uint(0))
        return {k: v for k, v in out.items() if v is not None}
    finally:
        lib.nvmlShutdown()


def msr_file(path, ratios, base, programmable):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.truncate(0x1AD + 8)
        f.seek(0xCE)
        f.write(struct.pack("<Q", (base << 8) | (int(programmable) << 28)))
        f.seek(0x1AD)
        f.write(struct.pack("<Q", sum((r & 0xFF) << (8 * i) for i, r in enumerate(ratios))))


def main(argv):
    if len(argv) != 2 or os.path.exists(argv[1]):
        print(__doc__, file=sys.stderr)
        return 2
    root = argv[1]
    os.makedirs(root)
    for d in sorted(glob.glob("/sys/class/hwmon/hwmon*")):
        try:
            with open(os.path.join(d, "name")) as f:
                name = f.read().strip()
        except OSError:
            continue
        if name not in ("corsaircpro", "coretemp", "k10temp", "zenpower"):
            continue
        for p in ["name"] + [os.path.basename(x) for x in glob.glob(os.path.join(d, "*"))
                             if os.path.basename(x).split("_")[0].rstrip("0123456789") in ("fan", "pwm", "temp")]:
            copy(os.path.join(d, p), root)
    for p in ["/etc/yggdrasil/case-fans.json", "/etc/yggdrasil/gpu-fan.json", "/etc/yggdrasil/tuning.json",
              "/var/lib/yggdrasil/tune/status.json", "/var/lib/yggdrasil/tune/stock.json", "/var/lib/yggdrasil/tune/tuned",
              "/run/yggdrasil/tune.json", "/proc/sys/kernel/random/boot_id", "/sys/devices/system/cpu/intel_pstate/no_turbo"]:
        copy(p, root)
    for p in glob.glob("/sys/class/powercap/intel-rapl:0/*"):
        if os.path.basename(p) in ("constraint_0_power_limit_uw", "constraint_1_power_limit_uw", "max_energy_range_uj", "energy_uj"):
            copy(p, root)
    try:
        with open(os.path.join(root, "nvml.json"), "w") as f:
            json.dump(nvml(), f)
    except OSError as e:
        print(f"record: no graphics card sample ({e})", file=sys.stderr)
    try:
        with open("/run/yggdrasil/tune.json") as f:
            cpu = json.load(f)["cpu"]
        for c in sorted(glob.glob("/dev/cpu/[0-9]*")):
            msr_file(os.path.join(root, c.lstrip("/"), "msr"), cpu["ratios"], cpu["base"], cpu["programmable"])
    except (OSError, ValueError, KeyError, TypeError) as e:
        print(f"record: no processor sample ({e})", file=sys.stderr)
    os.makedirs(os.path.join(root, "dev"), exist_ok=True)
    open(os.path.join(root, "dev", "kmsg"), "w").close()
    print(f"recorded into {root}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
