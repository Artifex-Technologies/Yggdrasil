"""Tests for ygg-theme (colour scheme, GTK colours, settings files) and ygg-cursors (the cursor theme). Run: python tests/test_ygg_theme.py"""
import importlib.machinery
import importlib.util
import os
import struct
import subprocess
import sys
import tempfile
import unittest

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BIN = os.path.join(ROOT, "packages", "theme", "bin")


def load(name):
    path = os.path.join(BIN, name)
    loader = importlib.machinery.SourceFileLoader(name.replace("-", "_"), path)
    spec = importlib.util.spec_from_loader(name.replace("-", "_"), loader)
    mod = importlib.util.module_from_spec(spec)
    loader.exec_module(mod)
    return mod


theme = load("ygg-theme")


def parse_ini(text):
    sections, cur = {}, None
    for line in text.splitlines():
        line = line.strip()
        if line.startswith("["):
            cur = line
            sections[cur] = {}
        elif "=" in line and cur:
            k, v = line.split("=", 1)
            sections[cur][k] = v
    return sections


class Scheme(unittest.TestCase):
    def test_every_section_has_every_key(self):
        for dark in (True, False):
            s = parse_ini(theme.scheme(dark, (124, 84, 214)))
            for sec in ("Button", "Complementary", "Header", "Selection", "Tooltip", "View", "Window"):
                keys = s[f"[Colors:{sec}]"]
                for k in ("BackgroundAlternate", "BackgroundNormal", "DecorationFocus", "DecorationHover", "ForegroundActive", "ForegroundInactive", "ForegroundLink",
                          "ForegroundNegative", "ForegroundNeutral", "ForegroundNormal", "ForegroundPositive", "ForegroundVisited"):
                    self.assertIn(k, keys, (sec, k))
                    self.assertEqual(len(keys[k].split(",")), 3)

    def test_the_name_says_dark_or_light(self):
        # the apps decide dark or light from the scheme's name
        self.assertEqual(parse_ini(theme.scheme(True, (1, 2, 3)))["[General]"]["ColorScheme"], "YggdrasilDark")
        self.assertEqual(parse_ini(theme.scheme(False, (1, 2, 3)))["[General]"]["ColorScheme"], "YggdrasilLight")

    def test_the_accent_is_the_selection(self):
        s = parse_ini(theme.scheme(True, (10, 200, 30)))
        self.assertEqual(s["[Colors:Selection]"]["BackgroundNormal"], "10,200,30")

    def test_text_on_a_light_accent_is_dark(self):
        s = parse_ini(theme.scheme(True, (255, 255, 255)))
        self.assertEqual(s["[Colors:Selection]"]["ForegroundNormal"], "24,20,36")
        s = parse_ini(theme.scheme(True, (124, 84, 214)))
        self.assertEqual(s["[Colors:Selection]"]["ForegroundNormal"], "255,255,255")

    def test_text_is_readable_on_its_background(self):
        for dark in (True, False):
            s = parse_ini(theme.scheme(dark, (124, 84, 214)))
            for sec in ("Window", "View", "Button"):
                bg = [int(x) for x in s[f"[Colors:{sec}]"]["BackgroundNormal"].split(",")]
                fg = [int(x) for x in s[f"[Colors:{sec}]"]["ForegroundNormal"].split(",")]
                lb, lf = theme.lum(bg), theme.lum(fg)
                ratio = (max(lb, lf) + 0.05) / (min(lb, lf) + 0.05)
                self.assertGreater(ratio, 7.0, (dark, sec, ratio))

    def test_gtk_colours_carry_the_accent(self):
        g = theme.gtk_colors(True, (10, 200, 30))
        self.assertIn("@define-color accent_bg_color rgb(10,200,30);", g)
        self.assertIn("@define-color theme_selected_bg_color rgb(10,200,30);", g)


class Settings(unittest.TestCase):
    def test_ini_set_keeps_the_rest(self):
        d = tempfile.mkdtemp()
        p = os.path.join(d, "kdeglobals")
        open(p, "w").write("[General]\nfoo=1\nColorScheme=Old\n\n[Other]\nx=2\n")
        theme.ini_set(p, "General", "ColorScheme", "New")
        theme.ini_set(p, "General", "bar", "3")
        theme.ini_set(p, "Icons", "Theme", "Liki")
        s = parse_ini(open(p).read())
        self.assertEqual(s["[General]"], {"foo": "1", "ColorScheme": "New", "bar": "3"})
        self.assertEqual(s["[Other]"], {"x": "2"})
        self.assertEqual(s["[Icons]"], {"Theme": "Liki"})

    def test_apply_writes_everything_and_keeps_the_users_own_css(self):
        d = tempfile.mkdtemp()
        env = dict(os.environ, HOME=d, XDG_CONFIG_HOME=os.path.join(d, ".config"), XDG_DATA_HOME=os.path.join(d, ".local/share"), PATH="/nonexistent")
        os.makedirs(os.path.join(d, ".config/gtk-4.0"))
        open(os.path.join(d, ".config/gtk-4.0/gtk.css"), "w").write("@import 'colors.css';\nwindow { border-radius: 3px; }\n")
        r = subprocess.run([sys.executable, os.path.join(BIN, "ygg-theme"), "apply", "--dark", "--accent", "12,34,56"], env=env, capture_output=True, text=True)
        self.assertEqual(r.returncode, 0, r.stderr)
        k = parse_ini(open(os.path.join(d, ".config/kdeglobals")).read())
        self.assertEqual(k["[General]"]["ColorScheme"], "YggdrasilDark")
        self.assertEqual(k["[General]"]["AccentColor"], "12,34,56")
        self.assertEqual(k["[KDE]"]["widgetStyle"], "Fusion")
        self.assertEqual(k["[Icons]"]["Theme"], "Liki")
        self.assertTrue(os.path.isfile(os.path.join(d, ".local/share/color-schemes/YggdrasilDark.colors")))
        g3 = parse_ini(open(os.path.join(d, ".config/gtk-3.0/settings.ini")).read())["[Settings]"]
        self.assertEqual((g3["gtk-theme-name"], g3["gtk-icon-theme-name"], g3["gtk-cursor-theme-name"]), ("Yggdrasil", "Liki", "Yggdrasil"))
        css = open(os.path.join(d, ".config/gtk-4.0/gtk.css")).read()
        self.assertIn("rgb(12,34,56)", css)
        self.assertIn("border-radius: 3px", css)
        self.assertNotIn("colors.css", css)
        # applying again does not pile things up
        subprocess.run([sys.executable, os.path.join(BIN, "ygg-theme"), "apply", "--light"], env=env, capture_output=True)
        css2 = open(os.path.join(d, ".config/gtk-4.0/gtk.css")).read()
        self.assertEqual(css2.count("border-radius: 3px"), 1)
        self.assertEqual(parse_ini(open(os.path.join(d, ".config/kdeglobals")).read())["[General]"]["ColorScheme"], "YggdrasilLight")


class Cursors(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.out = tempfile.mkdtemp()
        r = subprocess.run([sys.executable, os.path.join(BIN, "ygg-cursors"), cls.out], capture_output=True, text=True)
        assert r.returncode == 0, r.stderr

    def read(self, name):
        data = open(os.path.join(self.out, "cursors", name), "rb").read()
        magic, hdr, ver, n = struct.unpack("<4sIII", data[:16])
        self.assertEqual((magic, hdr), (b"Xcur", 16))
        sizes, chunks = set(), []
        for i in range(n):
            t, sub, pos = struct.unpack("<III", data[16 + 12 * i:28 + 12 * i])
            self.assertEqual(t, 0xFFFD0002)
            head = struct.unpack("<IIIIIIIII", data[pos:pos + 36])
            sizes.add(sub)
            chunks.append(head)
            w, h = head[4], head[5]
            self.assertEqual(w, sub)
            self.assertEqual(h, sub)
            self.assertLess(head[6], w)
            self.assertLess(head[7], h)
            self.assertEqual(len(data[pos + 36:pos + 36 + w * h * 4]), w * h * 4)
        return sizes, chunks

    def test_the_names_programs_ask_for_exist(self):
        for n in ("left_ptr", "default", "text", "pointer", "hand2", "wait", "progress", "crosshair", "not-allowed", "grabbing", "ew-resize", "ns-resize", "nwse-resize",
                  "nesw-resize", "move", "help", "copy", "context-menu", "col-resize", "row-resize", "zoom-in", "all-scroll", "sb_h_double_arrow", "top_left_corner"):
            self.assertTrue(os.path.lexists(os.path.join(self.out, "cursors", n)), n)

    def test_files_are_well_formed_in_five_sizes(self):
        sizes, _ = self.read("left_ptr")
        self.assertEqual(sizes, {24, 32, 48, 64, 96})

    def test_the_busy_cursor_is_animated(self):
        sizes, chunks = self.read("wait")
        self.assertEqual(len(chunks), 12 * 5)
        self.assertTrue(all(c[8] == 80 for c in chunks))

    def test_the_theme_has_a_name(self):
        self.assertIn("Name=Yggdrasil", open(os.path.join(self.out, "index.theme")).read())


if __name__ == "__main__":
    unittest.main()
