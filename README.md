# minecraft

Minecraft's Android APK built **from source** at upstream
[starling-client/starling](https://github.com/starling-client/starling) (branch
`starling`, built with the project's own pipeline
`packaging/android/build_apk.py`: Rust + Android NDK + JDK 25 + aapt2/d8), and
signed with **this repository's own key**.

- The keystore is stored as repository secrets
  (`STARLING_KEYSTORE_BASE64`, `STARLING_KEY_ALIAS`,
  `STARLING_STORE_PASSWORD`, `STARLING_KEY_PASSWORD`) and is never committed.
- One app, application id `rust.minecraft` and app name **Minecraft**. There is
  no variant suffix (no `.dev`, no `.preview`): every build is the same package.
  It is signed with this project's key, not the official upstream release key,
  so it cannot install over or update the official Minecraft app.
- The launcher icon is `packaging/android/icon/ic_launcher.svg`, a pixel-art
  isometric grass block in the style of the current Minecraft icon, rendered per
  density. It is drawn for this repository, not copied from Mojang, and the app
  is not affiliated with Mojang or Microsoft (`AGENTS.md`).
- Certificate SHA-256 of the signing key is documented in every release's
  notes (`apksigner verify --print-certs` output).

## Build

```bash
rustup target add aarch64-linux-android && cargo install cargo-ndk
source <(echo "$STARLING_KEYSTORE_BASE64" | base64 -d > /tmp/ks.jks; echo)
HORIZON_ANDROID_KEYSTORE=/tmp/ks.jks \
HORIZON_ANDROID_KEY_ALIAS=starling-build \
HORIZON_ANDROID_STORE_PASSWORD=... HORIZON_ANDROID_KEY_PASSWORD=... \
ANDROID_NDK_HOME=$ANDROID_HOME/ndk/<version> \
python3 packaging/android/build_apk.py --profile release
```

## Workflow

`.github/workflows/build.yml` builds and signs the APK, and publishes it when
the dispatch asks for it:

```bash
gh workflow run android-build --repo mdmuntakim/minecraft -f publish=true
```
