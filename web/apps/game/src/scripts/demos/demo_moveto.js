// M4 寻路 API 示例：r.move_to(target) 复合移动（寻路 + 自动提交一步）与
// Game.find_path 静态路径查询。适用于「B2 · 单机闭环」地图：机器人绕开
// 货架与地面货物，在四个巡逻点之间循环。
//
// 关键语义：
// - move_to 返回受理码：OK（已提交一步）/ ARRIVED（已到场，不占行动
//   机会，当 tick 可继续取放或充电）/ NO_PATH（不可达）/ ALREADY_ACTED。
// - 到达判定默认 range 1：走到目标相邻格即算到场（一切交互都按相邻）。
// - 路径缓存在 r.memory._move，跨代码热重载保留；改代码重载后照常续走。
// - 低电滞回是接入取放逻辑后的扩展位：巡逻只移动不耗电（docs 03），
//   能量不会下降；给你的机器人加上 take / give 等耗电动作后，这段
//   回桩逻辑就会自然接管。

const PATROL = [
  [3, 2],
  [8, 2],
  [8, 6],
  [3, 6],
];

export function loop() {
  const r = Game.robots()[0];
  if (!r) return;

  // 防御初始化：中途从其他脚本切换过来（tick > 0、memory 无键）也能起步。
  if (Game.memory["leg"] === undefined) Game.memory["leg"] = 0;
  if (Game.memory["charging"] === undefined) Game.memory["charging"] = false;

  if (Game.tick === 0) {
    // find_path 是纯查询：返回不含起点的最短路径、[]（已在到达范围）或
    // null（不可达）。注意 JS 的 [] 是 truthy——判不可达必须 === null。
    const p = Game.find_path(r.pos, PATROL[0]);
    Game.log("首段路径长：" + (p === null ? "不可达" : p.length));
  }

  // 低电优先回桩（滞回：低于两成去充，超过九成离开）。
  if (r.energy < r.energy_max * 0.2) Game.memory["charging"] = true;
  if (r.energy > r.energy_max * 0.9) Game.memory["charging"] = false;

  if (Game.memory["charging"]) {
    const c = Game.chargers()[0];
    if (c) {
      const code = r.move_to(c); // 对象目的地：充电桩取 pos
      if (code === Game.E.ARRIVED) r.charge();
      return;
    }
  }

  // 巡逻：到场即换下一段（ARRIVED 不占行动机会，同 tick 立即换目标）。
  const target = PATROL[Game.memory["leg"] % PATROL.length];
  const code = r.move_to(target);
  if (code === Game.E.ARRIVED) {
    Game.memory["leg"] = (Game.memory["leg"] + 1) % PATROL.length;
  } else if (code !== Game.E.OK) {
    Game.log("move_to:", code); // 受理失败也要看得见
  }
}
