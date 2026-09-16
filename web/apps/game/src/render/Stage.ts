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
import { suppressPageSelect } from "../pageSelect";
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

// 场景层清理：destroy 释放 GPU 侧对象（纹理共享自 bank，不随销毁）。
function destroyChildren(layer: Container): void {
  for (const c of layer.children) {
    c.destroy({ children: true });
  }
  layer.removeChildren();
}

export class Stage {
  private app: Application;
  private root = new Container();
  private worldW = new Container(); // 地面 + 墙（场景静态）
  private sceneL = new Container(); // 货架/充电桩/装卸位/地面箱/车辆（快照重建）
  private robotL = new Container(); // 机器人（持久 + 插值）
  private bank: SpriteBank;
  private info: StaticInfo | null = null;
  /** 当前视图变换（世界 → 屏幕）。 */
  private view = { scale: 1, x: 0, y: 0 };
  /** 用户手动视图（滚轮 / 拖拽产生）；null = 自动适配窗口。 */
  private userView: { scale: number; x: number; y: number } | null = null;
  private panCtx: { cx: number; cy: number; vx: number; vy: number } | null = null;
  private robots = new Map<number, RobotAnim>();
  /** 世界层（worldW）当前渲染的场景 id。 */
  private scenario = "";
  /** 快照内容（sceneL/robotL）最近已绘制的场景 id。 */
  private drawnScenario = "";
  /** 最新收到的快照：场景切换竞态中被丢弃的帧，待 setStatic 补渲染。 */
  private lastSnap: Snapshot | null = null;
  private lastSel: number | null = null;
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
    stage.attachControls();
    stage.resizeOb = new ResizeObserver(() => stage.layout());
    stage.resizeOb.observe(el);
    return stage;
  }

  destroy(): void {
    this.resizeOb?.disconnect();
    // 平移中销毁（HMR 热更等）：清掉页面级禁选遗留。
    this.releaseSelect?.();
    this.releaseSelect = null;
    const canvas = this.app.canvas;
    canvas.removeEventListener("wheel", this.onWheel);
    canvas.removeEventListener("pointerdown", this.onPointerDown);
    canvas.removeEventListener("pointermove", this.onPointerMove);
    canvas.removeEventListener("pointerup", this.onPointerUp);
    canvas.removeEventListener("pointercancel", this.onPointerUp);
    canvas.removeEventListener("dblclick", this.onDblClick);
    this.app.destroy(true, { children: true });
  }

  // -- 布局与视图控制 ------------------------------------------------------

  // userView 为 null 时自动适配（letterbox 居中），ResizeObserver 与场景
  // 切换都会重新适配；用户滚轮缩放 / 拖拽平移后进入手动视图，自动适配
  // 不再覆盖，双击画布回到适配。

  private attachControls(): void {
    const canvas = this.app.canvas;
    canvas.style.cursor = "grab";
    canvas.style.touchAction = "none";
    canvas.addEventListener("wheel", this.onWheel, { passive: false });
    canvas.addEventListener("pointerdown", this.onPointerDown);
    canvas.addEventListener("pointermove", this.onPointerMove);
    canvas.addEventListener("pointerup", this.onPointerUp);
    canvas.addEventListener("pointercancel", this.onPointerUp);
    canvas.addEventListener("dblclick", this.onDblClick);
  }

  private fitScale(): number | null {
    const info = this.info;
    const el = this.app.renderer.canvas.parentElement;
    if (!info || !el || el.clientWidth === 0 || el.clientHeight === 0) {
      return null;
    }
    return (
      Math.min(el.clientWidth / (info.map_w * CELL), el.clientHeight / (info.map_h * CELL)) * 0.98
    );
  }

  private fitView(): void {
    const info = this.info;
    const el = this.app.renderer.canvas.parentElement;
    const s = this.fitScale();
    if (!info || !el || s == null) {
      return;
    }
    this.view.scale = s;
    this.view.x = (el.clientWidth - info.map_w * CELL * s) / 2;
    this.view.y = (el.clientHeight - info.map_h * CELL * s) / 2;
  }

  private applyView(): void {
    this.root.scale.set(this.view.scale);
    this.root.position.set(this.view.x, this.view.y);
  }

  private layout(): void {
    // 容器尺寸变化（分栏拖拽 / 窗口缩放）不经 window resize 触发
    // ResizePlugin，backing store 需在此显式同步（容器隐藏时跳过）。
    const el = this.app.renderer.canvas.parentElement;
    if (el && el.clientWidth > 0 && el.clientHeight > 0) {
      this.app.resize();
    }
    if (!this.info) {
      return;
    }
    if (!this.userView) {
      this.fitView();
    }
    this.applyView();
  }

  private onWheel = (e: WheelEvent): void => {
    const fit = this.fitScale();
    if (fit == null) {
      return;
    }
    e.preventDefault();
    const rect = this.app.canvas.getBoundingClientRect();
    const px = e.clientX - rect.left;
    const py = e.clientY - rect.top;
    // Firefox 行模式按 ~16px/行 归一；pinch（ctrl+wheel）事件 delta 小，提速。
    const dy = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY;
    const speed = e.ctrlKey ? 0.008 : 0.0015;
    const next = Math.min(fit * 8, Math.max(fit * 0.25, this.view.scale * Math.exp(-dy * speed)));
    // 以光标为锚点：缩放前后光标下的世界坐标保持不动。
    const wx = (px - this.view.x) / this.view.scale;
    const wy = (py - this.view.y) / this.view.scale;
    this.view.scale = next;
    this.view.x = px - wx * next;
    this.view.y = py - wy * next;
    // 拖拽进行中缩放：以缩放后的视图重定平移基座，后续 move 不撤销缩放。
    if (this.panCtx) {
      this.panCtx.vx = this.view.x - (e.clientX - this.panCtx.cx);
      this.panCtx.vy = this.view.y - (e.clientY - this.panCtx.cy);
    }
    this.userView = { ...this.view };
    this.applyView();
  };

  /** 页面级文本选择抑制句柄（pageSelect.ts）：平移期间置位，up/销毁释放。 */
  private releaseSelect: (() => void) | null = null;

  private onPointerDown = (e: PointerEvent): void => {
    if (e.button !== 0 || !e.isPrimary) {
      return;
    }
    // 平移期间掐断原生拖拽选择（WebKit 不受指针捕获约束，Splitter 同款
    // 问题）；pointerdown 的 preventDefault 不影响双击适配（click 系事件
    // 独立于兼容鼠标事件派发），画布也无需点击聚焦。
    e.preventDefault();
    this.releaseSelect ??= suppressPageSelect();
    this.panCtx = { cx: e.clientX, cy: e.clientY, vx: this.view.x, vy: this.view.y };
    this.app.canvas.setPointerCapture(e.pointerId);
    this.app.canvas.style.cursor = "grabbing";
  };

  private onPointerMove = (e: PointerEvent): void => {
    if (!this.panCtx || !e.isPrimary) {
      return;
    }
    this.view.x = this.panCtx.vx + (e.clientX - this.panCtx.cx);
    this.view.y = this.panCtx.vy + (e.clientY - this.panCtx.cy);
    this.userView = { ...this.view };
    this.applyView();
  };

  private onPointerUp = (e: PointerEvent): void => {
    if (!this.panCtx || !e.isPrimary) {
      return;
    }
    this.panCtx = null;
    this.releaseSelect?.();
    this.releaseSelect = null;
    if (this.app.canvas.hasPointerCapture(e.pointerId)) {
      this.app.canvas.releasePointerCapture(e.pointerId);
    }
    this.app.canvas.style.cursor = "grab";
  };

  private onDblClick = (): void => {
    this.userView = null;
    this.fitView();
    this.applyView();
  };

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

  /** 两格足迹（1×2）的精灵：竖向占 1 宽 2 高，横向占 2 宽 1 高。 */
  private spanSprite(id: string, x: number, y: number, ext: [number, number]): Sprite {
    const sp = this.sprite(id, x, y);
    if (ext[0] !== 0) {
      sp.width = CELL * 2;
      sp.height = CELL;
    } else {
      sp.width = CELL;
      sp.height = CELL * 2;
    }
    return sp;
  }

  // -- 静态层（地面 + 墙瓦；场景切换时重建） --------------------------------

  setStatic(info: StaticInfo | null): void {
    this.info = info;
    if (!info) {
      return;
    }
    if (info.id !== this.scenario) {
      this.scenario = info.id;
      this.userView = null; // 新地图尺寸未知，视图回到自动适配
      this.panCtx = null; // 拖拽中切场景：陈旧基座会撤销适配
      destroyChildren(this.worldW);
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

      // 墙瓦贴边直铺：缺口 = 该格不铺（装卸位锚点格已从 walls 剔除）。
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
    }
    this.layout();
    // 切场景（含切回旧场景）时，新场景快照先于静态信息到达会被
    // render() 的场景比对丢弃；暂停状态下没有后续 tick 产生新帧，
    // 不补渲染画布将冻结在旧场景精灵上。此处用缓存的最新快照补绘。
    if (this.lastSnap && this.lastSnap.scenario === info.id && this.drawnScenario !== info.id) {
      this.drawnScenario = info.id;
      this.sceneL.sortableChildren = true;
      this.rebuildScene(this.lastSnap, this.lastSel);
      this.updateRobots(this.lastSnap);
    }
  }

  // -- 快照渲染 ------------------------------------------------------------

  render(snap: Snapshot, selected: number | null): void {
    // 缓存最新快照：静态信息未就绪 / 场景切换竞态时被丢弃的帧，
    // 由 setStatic 在场景信息就绪后补渲染。
    this.lastSnap = snap;
    this.lastSel = selected;
    if (!this.info || snap.scenario !== this.info.id) {
      return; // 静态信息未就绪 / 场景切换竞态：等下一次 setStatic
    }
    this.drawnScenario = snap.scenario;
    this.sceneL.sortableChildren = true; // 选中框置顶
    this.rebuildScene(snap, selected);
    this.updateRobots(snap);
  }

  /** 货架叠箱 / 充电桩 / 装卸位 / 地面箱 / 车辆（1×2 足迹）：快照级重建。 */
  private rebuildScene(snap: Snapshot, selected: number | null): void {
    destroyChildren(this.sceneL);

    // 装卸位（1×2 足迹 = 缺口锚点格 + ext 第二格；占用态切换精灵）。
    for (const d of snap.docks) {
      const occupied = d.docked_vehicle != null;
      const id =
        d.ext[0] !== 0
          ? occupied
            ? "port_h_occupied"
            : "port_h_empty"
          : occupied
            ? "port_v_occupied"
            : "port_v_empty";
      const [cx, cy] = this.spanCenter(d.x, d.y, d.ext);
      this.sceneL.addChild(this.spanSprite(id, cx, cy, d.ext));
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

    // 车辆：跨停靠装卸位的两格（锚点 + ext）；车斗叠箱堆在锚点格（靠墙格）。
    for (const v of snap.vehicles) {
      const d = snap.docks.find((q) => q.id === v.dock);
      if (!d) {
        // 协议断裂（车辆指向不存在的装卸位）宁炸勿瞒：错误兜底会把卡车
        // 画到 interact_pos 上无声错位一格。
        throw new Error(`车辆 ${v.id} 指向不存在的装卸位 #${v.dock}`);
      }
      if (d.ext[0] !== 0) {
        // 横向装卸位只有 port_h_* 素材，卡车纵版被 spanSprite 拉成 2×1 会形变；
        // docs/game-design/02 首期只用纵向，补横向素材时一并放开。
        throw new Error(`横向装卸位 #${d.id} 缺少卡车素材（truck_e/w 未提供）`);
      }
      const ext = d.ext;
      const [cx, cy] = this.spanCenter(d.x, d.y, ext);
      this.sceneL.addChild(this.spanSprite("truck_s_empty", cx, cy, ext));
      const [bx, by] = this.cellCenter(d.x, d.y);
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
    for (const id of this.robots.keys()) {
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
