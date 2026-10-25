"""yggdrasil-panelctl and yggdrasil-mediakey with stand-ins for busctl and wpctl: they must send the right calls. Run: python tests/test_panel_scripts.py"""
import os
import stat
import subprocess
import tempfile
import unittest

BIN = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "packages", "panel", "bin")


def run(script, args, busctl_out=""):
    d = tempfile.mkdtemp()
    log = os.path.join(d, "calls.log")
    for name, body in (("busctl", 'echo "busctl $*" >> %s; printf "%%s" "%s"' % (log, busctl_out)), ("wpctl", 'echo "wpctl $*" >> %s' % log), ("yggdrasil-clipboard", 'echo "yggdrasil-clipboard $*" >> %s' % log)):
        p = os.path.join(d, name)
        with open(p, "w") as f:
            f.write("#!/bin/sh\n" + body + "\n")
        os.chmod(p, os.stat(p).st_mode | stat.S_IEXEC)
    r = subprocess.run(["sh", os.path.join(BIN, script)] + args, capture_output=True, text=True, env={"PATH": d + ":/usr/bin:/bin"})
    calls = open(log).read().splitlines() if os.path.exists(log) else []
    return r.returncode, calls


class PanelCtl(unittest.TestCase):
    def test_commands_map_to_panel_methods(self):
        for arg, want in (("start", "ToggleStart"), ("calendar", "ToggleCalendar"), ("ask", "Ask"), ("notifications", "ShowFylgja u 3"), ("fylgja 2", "ShowFylgja u 2"), ("shortcuts", "ToggleShortcuts"), ("cursor-tap", "CursorTap")):
            rc, calls = run("yggdrasil-panelctl", arg.split())
            self.assertEqual(rc, 0, arg)
            self.assertEqual(len(calls), 1, arg)
            self.assertIn("org.yggdrasil.Panel /Panel org.yggdrasil.Panel " + want, calls[0])

    def test_clipboard_runs_the_window(self):
        rc, calls = run("yggdrasil-panelctl", ["clipboard"])
        self.assertEqual((rc, calls), (0, ["yggdrasil-clipboard "]))

    def test_unknown_command_is_an_error(self):
        rc, calls = run("yggdrasil-panelctl", ["nope"])
        self.assertEqual(rc, 2)
        self.assertEqual(calls, [])


class MediaKey(unittest.TestCase):
    def test_volume_and_mute(self):
        self.assertEqual(run("yggdrasil-mediakey", ["volume-up"])[1], ["wpctl set-volume -l 1.0 @DEFAULT_AUDIO_SINK@ 5%+"])
        self.assertEqual(run("yggdrasil-mediakey", ["volume-down"])[1], ["wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-"])
        self.assertEqual(run("yggdrasil-mediakey", ["mute"])[1], ["wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"])

    def test_player_keys_go_to_the_running_player(self):
        rc, calls = run("yggdrasil-mediakey", ["play-pause"], busctl_out="org.mpris.MediaPlayer2.spotify 1234 spotify")
        self.assertEqual(rc, 0)
        self.assertTrue(any("org.mpris.MediaPlayer2.spotify /org/mpris/MediaPlayer2 org.mpris.MediaPlayer2.Player PlayPause" in c for c in calls), calls)

    def test_no_player_is_not_an_error(self):
        rc, calls = run("yggdrasil-mediakey", ["next"], busctl_out="")
        self.assertEqual(rc, 0)
        self.assertFalse(any("Next" in c for c in calls))


if __name__ == "__main__":
    unittest.main()
