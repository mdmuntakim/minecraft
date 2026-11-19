#!/usr/bin/env python3
"""The APK's offline payload: every file a fresh install needs, inside the APK.

The game the setup fetches on a phone (the version file, the client and server
jars, the libraries, the asset index and its objects) and the Java runtime that
prepares them are normally downloaded when the player enters the app.  This
module gathers them once, at build time, into `assets/bundled/`:

    bundled/manifest.json              every payload file: path, size, sha256
    bundled/game-files.zip             the launcher layout horizon-prepare reads
                                       (versions/, libraries/, assets/)
    bundled/jre25-android-arm64.tar.xz the pinned Java runtime
    bundled/assets.zip                 Bedrock's control textures and the raw
                                       touch-button sources

Nothing is downloaded after the app is installed: `libhorizon_android.so` reads
these assets and unpacks them on the device (crates/horizon-client/src/bundled.rs).
The payload is never committed: it is built into `target/`, which is ignored.

    python3 packaging/android/offline_bundle.py --out target/android/assets/bundled
    python3 packaging/android/offline_bundle.py --out ... --from .local/minecraft

HORIZON_MINECRAFT_DOWNLOAD_BASE (or --base) may point at a mirror; the files
keep the paths they have on Mojang's hosts.  Every file is checked against the
SHA-1 its version file or index lists before it is packed.
"""
import argparse
import concurrent.futures
import hashlib
import json
import os
import re
import sys
import urllib.request
import zipfile
from pathlib import Path, PurePosixPath

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
MOJANG_META = "https://piston-meta.mojang.com"
MOJANG_DATA = "https://piston-data.mojang.com"
MOJANG_OBJECTS = "https://resources.download.minecraft.net"
ZIP_TIME = (2026, 1, 1, 0, 0, 0)
# The runtime the APK carries; the other ABIs are not built offline.
JRE_ASSET = "jre25-android-arm64.tar.xz"


def sha1(path):
    if not path.is_file():
        return None
    digest = hashlib.sha1()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download(url, path, checksum, local=None):
    """`url` into `path`, checked against `checksum` (SHA-1 of the game files,
    SHA-256 when the pin is 64 characters: the Java runtime)."""
    algorithm = hashlib.sha256 if len(checksum) == 64 else hashlib.sha1
    if path.is_file() and _digest(path, algorithm) == checksum:
        return path

    def digest_file(file):
        return _digest(file, algorithm)

    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_suffix(path.suffix + ".part")
    try:
        if local is not None and local.is_file() and digest_file(local) == checksum:
            part.write_bytes(local.read_bytes())
        else:
            with urllib.request.urlopen(url, timeout=120) as response, part.open("wb") as out:
                for chunk in iter(lambda: response.read(1 << 20), b""):
                    out.write(chunk)
        if digest_file(part) != checksum:
            raise ValueError(f"Checksum mismatch: {path.name}")
        part.replace(path)
    finally:
        part.unlink(missing_ok=True)
    return path


def _digest(path, algorithm):
    digest = algorithm()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def host_base(host, base):
    return (base or host).rstrip("/")


def version_file(pins, cache, base):
    """The pinned version file, as Mojang publishes it (or the mirror, same paths)."""
    version = pins["version"]
    checksum = pins["manifest_sha1"]
    accepted = [checksum, *pins.get("accepted_manifest_sha1", [])]
    path = cache / "versions" / version / f"{version}.json"
    if path.is_file() and sha1(path) in accepted:
        return path, version
    download(f"{host_base(MOJANG_META, base)}/v1/packages/{checksum}/{version}.json", path, checksum)
    return path, version


def library_applies(library):
    """The same rule starling-account/src/files.rs uses on the phone: Mojang's OS
    rules (a phone is Linux), artifacts only, no natives classifiers."""
    rules = library.get("rules")
    if not rules:
        return True
    allowed = False
    for rule in rules:
        name = rule.get("os", {}).get("name")
        if name is None or name == "linux":
            allowed = rule.get("action") == "allow"
    return allowed


def game_files(pins, base, cache, jobs, supplied=None):
    """Every file of the pinned version, in the official launcher's layout, with
    the SHA-1 and size the version file or the asset index lists."""
    manifest_path, version = version_file(pins, cache, base)
    data = json.loads(manifest_path.read_text(encoding="utf-8"))
    items = []

    def add(source, target, checksum, size):
        items.append({"source": source, "path": target, "sha1": checksum, "size": size})

    def jar(key, pinned, target):
        item = data["downloads"][key]
        if item["sha1"] != pinned:
            sys.exit(f"offline_bundle: the version file's {key} jar is not the pinned one")
        add(item["url"], target, item["sha1"], item["size"])

    add(f"{host_base(MOJANG_META, base)}/v1/packages/{pins['manifest_sha1']}/{version}.json",
        f"versions/{version}/{version}.json", pins["manifest_sha1"], manifest_path.stat().st_size)
    jar("client", pins["client_sha1"], f"versions/{version}/{version}.jar")
    jar("server", pins["server_sha1"], f"versions/{version}/{version}-server.jar")
    for library in data["libraries"]:
        artifact = library.get("downloads", {}).get("artifact")
        if not artifact or not library_applies(library):
            continue
        relative = PurePosixPath(artifact["path"])
        if relative.is_absolute() or ".." in relative.parts or "\\" in artifact["path"]:
            continue
        add(artifact["url"], f"libraries/{relative}", artifact["sha1"], artifact["size"])
    index = data["assetIndex"]
    index_path = cache / "assets" / "indexes" / f"{index['id']}.json"
    download(f"{host_base(MOJANG_META, base)}{_mojang_rest(index['url'], MOJANG_META)}",
             index_path, index["sha1"])
    add(f"{host_base(MOJANG_META, base)}{_mojang_rest(index['url'], MOJANG_META)}",
        f"assets/indexes/{index['id']}.json", index["sha1"], index_path.stat().st_size)
    objects = json.loads(index_path.read_text(encoding="utf-8"))["objects"]
    for object in objects.values():
        checksum = object["hash"]
        add(f"{host_base(MOJANG_OBJECTS, base)}/{checksum[:2]}/{checksum}",
            f"assets/objects/{checksum[:2]}/{checksum}", checksum, object["size"])
    items.sort(key=lambda item: item["path"])
    seen = set()
    unique = []
    for item in items:
        if item["path"] not in seen:
            seen.add(item["path"])
            unique.append(item)
            continue
        if item["sha1"] != next(i["sha1"] for i in unique if i["path"] == item["path"]):
            sys.exit(f"offline_bundle: {item['path']} is listed twice with different hashes")
    local = {
        f"versions/{version}/{version}.json": manifest_path,
    }
    if supplied:
        supplied = Path(supplied)
        for item in unique:
            candidate = supplied / item["path"]
            if candidate.is_file() and sha1(candidate) == item["sha1"]:
                local[item["path"]] = candidate
    with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
        futures = []
        for item in unique:
            url = item["source"]
            rest = _mojang_rest(url, MOJANG_META, MOJANG_DATA, MOJANG_OBJECTS)
            base_url = _base_for(url, base)
            url = f"{base_url}{rest}" if base_url else url
            target = cache / "files" / item["path"]
            futures.append(pool.submit(download, url, target, item["sha1"], local.get(item["path"])))
        for future in concurrent.futures.as_completed(futures):
            future.result()
    return version, unique


def _mojang_rest(url, *hosts):
    for host in hosts:
        if url.startswith(host):
            return url[len(host):]
    return url


def _base_for(url, base):
    if not base:
        return None
    for host in (MOJANG_META, MOJANG_DATA, MOJANG_OBJECTS):
        if url.startswith(host):
            return base.rstrip("/")
    return None


def jre_pin(abi):
    """The runtime starling-account/src/java.rs pins for the ABI."""
    if abi != "arm64-v8a":
        sys.exit("offline_bundle: the offline APK is arm64-v8a only")
    source = (ROOT / "crates/starling-account/src/java.rs").read_text(encoding="utf-8")
    block = source.split("pub const JRE25_ANDROID_ARM64: Runtime = Runtime {", 1)[1].split("};", 1)[0]
    url = re.search(r'url:\s*"([^"]+)"', block)
    if not url:
        # The phone build keeps an empty URL, so the runtime names the desktop's constant.
        name = re.search(r"url:\s*([A-Z0-9_]+)", block)
        url = name and re.search(
            rf'(?m)^const {re.escape(name.group(1))}: &str = "([^"]+)";', source)
    checksum = re.search(r'sha256:\s*"([0-9a-f]{64})"', block)
    if not url or not checksum:
        sys.exit("offline_bundle: the Java runtime pin is not in java.rs")
    return url.group(1), checksum.group(1)


def write_zip(folder, archive, entries):
    """`entries` (name, path) in a deterministic, stored zip: the payload does not
    compress further, and the same files make the same archive."""
    with zipfile.ZipFile(archive, "w", zipfile.ZIP_STORED) as z:
        for name, path in sorted(entries):
            info = zipfile.ZipInfo(name, ZIP_TIME)
            info.compress_type = zipfile.ZIP_STORED
            info.file_size = path.stat().st_size
            with path.open("rb") as source, z.open(info, "w") as out:
                for chunk in iter(lambda: source.read(1 << 20), b""):
                    out.write(chunk)


def blob_sha1(path):
    """Git's blob id, which bedrock-touch.json pins (`git hash-object`)."""
    digest = hashlib.sha1(b"blob %d\0" % path.stat().st_size)
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def controls(cache, jobs):
    """Bedrock's control textures (packaging/android/bedrock-controls.json) and the
    raw touch-button sources (crates/horizon-client/assets/bedrock-touch.json),
    each checked against the hash its manifest pins.  Entries carry the paths they
    have on Mojang's samples repository; sealed by the revision's own token."""
    entries = {}

    def fetch_all(manifest_file, fallback_repository, name_of):
        manifest = json.loads((ROOT / manifest_file).read_text(encoding="utf-8"))
        repository = manifest.get("repository", fallback_repository)
        revision = manifest["revision"]
        folder = cache / "controls" / hashlib.sha1(revision.encode()).hexdigest()[:16]
        tasks = []
        for entry in manifest["files"]:
            name = name_of(entry)
            url = f"https://raw.githubusercontent.com/{repository}/{revision}/{name}"
            tasks.append((name, folder / name, url, entry))
        with concurrent.futures.ThreadPoolExecutor(max_workers=jobs) as pool:
            futures = []
            for name, path, url, entry in tasks:
                if "sha256" in entry:
                    futures.append(pool.submit(download, url, path, entry["sha256"]))
                else:
                    futures.append(pool.submit(download_blob, url, path, entry["sha1"]))
            for future in concurrent.futures.as_completed(futures):
                future.result()
        for name, path, _url, entry in tasks:
            size = entry.get("size")
            if size is not None and path.stat().st_size != size:
                sys.exit(f"offline_bundle: {name} is not the size its manifest pins")
            if name in entries and entries[name].read_bytes() != path.read_bytes():
                sys.exit(f"offline_bundle: {name} differs between the two manifests")
            entries[name] = path

    def control_name(entry):
        source = entry.get("source", "")
        return "LICENSE.md" if entry.get("path") == "LICENSE.md" else \
            "resource_pack/textures/" + source

    fetch_all("packaging/android/bedrock-controls.json", "Mojang/bedrock-samples", control_name)
    fetch_all("crates/horizon-client/assets/bedrock-touch.json", "Mojang/bedrock-samples",
              lambda entry: entry["source"])
    return sorted(entries.items())


def download_blob(url, path, blob):
    """A file whose manifest pins its git blob id (SHA-1 with git's header)."""
    if path.is_file() and blob_sha1(path) == blob:
        return path
    path.parent.mkdir(parents=True, exist_ok=True)
    part = path.with_suffix(path.suffix + ".part")
    try:
        with urllib.request.urlopen(url, timeout=120) as response, part.open("wb") as out:
            for chunk in iter(lambda: response.read(1 << 20), b""):
                out.write(chunk)
        if blob_sha1(part) != blob:
            raise ValueError(f"Git blob mismatch: {path.name}")
        part.replace(path)
    finally:
        part.unlink(missing_ok=True)
    return path


def build(out, base=None, cache=None, from_dir=None, jobs=8):
    """Write `out` (bundled/) with the payload and its manifest; return the manifest."""
    out = Path(out)
    cache = Path(cache or ROOT / ".local/offline-bundle")
    cache.mkdir(parents=True, exist_ok=True)
    out.mkdir(parents=True, exist_ok=True)
    pins = json.loads((ROOT / "research-lock.json").read_text(encoding="utf-8"))["minecraft"]
    version, game = game_files(pins, base, cache, jobs, from_dir)

    game_zip = out / "game-files.zip"
    write_zip(cache / "files", game_zip, [(item["path"], cache / "files" / item["path"])
                                          for item in game])
    print(f"offline_bundle: {len(game)} game files, {game_zip.stat().st_size} bytes", flush=True)

    jre_url, jre_sha256 = jre_pin("arm64-v8a")
    jre = out / JRE_ASSET
    download(jre_url, jre, jre_sha256)
    print(f"offline_bundle: {JRE_ASSET}, {jre.stat().st_size} bytes", flush=True)

    assets_zip = out / "assets.zip"
    entries = controls(cache, jobs)
    write_zip(cache, assets_zip, entries)
    print(f"offline_bundle: {len(entries)} control files, {assets_zip.stat().st_size} bytes",
          flush=True)

    # What the rest of the APK already carries: the exporter bundle, the shader
    # pack, the fonts and the setup skin are listed with the payload, so one
    # manifest describes every offline file.
    extra = []
    for name in ("prepare-kit.zip", "lowbudget.zip", "StarlingVisuals.zip",
                 "diagnostic-fonts/Geist-Regular.ttf", "diagnostic-fonts/Geist-SemiBold.ttf"):
        path = out.parent / name
        if path.is_file():
            extra.append({"path": name, "size": path.stat().st_size, "sha256": sha256(path)})

    manifest = {
        "version": version,
        "abi": "arm64-v8a",
        "offline": True,
        "game_files": [
            {"path": item["path"], "size": item["size"], "sha1": item["sha1"]}
            for item in game
        ],
        "archives": [
            {"path": game_zip.name, "size": game_zip.stat().st_size, "sha256": sha256(game_zip),
             "entries": len(game)},
            {"path": jre.name, "size": jre.stat().st_size, "sha256": sha256(jre), "entries": 1},
            {"path": assets_zip.name, "size": assets_zip.stat().st_size,
             "sha256": sha256(assets_zip), "entries": len(entries)},
        ],
        "assets": extra,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1) + "\n", encoding="utf-8")
    print(f"offline_bundle: manifest with {len(game)} game files and {len(entries)} assets",
          flush=True)
    return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--out", default=str(ROOT / "target/android/assets/bundled"),
                        help="the bundled/ folder the APK carries")
    parser.add_argument("--base", default=os.environ.get("HORIZON_MINECRAFT_DOWNLOAD_BASE"),
                        help="a mirror of Mojang's hosts (same paths)")
    parser.add_argument("--cache", default=None, help="where downloaded files are kept")
    parser.add_argument("--from", dest="from_dir", default=os.environ.get("HORIZON_MINECRAFT_DIR"),
                        help="an existing launcher-layout folder to copy the game files from")
    parser.add_argument("--jobs", type=int, default=8)
    args = parser.parse_args()
    build(args.out, args.base, args.cache, args.from_dir, args.jobs)


if __name__ == "__main__":
    main()
