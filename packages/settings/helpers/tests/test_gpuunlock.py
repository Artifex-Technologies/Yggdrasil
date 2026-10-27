"""gpuunlock without a GPU: identity matching, the support table, the attempt markers, Secure Boot, the power hold, the GRUB entry
and arming with every outside program faked. Nothing on the system is read or touched (a fake /sys and /proc, a temporary state
folder)."""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
HELPER = os.path.join(HERE, "..", "gpuunlock")
POWERLIMIT = os.path.join(HERE, "..", "powerlimit")
TABLE = os.path.join(HERE, "..", "..", "data", "gpu-unlock-supported.json")
FRAGMENT = os.path.join(HERE, "..", "..", "data", "41_gpu_unlock")

MATT = {"bdf": "0000:01:00.0", "device": "1e07", "sub": ("1462", "3711"), "vbios": "90.02.17.00.3e", "bar0": "0x00000000de000000"}
ASUS = {"bdf": "0000:02:00.0", "device": "1e07", "sub": ("1043", "866a"), "vbios": "90.02.17.00.47", "bar0": "0x00000000fa000000"}
OPEN = "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  615.78.08  Release Build  (notroot@)  Fri Oct 23 2026\n"
CLOSED = "NVRM version: NVIDIA UNIX x86_64 Kernel Module  615.78.08  Release Build  (notroot@)  Fri Oct 23 2026\n"
GRUB_BASE = ("### BEGIN /etc/grub.d/10_linux ###\n"
             "menuentry 'Labyrinth Linux' --class labyrinth $menuentry_id_option 'gnulinux-simple-3118e509' {\n\tlinux /@/boot/vmlinuz\n}\n")
STATES = ("unsupported", "supported", "armed", "unlocked", "failed", "throttled", "secure-boot", "no-nvidia")


def load(path, name):
    loader = importlib.machinery.SourceFileLoader(name, path)
    spec = importlib.util.spec_from_loader(name, loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


def js(path):
    with open(path) as f:
        return json.load(f)


def text(path):
    with open(path) as f:
        return f.read()


def put(path, text, mode="w"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, mode) as f:
        f.write(text)


class Fake:
    """The outside programs: records each call and answers like the real one would."""

    def __init__(self, t):
        self.t, self.calls, self.fail = t, [], set()
        self.mkconfig_extra = ""

    def __call__(self, argv, timeout=60):
        self.calls.append(argv)
        name = argv[0]
        if name in self.fail:
            return 1, f"{name} failed"
        if name == "findmnt":
            return 0, "BD65-5893"
        if name == "snapper":
            return 0, "240"
        if name == "grub-mkconfig":
            env = dict(os.environ, YGGDRASIL_GPUUNLOCK_CONF=os.path.join(self.t.m.STATE_DIR, "loader.conf"))
            frag = subprocess.run(["sh", FRAGMENT], capture_output=True, text=True, env=env).stdout
            put(self.t.m.GRUB_CFG, self.mkconfig_extra + GRUB_BASE + frag)
            return 0, ""
        return 0, ""

    def ran(self, name):
        return [c for c in self.calls if c[0] == name]


class Base(unittest.TestCase):
    def setUp(self):
        self.enterContext(contextlib.redirect_stdout(io.StringIO()))
        self.enterContext(contextlib.redirect_stderr(io.StringIO()))
        self.d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.d)
        self.m = load(HELPER, "gpuunlock")
        m = self.m
        m.ROOT = os.path.join(self.d, "root")
        m.STATE_DIR = os.path.join(self.d, "state")
        m.TABLE = TABLE
        m.ESP = os.path.join(self.d, "esp")
        m.GRUB_CFG = os.path.join(self.d, "grub", "grub.cfg")
        m.GRUBENV = os.path.join(self.d, "grub", "grubenv")
        m.OLLAMA_PS = "http://127.0.0.1:9/api/ps"   # nothing listens there: no model loaded
        put(m.GRUB_CFG, GRUB_BASE)
        put(m.GRUBENV, "# GRUB Environment Block\n")
        self.fake = Fake(self)
        m.run = self.fake
        m.fstype_of = lambda path: "vfat"
        m.is_root = lambda: True
        self.boot("aaaa-1111")
        self.card(MATT)

    def boot(self, ident):
        put(os.path.join(self.m.ROOT, "proc/sys/kernel/random/boot_id"), ident + "\n")

    def card(self, c, driver=OPEN, loaded=True):
        root = self.m.ROOT
        shutil.rmtree(os.path.join(root, "sys"), ignore_errors=True)
        shutil.rmtree(os.path.join(root, "proc/driver"), ignore_errors=True)
        dev = os.path.join(root, "sys/bus/pci/devices", c["bdf"])
        for k, v in {"vendor": "0x10de", "device": "0x" + c["device"], "subsystem_vendor": "0x" + c["sub"][0], "subsystem_device": "0x" + c["sub"][1],
                     "class": "0x030000", "resource": f"{c['bar0']} 0x00000000deffffff 0x0000000000040200\n"}.items():
            put(os.path.join(dev, k), v + "\n")
        if loaded:
            put(os.path.join(root, "proc/driver/nvidia/version"), driver)
            put(os.path.join(root, f"proc/driver/nvidia/gpus/{c['bdf']}/information"),
                f"Model: \t\t NVIDIA GeForce RTX 2080 Ti\nGPU UUID: \t GPU-0000\nVideo BIOS: \t {c['vbios']}\nBus Location: \t {c['bdf']}\n")

    def loader(self):
        put(os.path.join(self.m.STATE_DIR, "loaders", "tu102_experiment_v4.efi"), "MZ fake loader")

    def state(self, name):
        return os.path.join(self.m.STATE_DIR, name)

    def written(self):
        return sorted(os.listdir(self.m.STATE_DIR)) if os.path.isdir(self.m.STATE_DIR) else []


class Identity(Base):
    def test_matts_card_is_not_supported_and_the_reason_names_it(self):
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertEqual(s["reason"], "No tested entry for MSI 1462:3711 with VBIOS 90.02.17.00.3E (tested: ASUS 1043:866a, VBIOS 90.02.17.00.47)")
        self.assertEqual(s["gpu"]["bar0"], "0xde000000")
        self.assertEqual(s["gpu"]["driver_series"], "open")
        self.assertIsNone(s["arm_blocker"])

    def test_the_tested_asus_card_is_supported(self):
        self.card(ASUS)
        self.assertEqual(self.m.status()["state"], "supported")

    def test_the_same_card_on_the_other_driver_is_not(self):
        self.card(ASUS, driver=CLOSED)
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertIn("another NVIDIA driver series", s["reason"])

    def test_no_driver_means_no_firmware_version_and_no_offer(self):
        self.card(ASUS, loaded=False)
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertIn("driver is not loaded", s["reason"])

    def test_no_nvidia_card(self):
        shutil.rmtree(os.path.join(self.m.ROOT, "sys"))
        self.assertEqual(self.m.status()["state"], "no-nvidia")

    def test_a_wildcard_or_a_missing_field_makes_the_table_refused(self):
        self.card(ASUS)
        good = js(TABLE)
        for field, bad in [("subsystem", "1043:*"), ("vbios", "90.02.17.00.*"), ("device", "10de:1e0?"), ("subsystem", ""),
                           ("driver_series", "any"), ("evidence", ""), ("loader", "../x.efi")]:
            t = json.loads(json.dumps(good))
            t["entries"][0][field] = bad
            path = os.path.join(self.d, "table.json")
            put(path, json.dumps(t))
            self.m.TABLE = path
            s = self.m.status()
            self.assertEqual(s["state"], "unsupported", f"{field}={bad!r}")
            self.assertIn("support table is not valid", s["reason"])

    def test_an_entry_no_longer_experimental_is_not_offered(self):
        self.card(ASUS)
        t = js(TABLE)
        t["entries"][0]["status"] = "measured-no-benefit"
        path = os.path.join(self.d, "table.json")
        put(path, json.dumps(t))
        self.m.TABLE = path
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertIn("marked measured-no-benefit", s["reason"])

    def test_secure_boot_on_refuses_and_arm_writes_nothing(self):
        self.card(ASUS)
        self.loader()
        put(os.path.join(self.m.ROOT, self.m.SECURE_BOOT_VAR.lstrip("/")), b"\x06\x00\x00\x00\x01", "wb")
        self.assertEqual(self.m.status()["state"], "secure-boot")
        before = self.written()
        self.assertEqual(self.m.cmd_arm(), 2)
        self.assertEqual(self.written(), before)
        self.assertEqual(self.fake.ran("snapper"), [])

    def test_status_json_has_the_same_shape_in_every_state(self):
        keys = {"gpu", "secure_boot", "state", "reason", "arm_blocker", "last_attempt", "since", "ratio", "power_hold", "log"}
        seen = set()
        def check():
            s = json.loads(json.dumps(self.m.status()))
            self.assertEqual(set(s), keys)
            self.assertIn(s["state"], STATES)
            self.assertIsInstance(s["reason"], str)
            self.assertIsInstance(s["power_hold"], bool)
            seen.add(s["state"])
        check()                                   # unsupported
        self.card(ASUS)
        check()                                   # supported
        put(self.state("armed"), json.dumps({"boot_id": "aaaa-1111"}))
        check()                                   # armed
        os.remove(self.state("armed"))
        put(self.state("state.json"), json.dumps({"state": "unlocked", "boot_id": "aaaa-1111", "since": "x", "ratio": 2.0}))
        check()                                   # unlocked
        put(self.state("state.json"), json.dumps({"state": "throttled", "boot_id": "aaaa-1111"}))
        check()                                   # throttled
        put(self.state("state.json"), json.dumps({"state": "failed", "why": "x"}))
        check()                                   # failed
        os.remove(self.state("state.json"))
        put(os.path.join(self.m.ROOT, self.m.SECURE_BOOT_VAR.lstrip("/")), b"\x06\x00\x00\x00\x01", "wb")
        check()                                   # secure-boot
        shutil.rmtree(os.path.join(self.m.ROOT, "sys"))
        check()                                   # no-nvidia
        self.assertEqual(seen, set(STATES))

    def test_an_unlock_from_an_earlier_start_is_gone(self):
        self.card(ASUS)
        put(self.state("state.json"), json.dumps({"state": "unlocked", "boot_id": "old-boot", "since": "x", "ratio": 2.0}))
        s = self.m.status()
        self.assertEqual(s["state"], "supported", "the unlock does not survive a restart")
        self.assertIsNone(s["ratio"])


class OneShot(Base):
    def test_btrfs_without_an_env_block_blocks_arming(self):
        self.card(ASUS)
        self.loader()
        self.m.fstype_of = lambda path: "btrfs"
        s = self.m.status()
        self.assertEqual(s["state"], "supported")
        self.assertIn("repeat at every restart", s["arm_blocker"])
        self.assertEqual(self.m.cmd_arm(), 2)
        self.assertEqual(self.fake.ran("snapper"), [], "refused before anything was done")

    def test_an_env_block_or_another_filesystem_is_fine(self):
        self.m.fstype_of = lambda path: "btrfs"
        put(self.m.GRUBENV, "# GRUB Environment Block\nenv_block=512+1\n")
        self.assertIsNone(self.m.one_shot_problem())
        self.m.fstype_of = lambda path: "ext4"
        put(self.m.GRUBENV, "# GRUB Environment Block\n")
        self.assertIsNone(self.m.one_shot_problem())
        self.m.fstype_of = lambda path: ""
        self.assertIn("unknown filesystem", self.m.one_shot_problem(), "not knowing is not safe")

    def test_no_boot_program_built_blocks_arming(self):
        self.card(ASUS)
        self.assertIn("has not been built", self.m.status()["arm_blocker"])
        self.assertEqual(self.m.cmd_arm(), 2)


class Arming(Base):
    def setUp(self):
        super().setUp()
        self.card(ASUS)
        self.loader()

    def test_arm_snapshots_adds_the_entry_holds_power_and_picks_it_once(self):
        self.assertEqual(self.m.cmd_arm(), 0)
        self.assertEqual(self.fake.ran("snapper")[0][-1], "before GPU unlock")
        cfg = text(self.m.GRUB_CFG)
        self.assertEqual(cfg.count("menuentry 'Graphics card unlock"), 1)
        self.assertIn("chainloader /EFI/yggdrasil/tu102_experiment_v4.efi bdf=0000:02:00.0 bar0=0xfa000000", cfg)
        self.assertEqual(self.m.first_entry_id(cfg), "gnulinux-simple-3118e509", "the normal start stays first")
        self.assertTrue(os.path.isfile(os.path.join(self.m.ESP, "EFI/yggdrasil/tu102_experiment_v4.efi")))
        self.assertEqual(self.fake.ran("grub-reboot"), [["grub-reboot", "yggdrasil-gpu-unlock"]])
        self.assertTrue(os.path.exists(self.state("power-hold")))
        a = js(self.state("armed"))
        self.assertEqual(a["boot_id"], "aaaa-1111")
        self.assertEqual(oct(os.stat(self.state("armed")).st_mode & 0o777), "0o600")
        self.assertFalse(any(c[:2] == ["grub-editenv", self.m.GRUBENV] and "set" in c for c in self.fake.calls), "GRUB_DEFAULT is never touched")
        self.assertEqual(self.m.status()["state"], "armed")

    def test_arm_stops_and_puts_back_if_the_first_entry_changes(self):
        self.fake.mkconfig_extra = "menuentry 'Something else' $menuentry_id_option 'other' {\n}\n"
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertFalse(os.path.exists(self.state("armed")))
        self.assertFalse(os.path.exists(self.state("loader.conf")))
        self.assertFalse(os.path.exists(self.state("power-hold")))
        self.assertFalse(os.path.exists(os.path.join(self.m.ESP, "EFI/yggdrasil/tu102_experiment_v4.efi")))
        self.assertEqual(self.fake.ran("grub-reboot"), [])

    def test_arm_puts_back_when_grub_reboot_fails(self):
        self.fake.fail.add("grub-reboot")
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertEqual(self.written(), ["loaders", "log"])
        self.assertNotIn("Graphics card unlock", text(self.m.GRUB_CFG))

    def test_a_failed_snapshot_stops_before_anything_else(self):
        self.fake.fail.add("snapper")
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertEqual(self.fake.ran("grub-mkconfig"), [])
        self.assertFalse(os.path.exists(self.state("armed")))

    def test_a_busy_card_is_not_armed(self):
        busy = Fake(self)
        def run(argv, timeout=60):
            if argv[0] == "nvidia-smi":
                return 0, "4242, ollama"
            return busy(argv, timeout)
        self.m.run = run
        self.assertEqual(self.m.cmd_arm(), 2)
        self.assertEqual(busy.ran("snapper"), [])

    def test_not_root_does_nothing(self):
        self.m.is_root = lambda: False
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertEqual(self.fake.calls, [])

    def test_disarm_takes_it_all_back(self):
        self.m.cmd_arm()
        self.assertEqual(self.m.cmd_disarm(), 0)
        self.assertEqual(self.written(), ["loaders", "log"])
        self.assertNotIn("Graphics card unlock", text(self.m.GRUB_CFG))
        self.assertIn(["grub-editenv", self.m.GRUBENV, "unset", "next_entry"], self.fake.calls)
        self.assertEqual(self.m.status()["state"], "supported")


class Markers(Base):
    def setUp(self):
        super().setUp()
        self.card(ASUS)
        self.loader()
        self.m.cmd_arm()

    def test_the_next_start_is_the_attempt(self):
        self.boot("bbbb-2222")
        self.m.cmd_boot()
        self.assertFalse(os.path.exists(self.state("armed")))
        self.assertEqual(js(self.state("trying"))["boot_id"], "bbbb-2222")
        self.assertEqual(self.m.status()["state"], "armed", "still pending until checked")

    def test_boot_again_in_the_arming_start_changes_nothing(self):
        self.m.cmd_boot()
        self.assertTrue(os.path.exists(self.state("armed")))
        self.assertFalse(os.path.exists(self.state("trying")))

    def test_an_attempt_that_never_reached_the_desktop_is_a_failure(self):
        self.boot("bbbb-2222")
        self.m.cmd_boot()           # the attempt starts ... and hangs: verify never runs
        self.boot("cccc-3333")
        self.m.cmd_boot()           # the power button, a normal start
        s = self.m.status()
        self.assertEqual(s["state"], "failed")
        self.assertIn("did not reach the desktop", s["reason"])
        self.assertFalse(os.path.exists(self.state("trying")))
        self.assertEqual(self.m.cmd_arm(), 2, "no arming until re-enabled")
        self.m.cmd_verify()         # the desktop of that normal start takes the entry out of the menu
        self.assertFalse(os.path.exists(self.state("loader.conf")))
        self.assertNotIn("Graphics card unlock", text(self.m.GRUB_CFG))
        self.assertEqual(self.m.cmd_reenable(), 0)
        self.assertEqual(self.m.status()["state"], "supported")

    def test_verify_in_the_attempt_fails_until_the_check_exists(self):
        self.boot("bbbb-2222")
        self.m.cmd_boot()
        self.m.cmd_verify()
        s = self.m.status()
        self.assertEqual(s["state"], "failed")
        self.assertIn("not built yet", s["reason"])
        self.assertFalse(os.path.exists(self.state("loader.conf")))
        self.assertTrue(os.path.exists(self.state("power-hold")), "the hold stays until released")

    def test_a_confirmed_unlock_shows_until_the_next_start(self):
        self.m.check_unlock = lambda: (True, 2.0, "")
        self.m.thermal_watch = lambda: False
        self.boot("bbbb-2222")
        self.m.cmd_boot()
        self.m.cmd_verify()
        s = self.m.status()
        self.assertEqual(s["state"], "unlocked")
        self.assertEqual(s["ratio"], 2.0)
        self.boot("cccc-3333")
        self.assertEqual(self.m.status()["state"], "supported")

    def test_verify_while_still_armed_waits(self):
        self.m.cmd_verify()
        self.assertTrue(os.path.exists(self.state("armed")))
        self.assertTrue(os.path.exists(self.state("loader.conf")))

    def test_release_waits_for_a_pending_attempt(self):
        self.assertEqual(self.m.cmd_release(), 2)
        self.assertTrue(os.path.exists(self.state("power-hold")))


class Heat(Base):
    def test_hot_or_slowing_down_holds_the_session_lower(self):
        self.assertFalse(self.m.thermal_verdict([(70, False), (84, False)]))
        self.assertTrue(self.m.thermal_verdict([(70, False), (86, False)]))
        self.assertTrue(self.m.thermal_verdict([(60, True)]))
        self.m.thermal_sample = lambda: (88, False)
        self.assertTrue(self.m.thermal_watch(minutes=1, every=0))
        self.assertEqual(js(self.state("power-hold"))["watts"], 200)
        self.assertEqual(js(self.state("state.json"))["state"], "throttled")


class PowerHold(unittest.TestCase):
    def setUp(self):
        self.p = load(POWERLIMIT, "powerlimit")
        self.d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.d)
        self.hold = os.path.join(self.d, "power-hold")

    def test_performance_may_use_290_without_a_hold(self):
        self.assertIsNone(self.p.hold_cap(260, self.hold))
        self.assertEqual(self.p.gpu_target(290, 260, 130, 290, None), 290)

    def test_a_hold_keeps_every_mode_at_the_default(self):
        put(self.hold, json.dumps({"watts": None}))
        cap = self.p.hold_cap(260, self.hold)
        self.assertEqual(cap, 260)
        self.assertEqual(self.p.gpu_target(290, 260, 130, 290, cap), 260)
        self.assertEqual(self.p.gpu_target(200, 260, 130, 290, cap), 200, "a lower mode limit stays")
        self.assertEqual(self.p.gpu_target(None, 260, 130, 290, cap), 260)

    def test_the_heat_cap_and_a_broken_file_still_hold(self):
        put(self.hold, json.dumps({"watts": 200}))
        self.assertEqual(self.p.hold_cap(260, self.hold), 200)
        put(self.hold, json.dumps({"watts": 400}))
        self.assertEqual(self.p.hold_cap(260, self.hold), 260, "a hold never raises")
        put(self.hold, "not json")
        self.assertEqual(self.p.hold_cap(260, self.hold), 260)


class Fragment(unittest.TestCase):
    def run_it(self, conf_text):
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d)
        conf = os.path.join(d, "loader.conf")
        if conf_text is not None:
            put(conf, conf_text)
        p = subprocess.run(["sh", FRAGMENT], capture_output=True, text=True, env=dict(os.environ, YGGDRASIL_GPUUNLOCK_CONF=conf))
        self.assertEqual(p.returncode, 0, "never fails grub-mkconfig")
        return p.stdout

    def test_nothing_without_an_attempt(self):
        self.assertEqual(self.run_it(None), "")

    def test_one_entry_with_one(self):
        out = self.run_it("ESP_UUID=BD65-5893\nLOADER=tu102_experiment_v4.efi\nBDF=0000:01:00.0\nBAR0=0xde000000\n")
        self.assertEqual(out.count("menuentry "), 1)
        self.assertIn("--id yggdrasil-gpu-unlock", out)
        self.assertIn("search --no-floppy --fs-uuid --set=root BD65-5893", out)

    def test_a_value_of_the_wrong_shape_prints_nothing(self):
        for bad in ["LOADER=x.efi; reboot", "BDF=01:00.0", "BAR0=de000000", "ESP_UUID=$(id)"]:
            conf = "ESP_UUID=BD65-5893\nLOADER=tu102_experiment_v4.efi\nBDF=0000:01:00.0\nBAR0=0xde000000\n"
            key = bad.split("=")[0]
            conf = "\n".join(l for l in conf.splitlines() if not l.startswith(key + "=")) + "\n" + bad + "\n"
            self.assertEqual(self.run_it(conf), "", bad)


if __name__ == "__main__":
    unittest.main()
