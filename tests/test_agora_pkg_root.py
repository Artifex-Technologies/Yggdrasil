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
            self.assertTrue("not allowed" in r.stderr or "may only name" in r.stderr, r.stderr)

    def test_odd_package_names_are_refused(self):
        for bad in ("krita;ls", "$(id)", "a b", "`x`"):
            self.assertEqual(run([GOOD, "pacman", "-S", bad]).returncode, 2, bad)

    def test_ignore_takes_a_plain_list_and_syu_is_allowed(self):
        self.assertEqual(run([GOOD, "pacman", "-Syu", "--ignore", "a b"]).returncode, 2)
        self.assertEqual(run([GOOD, "pacman", "-Syu", "--ignore", "x;y"]).returncode, 2)
        self.assertEqual(run([GOOD, "pacman", "-Syu", "--ignore"]).returncode, 2)

    def test_only_install_and_remove(self):
        self.assertEqual(run([GOOD, "pacman", "--needed", "krita"]).returncode, 2)


class WhatYaySends(unittest.TestCase):
    """yay installs what it built through the sudo stand-in: these exact shapes must pass, and nothing wider."""

    def setUp(self):
        import tempfile
        self.d = tempfile.mkdtemp()
        self.log = os.path.join(self.d, "pacman.log")
        # a stand-in pacman: records its arguments, lists nothing as installed
        with open(os.path.join(self.d, "pacman"), "w") as f:
            f.write('#!/bin/sh\n[ "$1" = -Qq ] && exit 0\necho "$*" >> %s\n' % self.log)
        os.chmod(os.path.join(self.d, "pacman"), 0o755)
        self.pkg = os.path.join(self.d, "cadcraft-bin-0.12.0-1-x86_64.pkg.tar.zst")
        open(self.pkg, "w").close()

    def go(self, args):
        env = dict(os.environ, PKEXEC_UID="1000", PATH=self.d + ":" + os.environ.get("PATH", ""))
        return subprocess.run([sys.executable, HELPER, GOOD, "pacman"] + args, capture_output=True, text=True, env=env)

    def test_installing_built_packages_passes(self):
        r = self.go(["-U", "--ignore", "klassy", "--ignore", "sarina-git", "--noconfirm", "--config", "/etc/pacman.conf", "--", self.pkg])
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("-U --ignore klassy", open(self.log).read())

    def test_marking_install_reasons_passes(self):
        self.assertEqual(self.go(["-D", "--asdeps", "--config", "/etc/pacman.conf", "--", "somelib"]).returncode, 0)
        self.assertEqual(self.go(["-D", "--", "somelib"]).returncode, 2, "-D without --asdeps/--asexplicit")

    def test_another_config_file_is_refused(self):
        r = self.go(["-U", "--noconfirm", "--config", "/tmp/evil.conf", "--", self.pkg])
        self.assertEqual(r.returncode, 2)
        self.assertIn("--config", r.stderr)

    def test_install_from_file_needs_a_real_package_file(self):
        for bad in ("https://example.com/x.pkg.tar.zst", "/etc/passwd", os.path.join(self.d, "missing-1-x86_64.pkg.tar.zst")):
            self.assertEqual(self.go(["-U", "--noconfirm", "--", bad]).returncode, 2, bad)

    def test_a_refresh_is_always_a_full_upgrade(self):
        r = self.go(["-S", "-y", "--ignore", "klassy", "--noconfirm", "--config", "/etc/pacman.conf", "--"])
        self.assertEqual(r.returncode, 0, r.stderr)
        ran = open(self.log).read().split()
        self.assertIn("-Syu", ran)
        self.assertNotIn("-y", ran)
        self.assertEqual(self.go(["-S", "-y", "--noconfirm", "--", "krita"]).returncode, 2, "refresh, then install one package: a partial upgrade")

    def test_options_after_the_separator_are_not_options(self):
        r = self.go(["-U", "--noconfirm", "--", self.pkg, "-S"])
        self.assertEqual(r.returncode, 2, "a second operation hidden after --")
        self.assertEqual(self.go(["--noconfirm", "--", "-S", "krita"]).returncode, 2, "no operation before --")


if __name__ == "__main__":
    unittest.main()
