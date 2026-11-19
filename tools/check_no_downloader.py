#!/usr/bin/env python3
"""The offline APK has no game-data downloader: this checks the sources and the build.

A phone build carries every file it needs (packaging/android/offline_bundle.py) and unpacks it
on first run (crates/horizon-client/src/bundled.rs).  That means:

  * no game-data host reaches an Android compilation: the downloader in
    starling-account/src/files.rs and the runtime fetch in java.rs are
    `#[cfg(not(target_os = "android"))]`, and the Java runtime's addresses are empty on
    Android;
  * the setup's game-file step calls the APK's payload, never
    `starling_account::files::download`;
  * the exporter bundle (crates/horizon-prepare/src/exporters.rs) has no network code;
  * the built `libhorizon_android.so` names the payload (`bundled/manifest.json`,
    `game-files.zip`, `assets.zip`, `jre25-android-arm64.tar.xz`) and none of the
    game-data hosts or the Java runtime's publishers;
  * an offline APK holds one ABI and `assets/bundled/manifest.json`.

    python3 tools/check_no_downloader.py                       # the sources, and target/ if built
    python3 tools/check_no_downloader.py --so libhorizon_android.so
    python3 tools/check_no_downloader.py --apk minecraft-arm64-0.9.0.apk

Exit status 0 means the build is offline; 1 names every check that failed.
"""
import argparse
import re
import sys
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]

# What the game files are fetched from, and where the desktop gets its Java runtime.
FORBIDDEN = [
    b"piston-meta.mojang.com",
    b"piston-data.mojang.com",
    b"resources.download.minecraft.net",
    b"minecraft-assets",  # the download base's repository name, only in the downloader
    b"angelauramc-openjdk-build",
    b"EclipseJRE",
]
# What the offline path is, and must stay, in the library.
REQUIRED = [
    b"bundled/manifest.json",
    b"game-files.zip",
    b"assets.zip",
    b"jre25-android-arm64.tar.xz",
]
CFG_OFF_ANDROID = '#[cfg(not(target_os = "android"))]'


class Checker:
    def __init__(self):
        self.failures = []
        self.passes = []

    def ok(self, message):
        self.passes.append(message)

    def fail(self, message):
        self.failures.append(message)

    def require(self, condition, message):
        (self.ok if condition else self.fail)(message)

    def report(self):
        for message in self.passes:
            print(f"pass  {message}")
        for message in self.failures:
            print(f"FAIL  {message}")
        print()
        if self.failures:
            print(f"{len(self.failures)} check(s) failed")
            return 1
        print(f"{len(self.passes)} check(s) passed: the upload fetches no game data")
        return 0


def read(path):
    return (ROOT / path).read_text(encoding="utf-8")


def hosts_are_android_excluded(check, path, hosts):
    """Every line naming one of `hosts` sits in a `#[cfg(not(target_os = "android"))]` item
    (the attribute is within the ten lines above it, which covers a doc comment)."""
    lines = read(path).splitlines()
    for number, line in enumerate(lines, 1):
        if not any(host in line for host in hosts):
            continue
        window = lines[max(0, number - 21):number - 1]
        check.require(
            any(CFG_OFF_ANDROID in above for above in window),
            f"{path}:{number} names "
            f"{next(h for h in hosts if h in line)!r} only outside an Android build",
        )


def check_sources(check):
    # 1. No game-data host survives an Android compile.
    hosts = ["piston-meta.mojang.com", "piston-data.mojang.com",
             "resources.download.minecraft.net"]
    for path in ("crates/starling-account/src/files.rs",
                 "crates/starling-account/src/java.rs"):
        hosts_are_android_excluded(check, path, hosts)
    java = read("crates/starling-account/src/java.rs")
    check.require(
        java.count('#[cfg(target_os = "android")]\nconst JRE25_') == 3,
        "java.rs: every Java runtime address is empty on Android",
    )
    check.require(
        '#[cfg(target_os = "android")]\npub fn install(' in java,
        "java.rs: a phone install() never fetches the runtime",
    )

    # 2. The setup's game-file step reads the APK's payload on Android.
    accounts = read("crates/horizon-client/src/accounts.rs")
    check.require(
        "crate::bundled::extract_game_files(&root" in accounts,
        "accounts.rs: the setup unpacks the APK's game files",
    )
    android_branch = accounts.split('#[cfg(target_os = "android")]', 1)[-1]
    check.require(
        "starling_account::files::download(" not in android_branch.split(
            '#[cfg(not(target_os = "android"))]', 1)[0],
        "accounts.rs: the Android branch never calls the downloader",
    )
    check.require(
        '#[cfg(not(target_os = "android"))]\n            let v = {\n                let pins = Pins::starling();\n                let wanted = Wanted'
        in accounts,
        "accounts.rs: the downloader is behind #[cfg(not(target_os = \"android\"))]",
    )

    # 3. The exporter bundle has no network code.
    exporters = read("crates/horizon-prepare/src/exporters.rs")
    for name in ("ureq", "https://", "TcpStream", "reqwest"):
        check.require(name not in exporters,
                      f"exporters.rs: no {name!r} (the bundle is built, never fetched)")

    # 4. The build script carries and asserts the payload.
    build = read("packaging/android/build_apk.py")
    for needle, message in (
        ('"--offline"', "build_apk.py: --offline exists"),
        ("offline_bundle.build(", "build_apk.py: --offline packs the payload"),
        ("assets/bundled/manifest.json", "build_apk.py: the APK's manifest is asserted"),
        ("check_offline_apk", "build_apk.py: the offline APK is checked before signing"),
    ):
        check.require(needle in build, message)
    check.require(
        "if args.offline and args.abi != ABI" in build,
        "build_apk.py: the offline payload is arm64-v8a only",
    )


def check_library(check, source, label):
    # A path on disk, or the bytes already read out of the APK.
    data = source if isinstance(source, bytes) else Path(source).read_bytes()
    for host in FORBIDDEN:
        check.require(host not in data, f"{label}: no {host.decode()!r} in the library")
    for name in REQUIRED:
        check.require(name in data, f"{label}: names {name.decode()!r}")


def check_apk(check, path):
    with zipfile.ZipFile(path) as apk:
        names = apk.namelist()
        libs = [name for name in names if name.startswith("lib/")]
        abis = sorted({name.split("/")[1] for name in libs})
        check.require(abis == ["arm64-v8a"],
                      f"{Path(path).name}: one ABI, arm64-v8a (found {abis})")
        check.require("assets/bundled/manifest.json" in names,
                      f"{Path(path).name}: assets/bundled/manifest.json is present")
        check.require(any(name.startswith("assets/bundled/game-files") for name in names),
                      f"{Path(path).name}: the game files are inside")
        check.require(any(name.startswith("assets/bundled/jre25") for name in names),
                      f"{Path(path).name}: the Java runtime is inside")
        check.require(any(name.startswith("assets/bundled/assets.zip") for name in names),
                      f"{Path(path).name}: Bedrock's controls are inside")
        for name in libs:
            if name.endswith("libhorizon_android.so"):
                check_library(check, bytes(apk.read(name)), f"{Path(path).name}:{name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--so", help="a built libhorizon_android.so to scan")
    parser.add_argument("--apk", help="an APK to scan")
    parser.add_argument("--sources-only", action="store_true",
                        help="skip target/android when it exists")
    args = parser.parse_args()
    check = Checker()
    check_sources(check)
    if args.so:
        check_library(check, args.so, Path(args.so).name)
    if args.apk:
        check_apk(check, args.apk)
    if not args.sources_only and not (args.so or args.apk):
        built = sorted((ROOT / "target/android").glob("libhorizon_android.so"))
        apks = sorted((ROOT / "target/android").glob("minecraft-*.apk"))
        for library in built:
            check_library(check, library, library.name)
        for apk in apks:
            check_apk(check, apk)
    return check.report()


if __name__ == "__main__":
    sys.exit(main())
