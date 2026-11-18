"""The launcher icon: every density is a PNG of the size build_apk.py advertises."""
import importlib.util
import struct
from pathlib import Path
import unittest

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("starling_launcher_icon", ROOT / "packaging/android/build_apk.py")
apk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(apk)
ICON_DIR = ROOT / "packaging/android/icon"
SIGNATURE = b"\x89PNG\r\n\x1a\n"


class LauncherIcon(unittest.TestCase):
    def test_every_density_has_a_png_at_its_size(self):
        for folder, size in apk.ICONS.items():
            with self.subTest(folder=folder):
                png = (ICON_DIR / folder / "ic_launcher.png").read_bytes()
                self.assertEqual(png[:8], SIGNATURE)
                self.assertEqual(struct.unpack(">II", png[16:24]), (size, size))

    def test_a_square_master_can_be_rendered_again(self):
        # The densities are generated from this file with render.py (icon/README.md).
        master = (ICON_DIR / "ic_launcher.svg").read_text(encoding="utf-8")
        self.assertIn('viewBox="0 0 16 16"', master)

    def test_the_pngs_are_what_the_renderer_draws(self):
        spec = importlib.util.spec_from_file_location("starling_icon_render", ICON_DIR / "render.py")
        render = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(render)
        for folder, size in apk.ICONS.items():
            with self.subTest(folder=folder):
                self.assertEqual((ICON_DIR / folder / "ic_launcher.png").read_bytes(), render.png(size))

    def test_the_application_id_is_the_one_the_issue_asked_for(self):
        self.assertEqual(apk.APPLICATION_ID, "rust.minecraft")
        self.assertEqual(apk.LABEL, "Minecraft")
        # One app: no variant suffixes, and the word "preview" is gone.
        self.assertFalse(hasattr(apk, "VARIANTS"))
        self.assertIn("rust.minecraft", str(apk.APPLICATION_ID))
        self.assertNotIn("preview", apk.APPLICATION_ID.lower())


if __name__ == "__main__":
    unittest.main()
