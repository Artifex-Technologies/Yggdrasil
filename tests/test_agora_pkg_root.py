"""Tests for the refusals of Agora's pkg-root (it must only ever run narrow pacman install/remove commands). Run: python tests/test_agora_pkg_root.py"""
import os
import subprocess
import sys
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
HELPER = os.path.join(ROOT, "packages", "market", "helpers", "pkg-root")


def run(args, uid="1000"):
    env = dict(os.environ, PKEXEC_UID=uid)
    return subprocess.run([sys.executable, HELPER] + args, capture_output=True, text=True, env=env)


GOOD = "/run/user/1000/agora/x.cancel"


class Refusals(unittest.TestCase):
    def test_needs_pacman_and_arguments(self):
        self.assertEqual(run([GOOD, "ls", "-S", "krita"]).returncode, 2)
        self.assertEqual(run([GOOD]).returncode, 2)

    def test_cancel_file_must_be_the_invoking_users_runtime_folder(self):
        self.assertEqual(run(["/etc/passwd", "pacman", "-S", "krita"]).returncode, 2)
        self.assertEqual(run(["/run/user/1000/agora/../../0/x", "pacman", "-S", "krita"]).returncode, 2)
        self.assertEqual(run(["/run/user/1001/agora/x", "pacman", "-S", "krita"], uid="1000").returncode, 2)
        self.assertEqual(run([GOOD, "pacman", "-S", "krita"], uid="").returncode, 2)

    def test_options_outside_the_short_list_are_refused(self):
        for bad in ("--root", "--config", "--hookdir", "--sysroot", "-Sy", "--overwrite", "--dbpath"):
            r = run([GOOD, "pacman", "-S", bad, "krita"])
            self.assertEqual(r.returncode, 2, bad)
            self.assertIn("not allowed", r.stderr)

    def test_odd_package_names_are_refused(self):
        for bad in ("krita;ls", "$(id)", "a b", "`x`"):
            self.assertEqual(run([GOOD, "pacman", "-S", bad]).returncode, 2, bad)

    def test_ignore_takes_a_plain_list_and_syu_is_allowed(self):
        self.assertEqual(run([GOOD, "pacman", "-Syu", "--ignore", "a b"]).returncode, 2)
        self.assertEqual(run([GOOD, "pacman", "-Syu", "--ignore", "x;y"]).returncode, 2)
        self.assertEqual(run([GOOD, "pacman", "-Syu", "--ignore"]).returncode, 2)

    def test_only_install_and_remove(self):
        self.assertEqual(run([GOOD, "pacman", "--needed", "krita"]).returncode, 2)


if __name__ == "__main__":
    unittest.main()
