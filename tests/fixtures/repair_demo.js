// 崩溃修复示例（docs/architecture/06 对账教学）：初始化与每 tick 以真实
// 订单修复任务表，不重复接单。
Game.memory["tasks"] = Game.memory["tasks"] || {};
function repair() {
  for (const o of Game.my_orders()) {
    const key = String(o.id);
    if (!(key in Game.memory["tasks"])) {
      Game.memory["tasks"][key] = { taken_tick: Game.tick, side: o.side };
      Game.log("repair", key);
    }
  }
}
repair();   // 初始化阶段即对账（查询与 memory 写在 init 允许）

export function loop() {
  repair();
  if (Game.my_orders().length === 0 && Game.market.sell_orders().length > 0) {
    const asks = Game.market.sell_orders();
    let best = asks[0];
    for (const o of asks) if (o.unit_price < best.unit_price) best = o;
    const code = Game.market.take(best.id);
    if (code === Game.E.OK) {
      Game.memory["tasks"][String(best.id)] = { taken_tick: Game.tick, side: best.side };
    }
    Game.log("take", best.id, code);
  }
}
