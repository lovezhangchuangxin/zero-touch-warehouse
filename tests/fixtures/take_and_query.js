// 管理操作当 tick 可见性验证：take 后同 tick 查询 my_orders 与 gold
//（增量回放路径），并确认挂单同步消失。
export function loop() {
  const before = Game.my_orders().length;
  const asks = Game.market.sell_orders();
  if (asks.length > 0 && before === 0) {
    const id = asks[0].id;
    const code = Game.market.take(id);
    const after = Game.my_orders().length;
    const listings = Game.market.sell_orders().length + Game.market.buy_orders().length;
    Game.log("take", code, "before", before, "after", after,
             "listings", listings, "gold", Game.gold);
  }
}
