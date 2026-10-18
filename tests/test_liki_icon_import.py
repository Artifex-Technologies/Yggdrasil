"""Tests for liki-icon-import (Android icon pack -> Linux icon theme). Run: python tests/test_liki_icon_import.py"""
import importlib.machinery
import importlib.util
import os
import struct
import tempfile
import unittest
import zipfile
import zlib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
path = os.path.join(ROOT, "packages", "liki", "bin", "liki-icon-import")
loader = importlib.machinery.SourceFileLoader("labyrinth_icon_import", path)
spec = importlib.util.spec_from_loader("labyrinth_icon_import", loader)
ii = importlib.util.module_from_spec(spec)
loader.exec_module(ii)


def png(size, rgb=(120, 80, 210)):
    """A solid square PNG, made by hand."""
    raw = b"".join(b"\x00" + bytes(rgb) * size for _ in range(size))
    def chunk(t, d):
        c = struct.pack(">I", len(d)) + t + d
        return c + struct.pack(">I", zlib.crc32(t + d) & 0xFFFFFFFF)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b"")


def axml(items):
    """A compiled Android XML file with <resources><item component=.. drawable=../>...</resources> (UTF-16 string pool)."""
    strings = ["resources", "item", "component", "drawable"]
    for c, d in items:
        for s in (c, d):
            if s not in strings:
                strings.append(s)
    idx = {s: i for i, s in enumerate(strings)}
    pool_data, offs = b"", []
    for s in strings:
        offs.append(len(pool_data))
        pool_data += struct.pack("<H", len(s)) + s.encode("utf-16-le") + b"\x00\x00"
    while len(pool_data) % 4:
        pool_data += b"\x00"
    hsize = 28
    pool = struct.pack("<HHIIIIII", 1, hsize, hsize + 4 * len(strings) + len(pool_data), len(strings), 0, 0, hsize + 4 * len(strings), 0) + b"".join(struct.pack("<I", o) for o in offs) + pool_data
    body = b""
    def start(tag, attrs):
        a = b"".join(struct.pack("<iiiHBBI", -1, idx[k], idx[v], 8, 0, 3, idx[v]) for k, v in attrs)
        size = 36 + len(a)
        return struct.pack("<HHIII", 0x0102, 16, size, 1, 0xFFFFFFFF) + struct.pack("<iiHHHHHH", -1, idx[tag], 20, 20, len(attrs), 0, 0, 0) + a
    body += start("resources", [])
    for c, d in items:
        body += start("item", [("component", c), ("drawable", d)])
    total = 8 + len(pool) + len(body)
    return struct.pack("<HHI", 3, 8, total) + pool + body


def make_apk(tmp, appfilter_xml=None, compiled=None, pics=None):
    p = os.path.join(tmp, "Test Pack v1.2.3 (Patched).apk")
    with zipfile.ZipFile(p, "w") as z:
        if appfilter_xml:
            z.writestr("assets/appfilter.xml", appfilter_xml)
        if compiled:
            z.writestr("res/xml/appfilter.xml", compiled)
        for name, size in (pics or {}).items():
            z.writestr("res/drawable-nodpi/%s.png" % name, png(size))
    return p


class Import(unittest.TestCase):
    def test_default_name_drops_versions_and_tags(self):
        self.assertEqual(ii.default_name("/x/Camo Dark v1.8.1 (Patched).apk"), "Camo Dark")

    def test_plain_appfilter(self):
        xml = '<resources><item component="ComponentInfo{org.mozilla.firefox/org.mozilla.fennec.App}" drawable="firefox"/><item component="ComponentInfo{com.x/com.x.A}" drawable="other"/></resources>'
        with tempfile.TemporaryDirectory() as tmp:
            with zipfile.ZipFile(make_apk(tmp, appfilter_xml=xml, pics={"firefox": 192, "other": 192})) as z:
                self.assertEqual([d for _, d in ii.parse_appfilter(z)], ["firefox", "other"])

    def test_compiled_appfilter(self):
        comp = axml([("ComponentInfo{com.spotify.music/com.spotify.music.MainActivity}", "spotify")])
        with tempfile.TemporaryDirectory() as tmp:
            with zipfile.ZipFile(make_apk(tmp, compiled=comp, pics={"spotify": 256})) as z:
                self.assertEqual(ii.parse_appfilter(z), [("ComponentInfo{com.spotify.music/com.spotify.music.MainActivity}", "spotify")])

    def test_matching_uses_package_and_picture_names(self):
        items = [("ComponentInfo{org.mozilla.firefox/x.Y}", "ic_browser"), ("ComponentInfo{com.spotify.music/z.A}", "spotify")]
        pngs = {"ic_browser": (192, b""), "spotify": (192, b"")}
        apps = [("firefox", {"firefox", "firefoxwebbrowser"}), ("spotify-client", {"spotifyclient", "spotify"}), ("unrelated", {"unrelated"})]
        self.assertEqual(ii.match(items, pngs, apps), {"firefox": "ic_browser", "spotify-client": "spotify"})

    def test_generic_names_are_not_guessed(self):
        # two Android apps both called settings: the program called Settings gets the picture literally named "settings"
        items = [("ComponentInfo{com.android.settings/x.A}", "ic_phone"), ("ComponentInfo{com.samsung.settings/x.A}", "ic_other"), ("ComponentInfo{a.b/x.A}", "settings")]
        pngs = {"ic_phone": (192, b""), "ic_other": (192, b""), "settings": (192, b"")}
        self.assertEqual(ii.match(items, pngs, [("preferences-system", {"preferencessystem", "settings"})]), {"preferences-system": "settings"})
        # with no picture of that name, the ambiguous package name is left alone
        items2 = items[:2]
        self.assertEqual(ii.match(items2, {k: v for k, v in pngs.items() if k != "settings"}, [("preferences-system", {"settings"})]), {})

    def test_build_writes_a_theme(self):
        xml = '<resources><item component="ComponentInfo{org.videolan.vlc/x.Y}" drawable="vlc"/></resources>'
        with tempfile.TemporaryDirectory() as tmp:
            apk = make_apk(tmp, appfilter_xml=xml, pics={"vlc": 192})
            orig = ii.installed_apps
            ii.installed_apps = lambda: [("vlc", {"vlc"})]
            try:
                theme, n, total = ii.build(apk, "Test Pack", tmp)
            finally:
                ii.installed_apps = orig
            self.assertEqual((n, total), (1, 1))
            self.assertTrue(os.path.isfile(os.path.join(theme, "192x192", "apps", "vlc.png")))
            text = open(os.path.join(theme, "index.theme")).read()
            self.assertIn("Directories=192x192/apps", text)
            self.assertIn("Inherits=hicolor", text)

    def test_junk_is_refused(self):
        with tempfile.TemporaryDirectory() as tmp:
            p = os.path.join(tmp, "x.apk")
            with zipfile.ZipFile(p, "w") as z:
                z.writestr("readme.txt", "no icons")
            with self.assertRaises(SystemExit):
                ii.build(p, "x", tmp)
            self.assertFalse(ii.safe_icon_name("../evil"))


if __name__ == "__main__":
    unittest.main()
