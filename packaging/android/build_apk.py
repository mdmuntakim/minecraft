#!/usr/bin/env python3
"""Build Starling's Android APK (Horizon on Android): horizon-android as a
NativeActivity library.

The APK holds the game library, the launcher icon, Starling Visuals and what prepares the game
files on the phone (the Java launchers in native/ and original exporter classes/sources), nothing of
Minecraft: the resources
horizon-prepare makes from the player's own copy are made on the phone, or copied there.

    python3 packaging/android/build_apk.py                 # build the library, then the APK
    python3 packaging/android/build_apk.py --no-build      # package the library built last
    python3 packaging/android/build_apk.py --install       # and adb install it
    python3 packaging/android/build_apk.py --zip           # and a zip of it to send
    python3 packaging/android/build_apk.py --offline       # carry the game files and the Java runtime

`--offline` packs the pinned game files, the Java runtime that prepares them and Bedrock's
control textures into `assets/bundled/` (offline_bundle.py), so a fresh install downloads
nothing.  It is arm64-v8a only, and every game-data host is out of the Android library
(tools/check_no_downloader.py).  The payload comes from Mojang's published files at build time,
checked against the SHA-1 pins in research-lock.json; it is never committed.

Needs the Android SDK (ANDROID_HOME, else the default folder) with the build-tools, platform and
NDK pinned below, the aarch64-linux-android Rust target, cargo-ndk and a JDK (javac, keytool,
apksigner), of the pinned Java major (currently 25). Original exporters are compiled
on the host against checksum-pinned jars, which are never packaged. Without
HORIZON_ANDROID_KEYSTORE it signs with a debug key kept in target/android.
"""

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import zipfile
from pathlib import Path

# The bundler is a sibling script, imported by name whatever the caller's cwd is.
if str(Path(__file__).resolve().parent) not in sys.path:
    sys.path.insert(0, str(Path(__file__).resolve().parent))

import offline_bundle

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent

APPLICATION_ID = "rust.minecraft"
LABEL = "Minecraft"
VERSION_NAME = "0.9.0"
VERSION_CODE = 14
MIN_SDK = 26  # AAudio (cpal)
TARGET_SDK = 35
PLATFORM = 36
BUILD_TOOLS = "35.0.0"
NDK = "28.2.13676358"
ABI = "arm64-v8a"
TRIPLE = "aarch64-linux-android"
# Rust target, NDK compiler prefix and NDK C++ runtime directory differ on ARMv7.
TARGETS = {
    "arm64-v8a": (TRIPLE, TRIPLE, TRIPLE),
    "armeabi-v7a": ("armv7-linux-androideabi", "armv7a-linux-androideabi", "arm-linux-androideabi"),
    "x86": ("i686-linux-android", "i686-linux-android", "i686-linux-android"),
    "x86_64": ("x86_64-linux-android", "x86_64-linux-android", "x86_64-linux-android"),
}
LIBRARY = "horizon_android"
# Every LOAD segment aligned for 16 KB pages, which Android 15 devices may use.
PAGE_FLAGS = "-C link-arg=-Wl,-z,max-page-size=16384"
# A BuildId in the library, so the native tombstones in diagnostic ZIPs name it and its frames can be matched to
# the unstripped copy kept beside the APK (`lib<LIBRARY>-symbols.so`, docs/android-diagnostics.md).
BUILD_ID_FLAGS = "-C link-arg=-Wl,--build-id=sha1"
# The launcher icon: every density of `packaging/android/icon/`, rendered from its `ic_launcher.svg`
# with `rsvg-convert` (the tool `cargo run -p xtask -- icons` uses for the other icons).
ICONS = {
    "mipmap-mdpi": 48,
    "mipmap-hdpi": 72,
    "mipmap-xhdpi": 96,
    "mipmap-xxhdpi": 128,
    "mipmap-xxxhdpi": 256,
}
# What horizon-prepare reads from its version folder on the phone (assets/prepare-kit.zip):
# the pins, original sources and verified original exporter classes. The phone
# runs these against the player's jars; ARM32's runtime has no javac module.
PREPARE_KIT = (
    "research-lock.json",
    "crates/horizon-prepare/src/exporters.rs",
    "mods/horizon/src/test/java",
)


def sdk_dir():
    env = os.environ.get("ANDROID_HOME") or os.environ.get("ANDROID_SDK_ROOT")
    if env:
        return Path(env)
    if sys.platform == "win32":
        return Path(os.environ["LOCALAPPDATA"]) / "Android" / "Sdk"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Android" / "sdk"
    return Path.home() / "Android" / "Sdk"


def tool(directory, name):
    """An SDK tool, with Windows' .exe or .bat suffix."""
    suffixes = (".exe", ".bat", ".cmd", "") if os.name == "nt" else ("", ".exe", ".bat", ".cmd")
    for suffix in suffixes:
        path = directory / (name + suffix)
        if path.is_file():
            return path
    sys.exit(f"build_apk: {name} not found in {directory}")


def run(*args, **kwargs):
    print("+", " ".join(str(a) for a in args), flush=True)
    subprocess.run([str(a) for a in args], check=True, **kwargs)


def ndk_tool(ndk, name):
    host = {"win32": "windows-x86_64", "darwin": "darwin-x86_64"}.get(sys.platform, "linux-x86_64")
    return tool(ndk / "toolchains" / "llvm" / "prebuilt" / host / "bin", name)


def build_library(profile, ndk, sdk, abi=ABI):
    env = dict(os.environ, ANDROID_NDK_HOME=str(ndk), ANDROID_HOME=str(sdk))
    # Preserve source identity in local diagnostic reports, including source builds.
    try:
        env["HORIZON_ANDROID_BUILD_COMMIT"] = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=ROOT, text=True, stderr=subprocess.DEVNULL).strip()
        env["HORIZON_ANDROID_BUILD_DIRTY"] = str(bool(subprocess.check_output(
            ["git", "status", "--porcelain"], cwd=ROOT, text=True))).lower()
    except (OSError, subprocess.CalledProcessError):
        pass  # An exported source archive may not have Git metadata.
    env["RUSTFLAGS"] = (env.get("RUSTFLAGS", "") + " " + PAGE_FLAGS + " " + BUILD_ID_FLAGS).strip()
    run(
        "cargo", "ndk", "-t", abi, "--platform", MIN_SDK,
        "build", "--profile", profile, "-p", "horizon-android",
        cwd=ROOT, env=env,
    )


def build_launchers(ndk, out, abi=ABI):
    """The `java` and `javac` horizon-prepare runs on the phone (native/java_launcher.c), with the
    C++ runtime the Java runtime's libraries need: an app runs programs only from its library
    folder, so they go in the APK as libraries."""
    _, compiler, sysroot_triple = TARGETS[abi]
    clang = ndk_tool(ndk, f"{compiler}{MIN_SDK}-clang")
    source = HERE / "native" / "java_launcher.c"
    libraries = {}
    for name, flags in (("libstarling_java.so", []), ("libstarling_javac.so", ["-DJAVAC"])):
        target = out / name
        run(
            clang, "-O2", "-fPIE", "-pie", "-Wall", "-Werror", *flags,
            "-Wl,-z,max-page-size=16384", "-o", target, source, "-ldl",
        )
        libraries[name] = target
    sysroot = ndk_tool(ndk, "clang").parent.parent / "sysroot"
    libraries["libc++_shared.so"] = sysroot / "usr" / "lib" / sysroot_triple / "libc++_shared.so"
    return libraries


def zip_folder(folder, archive):
    """`folder`'s files in the zip `archive`, all dated alike: the same files make the same zip,
    which the app tells from the next by its bytes."""
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as z:
        for file in sorted(p for p in folder.rglob("*") if p.is_file()):
            info = zipfile.ZipInfo(file.relative_to(folder).as_posix(), (2026, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            z.writestr(info, file.read_bytes())


def assets(out, offline=False):
    """The APK's assets/: prepare-kit.zip (PREPARE_KIT, with the paths horizon-prepare reads),
    lowbudget.zip, the skin the setup draws the menus with (packaging/lowbudget/pack, made by its
    make.py), and StarlingVisuals.zip, the shader pack the app puts in shaderpacks/ (also left in
    `out` for the release page).  With `offline`, bundled/ holds the payload a fresh install
    plays from: the game files, the Java runtime and Bedrock's controls (offline_bundle.py)."""
    run(sys.executable, ROOT / "tools/build_android_exporters.py")
    folder = out / "assets"
    shutil.rmtree(folder, ignore_errors=True)
    folder.mkdir()
    fonts = folder / "diagnostic-fonts"
    fonts.mkdir()
    for name in ("Geist-Regular.ttf", "Geist-SemiBold.ttf"):
        shutil.copyfile(ROOT / "crates/horizon-theme/fonts" / name, fonts / name)
    with zipfile.ZipFile(folder / "prepare-kit.zip", "w", zipfile.ZIP_DEFLATED) as kit:
        for entry in PREPARE_KIT:
            path = ROOT / entry
            files = sorted(p for p in path.rglob("*.java")) if path.is_dir() else [path]
            for file in files:
                kit.write(file, file.relative_to(ROOT).as_posix())
        bundle = ROOT / "exporters"
        manifest = json.loads((bundle / "manifest.json").read_text(encoding="utf-8"))
        for name in ["manifest.json", *sorted(manifest["classes"])]:
            kit.write(bundle / name, "exporters/" + name)
    zip_folder(HERE.parent / "lowbudget" / "pack", folder / "lowbudget.zip")
    zip_folder(HERE.parent / "shaderpacks" / "starling-visuals", folder / "StarlingVisuals.zip")
    shutil.copyfile(folder / "StarlingVisuals.zip", out / "StarlingVisuals.zip")
    if offline:
        offline_bundle.build(folder / "bundled")
        if not (folder / "bundled" / "manifest.json").is_file():
            sys.exit("build_apk: the offline payload has no manifest")
    return folder


def compile_java(build_tools, android_jar, out):
    """StarlingActivity and LauncherActivity (java/) as classes.dex."""
    classes, dex = out / "classes", out / "dex"
    for folder in (classes, dex):
        shutil.rmtree(folder, ignore_errors=True)
        folder.mkdir()
    sources = sorted((HERE / "java").rglob("*.java")) + [out / "generated" / "io/github/juanluci/starling/R.java"]
    run("javac", "--release", "8", "-nowarn", "-classpath", android_jar, "-d", classes, *sources)
    # A single archive also avoids cmd.exe's command-line limit on Windows.
    archive = out / "classes.jar"
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_DEFLATED) as jar:
        for path in sorted(classes.rglob("*.class")):
            jar.write(path, path.relative_to(classes).as_posix())
    run(
        tool(build_tools, "d8"), "--release", "--min-api", MIN_SDK, "--lib", android_jar,
        "--output", dex, archive,
    )
    return dex / "classes.dex"


def link_resources(aapt2, android_jar, out, assets, abi=ABI, offline=False):
    """The manifest, the launcher icon, the themes and `assets`, compiled and linked into
    `out/base.apk`."""
    manifest = (HERE / "AndroidManifest.xml").read_text(encoding="utf-8")
    manifest = manifest.replace("@APPLICATION_ID@", APPLICATION_ID).replace("@LABEL@", LABEL)
    if abi != ABI:
        # Additional architectures may expose GLES only. Validate actual GPU limits.
        manifest = manifest.replace('android:version="0x400003" android:required="true"',
                                    'android:version="0x400003" android:required="false"')
    (out / "AndroidManifest.xml").write_text(manifest, encoding="utf-8")
    res = out / "res"
    shutil.rmtree(res, ignore_errors=True)
    for folder in ICONS:
        target = res / folder
        target.mkdir(parents=True)
        shutil.copyfile(HERE / "icon" / folder / "ic_launcher.png", target / "ic_launcher.png")
    # The window themes (res/: the starting window's icon and background).
    shutil.copytree(HERE / "res", res, dirs_exist_ok=True)
    shortcuts = res / "xml" / "shortcuts.xml"
    shortcuts.write_text(shortcuts.read_text(encoding="utf-8").replace("@APPLICATION_ID@", APPLICATION_ID), encoding="utf-8")
    # Match the setup dialogs without loading Rust or the game's renderer. They are white with
    # dark text, and their primary action is the dark ink so it stays legible on white: no violet.
    theme = (ROOT / "crates/horizon-theme/src/lib.rs").read_text(encoding="utf-8")
    light = theme.split("pub const LIGHT: Palette = Palette {", 1)[1].split("};", 1)[0]
    colors = dict(re.findall(r"(\w+): 0x([0-9a-fA-F]{6})", light))
    colors["accent"] = colors["ink"]
    colors["on_accent"] = colors["surface"]
    entries = "\n".join(
        f'<color name="starling_{name}">#{colors[name]}</color>'
        for name in ("page", "rule", "ink", "ink_soft", "accent", "on_accent")
    )
    (res / "values/starling-colors.xml").write_text(f"<resources>\n{entries}\n</resources>\n", encoding="utf-8")
    compiled = out / "compiled"
    shutil.rmtree(compiled, ignore_errors=True)
    compiled.mkdir()
    run(aapt2, "compile", "--dir", res, "-o", compiled)
    (out / "generated").mkdir(exist_ok=True)
    # Debuggable (adb's run-as reaches the app's own files) except when signed for a release.
    debug = [] if os.environ.get("HORIZON_ANDROID_KEYSTORE") else ["--debug-mode"]
    run(
        aapt2, "link", "-o", out / "base.apk",
        "--manifest", out / "AndroidManifest.xml",
        "--java", out / "generated", "--custom-package", "io.github.juanluci.starling",
        "-I", android_jar,
        "--min-sdk-version", MIN_SDK, "--target-sdk-version", TARGET_SDK,
        "--version-code", VERSION_CODE, "--version-name", VERSION_NAME,
        *debug,
        # The offline payload's archives are already compressed (its PNGs and OGGs have nothing
        # left to squeeze): storing them makes the APK the payload's own size and its first-run
        # read a straight copy.
        *(["-0", "zip", "-0", "xz"] if offline else []),
        "-A", assets,
        *sorted(compiled.glob("*.flat")),
    )


def check_offline_apk(apk, abi):
    """The offline APK carries one ABI and its payload's manifest; a build that lost either is
    not a build to publish."""
    with zipfile.ZipFile(apk) as z:
        names = z.namelist()
    abis = sorted({name.split("/")[1] for name in names if name.startswith("lib/") and "/" in name[4:]})
    if abis != [abi]:
        sys.exit(f"build_apk: the APK's libraries are for {abis}, not only {abi}")
    if "assets/bundled/manifest.json" not in names:
        sys.exit("build_apk: the offline payload's manifest is not in the APK")
    if not any(name.startswith("assets/bundled/game-files") for name in names):
        sys.exit("build_apk: the offline payload's game files are not in the APK")
    print(f"build_apk: offline payload present, single ABI {abi}")


def add_code(base, libraries, dex, unsigned, abi=ABI):
    """`base` plus the activity's dex and the libraries, compressed: Android extracts them at
    install (extractNativeLibs), as it must for the Java launchers to run."""
    shutil.copyfile(base, unsigned)
    with zipfile.ZipFile(unsigned, "a") as apk:
        apk.write(dex, "classes.dex", compress_type=zipfile.ZIP_DEFLATED)
        for name, path in sorted(libraries.items()):
            apk.write(path, f"lib/{abi}/{name}", compress_type=zipfile.ZIP_DEFLATED)


def sign(build_tools, aligned, apk, out):
    keystore = os.environ.get("HORIZON_ANDROID_KEYSTORE")
    alias = os.environ.get("HORIZON_ANDROID_KEY_ALIAS", "horizon-debug")
    env = dict(os.environ)
    if not keystore:
        keystore = out / "debug.keystore"
        env["HORIZON_ANDROID_STORE_PASSWORD"] = env["HORIZON_ANDROID_KEY_PASSWORD"] = "android"
        if not keystore.exists():
            run(
                "keytool", "-genkeypair", "-keystore", keystore, "-storepass", "android",
                "-keypass", "android", "-alias", alias, "-keyalg", "RSA", "-keysize", "2048",
                "-validity", "10000", "-dname", "CN=Horizon Android debug",
            )
    apksigner = tool(build_tools, "apksigner")
    run(
        apksigner, "sign", "--ks", keystore, "--ks-key-alias", alias,
        "--ks-pass", "env:HORIZON_ANDROID_STORE_PASSWORD",
        "--key-pass", "env:HORIZON_ANDROID_KEY_PASSWORD",
        "--out", apk, aligned, env=env,
    )
    run(apksigner, "verify", apk)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--profile", default="quick",
        help="Cargo profile (default: quick, for iteration; fast or release to measure and ship)",
    )
    parser.add_argument("--no-build", action="store_true", help="package the library built last")
    parser.add_argument("--install", action="store_true", help="adb install the APK")
    parser.add_argument(
        "--zip", action="store_true",
        help="also zip the APK, for chats that take no .apk",
    )
    parser.add_argument(
        "--keep-symbols", action="store_true", help="ship the library unstripped (backtraces)"
    )
    parser.add_argument(
        "--library-only",
        action="store_true",
        help="stop at the stripped library: no APK",
    )
    parser.add_argument(
        "--offline",
        action="store_true",
        help="carry the pinned game files, the Java runtime and the controls in the APK, so a "
        "fresh install downloads nothing (arm64-v8a only)",
    )
    parser.add_argument("--abi", choices=TARGETS, default=ABI, help="Android ABI (default: arm64-v8a)")
    args = parser.parse_args()
    triple, _, _ = TARGETS[args.abi]
    if args.offline and args.abi != ABI:
        sys.exit(f"build_apk: --offline is {ABI} only (the payload's Java runtime is)")

    sdk = sdk_dir()
    ndk = Path(os.environ.get("ANDROID_NDK_HOME") or sdk / "ndk" / NDK)
    build_tools = sdk / "build-tools" / BUILD_TOOLS
    android_jar = sdk / "platforms" / f"android-{PLATFORM}" / "android.jar"
    for path in (ndk, build_tools, android_jar):
        if not path.exists():
            sys.exit(f"build_apk: missing {path}")

    if not args.no_build:
        build_library(args.profile, ndk, sdk, args.abi)
    profile_dir = "debug" if args.profile == "dev" else args.profile
    built = ROOT / "target" / triple / profile_dir / f"lib{LIBRARY}.so"
    if not built.is_file():
        sys.exit(f"build_apk: {built} not built")

    out = ROOT / "target" / "android"
    out.mkdir(parents=True, exist_ok=True)
    library = out / f"lib{LIBRARY}.so"
    if args.keep_symbols:
        shutil.copyfile(built, library)
    else:
        run(ndk_tool(ndk, "llvm-strip"), "--strip-unneeded", "-o", library, built)
    if args.library_only:
        return
    if not args.keep_symbols:
        # The same library with its symbols, never shipped: CI keeps it to read crash frames. Native-mod
        # builds (--library-only) don't need one.
        shutil.copyfile(built, out / f"lib{LIBRARY}-symbols.so")

    link_resources(tool(build_tools, "aapt2"), android_jar, out, assets(out, args.offline),
                   args.abi, args.offline)
    dex = compile_java(build_tools, android_jar, out)
    libraries = build_launchers(ndk, out, args.abi)
    libraries[library.name] = library
    add_code(out / "base.apk", libraries, dex, out / "unsigned.apk", args.abi)
    aligned = out / "aligned.apk"
    run(tool(build_tools, "zipalign"), "-P", "16", "-f", "4", out / "unsigned.apk", aligned)
    apk = out / f"minecraft-{VERSION_NAME}-{args.abi}.apk"
    sign(build_tools, aligned, apk, out)
    if args.offline:
        check_offline_apk(apk, args.abi)
    print(apk)
    if args.zip:
        # Sent where an .apk is refused (a chat, a phone away from adb): unzip, then install.
        packed = apk.with_suffix(".apk.zip")
        with zipfile.ZipFile(packed, "w", zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            z.write(apk, apk.name)
        print(packed)
    if args.install:
        adb = tool(sdk / "platform-tools", "adb")
        run(adb, "install", "-r", apk)


if __name__ == "__main__":
    main()
