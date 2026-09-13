// Pixi 8 画布渲染（docs/architecture/05 §渲染节奏）：
// - 逻辑 tick 与 rAF 完全分离：世界线程按模拟速率推进，本层按显示器
//   帧率渲染（app.ticker）。
// - 相邻快照之间对机器人位移插值；暂停 / 单步直显精确逻辑位置——
//   调试时所见即模拟状态。
// - 快进时快照已被 ack 门控合并，本层取最新渲染（插值时长随 tps 收窄，
//   高速下等效直显）。
// 本类的 Pixi 对象不进入 Vue 响应式系统（GameCanvas 只做挂载与桥接）。

import { Application, Container, Graphics, Sprite, type Texture } from "pixi.js";
import { boxSprite, loadSprites, wallKind, wallTile, type SpriteBank } from "./sprites";
import type { Snapshot, StaticInfo } from "../types";

const CELL = 48; // 屏幕像素 / 格（缩放自适应容器）
const BLINK_PERIOD = 3200;
const BLINK_MS = 160;
const LOW_ENERGY = 0.3;

interface RobotAnim {
  container: Container;
  body: Sprite;
  badge: Sprite;
  dir: "s" | "n" | "e" | "w";
  fromX: number;
  fromY: number;
  toX: number;
  toY: number;
  /** 目标格坐标（方向判定用）。 */
  toGX: number;
  toGY: number;
  t0: number;
  dur: number;
}

export class Stage {
  private app: Application;
  private root = new Container();
  private worldW = new Container(); // 地面 + 墙（场景静态）
  private sceneL = new Container(); // 货架/充电桩/装卸口/地面箱/车辆（快照重建）
  private robotL = new Container(); // 机器人（持久 + 插值）
  private bank: SpriteBank;
  private info: StaticInfo | null = null;
  private robots = new Map<number, RobotAnim>();
  private scenario = "";
  private resizeOb: ResizeObserver | null = null;

  private constructor(app: Application, bank: SpriteBank) {
    this.app = app;
    this.bank = bank;
    this.root.addChild(this.worldW, this.sceneL, this.robotL);
    app.stage.addChild(this.root);
    app.ticker.add(() => this.tick());
  }

  static async create(el: HTMLElement): Promise<Stage> {
    const app = new Application();
    await app.init({
      background: 0x101216,
      resizeTo: el,
      antialias: false,
      autoDensity: true,
      resolution: Math.min(window.devicePixelRatio || 1, 2),
    });
    el.appendChild(app.canvas);
    const bank = await loadSprites();
    const stage = new Stage(app, bank);
    stage.resizeOb = new ResizeObserver(() => stage.layout());
    stage.resizeOb.observe(el);
    return stage;
  }

  destroy(): void {
    this.resizeOb?.disconnect();
    this.app.destroy(true, { children: true });
  }

  // -- 布局 --------------------------------------------------------------

  private layout(): void {
    const info = this.info;
    if (!info) {
      return;
    }
    const el = this.app.renderer.canvas.parentElement;
    if (!el) {
      return;
    }
    const w = info.map_w * CELL;
    const h = info.map_h * CELL;
    const s = Math.min(el.clientWidth / w, el.clientHeight / h) * 0.98;
    this.root.scale.set(s);
    this.root.position.set((el.clientWidth - w * s) / 2, (el.clientHeight - h * s) / 2);
  }

  private cellCenter(x: number, y: number): [number, number] {
    return [(x + 0.5) * CELL, (y + 0.5) * CELL];
  }

  /** 两格足迹（1×2）的中点：锚点格 + 伸出格。 */
  private spanCenter(x: number, y: number, ext: [number, number]): [number, number] {
    return [(x + (x + ext[0]) + 1) * (CELL / 2), (y + (y + ext[1]) + 1) * (CELL / 2)];
  }

  private tex(id: string): Texture {
    const t = this.bank.get(id);
    if (!t) {
      throw new Error(`精灵缺失：${id}`);
    }
    return t;
  }

  private sprite(id: string, x: number, y: number): Sprite {
    const sp = new Sprite(this.tex(id));
    sp.anchor.set(0.5);
    sp.position.set(x, y);
    sp.width = CELL;
    sp.height = CELL;
    return sp;
  }

  // -- 静态层（地面 + 墙瓦；场景切换时重建） --------------------------------

  setStatic(info: StaticInfo | null): void {
    this.info = info;
    if (!info) {
      return;
    }
    if (info.id === this.scenario) {
      this.layout();
      return;
    }
    this.scenario = info.id;
    this.worldW.removeChildren();
    const w = info.map_w * CELL;
    const h = info.map_h * CELL;

    // 地面由引擎绘制（纯色 + 网格线，docs/art/atlas-v4：地砖精灵已弃用）。
    const floor = new Graphics().rect(0, 0, w, h).fill(0x2a2f37);
    const lines = new Graphics();
    for (let x = 0; x <= info.map_w; x++) {
      lines.moveTo(x * CELL, 0).lineTo(x * CELL, h);
    }
    for (let y = 0; y <= info.map_h; y++) {
      lines.moveTo(0, y * CELL).lineTo(w, y * CELL);
    }
    lines.stroke({ width: 1, color: 0x323743, alpha: 0.55 });
    this.worldW.addChild(floor, lines);

    // 墙瓦贴边直铺：门洞 = 该格不铺（装卸口足迹已从 walls 剔除）。
    for (const [x, y] of info.walls) {
      const kind = wallKind(x, y, info.map_w, info.map_h);
      if (!kind) {
        continue;
      }
      const id =
        kind.length === 2 ? `wall_corner_${kind}` : wallTile(kind as "n" | "s" | "w" | "e", x, y);
      const [cx, cy] = this.cellCenter(x, y);
      this.worldW.addChild(this.sprite(id, cx, cy));
    }
    this.layout();
  }

  // -- 快照渲染 ------------------------------------------------------------

  render(snap: Snapshot, selected: number | null): void {
    if (!this.info || snap.scenario !== this.info.id) {
      return; // 静态信息未就绪 / 场景切换竞态：等下一次 setStatic
    }
    this.sceneL.sortableChildren = true; // 选中框置顶
    this.rebuildScene(snap, selected);
    this.updateRobots(snap);
  }

  /** 货架叠箱 / 充电桩 / 装卸口 / 地面箱 / 车辆（1×2 渲染足迹）：快照级重建。 */
  private rebuildScene(snap: Snapshot, selected: number | null): void {
    this.sceneL.removeChildren();
    const info = this.info!;
    const portById = new Map(snap.ports.map((p) => [p.id, p]));

    // 装卸口（竖 1×2，第二格按场景 ext 伸出——纯渲染，逻辑仍 1×1 锚点）。
    for (const p of snap.ports) {
      const ext: [number, number] = info.ports.find((q) => q.id === p.id)?.ext ?? [0, 1];
      const id = p.docked_vehicle != null ? "port_v_occupied" : "port_v_empty";
      const [cx, cy] = this.spanCenter(p.x, p.y, ext);
      this.sceneL.addChild(this.sprite(id, cx, cy));
    }

    // 充电桩：相邻机器人上一 tick charge 成功时显示工作态。
    const charging = new Set<number>();
    for (const r of snap.robots) {
      if (r.last_result?.action === "charge" && r.last_result.code === "OK") {
        charging.add(r.id);
      }
    }
    for (const c of snap.chargers) {
      const on = snap.robots.some(
        (r) => charging.has(r.id) && Math.abs(r.x - c.x) + Math.abs(r.y - c.y) === 1,
      );
      const [cx, cy] = this.cellCenter(c.x, c.y);
      this.sceneL.addChild(this.sprite(on ? "charger_on" : "charger_idle", cx, cy));
    }

    // 货架：单层底座 + 固定槽位叠箱（容量 4，从底往上）。
    for (const s of snap.shelves) {
      const [cx, cy] = this.cellCenter(s.x, s.y);
      this.sceneL.addChild(this.sprite("shelf_empty", cx, cy));
      s.boxes.slice(0, s.capacity).forEach((b, i) => {
        const jx = ((b.id * 37) % 9) - 4;
        const sp = this.sprite(boxSprite(b.goods_type), cx + jx, cy + 10 - i * 13);
        sp.width = CELL * 0.72;
        sp.height = CELL * 0.72;
        this.sceneL.addChild(sp);
      });
    }

    // 地面箱（每箱占一格）。
    for (const b of snap.ground_boxes) {
      const [cx, cy] = this.cellCenter(b.x, b.y);
      this.sceneL.addChild(this.sprite(boxSprite(b.goods_type), cx, cy - 2));
    }

    // 车辆：以装卸口锚点格 + ext 伸出第二格；车斗叠箱堆在北格。
    for (const v of snap.vehicles) {
      const port = [...portById.values()].find((p) => p.x === v.x && p.y === v.y);
      const ext: [number, number] =
        port?.id != null ? (info.ports.find((q) => q.id === port!.id)?.ext ?? [0, 1]) : [0, 1];
      const [cx, cy] = this.spanCenter(v.x, v.y, ext);
      this.sceneL.addChild(this.sprite("truck_s_empty", cx, cy));
      const northY = Math.min(v.y, v.y + ext[1]);
      const [bx, by] = this.cellCenter(v.x, northY);
      v.boxes.forEach((b, i) => {
        const jx = ((b.id * 53) % 7) - 3;
        const sp = this.sprite(boxSprite(b.goods_type), bx + jx, by + 6 - i * 12);
        sp.width = CELL * 0.66;
        sp.height = CELL * 0.66;
        this.sceneL.addChild(sp);
      });
    }

    // 选中框（画布点击交互属后续里程碑；这里跟随面板选中）。
    if (selected != null) {
      const r = snap.robots.find((x) => x.id === selected);
      if (r) {
        const [cx, cy] = this.cellCenter(r.x, r.y);
        const sel = this.sprite("marker_select", cx, cy);
        sel.zIndex = 10;
        this.sceneL.addChild(sel);
      }
    }
  }

  // -- 机器人（持久精灵 + 相邻快照插值） ------------------------------------

  private updateRobots(snap: Snapshot): void {
    const seen = new Set<number>();
    const paused = !snap.running;
    const dur = Math.max(60, Math.min(400, 1000 / Math.max(1, snap.tps)));
    for (const r of snap.robots) {
      seen.add(r.id);
      let anim = this.robots.get(r.id);
      const [cx, cy] = this.cellCenter(r.x, r.y);
      if (!anim) {
        const body = new Sprite(this.tex("robot_empty_s"));
        body.anchor.set(0.5);
        body.width = CELL;
        body.height = CELL;
        const badge = new Sprite(this.tex("icon_battery_low"));
        badge.anchor.set(0.5);
        badge.width = CELL * 0.4;
        badge.height = CELL * 0.4;
        badge.visible = false;
        const container = new Container();
        container.addChild(body, badge);
        this.robotL.addChild(container);
        anim = {
          container,
          body,
          badge,
          dir: "s",
          fromX: cx,
          fromY: cy,
          toX: cx,
          toY: cy,
          toGX: r.x,
          toGY: r.y,
          t0: performance.now(),
          dur,
        };
        this.robots.set(r.id, anim);
      }
      // 方向由相邻快照位移推断（无位移保持原朝向）。
      if (r.x !== anim.toGX || r.y !== anim.toGY) {
        anim.dir =
          r.x > anim.toGX
            ? "e"
            : r.x < anim.toGX
              ? "w"
              : r.y > anim.toGY
                ? "s"
                : r.y < anim.toGY
                  ? "n"
                  : anim.dir;
        anim.fromX = anim.container.x;
        anim.fromY = anim.container.y;
        anim.toX = cx;
        anim.toY = cy;
        anim.toGX = r.x;
        anim.toGY = r.y;
        anim.t0 = performance.now();
        anim.dur = paused ? 0 : dur;
      } else if (paused) {
        anim.fromX = cx;
        anim.fromY = cy;
        anim.toX = cx;
        anim.toY = cy;
      }
      anim.container.zIndex = 20;
      anim.badge.position.set(CELL * 0.32, -CELL * 0.38);
      anim.badge.visible = r.energy / r.energy_max < LOW_ENERGY;
      anim.body.texture = this.tex(`robot_${r.carry ? "loaded" : "empty"}_${anim.dir}`);
      if (paused) {
        this.place(anim, 1);
      }
    }
    for (const id of [...this.robots.keys()]) {
      if (!seen.has(id)) {
        this.robots.get(id)!.container.destroy({ children: true });
        this.robots.delete(id);
      }
    }
  }

  private place(a: RobotAnim, p: number): void {
    a.container.position.set(a.fromX + (a.toX - a.fromX) * p, a.fromY + (a.toY - a.fromY) * p);
  }

  /** rAF：插值推进 + 眨眼帧（暂停时由 updateRobots 直显，本层幂等）。 */
  private tick(): void {
    const now = performance.now();
    for (const a of this.robots.values()) {
      if (a.dur > 0) {
        const p = Math.min(1, (now - a.t0) / a.dur);
        this.place(a, p);
      }
      const blink = now % BLINK_PERIOD < BLINK_MS;
      if (blink && a.body.texture === this.tex(`robot_empty_${a.dir}`)) {
        a.body.texture = this.tex(`robot_blink_${a.dir}`);
      } else if (!blink && a.body.texture === this.tex(`robot_blink_${a.dir}`)) {
        a.body.texture = this.tex(`robot_empty_${a.dir}`);
      }
    }
  }
}
