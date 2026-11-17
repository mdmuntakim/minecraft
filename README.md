# starling-build

Starling's Android APK built **from source** at upstream
[starling-client/starling](https://github.com/starling-client/starling) (branch
`starling`, built with the project's own pipeline
`packaging/android/build_apk.py`: Rust + Android NDK + JDK 25 + aapt2/d8), and
signed with **this repository's own key**.

- The keystore is stored as repository secrets
  (`STARLING_KEYSTORE_BASE64`, `STARLING_KEY_ALIAS`,
  `STARLING_STORE_PASSWORD`, `STARLING_KEY_PASSWORD`) and is never committed.
- The APK is built in the **preview** variant, application id
  `io.github.juanluci.starling.preview` ("Starling Preview"), so it installs
  **beside** the official app. It can never install over, update or replace the
  official Starling release: the official upstream release key is not used,
  requested or stored here.
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
python3 packaging/android/build_apk.py --preview --profile release
```

## Two-job workflow

`.github/workflows/build.yml` has **two parallel jobs**, `starling` and
`twofold`, started by **one dispatch**: the `starling` job builds this
repository from source, the `twofold` job checks out `mdmuntakim/twofold`,
runs its acceptance script and (optionally) publishes both releases.

```bash
gh workflow run android-build --repo mdmuntakim/starling-build -f publish=true
```
