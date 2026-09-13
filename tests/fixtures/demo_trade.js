// 交易示例：有在途订单时不再接单（幂等防重，崩溃修复演示的基础）。
function loop() {
  if (Game.my_orders().length > 0) return;
  const asks = Game.market.sell_orders();
  if (asks.length > 0) {
    let best = asks[0];
    for (const o of asks) {
      if (o.unit_price < best.unit_price) best = o;
    }
    const code = Game.market.take(best.id);
    Game.log("take", best.id, code);
  }
}
