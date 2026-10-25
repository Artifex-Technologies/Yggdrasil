"""Tests for the account helper's access commands, against a made-up /etc and stand-in system tools. python3 test_account.py"""
import json
import os
import subprocess
import sys
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "account")


class Access(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        w = lambda name, text: open(os.path.join(self.d, name), "w").write(text)
        w("passwd", "root:x:0:0::/root:/bin/bash\nada:x:1000:1000:Ada:/home/ada:/bin/bash\nbob:x:1001:1001:Bob:/home/bob:/bin/bash\n"
                    "kept:x:1002:1002:Kept:/home/kept:/bin/bash\n")
        w("group", "wheel:x:998:ada\nlibvirt:x:990:bob\nuucp:x:987:\n")
        w("protected", "kept\n")
        os.mkdir(os.path.join(self.d, "sudoers.d"))
        # stand-ins for the tools the helper calls: they only write down what they were asked
        bin_ = os.path.join(self.d, "bin")
        os.mkdir(bin_)
        for tool in ("gpasswd", "usermod", "chpasswd", "loginctl", "visudo"):
            path = os.path.join(bin_, tool)
            open(path, "w").write('#!/bin/sh\necho "%s $*" >> "%s/calls"\n' % (tool, self.d))
            os.chmod(path, 0o755)
        self.env = dict(os.environ, YGGDRASIL_ACCOUNT_TEST="1", YGGDRASIL_PASSWD=os.path.join(self.d, "passwd"), YGGDRASIL_GROUP=os.path.join(self.d, "group"),
                        YGGDRASIL_PROTECTED_ACCOUNTS=os.path.join(self.d, "protected"), YGGDRASIL_SUDOERS_D=os.path.join(self.d, "sudoers.d"),
                        PKEXEC_UID="1000", PATH=bin_ + ":" + os.environ["PATH"])

    def run_helper(self, *args, stdin=None):
        return subprocess.run([sys.executable, HELPER] + list(args), env=self.env, input=stdin, capture_output=True, text=True)

    def calls(self):
        p = os.path.join(self.d, "calls")
        return open(p).read() if os.path.exists(p) else ""

    def listed(self):
        return {a["login"]: a for a in json.loads(self.run_helper("list").stdout)}

    def test_the_list_tells_access(self):
        a = self.listed()
        self.assertTrue(a["ada"]["admin"] and not a["bob"]["admin"])
        self.assertEqual(a["bob"]["groups"], ["libvirt"])
        self.assertTrue(a["kept"]["protected"])
        self.assertEqual(json.loads(self.run_helper("groups").stdout), {"libvirt": "Run virtual machines", "uucp": "Use serial ports (Arduino, 3D printers, modems)"})

    def test_sudo_without_password_is_a_file_of_ours_and_goes_with_the_admin_rights(self):
        self.assertEqual(self.run_helper("sudo", "ada", "nopassword").returncode, 0)
        rule = open(os.path.join(self.d, "sudoers.d", "60-yggdrasil-nopasswd-ada")).read()
        self.assertIn("ada ALL=(ALL:ALL) NOPASSWD: ALL", rule)
        self.assertTrue(self.listed()["ada"]["sudo_nopassword"])
        self.assertNotEqual(self.run_helper("sudo", "bob", "nopassword").returncode, 0, "not an administrator")
        self.assertEqual(self.run_helper("sudo", "ada", "password").returncode, 0)
        self.assertFalse(self.listed()["ada"]["sudo_nopassword"])

    def test_accounts_kept_by_the_system_are_never_changed(self):
        for args in (("sudo", "kept", "nopassword"), ("group", "kept", "uucp", "on"), ("login", "kept", "off"), ("name", "kept", "X")):
            self.assertNotEqual(self.run_helper(*args).returncode, 0, args)
        self.assertEqual(self.calls(), "")

    def test_only_listed_groups_can_be_switched(self):
        self.assertEqual(self.run_helper("group", "bob", "uucp", "on").returncode, 0)
        self.assertIn("gpasswd -a bob uucp", self.calls())
        self.assertNotEqual(self.run_helper("group", "bob", "wheel", "on").returncode, 0)
        self.assertNotEqual(self.run_helper("group", "bob", "docker", "on").returncode, 0, "not on this system")

    def test_signing_in_cannot_be_taken_from_yourself_or_the_last_administrator(self):
        self.assertNotEqual(self.run_helper("login", "ada", "off").returncode, 0)
        self.assertEqual(self.run_helper("login", "bob", "off").returncode, 0)
        self.assertIn("usermod -s /usr/bin/nologin -L bob", self.calls())

    def test_a_password_for_someone_else_never_on_the_command_line(self):
        self.assertEqual(self.run_helper("password", "bob", stdin="a-long-password\n").returncode, 0)
        self.assertNotIn("a-long-password", self.calls())
        self.assertNotEqual(self.run_helper("password", "ada", stdin="a-long-password\n").returncode, 0, "your own goes through passwd")
        self.assertNotEqual(self.run_helper("password", "bob", stdin="short\n").returncode, 0)


if __name__ == "__main__":
    unittest.main()
