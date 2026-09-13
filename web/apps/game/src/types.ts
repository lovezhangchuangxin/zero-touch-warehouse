// 与 crates/desktop 侧 serde 输出逐字段对应的线协议类型：
// 经济值为十进制字符串（防 JSON 浮点精度，docs/architecture/02 数学规则），
// 坐标为 [x, y] 数组，Option 一律映射 null。

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
