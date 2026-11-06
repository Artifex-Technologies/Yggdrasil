"""yggdrasil-game-fix against a pretend Steam library: Dead Space 2 (a file made from catalogue lines, kv) and Fallout 4 (ini
files made from the game's templates, an existing file backed up), apply and undo, other lines kept, a game never started."""

import importlib.machinery
import importlib.util
import json
import os
import shutil
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
HELPER = os.path.join(HERE, "..", "game-fix")
CATALOGUE = os.path.join(HERE, "..", "..", "data", "game-fixes.json")


def load():
    loader = importlib.machinery.SourceFileLoader("game_fix", HELPER)
    spec = importlib.util.spec_from_loader("game_fix", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


m = load()


class GameFix(unittest.TestCase):
    def setUp(self):
        self.home = tempfile.mkdtemp(prefix="game-fix-")
        self.sa = os.path.join(self.home, ".local/share/Steam/steamapps")
        os.makedirs(self.sa)
        for appid, name, folder in (("47780", "Dead Space 2", "Dead Space 2"), ("377160", "Fallout 4", "Fallout 4")):
            with open(os.path.join(self.sa, f"appmanifest_{appid}.acf"), "w") as f:
                f.write(f'"AppState"\n{{\n\t"appid"\t\t"{appid}"\n\t"name"\t\t"{name}"\n\t"installdir"\t\t"{folder}"\n}}\n')
            os.makedirs(os.path.join(self.sa, "common", folder))
            os.makedirs(os.path.join(self.sa, "compatdata", appid, "pfx/drive_c/users/steamuser"))
        fo = os.path.join(self.sa, "common/Fallout 4")
        os.makedirs(os.path.join(fo, "Fallout4"))
        with open(os.path.join(fo, "Fallout4/Fallout4Prefs.ini"), "w", newline="") as f:
            f.write("[Display]\r\niPresentInterval=1\r\nbBorderless=1\r\nbFull Screen=0\r\niSize H=720\r\niSize W=1280\r\n\r\n[Imagespace]\r\nbDoDepthOfField=1\r\n")
        with open(os.path.join(fo, "Fallout4_Default.ini"), "w") as f:
            f.write("[General]\nsLanguage=en\n")
        os.environ["HOME"] = self.home
        os.environ["YGGDRASIL_SCREEN"] = "3440x1440"
        m.CATALOGUE = CATALOGUE
        self.fixes = {f["appid"]: f for f in m.load_catalogue()}
        self.games = m.installed(self.home)

    def tearDown(self):
        shutil.rmtree(self.home)
        os.environ.pop("YGGDRASIL_SCREEN", None)

    def ds2_file(self):
        return os.path.join(self.sa, "compatdata/47780/pfx/drive_c/users/steamuser/AppData/Local/EA Games/Dead Space 2/settings.txt")

    def prefs(self):
        return os.path.join(self.sa, "compatdata/377160/pfx/drive_c/users/steamuser/Documents/My Games/Fallout4/Fallout4Prefs.ini")

    def test_the_catalogue_reads(self):
        self.assertEqual(set(self.fixes), {"47780", "377160"})
        for f in self.fixes.values():
            self.assertTrue(f["title"] and f["files"])

    def test_dead_space_2_made_applied_and_undone(self):
        fix = self.fixes["47780"]
        self.assertEqual(m.status(fix, self.games)[0], "not applied")
        m.apply(fix, self.games)
        with open(self.ds2_file(), newline="") as f:
            text = f.read()
        self.assertIn("Window.Width = 3440\r\n", text)
        self.assertIn("Window.Height = 1440\r\n", text)
        self.assertIn("Audio.Output = 0\r\n", text, "the rest of the defaults are there")
        self.assertEqual(m.status(fix, self.games)[0], "applied")
        m.undo(fix, self.games)
        self.assertFalse(os.path.exists(self.ds2_file()), "a file it made is removed again")

    def test_an_existing_file_keeps_its_other_lines_and_comes_back(self):
        fix = self.fixes["47780"]
        os.makedirs(os.path.dirname(self.ds2_file()))
        with open(self.ds2_file(), "w", newline="") as f:
            f.write("Audio.Output = 2\r\nWindow.Width = 1024\r\n")
        m.apply(fix, self.games)
        with open(self.ds2_file(), newline="") as f:
            text = f.read()
        self.assertIn("Audio.Output = 2\r\n", text, "the player's own settings stay")
        self.assertIn("Window.Width = 3440\r\n", text)
        self.assertIn("Window.VSync = False\r\n", text, "a missing key is added")
        m.apply(fix, self.games)
        m.undo(fix, self.games)
        with open(self.ds2_file(), newline="") as f:
            self.assertEqual(f.read(), "Audio.Output = 2\r\nWindow.Width = 1024\r\n", "undo gives the file back as it was, even after two applies")

    def test_fallout_4_files_from_its_templates(self):
        fix = self.fixes["377160"]
        m.apply(fix, self.games)
        with open(self.prefs(), newline="") as f:
            text = f.read()
        self.assertIn("iSize W=3440\r\n", text)
        self.assertIn("iSize H=1440\r\n", text)
        self.assertIn("iPresentInterval=1\r\n", text, "the template's other lines are kept")
        self.assertIn("[Imagespace]\r\nbDoDepthOfField=1", text)
        self.assertTrue(os.path.isfile(os.path.join(os.path.dirname(self.prefs()), "Fallout4.ini")), "made from Fallout4_Default.ini")
        self.assertEqual(m.status(fix, self.games)[0], "applied")
        m.undo(fix, self.games)
        self.assertFalse(os.path.exists(self.prefs()))

    def test_fallout_4_offers_its_launch_option(self):
        f = self.fixes["377160"]
        self.assertIn("Fallout4Launcher.exe/Fallout4.exe", f["launch"])
        self.assertTrue(f["launch"].endswith("%command%"))
        self.assertEqual(self.fixes["47780"].get("launch", ""), "", "Dead Space 2 needs none")

    def test_ini_keys_go_into_their_section(self):
        lines = ["[Display]", "iSize W=1280", "", "[Other]", "x=1"]
        lines = m.ini_set(lines, "Display", "bBorderless", "1")
        self.assertEqual(lines[:3], ["[Display]", "iSize W=1280", "bBorderless=1"])
        lines = m.ini_set(lines, "New", "k", "v")
        self.assertEqual(lines[-2:], ["[New]", "k=v"])
        self.assertEqual(m.ini_get(lines, "display", "isize w"), "1280", "names compare without case, as the games do")

    def test_a_game_never_started_waits(self):
        shutil.rmtree(os.path.join(self.sa, "compatdata/47780"))
        self.assertEqual(m.status(self.fixes["47780"], self.games)[0], "start once")


if __name__ == "__main__":
    unittest.main()
