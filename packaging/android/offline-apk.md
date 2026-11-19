# The offline APK

The standard build carries no game files: the setup downloads the pinned version
file, the client and server jars, the libraries and the asset objects from
Mojang, then downloads a Java runtime and prepares the resources on the phone.
The offline build carries all of it inside the APK, so a fresh install needs no
network at all.

```bash
ANDROID_NDK_HOME="$ANDROID_HOME/ndk/28.2.13676358" \
python3 packaging/android/build_apk.py --profile release --offline
```

`packaging/android/offline_bundle.py` gathers the payload once, at build time:

| Path in the APK | What it is |
| --- | --- |
| `assets/bundled/manifest.json` | every archived and APK-carried payload file: path, size, SHA-256 |
| `assets/bundled/game-files.zip` | the launcher layout `horizon-prepare` reads: `versions/26.3/` (the version file, the client and server jars), `libraries/`, `assets/indexes/34.json`, `assets/objects/` |
| `assets/bundled/jre25-android-arm64.tar.xz` | the OpenJDK 25 runtime `crates/starling-account/src/java.rs` pins, which prepares the game on the phone |
| `assets/bundled/assets.zip` | Bedrock's control textures (`packaging/android/bedrock-controls.json`) and the raw touch-button sources (`crates/horizon-client/assets/bedrock-touch.json`) |

Every game file is checked against the SHA-1 its version file or asset index
lists, and every control file against its SHA-256 (or its git blob id); the
version file itself against `research-lock.json`'s pin.  The payload is written
under `target/` (ignored) and never committed.

## What the app does with it

`crates/horizon-client/src/bundled.rs` reads `assets/bundled/manifest.json`,
streams an archive out of the APK (the AssetManager, not whole-file reads),
checks its SHA-256 and unpacks it:

- the setup's "game files" step (`accounts::download_files`) unpacks
  `game-files.zip` into the private folder, writes `starling-files.json` and is
  ready.  On Android it never calls the downloader;
- the preparer unpacks `jre25-android-arm64.tar.xz` instead of fetching it, then
  runs as before;
- the control textures and the touch-button sources come from `assets.zip`
  instead of Mojang's samples repository.

The downloader is out of the Android library: `starling-account`'s game-data
hosts and the runtime's publishers are `#[cfg(not(target_os = "android"))]`, and
the phone `install()` refuses.  `tools/check_no_downloader.py` checks that on
the sources and on the built `.so`/APK:

```bash
python3 tools/check_no_downloader.py --apk target/android/minecraft-0.9.0-arm64-v8a.apk
```

## Size and install

The payload is about 674 MB of game files and 38 MB of Java runtime; the APK is
about 745 MB.  It installs and starts like any other APK; the first start
unpacks the game files (one progress bar) instead of downloading them.  It is
arm64-v8a only, because the carried Java runtime is.
