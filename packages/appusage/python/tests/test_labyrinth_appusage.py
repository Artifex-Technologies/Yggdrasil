"""Tests for the Python labyrinth-appusage (kept for a rollback; the Rust one carries the same tests in src/tests.rs).
From Labyrinth OS's tests/test_labyrinth_appusage.py, loading the script from this folder. Run: python3 test_labyrinth_appusage.py"""
import importlib.machinery
import importlib.util
import json
import os
import tempfile
import unittest

SCRIPT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "labyrinth-appusage")


def load_module(name, path):
    loader = importlib.machinery.SourceFileLoader(name, path)
    spec = importlib.util.spec_from_loader(name, loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


au = load_module("labyrinth_appusage", SCRIPT)
IDS = {"firefox", "spotify-launcher", "com.microsoft.VSCode", "org.gimp.GIMP", "foo"}


class UsageTests(unittest.TestCase):
    def test_units_map_to_desktop_ids(self):
        m = au.match_unit
        self.assertEqual(m("lapp_spotify_launcher_123.scope", IDS), "spotify-launcher")
        self.assertEqual(m("app-firefox-4567.scope", IDS), "firefox")
        self.assertEqual(m("app-com.microsoft.VSCode-1915.scope", IDS), "com.microsoft.VSCode")
        self.assertEqual(m("app-spotify\\x2dlauncher-9.scope", IDS), "spotify-launcher")
        self.assertEqual(m("app-flatpak-org.gimp.GIMP-3.scope", IDS), "org.gimp.GIMP")
        self.assertEqual(m("app-foo@abc123.scope", IDS), "foo")
        self.assertIsNone(m("session-3.scope", IDS))
        self.assertIsNone(m("app-unknown-1.scope", IDS))

    def test_seconds_add_up_once_per_app(self):
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "u", "app-usage.json")
            apps = au.sample(30, ids=IDS, units=["app-firefox-1.scope", "app-firefox-2.scope", "lapp_foo_9.scope"], path=p)
            self.assertEqual(apps, {"firefox", "foo"})
            au.sample(30, ids=IDS, units=["app-firefox-1.scope"], path=p)
            with open(p) as f:
                u = json.load(f)
            self.assertEqual(u["firefox"]["seconds"], 60)
            self.assertEqual(u["foo"]["seconds"], 30)
            # launch counts written by labyrinth-launch are kept
            u["firefox"]["launches"] = 5
            with open(p, "w") as f:
                json.dump(u, f)
            au.sample(30, ids=IDS, units=["app-firefox-1.scope"], path=p)
            with open(p) as f:
                self.assertEqual(json.load(f)["firefox"], {"seconds": 90, "launches": 5})


if __name__ == "__main__":
    unittest.main()
