// 全闭环示例（B1）：低买 → 卸车 → 空档补电 → 高卖交付。
// 世界布置见 b1_cargo_loop.rs：机器人 (1,1)，装卸位锚点 (0,3)、交互格
// (0,4)，充电桩 (1,5)，卖单 battery 1@4.000、买单 battery 1@6.000。毛利 = 2 金币。
let phase = "go-port"; // go-port → unload → charge → deliver
let sellId = null;
let buyId = null;

export function loop() {
  const r = Game.robots()[0];

  if (phase === "go-port") {
    if (sellId === null) {
      const asks = Game.market.sell_orders();
      if (asks.length > 0) {
        sellId = asks[0].id;
        Game.log("take-sell", Game.market.take(sellId));
      }
      return;
    }
    if (r.pos.y < 4) {
      const c = r.move(Game.SOUTH);
      if (c !== Game.E.OK) Game.log("mv", c);
      return;
    }
    phase = "unload";
    return;
  }

  if (phase === "unload") {
    const v = Game.vehicles("in")[0];
    if (v) {
      Game.log("take-box", r.take(v, v.boxes[0].id));
      // 取走即清空：同 tick 结算内车辆离场、装卸位释放。
    } else {
      phase = "charge"; // 车已离场，转入卖出阶段
    }
    return;
  }

  if (phase === "charge") {
    if (buyId === null) {
      const bids = Game.market.buy_orders();
      if (bids.length > 0) {
        buyId = bids[0].id;
        Game.log("take-buy", Game.market.take(buyId));
        return;
      }
    }
    // 等出库车到场的空档补一格电。
    const c = r.charge();
    if (c !== Game.E.OK) Game.log("charge", c);
    if (Game.vehicles("out").length > 0) phase = "deliver";
    return;
  }

  if (phase === "deliver") {
    const v = Game.vehicles("out")[0];
    if (v) Game.log("give-box", r.give(v));
  }
}
