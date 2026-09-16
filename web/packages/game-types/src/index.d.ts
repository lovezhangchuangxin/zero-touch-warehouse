// 玩家侧 Game API 的 TypeScript 类型声明。
//
// 权威来源（改动必须两处同步）：
//   - crates/api/bindings/bootstrap.js —— 绑定层唯一实现；
//   - crates/model/src/lib.rs codes::ALL —— 结果码全表；
//   - docs/game-design/08-api-design.md —— 字段与命名。
//
// B2 的代码编辑器为纯 textarea，尚未消费本包；它作为后续 Monaco /
// CodeMirror 选型（docs/architecture/05 待定项）的既定归属地先落地，
// 应用侧面板用同一份类型描述对齐字段名。

// ---------------------------------------------------------------------------
// 结果码（与 ztw_model::codes::ALL 一一对应）
// ---------------------------------------------------------------------------

export type GameCode =
  | "OK"
  | "ARRIVED"
  | "ALREADY_ACTED"
  | "INVALID_ARGUMENT"
  | "NOT_ADJACENT"
  | "LOADED"
  | "NOT_CARRYING"
  | "NOT_ENOUGH_ENERGY"
  | "TARGET_FULL"
  | "BOX_NOT_FOUND"
  | "WRONG_GOODS"
  | "INVALID_TARGET"
  | "OUT_OF_BOUNDS"
  | "CELL_BLOCKED"
  | "CELL_OCCUPIED"
  | "NO_SUCH_OBJECT"
  | "NO_PATH"
  | "CELL_CONTESTED"
  | "TARGET_CONTESTED"
  | "CHAIN_BLOCKED"
  | "CHARGER_BUSY"
  | "TARGET_MOVED"
  | "TARGET_GONE"
  | "NO_FUNDS"
  | "CREDIT_EXCEEDED"
  | "ON_VEHICLE"
  | "NO_FREE_DOCK"
  | "NOT_EMPTY"
  | "HAS_VEHICLE"
  | "ORDER_GONE"
  | "GOODS_MOVED"
  | "NOT_ON_WALL"
  | "INIT_PHASE";

export interface GameCodes {
  OK: "OK";
  ARRIVED: "ARRIVED";
  ALREADY_ACTED: "ALREADY_ACTED";
  INVALID_ARGUMENT: "INVALID_ARGUMENT";
  NOT_ADJACENT: "NOT_ADJACENT";
  LOADED: "LOADED";
  NOT_CARRYING: "NOT_CARRYING";
  NOT_ENOUGH_ENERGY: "NOT_ENOUGH_ENERGY";
  TARGET_FULL: "TARGET_FULL";
  BOX_NOT_FOUND: "BOX_NOT_FOUND";
  WRONG_GOODS: "WRONG_GOODS";
  INVALID_TARGET: "INVALID_TARGET";
  OUT_OF_BOUNDS: "OUT_OF_BOUNDS";
  CELL_BLOCKED: "CELL_BLOCKED";
  CELL_OCCUPIED: "CELL_OCCUPIED";
  NO_SUCH_OBJECT: "NO_SUCH_OBJECT";
  NO_PATH: "NO_PATH";
  CELL_CONTESTED: "CELL_CONTESTED";
  TARGET_CONTESTED: "TARGET_CONTESTED";
  CHAIN_BLOCKED: "CHAIN_BLOCKED";
  CHARGER_BUSY: "CHARGER_BUSY";
  TARGET_MOVED: "TARGET_MOVED";
  TARGET_GONE: "TARGET_GONE";
  NO_FUNDS: "NO_FUNDS";
  CREDIT_EXCEEDED: "CREDIT_EXCEEDED";
  ON_VEHICLE: "ON_VEHICLE";
  NO_FREE_DOCK: "NO_FREE_DOCK";
  NOT_EMPTY: "NOT_EMPTY";
  HAS_VEHICLE: "HAS_VEHICLE";
  ORDER_GONE: "ORDER_GONE";
  GOODS_MOVED: "GOODS_MOVED";
  NOT_ON_WALL: "NOT_ON_WALL";
  INIT_PHASE: "INIT_PHASE";
  [key: string]: GameCode;
}

// ---------------------------------------------------------------------------
// 值对象
// ---------------------------------------------------------------------------

/** 坐标（原点左上，x 向右，y 向下；下标 + 展开两用）。 */
export interface Position extends Array<number> {
  readonly x: number;
  readonly y: number;
  equals(other: PositionLike): boolean;
  adjacent(other: PositionLike): boolean;
  distance(other: PositionLike): number;
  neighbors(): Position[];
  toString(): string;
}

export type PositionLike = Position | number[] | { x: number; y: number };

/** 方向常量类型。 */
export type Direction = Position;

/** find_path / move_to 的坐标宽进形态（对象取坐标规则见 docs/game-design/08「寻路」）。 */
export type PathTargetLike =
  | Position
  | number[]
  | { x: number; y: number }
  | RobotView
  | ShelfView
  | ChargerView
  | DockView
  | VehicleView
  | BoxView;

/** find_path 的 opts。 */
export interface PathOpts {
  /** 到达判定半径（正交距离），默认 find_path 0 / move_to 1。 */
  range?: number;
}

// ---------------------------------------------------------------------------
// 对象视图（查询返回；动作方法立即返回受理码，结算结果下一 tick 查 last_result）
// ---------------------------------------------------------------------------

export interface BoxView {
  id: number;
  goods_type: string;
  holder: number | null;
  location: Position;
}

export interface LastResult {
  action: string;
  arg: string;
  code: GameCode;
}

export interface RobotView {
  id: number;
  pos: Position;
  energy: number;
  energy_max: number;
  carry: BoxView | null;
  /** 上一 tick 受理通过动作的结算结果；未受理为 null（docs/game-design/03）。 */
  last_result: LastResult | null;
  /** 机器人命名空间下的受控 memory（映射代理）。 */
  readonly memory: MemoryMap;
  move(dir: Direction | PositionLike): GameCode;
  charge(): GameCode;
  take(target: TargetLike, boxId: number): GameCode;
  give(target: TargetLike, boxId?: number): GameCode;
  pick(x: number, y: number): GameCode;
  drop(x: number, y: number, boxId?: number): GameCode;
  /** 复合移动：寻路并自动提交一步（docs/game-design/08「move_to 与 robot.memory」）。 */
  move_to(target: PathTargetLike, opts?: PathOpts): GameCode;
  move_to(x: number, y: number, opts?: PathOpts): GameCode;
}

export interface ShelfView {
  id: number;
  pos: Position;
  boxes: BoxView[];
  capacity: number;
}

export interface ChargerView {
  id: number;
  pos: Position;
}

export interface DockView {
  id: number;
  pos: Position;
  /** 指向库内第二格的单位偏移（[0, 1] = 北墙缺口向库内伸出），占地 1×2、两格均为障碍。 */
  ext: Position;
  docked_vehicle: number | null;
}

export interface VehicleView {
  id: number;
  kind: "in" | "out";
  goods_type: string;
  interact_pos: Position;
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
  /** 金币（显示值；权威为千分定点整数，见 docs/architecture/02）。 */
  unit_price: number;
  vehicle: number | null;
  dock: number | null;
}

export type TargetLike = RobotView | ShelfView | VehicleView | number;

// ---------------------------------------------------------------------------
// 受控 memory 代理（主进程权威数据树；docs/game-design/08）
// ---------------------------------------------------------------------------

export type MemValue = null | boolean | number | string | MemValue[] | MemoryMap;

// 方法与数据键共存：索引签名放宽到包含函数（受控 memory 的数据键
// 仍是 MemValue；keys/to_dict 等方法由宿主代理提供）。
export interface MemoryMap {
  [key: string]: MemValue | ((...args: never[]) => unknown);
  keys(): string[];
  readonly size: number;
  to_dict(): Record<string, unknown>;
}

export interface MemoryList {
  readonly length: number;
  [index: number]: MemValue;
  push(...items: MemValue[]): number;
  remove(index: number): void;
  to_list(): unknown[];
  [Symbol.iterator](): Iterator<MemValue>;
}

// ---------------------------------------------------------------------------
// Game 全局
// ---------------------------------------------------------------------------

export interface Market {
  sell_orders(): OrderView[];
  buy_orders(): OrderView[];
  take(orderId: number): GameCode;
  cancel(orderId: number): GameCode;
}

export interface Game {
  E: GameCodes;
  NORTH: Direction;
  SOUTH: Direction;
  WEST: Direction;
  EAST: Direction;
  robots(): RobotView[];
  shelves(): ShelfView[];
  chargers(): ChargerView[];
  docks(): DockView[];
  vehicles(kind?: "in" | "out"): VehicleView[];
  ground_boxes(): BoxView[];
  my_orders(): OrderView[];
  objects_at(
    x: number,
    y: number,
  ): Array<RobotView | ShelfView | ChargerView | DockView | VehicleView | BoxView>;
  get_object_by_id(
    id: number,
  ): RobotView | ShelfView | ChargerView | DockView | VehicleView | BoxView | OrderView | null;
  map_size(): [number, number];
  /**
   * 静态障碍最短路径（docs/game-design/08「寻路」），忠实三分支：
   * Position[] 路径（不含 start）/ []（已在到达范围）/ null（不可达）。
   */
  find_path(start: PathTargetLike, goal: PathTargetLike, opts?: PathOpts): Position[] | null;
  readonly gold: number;
  readonly debt: number;
  readonly tick: number;
  readonly market: Market;
  /** 管理操作（即时生效）。buy 的装卸位朝向由锚点边界墙唯一推导。 */
  buy(kind: "robot" | "shelf" | "charger" | "dock", x: number, y: number): GameCode;
  borrow(amount: number): GameCode;
  repay(amount: number): GameCode;
  destroy(id: number | TargetLike): GameCode;
  log(...args: unknown[]): void;
  /** 受控根 memory（映射代理）。 */
  readonly memory: MemoryMap;
}
