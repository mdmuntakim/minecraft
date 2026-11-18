# The Android launcher icon

`ic_launcher.svg` is the master: a grass block seen from above and left, drawn on the 16-unit grid
the other marks use (`docs/media/brand/`), on the app's dark tile. `build_apk.py` copies one PNG per
density from here into the APK's `res/mipmap-*dpi/ic_launcher.png`, so the home screen and the
Android 12 starting window (`res/values-v31/themes.xml`) show it.

The PNGs are generated, not hand-drawn. Re-render them after editing the master, with the same tool
`cargo run -p xtask -- icons` uses:

```bash
cd packaging/android/icon
for d in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:128 xxxhdpi:256; do
    folder="mipmap-${d%%:*}"; size="${d##*:}"
    rsvg-convert -w "$size" -h "$size" ic_launcher.svg -o "$folder/ic_launcher.png"
done
```

The art is original, drawn for this repository: it is not Mojang's Minecraft icon or any other
Mojang asset, which this repository never carries (`AGENTS.md`, `tools/check_public_tree.py`).
