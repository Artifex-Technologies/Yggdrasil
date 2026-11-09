#!/usr/bin/env python3
"""compare: the Rust helpers and the Python ones they replace, on the same inputs, in dry runs (nothing is written to any
device or system file). For each case both get the same fake tree; their output (stdout, stderr, exit code) must be the same
line for line, apart from what differs on purpose:

  - the time a file was written ("time" in /run/yggdrasil/tune.json) and the minute in "since";
  - the Rust `tune watch` logs where it reads the kernel's messages from (new; the Python one always ran journalctl);
  - the help text: the Rust one adds a paragraph on --dry-run after the Python text.

Cases: the Python tests' fixtures as trees, more cases around them, any recorded samples of a real computer (record.py), and
casefan's loop over many temperature sequences (the test sequences and seeded random walks).

Then the merged service (`power run`, one process for the four loops): on the same trees, its dry run must print, part by
part, the same lines in the same order as each Rust helper run on its own followed by what its old unit's ExecStopPost ran
(`casefan reset` / `gpufan reset`), with the same exit codes. Each of its lines is "[part] ..." (merged_case).

    compare.py [--python DIR] [--bin DIR] [--sample ROOT]... [--seeds N] [-v]

--python: the folder with the Python tune, casefan, gpufan and powerlimit (default: each from ../../settings/helpers/python,
          else ../../settings/helpers)
--bin: the folder with the Rust binaries (default: ../../../target/release; build with cargo build --release --bins --examples)
"""
import argparse
import difflib
import importlib.machinery
import importlib.util
import io
import json
import os
import random
import re
import shutil
import struct
import subprocess
import sys
import tempfile
from contextlib import redirect_stderr

HERE = os.path.dirname(os.path.abspath(__file__))
STOCK = [50, 50, 48, 48, 47, 47, 47, 47]
HELPERS = os.path.normpath(os.path.join(HERE, "..", "..", "settings", "helpers"))


def py_helper(args, name):
    """The Python script of a helper: in --python, else where the package keeps it (helpers/python after a switch to Rust,
    helpers before)."""
    for d in ([args.python] if args.python else []) + [os.path.join(HELPERS, "python"), HELPERS]:
        if os.path.exists(os.path.join(d, name)):
            return os.path.join(d, name)
    raise SystemExit(f"compare: no Python {name} (give --python)")


# ---------------------------------------------------------------- fake trees


def files(root, d, entries):
    p = os.path.join(root, d.lstrip("/"))
    os.makedirs(p, exist_ok=True)
    for name, text in entries.items():
        with open(os.path.join(p, name), "w") as f:
            f.write(text if isinstance(text, str) else json.dumps(text))
    return p


def msr(root, cpus, ratios, base=36, programmable=True):
    for c in range(cpus):
        p = os.path.join(root, "dev", "cpu", str(c), "msr")
        os.makedirs(os.path.dirname(p), exist_ok=True)
        with open(p, "wb") as f:
            f.truncate(0x1AD + 8)
            f.seek(0xCE)
            f.write(struct.pack("<Q", (base << 8) | (int(programmable) << 28)))
            f.seek(0x1AD)
            f.write(struct.pack("<Q", sum((r & 0xFF) << (8 * i) for i, r in enumerate(ratios))))


def casefan_tree(root, nvml=None, cfg=None, cpro=True, pkg_temp="53000"):
    """test_casefan's Hardware fixture (Commander Pro ports 1 and 6, coretemp at 53 C) with a card and a setting."""
    if cpro:
        files(root, "/sys/class/hwmon/hwmon5", {"name": "corsaircpro", "fan1_label": "fan1 4pin", "fan1_input": "3846",
                                                "fan6_label": "fan6 4pin", "fan6_input": "1399", "pwm1": "", "pwm6": ""})
    files(root, "/sys/class/hwmon/hwmon3", {"name": "coretemp", "temp1_label": "Package id 0", "temp1_input": pkg_temp,
                                            "temp2_label": "Core 0", "temp2_input": "51000", "temp10_label": "Core 8", "temp10_input": "50000"})
    if nvml is not None:
        files(root, "/", {"nvml.json": nvml})
    if cfg is not None:
        files(root, "/etc/yggdrasil", {"case-fans.json": cfg})


def gpufan_tree(root, nvml=None, cfg=None):
    if nvml is not None:
        files(root, "/", {"nvml.json": nvml})
    if cfg is not None:
        files(root, "/etc/yggdrasil", {"gpu-fan.json": cfg})


def tune_tree(root, conf=None, status=None, tuned=None, kmsg="", programmable=True, boot="boot-1"):
    """test_tune's Base fixture: the processor's own multipliers (STOCK, base 36), its power limits 95/125 W, boot-1."""
    msr(root, 4, STOCK, programmable=programmable)
    files(root, "/sys/class/powercap/intel-rapl:0", {"constraint_0_power_limit_uw": "95000000\n", "constraint_1_power_limit_uw": "125000000\n",
                                                     "max_energy_range_uj": "262143328850\n", "energy_uj": "123456789\n"})
    files(root, "/sys/devices/system/cpu/intel_pstate", {"no_turbo": "0\n"})
    files(root, "/proc/sys/kernel/random", {"boot_id": boot + "\n"})
    files(root, "/var/lib/yggdrasil/tune", {"stock.json": {"ratios": STOCK, "pl1": 95, "pl2": 125}})
    if status is not None:
        files(root, "/var/lib/yggdrasil/tune", {"status.json": status})
    if tuned is not None:
        files(root, "/var/lib/yggdrasil/tune", {"tuned": tuned})
    if conf is not None:
        files(root, "/etc/yggdrasil", {"tuning.json": conf})
    files(root, "/", {"nvml.json": {"temp": 40, "fans": [30, 30], "min": 24, "max": 100, "core": 0, "mem": 0, "max_core": 2100}})
    files(root, "/dev", {"kmsg": kmsg})


RTX = "NVIDIA GeForce RTX 2080 Ti, 260.00, 260.00, 100.00, 330.00\n"
UPOWER = "busctl get-property org.freedesktop.UPower.PowerProfiles"
HADESS = "busctl get-property net.hadess.PowerProfiles"


def profile(mode):
    return f's "{mode}"\n'


def powerlimit_tree(root, mode="balanced", limits=None, old=None, hold=None, smi=RTX, policies=4, cmds=None):
    """Four processor policies (0.8-4.7 GHz), the power mode and the card's limits through commands.json, the limits file
    (the new place, the old one, or neither) and the graphics card unlock's power hold."""
    for i in range(policies):
        files(root, f"/sys/devices/system/cpu/cpufreq/policy{i}",
              {"cpuinfo_min_freq": "800000\n", "cpuinfo_max_freq": "4700000\n" if i < 2 else "4600000\n", "scaling_max_freq": "4700000\n"})
    answers = {UPOWER: [profile(m) for m in mode] if isinstance(mode, list) else profile(mode), "nvidia-smi --query-gpu": smi}
    files(root, "/", {"commands.json": {**answers, **(cmds or {})}})
    if limits is not None:
        files(root, "/etc/yggdrasil", {"power-limits.json": limits})
    if old is not None:
        files(root, "/etc/labyrinth", {"power-limits.json": old})
    if hold is not None:
        files(root, "/var/lib/yggdrasil/gpu-unlock", {"power-hold": hold})


PUMP_AND_FAN = {"ports": {"1": {"name": "pump", "role": "pump", "fixed": 100}, "6": {"role": "fan"}}}
CURVE = {"mode": "curve", "fixed": 60, "curve": [[40, 30], [60, 45], [75, 75], [85, 100]]}
CARD = {"fans": [30, 30], "min": 24, "max": 100}


def cases(work, samples):
    """(name, helper, tree builder or sample root, args, steps, stdin)"""
    c = []
    cf = lambda **k: (lambda r: casefan_tree(r, **k))
    gf = lambda **k: (lambda r: gpufan_tree(r, **k))
    tn = lambda **k: (lambda r: tune_tree(r, **k))
    base = cf(nvml={"temp": 35, **CARD}, cfg=PUMP_AND_FAN)
    c += [
        ("casefan get", "casefan", base, ["get"], None, None),
        ("casefan run, idle", "casefan", base, ["run"], 20, None),
        ("casefan run, 40 rounds (a reapply at 30 s)", "casefan", base, ["run"], 40, None),
        ("casefan run, the card hot", "casefan", cf(nvml={"temp": 90, **CARD}, cfg=PUMP_AND_FAN), ["run"], 5, None),
        ("casefan run, the processor hot", "casefan", cf(nvml={"temp": 35, **CARD}, cfg=PUMP_AND_FAN, pkg_temp="95000"), ["run"], 5, None),
        ("casefan run, the card's readings run out", "casefan", cf(nvml={"temp": [35, 50, 70], **CARD}, cfg=PUMP_AND_FAN), ["run"], 10, None),
        ("casefan run, no processor temperature", "casefan", cf(nvml={"temp": 35, **CARD}, cfg=PUMP_AND_FAN, pkg_temp="hot"), ["run"], 5, None),
        ("casefan run, no NVIDIA driver", "casefan", cf(cfg=PUMP_AND_FAN), ["run"], 5, None),
        ("casefan run, no Commander Pro", "casefan", cf(nvml={"temp": 35, **CARD}, cfg=PUMP_AND_FAN, cpro=False), ["run"], 5, None),
        ("casefan run, no setting", "casefan", cf(nvml={"temp": 35, **CARD}), ["run"], 5, None),
        ("casefan run, a pump below its floor and a follower", "casefan",
         cf(nvml={"temp": [60, 61, 62, 63, 70, 75, 60, 55], **CARD},
            cfg={"ports": {"1": {"role": "pump", "fixed": 10}, "5": {"role": "fan", "cpu": [[55, 35], [70, 40], [85, 80], [92, 100]],
                                                                       "gpu": [[55, 35], [70, 40], [85, 80], [92, 100]]},
                           "6": {"role": "fan", "follow": "5"}}}), ["run"], 20, None),
        ("casefan reset", "casefan", base, ["reset"], None, None),
        ("casefan reset failed", "casefan", base, ["reset", "failed"], None, None),
        ("casefan reset, no setting", "casefan", cf(nvml={"temp": 35, **CARD}), ["reset"], None, None),
        ("casefan set - (Settings' stdin)", "casefan", base, ["set", "-"], None,
         json.dumps({"ports": {"1": {"role": "pump", "fixed": 10}, "6": {"role": "fan"}, "x": {}}})),
        ("casefan set -, bad JSON", "casefan", base, ["set", "-"], None, '{"ports": {"1": }'),
        ("casefan set FILE missing", "casefan", base, ["set", "/nonexistent/case-fans.json"], None, None),
        ("casefan get, no card and no controller", "casefan", cf(cfg=PUMP_AND_FAN, cpro=False), ["get"], None, None),
        ("casefan unknown command", "casefan", base, ["spin"], None, None),
        ("casefan reset with too many words", "casefan", base, ["reset", "a", "b"], None, None),
    ]
    g = gf(nvml={"temp": [40, 60, 75, 73, 71, 70], **CARD}, cfg=CURVE)
    c += [
        ("gpufan get", "gpufan", gf(nvml={"temp": 55, **CARD}, cfg=CURVE), ["get"], None, None),
        ("gpufan get, min/max cannot be read", "gpufan", gf(nvml={"temp": 55, **CARD, "fail": {"nvmlDeviceGetMinMaxFanSpeed": 3}}, cfg=CURVE), ["get"], None, None),
        ("gpufan get, no driver", "gpufan", gf(cfg=CURVE), ["get"], None, None),
        ("gpufan run, the test's curve", "gpufan", g, ["run"], 6, None),
        ("gpufan run, until the readings run out", "gpufan", g, ["run"], 20, None),
        ("gpufan run, hot over a low fixed speed", "gpufan", gf(nvml={"temp": [70, 86], **CARD}, cfg={"mode": "fixed", "fixed": 30}), ["run"], 2, None),
        ("gpufan run, auto ends at once", "gpufan", gf(nvml={"temp": 50, **CARD}, cfg={"mode": "auto"}), ["run"], 5, None),
        ("gpufan run, 40 rounds (reapply every 30 s)", "gpufan", gf(nvml={"temp": 50, **CARD}, cfg=CURVE), ["run"], 40, None),
        ("gpufan run, init fails", "gpufan", gf(nvml={"temp": 50, **CARD, "fail": {"nvmlInit_v2": 9}}, cfg=CURVE), ["run"], 5, None),
        ("gpufan run, setting a speed fails", "gpufan", gf(nvml={"temp": 50, **CARD, "fail": {"nvmlDeviceSetFanSpeed_v2": 3}}, cfg=CURVE), ["run"], 5, None),
        ("gpufan set auto", "gpufan", g, ["set", "auto"], None, None),
        ("gpufan set fixed=50", "gpufan", g, ["set", "fixed=50"], None, None),
        ("gpufan set fixed=101", "gpufan", g, ["set", "fixed=101"], None, None),
        ("gpufan set curve", "gpufan", g, ["set", "curve=40:30, 60:45,85:100"], None, None),
        ("gpufan set a bad curve", "gpufan", g, ["set", "curve=40-30,60:45"], None, None),
        ("gpufan set nonsense", "gpufan", g, ["set", "turbo"], None, None),
        ("gpufan reset", "gpufan", g, ["reset"], None, None),
        ("gpufan reset, no driver", "gpufan", gf(cfg=CURVE), ["reset"], None, None),
    ]
    trial_cpu = {"trial": {"cpu": {"ratios": [51, 51, 50, 50, 49, 49, 49, 49]}}}
    trial_gpu = {"trial": {"gpu": {"core": 150, "mem": 900, "cap": None}}}
    xid = "usb 1-3: new device\nNVRM: Xid (PCI:0000:01:00): 13, pid=4242, Graphics Exception\n"
    c += [
        ("tune get", "tune", tn(), ["get"], None, None),
        ("tune get, kept values out of range", "tune", tn(conf={"cpu": {"ratios": [70] * 8}, "gpu": {"core": 900}}), ["get"], None, None),
        ("tune try cpu", "tune", tn(), ["try", "cpu", "ratios=51,51,50,50,49,49,49,49"], None, None),
        ("tune try cpu, above the top", "tune", tn(), ["try", "cpu", "ratios=60,60,60,60,60,60,60,60"], None, None),
        ("tune try cpu, rising", "tune", tn(), ["try", "cpu", "ratios=50,51,50,50,49,49,49,49"], None, None),
        ("tune try cpu, not numbers", "tune", tn(), ["try", "cpu", "ratios=a,b"], None, None),
        ("tune try cpu, --5", "tune", tn(), ["try", "cpu", "ratios=--5,50"], None, None),
        ("tune try cpu, a locked processor", "tune", tn(programmable=False), ["try", "cpu", "ratios=50,50,50,50,50,50,50,50"], None, None),
        ("tune try gpu", "tune", tn(), ["try", "gpu", "core=120", "mem=600", "cap=1900"], None, None),
        ("tune try gpu, no cap", "tune", tn(), ["try", "gpu", "core=120", "cap=none"], None, None),
        ("tune try gpu, out of range", "tune", tn(), ["try", "gpu", "core=400"], None, None),
        ("tune try gpu, not a number", "tune", tn(), ["try", "gpu", "core=abc"], None, None),
        ("tune try gpu, not KEY=VALUE", "tune", tn(), ["try", "gpu", "core"], None, None),
        ("tune keep cpu", "tune", tn(status=trial_cpu), ["keep", "cpu"], None, None),
        ("tune keep gpu, nothing tried", "tune", tn(status=trial_cpu), ["keep", "gpu"], None, None),
        ("tune undo cpu", "tune", tn(status=trial_cpu, tuned={"boot_id": "boot-1", "since": "2026-11-09 07:00"}), ["undo", "cpu"], None, None),
        ("tune reset gpu", "tune", tn(conf={"gpu": {"core": 100, "mem": 0, "cap": None}}), ["reset", "gpu"], None, None),
        ("tune reenable", "tune", tn(conf={"cpu": {"ratios": [52] * 8}}, status={"cpu_off": "x", "gpu_off": "x", "trial": None}), ["reenable"], None, None),
        ("tune set power", "tune", tn(), ["set", "power", "pl1=120", "pl2=160"], None, None),
        ("tune set power, short below long", "tune", tn(), ["set", "power", "pl2=100"], None, None),
        ("tune set power, too high", "tune", tn(), ["set", "power", "pl1=500"], None, None),
        ("tune set power, defaults", "tune", tn(conf={"power": {"pl1": 120, "pl2": 160}}), ["set", "power", "pl1=default", "pl2=default"], None, None),
        ("tune set turbo off", "tune", tn(), ["set", "turbo", "off"], None, None),
        ("tune set turbo, a bad word", "tune", tn(), ["set", "turbo", "maybe"], None, None),
        ("tune set nothing known", "tune", tn(), ["set", "fans"], None, None),
        ("tune keep with no part", "tune", tn(), ["keep"], None, None),
        ("tune watch, nothing tuned", "tune", tn(), ["watch"], 3, None),
        ("tune watch, kept multipliers applied at start", "tune", tn(conf={"cpu": {"ratios": [52] * 8}, "power": {"pl1": 120}, "turbo": False}), ["watch"], 3, None),
        ("tune watch, the guard after a freeze", "tune",
         tn(conf={"cpu": {"ratios": [52] * 8}}, tuned={"boot_id": "boot-0", "since": "2026-11-08 22:10"}), ["watch"], 3, None),
        ("tune watch, an Xid with the clocks kept", "tune", tn(conf={"gpu": {"core": 150, "mem": 900, "cap": None}}, kmsg=xid), ["watch"], 3, None),
        ("tune watch, an Xid with nothing tuned", "tune", tn(kmsg=xid), ["watch"], 3, None),
        ("tune watch, stock read once per start", "tune", tn(), ["watch"], 1, None),
        ("tune on_kernel_line through a trial", "tune", tn(status=trial_gpu, kmsg=xid), ["watch"], 2, None),
        ("tune try gpu, the card refuses (undo, then put back fails too)", "tune",
         lambda r: (tune_tree(r), files(r, "/", {"nvml.json": {"temp": 40, "core": 0, "mem": 0, "max_core": 2100, "fail": {"nvmlDeviceSetClockOffsets": 3}}})),
         ["try", "gpu", "core=120"], None, None),
        ("tune watch, no card", "tune", lambda r: (tune_tree(r), os.remove(os.path.join(r, "nvml.json"))), ["watch"], 2, None),
    ]
    pl = lambda **k: (lambda r: powerlimit_tree(r, **k))
    LIMITS = {"balanced": {"cpu": 80, "gpu": 200}, "power-saver": {"cpu": 40, "gpu": 150}, "performance": {"gpu": 300}}
    c += [
        ("powerlimit get", "powerlimit", pl(limits=LIMITS), ["get"], None, None),
        ("powerlimit get, no card", "powerlimit", pl(limits=LIMITS, smi=None), ["get"], None, None),
        ("powerlimit get, the card cannot say ([N/A])", "powerlimit", pl(limits=LIMITS, smi="NVIDIA X, [N/A], [N/A], [N/A], [N/A]\n"), ["get"], None, None),
        ("powerlimit get, nvidia-smi fails", "powerlimit", pl(smi={"rc": 9, "out": "NVIDIA-SMI has failed\n"}), ["get"], None, None),
        ("powerlimit get, nvidia-smi too slow", "powerlimit", pl(smi={"timeout": True}), ["get"], None, None),
        ("powerlimit get, a hold at the default", "powerlimit", pl(limits=LIMITS, hold={"watts": None, "why": "armed"}), ["get"], None, None),
        ("powerlimit get, a heat hold", "powerlimit", pl(limits=LIMITS, hold={"watts": 200}), ["get"], None, None),
        ("powerlimit get, a broken hold", "powerlimit", pl(hold="not json"), ["get"], None, None),
        ("powerlimit get, the old place", "powerlimit", pl(old={"balanced": {"cpu": 50}}), ["get"], None, None),
        ("powerlimit get, unusable limits left out", "powerlimit",
         pl(limits={"balanced": {"cpu": 0, "gpu": 5000}, "turbo": {"cpu": 50}, "power-saver": {"cpu": True, "gpu": 10.5}, "performance": "x"}), ["get"], None, None),
        ("powerlimit get, an older power-profiles-daemon", "powerlimit", pl(cmds={UPOWER: None, HADESS: profile("power-saver")}), ["get"], None, None),
        ("powerlimit get, no busctl", "powerlimit", pl(cmds={UPOWER: None, HADESS: None}), ["get"], None, None),
        ("powerlimit get, no processor policies", "powerlimit", pl(policies=0), ["get"], None, None),
        ("powerlimit apply", "powerlimit", pl(limits=LIMITS), ["apply"], None, None),
        ("powerlimit apply, nothing set (full speed, the card's default)", "powerlimit", pl(smi="NVIDIA X, 200.00, 260.00, 100.00, 330.00\n"), ["apply"], None, None),
        ("powerlimit apply, the card already there", "powerlimit", pl(limits=LIMITS, smi="NVIDIA X, 200.00, 260.00, 100.00, 330.00\n"), ["apply"], None, None),
        ("powerlimit apply, a hold caps the card", "powerlimit", pl(mode="performance", limits=LIMITS, hold={"watts": 220}), ["apply"], None, None),
        ("powerlimit apply, above the card's top", "powerlimit", pl(mode="performance", limits={"performance": {"gpu": 900}}), ["apply"], None, None),
        ("powerlimit apply, no mode known", "powerlimit", pl(limits=LIMITS, cmds={UPOWER: None, HADESS: {"rc": 1, "out": ""}}), ["apply"], None, None),
        ("powerlimit apply, no card", "powerlimit", pl(mode="power-saver", limits=LIMITS, smi=None), ["apply"], None, None),
        ("powerlimit set, the active mode", "powerlimit", pl(limits=LIMITS), ["set", "balanced", "cpu=70", "gpu=0180"], None, None),
        ("powerlimit set, another mode (nothing applied)", "powerlimit", pl(limits=LIMITS), ["set", "performance", "cpu=95"], None, None),
        ("powerlimit set, default takes limits away", "powerlimit", pl(limits=LIMITS), ["set", "balanced", "cpu=default", "gpu=default"], None, None),
        ("powerlimit set, out of range is saved as given", "powerlimit", pl(limits=LIMITS), ["set", "balanced", "cpu=500"], None, None),
        ("powerlimit set, from the old place", "powerlimit", pl(old={"power-saver": {"cpu": 50}}), ["set", "balanced", "gpu=250"], None, None),
        ("powerlimit set, an unknown mode", "powerlimit", pl(limits=LIMITS), ["set", "turbo", "cpu=50"], None, None),
        ("powerlimit set, an unknown limit", "powerlimit", pl(limits=LIMITS), ["set", "balanced", "fan=50"], None, None),
        ("powerlimit set, not a number", "powerlimit", pl(limits=LIMITS), ["set", "balanced", "cpu=-5"], None, None),
        ("powerlimit set, no =", "powerlimit", pl(limits=LIMITS), ["set", "balanced", "gpu"], None, None),
        ("powerlimit set with too few words", "powerlimit", pl(limits=LIMITS), ["set", "balanced"], None, None),
        ("powerlimit watch, one mode", "powerlimit", pl(limits=LIMITS), ["watch"], 5, None),
        ("powerlimit watch, the mode changes", "powerlimit",
         pl(mode=["balanced", "balanced", "balanced", "performance", "performance", "performance", "power-saver"], limits=LIMITS), ["watch"], 8, None),
        ("powerlimit watch, no card, no mode", "powerlimit", pl(limits=LIMITS, smi=None, cmds={UPOWER: None, HADESS: None}), ["watch"], 3, None),
        ("powerlimit unknown command", "powerlimit", pl(), ["spin"], None, None),
    ]
    for i, s in enumerate(samples):
        tag = f"sample {i + 1}"
        same = lambda r, s=s: shutil.copytree(s, r, symlinks=True, dirs_exist_ok=True)
        c += [
            (f"{tag}: casefan get", "casefan", same, ["get"], None, None),
            (f"{tag}: casefan run", "casefan", same, ["run"], 40, None),
            (f"{tag}: casefan reset", "casefan", same, ["reset"], None, None),
            (f"{tag}: casefan reset failed", "casefan", same, ["reset", "failed"], None, None),
            (f"{tag}: gpufan get", "gpufan", same, ["get"], None, None),
            (f"{tag}: gpufan run (its own setting)", "gpufan", same, ["run"], 5, None),
            (f"{tag}: gpufan run (a curve)", "gpufan",
             lambda r, s=s: (same(r), files(r, "/etc/yggdrasil", {"gpu-fan.json": CURVE})), ["run"], 20, None),
            (f"{tag}: gpufan reset", "gpufan", same, ["reset"], None, None),
            (f"{tag}: tune get", "tune", same, ["get"], None, None),
            (f"{tag}: tune watch", "tune", same, ["watch"], 3, None),
        ]
    return c


# ---------------------------------------------------------------- running both


def normalize(text, root):
    text = text.replace(root, "ROOT")
    text = re.sub(r'"time": [0-9.e+-]+', '"time": T', text)
    text = re.sub(r'"since": "\d{4}-\d\d-\d\d \d\d:\d\d"', '"since": S', text)
    return text


NEW_IN_RUST = re.compile(r"^dry run: run logger -t yggdrasil-tune -p user\.notice -- (reading the kernel's messages from |the kernel's messages cannot be read)")


def run_case(args, case, work, n):
    name, helper, build, cargs, steps, stdin = case
    root = os.path.join(work, f"case{n}")
    os.makedirs(root)
    build(root)
    root = os.path.realpath(root)
    rust = [os.path.join(args.bin, helper), "--dry-run", "--root", root] + (["--steps", str(steps)] if steps else []) + cargs
    py = [sys.executable, os.path.join(HERE, "py_dryrun.py"), py_helper(args, helper), root] + (["--steps", str(steps)] if steps else []) + ["--"] + cargs
    out = []
    for cmd in (rust, py):
        p = subprocess.run(cmd, input=stdin, capture_output=True, text=True, timeout=120)
        out.append((normalize(p.stdout, root), normalize(p.stderr, root), p.returncode))
    (ro, re_, rc), (po, pe, pc) = out
    ro = "".join(line for line in ro.splitlines(True) if not NEW_IN_RUST.match(line))
    same = (ro, re_, rc) == (po, pe, pc)
    detail = ""
    if not same:
        detail = "".join(difflib.unified_diff((po + pe + f"exit {pc}\n").splitlines(True), (ro + re_ + f"exit {rc}\n").splitlines(True),
                                              "python", "rust"))
    return same, detail, (ro, re_, rc)


def help_case(args):
    """The help text: the Rust one is the Python docstring, then the dry-run paragraph."""
    bad = []
    for h in ("tune", "casefan", "gpufan", "powerlimit"):
        for a in ([], ["--help"]):
            r = subprocess.run([os.path.join(args.bin, h), *a], capture_output=True, text=True)
            p = subprocess.run([sys.executable, py_helper(args, h), *a], capture_output=True, text=True)
            doc = p.stdout[:-1]   # print(__doc__): the docstring, then a newline
            if not (r.stdout.startswith(doc) and "Dry run" in r.stdout[len(doc):] and r.returncode == p.returncode):
                bad.append(f"{h} {a}")
    return bad


# ---------------------------------------------------------------- casefan's loop over many sequences


def load_py(path):
    loader = importlib.machinery.SourceFileLoader("casefan_py", path)
    spec = importlib.util.spec_from_loader("casefan_py", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


def py_replay(m, setting, temps):
    cfg, temps, out, clock = [m.clean(setting)], list(temps), [], [1000.0]

    def read_cpu():
        if not temps:
            raise m.SensorError("the recorded readings ran out")
        return temps[0][0]

    def read_gpu():
        return temps.pop(0)[1] if temps else 0

    def sleep(s):
        clock[0] += s

    err = io.StringIO()
    with redirect_stderr(err):
        rc = m.run("x", lambda: cfg.pop() if cfg else None, read_cpu, read_gpu, lambda n, p: out.append(f"{n} {p}"), sleep, lambda: False,
                   clock=lambda: clock[0])
    return "\n".join(out + [f"exit {rc}"]) + "\n"


def sequences(seeds):
    swing = [(58, 60), (58, 60), (43, 54), (42, 54), (58, 59), (64, 60), (58, 60), (43, 55), (42, 54)] * 8
    seqs = [[(45, 35), (45, 35), (84, 35)], swing, [(50, 40)] * 5 + [(92, 40)], [(50, 40)] * 40 + [(91, 82)] * 3 + [(50, 40)] * 40]
    rng = random.Random(20261109)
    for _ in range(seeds):
        cpu, gpu, s = rng.randint(30, 70), rng.randint(25, 65), []
        for _ in range(rng.randint(20, 120)):
            cpu = max(25, min(99, cpu + rng.choice([-9, -4, -2, -1, 0, 0, 1, 2, 4, 9, 15])))
            gpu = max(20, min(95, gpu + rng.choice([-7, -3, -1, 0, 0, 1, 3, 7, 12])))
            s.append((cpu, gpu))
        seqs.append(s)
    return seqs


SETTINGS = [
    {"ports": {"6": {"role": "fan"}}},
    {"ports": {"1": {"name": "pump", "role": "pump", "fixed": 100}, "6": {"role": "fan"}}},
    {"ports": {"1": {"role": "pump", "fixed": 70}, "5": {"role": "fan", "cpu": [[50, 35], [70, 50], [85, 80], [90, 100]], "gpu": [[45, 35], [65, 50], [75, 80], [80, 100]]},
               "6": {"role": "fan", "cpu": [[50, 25], [65, 35], [75, 55], [85, 80], [90, 100]], "gpu": [[45, 25], [55, 35], [65, 55], [72, 80], [78, 100]]}}},
    {"ports": {"2": {"role": "fan", "cpu": [[30, 0], [90, 100]], "gpu": [[30, 0], [90, 100]]}, "3": {"role": "fan", "follow": "2"}}},
]


def replay_all(args):
    m = load_py(py_helper(args, "casefan"))
    exe = os.path.join(args.bin, "examples", "casefan_replay")
    total, bad = 0, []
    for si, setting in enumerate(SETTINGS):
        for qi, seq in enumerate(sequences(args.seeds)):
            total += 1
            text = json.dumps({"setting": setting, "temps": seq})
            r = subprocess.run([exe], input=text, capture_output=True, text=True).stdout
            p = py_replay(m, setting, seq)
            if r != p:
                bad.append((si, qi, "".join(difflib.unified_diff(p.splitlines(True), r.splitlines(True), "python", "rust"))))
    return total, bad


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--python")
    ap.add_argument("--bin", default=os.path.normpath(os.path.join(HERE, "..", "..", "..", "target", "release")))
    ap.add_argument("--sample", action="append", default=[])
    ap.add_argument("--seeds", type=int, default=300)
    ap.add_argument("-v", action="store_true")
    args = ap.parse_args(argv[1:])
    work = tempfile.mkdtemp(prefix="power-helpers-compare-")
    try:
        same = diff = 0
        for n, case in enumerate(cases(work, [os.path.realpath(s) for s in args.sample])):
            ok, detail, (ro, re_, rc) = run_case(args, case, work, n)
            same += ok
            diff += not ok
            print(f"{'same' if ok else 'DIFFERENT'}  {case[0]}")
            if not ok:
                print(detail)
            elif args.v:
                print("    " + (ro + re_).replace("\n", "\n    ") + f"exit {rc}")
        bad_help = help_case(args)
        print(f"{'same' if not bad_help else 'DIFFERENT'}  help text (Python's docstring, then the dry-run paragraph) {bad_help or ''}")
        total, bad = replay_all(args)
        print(f"{'same' if not bad else 'DIFFERENT'}  casefan's loop over {total} temperature sequences x settings ({len(bad)} differ)")
        for si, qi, d in bad[:5]:
            print(f"  setting {si}, sequence {qi}:\n{d}")
        print(f"\n{same} cases the same, {diff} different; help {'same' if not bad_help else 'different'}; "
              f"{total - len(bad)}/{total} sequences the same")
        return 0 if not diff and not bad and not bad_help else 1
    finally:
        shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
