#!/usr/bin/env python3
"""End-to-end scene composition test: renders a mock warehouse scene using
sprites/ + manifest.json the way an engine would (engine-drawn floor, sprites
placed by anchor at their logical footprint center).

Re-run after every slicer/asset change:  python3 tools/scene_test.py
Output: ../atlas-v4-scene-test.png
"""
import json
import os

from PIL import Image, ImageDraw

ART = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SP = os.path.join(ART, "sprites")

CELL = 96          # render px per cell
COLS, ROWS = 14, 9

mf = json.load(open(os.path.join(SP, "manifest.json"), encoding="utf-8"))
sp = {s["id"]: s for s in mf["sprites"]}

REQUIRED = [
    "wall_corner_nw", "wall_corner_ne", "wall_corner_sw", "wall_corner_se",
    "shelf_empty", "charger_on", "port_v_empty", "port_v_occupied",
    "truck_s_empty", "truck_n_empty", "robot_empty_s", "robot_empty_w",
    "robot_loaded_e", "box_water", "box_battery", "box_chip", "box_plain",
    "marker_select", "marker_dest", "icon_bolt", "icon_battery_low",
    "fx_dust_2", "floor_hazard", "drain_grate",
]
missing = [r for r in REQUIRED if r not in sp]
if missing:
    raise SystemExit(f"manifest is missing required sprites: {missing} — "
                     f"re-run the slicing scripts in order")
cache = {}


def sprite_img(name, px_w):
    if (name, px_w) not in cache:
        s = sp[name]
        img = Image.open(os.path.join(SP, s["file"]))
        sc = px_w / s["canvas"][0]
        img = img.resize((round(img.size[0] * sc), round(img.size[1] * sc)), Image.LANCZOS)
        cache[(name, px_w)] = (img, sc)
    return cache[(name, px_w)]


def put(name, cx, cy, px_w=None):
    """Draw sprite so anchor_px lands on scene point (cx, cy)."""
    px_w = px_w or CELL
    img, sc = sprite_img(name, px_w)
    ax, ay = sp[name]["anchor_px"]
    scene.alpha_composite(img, (round(cx - ax * sc), round(cy - ay * sc)))


def cell_center(cx, cy):
    return cx * CELL + CELL // 2, cy * CELL + CELL // 2


def region_center(cx0, cy0, w_cells, h_cells):
    return cell_center(cx0, cy0)[0] + (w_cells - 1) * CELL // 2, \
           cell_center(cx0, cy0)[1] + (h_cells - 1) * CELL // 2


scene = Image.new("RGBA", (COLS * CELL, ROWS * CELL))
d = ImageDraw.Draw(scene)
d.rectangle([0, 0, scene.size[0], scene.size[1]], fill=(228, 222, 208, 255))
for i in range(COLS + 1):
    d.line([(i * CELL, 0), (i * CELL, scene.size[1])], fill=(210, 203, 188, 255))
for j in range(ROWS + 1):
    d.line([(0, j * CELL), (scene.size[0], j * CELL)], fill=(210, 203, 188, 255))

# --- boundary walls: TILE-BASED (edge alignment baked into each tile) ---
def wall_pool(side):
    return sorted(k for k in sp if k.startswith(f"wall_tile_{side}"))


put("wall_corner_nw", *cell_center(0, 0))
put("wall_corner_ne", *cell_center(COLS - 1, 0))
put("wall_corner_sw", *cell_center(0, ROWS - 1))
put("wall_corner_se", *cell_center(COLS - 1, ROWS - 1))
openings = {(2, 0), (12, 0)}  # 装卸口开在墙的缺口上：缺口格不铺墙瓦
for i, x in enumerate(range(1, COLS - 1)):
    if (x, 0) not in openings:  # 只跳过北墙缺口，南墙不受影响
        put(wall_pool("n")[i % len(wall_pool("n"))], *cell_center(x, 0))
    put(wall_pool("s")[i % len(wall_pool("s"))], *cell_center(x, ROWS - 1))

for i, y in enumerate(range(1, ROWS - 1)):
    put(wall_pool("w")[i % len(wall_pool("w"))], *cell_center(0, y))
    put(wall_pool("e")[i % len(wall_pool("e"))], *cell_center(COLS - 1, y))

# --- 装卸口摆在墙缺口处：缺口格 + 库内格组成 1x2 ---
put("port_v_empty", *region_center(2, 0, 1, 2))      # 空闲装卸口（虚线框 = 可接单）
put("port_v_occupied", *region_center(12, 0, 1, 2))  # 占用装卸口 + 停靠卡车
put("truck_s_empty", *region_center(12, 0, 1, 2))
# --- shelves: base tile + boxes in the 2x2 sub-cell slots (±0.25 cell) ---
# 3 of 4 slots filled on purpose: a partially stocked shelf
put("shelf_empty", *cell_center(7, 2))
for dx, dy, box in ((-0.25, -0.25, "box_water"), (0.25, -0.25, "box_battery"),
                    (-0.25, 0.25, "box_chip")):
    put(box, *cell_center(7 + dx, 2 + dy), CELL * 0.5)
put("shelf_empty", *cell_center(8, 2))
put("box_plain", *cell_center(8, 2), CELL * 0.5)

# --- charger + engine glow + charging robot + status icon ---
cx, cy = cell_center(10, 2)
glow = Image.new("RGBA", (CELL * 3, CELL * 3), (0, 0, 0, 0))
gd = ImageDraw.Draw(glow)
for r in range(CELL, 0, -2):
    gd.ellipse([1.5 * CELL - r, 1.5 * CELL - r, 1.5 * CELL + r, 1.5 * CELL + r],
               fill=(64, 220, 210, int(70 * (1 - r / CELL))))
scene.alpha_composite(glow, (round(cx - 1.5 * CELL), round(cy - 1.5 * CELL)))
put("charger_on", cx, cy)
put("robot_empty_s", *cell_center(10, 3))
put("icon_bolt", cell_center(10, 3)[0], cell_center(10, 3)[1] - CELL * 0.55, CELL * 0.35)

# --- working robots: ground box = one FULL cell ---
put("robot_loaded_e", *cell_center(5, 5))
put("icon_battery_low", cell_center(5, 5)[0], cell_center(5, 5)[1] - CELL * 0.75, CELL * 0.3)
put("fx_dust_2", cell_center(5, 5)[0] + CELL * 0.4, cell_center(5, 5)[1] + CELL * 0.2, CELL * 0.45)
put("robot_empty_w", *cell_center(9, 5))
put("marker_select", *cell_center(9, 5))
put("marker_dest", *cell_center(11, 6))
put("box_water", *cell_center(10, 5))     # ground box occupies its whole cell

put("box_chip", *cell_center(3, 6))
put("floor_hazard", *cell_center(6, 7))
put("drain_grate", *cell_center(11, 3), CELL * 0.8)

scene.convert("RGB").save(os.path.join(ART, "atlas-v4-scene-test.png"))
print("scene updated:", os.path.join(ART, "atlas-v4-scene-test.png"))
