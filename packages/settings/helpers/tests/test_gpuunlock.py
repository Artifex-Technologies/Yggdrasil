"""gpuunlock without a GPU: identity matching, the support table, what blocks arming, the one-try boot entry, arming with every
outside program faked, the attempt markers across starts, the module's own result, the heat watch, the power hold and the GRUB
fragment. Nothing on the system is read or touched (a fake /sys, /proc and /etc, a temporary state folder)."""
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
FIRST = ("menuentry 'Labyrinth Linux' --class labyrinth --class gnu-linux $menuentry_id_option 'gnulinux-simple-3118e509' {\n"
         "\tload_video\n\tinsmod btrfs\n\tsearch --no-floppy --fs-uuid --set=root 3118e509\n"
         "\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw rootflags=subvol=@ quiet splash\n"
         "\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n}\n")
GRUB_BASE = "### BEGIN /etc/grub.d/10_linux ###\n" + FIRST + "submenu 'Advanced options' $menuentry_id_option 'gnulinux-advanced' {\n}\n"
UNLOCKED_LOG = ("turing_unlock: entry: plm=0xffffff8f ss0=0x2300000 ss1=0x6 wpr2=0x0:0x1ffffe00 boot=0x0\n"
                "turing_unlock: unlock result: ss0=0x88888888 ss1=0x8 readout=0x0 (want ss0=0x88888888 ss1=0x8 readout=0)\n"
                "turing_unlock: TU102 FEAT UNLOCKED\npci 0000:01:00.0: turing_unlock: unlocked\n")
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


def put(path, content, mode="w"):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, mode) as f:
        f.write(content)


class Fake:
    """The outside programs: records each call and answers like the real one would."""

    def __init__(self, t):
        self.t, self.calls, self.fail = t, [], set()
        self.mkconfig_extra = ""
        self.kernel_log = ""
        self.compute = ""

    def __call__(self, argv, timeout=60):
        self.calls.append(argv)
        name = argv[0]
        if name in self.fail:
            return 1, f"{name} failed"
        if name == "modinfo":
            return 0, "/usr/lib/modules/7.3.0-4-cachyos/updates/dkms/turing_unlock.ko.zst"
        if name == "snapper":
            return 0, "240"
        if name == "journalctl":
            return 0, self.kernel_log
        if name == "nvidia-smi":
            return 0, self.compute
        if name == "grub-mkconfig":
            env = dict(os.environ, YGGDRASIL_GPUUNLOCK_ENTRY=os.path.join(self.t.m.STATE_DIR, "entry.cfg"))
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
        m.GRUB_CFG = os.path.join(self.d, "grub", "grub.cfg")
        m.OLLAMA_PS = "http://127.0.0.1:9/api/ps"   # nothing listens there: no model loaded
        put(m.GRUB_CFG, GRUB_BASE)
        self.fake = Fake(self)
        m.run = self.fake
        m.is_root = lambda: True
        self.root("etc/default/grub", "GRUB_DEFAULT=0\nGRUB_TIMEOUT=2\nGRUB_TIMEOUT_STYLE=menu\n")
        self.root("usr/lib/firmware/turing-unlock/610.43.03/v67-tu102-610.43.03.bin", "fw")
        self.root("etc/mkinitcpio.conf.d/zz-turing-unlock.conf", "MODULES+=(turing_unlock)\n")
        self.boot("aaaa-1111", "root=UUID=3118e509 rw quiet splash")
        self.card(MATT)

    def root(self, rel, content, mode="w"):
        put(os.path.join(self.m.ROOT, rel), content, mode)

    def boot(self, ident, cmdline):
        self.root("proc/sys/kernel/random/boot_id", ident + "\n")
        self.root("proc/cmdline", cmdline + "\n")

    def card(self, c, driver=OPEN, loaded=True):
        root = self.m.ROOT
        shutil.rmtree(os.path.join(root, "sys"), ignore_errors=True)
        shutil.rmtree(os.path.join(root, "proc/driver"), ignore_errors=True)
        dev = os.path.join(root, "sys/bus/pci/devices", c["bdf"])
        for k, v in {"vendor": "0x10de", "device": "0x" + c["device"], "subsystem_vendor": "0x" + c["sub"][0], "subsystem_device": "0x" + c["sub"][1],
                     "class": "0x030000", "resource": f"{c['bar0']} 0x00000000deffffff 0x0000000000040200\n"}.items():
            put(os.path.join(dev, k), v + "\n")
        if loaded:
            self.root("proc/driver/nvidia/version", driver)
            self.root(f"proc/driver/nvidia/gpus/{c['bdf']}/information",
                      f"Model: \t\t NVIDIA GeForce RTX 2080 Ti\nGPU UUID: \t GPU-0000\nVideo BIOS: \t {c['vbios']}\nBus Location: \t {c['bdf']}\n")

    def state(self, name):
        return os.path.join(self.m.STATE_DIR, name)

    def written(self):
        return sorted(os.listdir(self.m.STATE_DIR)) if os.path.isdir(self.m.STATE_DIR) else []

    def table(self, change):
        t = js(TABLE)
        change(t["entries"][0])
        path = os.path.join(self.d, "table.json")
        put(path, json.dumps(t))
        self.m.TABLE = path


class Identity(Base):
    def test_matts_card_is_in_the_table(self):
        s = self.m.status()
        self.assertEqual(s["state"], "supported")
        self.assertIsNone(s["arm_blocker"])
        self.assertEqual((s["gpu"]["bar0"], s["gpu"]["driver_series"], s["gpu"]["vbios"]), ("0xde000000", "open", "90.02.17.00.3E"))

    def test_another_board_is_not_and_the_reason_names_it(self):
        self.card(ASUS)
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertEqual(s["reason"], "No tested entry for ASUS 1043:866a with VBIOS 90.02.17.00.47 (tested: MSI 1462:3711, VBIOS 90.02.17.00.3E)")

    def test_the_same_card_on_the_other_driver_is_not(self):
        self.card(MATT, driver=CLOSED)
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertIn("another NVIDIA driver series", s["reason"])

    def test_no_driver_means_no_firmware_version_and_no_offer(self):
        self.card(MATT, loaded=False)
        self.assertIn("driver is not loaded", self.m.status()["reason"])

    def test_no_nvidia_card(self):
        shutil.rmtree(os.path.join(self.m.ROOT, "sys"))
        self.assertEqual(self.m.status()["state"], "no-nvidia")

    def test_a_wildcard_a_missing_field_or_another_method_makes_the_table_refused(self):
        for field, bad in [("subsystem", "1462:*"), ("vbios", "90.02.17.00.*"), ("device", "10de:1e0?"), ("subsystem", ""),
                           ("driver_series", "any"), ("evidence", ""), ("method", "turing-unlock-efi"), ("method", None)]:
            self.table(lambda e: e.__setitem__(field, bad))
            s = self.m.status()
            self.assertEqual(s["state"], "unsupported", f"{field}={bad!r}")
            self.assertIn("support table is not valid", s["reason"])

    def test_an_entry_no_longer_experimental_is_not_offered(self):
        self.table(lambda e: e.__setitem__("status", "measured-no-benefit"))
        s = self.m.status()
        self.assertEqual(s["state"], "unsupported")
        self.assertIn("marked measured-no-benefit", s["reason"])

    def test_secure_boot_on_refuses_and_arm_writes_nothing(self):
        self.root(self.m.SECURE_BOOT_VAR.lstrip("/"), b"\x06\x00\x00\x00\x01", "wb")
        self.assertEqual(self.m.status()["state"], "secure-boot")
        self.assertEqual(self.m.cmd_arm(), 2)
        self.assertEqual(self.written(), [])
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
        self.root(self.m.SECURE_BOOT_VAR.lstrip("/"), b"\x06\x00\x00\x00\x01", "wb")
        check()                                   # secure-boot
        self.card(ASUS)
        check()                                   # unsupported
        shutil.rmtree(os.path.join(self.m.ROOT, "sys"))
        check()                                   # no-nvidia
        self.assertEqual(seen, set(STATES))

    def test_an_unlock_from_an_earlier_start_is_gone(self):
        put(self.state("state.json"), json.dumps({"state": "unlocked", "boot_id": "old-boot", "since": "x", "ratio": 2.0}))
        s = self.m.status()
        self.assertEqual(s["state"], "supported", "the unlock does not survive a restart")
        self.assertIsNone(s["ratio"])


class Blockers(Base):
    def test_the_module_its_firmware_and_its_start_up_hook_must_be_installed(self):
        self.fake.fail.add("modinfo")
        self.assertIn("module is not installed", self.m.status()["arm_blocker"])
        self.assertEqual(self.m.cmd_arm(), 2)
        self.assertEqual(self.fake.ran("snapper"), [], "refused before anything was done")
        self.fake.fail.clear()
        shutil.rmtree(os.path.join(self.m.ROOT, "usr/lib/firmware/turing-unlock"))
        self.assertIn("firmware is not installed", self.m.status()["arm_blocker"])
        self.root("usr/lib/firmware/turing-unlock/610.43.03/x", "fw")
        os.remove(os.path.join(self.m.ROOT, "etc/mkinitcpio.conf.d/zz-turing-unlock.conf"))
        self.assertIn("start-up image", self.m.status()["arm_blocker"])

    def test_a_hidden_boot_menu_blocks_arming(self):
        for grub in ["GRUB_TIMEOUT=2\nGRUB_TIMEOUT_STYLE=hidden\n", "GRUB_TIMEOUT=0\n", "GRUB_TIMEOUT='0'\nGRUB_TIMEOUT_STYLE=menu\n"]:
            self.root("etc/default/grub", grub)
            self.assertIn("boot menu is hidden", self.m.status()["arm_blocker"], grub)
        self.root("etc/default/grub", "GRUB_TIMEOUT=5\n")
        self.assertIsNone(self.m.status()["arm_blocker"])


class Entry(Base):
    def test_the_first_entry_copied_with_the_option(self):
        e = self.m.one_try_entry(GRUB_BASE, "0000:01:00.0")
        self.assertTrue(e.startswith("menuentry 'Labyrinth Linux, graphics card unlock (one try)' --class gnu-linux --class os --id yggdrasil-gpu-unlock {\n"))
        self.assertIn("\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw rootflags=subvol=@ quiet splash turing_unlock.targets=0000:01:00.0\n", e)
        self.assertIn("\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n", e)
        self.assertTrue(e.endswith("\n}\n"))
        self.assertNotIn("Advanced", e)

    def test_refused_when_it_is_not_one_plain_linux_entry(self):
        self.assertIsNone(self.m.one_try_entry(GRUB_BASE, "01:00.0"), "a full PCI address only")
        self.assertIsNone(self.m.one_try_entry(GRUB_BASE.replace("quiet splash", "quiet splash turing_unlock.targets=all"), "0000:01:00.0"))
        self.assertIsNone(self.m.one_try_entry(GRUB_BASE.replace("\tlinux\t", "\techo\t"), "0000:01:00.0"))
        self.assertIsNone(self.m.one_try_entry("submenu 'x' {\n}\n", "0000:01:00.0"))

    def test_the_option_on_the_command_line(self):
        self.assertEqual(self.m.unlock_requested("root=UUID=x rw quiet turing_unlock.targets=0000:01:00.0"), "0000:01:00.0")
        self.assertIsNone(self.m.unlock_requested("root=UUID=x rw quiet"))
        self.assertIsNone(self.m.unlock_requested("xturing_unlock.targets=all"))


class Arming(Base):
    def test_arm_snapshots_adds_the_entry_and_holds_power(self):
        self.assertEqual(self.m.cmd_arm(), 0)
        self.assertEqual(self.fake.ran("snapper")[0][-1], "before GPU unlock")
        cfg = text(self.m.GRUB_CFG)
        self.assertEqual(cfg.count("menuentry 'Labyrinth Linux, graphics card unlock (one try)'"), 1)
        self.assertIn("turing_unlock.targets=0000:01:00.0\n", cfg)
        self.assertEqual(self.m.first_entry_id(cfg), "gnulinux-simple-3118e509", "the normal start stays first and default")
        self.assertEqual(self.fake.ran("grub-reboot"), [], "nothing picks the entry but Matt")
        self.assertEqual(self.fake.ran("grub-editenv"), [])
        self.assertTrue(os.path.exists(self.state("power-hold")))
        self.assertEqual(oct(os.stat(self.state("armed")).st_mode & 0o777), "0o600")
        self.assertEqual(self.m.status()["state"], "armed")

    def test_arm_stops_and_puts_back_if_the_first_entry_changes(self):
        self.fake.mkconfig_extra = "menuentry 'Something else' $menuentry_id_option 'other' {\n}\n"
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertEqual(self.written(), ["log"])

    def test_a_failed_snapshot_stops_before_the_menu_is_touched(self):
        self.fake.fail.add("snapper")
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertEqual(self.fake.ran("grub-mkconfig"), [])
        self.assertEqual(self.written(), [])

    def test_a_busy_card_is_not_armed(self):
        self.fake.compute = "4242, ollama"
        self.assertEqual(self.m.cmd_arm(), 2)
        self.assertEqual(self.fake.ran("snapper"), [])

    def test_not_root_does_nothing(self):
        self.m.is_root = lambda: False
        self.assertEqual(self.m.cmd_arm(), 1)
        self.assertEqual(self.fake.calls, [])

    def test_disarm_takes_it_all_back(self):
        self.m.cmd_arm()
        self.assertEqual(self.m.cmd_disarm(), 0)
        self.assertEqual(self.written(), ["log"])
        self.assertNotIn("graphics card unlock", text(self.m.GRUB_CFG))
        self.assertEqual(self.m.status()["state"], "supported")


class Markers(Base):
    def setUp(self):
        super().setUp()
        self.m.cmd_arm()

    def picked(self, ident):
        self.boot(ident, "root=UUID=3118e509 rw quiet splash turing_unlock.targets=0000:01:00.0")

    def test_a_normal_start_while_armed_did_not_try(self):
        self.boot("bbbb-2222", "root=UUID=3118e509 rw quiet splash")
        self.m.cmd_boot()
        self.m.cmd_verify()
        self.assertTrue(os.path.exists(self.state("armed")))
        self.assertFalse(os.path.exists(self.state("trying")))
        self.assertIn("graphics card unlock", text(self.m.GRUB_CFG), "the entry stays offered")

    def test_picking_the_entry_is_the_attempt(self):
        self.picked("bbbb-2222")
        self.m.cmd_boot()
        self.assertFalse(os.path.exists(self.state("armed")))
        self.assertEqual(js(self.state("trying"))["boot_id"], "bbbb-2222")
        self.assertEqual(self.m.status()["state"], "armed", "still pending until checked")

    def test_an_attempt_that_never_reached_the_desktop_is_a_failure(self):
        self.picked("bbbb-2222")
        self.m.cmd_boot()           # the attempt starts ... and hangs: verify never runs
        self.boot("cccc-3333", "root=UUID=3118e509 rw quiet splash")
        self.m.cmd_boot()           # the power button, the normal entry
        s = self.m.status()
        self.assertEqual(s["state"], "failed")
        self.assertIn("did not reach the desktop", s["reason"])
        self.assertEqual(self.m.cmd_arm(), 2, "no arming until re-enabled")
        self.m.cmd_verify()         # that normal start's desktop takes the entry out of the menu
        self.assertFalse(os.path.exists(self.state("entry.cfg")))
        self.assertNotIn("graphics card unlock", text(self.m.GRUB_CFG))
        self.assertEqual(self.m.cmd_reenable(), 0)
        self.assertEqual(self.m.status()["state"], "supported")

    def test_the_modules_own_failure_is_the_reason(self):
        self.picked("bbbb-2222")
        self.m.cmd_boot()
        self.fake.kernel_log = "turing_unlock: failure unlock.c:314 gsp_fwsec_frts error=-5\npci 0000:01:00.0: turing_unlock: unlock failed (-5)\n"
        self.m.cmd_verify()
        s = self.m.status()
        self.assertEqual(s["state"], "failed")
        self.assertIn("The module reported: turing_unlock: failure unlock.c:314", s["reason"])
        self.assertFalse(os.path.exists(self.state("entry.cfg")), "one try: the entry is gone")
        self.assertTrue(os.path.exists(self.state("power-hold")), "the hold stays until released")

    def test_a_module_that_did_not_run(self):
        self.picked("bbbb-2222")
        self.m.cmd_boot()
        self.m.cmd_verify()
        self.assertIn("did not run at this start", self.m.status()["reason"])

    def test_unlocked_by_the_module_but_not_yet_confirmed(self):
        self.picked("bbbb-2222")
        self.m.cmd_boot()
        self.fake.kernel_log = UNLOCKED_LOG
        self.m.cmd_verify()
        s = self.m.status()
        self.assertEqual(s["state"], "failed", "never 'Unlocked' without the correctness check and benchmark")
        self.assertIn("module reports the unlock", s["reason"])

    def test_a_confirmed_unlock_shows_until_the_next_start(self):
        self.m.check_unlock = lambda: (True, 2.0, "")
        self.m.thermal_watch = lambda: False
        self.picked("bbbb-2222")
        self.m.cmd_boot()
        self.m.cmd_verify()
        s = self.m.status()
        self.assertEqual((s["state"], s["ratio"]), ("unlocked", 2.0))
        self.assertNotIn("graphics card unlock", text(self.m.GRUB_CFG), "one try: the entry is gone")
        self.boot("cccc-3333", "root=UUID=3118e509 rw quiet splash")
        self.assertEqual(self.m.status()["state"], "supported")

    def test_release_waits_for_a_pending_attempt(self):
        self.assertEqual(self.m.cmd_release(), 2)
        self.assertTrue(os.path.exists(self.state("power-hold")))


class ModuleLog(Base):
    def test_reading_the_modules_result(self):
        self.assertEqual(self.m.module_result(UNLOCKED_LOG), (True, "The module reports the card unlocked"))
        ok, why = self.m.module_result('turing_unlock: no supported card at "0000:02:00.0"\n')
        self.assertFalse(ok)
        self.assertIn("no supported card", why)
        ok, why = self.m.module_result("pci 0000:01:00.0: turing_unlock: firmware did not stop; retaining DMA resources until power off\n")
        self.assertIn("did not stop", why)
        self.assertEqual(self.m.module_result("usb 1-3: new device\n"), (False, "The module did not run at this start (it must be in the start-up image)"))


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
    GOOD = ("menuentry 'Labyrinth Linux, graphics card unlock (one try)' --class gnu-linux --class os --id yggdrasil-gpu-unlock {\n"
            "\tsearch --no-floppy --fs-uuid --set=root 3118e509\n"
            "\tlinux\t/@/boot/vmlinuz-linux-cachyos root=UUID=3118e509 rw quiet turing_unlock.targets=0000:01:00.0\n"
            "\tinitrd\t/@/boot/initramfs-linux-cachyos.img\n}\n")

    def run_it(self, content):
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d)
        path = os.path.join(d, "entry.cfg")
        if content is not None:
            put(path, content)
        p = subprocess.run(["sh", FRAGMENT], capture_output=True, text=True, env=dict(os.environ, YGGDRASIL_GPUUNLOCK_ENTRY=path))
        self.assertEqual(p.returncode, 0, "never fails grub-mkconfig")
        return p.stdout

    def test_nothing_without_an_attempt(self):
        self.assertEqual(self.run_it(None), "")

    def test_the_entry_while_armed(self):
        self.assertEqual(self.run_it(self.GOOD), self.GOOD)

    def test_a_file_of_the_wrong_shape_prints_nothing(self):
        g = self.GOOD
        for bad in [g.replace("--id yggdrasil-gpu-unlock", "--id other"),
                    g.replace("targets=0000:01:00.0", "targets=all"),
                    g.replace("targets=0000:01:00.0", "targets=0000:01:00.0 init=/bin/sh"),
                    g + g,
                    g.replace("\tinitrd", "\tsubmenu 'x' {\n\tinitrd"),
                    g.replace("\tinitrd", "\tlinux\t/other\n\tinitrd"),
                    g[:-2]]:
            self.assertEqual(self.run_it(bad), "", bad)


if __name__ == "__main__":
    unittest.main()
