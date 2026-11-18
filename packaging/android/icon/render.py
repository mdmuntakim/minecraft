#!/usr/bin/env python3
"""Render the launcher icon: a pixel-art isometric grass block, modern Minecraft style.

The artwork is generated here (it is original, not a Mojang file): a 16x16 texel grid
on each face of an isometric cube, the way the current game icon is drawn.  Writes
ic_launcher.svg and one PNG per density without a third-party renderer (a polygon
rasterizer and zlib), so it runs anywhere.

    python3 packaging/android/icon/render.py
"""
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent

N = 16
# Cube vertices on the 16-unit grid.
A = (8.0, 0.6)
B = (15.4, 4.8)
C = (8.0, 9.0)
D = (0.6, 4.8)
E = (0.6, 11.0)
F = (8.0, 15.2)
G = (15.4, 11.0)

TOP = ["#8BD45F", "#7FCB54", "#96DE6A", "#74C04B", "#86D25C", "#9BE673"]
GRASS_L = ["#6FBF49", "#63B03F", "#79C951", "#5BA83A"]
GRASS_R = ["#54A336", "#4A9430", "#5EAE3C", "#468A2C"]
DIRT_L = ["#9A6B41", "#8C5D35", "#A6774B", "#81522C", "#B08154", "#93643B"]
DIRT_R = ["#6E4828", "#624023", "#7A5230", "#57381D", "#845B36", "#684226"]
STONE_L = ["#8F8F8F", "#7C7C7C"]
STONE_R = ["#6E6E6E", "#5C5C5C"]

# A few stones in the dirt, in the same places on both sides.
LEFT_STONES = {(2, 7), (3, 8), (6, 5), (9, 12), (12, 9), (13, 13)}
RIGHT_STONES = {(1, 8), (4, 6), (7, 11), (10, 5), (12, 12), (14, 9)}

DENSITIES = {"mipmap-mdpi": 48, "mipmap-hdpi": 72, "mipmap-xhdpi": 96,
             "mipmap-xxhdpi": 128, "mipmap-xxxhdpi": 256}


def top_texel(i, j):
    return TOP[(i * 7 + j * 11 + (i // 3) * 5) % len(TOP)]


def side_texel(i, j, left):
    if j < 2:
        pal = GRASS_L if left else GRASS_R
        return pal[(i * 5 + j * 3) % len(pal)]
    stones = LEFT_STONES if left else RIGHT_STONES
    if (i, j) in stones:
        pal = STONE_L if left else STONE_R
        return pal[(i + j) % len(pal)]
    pal = DIRT_L if left else DIRT_R
    return pal[(i * 3 + j * 7 + (i // 4) * 3) % len(pal)]


def quad(p):
    return " ".join(f"{x:.3f},{y:.3f}" for x, y in p)


def add(p, q):
    return (p[0] + q[0], p[1] + q[1])


def scale(p, s):
    return (p[0] * s, p[1] * s)


def faces():
    """Every texel as (points, colour), with each face's base under it."""
    out = [((A, B, C, D), "#8BD45F"), ((D, C, F, E), "#8C5D35"), ((C, B, G, F), "#624023")]
    u_top = scale((B[0] - A[0], B[1] - A[1]), 1 / N)
    v_top = scale((D[0] - A[0], D[1] - A[1]), 1 / N)
    u_left = scale((C[0] - D[0], C[1] - D[1]), 1 / N)
    u_right = scale((B[0] - C[0], B[1] - C[1]), 1 / N)
    down = (0.0, (F[1] - C[1]) / N)
    for j in range(N):
        for i in range(N):
            p = add(add(A, scale(u_top, i)), scale(v_top, j))
            out.append(((p, add(p, u_top), add(add(p, u_top), v_top), add(p, v_top)),
                        top_texel(i, j)))
    for j in range(N):
        for i in range(N):
            p = add(add(D, scale(u_left, i)), scale(down, j))
            out.append(((p, add(p, u_left), add(add(p, u_left), down), add(p, down)),
                        side_texel(i, j, True)))
    for j in range(N):
        for i in range(N):
            p = add(add(C, scale(u_right, i)), scale(down, j))
            out.append(((p, add(p, u_right), add(add(p, u_right), down), add(p, down)),
                        side_texel(i, j, False)))
    return out


def svg():
    parts = [
        '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">',
        "  <title>Minecraft</title>",
        "  <!-- A pixel-art isometric grass block, drawn for this repository: 16x16 texels on",
        "       each face. Not a Mojang asset (see README.md in this folder). -->",
        '  <g shape-rendering="crispEdges">',
    ]
    for points, colour in faces():
        parts.append(f'    <polygon points="{quad(points)}" fill="{colour}"/>')
    parts += ["  </g>", "</svg>", ""]
    return "\n".join(parts)


def png(size):
    pixels = bytearray(size * size * 4)
    s = size / 16.0
    for points, colour in faces():
        rgb = tuple(int(colour[k:k + 2], 16) for k in (1, 3, 5))
        pts = [(x * s, y * s) for x, y in points]
        ys = [p[1] for p in pts]
        y0 = max(0, int(min(ys)))
        y1 = min(size - 1, int(max(ys)) + 1)
        for py in range(y0, y1 + 1):
            yc = py + 0.5
            xs = []
            for k in range(len(pts)):
                (x1, y1a), (x2, y2a) = pts[k], pts[(k + 1) % len(pts)]
                if (y1a <= yc < y2a) or (y2a <= yc < y1a):
                    xs.append(x1 + (yc - y1a) * (x2 - x1) / (y2a - y1a))
            xs.sort()
            for k in range(0, len(xs) - 1, 2):
                x0 = max(0, int(round(xs[k])))
                x1 = min(size, int(round(xs[k + 1])))
                for px in range(x0, x1):
                    o = (py * size + px) * 4
                    pixels[o:o + 4] = bytes((*rgb, 255))
    raw = b"".join(b"\x00" + bytes(pixels[y * size * 4:(y + 1) * size * 4]) for y in range(size))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xffffffff)

    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def main():
    (HERE / "ic_launcher.svg").write_text(svg(), encoding="utf-8")
    for folder, size in DENSITIES.items():
        (HERE / folder).mkdir(parents=True, exist_ok=True)
        (HERE / folder / "ic_launcher.png").write_bytes(png(size))
        print(HERE / folder / "ic_launcher.png")


if __name__ == "__main__":
    main()
