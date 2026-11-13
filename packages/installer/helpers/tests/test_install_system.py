"""Tests for the Python yggdrasil-install-system (helpers/python, kept for a rollback; the program installed is the Rust port in
packages/install-system, compared with this one by its parity/compare.py). python3 -m unittest discover -s helpers/tests"""
import contextlib
import importlib.machinery
import importlib.util
import io
import os
import shutil
import stat
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
# the Python script, kept for a rollback since the Rust port (packages/install-system) replaced it
TOOL = os.path.join(os.path.dirname(HERE), "python", "install-system")
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
        release = inst.os_release(os.path.join(self.folder({"os-release": 'NAME="LykanOS"\nID=lykanos\n'}), "os-release"))
        inst.configure({"cleanup": "/usr/local/bin/lykanos-install-cleanup", "finish": "relative/finish", "boot_menu_theme": "/a/../b",
                        "kernel_options": "quiet splash", "groups": "users wheel", "min_size_gb": "40", "first_login": "/var/lib/n/first.json"},
                       release)
        self.assertEqual((inst.OS_NAME, inst.OS_ID), ("LykanOS", "lykanos"), "os-release's name (nothing to drop)")
        self.assertEqual(inst.SFS, "/run/archiso/bootmnt/lykanos/x86_64/airootfs.sfs")
        self.assertEqual((inst.TARGET, inst.MAPPER), ("/mnt/lykanos-install", "lykanos-root"))
        self.assertEqual(inst.CLEANUP, "/usr/local/bin/lykanos-install-cleanup")
        self.assertIsNone(inst.FINISH, "only absolute paths")
        self.assertIsNone(inst.BOOT_THEME, "no '..'")
        self.assertEqual(inst.KERNEL_OPTIONS, ["quiet", "splash"])
        self.assertEqual(inst.GROUPS, ["users", "wheel"])
        self.assertEqual(inst.MIN_BYTES, 40 * 1024**3)
        self.assertEqual(inst.FIRST_LOGIN_FILE, "/var/lib/n/first.json")
        self.assertEqual(inst.GRUB_DEFAULTS["GRUB_DISTRIBUTOR"], "LykanOS")
        inst.configure({}, {"NAME": "Labyrinth OS", "ID": "labyrinth"})
        self.assertEqual(inst.OS_NAME, "Labyrinth", "os-release's name without the ' OS'")
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


class WhatThePlanMayNotSay(unittest.TestCase):
    """Plans the window never sends, refused with a message before anything is done (they used to stop the program with a
    traceback, or pass and stop the install after the disk was changed)."""

    def test_a_value_of_the_wrong_kind_gets_that_values_message(self):
        self.assertEqual(inst.check_plan([]), "the plan is not a JSON object")
        self.assertEqual(inst.check_plan(plan(disk=5)), "no disk chosen")
        self.assertEqual(inst.check_plan(plan(user=["a"])), "the account name must start with a small letter and use only small letters, digits, - and _")
        self.assertEqual(inst.check_plan(plan(user={"name": "A", "login": 5, "password": "x"})), "the account name must start with a small letter and use only small letters, digits, - and _")
        for pw in (None, 5, ["longenough"]):
            self.assertEqual(inst.check_plan(plan(user={"name": "A", "login": "a", "password": pw})), "the account needs a password", pw)
        self.assertEqual(inst.check_plan(plan(user={"name": None, "login": "a", "password": "x"})), "the full name may not contain : or a line break")
        self.assertEqual(inst.check_plan(plan(hostname=5)), "the computer's name may use letters, digits and - (not at the start or end)")
        self.assertEqual(inst.check_plan(plan(encrypt=True, encrypt_password=list("12345678"))), "the encryption password needs at least 8 characters")
        self.assertEqual(inst.check_plan(plan(encrypt=True, encrypt_password=12345678)), "the encryption password needs at least 8 characters")
        self.assertEqual(inst.check_plan(plan(timezone=5)), "unknown time zone")
        self.assertEqual(inst.check_plan(plan(locale=None)), "unknown language setting")
        self.assertEqual(inst.check_plan(plan(keymap=["us"])), "unknown keyboard layout")
        self.assertEqual(inst.check_plan(plan(keymap_variant=5)), "unknown keyboard variant")
        self.assertIsNone(inst.check_plan(plan(keymap_variant=None)), "no variant")

    def test_a_final_line_break_is_not_part_of_a_name(self):
        self.assertIn("account name", inst.check_plan(plan(user={"name": "A", "login": "ada\n", "password": "x"})))
        self.assertIn("computer's name", inst.check_plan(plan(hostname="x\n")))
        self.assertEqual(inst.check_plan(plan(locale="en_US.UTF-8\n")), "unknown language setting")
        self.assertEqual(inst.check_plan(plan(keymap="us\n")), "unknown keyboard layout")
        self.assertEqual(inst.check_plan(plan(keymap_variant="dvorak\n")), "unknown keyboard variant")

    def test_passwords_stay_one_line(self):
        self.assertEqual(inst.check_plan(plan(user={"name": "A", "login": "a", "password": "x\nroot:y"})),
                         "the account's password may not contain a line break", "chpasswd would set root's password")
        self.assertEqual(inst.check_plan(plan(user={"name": "A", "login": "a", "password": "x\0y"})), "the account's password may not contain a line break")
        self.assertEqual(inst.check_plan(plan(encrypt=True, encrypt_password="longenough\n")), "the encryption password may not contain a line break")
        self.assertEqual(inst.check_plan(plan(user={"name": "A\0", "login": "a", "password": "x"})), "the full name may not contain : or a line break")
        self.assertIsNone(inst.check_plan(plan(user={"name": "A", "login": "a", "password": "p:a s\tw"})))

    def test_a_time_zone_is_a_name_under_zoneinfo(self):
        self.assertEqual(inst.check_plan(plan(timezone="/etc/hostname")), "unknown time zone")
        self.assertEqual(inst.check_plan(plan(timezone="/usr/share/zoneinfo/UTC")), "unknown time zone")

    def test_a_manual_plan_says_exactly_what_it_means(self):
        self.assertEqual(inst.check_plan(manual(new_table="false")), "a new partition table is chosen with true or false",
                         "text was true: every partition erased")
        self.assertEqual(inst.check_plan(manual(new_table=0)), "a new partition table is chosen with true or false")
        self.assertIsNone(inst.check_plan(manual(new_table=None)))
        self.assertEqual(inst.check_plan(manual(delete=[True])), "a partition to delete is not a number", "true was partition 1")
        self.assertEqual(inst.check_plan(manual(delete=4)), "a partition to delete is not a number")
        self.assertEqual(inst.check_plan(manual(create=[{"id": 5, "start": 300 * GB, "size": 150 * GB, "kind": "linux"}])),
                         "a new partition has no proper id")
        self.assertEqual(inst.check_plan(manual(create=[{"id": "r", "start": True, "size": 150 * GB, "kind": "linux"}])),
                         "a new partition is not described properly")
        self.assertEqual(inst.check_plan(manual(create={"id": "r"})), "a new partition is not described properly")
        self.assertEqual(inst.check_plan(manual(create=["r"])), "a new partition has no proper id")
        self.assertEqual(inst.check_plan(manual(use="x")), "choose exactly one partition for / (the system)")
        self.assertEqual(inst.check_plan(manual(use=[5])), "None cannot be chosen as a mount point")
        self.assertEqual(inst.check_plan(manual(use=[{"part": "/dev/vda1", "mount": ["/"]}])), "['/'] cannot be chosen as a mount point")
        self.assertEqual(inst.check_plan(manual(use=[{"part": "/dev/vda1", "mount": "/boot/efi", "format": False}])),
                         "/boot/efi cannot be formatted as False")

    def test_a_number_to_delete_twice_is_deleted_once(self):
        calls = []
        old = (inst.run, inst.settle)

        def run(argv, stdin=None, check=True, capture=False):
            calls.append(argv)
            return '{"partitiontable": {"sectorsize": 512, "partitions": []}}' if capture else None
        inst.run, inst.settle = run, lambda: None
        try:
            inst.manual_partition(manual(delete=[2, 3, 2], create=[], use=[]))
        finally:
            inst.run, inst.settle = old
        self.assertEqual([c for c in calls if "--delete" in c], [["sfdisk", "--delete", "/dev/vda", "3"], ["sfdisk", "--delete", "/dev/vda", "2"]])

    def test_the_lists_keep_entries_with_a_text_id(self):
        self.assertEqual(inst.browser_catalog(written([{"id": "x"}])), [], "not an object at the top")
        self.assertEqual(inst.browser_catalog(written({"browsers": {"id": "x"}})), [])
        self.assertEqual([b["id"] for b in inst.browser_catalog(written({"browsers": [{"id": 5}, {"id": ["x"]}, {"id": "ok"}]}))], ["ok"])
        cat = inst.browser_catalog(written({"browsers": [{"id": "a", "packages": "abc", "flatpak": 5}, {"id": "b", "packages": ["b", 7]}]}))
        self.assertEqual(inst.browser_plan(cat, ["a"]), {"remove": ["b"], "install": [], "flatpaks": []})
        comps = inst.components(written({"components": [{"id": "c", "packages": ["p"], "aur": "q", "pkgbuild": 5, "name": None}]}))
        self.assertEqual(inst.component_packages(comps["c"]), ["p"])
        self.assertEqual(inst.component_name(comps, "c"), "c")

    def test_nesting(self):
        self.assertEqual(inst.nesting(5), 0)
        self.assertEqual(inst.nesting([[[]]]), 2)
        self.assertEqual(inst.nesting({"a": [{"b": 1}]}), 3)

    def test_what_is_not_a_plan(self):
        import subprocess
        for data in (b'{"user": {"name": "\xff"}}', b'{"user": {"name": "\\ud800"}}', b"[" * 22 + b"]" * 22, b"[" * 200000, b"1" * 4301,
                     b"\xef\xbb\xbf{}"):
            r = subprocess.run([TOOL, "--check"], input=data, capture_output=True)
            self.assertEqual((r.returncode, r.stdout, r.stderr), (2, b"ERROR the plan is not JSON\n", b""), data[:40])
        r = subprocess.run([TOOL, "--check"], input=b"[" * 21 + b"]" * 21, capture_output=True)
        self.assertEqual((r.returncode, r.stdout), (2, b"ERROR the plan is not a JSON object\n"), "20 levels inside are JSON")


class TheLiveSystemsMounts(unittest.TestCase):
    """The live system's /dev and /run/udev are bound one way only, in one call, and umount -R of the new system can never
    reach the live system's own mounts through them: after such a bind, every mount under the new system is made a slave
    first, and if that fails nothing is unmounted."""

    def setUp(self):
        self.calls, self.failing = [], []
        self.old = (inst.run, inst.TARGET, inst.BOUND)
        inst.TARGET, inst.BOUND = tempfile.mkdtemp(), False

        class R:
            def __init__(self, code):
                self.returncode, self.stdout, self.stderr = code, "", "failed"

        def run(argv, stdin=None, check=True, capture=False):
            self.calls.append(argv)
            code = 1 if any(argv[:len(f)] == f for f in self.failing) else 0
            if code and check:
                raise inst.Stop(f"{argv[0]} failed")
            return R(code)
        inst.run = run

    def tearDown(self):
        inst.run, inst.TARGET, inst.BOUND = self.old

    def test_bound_one_way_in_the_same_call(self):
        inst.bind_mounts()
        binds = [c for c in self.calls if "--rbind" in c or "--bind" in c]
        self.assertEqual(binds, [["mount", "--rbind", "--make-rslave", "/dev", inst.TARGET + "/dev"],
                                 ["mount", "--bind", "--make-rslave", "/run/udev", inst.TARGET + "/run/udev"]])
        self.assertFalse([c for c in self.calls if c[:2] == ["mount", "--make-rslave"]], "no separate call that could fail afterwards")
        self.assertTrue(inst.BOUND)

    def test_made_a_slave_before_umount_r(self):
        inst.BOUND = True
        self.assertTrue(inst.unmount_all())
        self.assertEqual(self.calls, [["mount", "--make-rslave", inst.TARGET], ["umount", "-R", inst.TARGET], ["cryptsetup", "close", inst.MAPPER]])

    def test_if_that_fails_nothing_is_unmounted(self):
        inst.BOUND, self.failing = True, [["mount", "--make-rslave"]]
        self.assertFalse(inst.unmount_all())
        self.assertEqual(self.calls, [["mount", "--make-rslave", inst.TARGET]])

    def test_a_failed_bind_still_counts(self):
        self.failing = [["mount", "--rbind"], ["mount", "--make-rslave"]]
        with self.assertRaises(inst.Stop):
            inst.bind_mounts()
        self.assertTrue(inst.BOUND, "the bind may have happened before the call failed")
        self.assertFalse(inst.unmount_all())
        self.assertFalse([c for c in self.calls if c[:2] == ["umount", "-R"]])

    def test_before_any_bind_as_before(self):
        self.assertTrue(inst.unmount_all())
        self.assertEqual(self.calls[0], ["umount", "-R", inst.TARGET])


class RemovePackages(unittest.TestCase):
    """A fake pacman: z stays and needs b; d (on the list) needs c."""

    def setUp(self):
        self.installed = {"a", "b", "c", "d", "z"}
        needs = {"z": {"b"}, "d": {"c"}}

        class R:
            def __init__(self, code):
                self.returncode, self.stdout, self.stderr = code, "", ""

        def inside(argv, stdin=None, check=True):
            if argv[:2] == ["pacman", "-Q"]:
                return R(0 if argv[2] in self.installed else 1)
            targets = [x for x in argv[1:] if not x.startswith("-")]
            staying = self.installed - set(targets)
            if any(t not in self.installed or any(t in needs.get(s, ()) for s in staying) for t in targets):
                return R(1)
            self.installed -= set(targets)
            return R(0)

        self.old, inst.inside = inst.inside, inside

    def tearDown(self):
        inst.inside = self.old

    def test_what_another_program_needs_is_kept_and_the_rest_goes(self):
        self.assertEqual(inst.remove_packages(["a", "b", "c", "d"]), ["b"])
        self.assertEqual(self.installed, {"b", "z"})

    def test_all_at_once_when_nothing_else_needs_them(self):
        self.assertEqual(inst.remove_packages(["c", "d", "a"]), [])
        self.assertEqual(self.installed, {"b", "z"})
        self.assertEqual(inst.remove_packages([]), [])



class SavedNetworks(unittest.TestCase):
    """The live session's saved networks carried over to the new system (scratch folders for both: never the real /etc)."""
    HOME = b"[connection]\nid=Home Net\ntype=wifi\npermissions=user:live:;\n\n[wifi-security]\nkey-mgmt=wpa-psk\npsk=pw-WIFI-3\n"

    def setUp(self):
        self.old = (inst.TARGET, inst.LIVE_NM_CONNECTIONS)
        top = tempfile.mkdtemp()
        self.top = top
        inst.TARGET = os.path.join(top, "target")
        inst.LIVE_NM_CONNECTIONS = os.path.join(top, "live/system-connections")
        os.makedirs(inst.TARGET)

    def tearDown(self):
        inst.TARGET, inst.LIVE_NM_CONNECTIONS = self.old
        shutil.rmtree(self.top, ignore_errors=True)

    def carry(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            inst.carry_networks("ada")
        return out.getvalue().splitlines()

    def live(self, name, data):
        os.makedirs(inst.LIVE_NM_CONNECTIONS, exist_ok=True)
        with open(os.path.join(inst.LIVE_NM_CONNECTIONS, name), "wb") as f:
            f.write(data)

    def test_plain_files_go_over_root_only_and_the_live_account_becomes_the_new_one(self):
        self.live("Home Net.nmconnection", self.HOME)
        self.live("Cafe.nmconnection", b"[connection]\nid=Cafe\n")
        self.live("notes.txt", b"not a profile\n")
        self.live("../Elsewhere.nmconnection", b"[connection]\nid=Elsewhere\n")
        os.symlink(os.path.join(self.top, "live/Elsewhere.nmconnection"), os.path.join(inst.LIVE_NM_CONNECTIONS, "Link.nmconnection"))
        os.mkdir(os.path.join(inst.LIVE_NM_CONNECTIONS, "Folder.nmconnection"))
        os.mkfifo(os.path.join(inst.LIVE_NM_CONNECTIONS, "Pipe.nmconnection"))
        said = self.carry()
        self.assertEqual(said, ["WARN left behind 3 of the live session's saved networks that are not plain files",
                                "INFO the live session's saved networks carried over: 2"])
        d = inst.TARGET + "/etc/NetworkManager/system-connections"
        self.assertEqual(stat.S_IMODE(os.lstat(d).st_mode), 0o700)
        self.assertEqual(sorted(os.listdir(d)), ["Cafe.nmconnection", "Home Net.nmconnection"])
        for n in os.listdir(d):
            self.assertEqual(stat.S_IMODE(os.lstat(os.path.join(d, n)).st_mode), 0o600, n)
        self.assertEqual(open(os.path.join(d, "Home Net.nmconnection"), "rb").read(), self.HOME.replace(b"user:live:", b"user:ada:"))
        self.assertEqual(open(os.path.join(d, "Cafe.nmconnection"), "rb").read(), b"[connection]\nid=Cafe\n")

    def test_nothing_saved_nothing_to_do(self):
        self.assertEqual(self.carry(), [], "no folder")
        os.makedirs(inst.LIVE_NM_CONNECTIONS)
        self.assertEqual(self.carry(), [], "an empty folder")
        self.assertFalse(os.path.exists(inst.TARGET + "/etc/NetworkManager"))
        os.rmdir(inst.LIVE_NM_CONNECTIONS)
        os.makedirs(os.path.join(self.top, "elsewhere"))
        open(os.path.join(self.top, "elsewhere/Home.nmconnection"), "w").write("[connection]\n")
        os.symlink(os.path.join(self.top, "elsewhere"), inst.LIVE_NM_CONNECTIONS)
        self.assertEqual(self.carry(), [], "a link in the folder's place is not followed")
        self.assertFalse(os.path.exists(inst.TARGET + "/etc/NetworkManager"))

    def test_a_link_in_the_new_system_is_not_written_through(self):
        self.live("Home.nmconnection", self.HOME)
        os.makedirs(inst.TARGET + "/etc/NetworkManager")
        os.makedirs(os.path.join(self.top, "victim"))
        os.symlink(os.path.join(self.top, "victim"), inst.TARGET + "/etc/NetworkManager/system-connections")
        self.assertEqual(self.carry(), ["WARN the new system's folder for saved networks is a link: none were carried over"])
        self.assertEqual(os.listdir(os.path.join(self.top, "victim")), [])

    def test_a_link_where_a_file_goes_is_replaced_not_followed(self):
        self.live("Home.nmconnection", self.HOME)
        d = inst.TARGET + "/etc/NetworkManager/system-connections"
        os.makedirs(d)
        victim = os.path.join(self.top, "victim")
        open(victim, "w").write("keep\n")
        os.symlink(victim, os.path.join(d, "Home.nmconnection"))
        self.assertEqual(self.carry(), ["INFO the live session's saved networks carried over: 1"])
        self.assertEqual(open(victim).read(), "keep\n")
        self.assertFalse(os.path.islink(os.path.join(d, "Home.nmconnection")))

    def test_only_the_live_accounts_permission_changes_hands(self):
        f = lambda t: inst.network_for(t.encode(), "ada").decode()  # noqa: E731
        self.assertEqual(f("[connection]\npermissions=user:live:;\n"), "[connection]\npermissions=user:ada:;\n")
        self.assertEqual(f("[connection]\npermissions=user:live;\n"), "[connection]\npermissions=user:ada;\n")
        self.assertEqual(f("[connection]\npermissions=user:bob:;user:live:;\n"), "[connection]\npermissions=user:bob:;user:ada:;\n")
        self.assertEqual(f("[connection]\r\npermissions = user:live\r\n"), "[connection]\r\npermissions = user:ada\r\n")
        for same in ["[connection]\npermissions=user:bob:;\n", "[connection]\npermissions=user:livewire:;\n",
                     "[connection]\npermissions=group:live:;\n", "[wifi]\npermissions=user:live:;\n", "[connection]\nid=user:live\nuuid=x\n",
                     "permissions=user:live:;\n", "", "[connection]\n\n[vpn]\npermissions=user:live:;"]:
            self.assertEqual(f(same), same)
        self.assertEqual(inst.network_for(b"[connection]\nid=\xff\xfe\npermissions=user:live:;\n", "ada"),
                         b"[connection]\nid=\xff\xfe\npermissions=user:ada:;\n")

if __name__ == "__main__":
    unittest.main()
