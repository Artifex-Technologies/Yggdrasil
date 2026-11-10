"""yggdrasil-gpu-run against a pretend /sys: one card, and an NVIDIA card next to an Intel Arc (the RTX 2070 + B580 a friend of
the owner runs, the setup Lossless Scaling's dual-GPU mode is for)."""

import importlib.machinery
import importlib.util
import os
import shutil
import tempfile
import unittest

HELPER = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "gpu-run")


def load():
    loader = importlib.machinery.SourceFileLoader("gpu_run", HELPER)
    spec = importlib.util.spec_from_loader("gpu_run", loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


m = load()

VULKANINFO = """
Devices:
========
GPU0:
	apiVersion         = 1.4.312
	driverVersion      = 580.95.5.0
	vendorID           = 0x10de
	deviceID           = 0x1f07
	deviceType         = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU
	deviceName         = NVIDIA GeForce RTX 2070
GPU1:
	apiVersion         = 1.4.318
	vendorID           = 0x8086
	deviceID           = 0xe20b
	deviceType         = PHYSICAL_DEVICE_TYPE_DISCRETE_GPU
	deviceName         = Intel(R) Arc(tm) B580 Graphics (BMG G21)
"""


class Sys:
    """A pretend /sys with the given cards: (cardN, slot, vendor, device, driver, boot, connected outputs)."""

    def __init__(self, cards):
        self.root = tempfile.mkdtemp(prefix="gpu-run-")
        drm = os.path.join(self.root, "class", "drm")
        os.makedirs(drm)
        for card, slot, vendor, device, driver, boot, outputs in cards:
            dev = os.path.join(self.root, "devices", "pci0000:00", slot)
            os.makedirs(dev)
            for k, v in (("vendor", vendor), ("device", device), ("boot_vga", "1" if boot else "0")):
                with open(os.path.join(dev, k), "w") as f:
                    f.write(v + "\n")
            drv = os.path.join(self.root, "bus", "pci", "drivers", driver)
            os.makedirs(drv, exist_ok=True)
            os.symlink(drv, os.path.join(dev, "driver"))
            os.makedirs(os.path.join(drm, card))
            os.symlink(dev, os.path.join(drm, card, "device"))
            for i, connected in enumerate(outputs):
                o = os.path.join(drm, f"{card}-DP-{i + 1}")
                os.makedirs(o)
                with open(os.path.join(o, "status"), "w") as f:
                    f.write("connected\n" if connected else "disconnected\n")

    def __enter__(self):
        self.old = m.SYSFS
        m.SYSFS = self.root
        return self

    def __exit__(self, *a):
        m.SYSFS = self.old
        shutil.rmtree(self.root)


class GpuRun(unittest.TestCase):
    def setUp(self):
        self.old_name = m.pci_name
        m.pci_name = lambda slot: {"0000:01:00.0": "GeForce RTX 2070", "0000:03:00.0": "Arc B580"}.get(slot, "")

    def tearDown(self):
        m.pci_name = self.old_name

    def two(self):
        return Sys([
            ("card1", "0000:01:00.0", "0x10de", "0x1f07", "nvidia", True, [True]),
            ("card2", "0000:03:00.0", "0x8086", "0xe20b", "xe", False, [False, False]),
        ])

    def test_vulkaninfo_is_read(self):
        self.assertEqual(m.vulkan_devices(VULKANINFO), [
            (0x10DE, 0x1F07, "NVIDIA GeForce RTX 2070"),
            (0x8086, 0xE20B, "Intel(R) Arc(tm) B580 Graphics (BMG G21)"),
        ])

    def test_cards_are_numbered_in_pci_order_with_their_screens(self):
        with self.two():
            cs = m.cards(m.vulkan_devices(VULKANINFO))
        self.assertEqual([(c["number"], c["name"], c["driver"], c["boot"], c["screens"]) for c in cs],
                         [(1, "GeForce RTX 2070", "nvidia", True, 1), (2, "Arc B580", "xe", False, 0)])
        self.assertEqual(cs[1]["vulkan_name"], "Intel(R) Arc(tm) B580 Graphics (BMG G21)")

    def test_one_card_needs_nothing(self):
        with Sys([("card0", "0000:01:00.0", "0x10de", "0x1e07", "nvidia", True, [True])]):
            cs = m.cards([])
        self.assertEqual(m.environment(cs[0], cs), {})

    def test_the_arc_is_picked_and_the_nvidia_card_hidden(self):
        with self.two():
            cs = m.cards(m.vulkan_devices(VULKANINFO))
        env = m.environment(cs[1], cs)
        self.assertEqual(env["DRI_PRIME"], "pci-0000_03_00_0")
        self.assertEqual(env["MESA_VK_DEVICE_SELECT"], "8086:e20b")
        self.assertEqual(env["__VK_LAYER_NV_optimus"], "non_NVIDIA_only")
        self.assertEqual(env["DXVK_FILTER_DEVICE_NAME"], "Intel(R) Arc(tm) B580 Graphics (BMG G21)")
        self.assertEqual(env["VKD3D_FILTER_DEVICE_NAME"], env["DXVK_FILTER_DEVICE_NAME"])
        self.assertNotIn("__NV_PRIME_RENDER_OFFLOAD", env)

    def test_the_nvidia_card_is_picked_by_prime_offload(self):
        with self.two():
            cs = m.cards(m.vulkan_devices(VULKANINFO))
        env = m.environment(cs[0], cs)
        self.assertEqual((env["__NV_PRIME_RENDER_OFFLOAD"], env["__GLX_VENDOR_LIBRARY_NAME"], env["__VK_LAYER_NV_optimus"]), ("1", "nvidia", "NVIDIA_only"))
        self.assertEqual(env["DXVK_FILTER_DEVICE_NAME"], "NVIDIA GeForce RTX 2070")
        self.assertNotIn("DRI_PRIME", env)

    def test_without_vulkaninfo_the_name_filters_are_left_out(self):
        with self.two():
            cs = m.cards([])
        env = m.environment(cs[1], cs)
        self.assertNotIn("DXVK_FILTER_DEVICE_NAME", env)
        self.assertEqual(env["DRI_PRIME"], "pci-0000_03_00_0")

    def test_frames_on_the_arc_while_the_2070_draws(self):
        with self.two():
            cs = m.cards(m.vulkan_devices(VULKANINFO))
        argv = m.frames_command(cs[0], cs[1], cs, ["%command%", "-arg"], 3)
        g = argv.index("gamescope")
        outer, gs, inner = argv[:g], argv[g:argv.index("--") + 1], argv[argv.index("--") + 1:]
        self.assertIn("LSFGVK_ENV=1", outer)
        self.assertIn("LSFGVK_MULTIPLIER=3", outer)
        self.assertIn("DRI_PRIME=pci-0000_03_00_0", outer, "gamescope composites on the Arc")
        self.assertEqual(gs[gs.index("--prefer-vk-device") + 1], "8086:e20b")
        self.assertEqual(gs[1:3], ["--backend", "sdl"], "a Vulkan swapchain lsfg-vk can work on")
        self.assertEqual(inner[-2:], ["%command%", "-arg"])
        self.assertIn("DISABLE_LSFGVK=1", inner, "no frame generation inside the game itself")
        for k in ("LSFGVK_ENV", "LSFGVK_MULTIPLIER", "DRI_PRIME", "MESA_VK_DEVICE_SELECT"):
            self.assertIn(k, inner[inner.index("-u") - 1:], f"{k} is taken away from the game")
        self.assertIn("__NV_PRIME_RENDER_OFFLOAD=1", inner, "the game draws on the 2070")
        self.assertIn("__VK_LAYER_NV_optimus=NVIDIA_only", inner, "the game's value replaces gamescope's")

    def test_a_card_number_that_does_not_exist_is_refused(self):
        with self.two():
            cs = m.cards([])
        self.assertIsNone(m.pick("3", cs))
        self.assertIsNone(m.pick("x", cs))
        self.assertEqual(m.pick("2", cs)["slot"], "0000:03:00.0")


if __name__ == "__main__":
    unittest.main()
