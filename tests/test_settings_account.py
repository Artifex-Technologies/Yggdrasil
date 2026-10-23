"""Settings' account helper with stand-ins for useradd, chpasswd, userdel, gpasswd and loginctl. Run: python tests/test_settings_account.py"""
import json
import os
import stat
import subprocess
import tempfile
import unittest

BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "packages", "settings", "helpers", "account")


class Account(unittest.TestCase):
    def setUp(self):
        self.d = tempfile.mkdtemp()
        self.passwd = os.path.join(self.d, "passwd")
        self.group = os.path.join(self.d, "group")
        self.calls = os.path.join(self.d, "calls.log")
        open(self.passwd, "w").write("root:x:0:0::/root:/bin/bash\nzlothra:x:1000:1000:Zlothra,,,:/home/zlothra:/bin/bash\nclaude-agent:x:1001:1001::/home/claude-agent:/bin/bash\n"
                                     "sam:x:1002:1002:Sam Doe:/home/sam:/bin/bash\nnobody:x:65534:65534::/:/usr/bin/nologin\n")
        open(self.group, "w").write("wheel:x:998:zlothra,claude-agent\nusers:x:985:\naudio:x:996:\nvideo:x:983:\nstorage:x:100:\nlp:x:7:\n")
        for name, body in [("useradd", 'echo "useradd $*" >> %s' % self.calls), ("userdel", 'echo "userdel $*" >> %s' % self.calls),
                           ("gpasswd", 'echo "gpasswd $*" >> %s' % self.calls), ("chpasswd", 'echo "chpasswd $(cat)" >> %s\n[ -z "$CHPASSWD_FAIL" ]' % self.calls),
                           ("getent", "exit 2"), ("loginctl", 'echo "$SESSIONS"')]:
            p = os.path.join(self.d, name)
            open(p, "w").write("#!/bin/sh\n" + body + "\n")
            os.chmod(p, os.stat(p).st_mode | stat.S_IEXEC)
        # what an operating system keeps for itself: here an assistant account and a service account
        self.protected = os.path.join(self.d, "protected-accounts")
        open(self.protected, "w").write("# the OS's own\nclaude-agent*\nsarina\n")
        self.env = {"PATH": self.d + ":/usr/bin:/bin", "YGGDRASIL_PASSWD": self.passwd, "YGGDRASIL_GROUP": self.group, "YGGDRASIL_ACCOUNT_TEST": "1", "PKEXEC_UID": "1000",
                    "YGGDRASIL_PROTECTED_ACCOUNTS": self.protected}

    def run_it(self, *args, stdin="", env=None):
        return subprocess.run(["python3", BIN, *args], input=stdin, capture_output=True, text=True, env=dict(self.env, **(env or {})))

    def calls_text(self):
        return open(self.calls).read() if os.path.exists(self.calls) else ""

    def test_list_shows_login_accounts_and_who_is_admin(self):
        accts = json.loads(self.run_it("list").stdout)
        self.assertEqual([(a["login"], a["admin"]) for a in accts], [("zlothra", True), ("claude-agent", True), ("sam", False)])
        self.assertEqual(accts[2]["full_name"], "Sam Doe")

    def test_add_standard_user_never_puts_the_password_on_a_command_line(self):
        r = self.run_it("add", "alex", "--full-name", "Alex: The Great\nroot", stdin="correct horse 9\n")
        self.assertEqual(r.returncode, 0, r.stderr)
        c = self.calls_text()
        self.assertIn("useradd -m -s /bin/bash -c Alex  The Great root -G users,audio,video,storage,lp alex", c.replace("  ", "  "))
        self.assertNotIn("wheel", c.split("chpasswd")[0])
        self.assertIn("chpasswd alex:correct horse 9", c)
        self.assertNotIn("correct horse", c.split("chpasswd")[0])   # not in the useradd line

    def test_add_administrator(self):
        self.assertEqual(self.run_it("add", "pat", "--admin", stdin="longenough1\n").returncode, 0)
        self.assertIn("wheel", self.calls_text().split("chpasswd")[0])

    def test_bad_names_short_passwords_and_duplicates_are_refused(self):
        for bad in ["Root", "1abc", "a b", "a:b", "../x", "", "root", "sarina", "claude-agent2", "x" * 40]:
            self.assertNotEqual(self.run_it("add", bad, stdin="longenough1\n").returncode, 0, bad)
        self.assertNotEqual(self.run_it("add", "alex", stdin="short\n").returncode, 0)
        self.assertNotEqual(self.run_it("add", "sam", stdin="longenough1\n").returncode, 0)  # exists
        self.assertNotIn("useradd", self.calls_text())

    def test_a_failed_password_rolls_the_account_back(self):
        r = self.run_it("add", "alex", stdin="longenough1\n", env={"CHPASSWD_FAIL": "1"})
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("userdel -r alex", self.calls_text())

    def test_remove_rules(self):
        self.assertEqual(self.run_it("remove", "sam").returncode, 0)
        self.assertIn("userdel sam", self.calls_text())
        self.assertNotIn("-r", self.calls_text().split("userdel")[1])                      # home kept by default
        self.assertEqual(self.run_it("remove", "sam", "--delete-home").returncode, 0)
        self.assertIn("userdel -r sam", self.calls_text())
        for who in ["zlothra", "claude-agent", "root", "nobody", "nosuch"]:
            self.assertNotEqual(self.run_it("remove", who).returncode, 0, who)                # yourself, the agent, system accounts
        self.assertNotEqual(self.run_it("remove", "sam", env={"SESSIONS": "1 1002 sam seat0"}).returncode, 0)  # logged in

    def test_admin_changes_and_the_last_administrator(self):
        self.assertEqual(self.run_it("admin", "sam", "on").returncode, 0)
        self.assertIn("gpasswd -a sam wheel", self.calls_text())
        self.assertNotEqual(self.run_it("admin", "zlothra", "off").returncode, 0)             # your own rights
        self.assertNotEqual(self.run_it("admin", "claude-agent", "off").returncode, 0)        # not ours to change here
        open(self.group, "w").write("wheel:x:998:sam,claude-agent\nusers:x:985:\n")
        self.assertNotEqual(self.run_it("admin", "sam", "off", env={"PKEXEC_UID": "1000"}).returncode, 0)   # the only human administrator

    def test_needs_root_when_not_testing(self):
        env = {k: v for k, v in self.env.items() if k != "YGGDRASIL_ACCOUNT_TEST"}
        r = subprocess.run(["python3", BIN, "add", "alex"], input="longenough1\n", capture_output=True, text=True, env=env)
        self.assertNotEqual(r.returncode, 0)
        self.assertIn("root", r.stderr)


if __name__ == "__main__":
    unittest.main()
