"""casefan without the hardware: the speed for each port (pump floor, curves from both parts, hot), settling, the saved setting,
the stop speeds, the Commander Pro found in a fake hwmon tree, and the service loop (a sensor failure puts every port at 100 %)."""
import importlib.machinery
import importlib.util
import json
import os
import shutil
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "casefan")


def load():
    loader = importlib.machinery.SourceFileLoader("casefan", HELPER)
    spec = importlib.util.spec_from_loader("casefan", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


m = load()
PUMP = {"name": "pump", "role": "pump", "fixed": 100}
FAN = m.clean({"ports": {"6": {"role": "fan"}}})["ports"]["6"]


class Speeds(unittest.TestCase):
    def test_pump_never_below_its_floor(self):
        self.assertEqual(m.clean({"ports": {"1": {"role": "pump", "fixed": 10}}})["ports"]["1"]["fixed"], m.PUMP_MIN_PCT)
        self.assertEqual(m.target(PUMP, 40, 30), 100)

    def test_fan_follows_the_hotter_part(self):
        idle = m.target(FAN, 45, 35)
        self.assertEqual(idle, 25)
        self.assertGreater(m.target(FAN, 45, 70), 60)   # the card working, the processor idle
        self.assertGreater(m.target(FAN, 84, 35), 70)   # the processor working, the card idle

    def test_hot_is_full_speed(self):
        self.assertEqual(m.target(FAN, m.CPU_HOT_C, 30), 100)
        self.assertEqual(m.target(FAN, 40, m.GPU_HOT_C), 100)

    def test_never_below_the_fan_floor(self):
        low = m.clean({"ports": {"2": {"role": "fan", "cpu": [[30, 0], [90, 100]], "gpu": [[30, 0], [90, 100]]}}})["ports"]["2"]
        self.assertEqual(m.target(low, 30, 30), m.FAN_MIN_PCT)

    def test_settle_waits_before_slowing(self):
        self.assertEqual(m.settle((60, -10), 40, -11), 60)
        self.assertEqual(m.settle((60, -10), 40, -13), 40)
        self.assertEqual(m.settle((60, -10), 80, -11), 80)

    def test_stop_speeds(self):
        self.assertEqual(m.stop_speed(FAN, False), m.STOP_FAN_PCT)
        self.assertEqual(m.stop_speed(FAN, True), 100)
        self.assertEqual(m.stop_speed(PUMP, False), 100)


class Setting(unittest.TestCase):
    def test_bad_entries_are_dropped_or_defaulted(self):
        c = m.clean({"ports": {"9": {"role": "fan"}, "x": {}, "3": {"role": "fan", "cpu": [[60, 50], [50, 40]]}}})
        self.assertEqual(list(c["ports"]), ["3"])
        self.assertEqual(c["ports"]["3"]["cpu"], m.DEFAULT_CPU)

    def test_round_trip(self):
        d = tempfile.mkdtemp()
        try:
            p = os.path.join(d, "case-fans.json")
            m.save({"ports": {"1": PUMP, "6": FAN}}, p)
            self.assertEqual(m.load(p)["ports"]["1"]["role"], "pump")
        finally:
            shutil.rmtree(d)


class Hardware(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        cp = os.path.join(self.root, "hwmon5")
        os.makedirs(cp)
        for name, text in {"name": "corsaircpro", "fan1_label": "fan1 4pin", "fan1_input": "3846", "fan6_label": "fan6 4pin",
                           "fan6_input": "1399", "pwm1": "", "pwm6": ""}.items():
            with open(os.path.join(cp, name), "w") as f:
                f.write(text)
        ct = os.path.join(self.root, "hwmon3")
        os.makedirs(ct)
        for name, text in {"name": "coretemp", "temp1_label": "Package id 0", "temp1_input": "53000"}.items():
            with open(os.path.join(ct, name), "w") as f:
                f.write(text)

    def tearDown(self):
        shutil.rmtree(self.root)

    def test_finds_ports_and_temperature(self):
        cp = m.find_cpro(self.root)
        self.assertTrue(cp.endswith("hwmon5"))
        self.assertEqual(m.ports(cp), {"1": {"label": "fan1 4pin", "rpm": 3846}, "6": {"label": "fan6 4pin", "rpm": 1399}})
        self.assertEqual(m.cpu_temp(self.root), 53)
        m.set_pct(cp, "6", 50)
        with open(os.path.join(cp, "pwm6")) as f:
            self.assertEqual(f.read(), "128")


class Loop(unittest.TestCase):
    def test_runs_then_full_speed_on_a_sensor_failure(self):
        writes, temps = [], [(45, 35), (45, 35), (84, 35)]
        cfg = m.clean({"ports": {"1": PUMP, "6": {"role": "fan"}}})

        def read_cpu():
            if not temps:
                raise m.SensorError("gone")
            return temps[0][0]

        def read_gpu():
            return temps.pop(0)[1]

        rc = m.run("x", lambda: None if writes else cfg, read_cpu, read_gpu, lambda n, p: writes.append((n, p)),
                   lambda s: None, lambda: False, clock=lambda: 0.0)
        self.assertEqual(rc, 1)
        self.assertIn(("1", 100), writes)
        self.assertIn(("6", 25), writes)
        self.assertTrue(any(n == "6" and p > 70 for n, p in writes))   # the processor got hot
        self.assertEqual(writes[-2:], [("1", 100), ("6", 100)])        # then the sensor failed: everything at full speed


if __name__ == "__main__":
    unittest.main()


class Calm(unittest.TestCase):
    """The night-3 pattern: each eval case swings the processor 42-64 C and the card 54-60 C within seconds."""

    def test_average_and_ramp_keep_the_fan_steady(self):
        writes = []
        swing = [(58, 60), (58, 60), (43, 54), (42, 54), (58, 59), (64, 60), (58, 60), (43, 55), (42, 54)] * 8
        temps = list(swing)

        def read_cpu():
            if not temps:
                raise m.SensorError("done")
            return temps[0][0]

        def read_gpu():
            return temps.pop(0)[1]

        cfg = m.clean({"ports": {"6": {"role": "fan"}}})
        m.run("x", lambda: None if writes else cfg, read_cpu, read_gpu, lambda n, p: writes.append(p), lambda s: None, lambda: False,
              clock=lambda: 0.0)
        speeds = writes[:-1]   # everything before the failure's own 100 % (a speed is written only when it changes)
        self.assertLessEqual(max(speeds) - min(speeds), 6, speeds)
        self.assertLess(len(speeds), len(swing) // 4, speeds)   # 72 readings, only a handful of changes
        steps = [abs(b - a) for a, b in zip(speeds, speeds[1:])]
        self.assertLessEqual(max(steps, default=0), m.RAMP_UP_PCT)
        self.assertTrue(30 <= speeds[-1] <= 50, speeds)   # where the averaged temperatures put it on the curve

    def test_hot_is_still_immediate(self):
        writes, temps = [], [(50, 40)] * 5 + [(m.CPU_HOT_C, 40)]

        def read_cpu():
            if not temps:
                raise m.SensorError("done")
            return temps[0][0]

        cfg = m.clean({"ports": {"6": {"role": "fan"}}})
        m.run("x", lambda: None if writes else cfg, read_cpu, lambda: temps.pop(0)[1], lambda n, p: writes.append(p), lambda s: None,
              lambda: False, clock=lambda: 0.0)
        self.assertEqual(writes[-2], 100)   # the hot reading, before the failure's own 100 %
        self.assertLess(writes[-3], 50)

    def test_ramp(self):
        self.assertEqual(m.ramp(None, 60), 60)
        self.assertEqual(m.ramp(40, 60), 44)
        self.assertEqual(m.ramp(60, 40), 59)
