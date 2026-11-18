# The Android launcher icon

`ic_launcher.svg` is the master: a pixel-art isometric grass block — a 16x16 texel grid on
each face of the cube — in the style of the current Minecraft icon. `build_apk.py` copies
one PNG per density from here into the APK's `res/mipmap-*dpi/ic_launcher.png`, so the home
screen and the Android 12 starting window (`res/values-v31/themes.xml`) show it.

The PNGs are generated, not hand-drawn, by `render.py` in this folder (a polygon rasterizer
that writes the same geometry at 48/72/96/128/256 px, with no third-party renderer):

```bash
python3 packaging/android/icon/render.py
```

Editing the master by hand works too; re-render the densities with the same tool
`cargo run -p xtask -- icons` uses:

```bash
cd packaging/android/icon
for d in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:128 xxxhdpi:256; do
    folder="mipmap-${d%%:*}"; size="${d##*:}"
    rsvg-convert -w "$size" -h "$size" ic_launcher.svg -o "$folder/ic_launcher.png"
done
```

The art is original, drawn for this repository in the modern icon's style: it is not
Mojang's Minecraft icon or any other Mojang asset, which this repository never carries
(`AGENTS.md`, `tools/check_public_tree.py`).
