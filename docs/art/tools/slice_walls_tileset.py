#!/usr/bin/env python3
"""Slice atlas-walls-tileset.png (a continuous room wall boundary) into
wall TILES for the sprite pack.

Method — uniform patch grid over the wall ring:
1. keep the largest alpha component (the wall ring); stray paint specks drop;
2. normalize the ring so its band thickness ~ BAND px, sized to an exact
   multiple of CELL (nc x nr patches);
3. cut the ring into nc x nr patches of exactly CELL x CELL. Every patch is a
   consecutive crop of one continuous painting, so neighboring tiles reassemble
   seamlessly BY CONSTRUCTION — corners, straight runs, everything.
4. boundary patches become tiles (corners + n/s/w/e runs); interior is empty.

Placement is trivial: draw every tile centered on its cell — edge alignment is
baked in (north-run tiles carry the band at their top, etc.).

Run AFTER tools/slice_atlas_v4.py; merges wall entries into sprites/manifest.json.
"""
import json
import os
import re
from collections import deque

from PIL import Image, ImageDraw, ImageFont

ART = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SRC = os.path.join(ART, "atlas-walls-tileset.png")
OUT = os.path.join(ART, "sprites")
CELL = 256   # pack px per logical cell
BAND = 128   # pack px wall band thickness (top face + front face)
ALPHA_T = 8

# object-style wall pieces removed from the pack when the tileset landed
SUPERSEDED_WALL_PIECES = [
    "corner_ne", "corner_nw", "corner_se", "corner_sw",
    "wall_h_door", "wall_h_end", "wall_h_light", "wall_h_mid", "wall_h_pipes",
    "wall_v_door", "wall_v_end", "wall_v_light", "wall_v_mid", "wall_v_pipes",
]


def label_components(alpha, min_size):
    w, h = alpha.size
    px = alpha.load()
    seen = bytearray(w * h)
    comps = []
    for y0 in range(h):
        base = y0 * w
        for x0 in range(w):
            if seen[base + x0] or px[x0, y0] < ALPHA_T:
                continue
            pixels = []
            q = deque([(x0, y0)])
            seen[base + x0] = 1
            while q:
                x, y = q.popleft()
                pixels.append((x, y))
                for nx in (x - 1, x, x + 1):
                    for ny in (y - 1, y, y + 1):
                        if 0 <= nx < w and 0 <= ny < h:
                            idx = ny * w + nx
                            if not seen[idx] and px[nx, ny] >= ALPHA_T:
                                seen[idx] = 1
                                q.append((nx, ny))
            if len(pixels) >= min_size:
                xs = [p[0] for p in pixels]
                ys = [p[1] for p in pixels]
                comps.append({
                    "pixels": pixels,
                    "bbox": (min(xs), min(ys), max(xs) + 1, max(ys) + 1),
                })
    return comps


def band_thickness(alpha, side, samples=24):
    """Median MAX contiguous opaque run along sampled lines perpendicular to
    the band (robust to corner posts being taller than the band)."""
    w, h = alpha.size
    px = alpha.load()
    vals = []
    for i in range(samples):
        f = (i + 0.5) / samples
        best = cur = 0
        if side in ("top", "bottom"):
            x = round(f * (w - 1))
            for y in range(h):
                if px[x, y] >= ALPHA_T:
                    cur += 1
                    best = max(best, cur)
                else:
                    cur = 0
        else:
            y = round(f * (h - 1))
            for x in range(w):
                if px[x, y] >= ALPHA_T:
                    cur += 1
                    best = max(best, cur)
                else:
                    cur = 0
        vals.append(best)
    vals.sort()
    return vals[len(vals) // 2]


def main():
    if not os.path.exists(SRC):
        raise SystemExit(f"source image missing: {SRC}")
    os.makedirs(OUT, exist_ok=True)
    img = Image.open(SRC).convert("RGBA")
    comps = label_components(img.getchannel("A"), min_size=200)
    if not comps:
        raise SystemExit("no usable wall pixels found — was the right image exported? "
                         "Re-generate atlas-walls-tileset.png with true transparency.")
    ring = max(comps, key=lambda c: len(c["pixels"]))  # the wall ring, drop stray specks
    ring_img = img.crop(ring["bbox"])
    rw, rh = ring_img.size

    t_mean = (band_thickness(ring_img.getchannel("A"), "top")
              + band_thickness(ring_img.getchannel("A"), "bottom")) / 2
    if t_mean <= 0:
        raise SystemExit("wall band thickness measured as 0 — the ring has no solid "
                         "horizontal band; check the source image")
    s_band = BAND / t_mean
    nc = max(2, round(rw * s_band / CELL))  # patches across
    nr = max(2, round(rh * s_band / CELL))  # patches down
    sx, sy = nc * CELL / rw, nr * CELL / rh
    ring_img = ring_img.resize((nc * CELL, nr * CELL), Image.LANCZOS)
    print(f"环 bbox {ring['bbox']}, 带厚均值 {t_mean:.0f}px, 网格 {nc}x{nr}, "
          f"缩放 sx={sx:.3f} sy={sy:.3f}, 归一后 {nc * CELL}x{nr * CELL}")

    manifest_path = os.path.join(OUT, "manifest.json")
    try:
        with open(manifest_path, encoding="utf-8") as f:
            mf = json.load(f)
    except FileNotFoundError:
        raise SystemExit("sprites/manifest.json not found — run slice_atlas_v4.py first")
    # drop this script's previous entries, then re-emit them
    mf["sprites"] = [s for s in mf["sprites"]
                     if not s["id"].startswith(("wall_tile", "wall_corner"))]
    for f in os.listdir(OUT):
        if f.startswith(("wall_tile_", "wall_corner_")) and f.endswith(".png"):
            os.remove(os.path.join(OUT, f))  # stale tiles from an older grid size

    def patch(i, j):
        return ring_img.crop((i * CELL, j * CELL, (i + 1) * CELL, (j + 1) * CELL))

    made = []

    def emit(pid, i, j, placement):
        t = patch(i, j)
        b = t.getchannel("A").getbbox()
        if not b:
            print(f"WARNING: {pid} is empty — wall run shorter than expected")
            return
        t.save(os.path.join(OUT, f"{pid}.png"))
        bx, by, bx1, by1 = b
        made.append({
            "id": pid, "file": f"{pid}.png", "class": "wall_corner" if "corner" in pid else "wall_tile",
            "logical_size": "1x1", "source": "atlas-walls-tileset.png",
            "canvas": [CELL, CELL], "anchor_px": [CELL // 2, CELL // 2],
            "content_px": [bx, by, bx1 - bx, by1 - by],
            "placement": placement,
        })

    emit("wall_corner_nw", 0, 0, "corner tile: draw centered on the NW corner cell")
    emit("wall_corner_ne", nc - 1, 0, "corner tile: draw centered on the NE corner cell")
    emit("wall_corner_sw", 0, nr - 1, "corner tile: draw centered on the SW corner cell")
    emit("wall_corner_se", nc - 1, nr - 1, "corner tile: draw centered on the SE corner cell")
    for i in range(1, nc - 1):
        emit(f"wall_tile_n{i}", i, 0, "north wall: band baked at tile top; draw centered on cell")
        emit(f"wall_tile_s{i}", i, nr - 1, "south wall: band baked at tile bottom; draw centered on cell")
    for j in range(1, nr - 1):
        emit(f"wall_tile_w{j}", 0, j, "west wall: band baked at tile left; draw centered on cell")
        emit(f"wall_tile_e{j}", nc - 1, j, "east wall: band baked at tile right; draw centered on cell")

    mf["sprites"].extend(made)
    mf["sprites"].sort(key=lambda s: s["id"])
    mf["superseded"] = {
        "wall pieces": sorted(SUPERSEDED_WALL_PIECES),
        "reason": "replaced by wall tiles cut from the continuous room-boundary "
                  "painting (seams impossible by construction)",
    }
    with open(manifest_path, "w", encoding="utf-8") as f:
        json.dump(mf, f, ensure_ascii=False, indent=2)

    # refresh preview.png from the full merged manifest
    names = [s["id"] for s in mf["sprites"]]
    thumbs = []
    for n in names:
        t = Image.open(os.path.join(OUT, f"{n}.png"))
        t.thumbnail((140, 140))
        thumbs.append((n, t))
    cols_n = 8
    rows_n = (len(thumbs) + cols_n - 1) // cols_n
    cell_w, cell_h = 170, 190
    sheet = Image.new("RGBA", (cols_n * cell_w, rows_n * cell_h), (52, 54, 64, 255))
    draw = ImageDraw.Draw(sheet)
    try:
        font = ImageFont.load_default(14)
    except TypeError:
        font = ImageFont.load_default()
    for i, (n, t) in enumerate(thumbs):
        cx, cy = (i % cols_n) * cell_w, (i // cols_n) * cell_h
        sheet.alpha_composite(t, (cx + (cell_w - t.size[0]) // 2, cy + (cell_h - 34 - t.size[1]) // 2))
        draw.text((cx + cell_w // 2, cy + cell_h - 24), n, fill=(230, 230, 230), font=font, anchor="mm")
    sheet.convert("RGB").save(os.path.join(OUT, "preview.png"))

    # keep the sprite count in README.md current
    readme = os.path.join(OUT, "README.md")
    if os.path.exists(readme):
        with open(readme, encoding="utf-8") as f:
            text = f.read()
        text = re.sub(r"共 \d+ 个精灵", f"共 {len(names)} 个精灵", text)
        with open(readme, "w", encoding="utf-8") as f:
            f.write(text)

    print(f"OK: {len(made)} wall tiles -> {OUT} (manifest merged, "
          f"{len(names)} sprites total)")


if __name__ == "__main__":
    main()
