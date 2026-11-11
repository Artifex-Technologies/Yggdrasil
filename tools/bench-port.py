#!/usr/bin/env python3
"""bench-port: each program ported to Rust against the Python script it replaces, on the same safe inputs.

The old version of each program comes from git history (`git show COMMIT:PATH`, the commit before its port; see OLD
below), so these numbers can be made again after the Python copies kept for a rollback are deleted. The new version is
this repository's release build in target/release (cargo build --release in each package first).

Every run is safe: --help, read-only commands and the parity scripts' made-up worlds (scratch folders, stand-in tools on
the PATH, fake hwmon/sysfs trees with a stand-in graphics card, dry runs). Nothing runs as root: where a helper needs to
be root, it is "root" of a user namespace of its own (unshare -rm), as its parity script runs it. No display, session bus
or audio is passed on (only the app usage runs ask the user's own systemd for its running scopes). Everything runs at
nice 19, one process at a time.

For each case the old and new versions run alternately (old, new, then new, old, ...), after two warm-up runs each that
are not counted. Each run is started by a tiny launcher (LAUNCHER below, built with rustc at the start) that forks, execs
the program and waits for it with wait4: wall time on the monotonic clock (the clock of time.perf_counter), CPU time
(user + system, its waited-for children included) and peak resident memory (ru_maxrss: the largest single process of the
run, so a stand-in tool started by the program counts when it is larger than the program). A process forked from this
Python harness would report the harness's own 20 MB as its peak, hence the launcher. Reported: the median and the
10th-90th percentile of wall time, the median CPU time and the median peak RSS.

The services (the power loops, app usage, crash watch) are also run for --daemon-seconds in real time after a 5 s start:
CPU time and context switches (wake-ups) in that span, per minute (each live thread's schedstat and status, plus the
children already waited for), and the memory of the whole process tree at its end (RSS and PSS from
/proc/PID/smaps_rollup, children such as journalctl included).

    nice -n 19 python3 tools/bench-port.py [--only NAME,...] [--runs N] [--heavy-runs N] [--daemon-seconds S]
                                           [--labyrinth DIR] [--json FILE] [-v]

NAME: power, settings-helpers, appusage, crashwatch, pkg-root, theme, cursors, liki, install-system
--labyrinth: the Labyrinth OS repository (the crash watch's Python script is in its history; default ../Labyrinth_OS)
"""
import argparse
import importlib.util
import json
import os
import shutil
import signal
import statistics
import subprocess
import sys
import tempfile
import time

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(REPO, "target", "release")
PKG = os.path.join(REPO, "packages")
PY = sys.executable

# program: (repository, commit, path): the commit is the one just before the port (or, for app usage, the port's own
# commit, which kept the script it replaced in python/)
OLD = {
    "tune": ("yggdrasil", "98e14bd", "packages/settings/helpers/tune"),
    "casefan": ("yggdrasil", "98e14bd", "packages/settings/helpers/casefan"),
    "gpufan": ("yggdrasil", "98e14bd", "packages/settings/helpers/gpufan"),
    "powerlimit": ("yggdrasil", "8b689f9", "packages/settings/helpers/powerlimit"),
    "account": ("yggdrasil", "725296c", "packages/settings/helpers/account"),
    "bootscreen": ("yggdrasil", "725296c", "packages/settings/helpers/bootscreen"),
    "game-fix": ("yggdrasil", "725296c", "packages/settings/helpers/game-fix"),
    "gpu-run": ("yggdrasil", "725296c", "packages/settings/helpers/gpu-run"),
    "labyrinth-appusage": ("yggdrasil", "725296c", "packages/appusage/python/labyrinth-appusage"),
    "labyrinth-crash-watch": ("labyrinth", "292a53b", "profile/airootfs/usr/local/bin/labyrinth-crash-watch"),
    "pkg-root": ("yggdrasil", "0cff417", "packages/market/helpers/pkg-root"),
    "ygg-theme": ("yggdrasil", "0cda95e", "packages/theme/bin/ygg-theme"),
    "ygg-cursors": ("yggdrasil", "daf2334", "packages/theme/bin/ygg-cursors"),
    "liki-build": ("yggdrasil", "1cd6211", "packages/liki/bin/liki-build"),
    "liki-icns": ("yggdrasil", "1cd6211", "packages/liki/bin/liki-icns"),
    "liki-icon-import": ("yggdrasil", "1cd6211", "packages/liki/bin/liki-icon-import"),
    "install-system": ("yggdrasil", "a234048", "packages/installer/helpers/install-system"),
}

ARGS = None
SCRATCH = None


# ---------------------------------------------------------------- measuring

# Why a launcher: Linux keeps a process's peak RSS across exec, and a child forked from this harness (about 20 MB of
# Python) starts with the harness's memory as its peak, so os.wait4 here would report at least 20 MB for any program.
# This launcher is a few hundred KB: it forks (so the child starts with the launcher's small memory), execs the program,
# and reports what os.wait4 reports here: wall time on the monotonic clock (as time.perf_counter), user + system CPU and
# ru_maxrss. Built with rustc into the scratch folder at the start of a run.
LAUNCHER = r"""
use std::ffi::CString;
use std::io::Write;
use std::os::raw::{c_char, c_int, c_long};
#[repr(C)] struct Timeval { sec: c_long, usec: c_long }
#[repr(C)] struct Rusage { utime: Timeval, stime: Timeval, maxrss: c_long, rest: [c_long; 13] }
#[repr(C)] struct Timespec { sec: c_long, nsec: c_long }
extern "C" {
    fn fork() -> c_int;
    fn execvp(file: *const c_char, argv: *const *const c_char) -> c_int;
    fn wait4(pid: c_int, status: *mut c_int, options: c_int, ru: *mut Rusage) -> c_int;
    fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
    fn _exit(code: c_int) -> !;
}
fn now() -> f64 { let mut t = Timespec { sec: 0, nsec: 0 }; unsafe { clock_gettime(1, &mut t) }; t.sec as f64 + t.nsec as f64 / 1e9 }
fn main() {
    let a: Vec<String> = std::env::args().collect();          // launch OUT -- PROGRAM ARGS...
    let args: Vec<CString> = a[3..].iter().map(|s| CString::new(s.as_str()).unwrap()).collect();
    let mut ptrs: Vec<*const c_char> = args.iter().map(|s| s.as_ptr()).collect();
    ptrs.push(std::ptr::null());
    let t0 = now();
    let pid = unsafe { fork() };
    if pid == 0 { unsafe { execvp(ptrs[0], ptrs.as_ptr()); _exit(127) } }
    let mut st: c_int = 0;
    let mut ru = Rusage { utime: Timeval { sec: 0, usec: 0 }, stime: Timeval { sec: 0, usec: 0 }, maxrss: 0, rest: [0; 13] };
    while unsafe { wait4(pid, &mut st, 0, &mut ru) } < 0 {}
    let wall = now() - t0;
    let cpu = (ru.utime.sec + ru.stime.sec) as f64 + (ru.utime.usec + ru.stime.usec) as f64 / 1e6;
    let code = if st & 0x7f == 0 { (st >> 8) & 0xff } else { -(st & 0x7f) };
    let mut f = std::fs::File::create(&a[1]).unwrap();
    writeln!(f, "{wall} {cpu} {} {code}", ru.maxrss).unwrap();
}
"""
LAUNCH = None


def build_launcher():
    global LAUNCH
    src = os.path.join(SCRATCH, "launch.rs")
    LAUNCH = os.path.join(SCRATCH, "launch")
    write(src, LAUNCHER)
    subprocess.run(["rustc", "-O", "-C", "strip=symbols", "-o", LAUNCH, src], check=True)


def measure(argv, env, stdin=None, cwd=None, wrap=()):
    """One run: (wall s, cpu s, peak RSS KB, exit code, stderr tail). wrap: what the launcher runs inside (a sandbox)."""
    out = os.path.join(SCRATCH, "run.txt")
    if os.path.exists(out):
        os.remove(out)
    with tempfile.TemporaryFile() as err:
        p = subprocess.Popen(list(wrap) + [LAUNCH, out, "--"] + argv, env=env, cwd=cwd,
                             stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=err)
        if stdin is not None:
            try:
                p.stdin.write(stdin)
                p.stdin.close()
            except BrokenPipeError:
                pass
        p.wait()
        err.seek(0)
        tail = err.read()[-400:].decode("utf-8", "replace")
    if not os.path.exists(out):
        raise SystemExit(f"bench-port: {argv[:3]} did not run ({p.returncode}): {tail!r}")
    with open(out) as f:
        wall, cpu, rss, code = f.read().split()
    return float(wall), float(cpu), int(rss), int(code), tail


class Case:
    """One input: the old and the new command lines. setup(), when given, makes the world again before every run (not
    timed). sandbox: run inside COMMAND (a list, e.g. unshare -rm sh sandbox.sh WORLD) with only the program measured."""

    def __init__(self, label, old, new, env, stdin=None, setup=None, cwd=None, runs=None, sandbox=None):
        self.label, self.old, self.new, self.env, self.stdin = label, old, new, env, stdin
        self.setup, self.cwd, self.runs, self.sandbox = setup, cwd, runs, sandbox


def run_one(case, argv):
    if case.setup:
        case.setup()
    return measure(argv, case.env, case.stdin, case.cwd, case.sandbox or ())


def pct(xs, q):
    if len(xs) < 2:
        return xs[0]
    d = statistics.quantiles(xs, n=10, method="inclusive")
    return d[0] if q == 10 else d[-1]


def summary(rs):
    walls, cpus, rss = [r[0] for r in rs], [r[1] for r in rs], [r[2] for r in rs]
    return {"wall_ms": statistics.median(walls) * 1000, "wall_p10_ms": pct(walls, 10) * 1000, "wall_p90_ms": pct(walls, 90) * 1000,
            "cpu_ms": statistics.median(cpus) * 1000, "rss_mb": statistics.median(rss) / 1024, "exit": sorted({r[3] for r in rs}),
            "runs": len(rs)}


def bench(case):
    n = case.runs or ARGS.runs
    res = {"old": [], "new": []}
    for _ in range(2):  # warm-up: the page cache, Python's bytecode of the standard library
        run_one(case, case.old)
        run_one(case, case.new)
    for i in range(n):
        order = ("old", "new") if i % 2 == 0 else ("new", "old")
        for who in order:
            res[who].append(run_one(case, case.old if who == "old" else case.new))
    out = {"label": case.label, "old": summary(res["old"]), "new": summary(res["new"])}
    if out["old"]["exit"] != out["new"]["exit"]:
        out["note"] = f"exit codes differ: old {out['old']['exit']}, new {out['new']['exit']}"
        print(f"  ! {case.label}: {out['note']}; old stderr {res['old'][-1][4]!r}; new stderr {res['new'][-1][4]!r}")
    if ARGS.v:
        print(f"  {case.label}: old {fmt_ms(out['old']['wall_ms'])} {out['old']['rss_mb']:.1f} MB, new {fmt_ms(out['new']['wall_ms'])} "
              f"{out['new']['rss_mb']:.1f} MB, exit {out['old']['exit']}/{out['new']['exit']}", flush=True)
    return out


# ---------------------------------------------------------------- services in real time


def proc_tree(roots):
    kids = {}
    for d in os.listdir("/proc"):
        if not d.isdigit():
            continue
        try:
            with open(f"/proc/{d}/stat") as f:
                s = f.read()
        except OSError:
            continue
        ppid = int(s[s.rindex(")") + 2:].split()[1])
        kids.setdefault(ppid, []).append(int(d))
    out, todo = [], list(roots)
    while todo:
        p = todo.pop()
        out.append(p)
        todo += kids.get(p, [])
    return out


def tree_cpu(pids):
    """(CPU seconds, context switches) of the tree: each live thread's time on a CPU (schedstat, in ns) and its voluntary
    and involuntary switches (a sleep that ends is one), plus the CPU of children already waited for (cutime + cstime,
    in clock ticks)."""
    tck, ticks, ns, switches = os.sysconf("SC_CLK_TCK"), 0, 0, 0
    for p in pids:
        try:
            with open(f"/proc/{p}/stat") as f:
                s = f.read()
            ticks += sum(int(x) for x in s[s.rindex(")") + 2:].split()[13:15])  # cutime, cstime
            for t in os.listdir(f"/proc/{p}/task"):
                with open(f"/proc/{p}/task/{t}/schedstat") as f:
                    ns += int(f.read().split()[0])
                with open(f"/proc/{p}/task/{t}/status") as f:
                    for line in f:
                        if "ctxt_switches:" in line:
                            switches += int(line.split()[1])
        except (OSError, ValueError):
            continue
    return ticks / tck + ns / 1e9, switches


def tree_mem(pids):
    rss = pss = 0
    for p in pids:
        try:
            with open(f"/proc/{p}/smaps_rollup") as f:
                for line in f:
                    if line.startswith("Rss:"):
                        rss += int(line.split()[1])
                    elif line.startswith("Pss:"):
                        pss += int(line.split()[1])
        except OSError:
            continue
    return rss / 1024, pss / 1024


def service(label, cmds, env, seconds, settle=5.0):
    """cmds: the command lines that together are the service (started at once, each in a session of its own)."""
    procs = [subprocess.Popen(c, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                              start_new_session=True) for c in cmds]
    try:
        time.sleep(settle)
        roots = [p.pid for p in procs]
        c0, w0 = tree_cpu(proc_tree(roots))
        time.sleep(seconds)
        pids = proc_tree(roots)
        c1, w1 = tree_cpu(pids)
        rss, pss = tree_mem(pids)
        alive = all(p.poll() is None for p in procs)
    finally:
        for p in procs:
            try:
                os.killpg(p.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for p in procs:
            try:
                p.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(p.pid, signal.SIGKILL)
                p.wait()
    out = {"label": label, "start_cpu_ms": c0 * 1000, "cpu_ms_per_min": (c1 - c0) / seconds * 60 * 1000, "rss_mb": rss, "pss_mb": pss,
           "wakeups_per_min": (w1 - w0) / seconds * 60, "processes": len(pids), "seconds": seconds, "alive": alive}
    if not alive:
        print(f"  ! {label}: a process ended early")
    if ARGS.v:
        print(f"  {label}: {out['cpu_ms_per_min']:.1f} ms CPU/min, {out['wakeups_per_min']:.0f} switches/min, {rss:.1f} MB RSS, "
              f"{pss:.1f} MB PSS, {len(pids)} processes", flush=True)
    return out


def services(label, old_cmds, new_cmds, env_old, env_new=None):
    s = ARGS.daemon_seconds
    return {"label": label, "old": service(label + " (old)", old_cmds, env_old, s), "new": service(label + " (new)", new_cmds, env_new or env_old, s)}


# ---------------------------------------------------------------- helpers


def git_show(repo, rev, path):
    return subprocess.run(["git", "-C", repo, "show", f"{rev}:{path}"], capture_output=True, check=True).stdout


def old_scripts():
    """The old scripts from git, under SCRATCH/old/NAME (each keeps its own name: py_dryrun.py goes by it)."""
    d = os.path.join(SCRATCH, "old")
    os.makedirs(d, exist_ok=True)
    out, commits = {}, {}
    for name, (repo, rev, path) in OLD.items():
        root = REPO if repo == "yggdrasil" else ARGS.labyrinth
        try:
            data = git_show(root, rev, path)
        except (subprocess.CalledProcessError, FileNotFoundError):
            print(f"bench-port: no {name} at {repo} {rev}:{path} (skipped)")
            continue
        p = os.path.join(d, name)
        with open(p, "wb") as f:
            f.write(data)
        os.chmod(p, 0o755)
        out[name] = p
        commits[name] = subprocess.run(["git", "-C", root, "rev-parse", "--short", rev], capture_output=True, text=True).stdout.strip()
    return out, commits


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    if isinstance(getattr(m, "SCRATCH", None), str) and m.SCRATCH.startswith(tempfile.gettempdir()):
        shutil.rmtree(m.SCRATCH, ignore_errors=True)   # the parity script's own scratch folder, made when it is loaded
    return m


def mkdir(*parts):
    p = os.path.join(SCRATCH, *parts)
    shutil.rmtree(p, ignore_errors=True)
    os.makedirs(p)
    return os.path.realpath(p)


def write(p, data, mode=None):
    os.makedirs(os.path.dirname(p), exist_ok=True)
    with open(p, "wb") as f:
        f.write(data.encode() if isinstance(data, str) else data)
    if mode is not None:
        os.chmod(p, mode)


def base_env(**extra):
    """No display, no session bus, no audio: a scratch HOME and runtime folder."""
    home = os.path.join(SCRATCH, "home")
    run = os.path.join(SCRATCH, "runtime")
    for d in (home, run):
        os.makedirs(d, exist_ok=True)
    os.chmod(run, 0o700)
    e = {"PATH": "/usr/bin:/bin", "HOME": home, "LANG": "C.UTF-8", "PYTHONDONTWRITEBYTECODE": "1", "XDG_RUNTIME_DIR": run,
         "TMPDIR": os.path.join(SCRATCH, "tmp")}
    os.makedirs(e["TMPDIR"], exist_ok=True)
    e.update(extra)
    return {k: v for k, v in e.items() if v is not None}


STUB_OK = "#!/bin/sh\nexit 0\n"


# ---------------------------------------------------------------- the programs


def power_cases(old):
    """tune, casefan, gpufan and powerlimit: --help; get and the service loops on the parity script's fake trees (Python
    through parity/py_dryrun.py, which keeps the script's own code and replaces only the outside; Rust with --dry-run
    --root); then the four loops as services for real."""
    par = os.path.join(PKG, "power-helpers", "parity")
    pc = load(os.path.join(par, "compare.py"), "power_compare")
    pyd = os.path.join(par, "py_dryrun.py")
    env = base_env()
    cases = []
    for h in ("tune", "casefan", "gpufan", "powerlimit"):
        cases.append(Case(f"{h} --help", [PY, old[h], "--help"], [os.path.join(BIN, h), "--help"], env))

    def tree(name, build):
        r = mkdir("power", name)
        build(r)
        return r
    limits = {"balanced": {"cpu": 80, "gpu": 200}, "power-saver": {"cpu": 40, "gpu": 150}, "performance": {"gpu": 300}}
    t = {"tune": tree("tune", lambda r: pc.tune_tree(r)),
         "casefan": tree("casefan", lambda r: pc.casefan_tree(r, nvml={"temp": 35, **pc.CARD}, cfg=pc.PUMP_AND_FAN)),
         "gpufan": tree("gpufan", lambda r: pc.gpufan_tree(r, nvml={"temp": 50, **pc.CARD}, cfg=pc.CURVE)),
         "powerlimit": tree("powerlimit", lambda r: pc.powerlimit_tree(r, limits=limits))}

    def dry(h, args, steps=None):
        s = ["--steps", str(steps)] if steps else []
        return [PY, pyd, old[h], t[h]] + s + ["--"] + args, [os.path.join(BIN, h), "--dry-run", "--root", t[h]] + s + args
    for h, args in (("tune", ["get"]), ("casefan", ["get"]), ("gpufan", ["get"]), ("powerlimit", ["get"])):
        o, n = dry(h, args)
        cases.append(Case(f"{h} {' '.join(args)} (fake tree)", o, n, env))
    # one minute of each loop on a virtual clock: 30 rounds of 2 s (powerlimit: 20 of 3 s)
    for h, args, steps in (("tune", ["watch"], 30), ("casefan", ["run"], 30), ("gpufan", ["run"], 30), ("powerlimit", ["watch"], 20)):
        o, n = dry(h, args, steps)
        cases.append(Case(f"{h} {args[0]}, one minute of rounds (fake tree)", o, n, env))
    out = [bench(c) for c in cases]

    # the four loops for real: the four Python scripts as they ran (four processes) against the merged Rust service
    allr = tree("all", lambda r: (pc.all_tree(r), pc.files(r, "/run/yggdrasil/power/on", {p: "token-" + p for p in pc.PARTS})))
    driver = os.path.join(SCRATCH, "py_dryrun_realtime.py")
    write(driver, f"""import sys, time
sys.path.insert(0, {par!r})
import py_dryrun


class RealClock:
    # the scripts' own clock: casefan and gpufan slept in 0.2 s slices (to notice a stop), tune and powerlimit as they are
    now = property(lambda self: time.monotonic())

    def module(self):
        return time

    def sleep(self, s):
        end = time.monotonic() + s
        while time.monotonic() < end:
            time.sleep(min(0.2, max(0.0, end - time.monotonic())))


py_dryrun.VClock = RealClock
sys.exit(py_dryrun.main(sys.argv))
""")
    loops = {"tune": ["watch"], "casefan": ["run"], "gpufan": ["run"], "powerlimit": ["watch"]}
    old_cmds = [[PY, driver, old[h], allr, "--"] + a for h, a in loops.items()]
    new_cmds = [[os.path.join(BIN, "power"), "--dry-run", "--root", allr, "run"]]
    svc = services("the four loops as services (fake tree)", old_cmds, new_cmds, env)
    return out, [svc]


def settings_cases(old):
    """account, bootscreen, game-fix and gpu-run on the parity script's made-up worlds (stand-in tools, scratch files)."""
    par = os.path.join(PKG, "settings-helpers", "parity")
    sc = load(os.path.join(par, "compare.py"), "settings_compare")
    sandbox_sh = os.path.join(par, "sandbox.sh")
    cases = []

    def world_env(name, build, **extra):
        w = mkdir("settings", name)
        e = build(w) or {}
        env = base_env(PATH=os.path.join(w, "bin") + ":/usr/bin:/bin", WORLD=w, HOME=os.path.join(w, "home"), LANG="en_US.UTF-8")
        env.update(e)
        env.update(extra)
        return w, env
    rs = lambda n: os.path.join(BIN, n)
    # account (as its tests run it: YGGDRASIL_ACCOUNT_TEST=1, no root)
    _, e = world_env("account", sc.account_world(sc.PASSWD_B, sc.GROUP_B, "# the OS's own\nclaude-agent*\nsarina\n", sudoers=("robin",)),
                     PKEXEC_UID="1000", YGGDRASIL_ACCOUNT_TEST="1")
    cases.append(Case("account list", [PY, old["account"], "list"], [rs("account"), "list"], e))
    cases.append(Case("account add (a new user, stand-in tools)", [PY, old["account"], "add", "alex"], [rs("account"), "add", "alex"], e,
                      stdin=b"longenough1\n"))
    # bootscreen
    G = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT='quiet loglevel=3'\nGRUB_CMDLINE_LINUX=\"\"\n"
    brand = "kind = branding\nboot_theme = labyrinth-rain\n"
    w, e = world_env("boot", sc.boot_world(grub=G, branding=brand))
    cases.append(Case("bootscreen get", [PY, old["bootscreen"], "get"], [rs("bootscreen"), "get"], e))
    # your own picture: a 12-megapixel photo (4000x3000 JPEG) made into the boot screen (scaled to fit 3840x2160, PNG);
    # set needs root: in a user namespace of its own, as the parity script runs it
    pic = os.path.join(SCRATCH, "settings", "photo.jpg")
    make_photo(pic, 4000, 3000)
    build = sc.boot_world(grub=G, branding=brand)

    def fresh_boot():
        shutil.rmtree(w, ignore_errors=True)
        os.makedirs(w)
        build(w)
    cases.append(Case("bootscreen set picture (12 MP photo)", [PY, old["bootscreen"], "set", "picture", pic], [rs("bootscreen"), "set", "picture", pic],
                      e, setup=fresh_boot, sandbox=["unshare", "-rm", "sh", sandbox_sh, w], runs=ARGS.heavy_runs))
    # game-fix
    gw, e = world_env("steam", sc.steam_world(), YGGDRASIL_SCREEN="2560x1440")
    sbuild = sc.steam_world()

    def fresh_steam():
        shutil.rmtree(gw, ignore_errors=True)
        os.makedirs(gw)
        sbuild(gw)
    cases.append(Case("game-fix list --json", [PY, old["game-fix"], "list", "--json"], [rs("game-fix"), "list", "--json"], e))
    cases.append(Case("game-fix apply (Dead Space 2)", [PY, old["game-fix"], "apply", "47780"], [rs("game-fix"), "apply", "47780"], e,
                      setup=fresh_steam))
    # gpu-run: two cards (the parity script's "two" world)
    two = [("card1", "0000:01:00.0", "0x10de", "0x1f07", "nvidia", True, [True]), ("card2", "0000:03:00.0", "0x8086", "0xe20b", "xe", False, [False, False])]
    names = {"0000:01:00.0": '01:00.0 "VGA compatible controller" "NVIDIA Corporation" "TU106 [GeForce RTX 2070 Rev. A]" -ra1 "x"',
             "0000:03:00.0": '03:00.0 "VGA" "Intel" "Battlemage G21"'}
    _, e = world_env("gpu", sc.gpu_world(two, names))
    cases.append(Case("gpu-run list --json", [PY, old["gpu-run"], "list", "--json"], [rs("gpu-run"), "list", "--json"], e))
    cases.append(Case("gpu-run 2 PROGRAM (starts a stand-in program)", [PY, old["gpu-run"], "2", "probe"], [rs("gpu-run"), "2", "probe"], e))
    return [bench(c) for c in cases], []


def make_photo(path, w, h):
    """A photo-like picture (smooth gradients and fine detail), the same bytes every time."""
    from PIL import Image, ImageFilter
    import random
    rnd = random.Random(7)
    small = Image.new("RGB", (w // 50, h // 50))
    small.putdata([(rnd.randrange(256), rnd.randrange(256), rnd.randrange(256)) for _ in range((w // 50) * (h // 50))])
    img = small.resize((w, h), Image.BICUBIC)
    noise = Image.effect_noise((w, h), 24).convert("RGB")
    img = Image.blend(img, noise, 0.15).filter(ImageFilter.SMOOTH)
    os.makedirs(os.path.dirname(path), exist_ok=True)
    img.save(path, quality=90)


def appusage_cases(old):
    """--once (one sample: asks this user's systemd for the running scopes, read only) and the service, writing to a
    scratch XDG_DATA_HOME."""
    data = mkdir("appusage", "data")
    real = {k: os.environ.get(k) for k in ("XDG_RUNTIME_DIR", "DBUS_SESSION_BUS_ADDRESS")}
    env = base_env(XDG_DATA_HOME=data, **real)
    cases = [Case("labyrinth-appusage --once", [PY, old["labyrinth-appusage"], "--once"], [os.path.join(BIN, "labyrinth-appusage"), "--once"], env)]
    out = [bench(c) for c in cases]
    svc = services("the service, idle", [[PY, old["labyrinth-appusage"]]], [[os.path.join(BIN, "labyrinth-appusage")]], env)
    return out, [svc]


def crashwatch_cases(old):
    """The service only: the Python script followed the journal through journalctl; the Rust one follows the core dump
    folder. notify-send and the assistant are stand-ins for the Python one, the Rust one runs with --dry-run: a crash
    meanwhile would show nothing on the desktop."""
    if "labyrinth-crash-watch" not in old:
        return [], []
    stubs = mkdir("crashwatch", "bin")
    for t in ("notify-send", "labyrinth-ask-sarina"):
        write(os.path.join(stubs, t), STUB_OK, 0o755)
    env = base_env(PATH=stubs + ":/usr/bin:/bin")
    svc = services("the service, idle", [[PY, old["labyrinth-crash-watch"]]], [[os.path.join(BIN, "yggdrasil-crashwatch"), "--dry-run"]], env)
    return [], [svc]


def pkgroot_cases(old):
    """A refusal (no sandbox needed: nothing runs) and an install with a stand-in pacman in the parity script's sandbox
    (a user and mount namespace: an empty /run/user, a scratch pacman cache)."""
    par = os.path.join(PKG, "agora-helpers")
    ac = load(os.path.join(par, "parity", "compare.py"), "agora_compare")
    w = mkdir("pkgroot", "w")
    shutil.rmtree(w)
    ac.world(w)
    env = base_env(PATH=os.path.join(w, "bin") + ":/usr/bin:/bin", W=w, PKEXEC_UID="1000", LANG="en_US.UTF-8")
    good = ac.GOOD
    rs = os.path.join(BIN, "pkg-root")
    refused = [good, "pacman", "-S", "--root", "krita"]
    cases = [Case("pkg-root, a refused command", [PY, old["pkg-root"]] + refused, [rs] + refused, env)]

    def fresh():
        shutil.rmtree(w, ignore_errors=True)
        ac.world(w)
    inst = [good, "pacman", "-S", "--needed", "--noconfirm", "krita"]
    cases.append(Case("pkg-root, an install (stand-in pacman)", [PY, old["pkg-root"]] + inst, [rs] + inst, env, setup=fresh,
                      sandbox=["unshare", "-rm", "sh", os.path.join(par, "tests", "sandbox.sh"), w]))
    return [bench(c) for c in cases], []


def theme_cases(old):
    """What ygg-theme prints, and apply in a scratch HOME with stand-ins for gsettings, systemctl, hyprctl and pkill. No
    display is passed on, so GTK 4 cannot start in either (gtk4-theme reads GTK's own theme without it)."""
    env = base_env()
    py, rs = [PY, old["ygg-theme"]], [os.path.join(BIN, "ygg-theme")]
    cases = []
    for args in (["scheme", "dark", "124,84,214"], ["gtk", "dark", "124,84,214"], ["gtk3-theme", "dark"], ["gtk4-theme", "dark"]):
        cases.append(Case("ygg-theme " + " ".join(args), py + args, rs + args, env))
    w = mkdir("theme", "w")
    for t in ("gsettings", "systemctl", "hyprctl", "pkill"):
        write(os.path.join(w, "bin", t), STUB_OK, 0o755)
    e = base_env(PATH=os.path.join(w, "bin") + ":/usr/bin:/bin", HOME=os.path.join(w, "h"), XDG_CONFIG_HOME=os.path.join(w, "c"),
                 XDG_DATA_HOME=os.path.join(w, "d"), YGG_THEME_KDE="0")

    def fresh():
        for d in ("h", "c", "d"):
            shutil.rmtree(os.path.join(w, d), ignore_errors=True)
        os.makedirs(os.path.join(w, "h"))
    args = ["apply", "--dark", "--accent", "124,84,214"]
    cases.append(Case("ygg-theme apply (scratch HOME)", py + args, rs + args, e, setup=fresh))
    return [bench(c) for c in cases], []


def cursors_cases(old):
    """The whole cursor theme into a scratch folder, with the preview (the package build runs it)."""
    env = base_env()
    out = os.path.join(SCRATCH, "cursors", "out")

    def fresh():
        shutil.rmtree(out, ignore_errors=True)
        os.makedirs(os.path.dirname(out), exist_ok=True)
    c = Case("ygg-cursors OUTDIR --preview", [PY, old["ygg-cursors"], out, "--preview"], [os.path.join(BIN, "ygg-cursors"), out, "--preview"],
             env, setup=fresh, runs=ARGS.heavy_runs)
    return [bench(c)], []


def liki_cases(old):
    """liki-build (the package's natives-only pack, and a user's pack with app logos), liki-icns and liki-icon-import
    (a small pack and a large one) on the parity script's made-up worlds."""
    lc = load(os.path.join(PKG, "liki", "parity", "compare.py"), "liki_compare")
    w = mkdir("liki", "w")
    src = os.path.join(SCRATCH, "liki", "input")
    shutil.rmtree(src, ignore_errors=True)
    lc.brand_icons(src)
    stub_bin = os.path.join(SCRATCH, "liki", "bin")
    write(os.path.join(stub_bin, "gtk-update-icon-cache"), lc.STUB, 0o755)
    home = os.path.join(w, "home")
    env = base_env(PATH=stub_bin + ":/usr/bin:/bin", HOME=home, W=w, TERM="xterm", LIKI_BRAND_ICONS=os.path.join(src, "brand-icons"))
    rs = lambda n: os.path.join(BIN, n)
    cases = []

    def fresh(desktops=False):
        def setup():
            shutil.rmtree(w, ignore_errors=True)
            os.makedirs(home)
            if desktops:
                # the parity script's desktop files, but those with a "/" in Icon= (the old script stopped there: a bug
                # the port fixed), so both do the same work
                for n, t in lc.DESKTOPS:
                    if "Icon=" in str(t) and "/" not in str(t).split("Icon=", 1)[1].split("\n")[0]:
                        write(os.path.join(home, ".local", "share", "applications", n), t)
        return setup
    a = ["--natives-only", "--out", os.path.join(w, "out", "Liki")]
    cases.append(Case("liki-build --natives-only (the package's pack)", [PY, old["liki-build"]] + a, [rs("liki-build")] + a, env, setup=fresh()))
    a = ["--shape", "squircle", "--style", "brand"]
    cases.append(Case("liki-build, a user's pack with app logos", [PY, old["liki-build"]] + a, [rs("liki-build")] + a, env, setup=fresh(True)))
    # liki-icns: the biggest PNG of a made .icns
    inp = os.path.join(SCRATCH, "liki", "in.icns")
    write(inp, lc.icns([(b"ic07", lc.png(128, 128)), (b"ARGB", b"\x00" * 64), (b"ic09", lc.png(512, 512, b"a")), (b"ic10", lc.png(1024, 1024, b"b")),
                        (b"ic08", lc.png(256, 256))]))
    png_out = os.path.join(SCRATCH, "liki", "out.png")
    cases.append(Case("liki-icns", [PY, old["liki-icns"], inp, png_out], [rs("liki-icns"), inp, png_out], env))
    # liki-icon-import: the parity script's pack, and a large one (3,000 icons, as big packs have)
    small = os.path.join(SCRATCH, "liki", "Small Pack v1.0.apk")
    plain = "<resources>" + "".join(f'<item component="{c}" drawable="{d}"/>' for c, d in lc.ITEMS) + "</resources>"
    lc.apk(small, plain=plain, pics=lc.PICS)
    big = os.path.join(SCRATCH, "liki", "Big Pack v2.0.apk")
    items = lc.ITEMS + [(f"ComponentInfo{{com.example.app{i}/com.example.app{i}.Main}}", f"app_{i}") for i in range(3000)]
    pics = lc.PICS + [(f"res/drawable-nodpi/app_{i}.png", lc.png(192, 192, rgb=(i % 256, (i * 7) % 256, (i * 13) % 256))) for i in range(3000)]
    lc.apk(big, plain="<resources>" + "".join(f'<item component="{c}" drawable="{d}"/>' for c, d in items) + "</resources>", pics=pics)
    themes = os.path.join(w, "themes")

    def fresh_import():
        shutil.rmtree(w, ignore_errors=True)
        os.makedirs(home)
        for n, t in [("firefox.desktop", "[Desktop Entry]\nName=Firefox\nIcon=firefox\nExec=firefox %u\n"),
                     ("vlc.desktop", "[Desktop Entry]\nName=VLC\nIcon=vlc\nExec=/usr/bin/vlc --started-from-file %U\n"),
                     ("spotify.desktop", "[Desktop Entry]\nName=Spotify\nIcon=spotify-client\nStartupWMClass=spotify\n"),
                     ("com.valvesoftware.Steam.desktop", "[Desktop Entry]\nName=Steam\nIcon=steam\n"),
                     ("settings.desktop", "[Desktop Entry]\nName=Settings\nIcon=preferences-system\nExec=settings\n")]:
            write(os.path.join(home, ".local", "share", "applications", n), t)
    for label, pack in (("liki-icon-import, a small pack", small), ("liki-icon-import, a pack of 3,000 icons", big)):
        a = [pack, "--out", themes]
        cases.append(Case(label, [PY, old["liki-icon-import"]] + a, [rs("liki-icon-import")] + a, env, setup=fresh_import))
    return [bench(c) for c in cases], []


# The install program's stand-ins for the bench: tiny shell scripts with canned answers (made by the parity script's own
# fake.py for the same made-up computer), so that what is timed is the install program, not a Python stand-in started 60
# times. They write nothing down and change nothing; unsquashfs copies the made-up system into the target (in the
# sandbox's own tmpfs).
INSTALL_STANDINS = {
    "lsblk": """case "$1 $2 $3" in
  "-J -b -d") cat "$WORLD/out/lsblk-d" ;;
  "-J -b -o") n=$(basename "$5"); [ -f "$WORLD/out/lsblk-$n" ] && cat "$WORLD/out/lsblk-$n" || exit 32 ;;
  "-nr -o MOUNTPOINTS") printf '\\n\\n\\n\\n' ;;
  *) [ "$3" = /dev/sdy1 ] && echo sdy ;;
esac""",
    "findmnt": "echo /dev/sdy1",
    "blkid": """if [ "$2" = TYPE ]; then echo btrfs; else echo 0f3c2a1e-0000-4000-8000-$(echo "$5" | cksum | cut -c1-12 | tr ' ' 0); fi""",
    "unsquashfs": """cp -a "$WORLD/medium/." "$4" && printf '0\\n4\\n9\\n10\\n17\\n20\\n35\\n41\\n50\\n63\\n70\\n79\\n80\\n90\\n99\\n100\\n'""",
}


def install_cases(old):
    """The parity script's made-up computer: a plan refused (nothing runs, no sandbox), --check of a whole plan (with
    browsers and optional parts), --disks (Windows, NVMe, a disk too small, the live USB) and a whole install of the
    whole-disk plan, all three in the parity script's namespace sandbox (an empty /dev, tmpfs for /mnt, /run and
    /sys/firmware; every program a stand-in). Both versions spend 1 s of the install in udevadm settle's wait."""
    par = os.path.join(PKG, "install-system")
    ic = load(os.path.join(par, "parity", "compare.py"), "install_compare")
    w = mkdir("install", "w")
    nv = "/dev/nvmez0n1"
    spec = ic.world(state={"disks": [ic.windows_disk(), ic.disk(nv, size=1000 * ic.GB, table="gpt", tran="nvme", parts=[
        ic.part(nv, 1, 1, 512, ic.ESP, "vfat"), ic.part(nv, 2, 513, 100 * 1024, ic.LINUX, "ext4")]),
        ic.disk("/dev/vdx", size=20 * ic.GB), ic.disk("/dev/sdy", size=16 * ic.GB, table="dos", tran="usb", rm=True)]})
    ic.build(w, spec)
    env = base_env(PATH="/usr/bin:/bin", FAKEPATH=os.path.join(w, "bin"), WORLD=w)
    out = os.path.join(w, "out")
    os.makedirs(out)
    # the canned answers, from fake.py itself
    fake_env = dict(env, WORLD=w)
    for name, args in [("d", ["-J", "-b", "-d", "-o", "NAME,PATH,SIZE,MODEL,TRAN,RO,TYPE,RM"])] + \
            [(d["path"][5:], ["-J", "-b", "-o", "PATH,START,SIZE,FSTYPE,LABEL,PARTLABEL,PARTTYPE,PARTN,PTTYPE,MOUNTPOINTS,TYPE", d["path"]]) for d in spec["state"]["disks"]]:
        r = subprocess.run([os.path.join(w, "bin", "lsblk")] + args, env=fake_env, capture_output=True, check=True)
        write(os.path.join(out, "lsblk-" + name), r.stdout)
    medium = os.path.join(w, "medium")
    for path, text in ic.medium()["files"].items():
        write(os.path.join(medium, path), text)
    for path, dest in ic.medium()["links"].items():
        os.makedirs(os.path.dirname(os.path.join(medium, path)), exist_ok=True)
        os.symlink(dest, os.path.join(medium, path))
    for t in ic.TOOLS:
        p = os.path.join(w, "bin", t)
        os.remove(p)
        write(p, "#!/bin/sh\nPATH=/usr/bin:/bin\n" + INSTALL_STANDINS.get(t, "exit 0") + "\n", 0o755)
    sandbox = ["unshare", "-r", "-m", "--propagation", "private", "sh", os.path.join(par, "parity", "sandbox.sh"), w]
    py, rs = [PY, "-I", old["install-system"]], [os.path.join(BIN, "yggdrasil-install-system")]
    whole = json.dumps(ic.plan()).encode()
    choices = json.dumps(ic.plan(browsers=["firefox", "librewolf"], default_browser="librewolf", extras=["monitor"])).encode()
    refused = json.dumps(ic.plan(hostname="-x")).encode()
    cases = [
        Case("a plan refused (nothing runs)", py, rs, base_env(), stdin=refused),
        Case("--check, a plan with browsers and optional parts", py + ["--check"], rs + ["--check"], env, stdin=choices, sandbox=sandbox),
        Case("--disks (four made-up disks)", py + ["--disks"], rs + ["--disks"], env, stdin=b"", sandbox=sandbox),
        Case("a whole install (stand-ins; 1 s of it is udevadm settle's wait)", py, rs, env, stdin=whole, sandbox=sandbox, runs=ARGS.heavy_runs),
    ]
    return [bench(c) for c in cases], []


PROGRAMS = {
    "power": (power_cases, ["tune", "casefan", "gpufan", "powerlimit"], ["tune", "casefan", "gpufan", "powerlimit", "power"]),
    "settings-helpers": (settings_cases, ["account", "bootscreen", "game-fix", "gpu-run"], ["account", "bootscreen", "game-fix", "gpu-run"]),
    "appusage": (appusage_cases, ["labyrinth-appusage"], ["labyrinth-appusage"]),
    "crashwatch": (crashwatch_cases, ["labyrinth-crash-watch"], ["yggdrasil-crashwatch", "crashwatch-notice"]),
    "pkg-root": (pkgroot_cases, ["pkg-root"], ["pkg-root"]),
    "theme": (theme_cases, ["ygg-theme"], ["ygg-theme"]),
    "cursors": (cursors_cases, ["ygg-cursors"], ["ygg-cursors"]),
    "liki": (liki_cases, ["liki-build", "liki-icns", "liki-icon-import"], ["liki-build", "liki-icns", "liki-icon-import"]),
    "install-system": (install_cases, ["install-system"], ["yggdrasil-install-system"]),
}


# ---------------------------------------------------------------- output


def fmt_ms(ms):
    if ms >= 10000:
        return f"{ms / 1000:.1f} s"
    if ms >= 100:
        return f"{ms:.0f} ms"
    if ms >= 10:
        return f"{ms:.1f} ms"
    return f"{ms:.2f} ms"


def change(old, new, unit, less, more):
    if old == 0:
        return "n/a"
    p = (new - old) / old * 100
    d = abs(new - old)
    amount = fmt_ms(d) if unit == "ms" else f"{d:.1f} MB" if unit == "MB" else f"{d:.0f}"
    return f"{'−' if p < 0 else '+'}{abs(p):.0f} %, {amount} {less if new < old else more}"


def table(prog, res):
    lines = [f"### {prog}", "", "| case | metric | old (Python) | Rust | change |", "| --- | --- | --- | --- | --- |"]
    for c in res["cases"]:
        o, n = c["old"], c["new"]
        lines.append(f"| {c['label']} | wall (median; p10–p90) | {fmt_ms(o['wall_ms'])} ({fmt_ms(o['wall_p10_ms'])}–{fmt_ms(o['wall_p90_ms'])}) | "
                     f"{fmt_ms(n['wall_ms'])} ({fmt_ms(n['wall_p10_ms'])}–{fmt_ms(n['wall_p90_ms'])}) | {change(o['wall_ms'], n['wall_ms'], 'ms', 'faster', 'slower')} |")
        lines.append(f"| | CPU time | {fmt_ms(o['cpu_ms'])} | {fmt_ms(n['cpu_ms'])} | {change(o['cpu_ms'], n['cpu_ms'], 'ms', 'less', 'more')} |")
        lines.append(f"| | peak RSS | {o['rss_mb']:.1f} MB | {n['rss_mb']:.1f} MB | {change(o['rss_mb'], n['rss_mb'], 'MB', 'less', 'more')} |")
    for s in res["services"]:
        o, n = s["old"], s["new"]
        lines.append(f"| {s['label']} | CPU per minute | {fmt_ms(o['cpu_ms_per_min'])} | {fmt_ms(n['cpu_ms_per_min'])} | "
                     f"{change(o['cpu_ms_per_min'], n['cpu_ms_per_min'], 'ms', 'less', 'more')} |")
        lines.append(f"| | wake-ups per minute | {o['wakeups_per_min']:.0f} | {n['wakeups_per_min']:.0f} | "
                     f"{change(o['wakeups_per_min'], n['wakeups_per_min'], 'n', 'fewer', 'more')} |")
        lines.append(f"| | RSS ({o['processes']} / {n['processes']} processes) | {o['rss_mb']:.1f} MB | {n['rss_mb']:.1f} MB | "
                     f"{change(o['rss_mb'], n['rss_mb'], 'MB', 'less', 'more')} |")
        lines.append(f"| | PSS | {o['pss_mb']:.1f} MB | {n['pss_mb']:.1f} MB | {change(o['pss_mb'], n['pss_mb'], 'MB', 'less', 'more')} |")
        lines.append(f"| | start (CPU in the first 5 s) | {fmt_ms(o['start_cpu_ms'])} | {fmt_ms(n['start_cpu_ms'])} | "
                     f"{change(o['start_cpu_ms'], n['start_cpu_ms'], 'ms', 'less', 'more')} |")
    o, n = res["sizes"]["old"], res["sizes"]["new"]
    lines.append(f"| on disk | | {', '.join(f'{k} {v / 1024:.1f} KB' for k, v in o.items())} (scripts; the interpreter is shared) | "
                 f"{', '.join(f'{k} {v / 1024:.0f} KB' for k, v in n.items())} (binaries) | |")
    return "\n".join(lines)


def main():
    global ARGS, SCRATCH
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--only", help="comma-separated: " + ", ".join(PROGRAMS))
    ap.add_argument("--runs", type=int, default=30, help="runs of each version per case (default 30)")
    ap.add_argument("--heavy-runs", type=int, default=10, help="runs for the long cases (cursors, a boot picture; default 10)")
    ap.add_argument("--daemon-seconds", type=float, default=60, help="how long each service runs (default 60)")
    ap.add_argument("--labyrinth", default=os.path.join(os.path.dirname(REPO), "Labyrinth_OS"))
    ap.add_argument("--json", help="write every result to this file")
    ap.add_argument("-v", action="store_true")
    ARGS = ap.parse_args()
    try:
        os.nice(19 - os.nice(0))
    except OSError:
        pass
    only = ARGS.only.split(",") if ARGS.only else list(PROGRAMS)
    SCRATCH = tempfile.mkdtemp(prefix="bench-port-")
    build_launcher()
    results = {"machine": {"threads": os.cpu_count(), "kernel": os.uname().release, "python": sys.version.split()[0]}, "programs": {}}
    try:
        old, commits = old_scripts()
        results["old_sources"] = {n: {"repo": OLD[n][0], "commit": commits[n], "path": OLD[n][2]} for n in old}
        # the interpreter alone, for scale
        results["python_startup"] = bench(Case("python3 -c pass", [PY, "-c", "pass"], [PY, "-c", "pass"], base_env()))["old"]
        for prog in only:
            fn, olds, news = PROGRAMS[prog]
            if any(o not in old for o in olds):
                print(f"{prog}: skipped (no old version)")
                continue
            for n in news:
                if not os.path.exists(os.path.join(BIN, n)):
                    raise SystemExit(f"bench-port: no {n} in {BIN} (cargo build --release in its package)")
            print(f"{prog} ...", flush=True)
            t0 = time.monotonic()
            cases, svcs = fn(old)
            sizes = {"old": {o: os.path.getsize(old[o]) for o in olds}, "new": {n: os.path.getsize(os.path.join(BIN, n)) for n in news}}
            results["programs"][prog] = {"cases": cases, "services": svcs, "sizes": sizes, "seconds": time.monotonic() - t0}
            print(table(prog, results["programs"][prog]) + "\n", flush=True)
    finally:
        shutil.rmtree(SCRATCH, ignore_errors=True)
    print(f"python3 -c pass: {fmt_ms(results['python_startup']['wall_ms'])} wall, {fmt_ms(results['python_startup']['cpu_ms'])} CPU, "
          f"{results['python_startup']['rss_mb']:.1f} MB")
    if ARGS.json:
        with open(ARGS.json, "w") as f:
            json.dump(results, f, indent=1)
    return 0


if __name__ == "__main__":
    sys.exit(main())
