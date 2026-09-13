// 精灵资产加载（docs/art/atlas-v4.md：切片画布 = 逻辑足迹，锚点 = 画布
// 中心 = 格子中心；墙瓦贴边直铺由切片烘进瓦片，居中摆放即可）。
// 本模块只做加载与静态映射，渲染节奏在 Stage。

import { Assets, Texture } from "pixi.js";

interface ManifestEntry {
  id: string;
  file: string;
  canvas: [number, number];
}

interface Manifest {
  sprites: ManifestEntry[];
}

/** B2 实际用到的精灵子集（其余 fx/图标留待后续里程碑）。 */
const USED_PREFIXES = ["robot_", "box_", "shelf_", "charger_", "port_", "truck_", "wall_"];
const USED_IDS = new Set(["marker_select", "icon_battery_low"]);

export type SpriteBank = Map<string, Texture>;

export async function loadSprites(base = "/sprites"): Promise<SpriteBank> {
  const manifest = (await (await fetch(`${base}/manifest.json`)).json()) as Manifest;
  const bank: SpriteBank = new Map();
  const wanted = manifest.sprites.filter(
    (s) => USED_PREFIXES.some((p) => s.id.startsWith(p)) || USED_IDS.has(s.id),
  );
  await Promise.all(
    wanted.map(async (s) => {
      bank.set(s.id, (await Assets.load(`${base}/${s.file}`)) as Texture);
    }),
  );
  if (bank.size < 40) {
    throw new Error(`精灵加载不完整：仅 ${bank.size} 张（请先运行 pnpm sync:sprites）`);
  }
  return bank;
}

/** 货物类型 → 箱子精灵 id（未知类型退普通箱）。 */
export function boxSprite(goodsType: string): string {
  const known = ["water", "battery", "chip"];
  return known.includes(goodsType) ? `box_${goodsType}` : "box_plain";
}

/** 墙瓦变体：按格坐标确定性选取（同位置不闪烁，跨快照稳定）。 */
export function wallTile(side: "n" | "s" | "w" | "e", x: number, y: number): string {
  const v = (x * 73856093 + y * 19349663) % 5;
  return `wall_tile_${side}${v + 1}`;
}

export type Corner = "nw" | "ne" | "sw" | "se";

/** 周边墙格分类：四角 / 四边（staticInfo.walls 已剔除装卸位缺口）。 */
export function wallKind(
  x: number,
  y: number,
  w: number,
  h: number,
): Corner | "n" | "s" | "w" | "e" | null {
  const north = y === 0;
  const south = y === h - 1;
  const west = x === 0;
  const east = x === w - 1;
  if (north && west) return "nw";
  if (north && east) return "ne";
  if (south && west) return "sw";
  if (south && east) return "se";
  if (north) return "n";
  if (south) return "s";
  if (west) return "w";
  if (east) return "e";
  return null;
}
