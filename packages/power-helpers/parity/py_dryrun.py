#!/usr/bin/env python3
"""py_dryrun: the Python helpers (tune, casefan, gpufan) run as the Rust ones run with --dry-run --root ROOT [--steps N], so the
two can be compared line by line. The Python scripts' own code makes every decision; only the outside is replaced:

  - every system path is taken under a scratch copy of ROOT (writes land there, so later reads see them, as the Rust dry run's
    memory of its writes does), and printed under ROOT;
  - each write is printed as the Rust dry run prints it ("dry run: write PATH: TEXT", "dry run: remove PATH",
    "dry run: run systemctl ...", "dry run: run logger ...", "dry run: write PATH at 0x1ad: 0x...", "dry run: nvml...(...)");
    MSR writes are printed only (the Rust dry run does not remember them either);
  - the card is ROOT/nvml.json, behind a stand-in libnvidia-ml with the same rules as the Rust one (a list of temperatures is
    read one by one, then the card is lost, NVML error 15);
  - root checks pass; systemctl is-active and is-system-running really run (they only ask);
  - with --steps N the service loops run N rounds on a virtual clock that starts at 1000.0 s.

    py_dryrun.py HELPER ROOT [--steps N] -- ARGS...       (HELPER: the Python script's path; stdin is passed on)
"""
import ctypes
import importlib.machinery
import importlib.util
import json
import os
import shutil
import signal
import sys
import tempfile
import types

VCLOCK_START = 1000.0


def say(msg):
    print(f"dry run: {msg}", flush=True)


def load(path):
    name = os.path.basename(path)
    loader = importlib.machinery.SourceFileLoader(name, path)
    spec = importlib.util.spec_from_loader(name, loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


class Paths:
    def __init__(self, root):
        self.root = os.path.realpath(root)
        self.scratch = tempfile.mkdtemp(prefix="py-dryrun-")
        shutil.rmtree(self.scratch)
        shutil.copytree(self.root, self.scratch, symlinks=True)

    def __call__(self, p):
        """A system path under the scratch copy."""
        return os.path.join(self.scratch, p.lstrip("/"))

    def shown(self, p):
        """A scratch path as the Rust dry run names it (under ROOT)."""
        p = str(p)
        return self.root + p[len(self.scratch):] if p.startswith(self.scratch) else p

    def done(self):
        shutil.rmtree(self.scratch, ignore_errors=True)


# ---------------------------------------------------------------- a stand-in libnvidia-ml (the Rust Fake's rules)


class FakeNvml:
    def __init__(self, path):
        with open(path) as f:
            d = json.load(f)
        t = d.get("temp", 40)
        self.temps, self.fixed = (list(t), None) if isinstance(t, list) else ([], t)
        self.fans = d.get("fans", [])
        self.min, self.max = d.get("min", 0), d.get("max", 100)
        self.core, self.mem, self.max_core = d.get("core", 0), d.get("mem", 0), d.get("max_core", 0)
        self.fail = d.get("fail", {})

    def __getattr__(self, name):
        if not name.startswith("nvml"):
            raise AttributeError(name)
        return lambda *a: self.call(name, *a)

    def call(self, name, *a):
        if name in self.fail:
            return self.fail[name]
        v = lambda x: x.value if hasattr(x, "value") else x
        out = lambda ref: ref._obj
        if name in ("nvmlInit_v2", "nvmlShutdown", "nvmlDeviceGetHandleByIndex_v2"):
            return 0
        if name == "nvmlDeviceGetTemperature":
            if self.fixed is not None:
                out(a[2]).value = self.fixed
            elif self.temps:
                out(a[2]).value = self.temps.pop(0)
            else:
                return 15
            return 0
        if name == "nvmlDeviceGetNumFans":
            out(a[1]).value = len(self.fans)
            return 0
        if name == "nvmlDeviceGetFanSpeed_v2":
            i = v(a[1])
            if i >= len(self.fans):
                return 2
            out(a[2]).value = self.fans[i]
            return 0
        if name == "nvmlDeviceGetMinMaxFanSpeed":
            out(a[1]).value, out(a[2]).value = self.min, self.max
            return 0
        if name == "nvmlDeviceGetClockOffsets":
            o = out(a[1])
            o.clockOffsetMHz = self.mem if o.type == 2 else self.core
            return 0
        if name == "nvmlDeviceGetMaxClockInfo":
            out(a[2]).value = self.max_core
            return 0
        if name == "nvmlDeviceSetFanSpeed_v2":
            say(f"nvmlDeviceSetFanSpeed_v2(fan {v(a[1])}, {v(a[2])})")
            return 0
        if name == "nvmlDeviceSetDefaultFanSpeed_v2":
            say(f"nvmlDeviceSetDefaultFanSpeed_v2(fan {v(a[1])})")
            return 0
        if name == "nvmlDeviceSetClockOffsets":
            o = out(a[1])
            say(f"nvmlDeviceSetClockOffsets(type {o.type}, pstate 0, {o.clockOffsetMHz} MHz)")
            return 0
        if name == "nvmlDeviceSetGpuLockedClocks":
            say(f"nvmlDeviceSetGpuLockedClocks({v(a[1])}, {v(a[2])})")
            return 0
        if name == "nvmlDeviceResetGpuLockedClocks":
            say("nvmlDeviceResetGpuLockedClocks()")
            return 0
        return 13


def fake_ctypes(paths):
    def cdll(name, *a, **k):
        p = paths("/nvml.json")
        if not os.path.exists(p):
            raise OSError(f"{name}: cannot open shared object file: No such file or directory")
        return FakeNvml(p)

    ns = types.SimpleNamespace(**{k: getattr(ctypes, k) for k in dir(ctypes) if not k.startswith("__")})
    ns.CDLL = cdll
    return ns


class VClock:
    """time.monotonic() and friends on a virtual clock; time.time() and strftime as they are."""

    def __init__(self):
        self.now = VCLOCK_START

    def module(self):
        import time as real
        ns = types.SimpleNamespace(**{k: getattr(real, k) for k in dir(real) if not k.startswith("__")})
        ns.monotonic = lambda: self.now
        ns.sleep = self.sleep
        return ns

    def sleep(self, s):
        self.now += max(0.0, s)


def systemctl_for(say_prefix=""):
    import subprocess

    def systemctl(*args):
        if args and args[0] in ("is-active", "is-system-running"):
            return subprocess.run(["systemctl", *args], capture_output=True, text=True).returncode
        say("run systemctl " + " ".join(args))
        return 0
    return systemctl


# ---------------------------------------------------------------- casefan


def casefan(m, paths, steps, args):
    m.CONF = paths(m.CONF)
    hwmon = paths("/sys/class/hwmon")
    orig_find, orig_temp, orig_save = m.find_cpro, m.cpu_temp, m.save
    m.find_cpro = lambda root=hwmon: orig_find(root)
    m.cpu_temp = lambda root=hwmon: orig_temp(root)
    m.is_root = lambda: True
    m.systemctl = systemctl_for()
    m.ctypes = fake_ctypes(paths)
    orig_set = m.set_pct

    def set_pct(cpro, n, pct):
        orig_set(cpro, n, pct)
        with open(os.path.join(cpro, f"pwm{n}")) as f:
            say(f"write {paths.shown(os.path.join(cpro, f'pwm{n}'))}: {f.read()}")
    m.set_pct = set_pct

    def save(cfg, path=None):
        orig_save(cfg, path)
        say(f"write {paths.shown(path or m.CONF)}: {json.dumps(m.clean(cfg))}")
    m.save = save

    if args == ["run"]:
        # cmd_run's own steps, with the loop's sleep and stop on the virtual clock
        cpro = m.find_cpro()
        if not cpro:
            print("casefan: no Corsair Commander Pro (corsair_cpro driver) found", file=sys.stderr)
            return 1
        seen, clock, rounds = [None], VClock(), [0]

        def read_cfg():
            try:
                m_ = os.stat(m.CONF).st_mtime_ns
            except OSError:
                m_ = 0
            if m_ == seen[0]:
                return None
            seen[0] = m_
            return m.load()

        def sleep(s):
            clock.sleep(s)
            rounds[0] += 1

        try:
            gpu = m.Gpu()
        except m.SensorError as e:
            print(f"casefan: {e}", file=sys.stderr)
            return 1
        return m.run(cpro, read_cfg, m.cpu_temp, gpu.temp, lambda n, p: m.set_pct(cpro, n, p), sleep,
                     lambda: steps is not None and rounds[0] >= steps, clock=lambda: clock.now)
    return m.main(["casefan", *args])


# ---------------------------------------------------------------- gpufan


def gpufan(m, paths, steps, args):
    m.CONF = paths(m.CONF)
    m.is_root = lambda: True
    m.systemctl = systemctl_for()
    m.ctypes = fake_ctypes(paths)
    orig_save = m.save

    def save(cfg, path=None):
        orig_save(cfg, path)
        say(f"write {paths.shown(path or m.CONF)}: {json.dumps(m.clean(cfg))}")
    m.save = save

    if args == ["run"]:
        seen, clock, rounds = [None], VClock(), [0]

        def read_cfg():
            try:
                m_ = os.stat(m.CONF).st_mtime_ns
            except OSError:
                m_ = 0
            if m_ == seen[0]:
                return None
            seen[0] = m_
            return m.load()

        def sleep(s):
            clock.sleep(s)
            rounds[0] += 1

        try:
            nv = m.Nvml()
        except m.NvmlError as e:
            print(f"gpufan: {e}", file=sys.stderr)
            return 1
        try:
            return m.run(nv, read_cfg, sleep, lambda: steps is not None and rounds[0] >= steps, clock=lambda: clock.now)
        finally:
            nv.close()
    return m.main(["gpufan", *args])


# ---------------------------------------------------------------- tune


def tune(m, paths, steps, args):
    m.CONF, m.STATE, m.RUN = paths(m.CONF), paths(m.STATE), paths(m.RUN)
    m.RAPL, m.PSTATE = paths(m.RAPL), paths(m.PSTATE)
    m.is_root = lambda: True
    m.ctypes = fake_ctypes(paths)
    m.log = lambda event: say("run logger -t yggdrasil-tune -p user.notice -- " + event)

    def boot_id():
        try:
            with open(paths("/proc/sys/kernel/random/boot_id")) as f:
                return f.read().strip()
        except OSError:
            return ""
    m.boot_id = boot_id
    dev_cpu = paths("/dev/cpu")
    m.cpus = lambda: sorted(int(n) for n in os.listdir(dev_cpu) if n.isdigit()) if os.path.isdir(dev_cpu) else []

    def rdmsr(reg, cpu=0):
        fd = os.open(os.path.join(dev_cpu, str(cpu), "msr"), os.O_RDONLY)
        try:
            return m.struct.unpack("<Q", os.pread(fd, 8, reg))[0]
        finally:
            os.close(fd)
    m.rdmsr = rdmsr

    def wrmsr(reg, value):
        for c in m.cpus():
            say(f"write {paths.shown(os.path.join(dev_cpu, str(c), 'msr'))} at {reg:#x}: {value:#018x}")
    m.wrmsr = wrmsr
    orig_write_json, orig_remove = m.write_json, m.remove

    def write_json(path, d, mode=0o644):
        orig_write_json(path, d, mode)
        say(f"write {paths.shown(path)}: {json.dumps(d)}")
    m.write_json = write_json

    def remove(path):
        if os.path.exists(path):
            say(f"remove {paths.shown(path)}")
        orig_remove(path)
    m.remove = remove

    def write_text(path, text):
        with open(path, "w") as f:
            f.write(text)
        say(f"write {paths.shown(path)}: {text}")
    m.write_text = write_text

    import subprocess as real_sp
    sp = types.SimpleNamespace(**{k: getattr(real_sp, k) for k in dir(real_sp) if not k.startswith("__")})

    class Journal:
        """journalctl -k -f -o cat, from ROOT/dev/kmsg (a file of lines, as the Rust dry run reads it in a fake tree)."""

        def __init__(self, *a, **k):
            p = paths("/dev/kmsg")
            self.stdout = open(p) if os.path.exists(p) else open(os.devnull)

        def terminate(self):
            self.stdout.close()

    def run(cmd, *a, **k):
        if cmd[:2] == ["systemctl", "is-active"] or cmd[:2] == ["systemctl", "is-system-running"]:
            return real_sp.run(cmd, *a, **k)
        raise RuntimeError(f"unexpected command {cmd}")
    sp.Popen = Journal
    sp.run = run
    m.subprocess = sp

    if args == ["watch"] and steps is not None:
        clock = VClock()
        m.time = clock.module()
        count, orig_publish = [0], m.publish

        def publish(power_w=None, gpu=None):
            out = orig_publish(power_w, gpu)
            count[0] += 1
            if count[0] >= steps:
                os.kill(os.getpid(), signal.SIGTERM)
            return out
        m.publish = publish
    return m.main(["tune", *args])


def main(argv):
    if "--" not in argv or len(argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2
    i = argv.index("--")
    helper, root, opts, args = argv[1], argv[2], argv[3:i], argv[i + 1:]
    steps = int(opts[opts.index("--steps") + 1]) if "--steps" in opts else None
    paths = Paths(root)
    try:
        m = load(helper)
        kind = os.path.basename(helper)
        rc = {"casefan": casefan, "gpufan": gpufan, "tune": tune}[kind](m, paths, steps, args)
    finally:
        paths.done()
    sys.stdout.flush()
    return rc


if __name__ == "__main__":
    sys.exit(main(sys.argv))
