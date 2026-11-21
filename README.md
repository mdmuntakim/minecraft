# minecraft

Minecraft's Android APK built **from source** at upstream
[starling-client/starling](https://github.com/starling-client/starling) (branch
`starling`, built with the project's own pipeline
`packaging/android/build_apk.py`: Rust + Android NDK + JDK 25 + aapt2/d8), and
signed with **this repository's own key**.

- The keystore is stored as repository secrets
  (`STARLING_KEYSTORE_BASE64`, `STARLING_KEY_ALIAS`,
  `STARLING_STORE_PASSWORD`, `STARLING_KEY_PASSWORD`) and is never committed.
- One app, application id `rust.minecraft` and app name **Minecraft**, with no
  variant suffix: every build is the same package.
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

## Project status

**Player-visible gaps: 0 open** of the 833 the [port ledger](tools/port_ledger.py) found, reading every vanilla 26.3 method against Horizon's code: vanilla behaviour a player would notice that Horizon doesn't have yet. By area: Animals [0](https://github.com/horizon-rust/horizon/issues/3512); Monsters and bosses [0](https://github.com/horizon-rust/horizon/issues/3513); Entities, projectiles, vehicles and decorations [0](https://github.com/horizon-rust/horizon/issues/3514); Items, enchantments and inventories [0](https://github.com/horizon-rust/horizon/issues/3515); Blocks, redstone and fluids [0](https://github.com/horizon-rust/horizon/issues/3516); World, world generation and saves [0](https://github.com/horizon-rust/horizon/issues/3517); Screens, HUD and client packets [0](https://github.com/horizon-rust/horizon/issues/3518); Entity and block rendering, models [0](https://github.com/horizon-rust/horizon/issues/3519); Server and players on the server [0](https://github.com/horizon-rust/horizon/issues/3520); Mob AI, villagers and raids [0](https://github.com/horizon-rust/horizon/issues/3521). This number goes down as gaps close (`tools/parity_gaps.py`); the harness results below say that what is tested matches.

[![Player-visible gaps open](https://img.shields.io/badge/player--visible%20gaps-0%20open-F0795A)](https://github.com/mdmuntakim/minecraft/issues)

### Java parity: 100%

The Android port of Horizon, measured against vanilla by the differential
harnesses in `tests/` and the port ledger in `tools/port_ledger.py`. Every
subsystem is at or above parity as of the last audit; the per-subsystem scores
and the known differences live in [the parity audit](docs/horizon-parity.md).
