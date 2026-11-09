"""gpufan without a card: the curve, the speed for a temperature (minimum, hot), settling, the saved setting, and the service's
loop against a fake card (the fans always go back to the card at the end)."""
import contextlib
import importlib.machinery
import importlib.util
import io
import json
import os
import shutil
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gpufan")


def load():
    loader = importlib.machinery.SourceFileLoader("gpufan", HELPER)
    spec = importlib.util.spec_from_loader("gpufan", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


class FakeCard:
    def __init__(self, m, temps, fail_at=None):
        self.m, self.temps, self.fail_at = m, list(temps), fail_at
        self.reads, self.sets, self.defaults = 0, [], []

    def fans(self):
        return 2

    def minmax(self):
        return 24, 100

    def temp(self):
        self.reads += 1
        if self.fail_at is not None and self.reads >= self.fail_at:
            raise self.m.NvmlError("the card went away")
        return self.temps[min(self.reads - 1, len(self.temps) - 1)]

    def set_speed(self, i, pct):
        self.sets.append((i, pct))

    def set_default(self, i):
        self.defaults.append(i)


class Curve(unittest.TestCase):
    def setUp(self):
        self.m = load()

    def test_parse_and_refuse(self):
        self.assertEqual(self.m.parse_curve("40:30, 60:45,85:100"), [[40, 30], [60, 45], [85, 100]])
        for bad in ["40:30", "40-30,60:45", "60:30,40:45", "40:50,60:40", "10:30,60:45", "40:30,60:120", "a:b,c:d",
                    ",".join(f"{20 + 5 * i}:{10 * i}" for i in range(9))]:
            with self.assertRaises(ValueError, msg=bad):
                self.m.parse_curve(bad)

    def test_straight_lines_between_points(self):
        pts = [[40, 30], [60, 50], [80, 100]]
        self.assertEqual(self.m.curve_at(pts, 20), 30, "below the first point")
        self.assertEqual(self.m.curve_at(pts, 50), 40)
        self.assertEqual(self.m.curve_at(pts, 70), 75)
        self.assertEqual(self.m.curve_at(pts, 95), 100, "above the last point")

    def test_never_below_the_cards_minimum_and_full_when_hot(self):
        cfg = {"mode": "curve", "fixed": 60, "curve": [[40, 0], [70, 50], [90, 80]]}
        self.assertEqual(self.m.target(cfg, 30, 24), 24)
        self.assertEqual(self.m.target(cfg, 84, 24), 71)
        self.assertEqual(self.m.target(cfg, 85, 24), 100, "hot: full speed whatever the curve")
        self.assertEqual(self.m.target(dict(cfg, mode="fixed", fixed=10), 50, 24), 24)
        self.assertEqual(self.m.target(dict(cfg, mode="fixed", fixed=10), 90, 24), 100)

    def test_settling_up_at_once_down_when_cooler(self):
        self.assertEqual(self.m.settle((40, 60), 55, 63), 55)
        self.assertEqual(self.m.settle((55, 63), 50, 61), 55, "only 2 C cooler: stays")
        self.assertEqual(self.m.settle((55, 63), 45, 60), 45, "3 C cooler: comes down")
        self.assertEqual(self.m.settle(None, 30, 40), 30)

    def test_the_saved_setting_is_cleaned(self):
        d = tempfile.mkdtemp()
        self.addCleanup(shutil.rmtree, d)
        path = os.path.join(d, "gpu-fan.json")
        self.assertEqual(self.m.load(path)["mode"], "auto", "nothing saved: the card's own control")
        with open(path, "w") as f:
            json.dump({"mode": "turbo", "fixed": 300, "curve": [[60, 30], [40, 50]]}, f)
        c = self.m.load(path)
        self.assertEqual((c["mode"], c["fixed"], c["curve"]), ("auto", 60, self.m.DEFAULT_CURVE))
        self.m.save({"mode": "curve", "fixed": 70, "curve": [[45, 35], [80, 90]]}, path)
        self.assertEqual(self.m.load(path), {"mode": "curve", "fixed": 70, "curve": [[45, 35], [80, 90]]})

    def test_set_needs_root_and_changes_nothing_without_it(self):
        self.m.is_root = lambda: False
        self.m.CONF = os.path.join(tempfile.mkdtemp(), "gpu-fan.json")
        self.addCleanup(shutil.rmtree, os.path.dirname(self.m.CONF))
        with contextlib.redirect_stderr(io.StringIO()):
            self.assertEqual(self.m.cmd_set("fixed=50"), 1)
        self.assertFalse(os.path.exists(self.m.CONF))


class Service(unittest.TestCase):
    def setUp(self):
        self.m = load()
        self.m.REAPPLY_S = 10 ** 9

    def drive(self, card, cfg, steps):
        n = [0]
        given = [cfg]

        def read_cfg():
            c, given[0] = given[0], None
            return c

        def stop():
            n[0] += 1
            return n[0] > steps

        with contextlib.redirect_stderr(io.StringIO()):
            return self.m.run(card, read_cfg, lambda s: None, stop, clock=lambda: 0.0)

    def test_follows_the_curve_settles_and_gives_back(self):
        card = FakeCard(self.m, [40, 60, 75, 73, 71, 70])
        cfg = {"mode": "curve", "fixed": 60, "curve": [[40, 30], [60, 45], [75, 75], [85, 100]]}
        self.assertEqual(self.drive(card, cfg, 6), 0)
        speeds = [p for i, p in card.sets if i == 0]
        self.assertEqual(speeds, [30, 45, 75, 67], "up at once; at 73 C it stays at 75 %; at 71 C (3 cooler) it comes down to 67 %, and at 70 C it stays there")
        self.assertEqual(sorted(card.defaults), [0, 1], "both fans back to the card at the end")

    def test_a_card_that_cannot_be_read_goes_back_to_its_own_control(self):
        card = FakeCard(self.m, [50, 50, 50], fail_at=3)
        self.assertEqual(self.drive(card, {"mode": "fixed", "fixed": 70, "curve": self.m.DEFAULT_CURVE}, 10), 1)
        self.assertEqual(sorted(card.defaults), [0, 1])

    def test_auto_ends_the_loop(self):
        card = FakeCard(self.m, [50])
        self.assertEqual(self.drive(card, {"mode": "auto", "fixed": 60, "curve": self.m.DEFAULT_CURVE}, 5), 0)
        self.assertEqual(card.sets, [])
        self.assertEqual(sorted(card.defaults), [0, 1])

    def test_hot_overrides_a_low_fixed_speed(self):
        card = FakeCard(self.m, [70, 86])
        self.drive(card, {"mode": "fixed", "fixed": 30, "curve": self.m.DEFAULT_CURVE}, 2)
        self.assertEqual([p for i, p in card.sets if i == 1], [30, 100])


if __name__ == "__main__":
    unittest.main()
