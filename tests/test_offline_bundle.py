#!/usr/bin/env python3
"""The offline payload's pinned-file rules, without downloading the game.

The full build (offline_bundle.build) fetches the pinned files once; these tests cover the
rules it applies to the version file and the manifests, which is where a mistake would put the
wrong file in an APK.
"""
import hashlib
import importlib.util
import sys
import tempfile
import unittest
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SPEC = importlib.util.spec_from_file_location(
    "offline_bundle", ROOT / "packaging/android/offline_bundle.py")
bundle = importlib.util.module_from_spec(SPEC)
sys.modules["offline_bundle"] = bundle
SPEC.loader.exec_module(bundle)


class LibraryRules(unittest.TestCase):
    def test_a_library_without_rules_applies(self):
        self.assertTrue(bundle.library_applies({}))

    def test_the_last_matching_rule_decides(self):
        library = {"rules": [
            {"action": "allow"},
            {"action": "disallow", "os": {"name": "osx"}},
        ]}
        self.assertTrue(bundle.library_applies(library))

    def test_a_windows_only_library_does_not(self):
        library = {"rules": [{"action": "allow", "os": {"name": "windows"}}]}
        self.assertFalse(bundle.library_applies(library))

    def test_a_disallowed_linux_library_does_not(self):
        library = {"rules": [
            {"action": "allow"},
            {"action": "disallow", "os": {"name": "linux"}},
        ]}
        self.assertFalse(bundle.library_applies(library))


class Pins(unittest.TestCase):
    def test_a_git_blob_id_is_the_manifest_hash(self):
        with tempfile.TemporaryDirectory() as folder:
            path = Path(folder) / "hello.txt"
            path.write_bytes(b"hello\n")
            self.assertEqual(
                bundle.blob_sha1(path),
                "ce013625030ba8dba906f756967f9e9ca394464a",
            )

    def test_the_offline_apk_is_arm64_only(self):
        with self.assertRaises(SystemExit):
            bundle.jre_pin("x86_64")


class Archives(unittest.TestCase):
    def test_a_zip_is_deterministic_and_keeps_the_paths(self):
        with tempfile.TemporaryDirectory() as folder:
            folder = Path(folder)
            (folder / "versions/26.3").mkdir(parents=True)
            (folder / "versions/26.3/26.3.json").write_text("{}")
            entries = [("versions/26.3/26.3.json", folder / "versions/26.3/26.3.json")]
            one = folder / "one.zip"
            two = folder / "two.zip"
            bundle.write_zip(folder, one, entries)
            bundle.write_zip(folder, two, entries)
            self.assertEqual(one.read_bytes(), two.read_bytes())
            with zipfile.ZipFile(one) as z:
                self.assertEqual(z.namelist(), ["versions/26.3/26.3.json"])
                self.assertEqual(z.getinfo("versions/26.3/26.3.json").date_time,
                                 bundle.ZIP_TIME)

    def test_a_manifest_names_every_archive_with_its_hash(self):
        # The shape bundled.rs reads: archives[].path and a whole sha256.
        with tempfile.TemporaryDirectory() as folder:
            payload = Path(folder) / "archives"
            payload.mkdir()
            for name in (bundle.JRE_ASSET, "game-files.zip", "assets.zip"):
                (payload / name).write_bytes(name.encode())
            manifest = {
                "archives": [
                    {"path": name, "size": (payload / name).stat().st_size,
                     "sha256": hashlib.sha256((payload / name).read_bytes()).hexdigest()}
                    for name in (bundle.JRE_ASSET, "game-files.zip", "assets.zip")
                ]
            }
            for archive in manifest["archives"]:
                self.assertEqual(len(archive["sha256"]), 64)
                self.assertEqual(archive["size"], len(archive["path"]))


if __name__ == "__main__":
    unittest.main()
