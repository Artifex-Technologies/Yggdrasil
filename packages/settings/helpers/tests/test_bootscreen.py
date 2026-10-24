"""bootscreen's pure parts: GRUB's kernel options, the OS's own animation, the mkinitcpio drop-in. Nothing on the system is touched."""
import importlib.machinery
import importlib.util
import os
import subprocess
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "bootscreen")


def load():
    loader = importlib.machinery.SourceFileLoader("bootscreen", HELPER)
    spec = importlib.util.spec_from_loader("bootscreen", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


class BootScreen(unittest.TestCase):
    def setUp(self):
        self.m = load()

    def test_splash_is_added_once_and_quotes_kept(self):
        g = "GRUB_TIMEOUT=5\nGRUB_CMDLINE_LINUX_DEFAULT='quiet loglevel=3'\nGRUB_CMDLINE_LINUX=\"\"\n"
        once = self.m.grub_splash(g)
        self.assertIn("GRUB_CMDLINE_LINUX_DEFAULT='quiet loglevel=3 splash'", once)
        self.assertIn('GRUB_CMDLINE_LINUX=""', once, "the other line stays")
        self.assertEqual(self.m.grub_splash(once), once, "not twice")
        self.assertEqual(self.m.with_splash('GRUB_CMDLINE_LINUX_DEFAULT="quiet"'), 'GRUB_CMDLINE_LINUX_DEFAULT="quiet splash"')

    def test_the_os_names_its_own_animation(self):
        d = tempfile.mkdtemp()
        themes, ints = os.path.join(d, "themes"), os.path.join(d, "int")
        os.makedirs(os.path.join(themes, "labyrinth-rain"))
        os.makedirs(ints)
        self.m.THEMES, self.m.INTEGRATIONS = themes, ints
        self.assertEqual(self.m.os_theme(), "spinner", "no branding: Plymouth's own")
        with open(os.path.join(ints, "x-branding.conf"), "w") as f:
            f.write("kind = branding\nboot_theme = labyrinth-rain\n")
        self.assertEqual(self.m.os_theme(), "labyrinth-rain")
        with open(os.path.join(ints, "x-branding.conf"), "w") as f:
            f.write("kind = branding\nboot_theme = not-installed\n")
        self.assertEqual(self.m.os_theme(), "spinner", "a theme that is not there")

    def test_choices(self):
        self.assertEqual(self.m.choice_of("details"), "messages")
        self.assertEqual(self.m.choice_of("yggdrasil-picture"), "picture")
        self.assertEqual(self.m.choice_of("labyrinth-rain"), "animation")

    def test_the_nvidia_dropin_adds_its_modules_once(self):
        for mods, want in [("", "nvidia nvidia_modeset nvidia_uvm nvidia_drm"), ("btrfs nvidia", "btrfs nvidia nvidia_modeset nvidia_uvm nvidia_drm")]:
            script = f"MODULES=({mods})\n{self.m.NVIDIA_DROPIN_TEXT}\n{self.m.NVIDIA_DROPIN_TEXT}\necho \"${{MODULES[*]}}\"\n"
            out = subprocess.run(["bash", "-c", script], capture_output=True, text=True).stdout.strip()
            self.assertEqual(out, want)

    def test_the_dropin_puts_plymouth_after_kms(self):
        for hooks, want in [("base udev autodetect microcode kms modconf block filesystems", "base udev autodetect microcode kms plymouth modconf block filesystems"),
                            ("base udev autodetect block filesystems", "base udev plymouth autodetect block filesystems"),
                            ("base udev plymouth kms block", "base udev plymouth kms block")]:
            script = f"HOOKS=({hooks})\n{self.m.DROPIN_TEXT}\necho \"${{HOOKS[*]}}\"\n"
            out = subprocess.run(["bash", "-c", script], capture_output=True, text=True).stdout.strip()
            self.assertEqual(out, want)


if __name__ == "__main__":
    unittest.main()
