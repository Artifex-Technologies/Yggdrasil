"""tune without touching the hardware: multipliers in and out of the MSR's bytes, what is refused, the crash guard across
starts, trying and keeping, and the graphics driver's errors. MSRs, NVML and the files are stand-ins."""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "tune")
STOCK = [50, 50, 48, 48, 47, 47, 47, 47]


def load():
    loader = importlib.machinery.SourceFileLoader("tune", HELPER)
    spec = importlib.util.spec_from_loader("tune", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


class Base(unittest.TestCase):
    def setUp(self):
        self.enterContext(contextlib.redirect_stderr(io.StringIO()))
        self.d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, self.d)
        m = self.m = load()
        m.CONF = os.path.join(self.d, "etc", "tuning.json")
        m.STATE = os.path.join(self.d, "state")
        m.RUN = os.path.join(self.d, "run", "tune.json")
        m.is_root = lambda: True
        m.log = lambda e: None
        self.msr = {m.MSR_TURBO_RATIO_LIMIT: m.value_of(STOCK)}
        m.rdmsr = lambda reg, cpu=0: self.msr[reg]
        m.wrmsr = lambda reg, v: self.msr.__setitem__(reg, v)
        m.platform = lambda: (36, True)
        self.gpu = []
        m.apply_gpu = lambda v: self.gpu.append(dict(v))
        m.publish = lambda *a, **k: None
        self.boot = "boot-1"
        m.boot_id = lambda: self.boot
        m.write_json(os.path.join(m.STATE, "stock.json"), {"ratios": STOCK, "pl1": 95, "pl2": 125})

    def ratios(self):
        return self.m.ratios_of(self.msr[self.m.MSR_TURBO_RATIO_LIMIT])


class Multipliers(Base):
    def test_bytes_round_trip(self):
        self.assertEqual(self.m.ratios_of(self.m.value_of(STOCK)), STOCK)
        self.assertEqual(self.m.value_of([50, 0, 0, 0, 0, 0, 0, 0]), 50)

    def test_refused(self):
        for bad in [[50] * 7, [54] * 8, [7] * 8, [48, 50, 48, 48, 47, 47, 47, 47], ["50"] * 8, [True] * 8]:
            with self.assertRaises(ValueError, msg=bad):
                self.m.check_ratios(bad, STOCK)
        self.assertEqual(self.m.check_ratios([53] * 8, STOCK), [53] * 8, "up to 3 above the processor's own top")

    def test_try_applies_now_and_keep_makes_it_start_with_the_computer(self):
        self.assertEqual(self.m.cmd_try("cpu", ["ratios=51,51,50,50,49,49,49,49"]), 0)
        self.assertEqual(self.ratios(), [51, 51, 50, 50, 49, 49, 49, 49])
        self.assertTrue(os.path.exists(os.path.join(self.m.STATE, "tuned")), "the guard is set before anything is applied")
        self.assertIsNone(self.m.conf()["cpu"], "not kept yet")
        self.assertEqual(self.m.cmd_keep("cpu"), 0)
        self.assertEqual(self.m.conf()["cpu"], {"ratios": [51, 51, 50, 50, 49, 49, 49, 49]})
        self.assertIsNone(self.m.status().get("trial"))

    def test_undo_goes_back_to_the_kept_values(self):
        self.m.cmd_try("cpu", ["ratios=52,52,52,52,52,52,52,52"])
        self.m.cmd_undo("cpu")
        self.assertEqual(self.ratios(), STOCK, "nothing kept: the processor's own")

    def test_a_locked_processor_is_refused(self):
        self.m.platform = lambda: (36, False)
        self.assertEqual(self.m.cmd_try("cpu", ["ratios=50,50,50,50,50,50,50,50"]), 2)
        self.assertEqual(self.ratios(), STOCK)

    def test_not_root_changes_nothing(self):
        self.m.is_root = lambda: False
        self.assertEqual(self.m.cmd_try("cpu", ["ratios=52,52,52,52,52,52,52,52"]), 1)
        self.assertEqual(self.ratios(), STOCK)


class Guard(Base):
    def keep(self, ratios, gpu=None):
        self.m.write_json(self.m.CONF, {"cpu": {"ratios": ratios}, "gpu": gpu})

    def start(self):
        """What the service does at a start, without its endless loop."""
        m = self.m
        guarded = m.guard_check(self.boot)
        c, st = m.conf(), m.status()
        if not guarded:
            m.refresh_mark()
            for what in ("cpu", "gpu"):
                if not st.get(f"{what}_off"):
                    m.apply_kept(what, c)
        return guarded

    def test_a_clean_shutdown_keeps_the_values_and_a_freeze_does_not(self):
        self.keep([52] * 8)
        self.start()
        self.assertEqual(self.ratios(), [52] * 8)
        # clean shutdown: the service removes the marker
        os.remove(os.path.join(self.m.STATE, "tuned"))
        self.msr[self.m.MSR_TURBO_RATIO_LIMIT] = self.m.value_of(STOCK)   # a restart puts the processor's own back
        self.boot = "boot-2"
        self.assertFalse(self.start())
        self.assertEqual(self.ratios(), [52] * 8)
        # this time it freezes: the marker stays
        self.msr[self.m.MSR_TURBO_RATIO_LIMIT] = self.m.value_of(STOCK)
        self.boot = "boot-3"
        self.assertTrue(self.start())
        self.assertEqual(self.ratios(), STOCK, "after a freeze the defaults stay")
        self.assertIn("did not shut down cleanly", self.m.status()["cpu_off"])
        self.assertEqual(self.m.conf()["cpu"], {"ratios": [52] * 8}, "the kept values are not lost")
        self.boot = "boot-4"
        self.assertFalse(self.start(), "the marker was cleared")
        self.assertEqual(self.ratios(), STOCK, "still off until switched on again")
        self.assertEqual(self.m.cmd_reenable(), 0)
        self.assertEqual(self.ratios(), [52] * 8)

    def test_a_freeze_during_a_try_never_brings_the_tried_values_back(self):
        self.m.cmd_try("cpu", ["ratios=53,53,53,53,53,53,53,53"])
        self.msr[self.m.MSR_TURBO_RATIO_LIMIT] = self.m.value_of(STOCK)
        self.boot = "boot-2"
        self.assertTrue(self.start())
        self.assertEqual(self.ratios(), STOCK)

    def test_nothing_tuned_needs_no_guard(self):
        self.start()
        self.assertFalse(os.path.exists(os.path.join(self.m.STATE, "tuned")))

    def test_back_at_the_defaults_the_guard_lets_go(self):
        self.m.cmd_try("cpu", ["ratios=51,51,51,51,51,51,51,51"])
        self.assertTrue(os.path.exists(os.path.join(self.m.STATE, "tuned")))
        self.m.cmd_undo("cpu")
        self.assertFalse(os.path.exists(os.path.join(self.m.STATE, "tuned")), "an unrelated crash later must not switch tuning off")
        self.boot = "boot-2"
        self.assertFalse(self.start())


class Stock(Base):
    def test_the_processors_own_values_are_read_once_per_start(self):
        self.m.rapl_limits = lambda: (95, 125)
        os.remove(os.path.join(self.m.STATE, "stock.json"))
        self.assertEqual(self.m.watch_stock()["ratios"], STOCK)
        self.msr[self.m.MSR_TURBO_RATIO_LIMIT] = self.m.value_of([49] * 8)   # tuned, then the service restarts
        self.assertEqual(self.m.watch_stock()["ratios"], STOCK, "a restart of the service alone keeps what it read")
        self.boot = "boot-2"
        self.msr[self.m.MSR_TURBO_RATIO_LIMIT] = self.m.value_of(STOCK)      # the computer restarted: the processor's own
        self.assertEqual(self.m.watch_stock()["ratios"], STOCK)


class GraphicsCard(Base):
    def test_offsets_checked_and_tried(self):
        for bad in [{"core": 400, "mem": 0}, {"core": 0, "mem": 3000}, {"core": 0, "mem": 0, "cap": 100}, {"core": "1"}]:
            with self.assertRaises(ValueError, msg=bad):
                self.m.check_gpu(bad)
        self.assertEqual(self.m.cmd_try("gpu", ["core=120", "mem=600", "cap=1900"]), 0)
        self.assertEqual(self.gpu[-1], {"core": 120, "mem": 600, "cap": 1900})
        self.assertEqual(self.m.cmd_undo("gpu"), 0)
        self.assertEqual(self.gpu[-1], {"core": 0, "mem": 0, "cap": None})

    def test_a_driver_error_puts_the_clocks_back(self):
        self.m.cmd_try("gpu", ["core=150", "mem=900", "cap=none"])
        self.assertFalse(self.m.on_kernel_line("usb 1-3: new device"))
        self.assertTrue(self.m.on_kernel_line("NVRM: Xid (PCI:0000:01:00): 13, pid=4242, Graphics Exception"))
        self.assertEqual(self.gpu[-1], {"core": 0, "mem": 0, "cap": None})
        st = self.m.status()
        self.assertIn("graphics driver reported an error", st["gpu_off"])
        self.assertNotIn("gpu", st.get("trial") or {})

    def test_a_driver_error_with_nothing_tuned_is_left_alone(self):
        self.assertFalse(self.m.on_kernel_line("NVRM: Xid (PCI:0000:01:00): 13, pid=4242"))
        self.assertEqual(self.gpu, [])


class Power(Base):
    def test_limits_checked_and_saved(self):
        written = {}
        self.m.write_text = lambda path, text: written.__setitem__(os.path.basename(path), text)
        self.assertEqual(self.m.cmd_set("power", ["pl1=120", "pl2=160"]), 0)
        self.assertEqual(written, {"constraint_0_power_limit_uw": "120000000", "constraint_1_power_limit_uw": "160000000"})
        self.assertEqual(self.m.conf()["power"], {"pl1": 120, "pl2": 160})
        self.assertEqual(self.m.cmd_set("power", ["pl2=100"]), 2, "short-term below long-term")
        self.assertEqual(self.m.cmd_set("power", ["pl1=500"]), 2)
        self.assertEqual(self.m.cmd_set("power", ["pl1=default", "pl2=default"]), 0)
        self.assertEqual(written["constraint_0_power_limit_uw"], "95000000", "default: the BIOS's own, read at start")

    def test_turbo(self):
        written = {}
        self.m.write_text = lambda path, text: written.__setitem__(os.path.basename(path), text)
        self.assertEqual(self.m.cmd_set("turbo", ["off"]), 0)
        self.assertEqual(written["no_turbo"], "1")
        self.assertIs(self.m.conf()["turbo"], False)


if __name__ == "__main__":
    unittest.main()
