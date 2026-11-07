"""Tests for yggdrasil-install-system's pure parts. python3 -m unittest discover -s helpers/tests"""
import importlib.machinery
import importlib.util
import os
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
TOOL = os.path.join(os.path.dirname(HERE), "install-system")
loader = importlib.machinery.SourceFileLoader("install_system", TOOL)
spec = importlib.util.spec_from_loader("install_system", loader)
inst = importlib.util.module_from_spec(spec)
loader.exec_module(inst)

MODEL_MAP = """# consolelayout\txlayout\txmodel\txvariant\txoptions\tbcp47
uk\tgb\tpc105\t-\tterminate:ctrl_alt_bksp\ten-GB
us\tus\tpc105+inet\t-\tterminate:ctrl_alt_bksp\ten-US,en
dvorak\tus\tpc105\tdvorak\tterminate:ctrl_alt_bksp\t-
es-dvorak\tes\tmicrosoftpro\tdvorak\tterminate:ctrl_alt_bksp\t-
"""


class ConsoleKeymap(unittest.TestCase):
    def setUp(self):
        self.root = tempfile.mkdtemp()
        os.makedirs(os.path.join(self.root, "usr/share/systemd"))
        open(os.path.join(self.root, "usr/share/systemd/kbd-model-map"), "w").write(MODEL_MAP)
        for name in ("colemak", "fr"):
            d = os.path.join(self.root, "usr/share/kbd/keymaps/i386", name)
            os.makedirs(d, exist_ok=True)
            open(os.path.join(d, name + ".map.gz"), "w").close()

    def test_the_console_gets_its_own_name_for_a_layout(self):
        k = lambda layout, variant="": inst.console_keymap(layout, variant, self.root)
        self.assertEqual(k("gb"), "uk")
        self.assertEqual(k("us", "dvorak"), "dvorak")
        self.assertEqual(k("es", "dvorak"), "es-dvorak")
        self.assertEqual(k("us", "colemak"), "colemak", "a console keymap named after the variant")
        self.assertEqual(k("fr"), "fr", "not in the table: a console keymap of the same name")
        self.assertEqual(k("zz"), "us", "nothing fits")

    def test_a_variant_is_checked_like_a_layout(self):
        plan = {"disk": "/dev/vda", "mode": "erase", "encrypt": False, "user": {"name": "A", "login": "a", "password": "longenough"},
                "hostname": "x", "timezone": "UTC", "locale": "en_US.UTF-8", "keymap": "us", "keymap_variant": "dvorak; rm"}
        self.assertEqual(inst.check_plan(plan), "unknown keyboard variant")


GB = 1024**3
MIB = 1024**2
# a 500 GB disk with Windows on it: its EFI partition, Windows itself, and 200 GB free at the end
DISK = {"path": "/dev/vda", "size": 500 * GB, "table": "gpt", "partitions": [
    {"path": "/dev/vda1", "number": 1, "start": MIB, "size": 300 * MIB, "fstype": "vfat", "label": "EFI", "efi": True, "mounted": False},
    {"path": "/dev/vda2", "number": 2, "start": 301 * MIB, "size": 299 * GB, "fstype": "ntfs", "label": "Windows", "efi": False, "mounted": False}]}


def manual(**kw):
    p = {"disk": "/dev/vda", "mode": "manual", "encrypt": False, "user": {"name": "A", "login": "a", "password": "longenough"},
         "hostname": "x", "timezone": "UTC", "locale": "en_US.UTF-8", "keymap": "us",
         "create": [{"id": "r", "start": 300 * GB, "size": 150 * GB, "kind": "linux"}],
         "use": [{"part": "/dev/vda1", "mount": "/boot/efi", "format": None}, {"part": "new:r", "mount": "/", "format": "btrfs"}]}
    p.update(kw)
    return p


class Manual(unittest.TestCase):
    def test_free_space_lies_between_the_partitions(self):
        gaps = inst.free_space([(q["start"], q["size"]) for q in DISK["partitions"]], MIB, DISK["size"] - MIB)
        self.assertEqual(len(gaps), 1)
        self.assertGreaterEqual(gaps[0]["start"], 301 * MIB + 299 * GB)
        self.assertEqual(gaps[0]["start"] % MIB, 0)
        self.assertEqual(inst.free_space([], MIB, 100 * MIB), [{"start": MIB, "size": 99 * MIB}])
        self.assertEqual(inst.free_space([(2 * MIB, 10 * MIB)], MIB, 12 * MIB), [], "a 1 MiB sliver is not free space")

    def test_beside_windows_on_its_own_efi_partition(self):
        p = manual()
        self.assertIsNone(inst.check_plan(p))
        self.assertIsNone(inst.manual_fits(p, DISK))

    def test_what_a_plan_needs(self):
        self.assertIn("exactly one", inst.check_plan(manual(use=[{"part": "/dev/vda1", "mount": "/boot/efi", "format": None}])))
        self.assertIn("EFI", inst.check_plan(manual(use=[{"part": "new:r", "mount": "/", "format": "btrfs"}])))
        self.assertIn("formatted", inst.check_plan(manual(use=[{"part": "/dev/vda1", "mount": "/boot/efi", "format": None},
                                                               {"part": "/dev/vda2", "mount": "/", "format": None}])))
        self.assertIn("cannot be formatted as", inst.check_plan(manual(use=[{"part": "/dev/vda1", "mount": "/boot/efi", "format": "ext4"},
                                                                            {"part": "new:r", "mount": "/", "format": "btrfs"}])))
        self.assertIn("mount point", inst.check_plan(manual(use=[{"part": "/dev/vda1", "mount": "/etc", "format": None}])))

    def test_against_the_disk(self):
        self.assertIn("does not fit", inst.manual_fits(manual(create=[{"id": "r", "start": 200 * GB, "size": 150 * GB, "kind": "linux"}]), DISK),
                      "overlaps Windows")
        self.assertIn("at least", inst.manual_fits(manual(create=[{"id": "r", "start": 300 * GB, "size": 10 * GB, "kind": "linux"}]), DISK))
        # deleting Windows frees its space
        p = manual(delete=[2], create=[{"id": "r", "start": 301 * MIB, "size": 400 * GB, "kind": "linux"}])
        self.assertIsNone(inst.manual_fits(p, DISK))
        busy = dict(DISK, partitions=[dict(DISK["partitions"][0], mounted=True), DISK["partitions"][1]])
        self.assertIn("in use", inst.manual_fits(manual(), busy))
        self.assertIn("no partition table", inst.manual_fits(manual(), dict(DISK, table="", partitions=[])))



BROWSERS = {"browsers": [
    {"id": "vivaldi", "name": "Vivaldi", "chosen": True, "packages": ["vivaldi", "vivaldi-ffmpeg-codecs"], "desktop": ["vivaldi-stable.desktop"]},
    {"id": "firefox", "name": "Firefox", "packages": ["firefox"], "desktop": ["firefox.desktop"]},
    {"id": "librewolf", "name": "LibreWolf", "packages": ["librewolf"], "desktop": ["librewolf.desktop"]},
    {"id": "opera-gx", "name": "Opera GX", "flatpak": "com.opera.opera-gx", "desktop": ["com.opera.opera-gx.desktop"]},
    {"name": "no id"}]}
EXTRAS = {"extras": [{"id": "assistant", "name": "Assistant", "components": ["assistant"]},
                     {"id": "monitor", "name": "Monitor", "components": ["monitor"]}]}
COMPONENTS = {"components": [{"id": "assistant", "packages": ["assistant-git"]},
                             {"id": "monitor", "packages": ["monitor-git", "clamav"], "enable_system": ["monitor.service"]}]}


def written(data):
    d = tempfile.mkdtemp()
    f = os.path.join(d, "list.json")
    with open(f, "w") as out:
        import json
        json.dump(data, out)
    return f


def plan(**kw):
    p = {"disk": "/dev/vda", "mode": "erase", "encrypt": False, "user": {"name": "A", "login": "a", "password": "longenough"},
         "hostname": "x", "timezone": "UTC", "locale": "en_US.UTF-8", "keymap": "us"}
    p.update(kw)
    return p


class Browsers(unittest.TestCase):
    def setUp(self):
        self.file = written(BROWSERS)
        self.cat = inst.browser_catalog(self.file)

    def test_entries_without_an_id_are_left_out(self):
        self.assertEqual([b["id"] for b in self.cat], ["vivaldi", "firefox", "librewolf", "opera-gx"])
        self.assertEqual(inst.browser_catalog("/nonexistent.json"), [])

    def test_a_choice_removes_the_others_and_installs_its_own(self):
        p = inst.browser_plan(self.cat, ["librewolf", "opera-gx"])
        self.assertEqual(p["remove"], ["vivaldi", "vivaldi-ffmpeg-codecs", "firefox"])
        self.assertEqual(p["install"], ["librewolf"])
        self.assertEqual(p["flatpaks"], ["com.opera.opera-gx"])
        none = inst.browser_plan(self.cat, [])
        self.assertEqual((none["install"], none["flatpaks"]), ([], []))

    def test_the_plan_names_known_browsers_only(self):
        old = inst.BROWSERS_FILE
        inst.BROWSERS_FILE = self.file
        try:
            self.assertIsNone(inst.check_plan(plan()), "no browsers key: the medium's browsers stay")
            self.assertIsNone(inst.check_plan(plan(browsers=["librewolf", "firefox"], default_browser="firefox")))
            self.assertIsNone(inst.check_plan(plan(browsers=[])))
            self.assertEqual(inst.check_plan(plan(browsers=["duckduckgo"])), "unknown browser chosen")
            self.assertEqual(inst.check_plan(plan(browsers="vivaldi")), "unknown browser chosen")
            self.assertEqual(inst.check_plan(plan(browsers=["firefox"], default_browser="vivaldi")), "the default browser must be one of those chosen")
        finally:
            inst.BROWSERS_FILE = old
        self.assertEqual(inst.check_plan(plan(browsers=["firefox"])), "unknown browser chosen", "an OS without a list offers none")

    def test_the_default_browser_opens_links_and_sits_in_the_dock(self):
        mime = "# comment\n[Default Applications]\nx-scheme-handler/http=vivaldi-stable.desktop\ntext/html=vivaldi-stable.desktop\nvideo/mp4=yggdrasil-player.desktop\n"
        out = inst.mimeapps_with_browser(mime, "librewolf.desktop")
        self.assertIn("x-scheme-handler/http=librewolf.desktop", out)
        self.assertIn("x-scheme-handler/https=librewolf.desktop", out, "a missing key is added")
        self.assertIn("video/mp4=yggdrasil-player.desktop", out)
        self.assertNotIn("vivaldi", out)
        self.assertIn("text/html=x.desktop", inst.mimeapps_with_browser("", "x.desktop"))
        dock = "# pins\nminos\nvivaldi-stable\nfirefox\nsteam\n"
        ids = {"vivaldi-stable", "firefox", "librewolf"}
        self.assertEqual(inst.dock_with_browser(dock, ids, "librewolf"), "# pins\nminos\nlibrewolf\nsteam\n")
        self.assertEqual(inst.dock_with_browser(dock, ids, None), "# pins\nminos\nsteam\n")


class Extras(unittest.TestCase):
    def setUp(self):
        self.file = written(EXTRAS)
        self.cat = inst.extras_catalog(self.file)
        self.comps = inst.components(written(COMPONENTS))

    def test_on_the_medium_kept_or_removed_otherwise_left_for_later(self):
        everything = {"assistant-git", "monitor-git", "clamav"}
        p = inst.extras_plan(self.cat, ["monitor"], self.comps, everything)
        self.assertEqual((p["remove"], p["keep"], p["queue"]), (["assistant-git"], ["monitor"], []))
        p = inst.extras_plan(self.cat, ["assistant", "monitor"], self.comps, set())
        self.assertEqual((p["remove"], p["keep"], p["queue"]), ([], [], ["assistant", "monitor"]))
        p = inst.extras_plan(self.cat, [], self.comps, {"monitor-git"})
        self.assertEqual((p["remove"], p["keep"], p["queue"]), (["monitor-git"], [], []), "what the medium lacks is not removed")

    def test_the_plan_names_known_parts_only(self):
        old = inst.EXTRAS_FILE
        inst.EXTRAS_FILE = self.file
        try:
            self.assertIsNone(inst.check_plan(plan(extras=["assistant"])))
            self.assertIsNone(inst.check_plan(plan(extras=[])))
            self.assertEqual(inst.check_plan(plan(extras=["steam"])), "unknown optional part chosen")
            self.assertEqual(inst.check_plan(plan(extras="assistant")), "unknown optional part chosen")
        finally:
            inst.EXTRAS_FILE = old


class TheOperatingSystem(unittest.TestCase):
    """What belongs to the OS comes from its installer integration file and os-release."""

    def tearDown(self):
        inst.configure({}, {})

    def folder(self, files):
        d = tempfile.mkdtemp()
        for name, text in files.items():
            with open(os.path.join(d, name), "w") as f:
                f.write(text)
        return d

    def test_the_first_installer_integration_by_name_as_the_window_picks_it(self):
        share = self.folder({"a-branding.conf": "kind = branding\nname = X\n",
                             "b-installer.conf": "kind = installer\ncommand = /usr/bin/yggdrasil-install-system\nname = Bee # the OS\nempty =\n",
                             "c-installer.conf": "kind = installer\nname = Cee\n", "notes.txt": "kind = installer\n"})
        conf = inst.installer_integration([share])
        self.assertEqual(conf["name"], "Bee", "a comment is cut off")
        self.assertNotIn("empty", conf)
        etc = self.folder({"b-installer.conf": "kind = installer\nname = Admin\n"})
        self.assertEqual(inst.installer_integration([share, etc])["name"], "Admin", "the administrator's file of the same name wins")
        self.assertEqual(inst.installer_integration([self.folder({})]), {})

    def test_names_paths_and_labels_follow_the_os(self):
        release = inst.os_release(os.path.join(self.folder({"os-release": 'NAME="Niflheim OS"\nID=niflheim\n'}), "os-release"))
        inst.configure({"cleanup": "/usr/local/bin/niflheim-install-cleanup", "finish": "relative/finish", "boot_menu_theme": "/a/../b",
                        "kernel_options": "quiet splash", "groups": "users wheel", "min_size_gb": "40", "first_login": "/var/lib/n/first.json"},
                       release)
        self.assertEqual((inst.OS_NAME, inst.OS_ID), ("Niflheim", "niflheim"), "os-release's name without the ' OS'")
        self.assertEqual(inst.SFS, "/run/archiso/bootmnt/niflheim/x86_64/airootfs.sfs")
        self.assertEqual((inst.TARGET, inst.MAPPER), ("/mnt/niflheim-install", "niflheim-root"))
        self.assertEqual(inst.CLEANUP, "/usr/local/bin/niflheim-install-cleanup")
        self.assertIsNone(inst.FINISH, "only absolute paths")
        self.assertIsNone(inst.BOOT_THEME, "no '..'")
        self.assertEqual(inst.KERNEL_OPTIONS, ["quiet", "splash"])
        self.assertEqual(inst.GROUPS, ["users", "wheel"])
        self.assertEqual(inst.MIN_BYTES, 40 * 1024**3)
        self.assertEqual(inst.FIRST_LOGIN_FILE, "/var/lib/n/first.json")
        self.assertEqual(inst.GRUB_DEFAULTS["GRUB_DISTRIBUTOR"], "Niflheim")
        inst.configure({"name": "Labyrinth", "medium": "/run/archiso/bootmnt/lab/x86_64/airootfs.sfs"}, {"NAME": "Labyrinth OS", "ID": "labyrinth"})
        self.assertEqual((inst.OS_NAME, inst.SFS), ("Labyrinth", "/run/archiso/bootmnt/lab/x86_64/airootfs.sfs"))

    def test_a_system_that_says_nothing(self):
        inst.configure({}, {})
        self.assertEqual((inst.OS_NAME, inst.OS_ID), ("Linux", "linux"))
        self.assertIsNone(inst.CLEANUP)
        self.assertIsNone(inst.BROWSERS_FILE)
        self.assertEqual(inst.browser_catalog(), [], "no list: no Browsers page and no browsers in a plan")

    def test_account_names_the_system_uses_are_taken(self):
        inst.configure({"reserved_logins": "assistant monitor"}, {})
        for login in ("root", "live", "greeter", "assistant", "monitor", "nobody"):
            self.assertEqual(inst.check_plan(plan(user={"name": "A", "login": login, "password": "x"})), "that account name is taken by the system", login)
        self.assertIsNone(inst.check_plan(plan()))


if __name__ == "__main__":
    unittest.main()
