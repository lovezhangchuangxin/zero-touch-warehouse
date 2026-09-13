#!/usr/bin/env python3
"""Slice atlas-v4 sprite sheets into a normalized production sprite pack.

Reads atlas-v4-{A,B,C,D}.png next to this script's parent, writes:
  ../sprites/<name>.png          individual sprites on footprint-sized canvases
  ../sprites/manifest.json       engine coordinate table
  ../sprites/preview.png         labeled contact sheet
  ../sprites/README.md           usage notes

EXTRACTION METHOD: connected-component extraction. The sheet is labeled into
8-connected alpha components once; each component is assigned to the slot it
belongs to (its bbox intersects the slot AND its centroid falls inside the
slot neighborhood). Objects drawn LARGER than their grid slot survive
unclipped — fixed-grid cropping would cut them. Tiny stray components are
dropped unless the slot is whitelisted (fx_dust_*, box_crushed: their
scatter is intentional).

CANVAS = LOGICAL FOOTPRINT:
- 1x1 objects        -> 256x256
- 1 wide x 2 deep    -> 256x512
- 2 wide x 1 deep    -> 512x256
- markers            -> 256x256 (cell overlays)
- icons / fx         -> 192x192 (not grid-bound; never upscaled)
Content is uniformly scaled to fit its canvas and centered, so the canvas
center == footprint center == cell center (icons/fx keep native size unless
oversized). Exceptionally tall art (pillar) is scaled down to fit and is
therefore slightly narrower than the cell — tune per-object scale in the
engine if it matters.
Wall pieces are NOT produced here: they are tiles cut from a continuous
room-boundary painting by slice_walls_tileset.py (seamless by construction).
"""
import json
import os
from collections import deque

from PIL import Image, ImageDraw, ImageFont

ART = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ART, "sprites")

CELL = 256  # pack px per logical cell

CANVAS = {
    "grounded1": (256, 256), "grounded1h": (256, 256),
    "grounded2v": (256, 512), "grounded2h": (512, 256),
    "marker": (256, 256), "icon": (192, 192), "fx": (192, 192),
}
ALPHA_T = 8
DESPECKLE_MIN = 100
CENTROID_BAND = 0.3  # slot-claim neighborhood: 0.3 slot beyond the slot rect

# grounded1: 1-cell object, footprint axis = width
# grounded1h: 1-cell object whose length runs vertically (N-S walls),
#             footprint axis = height (thickness becomes the free axis)
# grounded2v: 1 wide x 2 deep, footprint axis = width
# grounded2h: 2 wide x 1 deep, footprint axis = height
TABLE = {
    "atlas-v4-A.png": (4, 3, {
        (0, 0): ("robot_empty_s", "grounded1"), (1, 0): ("robot_empty_n", "grounded1"),
        (2, 0): ("robot_empty_e", "grounded1"), (3, 0): ("robot_empty_w", "grounded1"),
        (0, 1): ("robot_blink_s", "grounded1"), (1, 1): ("robot_blink_n", "grounded1"),
        (2, 1): ("robot_blink_e", "grounded1"), (3, 1): ("robot_blink_w", "grounded1"),
        (0, 2): ("robot_loaded_s", "grounded1"), (1, 2): ("robot_loaded_n", "grounded1"),
        (2, 2): ("robot_loaded_e", "grounded1"), (3, 2): ("robot_loaded_w", "grounded1"),
    }),
    "atlas-v4-B.png": (5, 3, {
        (0, 0): ("box_water", "grounded1"), (1, 0): ("box_battery", "grounded1"),
        (2, 0): ("box_chip", "grounded1"), (3, 0): ("box_plain", "grounded1"),
        (4, 0): ("box_crushed", "grounded1"),
        (0, 1): ("shelf_empty", "grounded1"), (1, 1): ("charger_idle", "grounded1"),
        (2, 1): ("charger_on", "grounded1"),
        (3, 1): ("truck_s_empty", "grounded2v"), (4, 1): ("truck_n_empty", "grounded2v"),
        (0, 2): ("port_v_empty", "grounded2v"), (1, 2): ("port_v_occupied", "grounded2v"),
        (2, 2): ("port_h_empty", "grounded2h"), (3, 2): ("port_h_occupied", "grounded2h"),
        (4, 2): ("floor_hazard", "grounded1"),
    }),
    "atlas-v4-C.png": (4, 4, {
        # wall pieces live in the tileset now (tools/slice_walls_tileset.py);
        # only the two standalone props remain in this atlas.
        (0, 3): ("pillar", "grounded1"), (3, 3): ("drain_grate", "grounded1"),
    }),
    "atlas-v4-D.png": (4, 4, {
        (0, 0): ("fx_dust_1", "fx"), (1, 0): ("fx_dust_2", "fx"),
        (2, 0): ("fx_dust_3", "fx"), (3, 0): ("fx_pop_star", "fx"),
        (0, 1): ("fx_sparkle_1", "fx"), (1, 1): ("fx_sparkle_2", "fx"),
        (2, 1): ("fx_sparkle_3", "fx"), (3, 1): ("icon_bolt", "icon"),
        (0, 2): ("icon_alert", "icon"), (1, 2): ("icon_check", "icon"),
        (2, 2): ("icon_battery_full", "icon"), (3, 2): ("icon_battery_low", "icon"),
        (0, 3): ("icon_zzz", "icon"), (1, 3): ("marker_select", "marker"),
        (2, 3): ("marker_dest", "marker"), (3, 3): ("fx_twinkle", "fx"),
    }),
}

NO_DESPECKLE = {"fx_dust_1", "fx_dust_2", "fx_dust_3", "box_crushed"}

PPC_REFS = {
    "atlas-v4-A.png": [(0, 0), (1, 0), (2, 0), (3, 0)],
    "atlas-v4-B.png": [(0, 1), (1, 1), (2, 1)],
    "atlas-v4-C.png": [(0, 3), (3, 3)],
}

FOOTPRINT = {
    "grounded1": "1x1", "grounded1h": "1x1",
    "grounded2v": "1x2v", "grounded2h": "2x1h", "marker": "1x1",
}


def slot_rect(w, h, cols, rows, col, row):
    x0 = round(col * w / cols)
    x1 = round((col + 1) * w / cols)
    y0 = round(row * h / rows)
    y1 = round((row + 1) * h / rows)
    return x0, y0, x1, y1


def label_components(alpha):
    """8-connected components of alpha >= ALPHA_T over the whole sheet."""
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
            xs = 0
            ys = 0
            bx0 = by0 = 10 ** 9
            bx1 = by1 = -1
            for x, y in pixels:
                xs += x
                ys += y
                bx0 = min(bx0, x)
                by0 = min(by0, y)
                bx1 = max(bx1, x)
                by1 = max(by1, y)
            comps.append({
                "pixels": pixels, "size": len(pixels),
                "bbox": (bx0, by0, bx1 + 1, by1 + 1),
                "cx": xs / len(pixels), "cy": ys / len(pixels),
            })
    return comps


def extract(img, comps, claims, rect, cls, name, slot_w, slot_h):
    """Collect one slot's components, scale to fit the footprint canvas.
    claims[i] counts how many slots claimed component i — a component claimed
    by two slots is copied into both sprites (silent duplication), so the
    caller warns about it."""
    x0, y0, x1, y1 = rect
    keep = []
    for ci, c in enumerate(comps):
        bx0, by0, bx1, by1 = c["bbox"]
        if not (bx0 < x1 and bx1 > x0 and by0 < y1 and by1 > y0):
            continue
        mx, my = slot_w * CENTROID_BAND, slot_h * CENTROID_BAND  # centroid must belong to this slot
        if not (x0 - mx <= c["cx"] <= x1 + mx and y0 - my <= c["cy"] <= y1 + my):
            continue
        if c["size"] < DESPECKLE_MIN and name not in NO_DESPECKLE:
            continue  # stray speck
        keep.append(ci)
    if not keep:
        raise SystemExit(f"no components for slot {rect} ({name})")

    bx0 = min(comps[ci]["bbox"][0] for ci in keep)
    by0 = min(comps[ci]["bbox"][1] for ci in keep)
    bx1 = max(comps[ci]["bbox"][2] for ci in keep)
    by1 = max(comps[ci]["bbox"][3] for ci in keep)
    sprite = img.crop((bx0, by0, bx1, by1)).copy()
    mask = Image.new("L", sprite.size, 0)
    mp = mask.load()
    op = img.load()
    for ci in keep:
        for x, y in comps[ci]["pixels"]:
            mp[x - bx0, y - by0] = op[x, y][3]
    sprite.putalpha(mask)
    for ci in keep:
        claims[ci].append(name)

    cw, ch = CANVAS[cls]
    bw, bh = sprite.size
    if cls in ("icon", "fx"):
        scale = min(1.0, min(cw / bw, ch / bh))  # never upscale icons/fx
    else:
        scale = min(cw / bw, ch / bh)
    if abs(scale - 1.0) > 1e-9:
        sprite = sprite.resize((round(bw * scale), round(bh * scale)), Image.LANCZOS)
    # snap to even dims so centering is exact
    w, h = sprite.size
    if w % 2 or h % 2:
        even = Image.new("RGBA", (w + w % 2, h + h % 2), (0, 0, 0, 0))
        even.paste(sprite, (0, 0))
        sprite = even
    return sprite


def main():
    os.makedirs(OUT, exist_ok=True)

    missing = [a for a in TABLE if not os.path.exists(os.path.join(ART, a))]
    if missing:
        raise SystemExit(f"missing atlas images: {missing} — regenerate them first")

    entries = []  # (name, cls, atlas, slot, sprite)
    px_ppc = {}
    for atlas, (cols, rows, slots) in TABLE.items():
        img = Image.open(os.path.join(ART, atlas)).convert("RGBA")
        w, h = img.size
        comps = label_components(img.getchannel("A"))
        claims = [[] for _ in comps]
        widths = []
        for col, row in PPC_REFS.get(atlas, []):
            b = img.crop(slot_rect(w, h, cols, rows, col, row)).getchannel("A").getbbox()
            if b:
                widths.append(b[2] - b[0])
        if widths:
            widths.sort()
            px_ppc[atlas] = widths[len(widths) // 2]
        slot_w, slot_h = w / cols, h / rows
        for (col, row), (name, cls) in slots.items():
            rect = slot_rect(w, h, cols, rows, col, row)
            sp = extract(img, comps, claims, rect, cls, name, slot_w, slot_h)
            entries.append((name, cls, atlas, [col, row], sp))
        for ci, names_ in enumerate(claims):
            if len(names_) > 1:
                print(f"WARNING: {atlas} component #{ci} claimed by {names_} "
                      f"— copied into each; check those sprites")
    if not entries:
        raise SystemExit("no sprites extracted")

    manifest_sprites = []
    for name, cls, atlas, slot, sp in sorted(entries):
        cw, ch = CANVAS[cls]
        canvas = Image.new("RGBA", (cw, ch), (0, 0, 0, 0))
        x, y = (cw - sp.size[0]) // 2, (ch - sp.size[1]) // 2
        canvas.paste(sp, (x, y))
        canvas.save(os.path.join(OUT, f"{name}.png"))
        manifest_sprites.append({
            "id": name, "file": f"{name}.png", "class": cls,
            "logical_size": FOOTPRINT.get(cls),
            "source": atlas, "slot": slot,
            "canvas": [cw, ch], "anchor_px": [cw // 2, ch // 2],
            "content_px": [x, y, sp.size[0], sp.size[1]],
        })

    manifest = {
        "conventions": {
            "cell_px": CELL,
            "canvas": "canvas == logical footprint: 1x1 = 256x256, 1x2v = 256x512, "
                      "2x1h = 512x256; markers 256x256; icons/fx 192x192.",
            "anchor": "anchor_px = canvas center = footprint center. Draw the canvas "
                      "centered on the object's cell (or 2-cell region center).",
            "content_px": "content rect inside the canvas as [x, y, w, h], for hit "
                          "tests/occlusion",
            "extraction": "connected-component assignment to slots (objects larger than "
                          "their grid slot survive unclipped)",
            "note": "art with strong perspective (pillar) is scaled down to fit "
                    "its footprint canvas and is slightly narrower than the cell; "
                    "icons/fx keep their native size, only scaled down when oversized",
        },
        "px_per_cell_source_sheets": px_ppc,
        "sprites": manifest_sprites,
    }
    with open(os.path.join(OUT, "manifest.json"), "w", encoding="utf-8") as f:
        json.dump(manifest, f, ensure_ascii=False, indent=2)

    # drop stale PNGs that this run no longer produces (e.g. pieces superseded
    # by the wall tileset); manifest.json / preview.png / README.md are kept
    known = {f"{s['id']}.png" for s in manifest_sprites}
    for f in os.listdir(OUT):
        if f.endswith(".png") and f not in known:
            os.remove(os.path.join(OUT, f))
            print(f"removed stale {f}")

    # labeled contact sheet
    names = [s["id"] for s in manifest_sprites]
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

    with open(os.path.join(OUT, "README.md"), "w", encoding="utf-8") as f:
        f.write(f"""# v4 切片产出（自动生成，勿手改）

由 `../tools/slice_atlas_v4.py` 从 atlas-v4-{{A,B,C,D}}.png 切出，共 {len(names)} 个精灵。

## 画布 = 逻辑足迹

- 1×1 物 → {CELL}×{CELL}；1×2 竖物（卡车/竖装卸位）→ {CELL}×512；2×1 横物 → 512×{CELL}；
  标记 256×256；图标/特效 192×192。**画布尺寸即占格数，一张图对应一个逻辑区域。**
- 内容等比缩放到恰好放进画布并**居中**；`anchor_px` = 画布中心 = 格子（或两格区域）中心。
  引擎只需把画布中心对齐到格子中心，无需其他偏移。图标/特效保持原始尺寸居中，仅超限时缩小。
- 墙瓦片由 `slice_walls_tileset.py` 单独生成（贴边对齐已烘进瓦片），不在本脚本产出内。
- 特别高挑的物件（立柱等）会等比缩小到画布内，宽度略小于一格，引擎可按需微调。

## 切取方法

连通域提取：整图按 8 连通标记 alpha 连通块，按"包围盒与槽位相交 + 质心落在槽位邻域"归属
槽位；超出槽位的物体（长卡车等）完整保留不裁剪；小于 {DESPECKLE_MIN}px 的杂散斑自动丢弃
（尘土序列与压毁箱白名单除外）。

## 其他

- `logical_size`：1x1 / 1x2v / 2x1h；`content_px` 为内容在画布内的精确位置（可用于命中测试）。
- 地面由引擎绘制（纯色 + 网格线）；`drain_grate` 为独立排水格栅贴花。
- 重新生成：`python3 ../tools/slice_atlas_v4.py`
""")
    print(f"OK: {len(names)} sprites -> {OUT}")


if __name__ == "__main__":
    main()
