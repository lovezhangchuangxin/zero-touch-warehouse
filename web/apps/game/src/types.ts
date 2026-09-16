// 与 crates/desktop 侧 serde 输出逐字段对应的线协议类型：
// 经济值为十进制字符串（防 JSON 浮点精度，docs/architecture/02 数学规则），
// 坐标为 [x, y] 数组，Option 一律映射 null。

import type { Language } from "./scripts";

export interface BoxView {
  id: number;
  goods_type: string;
  holder: number | null;
  x: number;
  y: number;
}

export interface LastResult {
  action: string;
  arg: string;
  code: string;
}

export interface RobotView {
  id: number;
  x: number;
  y: number;
  energy: number;
  energy_max: number;
  carry: BoxView | null;
  last_result: LastResult | null;
}

export interface ShelfView {
  id: number;
  x: number;
  y: number;
  boxes: BoxView[];
  capacity: number;
}

export interface ChargerView {
  id: number;
  x: number;
  y: number;
}

export interface DockView {
  id: number;
  x: number;
  y: number;
  /** 指向库内第二格的单位偏移（[0,1] = 北墙缺口锚点向库内伸出）。 */
  ext: [number, number];
  docked_vehicle: number | null;
}

export interface VehicleView {
  id: number;
  kind: "in" | "out";
  goods_type: string;
  x: number;
  y: number;
  order_id: number;
  /** 所停靠装卸位 id。 */
  dock: number;
  boxes: BoxView[];
}

export interface OrderView {
  id: number;
  side: "sell" | "buy";
  goods_type: string;
  qty: number;
  unit_price_milli: string;
  vehicle?: number | null;
  dock?: number | null;
}

export interface FaultView {
  class: string;
  code: string;
  message: string;
  stack: string;
  tick: number;
  last_request_id: number;
  last_op: string;
  requests_served: number;
}

/** 渲染快照：MirrorView 扁平化 + 控制面（world_thread::publish）。 */
export interface Snapshot {
  tick: number;
  revision: number;
  gold_milli: string;
  debt_milli: string;
  map_w: number;
  map_h: number;
  blocked: [number, number][];
  robots: RobotView[];
  shelves: ShelfView[];
  chargers: ChargerView[];
  docks: DockView[];
  vehicles: VehicleView[];
  ground_boxes: BoxView[];
  sell_orders: OrderView[];
  buy_orders: OrderView[];
  my_orders: OrderView[];
  scenario: string;
  running: boolean;
  tps: number;
  loaded: boolean;
  fault: FaultView | null;
}

export interface StatusView {
  scenario: string;
  running: boolean;
  tps: number;
  loaded: boolean;
  fault_class: string | null;
  tick: number;
  /** 当前玩家代码语言（"js" / "py"）。 */
  language: string;
}

/** 场景静态信息里的装卸位（与快照 docks 字段一致；渲染以快照为准）。 */
export interface DockExt {
  id: number;
  x: number;
  y: number;
  ext: [number, number];
}

export interface StaticInfo {
  id: string;
  name: string;
  desc: string;
  map_w: number;
  map_h: number;
  walls: [number, number][];
  docks: DockExt[];
  scenarios: { id: string; name: string; desc: string; robots: number }[];
}

export interface DiagEvent {
  seq: number;
  tick: number;
  kind: string;
  payload: Record<string, unknown>;
}

export interface DiagPage {
  events: DiagEvent[];
  next: number;
  gap: { from: number; to: number } | null;
}

// ---------------------------------------------------------------------------
// 存档与设置（C1，与 crates/desktop/src/saves.rs、main.rs 命令输出对应）
// ---------------------------------------------------------------------------

/** 存档摘要（list_saves / save_game 回执）。毫秒时间戳为字符串：经 Tauri
 *  IPC 会过 JS Number，超 2^53 丢精度（docs/architecture/06 §文件格式）。 */
export interface SaveSummary {
  /** 相对存档根的稳定 id（正斜杠分隔）。 */
  id: string;
  scenario: string;
  name: string | null;
  auto: boolean;
  tick: number;
  created_at_ms: string;
  language: string;
  /** false = 未过版本 / 校验和门禁（灰条展示，error 给原因）。 */
  ok: boolean;
  error: string | null;
}

/** 编辑器草稿段（存档内与 drafts.json 防丢文件共用同一形状；与已加载
 *  程序分开保存，不冒充正在运行的代码）。 */
export interface DraftsPayload {
  /** 双语言文件集草稿（文件名 → 源码；入口文件随语言固定）。 */
  files: Record<Language, Record<string, string>>;
  active: Language;
}

/** 本机设置（settings.json；不入 Cloud 同步集）。 */
export interface SettingsPayload {
  lang: Language;
  layout: { edW: number; drH: number } | null;
}
