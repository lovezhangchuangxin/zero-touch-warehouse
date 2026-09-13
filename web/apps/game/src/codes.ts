// 结果码 → 中文说明（docs/game-design/03 受理与结算共用同一词表；
// 清单权威来源 ztw_model::codes::ALL，与 @ztw/game-types 的 GameCode 同步）。

export const CODE_LABELS: Record<string, string> = {
  OK: "成功",
  ARRIVED: "已到场",
  ALREADY_ACTED: "本 tick 已行动",
  INVALID_ARGUMENT: "参数非法",
  NOT_ADJACENT: "目标不相邻",
  LOADED: "非空载，不能取货",
  NOT_CARRYING: "空载，无物可交",
  NOT_ENOUGH_ENERGY: "电量不足",
  TARGET_FULL: "目标已满",
  BOX_NOT_FOUND: "货物不存在",
  WRONG_GOODS: "货物类型不符",
  INVALID_TARGET: "目标不可交互",
  OUT_OF_BOUNDS: "越界",
  CELL_BLOCKED: "目标格静态阻挡",
  CELL_OCCUPIED: "目标格被占",
  NO_SUCH_OBJECT: "对象不存在",
  NO_PATH: "无可行路径",
  CELL_CONTESTED: "落点争抢失败",
  TARGET_CONTESTED: "资源争抢失败",
  CHAIN_BLOCKED: "移动链受阻（含迎面对穿）",
  CHARGER_BUSY: "充电桩本 tick 被占",
  TARGET_MOVED: "目标已移走",
  TARGET_GONE: "目标已被销毁",
  NO_FUNDS: "金币不足",
  CREDIT_EXCEEDED: "超出信用额度",
  ON_VEHICLE: "货物仍在车上",
  NO_FREE_DOCK: "无空闲装卸位",
  NOT_EMPTY: "非空，不可销毁",
  HAS_VEHICLE: "有车辆停靠",
  ORDER_GONE: "订单已不在市场",
  GOODS_MOVED: "货物已转移",
  INIT_PHASE: "初始化阶段禁用",
};

export function codeLabel(code: string): string {
  return CODE_LABELS[code] ?? code;
}

export function isOkCode(code: string): boolean {
  return code === "OK";
}
