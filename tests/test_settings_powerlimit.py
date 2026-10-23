"""Tests for Settings' power-limit helper. Run: python tests/test_settings_powerlimit.py"""
import json
import os
import sys
import tempfile
import unittest

import importlib.machinery
import importlib.util

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def load_module(name, path):
    loader = importlib.machinery.SourceFileLoader(name, path)
    spec = importlib.util.spec_from_loader(name, loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


pl = load_module("powerlimit", os.path.join(ROOT, "packages", "settings", "helpers", "powerlimit"))


class PowerLimitTests(unittest.TestCase):
    def test_cpu_ceiling_never_goes_below_what_the_processor_can_run_at(self):
        self.assertEqual(pl.cpu_target_khz(100, 800000, 5000000), 5000000)
        self.assertEqual(pl.cpu_target_khz(50, 800000, 5000000), 2500000)
        self.assertEqual(pl.cpu_target_khz(5, 800000, 5000000), 800000)

    def test_cpu_limits_are_written_to_every_policy_and_restored(self):
        with tempfile.TemporaryDirectory() as d:
            for i in range(3):
                p = os.path.join(d, "policy%d" % i)
                os.makedirs(p)
                for f, v in (("cpuinfo_min_freq", 800000), ("cpuinfo_max_freq", 4000000), ("scaling_max_freq", 4000000)):
                    with open(os.path.join(p, f), "w") as fh:
                        fh.write(str(v))
            self.assertEqual(pl.apply_cpu(75, d), 3)
            for i in range(3):
                self.assertEqual(open(os.path.join(d, "policy%d" % i, "scaling_max_freq")).read(), "3000000")
            pl.apply_cpu(None, d)
            self.assertEqual(open(os.path.join(d, "policy0", "scaling_max_freq")).read(), "4000000")

    def test_state_is_validated(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "s.json")
            with open(p, "w") as f:
                json.dump({"balanced": {"cpu": 80, "gpu": 200, "x": 1}, "turbo": {"cpu": 100}, "performance": {"cpu": 500, "gpu": "a"}, "power-saver": {"cpu": 40}}, f)
            self.assertEqual(pl.load_state(p), {"balanced": {"cpu": 80, "gpu": 200}, "power-saver": {"cpu": 40}})
            pl.save_state({"balanced": {"cpu": 60}}, p)
            self.assertEqual(pl.load_state(p), {"balanced": {"cpu": 60}})
            self.assertEqual(os.stat(p).st_mode & 0o777, 0o644)
            self.assertEqual(pl.load_state(os.path.join(d, "none.json")), {})


if __name__ == "__main__":
    unittest.main()
